//! A minimal text editor widget — a cursor over a buffer of lines, used both for
//! single-line fields (title, tags, custom values) and the multi-line body. No
//! external dependency; just enough editing for the edit/new screens.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::markdown;

pub struct TextArea {
    /// Always at least one line; characters (not bytes) so the cursor column is
    /// a simple index.
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    scroll: usize,
    multiline: bool,
    /// Apply (non-destructive) markdown syntax highlighting on render.
    highlight: bool,
}

impl TextArea {
    pub fn new(text: &str, multiline: bool) -> TextArea {
        let mut lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        let row = lines.len() - 1;
        let col = lines[row].len();
        TextArea {
            lines,
            row,
            col,
            scroll: 0,
            multiline,
            highlight: false,
        }
    }

    /// Enable markdown syntax highlighting while editing (for the body editor).
    pub fn highlighted(mut self) -> TextArea {
        self.highlight = true;
        self
    }

    /// Whether each line sits inside a fenced code block (for highlighting).
    fn code_states(&self) -> Vec<bool> {
        let mut states = Vec::with_capacity(self.lines.len());
        let mut inside = false;
        for line in &self.lines {
            let is_fence = line
                .iter()
                .collect::<String>()
                .trim_start()
                .starts_with("```");
            if is_fence {
                states.push(false);
                inside = !inside;
            } else {
                states.push(inside);
            }
        }
        states
    }

    /// Single-line convenience: the buffer joined with newlines.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Handle a key. Returns whether the content changed (to mark the form
    /// dirty); pure navigation returns `false`. `Tab` is never consumed here —
    /// the form uses it to move focus.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                self.insert(c);
                true
            }
            KeyCode::Enter if self.multiline => {
                self.newline();
                true
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Left => {
                self.move_left();
                false
            }
            KeyCode::Right => {
                self.move_right();
                false
            }
            KeyCode::Up if self.multiline => {
                self.move_up();
                false
            }
            KeyCode::Down if self.multiline => {
                self.move_down();
                false
            }
            KeyCode::Home => {
                self.col = 0;
                false
            }
            KeyCode::End => {
                self.col = self.lines[self.row].len();
                false
            }
            _ => false,
        }
    }

    fn insert(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
    }

    fn newline(&mut self) {
        let tail = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) -> bool {
        if self.col > 0 {
            self.lines[self.row].remove(self.col - 1);
            self.col -= 1;
            true
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(cur);
            true
        } else {
            false
        }
    }

    fn delete_forward(&mut self) -> bool {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
            true
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
            true
        } else {
            false
        }
    }

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }

    fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + height {
            self.scroll = self.row - height + 1;
        }
    }

    /// Render inside `area` with a bordered block titled `label`; draws the
    /// terminal cursor when `focused`.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, label: &str) {
        let inner_h = area.height.saturating_sub(2) as usize;
        self.ensure_visible(inner_h);

        let end = (self.scroll + inner_h.max(1)).min(self.lines.len());
        let visible: Vec<String> = self.lines[self.scroll..end]
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect();
        let rendered: Vec<Line> = if self.highlight {
            let states = self.code_states();
            visible
                .iter()
                .enumerate()
                .map(|(k, s)| markdown::highlight_edit_line(s, states[self.scroll + k]))
                .collect()
        } else {
            visible.iter().map(|s| Line::from(s.clone())).collect()
        };

        let border = if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let title = if focused {
            format!(" {label} ◂ ")
        } else {
            format!(" {label} ")
        };
        let title_style = Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(if focused { Color::Yellow } else { Color::Gray });
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title_style(title_style)
            .title(title);
        frame.render_widget(Paragraph::new(rendered).block(block), area);

        if focused {
            let cx = area.x + 1 + self.col.min(area.width.saturating_sub(2) as usize) as u16;
            let cy = area.y + 1 + (self.row - self.scroll) as u16;
            frame.set_cursor_position((cx, cy));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn types_and_deletes() {
        let mut ta = TextArea::new("", true);
        for c in "hi".chars() {
            ta.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(ta.text(), "hi");
        ta.handle_key(key(KeyCode::Backspace));
        assert_eq!(ta.text(), "h");
    }

    #[test]
    fn newline_splits_and_backspace_joins() {
        let mut ta = TextArea::new("ab", true);
        // Cursor starts at end (after "ab"); move left once → between a and b.
        ta.handle_key(key(KeyCode::Left));
        ta.handle_key(key(KeyCode::Enter));
        assert_eq!(ta.text(), "a\nb");
        ta.handle_key(key(KeyCode::Backspace));
        assert_eq!(ta.text(), "ab");
    }

    #[test]
    fn single_line_ignores_enter() {
        let mut ta = TextArea::new("x", false);
        let changed = ta.handle_key(key(KeyCode::Enter));
        assert!(!changed);
        assert_eq!(ta.text(), "x");
    }
}
