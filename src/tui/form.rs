//! The edit / new screen: a form over a single document. Frontmatter fields are
//! edited inline (title, status, tags, and the type's declared custom fields,
//! each by its `FieldSpec` kind); the markdown body uses the inline
//! [`TextArea`]. Relations (references/blocked_by/blocks) are shown read-only —
//! they are managed by sync and `block`. Saving goes through
//! [`super::save::save_edited_doc`].

use std::time::SystemTime;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_norway::Value;

use crate::body;
use crate::config::FieldType;
use crate::doc::Doc;
use crate::project::Project;

use super::textarea::TextArea;

/// What the form asks the app to do after a key.
pub enum FormAction {
    None,
    Save,
    Cancel,
    /// Reload the form's document from disk, discarding local edits (the
    /// conflict-prompt "reload" choice). The app supplies the project.
    Reload,
}

/// An editor for one declared custom field, by its `FieldType`.
enum CustomEditor {
    Enum {
        values: Vec<String>,
        /// `None` means unset.
        idx: Option<usize>,
    },
    Bool(bool),
    /// int / list / string all use a single-line text editor.
    Text(TextArea),
}

struct CustomField {
    key: String,
    field_type: FieldType,
    editor: CustomEditor,
}

/// Which widget currently has focus.
enum Focus {
    Title,
    Status,
    Tags,
    Custom(usize),
    Body,
}

/// A pending conflict: the file changed on disk while the form had unsaved edits.
pub struct Conflict;

pub struct EditForm {
    /// The working document — kept so relations and untouched frontmatter keys
    /// survive a save. Editors are written back into it on save.
    doc: Doc,
    pub is_new: bool,
    pub dirty: bool,
    loaded_mtime: Option<SystemTime>,
    pub conflict: Option<Conflict>,
    pub message: Option<String>,

    title: TextArea,
    statuses: Vec<String>,
    status_idx: usize,
    tags: TextArea,
    custom: Vec<CustomField>,
    body: TextArea,

    order: Vec<Focus>,
    focus: usize,
}

impl EditForm {
    pub fn new(prj: &Project, doc: Doc, is_new: bool) -> EditForm {
        let id = doc.id().unwrap_or("").to_string();
        let tname = prj.pcfg.type_name_for_id(&id).unwrap_or("");
        let t = prj.pcfg.types.get(tname);

        let (title_text, body_rest) = split_title(&doc.body);

        // Selectable statuses: the type's non-terminal ones (terminal is reached
        // only via close).
        let statuses: Vec<String> = t
            .map(|t| {
                t.statuses
                    .iter()
                    .filter(|s| !t.terminal_statuses.contains(s))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let status_idx = doc
            .status()
            .and_then(|s| statuses.iter().position(|x| x == s))
            .unwrap_or(0);

        let tags = doc.frontmatter.tags().unwrap_or_default().join(", ");

        let custom: Vec<CustomField> = t
            .map(|t| {
                t.fields
                    .iter()
                    .map(|(key, spec)| CustomField {
                        key: key.clone(),
                        field_type: spec.field_type,
                        editor: build_editor(
                            spec.field_type,
                            &spec.values,
                            doc.frontmatter.get(key),
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut order = vec![Focus::Title, Focus::Status, Focus::Tags];
        for i in 0..custom.len() {
            order.push(Focus::Custom(i));
        }
        order.push(Focus::Body);

        let loaded_mtime = file_mtime(&doc.path);

        EditForm {
            doc,
            is_new,
            dirty: false,
            loaded_mtime,
            conflict: None,
            message: None,
            title: TextArea::new(&title_text, false),
            statuses,
            status_idx,
            tags: TextArea::new(&tags, false),
            custom,
            body: TextArea::new(&body_rest, true),
            order,
            focus: 0,
        }
    }

    pub fn title_id(&self) -> String {
        self.doc.id().unwrap_or("(new)").to_string()
    }

    fn focus_next(&mut self) {
        self.focus = (self.focus + 1) % self.order.len();
    }

    fn focus_prev(&mut self) {
        self.focus = (self.focus + self.order.len() - 1) % self.order.len();
    }

    /// Handle a key, returning what the app should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        // A pending conflict captures input until resolved.
        if self.conflict.is_some() {
            match key.code {
                KeyCode::Char('r') | KeyCode::Char('R') => return FormAction::Reload,
                KeyCode::Char('i') | KeyCode::Char('I') => {
                    self.conflict = None;
                    self.message = Some("kept your edits; save will overwrite".into());
                }
                _ => {}
            }
            return FormAction::None;
        }

        // Global form keys.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return FormAction::Save;
        }
        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Tab => {
                self.focus_next();
                return FormAction::None;
            }
            KeyCode::BackTab => {
                self.focus_prev();
                return FormAction::None;
            }
            _ => {}
        }

        match self.order[self.focus] {
            Focus::Title => self.edit_line_field(key, FieldRef::Title),
            Focus::Tags => self.edit_line_field(key, FieldRef::Tags),
            Focus::Body => {
                if self.body.handle_key(key) {
                    self.dirty = true;
                }
            }
            Focus::Status => self.cycle_status(key),
            Focus::Custom(i) => self.edit_custom(i, key),
        }
        FormAction::None
    }

    fn edit_line_field(&mut self, key: KeyEvent, which: FieldRef) {
        // Enter on a single-line field advances focus instead of inserting.
        if key.code == KeyCode::Enter {
            self.focus_next();
            return;
        }
        let ta = match which {
            FieldRef::Title => &mut self.title,
            FieldRef::Tags => &mut self.tags,
        };
        if ta.handle_key(key) {
            self.dirty = true;
        }
    }

    fn cycle_status(&mut self, key: KeyEvent) {
        if self.statuses.is_empty() {
            return;
        }
        let step: i32 = match key.code {
            KeyCode::Left => -1,
            KeyCode::Right | KeyCode::Char(' ') => 1,
            _ => return,
        };
        let len = self.statuses.len() as i32;
        self.status_idx = (self.status_idx as i32 + step).rem_euclid(len) as usize;
        self.dirty = true;
    }

    fn edit_custom(&mut self, i: usize, key: KeyEvent) {
        match &mut self.custom[i].editor {
            CustomEditor::Enum { values, idx } => {
                if values.is_empty() {
                    return;
                }
                // Index space includes "unset" at 0.
                let step: i32 = match key.code {
                    KeyCode::Left => -1,
                    KeyCode::Right | KeyCode::Char(' ') => 1,
                    _ => return,
                };
                let span = values.len() as i32 + 1;
                let cur = idx.map(|v| v as i32 + 1).unwrap_or(0);
                let next = (cur + step).rem_euclid(span);
                *idx = if next == 0 {
                    None
                } else {
                    Some((next - 1) as usize)
                };
                self.dirty = true;
            }
            CustomEditor::Bool(b) => {
                if matches!(
                    key.code,
                    KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
                ) {
                    *b = !*b;
                    self.dirty = true;
                }
            }
            CustomEditor::Text(ta) => {
                if ta.handle_key(key) {
                    self.dirty = true;
                }
            }
        }
    }

    /// Write the editors back into `doc`, returning a mutable borrow for saving.
    /// Returns an error message when a field is malformed (e.g. a non-integer in
    /// an int field, or an empty title).
    pub fn apply(&mut self) -> Result<&mut Doc, String> {
        let title = self.title.text();
        if title.trim().is_empty() {
            return Err("title is required (the # heading)".into());
        }
        let rest = self.body.text();
        self.doc.body = if rest.is_empty() {
            format!("# {title}\n")
        } else {
            format!("# {title}\n{rest}")
        };

        // Status.
        if let Some(s) = self.statuses.get(self.status_idx) {
            self.doc.frontmatter.set_str("status", s);
        }

        // Tags.
        let tags: Vec<String> = self
            .tags
            .text()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if tags.is_empty() {
            self.doc.frontmatter.remove("tags");
        } else {
            self.doc.frontmatter.set_tags(&tags);
        }

        // Custom fields.
        for cf in &self.custom {
            match &cf.editor {
                CustomEditor::Enum { values, idx } => match idx {
                    Some(i) => self.doc.frontmatter.set_str(&cf.key, values[*i].clone()),
                    None => {
                        self.doc.frontmatter.remove(&cf.key);
                    }
                },
                CustomEditor::Bool(b) => {
                    self.doc.frontmatter.insert(&cf.key, Value::Bool(*b));
                }
                CustomEditor::Text(ta) => {
                    let raw = ta.text();
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        self.doc.frontmatter.remove(&cf.key);
                        continue;
                    }
                    match cf.field_type {
                        FieldType::Int => match trimmed.parse::<i64>() {
                            Ok(n) => self
                                .doc
                                .frontmatter
                                .insert(&cf.key, Value::Number(n.into())),
                            Err(_) => return Err(format!("field '{}' must be an integer", cf.key)),
                        },
                        FieldType::List => {
                            let items: Vec<Value> = trimmed
                                .split(',')
                                .map(|t| t.trim())
                                .filter(|t| !t.is_empty())
                                .map(|t| Value::String(t.to_string()))
                                .collect();
                            self.doc.frontmatter.insert(&cf.key, Value::Sequence(items));
                        }
                        _ => self.doc.frontmatter.set_str(&cf.key, trimmed),
                    }
                }
            }
        }

        // Keep the cached title in sync (save_edited_doc also recomputes it).
        self.doc.title = body::title(&self.doc.body);
        Ok(&mut self.doc)
    }

    /// After a successful save: clear dirty and refresh the recorded mtime.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.is_new = false;
        self.loaded_mtime = file_mtime(&self.doc.path);
        self.message = None;
    }

    /// React to an external change while the form is open. When the file changed
    /// and there are no local edits, silently reload from disk; when there are,
    /// raise a conflict prompt. Returns whether the form replaced its contents
    /// (so the app can re-render).
    pub fn on_external_change(&mut self, prj: &Project) {
        if self.is_new {
            return;
        }
        let current = file_mtime(&self.doc.path);
        if current == self.loaded_mtime {
            return;
        }
        if self.dirty {
            self.conflict = Some(Conflict);
            return;
        }
        // No local edits → reload silently.
        self.reload_from_disk(prj);
    }

    /// Replace the form's contents with the document as it is on disk,
    /// discarding local edits. Used by the silent reload and the conflict
    /// prompt's "reload" choice.
    pub fn reload_from_disk(&mut self, prj: &Project) {
        if let Ok(text) = std::fs::read_to_string(&self.doc.path) {
            if let Ok(doc) = Doc::parse(self.doc.path.clone(), &text) {
                *self = EditForm::new(prj, doc, false);
            }
        }
    }

    // --- accessors for the view ---

    pub fn fields(&self) -> Vec<FieldView> {
        let mut out = Vec::new();
        for (i, f) in self.order.iter().enumerate() {
            let focused = i == self.focus;
            match f {
                Focus::Title => out.push(FieldView::Line {
                    focused,
                    label: "title",
                    text: self.title.text(),
                }),
                Focus::Status => out.push(FieldView::Choice {
                    focused,
                    label: "status",
                    value: self
                        .statuses
                        .get(self.status_idx)
                        .cloned()
                        .unwrap_or_else(|| "(none)".into()),
                }),
                Focus::Tags => out.push(FieldView::Line {
                    focused,
                    label: "tags",
                    text: self.tags.text(),
                }),
                Focus::Custom(ci) => out.push(self.custom[*ci].view(focused)),
                Focus::Body => out.push(FieldView::Body { focused }),
            }
        }
        out
    }

    /// Render the body editor (the only multi-line widget) into `area`.
    pub fn render_body(&mut self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let focused = matches!(self.order[self.focus], Focus::Body);
        self.body.render(frame, area, focused, "body");
    }

    pub fn relations_summary(&self) -> Vec<(String, Vec<(String, String)>)> {
        use crate::refs;
        refs::RELATION_FIELDS
            .iter()
            .map(|f| (f.to_string(), refs::parse_in(&self.doc.frontmatter, f)))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

enum FieldRef {
    Title,
    Tags,
}

/// A view-model for one form field (for rendering).
pub enum FieldView {
    Line {
        focused: bool,
        label: &'static str,
        text: String,
    },
    Choice {
        focused: bool,
        label: &'static str,
        value: String,
    },
    Custom {
        focused: bool,
        label: String,
        value: String,
    },
    Body {
        focused: bool,
    },
}

impl CustomField {
    fn view(&self, focused: bool) -> FieldView {
        let value = match &self.editor {
            CustomEditor::Enum { values, idx } => match idx {
                Some(i) => values[*i].clone(),
                None => "(unset)".into(),
            },
            CustomEditor::Bool(b) => b.to_string(),
            CustomEditor::Text(ta) => {
                let t = ta.text();
                if t.is_empty() {
                    "(empty)".into()
                } else {
                    t
                }
            }
        };
        FieldView::Custom {
            focused,
            label: self.key.clone(),
            value,
        }
    }
}

fn build_editor(field_type: FieldType, values: &[String], current: Option<&Value>) -> CustomEditor {
    match field_type {
        FieldType::Enum => {
            let idx = current
                .and_then(Value::as_str)
                .and_then(|s| values.iter().position(|v| v == s));
            CustomEditor::Enum {
                values: values.to_vec(),
                idx,
            }
        }
        FieldType::Bool => CustomEditor::Bool(current.and_then(Value::as_bool).unwrap_or(false)),
        FieldType::Int => {
            let text = current
                .and_then(Value::as_i64)
                .map(|n| n.to_string())
                .unwrap_or_default();
            CustomEditor::Text(TextArea::new(&text, false))
        }
        FieldType::List => {
            let text = current
                .and_then(Value::as_sequence)
                .map(|seq| {
                    seq.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            CustomEditor::Text(TextArea::new(&text, false))
        }
        FieldType::String => {
            let text = current.and_then(Value::as_str).unwrap_or("").to_string();
            CustomEditor::Text(TextArea::new(&text, false))
        }
    }
}

/// Split a body into (title, remainder), removing the first `# ` heading line.
fn split_title(body: &str) -> (String, String) {
    let mut title = String::new();
    let mut rest: Vec<&str> = Vec::new();
    let mut taken = false;
    for line in body.split('\n') {
        if !taken && line.starts_with("# ") {
            title = line[2..].trim().to_string();
            taken = true;
        } else {
            rest.push(line);
        }
    }
    (title, rest.join("\n"))
}

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::split_title;

    #[test]
    fn splits_and_rejoins_title() {
        let (title, rest) = split_title("# Dark mode\n\n## Test plan\n- [ ] x\n");
        assert_eq!(title, "Dark mode");
        assert_eq!(rest, "\n## Test plan\n- [ ] x\n");
        // Reconstruct the way `apply` does.
        let body = format!("# {title}\n{rest}");
        assert_eq!(body, "# Dark mode\n\n## Test plan\n- [ ] x\n");
    }

    #[test]
    fn missing_title_yields_empty() {
        let (title, rest) = split_title("no heading here\nmore\n");
        assert_eq!(title, "");
        assert_eq!(rest, "no heading here\nmore\n");
    }
}
