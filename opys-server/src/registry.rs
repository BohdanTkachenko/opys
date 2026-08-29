//! The allowlist: what the node is permitted to serve (ADR-0077).
//!
//! A TOML file the user owns, holding two kinds of entry — an explicit project
//! and a directory prefix. Nothing outside a matching entry is ever served, and
//! discovery can only *suggest* additions; it never writes here.
//!
//! `add`/`remove` edit this file and nothing else. They never speak to a running
//! node, which is what keeps filesystem paths out of the API surface entirely
//! (ADR-0077): the node watches the file instead.

use std::path::{Path, PathBuf};

use opys_engine::error::{usage, OpysError, Result};
use serde::Serialize;

/// Depth bound for expanding a prefix entry and for suggestion scans. Ten
/// levels covers every plausible project layout; the cost of going deeper is
/// measured in ADR-0077.
pub const DEFAULT_DEPTH: usize = 10;

/// What an allowlist entry authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// Exactly this directory, which must contain `opys.toml`.
    Project,
    /// Every project found beneath this directory, to `depth` levels.
    Prefix,
}

impl EntryKind {
    /// The config key an entry of this kind is written under, which doubles as
    /// its label when the CLI lists the allowlist.
    pub fn key(self) -> &'static str {
        match self {
            EntryKind::Project => "project",
            EntryKind::Prefix => "prefix",
        }
    }
}

/// One allowlist entry, as parsed.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    /// The path exactly as written in the file, before `~` expansion.
    pub raw_path: String,
    /// Expanded, and canonicalized when the directory exists.
    pub path: PathBuf,
    /// Depth bound for a prefix entry; meaningless for a project entry.
    pub depth: usize,
    /// Why this entry is unusable, if it is. Kept rather than dropped so the UI
    /// can say "you allowlisted this and it is gone" instead of forgetting it.
    pub error: Option<String>,
}

impl Entry {
    /// Whether this entry authorizes `path` — exactly, for a project entry, or
    /// within the depth bound, for a prefix. An entry carrying an error
    /// authorizes nothing.
    ///
    /// The one definition of "allowlisted", so `Registry::covers` and the CLI's
    /// "which entry is responsible?" message can never disagree.
    ///
    /// `depth` counts *directories* below the prefix, which is what
    /// `discover::scan_projects` walks to (one level deeper, for the `opys.toml`
    /// inside). The two are pinned together by a test — a `covers` that reaches
    /// further than the walk makes the CLI refuse to allowlist projects the node
    /// then never serves.
    pub fn covers(&self, path: &Path) -> bool {
        if self.error.is_some() {
            return false;
        }
        match self.kind {
            EntryKind::Project => self.path == path,
            EntryKind::Prefix => path
                .strip_prefix(&self.path)
                .is_ok_and(|rest| rest.components().count() <= self.depth),
        }
    }
}

/// What the periodic scan does with what it finds (FEAT-0083).
///
/// There is deliberately no auto-add. Allowlisting a project is what causes it
/// to be *opened*, and opening it reads whatever its `opys.toml` points `base`
/// at — so a mode that allowlisted without a person in the loop would also read
/// without one. Once suggestions reach the UI, auto-add saves a single click and
/// costs the explicit-allowlist property ADR-0077 exists to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    /// Never walk. For a large `$HOME`, or a user who wants the allowlist to be
    /// exactly what they wrote.
    Off,
    /// Walk, and offer what is found for approval. Nothing is served until
    /// accepted. The default, and what every config predating this setting gets.
    #[default]
    Suggest,
}

impl ScanMode {
    fn parse(s: &str) -> Option<ScanMode> {
        match s {
            "off" => Some(ScanMode::Off),
            "suggest" => Some(ScanMode::Suggest),
            _ => None,
        }
    }

    /// The spelling written back to the file.
    pub fn key(self) -> &'static str {
        match self {
            ScanMode::Off => "off",
            ScanMode::Suggest => "suggest",
        }
    }
}

/// The parsed allowlist, plus the raw table it came from so edits preserve keys
/// this version does not know about.
#[derive(Debug, Clone)]
pub struct Registry {
    /// Where this was loaded from (and where `save` writes).
    pub path: PathBuf,
    pub bind: Option<String>,
    /// What the scan does with what it finds. Absent in the file means
    /// [`ScanMode::Suggest`] — today's behaviour, so an existing config keeps it.
    pub mode: ScanMode,
    /// Where suggestion scans start. `None` means "no preference": callers fall
    /// back to their own default rather than assuming `$HOME` here.
    pub scan_root: Option<PathBuf>,
    pub entries: Vec<Entry>,
    raw: toml::Table,
}

/// `$XDG_CONFIG_HOME`, falling back to `~/.config`.
///
/// The single definition of that precedence: the allowlist file and the systemd
/// user unit both hang off it, so a test that redirects one redirects both.
pub fn config_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = home_dir().ok_or_else(|| usage("neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(home.join(".config"))
}

/// `$XDG_CONFIG_HOME/opys/server.toml`, falling back to `~/.config/opys/server.toml`.
pub fn config_path() -> Result<PathBuf> {
    Ok(config_home()?.join("opys").join("server.toml"))
}

/// `$HOME`, when it is set to something non-empty.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` against `$HOME`. Anything else is returned as-is.
pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(s));
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Vet a path the *UI* supplied, before it can reach the allowlist (ADR-0082).
///
/// This is the whole security boundary for browser-driven allowlisting, so it is
/// written to be read rather than to be clever. Three rules, in this order:
///
/// 1. **Canonicalize first.** Resolving `..` and symlinks before any comparison
///    is what makes the rest meaningful — a link inside `$HOME` pointing out of
///    it must fail on where it *lands*, not on how it is spelled.
/// 2. **Under `$HOME`.** `$HOME` is canonicalized too: on a system where `/home`
///    is itself a symlink, comparing a resolved path against an unresolved home
///    rejects everything.
/// 3. **No dot-components, at any depth.** `~/.ssh` and `~/.config` are out of
///    reach, and so is `~/projects/.hidden/x`. The predicate is
///    [`crate::discover::is_skipped`]'s leading-dot half, so the scan and the UI
///    cannot drift on what "hidden" means.
///
/// Serving a path outside `$HOME` stays possible by editing `server.toml`
/// directly. That is the escape hatch, and deliberately the only one: the file
/// is reachable by someone with a shell, which is a different threat model from
/// a request arriving at a socket.
///
/// The path must exist — a directory that is not there cannot be canonicalized,
/// and allowlisting one would only produce an entry that never resolves.
pub fn vet_ui_path(raw: &str) -> Result<PathBuf> {
    let refused = |why: &str| usage(format!("refusing {raw:?}: {why}"));
    if raw.trim().is_empty() {
        return Err(refused("no path given"));
    }
    let expanded = expand_tilde(raw.trim());
    let path = std::fs::canonicalize(&expanded)
        .map_err(|e| refused(&format!("cannot resolve {}: {e}", expanded.display())))?;
    if !path.is_dir() {
        return Err(refused("not a directory"));
    }

    let home = home_dir().ok_or_else(|| usage("HOME is not set, so nothing can be vetted"))?;
    let home = std::fs::canonicalize(&home).unwrap_or(home);
    if !path.starts_with(&home) {
        return Err(refused(&format!(
            "it resolves to {}, outside your home directory ({}). Paths outside \
             $HOME can only be added by editing the allowlist file",
            path.display(),
            home.display()
        )));
    }

    // Only the part below `$HOME`: the home directory's own ancestors are not
    // the user's to be judged on (`/home`, `/Users/...`), and on some systems
    // one of them legitimately begins with a dot.
    let rest = path.strip_prefix(&home).unwrap_or(&path);
    for c in rest.components() {
        let name = c.as_os_str().to_string_lossy();
        if name.starts_with('.') {
            return Err(refused(&format!(
                "{name:?} is a hidden directory. Hidden paths cannot be added \
                 from the browser"
            )));
        }
    }
    Ok(path)
}

/// An exclusive lock over one allowlist file, held for the whole of a
/// read-modify-write. Released when dropped — or, if the process dies, by the
/// OS, so a stale lock cannot happen.
pub struct Lock {
    /// Held open for the lock's lifetime: closing the handle is what releases
    /// the flock, so this is the whole value of the struct.
    _file: std::fs::File,
}

/// Take the exclusive lock for the allowlist at `path`.
///
/// `add` and `remove` load, mutate and save. Two of them racing would each load
/// the same pre-state and the second save would drop the first's entry — after
/// both printed "added". The corpus gets exactly this protection from the
/// backend's inventory flock; the allowlist is the other shared mutable file.
///
/// The lock file lives in `$XDG_RUNTIME_DIR` (else the OS temp dir) rather than
/// beside the allowlist, for the same two reasons the backend's does: nothing
/// untracked appears next to the user's config, and `save` renames a *new* inode
/// over the allowlist, so a lock held on the allowlist itself would stop
/// excluding anything the moment it was used.
pub fn lock(path: &Path) -> Result<Lock> {
    use fs4::fs_std::FileExt;

    let key = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir(),
    };
    let lock_path = dir.join(format!("opys-allowlist-{}", crate::discover::id_for(&key)));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| usage(format!("{}: {e}", lock_path.display())))?;

    let timeout_ms: u64 = std::env::var("OPYS_LOCK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        match file.try_lock_exclusive() {
            Ok(true) => return Ok(Lock { _file: file }),
            Ok(false) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(usage(format!("{}: {e}", lock_path.display()))),
        }
        if std::time::Instant::now() >= deadline {
            return Err(usage(format!(
                "timed out after {timeout_ms} ms waiting for the allowlist lock for {} ({}) — \
                 another opys invocation is editing it",
                path.display(),
                lock_path.display()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

/// The inverse of [`expand_tilde`]: write paths under `$HOME` back as `~/…` so
/// the file stays portable between machines.
fn contract_tilde(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Refuse a hand-edited allowlist whose entries are the wrong shape.
///
/// The same policy as a syntax error, for the same reason: an entry this version
/// cannot read is one the user asked for and would not get. Dropping it silently
/// makes `web list` say "nothing allowlisted" about a file that plainly names a
/// project — and leaves `[project]` (one bracket) or a misspelled `path` sitting
/// there forever, because nothing ever complains about it.
fn check_shape(raw: &toml::Table, path: &Path) -> Result<()> {
    let bad = |msg: String| usage(format!("{}: {msg} — fix it by hand", path.display()));
    for kind in [EntryKind::Project, EntryKind::Prefix] {
        let key = kind.key();
        let Some(value) = raw.get(key) else { continue };
        let Some(items) = value.as_array() else {
            return Err(bad(format!(
                "`{key}` is not a list of entries (write `[[{key}]]`, not `[{key}]`)"
            )));
        };
        for item in items {
            let Some(table) = item.as_table() else {
                return Err(bad(format!("a `{key}` entry is not a table")));
            };
            if table.get("path").and_then(toml::Value::as_str).is_none() {
                return Err(bad(format!("a `{key}` entry has no `path` string")));
            }
            match table.get("depth") {
                None => {}
                // A `depth` this version cannot read must not widen silently to
                // the default: the user picked a number to bound the walk.
                Some(v) if v.as_integer().is_some_and(|d| usize::try_from(d).is_ok()) => {}
                Some(v) => {
                    return Err(bad(format!(
                        "`depth` in a `{key}` entry is not a non-negative integer: {v}"
                    )))
                }
            }
        }
    }
    // A `mode` this version cannot read must not fall back to the default:
    // someone who wrote `mode = "auto-add"` expecting no prompts should be told
    // it does not exist, not quietly given the mode that prompts.
    match raw.get("mode") {
        None => {}
        Some(v) if v.as_str().is_some_and(|m| ScanMode::parse(m).is_some()) => {}
        Some(v) => return Err(bad(format!("`mode` is not one of `off` or `suggest`: {v}"))),
    }
    if raw.get("scan_root").is_some_and(|v| v.as_str().is_none()) {
        return Err(bad("`scan_root` is not a string".to_string()));
    }
    Ok(())
}

impl Registry {
    /// Load from the default config path.
    pub fn load() -> Result<Registry> {
        Self::load_from(&config_path()?)
    }

    /// Load from `path`. A missing file is an empty allowlist — the node serves
    /// nothing until something is added. A malformed one is an error: silently
    /// serving less than the user asked for is worse than refusing to start.
    pub fn load_from(path: &Path) -> Result<Registry> {
        let raw = match std::fs::read_to_string(path) {
            Ok(text) => text
                .parse::<toml::Table>()
                .map_err(|source| OpysError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            // The path, not a bare errno: `web list` is the command you run to
            // find out why the node is serving nothing, so it has to say which
            // file it could not read.
            Err(e) => return Err(usage(format!("{}: {e}", path.display()))),
        };
        check_shape(&raw, path)?;
        let mut reg = Registry {
            path: path.to_path_buf(),
            bind: None,
            mode: ScanMode::default(),
            scan_root: None,
            entries: Vec::new(),
            raw,
        };
        reg.reparse();
        Ok(reg)
    }

    /// Rebuild the typed view from `raw`. Called after every edit so the two
    /// can never drift.
    fn reparse(&mut self) {
        self.bind = self
            .raw
            .get("bind")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        self.mode = self
            .raw
            .get("mode")
            .and_then(toml::Value::as_str)
            .and_then(ScanMode::parse)
            .unwrap_or_default();
        self.scan_root = self
            .raw
            .get("scan_root")
            .and_then(toml::Value::as_str)
            .map(expand_tilde);
        self.entries.clear();
        for kind in [EntryKind::Project, EntryKind::Prefix] {
            let Some(items) = self.raw.get(kind.key()).and_then(toml::Value::as_array) else {
                continue;
            };
            for item in items {
                let Some(raw_path) = item.get("path").and_then(toml::Value::as_str) else {
                    continue;
                };
                let expanded = expand_tilde(raw_path);
                let depth = item
                    .get("depth")
                    .and_then(toml::Value::as_integer)
                    .and_then(|d| usize::try_from(d).ok())
                    .unwrap_or(DEFAULT_DEPTH);
                let (path, error) = match std::fs::canonicalize(&expanded) {
                    Ok(p) if p.is_dir() => (p, None),
                    Ok(p) => (p, Some("not a directory".to_string())),
                    Err(e) => (expanded, Some(e.to_string())),
                };
                let error = error.or_else(|| {
                    // A project entry must actually be a project; a prefix entry
                    // is a place to look, so it need not contain one itself.
                    match kind {
                        EntryKind::Project if !path.join("opys.toml").is_file() => {
                            Some("no opys.toml here".to_string())
                        }
                        _ => None,
                    }
                });
                self.entries.push(Entry {
                    kind,
                    raw_path: raw_path.to_string(),
                    path,
                    depth,
                    error,
                });
            }
        }
    }

    /// Entries of one kind, skipping any that carry an error.
    pub fn usable(&self, kind: EntryKind) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |e| e.kind == kind && e.error.is_none())
    }

    /// Whether `path` is already allowlisted, either explicitly or by a prefix
    /// that covers it within its depth bound.
    pub fn covers(&self, path: &Path) -> bool {
        self.entry_covering(path).is_some()
    }

    /// The first entry that authorizes `path`, so a caller can name it.
    pub fn entry_covering(&self, path: &Path) -> Option<&Entry> {
        self.entries.iter().find(|e| e.covers(path))
    }

    /// Add an entry. Idempotent: adding the same path and kind twice is a no-op.
    /// Returns whether the file needs saving.
    pub fn add(&mut self, path: &Path, kind: EntryKind) -> Result<bool> {
        let path =
            std::fs::canonicalize(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
        if kind == EntryKind::Project && !path.join("opys.toml").is_file() {
            return Err(usage(format!(
                "{} has no opys.toml — pass a project directory, or add it as a prefix",
                path.display()
            )));
        }
        if self
            .entries
            .iter()
            .any(|e| e.kind == kind && e.path == path)
        {
            return Ok(false);
        }
        let mut item = toml::Table::new();
        item.insert("path".into(), contract_tilde(&path).into());
        let list = self
            .raw
            .entry(kind.key().to_string())
            .or_insert_with(|| toml::Value::Array(Vec::new()));
        match list.as_array_mut() {
            Some(arr) => arr.push(toml::Value::Table(item)),
            // The key exists but is not an array of tables: the user hand-edited
            // it into something else. Refuse rather than clobber it.
            None => {
                return Err(usage(format!(
                    "{}: `{}` is not a list of entries — fix it by hand",
                    self.path.display(),
                    kind.key()
                )))
            }
        }
        self.reparse();
        Ok(true)
    }

    /// Remove every entry (of either kind) pointing at `path`. Idempotent.
    /// Returns whether the file needs saving.
    pub fn remove(&mut self, path: &Path) -> Result<bool> {
        // Canonicalize when possible so `.`/symlinks match, but fall back to the
        // literal path so an entry whose directory is gone can still be removed.
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut changed = false;
        for kind in [EntryKind::Project, EntryKind::Prefix] {
            let Some(arr) = self
                .raw
                .get_mut(kind.key())
                .and_then(toml::Value::as_array_mut)
            else {
                continue;
            };
            let before = arr.len();
            arr.retain(|item| {
                let Some(raw) = item.get("path").and_then(toml::Value::as_str) else {
                    return true;
                };
                let expanded = expand_tilde(raw);
                let canon = std::fs::canonicalize(&expanded).unwrap_or(expanded);
                canon != target
            });
            changed |= arr.len() != before;
        }
        if changed {
            self.raw
                .retain(|_, v| !matches!(v.as_array(), Some(a) if a.is_empty()));
            self.reparse();
        }
        Ok(changed)
    }

    /// Write the file, creating parent directories.
    ///
    /// Through a temporary file and a rename, not a truncating write: the node
    /// watches this file, and `fs::write` leaves a window in which a rescan
    /// reads half an allowlist. A rename within the same directory is atomic, so
    /// a reader sees either the old file or the new one.
    /// Set the scan mode, writing through `raw` so `save` renders it.
    pub fn set_mode(&mut self, mode: ScanMode) {
        self.raw.insert(
            "mode".to_string(),
            toml::Value::String(mode.key().to_string()),
        );
        self.reparse();
    }

    /// Set (or clear) where suggestion scans start.
    ///
    /// Stored with `~` contracted back, like entry paths: the file stays legible
    /// and portable between machines whose home directories differ.
    pub fn set_scan_root(&mut self, root: Option<&Path>) {
        match root {
            Some(p) => {
                self.raw.insert(
                    "scan_root".to_string(),
                    toml::Value::String(contract_tilde(p)),
                );
            }
            None => {
                self.raw.remove("scan_root");
            }
        }
        self.reparse();
    }

    pub fn save(&self) -> Result<()> {
        let text = self.render()?;
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| usage(format!("{}: {e}", dir.display())))?;
        let tmp = self.path.with_file_name(format!(
            ".{}.{}.tmp",
            self.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "server.toml".to_string()),
            std::process::id()
        ));
        std::fs::write(&tmp, text).map_err(|e| usage(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            usage(format!("{}: {e}", self.path.display()))
        })?;
        Ok(())
    }

    /// Render `raw` back to TOML, scalars before tables.
    ///
    /// The toml serializer rejects a bare value that sorts after a table
    /// (`ValueAfterTable`), which a future scalar key like `scan_root` would
    /// trigger. Emitting each group separately sidesteps the ordering problem
    /// entirely and keeps unknown keys intact.
    fn render(&self) -> Result<String> {
        let store_err = |e: toml::ser::Error| OpysError::Store(e.to_string());
        let mut scalars = toml::Table::new();
        let mut rest: Vec<(&String, &toml::Value)> = Vec::new();
        for (key, value) in &self.raw {
            match value {
                toml::Value::Table(_) => rest.push((key, value)),
                toml::Value::Array(a) if a.iter().any(toml::Value::is_table) => {
                    rest.push((key, value))
                }
                _ => {
                    scalars.insert(key.clone(), value.clone());
                }
            }
        }
        let mut out = String::new();
        if !scalars.is_empty() {
            out.push_str(&toml::to_string(&scalars).map_err(store_err)?);
        }
        for (key, value) in rest {
            let mut one = toml::Table::new();
            one.insert(key.clone(), value.clone());
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&toml::to_string(&one).map_err(store_err)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `$HOME` is process-global, so every fixture-using test takes this lock
    /// for its whole body — otherwise parallel tests would see each other's
    /// home directory.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A registry rooted in a tempdir, with `$HOME` pointed at it so `~`
    /// handling is exercised for real.
    struct Fixture {
        _dir: tempfile::TempDir,
        _guard: std::sync::MutexGuard<'static, ()>,
        home: PathBuf,
        config: PathBuf,
    }

    impl Fixture {
        fn new() -> Fixture {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = tempfile::tempdir().unwrap();
            let home = std::fs::canonicalize(dir.path()).unwrap();
            std::env::set_var("HOME", &home);
            std::env::remove_var("XDG_CONFIG_HOME");
            let config = home.join(".config/opys/server.toml");
            Fixture {
                _dir: dir,
                _guard: guard,
                home,
                config,
            }
        }

        /// A directory under the fake home, optionally a real opys project.
        fn dir(&self, rel: &str, project: bool) -> PathBuf {
            let p = self.home.join(rel);
            std::fs::create_dir_all(&p).unwrap();
            if project {
                std::fs::write(p.join("opys.toml"), "base = \"inventory\"\n").unwrap();
            }
            p
        }

        fn load(&self) -> Registry {
            Registry::load_from(&self.config).unwrap()
        }
    }

    #[test]
    fn missing_file_is_an_empty_allowlist() {
        let fx = Fixture::new();
        let reg = fx.load();
        assert!(reg.entries.is_empty());
        assert_eq!(reg.bind, None);
    }

    #[test]
    fn malformed_file_is_an_error_not_an_empty_allowlist() {
        let fx = Fixture::new();
        std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
        std::fs::write(&fx.config, "this is not = = toml").unwrap();
        let err = Registry::load_from(&fx.config).unwrap_err();
        assert!(
            matches!(err, OpysError::Toml { .. }),
            "expected a toml error, got {err}"
        );
    }

    #[test]
    fn add_writes_a_tilde_path_and_is_idempotent() {
        let fx = Fixture::new();
        let proj = fx.dir("Projects/thing", true);

        let mut reg = fx.load();
        assert!(reg.add(&proj, EntryKind::Project).unwrap());
        reg.save().unwrap();

        let text = std::fs::read_to_string(&fx.config).unwrap();
        assert!(
            text.contains("~/Projects/thing"),
            "path should be stored tilde-relative, got: {text}"
        );

        let mut reg = fx.load();
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].path, proj);
        assert_eq!(reg.entries[0].depth, DEFAULT_DEPTH);
        assert!(
            !reg.add(&proj, EntryKind::Project).unwrap(),
            "second add is a no-op"
        );
    }

    #[test]
    fn add_rejects_a_directory_that_is_not_a_project() {
        let fx = Fixture::new();
        let plain = fx.dir("Projects/empty", false);
        let mut reg = fx.load();
        let err = reg.add(&plain, EntryKind::Project).unwrap_err();
        assert!(err.to_string().contains("no opys.toml"), "got: {err}");
        // …but the same directory is fine as a prefix.
        assert!(reg.add(&plain, EntryKind::Prefix).unwrap());
    }

    #[test]
    fn remove_is_idempotent_and_drops_the_emptied_key() {
        let fx = Fixture::new();
        let proj = fx.dir("Projects/thing", true);
        let mut reg = fx.load();
        reg.add(&proj, EntryKind::Project).unwrap();
        reg.save().unwrap();

        let mut reg = fx.load();
        assert!(reg.remove(&proj).unwrap());
        assert!(reg.entries.is_empty());
        assert!(!reg.remove(&proj).unwrap(), "second remove is a no-op");
        reg.save().unwrap();
        let text = std::fs::read_to_string(&fx.config).unwrap();
        assert!(
            !text.contains("[[project]]"),
            "emptied key should go: {text}"
        );
    }

    #[test]
    fn editing_preserves_keys_this_version_does_not_know() {
        let fx = Fixture::new();
        let proj = fx.dir("Projects/thing", true);
        std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
        std::fs::write(
            &fx.config,
            "bind = \"0.0.0.0:9999\"\nfuture_key = 7\n\n[future_table]\na = 1\n",
        )
        .unwrap();

        let mut reg = fx.load();
        assert_eq!(reg.bind.as_deref(), Some("0.0.0.0:9999"));
        reg.add(&proj, EntryKind::Project).unwrap();
        reg.save().unwrap();

        let text = std::fs::read_to_string(&fx.config).unwrap();
        assert!(text.contains("future_key = 7"), "got: {text}");
        assert!(text.contains("[future_table]"), "got: {text}");
        assert!(text.contains("bind = \"0.0.0.0:9999\""), "got: {text}");
        assert!(text.contains("[[project]]"), "got: {text}");
        // And it still round-trips.
        let reg = fx.load();
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.bind.as_deref(), Some("0.0.0.0:9999"));
    }

    #[test]
    fn a_vanished_directory_is_kept_as_an_error_entry() {
        let fx = Fixture::new();
        let proj = fx.dir("Projects/gone", true);
        let mut reg = fx.load();
        reg.add(&proj, EntryKind::Project).unwrap();
        reg.save().unwrap();
        std::fs::remove_dir_all(&proj).unwrap();

        let reg = fx.load();
        assert_eq!(reg.entries.len(), 1, "the entry must not be dropped");
        assert!(reg.entries[0].error.is_some());
        assert_eq!(reg.usable(EntryKind::Project).count(), 0);
    }

    #[test]
    fn covers_respects_kind_and_depth() {
        let fx = Fixture::new();
        let proj = fx.dir("Projects/thing", true);
        let deep = fx.dir("work/a/b/c/d", true);
        std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
        std::fs::write(
            &fx.config,
            "[[project]]\npath = \"~/Projects/thing\"\n\n[[prefix]]\npath = \"~/work\"\ndepth = 2\n",
        )
        .unwrap();

        let reg = fx.load();
        assert!(reg.covers(&proj));
        assert!(!reg.covers(&fx.home.join("Projects/other")));
        assert!(reg.covers(&fx.home.join("work/a/b")), "within depth 2");
        assert!(!reg.covers(&deep), "beyond depth 2");
    }

    #[test]
    fn config_path_prefers_xdg() {
        let fx = Fixture::new();
        std::env::set_var("XDG_CONFIG_HOME", fx.home.join("xdg"));
        let path = config_path().unwrap();
        std::env::remove_var("XDG_CONFIG_HOME");
        assert_eq!(path, fx.home.join("xdg/opys/server.toml"));
    }
}
