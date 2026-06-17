//! The TUI application state (the TEA model) and the input reducer.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::doc::Doc;
use crate::error::Result;
use crate::project::Project;
use crate::Ctx;

use super::data::Board;
use super::sort::{sort_docs, SortKey, SortState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreviewLayout {
    Right,
    Bottom,
    Hidden,
}

pub struct App {
    pub prj: Project,
    pub board: Board,
    pub selected: usize,
    pub preview: PreviewLayout,
    pub sort: SortState,
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
            selected: 0,
            preview: PreviewLayout::Right,
            sort,
            status: None,
            should_quit: false,
        };
        app.refresh_status();
        Ok(app)
    }

    pub fn selected_doc(&self) -> Option<&Doc> {
        self.board.docs.get(self.selected)
    }

    /// Reload the board from disk, keeping the selection on the same document id
    /// when it still exists, otherwise clamping into range.
    pub fn reload(&mut self) {
        let current = self.selected_id();
        self.board.reload(&self.prj, self.sort);
        self.restore_selection(current.as_deref());
        self.refresh_status();
    }

    /// Re-sort in place (e.g. after a sort-key change), preserving selection.
    fn resort(&mut self) {
        let current = self.selected_id();
        sort_docs(&mut self.board.docs, self.sort);
        self.restore_selection(current.as_deref());
    }

    fn selected_id(&self) -> Option<String> {
        self.selected_doc().and_then(|d| d.id()).map(str::to_string)
    }

    fn restore_selection(&mut self, prev_id: Option<&str>) {
        if let Some(id) = prev_id {
            if let Some(idx) = self.board.docs.iter().position(|d| d.id() == Some(id)) {
                self.selected = idx;
                return;
            }
        }
        let len = self.board.docs.len();
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
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
        if self.selected + 1 < self.board.docs.len() {
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

    /// Map a key press to a state change (the read-only Phase 1 bindings).
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.board.docs.len().saturating_sub(1)
            }
            KeyCode::Char('p') => self.toggle_preview(),
            KeyCode::Char('u') => self.set_sort(SortKey::Updated),
            KeyCode::Char('c') => self.set_sort(SortKey::Created),
            KeyCode::Char('s') => self.set_sort(SortKey::Status),
            KeyCode::Char('t') => self.set_sort(SortKey::Title),
            KeyCode::Char('i') => self.set_sort(SortKey::Id),
            _ => {}
        }
    }
}
