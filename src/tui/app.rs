//! The TUI application state (the TEA model) and the input reducer.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::new::scaffold_body;
use crate::doc::Doc;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::Ctx;

use super::data::Board;
use super::filter::{self, FilterField, FilterState};
use super::form::{EditForm, FormAction};
use super::save;
use super::sort::{sort_docs, SortKey, SortState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewLayout {
    Right,
    Bottom,
    Hidden,
}

/// The active screen / input mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Filter,
    Stats,
    /// Picking a type for a new document.
    NewType,
    /// Editing (or creating) a document.
    Edit,
}

pub struct App {
    pub prj: Project,
    pub board: Board,
    /// Indices into `board.docs` that pass the filter, in sort order. The
    /// selection and rendering operate on this view.
    pub visible: Vec<usize>,
    pub selected: usize,
    pub preview: PreviewLayout,
    pub sort: SortState,
    pub mode: Mode,
    pub filter: FilterState,
    pub filter_focus: FilterField,
    /// The open edit/new form, when `mode == Edit`.
    pub edit: Option<EditForm>,
    /// Type names offered by the new-document picker, and the cursor into them.
    pub new_types: Vec<String>,
    pub new_type_idx: usize,
    /// A pending close confirmation (the document id awaiting `y`).
    pub confirm_close: Option<String>,
    pub status: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(ctx: &Ctx) -> Result<App> {
        let prj = ctx.open()?;
        let sort = SortState::default();
        let board = Board::load(&prj, sort);
        let mut app = App {
            prj,
            board,
            visible: Vec::new(),
            selected: 0,
            preview: PreviewLayout::Right,
            sort,
            mode: Mode::Browse,
            filter: FilterState::default(),
            filter_focus: FilterField::Type,
            edit: None,
            new_types: Vec::new(),
            new_type_idx: 0,
            confirm_close: None,
            status: None,
            should_quit: false,
        };
        app.recompute_visible(None);
        app.refresh_status();
        Ok(app)
    }

    pub fn selected_doc(&self) -> Option<&Doc> {
        self.visible
            .get(self.selected)
            .and_then(|&i| self.board.docs.get(i))
    }

    /// The documents currently visible (filtered + sorted), for the stats screen.
    pub fn visible_docs(&self) -> Vec<&Doc> {
        self.visible.iter().map(|&i| &self.board.docs[i]).collect()
    }

    fn selected_id(&self) -> Option<String> {
        self.selected_doc().and_then(|d| d.id()).map(str::to_string)
    }

    /// Rebuild `visible` from `board.docs` + the filter, restoring the selection
    /// onto `keep_id` when it is still visible, else clamping into range.
    fn recompute_visible(&mut self, keep_id: Option<&str>) {
        self.visible = self
            .board
            .docs
            .iter()
            .enumerate()
            .filter(|(_, d)| self.filter.matches(&self.prj, d))
            .map(|(i, _)| i)
            .collect();

        if let Some(id) = keep_id {
            if let Some(pos) = self
                .visible
                .iter()
                .position(|&i| self.board.docs[i].id() == Some(id))
            {
                self.selected = pos;
                return;
            }
        }
        if self.selected >= self.visible.len() {
            self.selected = self.visible.len().saturating_sub(1);
        }
    }

    /// Reload the board from disk, preserving selection and the active filter.
    /// While a form is open, also reconcile it with the external change
    /// (silent reload when clean, conflict prompt when dirty).
    pub fn reload(&mut self) {
        let keep = self.selected_id();
        self.board.reload(&self.prj, self.sort);
        self.recompute_visible(keep.as_deref());
        self.refresh_status();
        if self.mode == Mode::Edit {
            if let Some(form) = self.edit.as_mut() {
                form.on_external_change(&self.prj);
            }
        }
    }

    fn resort(&mut self) {
        let keep = self.selected_id();
        sort_docs(&mut self.board.docs, self.sort);
        self.recompute_visible(keep.as_deref());
    }

    fn refilter(&mut self) {
        let keep = self.selected_id();
        self.recompute_visible(keep.as_deref());
    }

    fn refresh_status(&mut self) {
        self.status = match self.board.errors.len() {
            0 => None,
            n => Some(format!(
                "{n} document(s) failed to parse — run `opys verify`"
            )),
        };
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.visible.len() {
            self.selected += 1;
        }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn set_sort(&mut self, key: SortKey) {
        if self.sort.key == key {
            self.sort.desc = !self.sort.desc;
        } else {
            self.sort = SortState { key, desc: true };
        }
        self.resort();
    }

    fn toggle_preview(&mut self) {
        self.preview = match self.preview {
            PreviewLayout::Right => PreviewLayout::Bottom,
            PreviewLayout::Bottom => PreviewLayout::Hidden,
            PreviewLayout::Hidden => PreviewLayout::Right,
        };
    }

    /// Map a key press to a state change, dispatched by the current mode.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let key = normalize_key(key);
        match self.mode {
            Mode::Browse => self.handle_browse(key),
            Mode::Filter => self.handle_filter(key),
            Mode::Stats => self.handle_stats(key),
            Mode::NewType => self.handle_new_type(key),
            Mode::Edit => self.handle_edit(key),
        }
    }

    fn handle_browse(&mut self, key: KeyEvent) {
        // A pending close confirmation captures y/n first.
        if let Some(id) = self.confirm_close.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_close = None;
                    self.do_close(&id);
                }
                _ => {
                    self.confirm_close = None;
                    self.status = Some("close cancelled".into());
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.visible.len().saturating_sub(1)
            }
            KeyCode::Char('p') => self.toggle_preview(),
            KeyCode::Char('u') => self.set_sort(SortKey::Updated),
            KeyCode::Char('c') => self.set_sort(SortKey::Created),
            KeyCode::Char('s') => self.set_sort(SortKey::Status),
            KeyCode::Char('t') => self.set_sort(SortKey::Title),
            KeyCode::Char('i') => self.set_sort(SortKey::Id),
            KeyCode::Char('f') | KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('x') => {
                self.filter.clear();
                self.refilter();
            }
            KeyCode::Char('S') => self.mode = Mode::Stats,
            KeyCode::Char('e') | KeyCode::Enter => self.start_edit(),
            KeyCode::Char('n') => self.start_new(),
            KeyCode::Char('D') => self.request_close(),
            _ => {}
        }
    }

    fn handle_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = Mode::Browse,
            KeyCode::Tab | KeyCode::Down => self.cycle_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.cycle_focus(-1),
            KeyCode::Left => self.cycle_value(-1),
            KeyCode::Right => self.cycle_value(1),
            KeyCode::Backspace => {
                if let Some(text) = self.focused_text_mut() {
                    text.pop();
                    self.refilter();
                }
            }
            KeyCode::Char(c) if !is_control_combo(&key) => {
                if let Some(text) = self.focused_text_mut() {
                    text.push(c);
                    self.refilter();
                }
            }
            _ => {}
        }
    }

    fn handle_stats(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc | KeyCode::Char('S') => self.mode = Mode::Browse,
            _ => {}
        }
    }

    fn cycle_focus(&mut self, step: i32) {
        let fields = FilterField::ALL;
        let cur = fields
            .iter()
            .position(|f| *f == self.filter_focus)
            .unwrap_or(0) as i32;
        let next = (cur + step).rem_euclid(fields.len() as i32) as usize;
        self.filter_focus = fields[next];
    }

    /// A mutable handle to the free-text field under focus (tag or query), or
    /// `None` when a cyclable field (type/status) is focused.
    fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.filter_focus {
            FilterField::Tag => Some(self.filter.tag.get_or_insert_with(String::new)),
            FilterField::Query => Some(&mut self.filter.query),
            _ => None,
        }
    }

    fn cycle_value(&mut self, step: i32) {
        match self.filter_focus {
            FilterField::Type => {
                let opts = filter::type_options(&self.prj);
                self.filter.doc_type = filter::cycle(&self.filter.doc_type, &opts, step);
                // A narrowed type may make the current status invalid; drop it.
                let statuses = filter::status_options(&self.prj, self.filter.doc_type.as_deref());
                if let Some(s) = &self.filter.status {
                    if !statuses.contains(s) {
                        self.filter.status = None;
                    }
                }
            }
            FilterField::Status => {
                let opts = filter::status_options(&self.prj, self.filter.doc_type.as_deref());
                self.filter.status = filter::cycle(&self.filter.status, &opts, step);
            }
            _ => {}
        }
        self.refilter();
    }

    // --- edit / new / close ---

    fn start_edit(&mut self) {
        if let Some(doc) = self.selected_doc().cloned() {
            self.edit = Some(EditForm::new(&self.prj, doc, false));
            self.mode = Mode::Edit;
            self.status = None;
        }
    }

    fn start_new(&mut self) {
        self.new_types = self.prj.pcfg.types.keys().cloned().collect();
        if self.new_types.is_empty() {
            self.status = Some("no document types defined".into());
            return;
        }
        self.new_type_idx = 0;
        if self.new_types.len() == 1 {
            // Only one type — skip the picker.
            self.scaffold_new(0);
        } else {
            self.mode = Mode::NewType;
        }
    }

    fn handle_new_type(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Browse,
            KeyCode::Up | KeyCode::Char('k') => {
                self.new_type_idx = self.new_type_idx.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') if self.new_type_idx + 1 < self.new_types.len() => {
                self.new_type_idx += 1;
            }
            KeyCode::Enter => self.scaffold_new(self.new_type_idx),
            _ => {}
        }
    }

    /// Scaffold a fresh in-memory document of the chosen type and open it in the
    /// edit form. It is not written until the form is saved.
    fn scaffold_new(&mut self, type_idx: usize) {
        let Some(tname) = self.new_types.get(type_idx).cloned() else {
            return;
        };
        let t = &self.prj.pcfg.types[&tname];
        let id = self.prj.next_id_for(&t.prefix, &self.board.docs);
        let status = t.default_status.clone();
        let body = scaffold_body("", t);
        let mut fm = Frontmatter::new();
        fm.set_str("id", &id);
        fm.set_str("status", &status);
        let path = self.prj.doc_path(&id, &status);
        let doc = Doc {
            path,
            frontmatter: fm,
            body,
            title: String::new(),
        };
        self.edit = Some(EditForm::new(&self.prj, doc, true));
        self.mode = Mode::Edit;
        self.status = None;
    }

    fn handle_edit(&mut self, key: KeyEvent) {
        let action = match self.edit.as_mut() {
            Some(form) => form.handle_key(key),
            None => {
                self.mode = Mode::Browse;
                return;
            }
        };
        match action {
            FormAction::None => {}
            FormAction::Cancel => {
                self.edit = None;
                self.mode = Mode::Browse;
            }
            FormAction::Reload => {
                if let Some(form) = self.edit.as_mut() {
                    form.reload_from_disk(&self.prj);
                }
            }
            FormAction::Save => self.save_form(),
        }
    }

    fn save_form(&mut self) {
        // Borrow the disjoint fields directly so `apply` (→ &mut Doc) and `&prj`
        // can be held together.
        let result = {
            let Some(form) = self.edit.as_mut() else {
                return;
            };
            match form.apply() {
                Err(msg) => Err(msg),
                Ok(doc) => save::save_edited_doc(&self.prj, doc).map_err(|e| e.to_string()),
            }
        };
        match result {
            Ok(()) => {
                let id = self.edit.as_ref().map(|f| f.title_id()).unwrap_or_default();
                if let Some(form) = self.edit.as_mut() {
                    form.mark_saved();
                }
                // Persist the relation/linkify/relocate pass, then refresh.
                let _ = crate::commands::sync::run(&self.prj);
                self.edit = None;
                self.mode = Mode::Browse;
                self.reload();
                self.select_id(&id);
                self.status = Some(format!("saved {id}"));
            }
            Err(msg) => self.status = Some(format!("not saved: {msg}")),
        }
    }

    fn select_id(&mut self, id: &str) {
        if let Some(pos) = self
            .visible
            .iter()
            .position(|&i| self.board.docs[i].id() == Some(id))
        {
            self.selected = pos;
        }
    }

    fn request_close(&mut self) {
        let Some(doc) = self.selected_doc() else {
            return;
        };
        let Some(id) = doc.id() else { return };
        let tname = self.prj.pcfg.type_name_for_id(id);
        let closable = tname
            .and_then(|n| self.prj.pcfg.types.get(n))
            .is_some_and(|t| !t.terminal_statuses.is_empty());
        if !closable {
            self.status = Some(format!("{id}: type has no terminal status — cannot close"));
            return;
        }
        self.confirm_close = Some(id.to_string());
    }

    fn do_close(&mut self, id: &str) {
        match crate::commands::close::core(&self.prj, id, false) {
            Ok(()) => {
                let _ = crate::commands::sync::run(&self.prj);
                self.reload();
                self.status = Some(format!("closed {id}"));
            }
            Err(e) => self.status = Some(format!("close failed: {e}")),
        }
    }
}

/// True when a `Char` event carries Ctrl/Alt — a control combo, not text input.
fn is_control_combo(key: &KeyEvent) -> bool {
    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Normalize quirky terminal key reports. Some terminals send Backspace as
/// Ctrl-H (ASCII 0x08), which crossterm surfaces as Ctrl+'h' — treat it as
/// Backspace so it deletes instead of typing an 'h'.
fn normalize_key(key: KeyEvent) -> KeyEvent {
    if key.code == KeyCode::Char('h') && key.modifiers == KeyModifiers::CONTROL {
        return KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::{is_control_combo, normalize_key};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn ctrl_h_normalizes_to_backspace() {
        let k = normalize_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(k.code, KeyCode::Backspace);
    }

    #[test]
    fn plain_h_is_unchanged() {
        let k = normalize_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert_eq!(k.code, KeyCode::Char('h'));
        assert!(!is_control_combo(&k));
    }
}
