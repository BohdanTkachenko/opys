//! Interactive terminal UI for opys — a live board over the inventory that
//! updates as documents change on disk.
//!
//! This is a thin frontend over the library: every read goes through
//! [`Project::load_docs`](crate::project::Project::load_docs) and (in later
//! phases) every write through the existing command cores, so on-disk
//! invariants hold exactly as in the CLI. Compiled only with the `tui` feature.

mod app;
mod data;
mod event;
mod filter;
mod form;
mod markdown;
mod save;
mod sort;
mod textarea;
mod theme;
mod view;

use std::sync::mpsc;

use ratatui::crossterm::event::{Event as CtEvent, KeyEventKind};

use crate::error::{OpysError, Result};
use crate::Ctx;

use app::App;
use event::Event;

/// Entry point for `opys tui`. Sets up the alternate screen (with a panic hook
/// that restores the terminal), runs the event loop, and always restores the
/// terminal on exit. Returns the process exit code.
pub fn run(ctx: &Ctx) -> Result<i32> {
    let mut app = App::new(ctx)?;

    let (tx, rx) = mpsc::channel();
    event::spawn_input(tx.clone());
    let base = app.prj.base.clone();
    // Held until the loop ends; dropping the guard stops the file watcher.
    let _watcher = event::spawn_watcher(tx, &base)?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    result?;
    Ok(0)
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mpsc::Receiver<Event>,
) -> Result<()> {
    loop {
        terminal
            .draw(|frame| view::render(frame, app))
            .map_err(OpysError::from)?;
        // (app is &mut App; view::render takes &mut to scroll the body editor)

        match rx.recv() {
            Ok(Event::Input(CtEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                app.handle_key(key);
            }
            Ok(Event::Input(_)) => {}
            Ok(Event::FsChanged) => app.reload(),
            // Both producer threads gone — nothing left to drive the loop.
            Err(_) => break,
        }

        if app.should_quit {
            break;
        }
    }
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
        };
        let mut app = App::new(&ctx).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| view::render(frame, &mut app))
            .unwrap();
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
