//! One ref-reader thread per project group: what the branches hold (ADR-0053).
//!
//! A project group is a repository's worktrees, and the node has a corpus actor
//! per worktree — a directory, a watcher, a warm store. A branch nobody has
//! checked out has none of those: its documents exist only as git objects. This
//! is the producer for those, so the union view can show a column per branch and
//! a row can say which refs hold it at a status main's copy does not.
//!
//! **It is a read and nothing else.** Nothing here writes to the repository, to
//! the corpus, or to the registry; the node initiates no writes at all
//! (ADR-0027). A failure degrades to a notice or a column error and never to a
//! failed request.
//!
//! Three decisions worth stating, because they are not the corpus actor's:
//!
//! - **The result lives behind a mutex, not behind the channel.** The corpus
//!   actor answers reads over its request channel, which means a read can queue
//!   behind an in-flight reload — bounded by `api::UNION_TIMEOUT`. A git pass is
//!   the same kind of work but nothing about it is per-request: it depends only
//!   on the refs. So the thread *publishes* a [`Branches`] snapshot into a
//!   mutex the manager holds, and a request is a lock and a clone. The channel
//!   carries only commands *into* the thread.
//! - **A pass runs once per ref change, not once per request.** On start, on a
//!   debounced change under the git directory, and every [`BACKSTOP`] as a
//!   floor. The event is the invalidation. The backstop is not decoration: a
//!   document's occupancy also moves when *main's working copy* does, and main's
//!   inventory is not under `.git`, so no ref event ever fires for it — sixty
//!   seconds is the staleness bound on that one case, and wiring this thread
//!   into the corpus broadcast to close it would trade a bounded staleness for a
//!   git walk on every keystroke-driven reload.
//! - **A project that is no git repository runs no thread.** The first pass
//!   publishes its one notice and the thread returns: with no git directory to
//!   watch, every later pass would produce the same answer. A directory that
//!   later becomes a repository is picked up when the manager next rebuilds the
//!   group, and that is the accepted cost. A repository that *is* there and
//!   cannot be opened is a different state and keeps its thread: nothing would
//!   restart it, since the manager keeps a reader whose group shape has not
//!   moved.
//! - **Main is a corpus the reader chooses, not the one it was handed.**
//!   Occupancy is "a status main's copy does not have" and a branch column is
//!   main's inventory with a diff applied, so both are measured against the
//!   served corpus sitting on the truth branch — picked per pass inside
//!   [`git::survey_with`], where `[git] main` and every checkout's head are
//!   known. When the node serves no checkout of main, that is a notice and no
//!   branch columns, never a column filled in from some third branch.
//!
//! The event is published only when the answer actually changed. The client
//! treats a frame it cannot attribute to a corpus as affecting every view, so an
//! unconditional broadcast would turn the backstop into a node-wide refetch
//! every minute in every open tab.
//!
//! `gix::Repository` never leaves this thread: it is opened inside each pass by
//! the engine's reader and dropped there, the same rule the corpus actor follows
//! for its `Store`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use opys_engine::commands::now_rfc3339;
use opys_engine::doc::Doc;
use opys_engine::git;
use opys_engine::project::Project;
use opys_engine::refs;
use tokio::sync::broadcast;

use crate::actor::{summary, DocSummary, Event, DEBOUNCE};
use crate::union::{BranchColumn, BranchDocs, Branches};

/// How long a quiet reader waits before reading anyway.
///
/// The ref watcher cannot see main's working copy, and occupancy is defined
/// against it — so this is the bound on how stale an occupancy chip can be after
/// an edit that touched no ref. See the module docs.
pub const BACKSTOP: Duration = Duration::from_secs(60);

/// Main's documents, as the reader asks for them each pass.
///
/// A closure rather than a `CorpusClient` so the reader owes nothing to the
/// actor module's shape, and so its own tests can hand it a fixed corpus without
/// standing up an actor. In the node it closes over one corpus's client;
/// blocking on that actor from this thread is safe, because no edge runs the
/// other way.
pub type MainDocs = Box<dyn Fn() -> Result<Vec<DocSummary>, String> + Send>;

/// One corpus the reader may measure branches against, if it turns out to be
/// the one sitting on the truth branch.
///
/// Which corpus that is cannot be decided when the reader starts: it depends on
/// `[git] main`, on what each checkout's `HEAD` says, and both can move under a
/// running node. So the manager hands over *every* corpus of the group and the
/// reader picks per pass — see [`Reader::read`]. The alternative, taking the
/// group's primary corpus (git's main worktree, on whatever branch it happens to
/// sit), silently measured every branch against a third branch's working copy.
pub struct Baseline {
    /// The corpus root, matched against [`opys_engine::git::Checkout::root`].
    pub root: PathBuf,
    /// How to ask that corpus for its documents.
    pub docs: MainDocs,
}

/// One corpus the node already serves, as far as the filter below cares.
///
/// A ref that one of these has checked out gets no branch column: the checkout
/// *is* a column, with a live watcher and a warm store behind it, and two
/// columns for one commit would double-count the work in flight.
#[derive(Debug, Clone)]
pub struct Served {
    /// The branch git reports for this corpus, if any. Filled even for a
    /// detached HEAD sitting on a branch tip, which is the jj-colocated case.
    pub branch: Option<String>,
    /// The corpus root — under the worktree's checkout directory, and equal to
    /// it unless the project is nested inside the repository.
    pub root: PathBuf,
}

/// What the thread should do after a pass.
enum Outcome {
    /// There is a git directory: watch it (if not already) and carry on.
    Watch(PathBuf),
    /// Nothing to watch and nothing that could change the answer without the
    /// manager rebuilding the group — a project that is no repository, or a
    /// build with no `git` feature. The thread ends.
    Stop,
    /// This pass could not tell: the project's config would not open, or a
    /// repository was found and would not open. Both are routinely transient (an
    /// editor mid-save, a half-written `.git/config`), and neither is answered
    /// by ending the thread — nothing would ever restart it, because the
    /// manager keeps a reader whose group shape has not moved. Keep the thread
    /// and its backstop, and install the watcher when a later pass succeeds.
    Retry,
}

enum Msg {
    /// Something changed under the git directory.
    Fs,
    /// Read now and reply when the snapshot has been published — the
    /// deterministic path for tests.
    Read(SyncSender<()>),
    Shutdown,
}

/// Everything one reader is started with.
///
/// A struct rather than seven positional arguments, and the reason there are
/// seven is [`Spawn::backstop`]: a timer measured in minutes is not observable
/// from a test that must finish in milliseconds, so it is an input like the
/// rest rather than a constant read from inside.
pub struct Spawn {
    /// The project group's key — what a [`Event::RefsChanged`] names.
    pub key: String,
    /// The project root to open each pass. Any corpus of the group will do;
    /// the manager passes the primary's.
    pub root: PathBuf,
    /// Every corpus the node serves for this group, for the checked-out filter.
    pub served: Vec<Served>,
    /// Every corpus the reader may measure against, in group order.
    pub baselines: Vec<Baseline>,
    pub events: broadcast::Sender<Event>,
    /// What the group looked like when this reader started; see
    /// [`RefReaderHandle::shape`].
    pub shape: String,
    /// How long a quiet reader waits before reading anyway. [`BACKSTOP`] in the
    /// node.
    pub backstop: Duration,
}

/// A handle to one project's ref reader: the thread, its published snapshot, and
/// the shape of the group it was started for.
pub struct RefReaderHandle {
    snapshot: Arc<Mutex<Arc<Branches>>>,
    tx: mpsc::Sender<Msg>,
    join: Option<JoinHandle<()>>,
    /// What the group looked like when this reader started. The reader captured
    /// the group's corpora (to filter checked-out refs) and its primary's
    /// client, neither of which it can be told about afterwards, so the manager
    /// restarts it when this changes rather than serving a stale filter.
    pub shape: String,
}

impl RefReaderHandle {
    /// Start a reader for one project group. The first pass runs on the thread,
    /// so this returns immediately and the snapshot fills in behind it.
    pub fn spawn(spec: Spawn) -> RefReaderHandle {
        let Spawn {
            key,
            root,
            served,
            baselines,
            events,
            shape,
            backstop,
        } = spec;
        let (tx, rx) = mpsc::channel::<Msg>();
        let watch_tx = tx.clone();
        let snapshot = Arc::new(Mutex::new(Arc::new(Branches::default())));
        let thread_snapshot = Arc::clone(&snapshot);
        let thread_key = key.clone();
        let join = std::thread::Builder::new()
            .name(format!("opys-refs-{key}"))
            .spawn(move || {
                let mut reader = Reader {
                    key: thread_key,
                    root,
                    served,
                    baselines,
                    events,
                    snapshot: thread_snapshot,
                    backstop,
                };
                // Held for the life of the loop: dropping a notify watcher stops
                // it delivering. `None` until a pass finds a git directory, and
                // when a watcher cannot be established at all the reader runs on
                // its backstop alone rather than refusing to serve branches.
                let mut watch: Option<Watch> = None;
                // The watcher is armed *between* the first read and its
                // announcement, so nothing can change under the git directory
                // in the window where a client has been told the answer and
                // nothing is watching for the next one. Arming it before the
                // read instead would only make the first pass race itself.
                let (next, outcome) = reader.read();
                if let Outcome::Watch(dir) = &outcome {
                    watch = spawn_watcher(dir, watch_tx.clone());
                }
                reader.publish(next);
                if matches!(outcome, Outcome::Stop) {
                    return;
                }
                reader.run(rx, &watch_tx, &mut watch);
            })
            .expect("spawning a ref reader thread");
        RefReaderHandle {
            snapshot,
            tx,
            join: Some(join),
            shape,
        }
    }

    /// The last published answer. A lock and a clone of an `Arc`; never blocks
    /// on git.
    pub fn branches(&self) -> Arc<Branches> {
        match self.snapshot.lock() {
            Ok(guard) => Arc::clone(&guard),
            // A panic in the reader thread while publishing would poison this.
            // The data behind it is still a valid snapshot, and a view that
            // refuses to render because a background thread died is worse than
            // one branch column short.
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Read now and wait for the snapshot — for tests and for a caller that
    /// wants a fresh answer rather than the last one. `false` if the thread is
    /// gone (which includes the ordinary "this project is no repository" exit).
    pub fn read_now(&self) -> bool {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.send(Msg::Read(tx)).is_ok() && rx.recv().is_ok()
    }

    /// Stop the reader and wait for its thread.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RefReaderHandle {
    fn drop(&mut self) {
        // An explicit message, not channel closure: the watcher callback owns a
        // cloned sender that lives *inside* the thread, so the receiver never
        // sees a disconnect on its own. No join — a `Drop` that blocked on a git
        // walk would be a surprising place to spend a second.
        let _ = self.tx.send(Msg::Shutdown);
    }
}

struct Reader {
    key: String,
    root: PathBuf,
    served: Vec<Served>,
    baselines: Vec<Baseline>,
    events: broadcast::Sender<Event>,
    snapshot: Arc<Mutex<Arc<Branches>>>,
    backstop: Duration,
}

/// The reader's notify watcher, and whether its second watch is attached.
///
/// `<git-dir>/worktrees` frequently does not exist when a reader starts — a
/// repository grows its first linked worktree (or its first `jj` workspace)
/// later — and the failed watch has to be *retried*, not forgotten: a reader
/// that gave up on it would never see a workspace head move, which under jj is
/// the only thing that moves at all.
struct Watch {
    watcher: RecommendedWatcher,
    /// Whether `<git-dir>/worktrees` is being watched.
    worktrees: bool,
}

impl Reader {
    /// The debounce-and-backstop loop.
    ///
    /// One thread, one timer: the wake-up is whichever of the settling deadline
    /// and the backstop comes first, so a burst of ref writes collapses into one
    /// pass without a second thread to coordinate with.
    fn run(
        &mut self,
        rx: mpsc::Receiver<Msg>,
        watch_tx: &mpsc::Sender<Msg>,
        watch: &mut Option<Watch>,
    ) {
        let mut backstop = Instant::now() + self.backstop;
        // `Some(deadline)` means a burst under the git directory is settling.
        let mut quiet_until: Option<Instant> = None;
        loop {
            let wake = match quiet_until {
                Some(deadline) => deadline.min(backstop),
                None => backstop,
            };
            let reply = match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(Msg::Fs) => {
                    quiet_until = Some(Instant::now() + DEBOUNCE);
                    continue;
                }
                Ok(Msg::Read(reply)) => Some(reply),
                Ok(Msg::Shutdown) => break,
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let outcome = self.pass();
            quiet_until = None;
            backstop = Instant::now() + self.backstop;
            let stop = match outcome {
                Outcome::Stop => true,
                // A repository that has only now become readable: start watching
                // it rather than living on the backstop forever. And a
                // repository that has only now grown a `worktrees` directory:
                // attach the watch that could not be attached before.
                Outcome::Watch(dir) => {
                    match watch {
                        None => *watch = spawn_watcher(&dir, watch_tx.clone()),
                        Some(watch) => watch_worktrees(watch, &dir),
                    }
                    false
                }
                Outcome::Retry => false,
            };
            // After the watcher, never before: a caller told the answer must not
            // be able to change a ref in a window where nothing is watching for
            // it — the same rule the first pass follows.
            if let Some(reply) = reply {
                let _ = reply.send(());
            }
            if stop {
                break;
            }
        }
    }

    /// One read: survey the refs, build the columns, publish, and announce it if
    /// the answer moved. What the thread should do next comes back with it.
    ///
    /// The two halves are separate ([`Reader::read`], [`Reader::publish`]) so
    /// the very first pass can arm the watcher in between: a client that acts on
    /// the first announcement must not be able to change a ref before anything
    /// is watching for it.
    fn pass(&mut self) -> Outcome {
        let (next, outcome) = self.read();
        self.publish(next);
        outcome
    }

    /// Store a fresh answer and announce it, but only if it moved.
    fn publish(&self, next: Branches) {
        let changed = {
            let mut guard = match self.snapshot.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let changed = !guard.same_answer(&next);
            *guard = Arc::new(next);
            changed
        };
        if changed {
            let _ = self.events.send(Event::RefsChanged {
                project: self.key.clone(),
            });
        }
    }

    fn read(&self) -> (Branches, Outcome) {
        let read_at = Some(now_rfc3339());
        let prj = match Project::open(&self.root.to_string_lossy()) {
            Ok(prj) => prj,
            // The corpus actor reports the same config problem against the
            // corpus itself; saying it once more here, as a notice, is what
            // stops the union claiming this project simply has no branches.
            Err(e) => {
                return (
                    Branches {
                        notices: vec![format!("branches could not be read: {e}")],
                        read_at,
                        ..Branches::default()
                    },
                    Outcome::Retry,
                )
            }
        };

        // Main's documents are the baseline for both halves: occupancy compares
        // a ref's status against main's, and a branch column *is* main's
        // inventory with the ref's changes applied. *Which* corpus holds main's
        // copy is decided inside the survey, where the truth branch and every
        // checkout's head are known — see `Baseline`.
        // The truth branch, once the survey has resolved it, and — when a served
        // corpus is on it — what that corpus answered. Both are set inside the
        // callback because only the survey can say either.
        let mut measured: Option<String> = None;
        let mut main: Option<(String, Result<Vec<DocSummary>, String>)> = None;
        let surveyed = git::survey_with(&prj, |truth, checkouts| {
            measured = Some(truth.short.clone());
            let baseline = self.baseline(truth, checkouts)?;
            let docs = (baseline.docs)();
            let statuses: BTreeMap<String, String> = docs
                .iter()
                .flatten()
                .map(|d| (d.id.clone(), d.status.clone()))
                .collect();
            let readable = docs.is_ok();
            main = Some((truth.short.clone(), docs));
            // A baseline that could not be read is no baseline: comparing
            // against nothing would report every branch as holding the whole
            // inventory. The notice below says so instead.
            readable.then_some(statuses)
        });
        let outcome = match (&surveyed.git_dir, surveyed.retry) {
            (Some(dir), _) => Outcome::Watch(dir.clone()),
            // Discovered and unreadable: this pass could not tell, so the thread
            // stays. Only "there is no repository" is forever.
            (None, true) => Outcome::Retry,
            (None, false) => Outcome::Stop,
        };
        let mut notices = surveyed.notices;
        // Computed before every early return below, not after them: the pass
        // that finds no served worktree on main is exactly the pass whose notice
        // a stale column heading contradicts.
        let branch_of = branch_of(&self.served, &surveyed.checkouts);

        let main = match main {
            Some((_, Ok(main))) => main,
            Some((truth, Err(e))) => {
                notices.push(format!(
                    "the worktree on `{truth}` could not be read, so branches cannot be \
                     compared against it: {e}"
                ));
                return (
                    Branches {
                        notices,
                        read_at,
                        branch_of,
                        ..Branches::default()
                    },
                    outcome,
                );
            }
            // No baseline was asked for (no repository, no truth branch: the
            // survey said so itself), or none of the served corpora is the
            // checkout of main that occupancy is defined against.
            None => {
                if let Some(truth) = measured {
                    notices.push(format!(
                        "no served worktree is on `{truth}` — branches are compared against \
                         main's copy, so there is nothing here to compare them with"
                    ));
                }
                return (
                    Branches {
                        notices,
                        read_at,
                        branch_of,
                        ..Branches::default()
                    },
                    outcome,
                );
            }
        };

        let occupied_commits = served_commits(&self.served, &surveyed.checkouts);
        let mut columns: Vec<BranchDocs> = Vec::new();
        let mut occupancy: BTreeMap<String, Vec<opys_engine::git::Occupant>> = BTreeMap::new();
        for state in surveyed.refs {
            if checked_out(&state.reference, &self.served, &occupied_commits) {
                continue;
            }
            let column = BranchColumn {
                key: format!("ref-{}", state.reference.commit),
                label: state.reference.label.clone(),
                commit: state.reference.commit.clone(),
            };
            let docs = match &state.changed {
                // A ref whose diff broke keeps its column, with the reason: a
                // column that vanished would read as a branch that never was.
                Err(e) => Err(e.clone()),
                Ok(changed) => Ok(overlay(&prj, &main, changed)),
            };
            if docs.is_ok() {
                for (id, occupant) in state.occupied {
                    occupancy.entry(id).or_default().push(occupant);
                }
            }
            columns.push((column, docs));
        }

        (
            Branches {
                columns,
                occupancy,
                notices,
                read_at,
                branch_of,
            },
            outcome,
        )
    }

    /// The corpus whose working copy *is* main's, or `None` when the node serves
    /// none.
    ///
    /// A corpus qualifies when it sits in a checkout that is on the truth branch
    /// — by branch name, or, for the detached head a jj-colocated repository
    /// leaves everywhere, by commit ([`git::Checkout::is_on`]). Group order
    /// breaks a tie, which puts the primary first.
    fn baseline(&self, truth: &git::Truth, checkouts: &[git::Checkout]) -> Option<&Baseline> {
        let on_main: Vec<&Path> = checkouts
            .iter()
            .filter(|c| c.is_on(truth))
            .map(|c| c.root.as_path())
            .collect();
        self.baselines
            .iter()
            .find(|b| on_main.iter().any(|root| within(root, &b.root)))
    }
}

/// Whether `path` is `dir` or something under it — the relation a corpus root
/// has to the checkout directory it lives in.
///
/// Canonicalized on the way, and only if the plain comparison fails: gix
/// canonicalizes a work directory while discovery's roots come from `git
/// worktree list`, and a symlinked `TMPDIR` (or `/home` on a machine where it is
/// one) is enough to make two spellings of one directory miss.
fn within(dir: &Path, path: &Path) -> bool {
    if path.starts_with(dir) {
        return true;
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(path).starts_with(canon(dir))
}

/// The commits the corpora this node serves are sitting on.
///
/// One per served corpus, taken from the *longest* checkout directory that
/// contains it — nested worktrees are pathological but the innermost one is the
/// checkout that corpus is really in.
fn served_commits(served: &[Served], checkouts: &[git::Checkout]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for corpus in served {
        let found = checkouts
            .iter()
            .filter(|c| within(&c.root, &corpus.root))
            .max_by_key(|c| c.root.as_os_str().len());
        if let Some(commit) = found.and_then(|c| c.commit.clone()) {
            out.insert(commit);
        }
    }
    out
}

/// The branch each served corpus's checkout is on, this pass.
///
/// Same rule as [`served_commits`] — the *longest* checkout directory containing
/// the corpus — so a corpus nested inside a worktree is answered for by the
/// worktree it is really in. A corpus no checkout contains gets no entry rather
/// than `None`: "the reader has no answer" and "the checkout is on no branch"
/// are different, and only the second may overwrite what discovery recorded.
fn branch_of(served: &[Served], checkouts: &[git::Checkout]) -> BTreeMap<PathBuf, Option<String>> {
    let mut out = BTreeMap::new();
    for corpus in served {
        let found = checkouts
            .iter()
            .filter(|c| within(&c.root, &corpus.root))
            .max_by_key(|c| c.root.as_os_str().len());
        if let Some(checkout) = found {
            out.insert(corpus.root.clone(), checkout.branch.clone());
        }
    }
    out
}

/// Whether one of the corpora the node already serves has this ref checked out.
///
/// By **commit**, which is the definition the ref set is deduplicated by: one
/// commit is one column, and a corpus sitting on that commit is already that
/// column. `commits` is [`served_commits`] — every served corpus's checkout head
/// — and it is the rule that decides, because the alternatives each have a hole.
/// A name comparison sees only [`git::Ref::label`], which is the alphabetically
/// first of however many branches point at the commit, so an ordinary leftover
/// branch at the same commit produced a *second* column duplicating a checkout.
/// A path comparison cannot see the repository's main worktree at all, because
/// `gix` lists only the linked ones.
///
/// The other two rules are kept behind it, for the pass that could not read the
/// checkouts (an empty `commits` set) — a name match on `Corpus::branch`, and a
/// path match against the linked-worktree directories the ref itself carries.
///
/// A worktree that carries no `opys.toml` is not a served corpus and so is not
/// matched by any of them — its ref does get a column, which is right: nothing
/// else in the view shows it.
fn checked_out(r: &git::Ref, served: &[Served], commits: &BTreeSet<String>) -> bool {
    commits.contains(&r.commit)
        || served.iter().any(|corpus| {
            corpus.branch.as_deref() == Some(r.label.as_str())
                || r.worktrees.iter().any(|wt| corpus.root.starts_with(wt))
        })
}

/// One ref's inventory as summaries: main's documents, minus what the ref
/// deleted, with what the ref changed applied over the top.
///
/// The overlay is what makes a branch column comparable to a worktree column.
/// `docs_changed_vs_main` returns only the *diff* — feeding that to the merge
/// directly would render every document the branch left alone as "not here",
/// which is the view claiming the branch deleted the inventory.
///
/// The one approximation, stated because it is invisible otherwise: "left alone"
/// means *identical to main's committed blob*, while the documents it is filled
/// in from are the working copy of the checkout that is on main. They differ
/// exactly while that checkout has uncommitted edits, and then a branch column
/// echoes those edits on the rows it did not touch. The alternative — decoding
/// main's whole tree on every pass — costs the property the reader is built
/// around, that an unchanged document is never parsed. The union's legend says
/// this in the user's words ("reads as main's working copy"); keep the two in
/// step.
fn overlay(prj: &Project, main: &[DocSummary], changed: &git::Changed) -> Vec<DocSummary> {
    let deleted: BTreeSet<&str> = changed.deleted.iter().map(String::as_str).collect();
    let mut moved: BTreeMap<&str, &Doc> = BTreeMap::new();
    for doc in &changed.docs {
        if let Some(id) = doc.id() {
            moved.insert(id, doc);
        }
    }
    let kept: Vec<&DocSummary> = main
        .iter()
        .filter(|d| !deleted.contains(d.id.as_str()) && !moved.contains_key(d.id.as_str()))
        .collect();

    // `blocked`, `live_blockers` and `blocks` are properties of the *corpus*,
    // not of a document on its own (ADR-0050 decision 3), so the index the
    // changed documents are summarised against is the overlaid one — this ref's
    // statuses, not main's. The documents the ref left alone keep main's own
    // summary, which is the same approximation as their status: they are
    // identical to main's committed copy.
    let mut states: Vec<(&str, &str)> = kept
        .iter()
        .map(|d| (d.id.as_str(), d.status.as_str()))
        .collect();
    states.extend(
        moved
            .iter()
            .map(|(id, doc)| (*id, doc.status().unwrap_or_default())),
    );
    let corpus = refs::Corpus::new(&prj.pcfg, states.iter().copied());

    let mut out: Vec<DocSummary> = kept.into_iter().cloned().collect();
    out.extend(moved.values().filter_map(|doc| summary(prj, doc, &corpus)));
    out
}

/// Watch what determines the ref set: the loose refs, the linked worktree heads,
/// and the git directory itself for `packed-refs`.
///
/// Returns `None` when no watcher can be established, which degrades to a reader
/// that only runs its backstop — worth strictly more than refusing to serve the
/// branches. `<git-dir>/worktrees` frequently does not exist, so its watch is
/// allowed to fail here and is retried by [`watch_worktrees`] on every later
/// pass until it takes.
///
/// The git directory is watched **non-recursively** on purpose: `objects/`
/// churns thousands of events on a single fetch, and none of them can change the
/// ref set on their own.
fn spawn_watcher(git_dir: &Path, tx: mpsc::Sender<Msg>) -> Option<Watch> {
    let dir = git_dir.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if is_change(&dir, &event) {
            // The reader debounces; this only has to be cheap and non-blocking.
            let _ = tx.send(Msg::Fs);
        }
    })
    .ok()?;
    watcher.watch(git_dir, RecursiveMode::NonRecursive).ok()?;
    let _ = watcher.watch(&git_dir.join("refs"), RecursiveMode::Recursive);
    let mut watch = Watch {
        watcher,
        worktrees: false,
    };
    watch_worktrees(&mut watch, git_dir);
    Some(watch)
}

/// Attach the `<git-dir>/worktrees` watch if it is not attached yet.
///
/// Idempotent and cheap, and called on every pass, because the directory is
/// created when the repository gets its first linked worktree — long after most
/// readers start. Without the retry a node that came up before the first `git
/// worktree add` (or the first `jj workspace add`) would never see a workspace
/// head move again, which is the one thing that moves in a jj-colocated
/// repository: its heads are detached, so no `refs/` write accompanies them.
fn watch_worktrees(watch: &mut Watch, git_dir: &Path) {
    if watch.worktrees {
        return;
    }
    watch.worktrees = watch
        .watcher
        .watch(&git_dir.join("worktrees"), RecursiveMode::Recursive)
        .is_ok();
}

/// Whether a filesystem event under the git directory should cause a read.
///
/// **Reads are not changes.** A pass reads `refs/`, `packed-refs` and every
/// `worktrees/*/HEAD`, and inotify reports opening a file — so counting
/// `Access` events would make this thread read the repository in a loop, forever,
/// for as long as the node runs. It is the same trap `crate::actor::is_change`
/// documents, and `reads_are_not_ref_changes` pins it.
fn is_change(git_dir: &Path, event: &notify::Event) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|p| is_ref_path(git_dir, p))
}

/// Whether a changed path can change which commits hold the inventory.
///
/// Matched as a position *relative to the watched git directory*, because every
/// segment here is a name a repository could also be living under: a checkout
/// inside a directory called `refs` is not a ref.
///
/// Four shapes count because they are the four a pass reads:
/// `<git-dir>/packed-refs`, anything under `<git-dir>/refs/` (a branch may be
/// called anything, at any depth), `<git-dir>/worktrees/<name>/HEAD`, and
/// `<git-dir>/HEAD` — the **main** worktree's, which is what says which branch
/// that checkout is on, and so which of the served corpora is main's copy and
/// what its column is called. Leaving it out left both answers to the backstop,
/// a minute behind a `git checkout`, while the identical file one directory down
/// was watched. A fifth counts because of *when* it happens: `<git-dir>/worktrees`
/// itself appearing is the repository's first linked worktree, which both
/// changes the ref set and is the moment [`watch_worktrees`] can finally attach.
///
/// Everything else is churn. The git directory is watched non-recursively for
/// `packed-refs`, and that watch also reports `index`, `COMMIT_EDITMSG`,
/// `ORIG_HEAD` and `FETCH_HEAD` — every one of which a commit, a fetch or a `jj`
/// command rewrites constantly. A linked worktree's admin directory is watched
/// recursively for its `HEAD` and holds its own `index`, `ORIG_HEAD` and
/// `logs/`, rewritten just as often by ordinary work in that checkout; only its
/// `HEAD` names a commit. A lock file is git's write-then-rename dance and the
/// rename that follows is the real event.
fn is_ref_path(git_dir: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.ends_with(".lock") {
        return false;
    }
    let Ok(rel) = path.strip_prefix(git_dir) else {
        return false;
    };
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(s) => parts.push(s),
            // `..` and friends cannot appear in an inotify path, and a relative
            // position that is not a plain chain of names is not one of the
            // three shapes above.
            _ => return false,
        }
    }
    let refs = std::ffi::OsStr::new("refs");
    let packed = std::ffi::OsStr::new("packed-refs");
    let worktrees = std::ffi::OsStr::new("worktrees");
    let head = std::ffi::OsStr::new("HEAD");
    match parts.as_slice() {
        // The tree of loose refs, and the directory itself: a ref appearing in
        // it is reported against the parent on some platforms.
        [first, ..] if *first == refs => true,
        [only] => *only == packed || *only == worktrees || *only == head,
        [first, _name, last] if *first == worktrees => *last == head,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn served(branch: Option<&str>, root: &str) -> Served {
        Served {
            branch: branch.map(str::to_string),
            root: PathBuf::from(root),
        }
    }

    fn a_ref(label: &str, worktrees: &[&str]) -> git::Ref {
        at_commit(label, worktrees, &"0".repeat(40))
    }

    fn at_commit(label: &str, worktrees: &[&str], commit: &str) -> git::Ref {
        git::Ref {
            name: format!("refs/heads/{label}"),
            commit: commit.to_string(),
            label: label.into(),
            worktrees: worktrees.iter().map(PathBuf::from).collect(),
        }
    }

    fn commits(commits: &[&str]) -> BTreeSet<String> {
        commits.iter().map(|c| (*c).to_string()).collect()
    }

    fn checkout(root: &str, branch: Option<&str>, commit: &str) -> git::Checkout {
        git::Checkout {
            root: PathBuf::from(root),
            branch: branch.map(str::to_string),
            commit: Some(commit.to_string()),
        }
    }

    #[test]
    fn a_branch_a_worktree_has_checked_out_is_not_a_column() {
        let served = vec![served(Some("feat-x"), "/repo/wt-x")];
        assert!(checked_out(
            &a_ref("feat-x", &[]),
            &served,
            &BTreeSet::new()
        ));
        assert!(!checked_out(
            &a_ref("feat-y", &[]),
            &served,
            &BTreeSet::new()
        ));
    }

    #[test]
    fn a_detached_checkout_with_no_branch_is_matched_by_its_directory() {
        // The jj shape at its most awkward: no branch name to compare, and the
        // project nested one level inside the checkout.
        let served = vec![served(None, "/repo/wt-x/project")];
        let none = BTreeSet::new();
        assert!(checked_out(&a_ref("wt-x", &["/repo/wt-x"]), &served, &none));
        assert!(!checked_out(
            &a_ref("wt-z", &["/repo/wt-z"]),
            &served,
            &none
        ));
    }

    /// The hole the commit rule closes. Several branches can point at one
    /// commit, and `git::Ref` carries only the alphabetically first as its
    /// label; the repository's *main* worktree is in no `worktrees` list at all.
    /// So a corpus in the main worktree on `topic`, with a leftover `alpha` at
    /// the same commit, matched neither of the older rules and grew a second
    /// column duplicating itself.
    #[test]
    fn a_commit_a_served_corpus_sits_on_is_not_a_column_whatever_it_is_labelled() {
        let served = vec![served(Some("topic"), "/repo")];
        let head = "a".repeat(40);
        let alpha = at_commit("alpha", &[], &head);
        assert!(
            !checked_out(&alpha, &served, &BTreeSet::new()),
            "the shape that used to duplicate the primary column"
        );
        assert!(checked_out(&alpha, &served, &commits(&[&head])));
        // And a ref on another commit is still a column of its own.
        assert!(!checked_out(
            &at_commit("beta", &[], &"b".repeat(40)),
            &served,
            &commits(&[&head])
        ));
    }

    #[test]
    fn the_commits_of_the_served_corpora_come_from_the_checkout_each_sits_in() {
        let head = "a".repeat(40);
        let other = "b".repeat(40);
        let checkouts = vec![
            checkout("/repo", Some("topic"), &head),
            checkout("/repo/wt-x", None, &other),
        ];
        // Nested: the corpus is inside the *linked* worktree, which is itself
        // under the main one, so the innermost checkout is the one it is in.
        let served = vec![served(None, "/repo/wt-x/project")];
        assert_eq!(served_commits(&served, &checkouts), commits(&[&other]));
        assert_eq!(
            served_commits(&[self::served(None, "/repo/project")], &checkouts),
            commits(&[&head])
        );
        // A corpus in no checkout of this repository contributes nothing.
        assert!(served_commits(&[self::served(None, "/elsewhere")], &checkouts).is_empty());
    }

    #[test]
    fn reads_are_not_ref_changes() {
        let git_dir = Path::new("/p/.git");
        let ref_path = PathBuf::from("/p/.git/refs/heads/feat-x");
        let read = notify::Event {
            kind: notify::EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![ref_path.clone()],
            attrs: notify::event::EventAttributes::default(),
        };
        assert!(
            !is_change(git_dir, &read),
            "a pass reads every ref it watches"
        );
        let wrote = notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![ref_path],
            attrs: notify::event::EventAttributes::default(),
        };
        assert!(is_change(git_dir, &wrote));
    }

    #[test]
    fn only_ref_shaped_paths_are_changes() {
        let git_dir = Path::new("/p/.git");
        assert!(is_ref_path(git_dir, Path::new("/p/.git/refs/heads/feat-x")));
        assert!(is_ref_path(git_dir, Path::new("/p/.git/packed-refs")));
        assert!(is_ref_path(git_dir, Path::new("/p/.git/worktrees/wt/HEAD")));
        // The main worktree's own head, which says which branch that checkout
        // is on — the same file the line above watches, one directory up.
        assert!(is_ref_path(git_dir, Path::new("/p/.git/HEAD")));
        // The first linked worktree appearing: a change, and the moment the
        // `worktrees` watch can be attached.
        assert!(is_ref_path(git_dir, Path::new("/p/.git/worktrees")));
        // The churn the non-recursive watch on the git directory picks up.
        assert!(!is_ref_path(git_dir, Path::new("/p/.git/index")));
        assert!(!is_ref_path(git_dir, Path::new("/p/.git/COMMIT_EDITMSG")));
        assert!(!is_ref_path(git_dir, Path::new("/p/.git/FETCH_HEAD")));
        // And the churn the *recursive* watch on `worktrees` picks up: a linked
        // worktree's admin directory is a whole git directory of its own, and
        // an ordinary `git status` in that checkout rewrites its index.
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/worktrees/wt/index")
        ));
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/worktrees/wt/ORIG_HEAD")
        ));
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/worktrees/wt/logs/HEAD")
        ));
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/worktrees/wt/commondir")
        ));
        // Git writes a ref as `foo.lock` and renames; the rename is the event.
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/refs/heads/feat-x.lock")
        ));
        assert!(!is_ref_path(
            git_dir,
            Path::new("/p/.git/worktrees/wt/HEAD.lock")
        ));
        // Position, not name: a repository that happens to live under a
        // directory called `refs` is still just a repository.
        let odd = Path::new("/refs/p/.git");
        assert!(!is_ref_path(odd, Path::new("/refs/p/.git/index")));
        assert!(is_ref_path(odd, Path::new("/refs/p/.git/refs/heads/main")));
        // A path outside the watched directory is nothing to do with us.
        assert!(!is_ref_path(git_dir, Path::new("/q/.git/refs/heads/main")));
    }
}
