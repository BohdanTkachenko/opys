//! The allowlist: what the node is permitted to serve (ADR-0077).
//!
//! A TOML file the user owns, holding two kinds of entry — an explicit project
//! and a directory prefix. Nothing outside a matching entry is ever served, and
//! discovery can only *suggest* additions; it never writes here.
//!
//! `add`/`remove` edit this file and nothing else. They never speak to a running
//! node, which is what keeps filesystem paths out of the API surface entirely
//! (ADR-0052): the node watches the file instead.

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
    /// The config key an entry of this kind is written under.
    fn key(self) -> &'static str {
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

/// The parsed allowlist, plus the raw table it came from so edits preserve keys
/// this version does not know about.
#[derive(Debug, Clone)]
pub struct Registry {
    /// Where this was loaded from (and where `save` writes).
    pub path: PathBuf,
    pub bind: Option<String>,
    pub entries: Vec<Entry>,
    raw: toml::Table,
}

/// `$XDG_CONFIG_HOME/opys/server.toml`, falling back to `~/.config/opys/server.toml`.
pub fn config_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir).join("opys").join("server.toml"));
    }
    let home = home_dir().ok_or_else(|| usage("neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(home.join(".config").join("opys").join("server.toml"))
}

fn home_dir() -> Option<PathBuf> {
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
            Err(e) => return Err(e.into()),
        };
        let mut reg = Registry {
            path: path.to_path_buf(),
            bind: None,
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
        self.entries.iter().any(|e| match e.kind {
            EntryKind::Project => e.path == path,
            EntryKind::Prefix => path
                .strip_prefix(&e.path)
                .is_ok_and(|rest| rest.components().count() <= e.depth),
        })
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
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, self.render()?)?;
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
