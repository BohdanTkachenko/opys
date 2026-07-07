//! The file-watcher event source. Keyboard input is polled directly by the main
//! loop (see `lib::event_loop`) rather than pumped through a thread, so it does
//! not compete with a spawned `$EDITOR` for the terminal's stdin.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use notify_debouncer_full::notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

use opys_engine::error::{usage, Result};

/// A debounced signal that documents on disk changed and the board should
/// reload. (Keyboard events are read inline by the loop, not sent here.)
pub enum Event {
    FsChanged,
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
