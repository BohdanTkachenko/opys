//! The TUI application state (the TEA model) and the input reducer.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::doc::Doc;
use crate::error::Result;
use crate::project::Project;
use crate::Ctx;

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
        match self.mode {
            Mode::Browse => self.handle_browse(key),
            Mode::Filter => self.handle_filter(key),
            Mode::Stats => self.handle_stats(key),
        }
    }

    fn handle_browse(&mut self, key: KeyEvent) {
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
            KeyCode::Char(c) => {
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
}
