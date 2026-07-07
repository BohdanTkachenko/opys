//! Interactive terminal UI for opys — a live, read-mostly board over the
//! inventory that updates as documents change on disk.
//!
//! This is a thin frontend over the library: reads go through
//! [`load_docs`](opys_engine::backend::Backend::load_docs), and the two writes it
//! offers — a status change (`set_status::core`) and `close` (`close::core`) —
//! go through the existing command cores, so on-disk invariants hold exactly as
//! in the CLI. Body edits are delegated to `$EDITOR` (`e`/Enter), after which the
//! board runs the auto-sync + `verify` pass. Compiled only with the `tui` feature.

mod app;
mod data;
mod event;
mod filter;
mod markdown;
mod sort;
mod theme;
mod view;

use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::KeyEventKind;
use ratatui::crossterm::event::{poll as poll_key, read as read_event, Event as CtEvent};

use opys_engine::error::{OpysError, Result};
use opys_engine::Ctx;

use app::App;
use event::Event;

/// How long each loop iteration waits for a key before checking the file-watcher
/// channel and redrawing. Small enough to feel responsive to on-disk changes.
const POLL: Duration = Duration::from_millis(150);

/// Entry point for `opys tui`. Sets up the alternate screen (with a panic hook
/// that restores the terminal), runs the event loop, and always restores the
/// terminal on exit. Returns the process exit code.
pub fn run(ctx: &Ctx) -> Result<i32> {
    let mut app = App::new(ctx)?;

    let (tx, rx) = mpsc::channel();
    let base = app.prj.base.clone();
    // Held until the loop ends; dropping the guard stops the file watcher.
    let _watcher = event::spawn_watcher(tx, &base)?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    result?;
    Ok(0)
}

/// The main loop. Keyboard input is polled inline (no input thread) so that a
/// spawned `$EDITOR` gets exclusive use of the terminal; the file watcher feeds
/// reload signals over `rx`, drained after each poll window.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mpsc::Receiver<Event>,
) -> Result<()> {
    let mut redraw = true;
    loop {
        if redraw {
            terminal
                .draw(|frame| view::render(frame, app))
                .map_err(OpysError::from)?;
            redraw = false;
        }

        // Poll for a key, then drain any file-change signals.
        if poll_key(POLL).map_err(OpysError::from)? {
            if let CtEvent::Key(key) = read_event().map_err(OpysError::from)? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key);
                    redraw = true;
                }
            }
        }
        while let Ok(Event::FsChanged) = rx.try_recv() {
            app.reload();
            redraw = true;
        }

        // A key may have requested opening the document in $EDITOR. Do it here,
        // outside `handle_key`, because it needs to suspend and restore the TUI.
        if let Some(path) = app.pending_editor.take() {
            open_in_editor(terminal, &path)?;
            app.after_external_edit();
            redraw = true;
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Suspend the TUI, run `$EDITOR` (falling back to `$VISUAL`, then `vi`) on
/// `path`, then restore the alternate screen. The editor string is split on
/// whitespace so `EDITOR="code -w"`-style commands work.
fn open_in_editor(terminal: &mut ratatui::DefaultTerminal, path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");

    ratatui::restore();
    let status = Command::new(program).args(parts).arg(path).status();
    *terminal = ratatui::init();
    terminal.clear().map_err(OpysError::from)?;

    // A launch failure (editor not found) is surfaced; a non-zero editor exit is
    // ignored — the user may have quit without saving, which is fine.
    status.map_err(|e| OpysError::from(std::io::Error::new(e.kind(), format!("{editor}: {e}"))))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    const CONFIG: &str = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
default_status = \"planned\"\ntags_required = false\n";

    fn temp_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("opys.toml"), CONFIG).unwrap();
        std::fs::create_dir_all(dir.path().join("opys")).unwrap();
        std::fs::write(
            dir.path().join("opys/FEAT-0001.md"),
            "---\nid: FEAT-0001\nstatus: planned\ntags: [demo]\n---\n\n# Hello world\n",
        )
        .unwrap();
        dir
    }

    fn buffer_text(dir: &tempfile::TempDir) -> String {
        let ctx = Ctx {
            root: dir.path().to_string_lossy().into_owned(),
            no_sync: true,
            backend: Box::new(opys_backend_markdown_local::MarkdownLocal),
        };
        let app = App::new(&ctx).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| view::render(frame, &app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn renders_board_with_header_and_document() {
        let dir = temp_project();
        let text = buffer_text(&dir);
        assert!(text.contains("inventory"), "missing title in: {text}");
        assert!(text.contains("FEAT-0001"), "missing doc id");
        assert!(text.contains("Hello world"), "missing doc title");
    }
}
