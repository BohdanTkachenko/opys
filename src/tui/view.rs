//! Rendering. Pure presentation: reads [`App`] state and draws it, holding no
//! business logic of its own.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::commands::stats;

use super::app::{App, Mode, PreviewLayout};
use super::filter::{self, FilterField};
use super::theme;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let (body, status_bar) = (rows[0], rows[1]);

    if app.mode == Mode::Stats {
        render_stats(frame, app, body);
        render_status(frame, app, status_bar);
        return;
    }

    // In filter mode a panel takes the bottom of the body; the list stays live
    // above it so filtering is visible as it is edited.
    let (content, filter_area) = if app.mode == Mode::Filter {
        let split = Layout::vertical([Constraint::Min(1), Constraint::Length(6)]).split(body);
        (split[0], Some(split[1]))
    } else {
        (body, None)
    };

    let (list_area, preview_area) = match app.preview {
        PreviewLayout::Right => {
            let cols = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(content);
            (cols[0], Some(cols[1]))
        }
        PreviewLayout::Bottom => {
            let cols = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(content);
            (cols[0], Some(cols[1]))
        }
        PreviewLayout::Hidden => (content, None),
    };

    render_list(frame, app, list_area);
    if let Some(preview) = preview_area {
        render_preview(frame, app, preview);
    }
    if let Some(fa) = filter_area {
        render_filter(frame, app, fa);
    }
    render_status(frame, app, status_bar);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(["", "id", "title", "status", "tags"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.visible.iter().map(|&i| {
        let d = &app.board.docs[i];
        let st = theme::doc_style(&app.prj, d);
        let id = d.id().unwrap_or("?").to_string();
        let status = d.status().unwrap_or("").to_string();
        let tags = d.frontmatter.tags().unwrap_or_default().join(", ");
        Row::new(vec![
            Cell::from(st.icon),
            Cell::from(id),
            Cell::from(d.title.clone()),
            Cell::from(status),
            Cell::from(tags),
        ])
        .style(st.style)
    });

    let widths = [
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Min(16),
        Constraint::Length(14),
        Constraint::Length(22),
    ];

    let arrow = if app.sort.desc { "▼" } else { "▲" };
    let count = if app.filter.is_active() {
        format!("{}/{}", app.visible.len(), app.board.docs.len())
    } else {
        app.board.docs.len().to_string()
    };
    let title = format!(
        " inventory · {count} docs · sort: {} {arrow} ",
        app.sort.key.label()
    );

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(Block::default().borders(Borders::ALL).title(title));

    let mut state = TableState::default();
    if !app.visible.is_empty() {
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

fn render_filter(frame: &mut Frame, app: &App, area: Rect) {
    let type_opts = filter::type_options(&app.prj);
    let status_opts = filter::status_options(&app.prj, app.filter.doc_type.as_deref());

    let value = |f: FilterField| -> String {
        match f {
            FilterField::Type => app
                .filter
                .doc_type
                .clone()
                .unwrap_or_else(|| "(any)".into()),
            FilterField::Status => app.filter.status.clone().unwrap_or_else(|| "(any)".into()),
            FilterField::Tag => {
                let t = app.filter.tag.clone().unwrap_or_default();
                if t.is_empty() {
                    "(any)".into()
                } else {
                    t
                }
            }
            FilterField::Query => {
                let q = app.filter.query.clone();
                if q.is_empty() {
                    "(none)".into()
                } else {
                    q
                }
            }
        }
    };

    let lines: Vec<Line> = FilterField::ALL
        .iter()
        .map(|&f| {
            let focused = app.filter_focus == f;
            let marker = if focused { "›" } else { " " };
            let label_style = if focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let hint = match f {
                FilterField::Type if !type_opts.is_empty() => " (←/→)",
                FilterField::Status if !status_opts.is_empty() => " (←/→)",
                FilterField::Tag | FilterField::Query => " (type)",
                _ => "",
            };
            Line::from(vec![
                Span::styled(format!("{marker} {:<7}", f.label()), label_style),
                Span::raw(format!(": {}{hint}", value(f))),
            ])
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" filter · Tab field · ←/→ choose · Enter/Esc done · x clears ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_stats(frame: &mut Frame, app: &App, area: Rect) {
    let docs = app.visible_docs();
    let report = stats::compute(&app.prj.pcfg, &docs);

    let mut lines: Vec<Line> = Vec::new();
    let scope = if app.filter.is_active() {
        format!("filtered: {}", app.filter.summary())
    } else {
        "all documents".to_string()
    };
    lines.push(Line::from(Span::styled(
        format!("documents: {}  ({scope})", report.total),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    for ts in &report.per_type {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("{}: {}", ts.name, ts.total),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for sc in &ts.by_status {
            let color = theme::status_color_for(Some(&sc.status));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{:<16}", sc.status), Style::default().fg(color)),
                Span::raw(format!("{:>4}  {:>3}%", sc.count, sc.pct)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "uncovered test-plan items: {}",
        report.uncovered_testplan
    )));
    lines.push(Line::from(format!(
        "manual verification items: {}  (without automated coverage: {})",
        report.manual_total, report.manual_uncovered
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" stats · Esc/S back ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(msg) = &app.status {
        let line = Line::from(Span::styled(
            format!(" {msg} "),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let help = match app.mode {
        Mode::Browse => {
            " q quit · j/k move · p preview · f filter · x clear · S stats · u/c/s/t/i sort "
        }
        Mode::Filter => " filter mode · Tab field · ←/→ choose · type to search · Enter/Esc done ",
        Mode::Stats => " stats · Esc/S back · q quit ",
    };
    let filter_hint = if app.filter.is_active() {
        format!(" · filter: {}", app.filter.summary())
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::from(format!("{help}{filter_hint}"))),
        area,
    );
}
