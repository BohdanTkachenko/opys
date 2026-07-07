//! The TUI application state (the TEA model) and the input reducer.
//!
//! The board is **read-mostly**: it filters, sorts, and previews the inventory,
//! and the only writes it performs go through the real command cores —
//! `set_status::core` (status picker) and `close::core` (`D`) — so on-disk
//! invariants hold exactly as in the CLI. Body edits are delegated to `$EDITOR`
//! (`e`/Enter): the loop suspends the TUI, runs the editor on the document file,
//! then runs the sync/verify pass on return.
use std::path::PathBuf;

use opys_engine::backend::Backend;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use opys_engine::doc::Doc;
use opys_engine::error::Result;
use opys_engine::project::Project;
use opys_engine::Ctx;

use super::data::Board;
use super::filter::{self, FilterField, FilterState};
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
    /// Picking a new status for the selected document.
    Status,
}

/// A pending status change: the target document and the choices offered.
pub struct StatusPick {
    pub id: String,
    pub options: Vec<String>,
    pub idx: usize,
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
    /// The open status picker, when `mode == Status`.
    pub status_pick: Option<StatusPick>,
    /// A pending close confirmation (the document id awaiting `y`).
    pub confirm_close: Option<String>,
    /// A document file the loop should open in `$EDITOR` before the next draw.
    pub pending_editor: Option<PathBuf>,
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
            status_pick: None,
            confirm_close: None,
            pending_editor: None,
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
    pub fn reload(&mut self) {
        let keep = self.selected_id();
        self.board.reload(&self.prj, self.sort);
        self.recompute_visible(keep.as_deref());
        self.refresh_status();
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
            Mode::Status => self.handle_status(key),
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
            KeyCode::Char('e') | KeyCode::Enter => self.request_editor(),
            KeyCode::Char(' ') => self.start_status_pick(),
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

    // --- edit ($EDITOR) / status / close ---

    /// Ask the loop to open the selected document in `$EDITOR`. The loop
    /// suspends the TUI, runs the editor, and calls [`App::after_external_edit`].
    fn request_editor(&mut self) {
        if let Some(doc) = self.selected_doc() {
            self.pending_editor = Some(doc.path.clone());
        }
    }

    /// Called by the loop after `$EDITOR` returns: run the auto-sync pass
    /// (reconcile + linkify + relocate), then reload and surface any problems
    /// `verify` reports so a bad hand-edit is visible rather than silent.
    pub fn after_external_edit(&mut self) {
        let backend = opys_backend_markdown_local::MarkdownLocal;
        let sync = opys_engine::commands::sync::run(&self.prj, &backend);
        self.reload();
        // If parsing already failed, `refresh_status` set that message — keep it.
        if self.status.is_some() {
            return;
        }
        self.status = match sync {
            Err(e) => Some(format!("sync failed: {e}")),
            Ok(_) => {
                let (docs, parse_errors) = backend.load_docs(&self.prj);
                let problems =
                    opys_engine::commands::verify::collect_problems(&self.prj, &docs, parse_errors);
                match problems.len() {
                    0 => Some("saved".into()),
                    n => Some(format!("{n} problem(s) — run `opys verify`")),
                }
            }
        };
    }

    /// Open the status picker for the selected document, offering the type's
    /// non-terminal statuses (terminal ones are reached only via `close`).
    fn start_status_pick(&mut self) {
        let Some(doc) = self.selected_doc() else {
            return;
        };
        let Some(id) = doc.id() else { return };
        let id = id.to_string();
        let Some(tname) = self.prj.pcfg.type_name_for_id(&id) else {
            self.status = Some(format!("{id}: unrecognized type"));
            return;
        };
        let t = &self.prj.pcfg.types[tname];
        let cur = doc.status();
        let options: Vec<String> = t
            .statuses
            .iter()
            .filter(|s| !t.terminal_statuses.iter().any(|term| term == *s))
            .cloned()
            .collect();
        if options.is_empty() {
            self.status = Some(format!("{id}: type has no settable status"));
            return;
        }
        let idx = cur
            .and_then(|c| options.iter().position(|s| s == c))
            .unwrap_or(0);
        self.status_pick = Some(StatusPick { id, options, idx });
        self.mode = Mode::Status;
        self.status = None;
    }

    fn handle_status(&mut self, key: KeyEvent) {
        let Some(pick) = self.status_pick.as_mut() else {
            self.mode = Mode::Browse;
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.status_pick = None;
                self.mode = Mode::Browse;
            }
            KeyCode::Up | KeyCode::Char('k') => pick.idx = pick.idx.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if pick.idx + 1 < pick.options.len() => {
                pick.idx += 1;
            }
            KeyCode::Enter => {
                let id = pick.id.clone();
                let status = pick.options[pick.idx].clone();
                self.status_pick = None;
                self.mode = Mode::Browse;
                self.apply_status(&id, &status);
            }
            _ => {}
        }
    }

    /// Apply a status transition through `set_status::core`, then flush + sync.
    /// A rule failure (e.g. a status that requires a reason) is surfaced on the
    /// status line and nothing is written.
    fn apply_status(&mut self, id: &str, status: &str) {
        let backend = opys_backend_markdown_local::MarkdownLocal;
        let result = backend.load(&self.prj).and_then(|(mut store, _)| {
            opys_engine::commands::set_status::core(&self.prj, &mut store, id, status, None)?;
            backend.flush(&self.prj, store)
        });
        match result {
            Ok(()) => {
                let _ = opys_engine::commands::sync::run(&self.prj, &backend);
                self.reload();
                self.status = Some(format!("{id} -> {status}"));
            }
            Err(e) => self.status = Some(format!("not changed: {e}")),
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
        let backend = opys_backend_markdown_local::MarkdownLocal;
        let result = backend.load(&self.prj).and_then(|(mut store, _)| {
            opys_engine::commands::close::core(&self.prj, &mut store, id, false)?;
            backend.flush(&self.prj, store)
        });
        match result {
            Ok(()) => {
                let _ = opys_engine::commands::sync::run(&self.prj, &backend);
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
    use super::{is_control_combo, normalize_key, App, Mode};
    use opys_engine::Ctx;
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

    /// A `task` type with a terminal `done` status and one live doc.
    fn app_with_task() -> App {
        let dir = tempfile::tempdir().unwrap();
        let config = "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"in-progress\", \"done\"]\n\
default_status = \"todo\"\nterminal_statuses = [\"done\"]\ntags_required = false\n";
        std::fs::write(dir.path().join("opys.toml"), config).unwrap();
        std::fs::create_dir_all(dir.path().join("opys")).unwrap();
        std::fs::write(
            dir.path().join("opys/TASK-0001.md"),
            "---\nid: TASK-0001\nstatus: todo\n---\n\n# A task\n",
        )
        .unwrap();
        let ctx = Ctx {
            root: dir.path().to_string_lossy().into_owned(),
            no_sync: true,
            backend: Box::new(opys_backend_markdown_local::MarkdownLocal),
        };
        // Keep the temp dir alive for the App's lifetime by leaking it — the test
        // process is short-lived, so this is fine and keeps the helper simple.
        std::mem::forget(dir);
        App::new(&ctx).unwrap()
    }

    #[test]
    fn status_picker_excludes_terminal_statuses() {
        let mut app = app_with_task();
        app.start_status_pick();
        assert!(app.mode == Mode::Status);
        let pick = app.status_pick.as_ref().expect("picker opened");
        assert!(pick.options.contains(&"todo".to_string()));
        assert!(pick.options.contains(&"in-progress".to_string()));
        // `done` is terminal — reachable only via `close`, never the picker.
        assert!(!pick.options.contains(&"done".to_string()));
        // The picker starts on the document's current status.
        assert_eq!(pick.options[pick.idx], "todo");
    }
}
