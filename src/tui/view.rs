//! Rendering. Pure presentation: reads [`App`] state and draws it, holding no
//! business logic of its own.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use serde_norway::Value;

use crate::commands::stats;
use crate::doc::Doc;
use crate::frontmatter::Frontmatter;
use crate::project::Project;

use super::app::{App, Mode, PreviewLayout};
use super::filter::{self, FilterField};
use super::form::FieldView;
use super::markdown;
use super::theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let (body, status_bar) = (rows[0], rows[1]);

    if app.mode == Mode::Stats {
        render_stats(frame, app, body);
        render_status(frame, app, status_bar);
        return;
    }

    if app.mode == Mode::Edit {
        render_edit(frame, app, body);
        render_status(frame, app, status_bar);
        return;
    }

    if app.mode == Mode::NewType {
        render_new_type(frame, app, body);
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
    let columns = &app.prj.pcfg.tui.columns;

    // Header: a leading icon column, then the configured columns.
    let mut header_cells = vec![Cell::from("")];
    header_cells.extend(columns.iter().map(|c| Cell::from(c.as_str())));
    let header = Row::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.visible.iter().map(|&i| {
        let d = &app.board.docs[i];
        let st = theme::doc_style(&app.prj, d);
        let mut cells = vec![Cell::from(st.icon)];
        cells.extend(
            columns
                .iter()
                .map(|c| Cell::from(column_value(&app.prj, d, c))),
        );
        Row::new(cells).style(st.style)
    });

    let mut widths = vec![Constraint::Length(2)];
    widths.extend(columns.iter().map(|c| column_width(c)));

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

/// The display value of a list column for a document — a built-in or a custom
/// frontmatter field.
fn column_value(prj: &Project, d: &Doc, key: &str) -> String {
    match key {
        "id" => d.id().unwrap_or("?").to_string(),
        "type" => d
            .id()
            .and_then(|id| prj.pcfg.type_name_for_id(id))
            .unwrap_or("")
            .to_string(),
        "title" => d.title.clone(),
        "status" => d.status().unwrap_or("").to_string(),
        "tags" => d.frontmatter.tags().unwrap_or_default().join(", "),
        // Trim the timezone for a compact, aligned timestamp column.
        "created" | "updated" => d
            .frontmatter
            .get_str(key)
            .map(|s| s.chars().take(16).collect())
            .unwrap_or_default(),
        other => custom_value(&d.frontmatter, other),
    }
}

fn custom_value(fm: &Frontmatter, key: &str) -> String {
    fn scalar(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }
    match fm.get(key) {
        Some(Value::Sequence(seq)) => seq.iter().filter_map(scalar).collect::<Vec<_>>().join(", "),
        Some(v) => scalar(v).unwrap_or_default(),
        None => String::new(),
    }
}

fn column_width(key: &str) -> Constraint {
    match key {
        "title" => Constraint::Min(16),
        "id" => Constraint::Length(12),
        "status" => Constraint::Length(13),
        "tags" => Constraint::Length(22),
        "type" => Constraint::Length(10),
        "created" | "updated" => Constraint::Length(18),
        _ => Constraint::Length(14),
    }
}

fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" preview ");
    let paragraph = match app.selected_doc() {
        Some(d) => Paragraph::new(markdown::render(&d.to_text())),
        None => Paragraph::new("no document selected"),
    };
    frame.render_widget(paragraph.block(block).wrap(Wrap { trim: false }), area);
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

    if !report.tag_keys.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "tags by key",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for tk in &report.tag_keys {
            lines.push(Line::from(format!("  {} ({} docs)", tk.key, tk.docs)));
            for v in &tk.by_value {
                lines.push(Line::from(format!("    {:<16}{:>4}", v.value, v.count)));
            }
        }
    }
    if !report.plain_tags.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "tags",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for tc in &report.plain_tags {
            lines.push(Line::from(format!("  {:<16}{:>4}", tc.tag, tc.count)));
        }
    }

    if !report.coverage.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "coverage",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for c in &report.coverage {
            lines.push(Line::from(format!(
                "  {:<16} {:<10} {} uncovered / {} items",
                c.heading, c.kind, c.uncovered, c.items
            )));
        }
    }

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

fn render_edit(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(form) = app.edit.as_mut() else {
        return;
    };

    let fields = form.fields();
    // Fields block height: one line per field (minus the body) + borders.
    let field_lines = fields.len().saturating_sub(1).max(1) as u16;
    let relations = form.relations_summary();
    let rel_height = if relations.is_empty() {
        0
    } else {
        relations.len() as u16 + 2
    };

    let chunks = Layout::vertical([
        Constraint::Length(field_lines + 2),
        Constraint::Length(rel_height),
        Constraint::Min(5),
    ])
    .split(area);
    let (fields_area, rel_area, body_area) = (chunks[0], chunks[1], chunks[2]);

    // Fields.
    let mut lines: Vec<Line> = Vec::new();
    for fv in &fields {
        lines.push(field_line(fv));
    }
    let kind = if form.is_new { "new" } else { "edit" };
    let dirty = if form.dirty { " ●" } else { "" };
    let title = format!(" {kind} {}{dirty} ", form.title_id());
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        fields_area,
    );

    // Read-only relations.
    if rel_height > 0 {
        let rel_lines: Vec<Line> = relations
            .iter()
            .map(|(field, items)| {
                let joined = items
                    .iter()
                    .map(|(id, t)| format!("{id} ({t})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Line::from(format!("{field}: {joined}"))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(rel_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" relations (read-only) "),
            ),
            rel_area,
        );
    }

    // Body editor (the one multi-line widget; draws its own cursor when focused).
    form.render_body(frame, body_area);
}

fn field_line(fv: &FieldView) -> Line<'static> {
    let (focused, label, value, hint) = match fv {
        FieldView::Line {
            focused,
            label,
            text,
        } => (*focused, label.to_string(), text.clone(), "type"),
        FieldView::Choice {
            focused,
            label,
            value,
        } => (*focused, label.to_string(), value.clone(), "←/→"),
        FieldView::Custom {
            focused,
            label,
            value,
        } => (*focused, label.clone(), value.clone(), "edit"),
        FieldView::Body { focused } => (*focused, "body".to_string(), "(below)".to_string(), ""),
    };
    let marker = if focused { "›" } else { " " };
    let label_style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let suffix = if focused && !hint.is_empty() {
        format!("  ({hint})")
    } else {
        String::new()
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label:<12}"), label_style),
        Span::raw(format!(": {value}{suffix}")),
    ])
}

fn render_new_type(frame: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app
        .new_types
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == app.new_type_idx { "›" } else { " " };
            let style = if i == app.new_type_idx {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!("{marker} {name}"), style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" new document · pick a type · ↑/↓ · Enter · Esc "),
        ),
        area,
    );
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    // Highest-priority transient prompts first.
    if let Some(id) = &app.confirm_close {
        return reversed(
            frame,
            area,
            &format!(" close {id}? this deletes the file · y / n "),
        );
    }
    if app.mode == Mode::Edit {
        if let Some(form) = &app.edit {
            if form.conflict.is_some() {
                return reversed(
                    frame,
                    area,
                    " file changed on disk · r reload (discard) · i ignore (keep edits) ",
                );
            }
            if let Some(msg) = &form.message {
                return reversed(frame, area, &format!(" {msg} "));
            }
        }
    }
    if let Some(msg) = &app.status {
        return reversed(frame, area, &format!(" {msg} "));
    }

    let help = match app.mode {
        Mode::Browse => {
            " q quit · j/k · e edit · n new · D close · p preview · f filter · S stats · u/c/s/t/i sort "
        }
        Mode::Filter => " filter · Tab field · ←/→ choose · type to search · Enter/Esc done ",
        Mode::Stats => " stats · Esc/S back · q quit ",
        Mode::NewType => " new · ↑/↓ pick type · Enter create · Esc cancel ",
        Mode::Edit => " edit · Tab next field · ←/→ choose · Ctrl-S save · Esc cancel ",
    };
    let filter_hint = if app.mode == Mode::Browse && app.filter.is_active() {
        format!(" · filter: {}", app.filter.summary())
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::from(format!("{help}{filter_hint}"))),
        area,
    );
}

fn reversed(frame: &mut Frame, area: Rect, text: &str) {
    let line = Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::REVERSED),
    ));
    frame.render_widget(Paragraph::new(line), area);
}
