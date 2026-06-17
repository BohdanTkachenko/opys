//! Rendering. Pure presentation: reads [`App`] state and draws it, holding no
//! business logic of its own.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use super::app::{App, PreviewLayout};
use super::theme;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let (body, status_bar) = (rows[0], rows[1]);

    let (list_area, preview_area) = match app.preview {
        PreviewLayout::Right => {
            let cols = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(body);
            (cols[0], Some(cols[1]))
        }
        PreviewLayout::Bottom => {
            let cols = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(body);
            (cols[0], Some(cols[1]))
        }
        PreviewLayout::Hidden => (body, None),
    };

    render_list(frame, app, list_area);
    if let Some(preview) = preview_area {
        render_preview(frame, app, preview);
    }
    render_status(frame, app, status_bar);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(["", "id", "title", "status", "tags"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.board.docs.iter().map(|d| {
        let st = theme::doc_style(&app.prj, d);
        let id = d.id().unwrap_or("?").to_string();
        let status = d.status().unwrap_or("").to_string();
        let tags = d.frontmatter.tags().unwrap_or_default().join(", ");
        Row::new(vec![
            Cell::from(st.icon).style(Style::default().fg(st.fg)),
            Cell::from(id),
            Cell::from(d.title.clone()),
            Cell::from(status).style(Style::default().fg(st.fg)),
            Cell::from(tags),
        ])
    });

    let widths = [
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Min(16),
        Constraint::Length(14),
        Constraint::Length(22),
    ];

    let arrow = if app.sort.desc { "▼" } else { "▲" };
    let title = format!(
        " inventory · {} docs · sort: {} {} ",
        app.board.docs.len(),
        app.sort.key.label(),
        arrow
    );

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(Block::default().borders(Borders::ALL).title(title));

    let mut state = TableState::default();
    if !app.board.docs.is_empty() {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" preview ");
    let text = match app.selected_doc() {
        Some(d) => d.to_text(),
        None => "no document selected".to_string(),
    };
    let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let line = match &app.status {
        Some(msg) => Line::from(Span::styled(
            format!(" {msg} "),
            Style::default().add_modifier(Modifier::REVERSED),
        )),
        None => Line::from(
            " q quit · j/k move · p preview · sort: u updated · c created · s status · t title · i id ",
        ),
    };
    frame.render_widget(Paragraph::new(line), area);
}
