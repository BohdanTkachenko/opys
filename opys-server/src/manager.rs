//! The set of live corpus actors, kept in step with the allowlist.
//!
//! Two loops with very different costs, per ADR-0031:
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
use crate::refreader::{self, Baseline, RefReaderHandle, Served, Spawn};
use crate::registry::Registry;
use crate::union::Branches;

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
    /// One ref reader per project group, keyed by [`ProjectGroup::key`]. Started
    /// after the corpus actors, because a reader is handed a client per corpus,
    /// and stopped before them on shutdown.
    readers: BTreeMap<String, RefReaderHandle>,
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
            readers: BTreeMap::new(),
            groups: Vec::new(),
            events,
            backend,
        }
    }

    /// The allowlist file this manager serves from.
    pub fn registry_path(&self) -> &std::path::Path {
        &self.registry_path
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
        self.reconcile_readers();
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
        self.reconcile_readers();
        Ok(())
    }

    /// Keep one ref reader per served group, started for the group as it is now.
    ///
    /// A reader captures two things it cannot be told about later — the group's
    /// corpora, so it knows which refs are already columns, and a client per
    /// corpus, so it can read whichever of them is main — which is why a group
    /// whose shape moved gets a *new* reader rather than a nudge. The shape is
    /// compared as a string so a worktree appearing, vanishing or changing
    /// branch all count.
    ///
    /// Every running corpus is handed over as a [`Baseline`], not just the
    /// primary: the corpus to measure branches against is the one sitting on the
    /// truth branch, which git's main worktree need not be, and only the reader
    /// — which resolves `[git] main` and every checkout's head on each pass —
    /// can say which that is. A group with no corpus running yet gets no reader
    /// at all: with no baseline to compare against, every branch would look like
    /// the whole inventory. The next pass picks it up.
    fn reconcile_readers(&mut self) {
        let wanted: BTreeMap<String, String> = self
            .groups
            .iter()
            .map(|group| (group.key.clone(), shape_of(group)))
            .collect();
        self.readers
            .retain(|key, reader| wanted.get(key).is_some_and(|shape| *shape == reader.shape));
        for group in &self.groups {
            if self.readers.contains_key(&group.key) {
                continue;
            }
            let primary = group
                .corpora
                .iter()
                .find(|c| c.is_primary)
                .or_else(|| group.corpora.first());
            let Some(primary) = primary else { continue };
            let baselines: Vec<Baseline> = group
                .corpora
                .iter()
                .filter_map(|c| {
                    let client = self.corpora.get(&c.cid).map(CorpusHandle::client)?;
                    Some(Baseline {
                        root: c.root.clone(),
                        docs: Box::new(move || {
                            client
                                .docs(crate::actor::DocFilter::default())
                                .map_err(|e| e.to_string())
                        }),
                    })
                })
                .collect();
            if baselines.is_empty() {
                continue;
            }
            let served: Vec<Served> = group
                .corpora
                .iter()
                .map(|c| Served {
                    branch: c.branch.clone(),
                    root: c.root.clone(),
                })
                .collect();
            let reader = RefReaderHandle::spawn(Spawn {
                key: group.key.clone(),
                root: primary.root.clone(),
                served,
                baselines,
                events: self.events.clone(),
                shape: shape_of(group),
                backstop: refreader::BACKSTOP,
            });
            self.readers.insert(group.key.clone(), reader);
        }
    }

    /// What the ref reader of this group last read: its branch columns, the
    /// occupancy behind them and any notice. Empty for a group with no reader —
    /// which is the same shape as "read, and there are no branches", on purpose:
    /// a union view must render either way.
    ///
    /// The `Arc` the reader publishes, not a copy of what is inside it. Each
    /// branch column carries main's whole inventory overlaid, so a deep clone is
    /// O(branches x documents) `DocSummary`s — and this is called under the
    /// manager lock, on a route the sidebar polls and every `RefsChanged` frame
    /// re-triggers, sometimes only to read `columns.len()`. The caller clones
    /// the part it actually reshapes.
    pub fn branches_for(&self, key: &str) -> Arc<Branches> {
        self.readers
            .get(key)
            .map(|reader| reader.branches())
            .unwrap_or_default()
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

    /// How to build a backend for a caller that is not an actor.
    ///
    /// The write path needs one (`crate::action`), and it must be a *fresh*
    /// instance over a *fresh* store: reusing an actor's would mean writing
    /// through the warm cache, which is the one thing the node must never do.
    /// Exposing the factory rather than a backend keeps the choice of medium
    /// where the binary put it.
    pub fn backend(&self) -> BackendFactory {
        self.backend
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
    ///
    /// Readers first: one may be blocked on its primary corpus's client, and a
    /// corpus actor that has already stopped answers that immediately, whereas
    /// the reverse order would have each reader wait out its own channel.
    pub fn shutdown(mut self) {
        for (_, reader) in std::mem::take(&mut self.readers) {
            reader.shutdown();
        }
        for (_, handle) in std::mem::take(&mut self.corpora) {
            handle.shutdown();
        }
    }
}

/// A group's shape as one comparable string: every corpus, in order, with the
/// branch it has checked out and whether it is the primary.
fn shape_of(group: &ProjectGroup) -> String {
    group
        .corpora
        .iter()
        .map(|c| {
            format!(
                "{}\u{1}{}\u{1}{}",
                c.cid,
                c.branch.as_deref().unwrap_or_default(),
                c.is_primary
            )
        })
        .collect::<Vec<_>>()
        .join("\u{2}")
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
