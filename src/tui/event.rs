//! The two event sources — keyboard input and the debounced file watcher —
//! merged onto one channel that the main loop consumes.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use notify_debouncer_full::notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use ratatui::crossterm::event;

use crate::error::{usage, Result};

/// A unit of work for the main loop: a terminal input event, or a debounced
/// signal that documents on disk changed and the board should reload.
pub enum Event {
    Input(event::Event),
    FsChanged,
}

/// Spawn a thread that blocks on terminal input and forwards each event. The
/// thread exits when the receiver is dropped (the loop has ended).
pub fn spawn_input(tx: Sender<Event>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(Event::Input(ev)).is_err() {
                break;
            }
        }
    });
}

/// Watch `base` recursively, coalescing the burst of writes a single command or
/// sync produces into one [`Event::FsChanged`] per debounce window. The returned
/// guard must be held for the lifetime of the loop — dropping it stops watching.
pub fn spawn_watcher(tx: Sender<Event>, base: &Path) -> Result<impl Drop> {
    let mut debouncer = new_debouncer(
        Duration::from_millis(250),
        None,
        move |res: DebounceEventResult| {
            if res.is_ok() {
                let _ = tx.send(Event::FsChanged);
            }
        },
    )
    .map_err(|e| usage(format!("file watcher: {e}")))?;
    debouncer
        .watcher()
        .watch(base, RecursiveMode::Recursive)
        .map_err(|e| usage(format!("watch {}: {e}", base.display())))?;
    Ok(debouncer)
}
