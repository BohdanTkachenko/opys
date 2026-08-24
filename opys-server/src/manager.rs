//! The set of live corpus actors, kept in step with the allowlist (TASK-0070).
//!
//! Two loops with very different costs, per ADR-0077:
//!
//! - [`Manager::refresh`] is the frequent one (a 60 s tick). It re-checks only
//!   what is already served — one stat per corpus — and reacts to the registry
//!   file changing. It never walks the filesystem looking for new projects.
//! - [`Manager::rescan`] is the expensive one. It re-reads the allowlist and
//!   re-expands it, which for a prefix entry means a bounded scan. It belongs on
//!   a slow timer, at startup, and on demand.
//!
//! Half a second of walking every minute forever is a real cost for a result
//! that changes weekly, which is why finding is not on the fast path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use opys_engine::backend::Backend;
use opys_engine::error::Result;
use tokio::sync::broadcast;

use crate::actor::{CorpusHandle, Event};
use crate::discover::{self, ProjectGroup};
use crate::registry::Registry;

/// How a corpus actor gets its backend. Each actor needs its own instance, and
/// it must cross a thread boundary, so this is a factory rather than a value.
pub type BackendFactory = fn() -> Box<dyn Backend + Send>;

/// Owns every live corpus actor and the project grouping the API presents.
pub struct Manager {
    registry_path: PathBuf,
    /// Cheap change detection for the allowlist file: modified time and length.
    stamp: Option<(std::time::SystemTime, u64)>,
    /// Set by the registry watcher; makes an edit visible on the next tick
    /// instead of the next rescan.
    dirty: Arc<AtomicBool>,
    _watcher: Option<RecommendedWatcher>,
    corpora: BTreeMap<String, CorpusHandle>,
    groups: Vec<ProjectGroup>,
    events: broadcast::Sender<Event>,
    backend: BackendFactory,
}

impl Manager {
    /// Build a manager over `registry_path`. Nothing is served until
    /// [`Manager::rescan`] runs.
    pub fn new(
        registry_path: PathBuf,
        events: broadcast::Sender<Event>,
        backend: BackendFactory,
    ) -> Manager {
        let dirty = Arc::new(AtomicBool::new(false));
        let watcher = watch_registry(&registry_path, Arc::clone(&dirty));
        Manager {
            registry_path,
            stamp: None,
            dirty,
            _watcher: watcher,
            corpora: BTreeMap::new(),
            groups: Vec::new(),
            events,
            backend,
        }
    }

    /// The frequent tick. Rescans when the allowlist changed; otherwise just
    /// notices corpora that have gone away.
    pub fn refresh(&mut self) -> Result<()> {
        if self.dirty.swap(false, Ordering::SeqCst) || self.stamp != stamp_of(&self.registry_path) {
            return self.rescan();
        }
        let gone: Vec<String> = self
            .corpora
            .iter()
            .filter(|(_, h)| !h.corpus.root.join("opys.toml").is_file())
            .map(|(cid, _)| cid.clone())
            .collect();
        for cid in gone {
            self.drop_corpus(&cid);
        }
        Ok(())
    }

    /// The expensive pass: re-read the allowlist, re-expand it, and reconcile
    /// the live actors against the result.
    pub fn rescan(&mut self) -> Result<()> {
        let registry = Registry::load_from(&self.registry_path)?;
        self.stamp = stamp_of(&self.registry_path);
        self.dirty.store(false, Ordering::SeqCst);

        let groups = discover::expand(&registry);
        let desired: BTreeMap<String, crate::discover::Corpus> = groups
            .iter()
            .flat_map(|g| g.corpora.iter().map(|c| (c.cid.clone(), c.clone())))
            .collect();

        let removed: Vec<String> = self
            .corpora
            .keys()
            .filter(|cid| !desired.contains_key(*cid))
            .cloned()
            .collect();
        for cid in removed {
            self.drop_corpus(&cid);
        }

        for (cid, corpus) in desired {
            if self.corpora.contains_key(&cid) {
                continue;
            }
            let handle = CorpusHandle::spawn(corpus, (self.backend)(), self.events.clone());
            self.corpora.insert(cid.clone(), handle);
            let _ = self.events.send(Event::CorpusAdded { cid });
        }
        self.groups = groups;
        self.prune_groups();
        Ok(())
    }

    /// Stop one corpus and announce it.
    fn drop_corpus(&mut self, cid: &str) {
        if let Some(handle) = self.corpora.remove(cid) {
            handle.shutdown();
            let _ = self.events.send(Event::CorpusRemoved {
                cid: cid.to_string(),
            });
        }
        self.prune_groups();
    }

    /// Keep the presented grouping in step with what is actually running.
    fn prune_groups(&mut self) {
        let live: BTreeSet<&String> = self.corpora.keys().collect();
        let live: BTreeSet<String> = live.into_iter().cloned().collect();
        for group in &mut self.groups {
            group.corpora.retain(|c| live.contains(&c.cid));
        }
        self.groups.retain(|g| !g.corpora.is_empty());
    }

    /// The corpus with this id, if it is being served.
    pub fn get(&self, cid: &str) -> Option<&CorpusHandle> {
        self.corpora.get(cid)
    }

    /// Every live corpus id, sorted.
    pub fn cids(&self) -> Vec<String> {
        self.corpora.keys().cloned().collect()
    }

    /// The project grouping to present, as of the last rescan.
    pub fn groups(&self) -> &[ProjectGroup] {
        &self.groups
    }

    /// How many corpora are being served.
    pub fn len(&self) -> usize {
        self.corpora.len()
    }

    /// Whether nothing is being served — the state of a fresh install, since an
    /// empty allowlist serves nothing.
    pub fn is_empty(&self) -> bool {
        self.corpora.is_empty()
    }

    /// Stop every actor and wait for their threads.
    pub fn shutdown(mut self) {
        for (_, handle) in std::mem::take(&mut self.corpora) {
            handle.shutdown();
        }
    }
}

/// Modified time and length: enough to notice an edit without reading the file.
fn stamp_of(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Watch the allowlist file so `opys web add` shows up on the next tick rather
/// than the next rescan.
///
/// The *directory* is watched, not the file: an editor that writes by rename
/// replaces the inode, and a watch on the old one would go quiet. Returns
/// `None` when no watcher can be established, which degrades to the stamp check
/// in [`Manager::refresh`].
fn watch_registry(path: &Path, dirty: Arc<AtomicBool>) -> Option<RecommendedWatcher> {
    let dir = path.parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    let target = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if event.paths.contains(&target) {
            dirty.store(true, Ordering::SeqCst);
        }
    })
    .ok()?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}
