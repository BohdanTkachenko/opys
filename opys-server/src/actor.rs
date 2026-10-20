//! One actor thread per corpus, owning a warm [`Store`].
//!
//! All store access is single-threaded by construction: the store is created
//! inside the actor thread and never moves across threads, so nothing here
//! assumes it is `Send` or `Sync`. Callers talk to it over a channel.
//!
//! **The one rule that must not be broken:** the warm store is a read-only
//! cache and must never retain the inventory lock. `backend.load` takes the
//! flock; the actor releases it immediately afterwards. A warm store that keeps
//! it deadlocks every CLI invocation against this server for
//! `OPYS_LOCK_TIMEOUT_MS` and then fails it. `reload_releases_the_inventory_lock`
//! in `tests/actor.rs` pins this.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use opys_engine::backend::Backend;
use opys_engine::commands::{now_rfc3339, verify};
use opys_engine::doc::Doc;
use opys_engine::error::{OpysError, Result};
use opys_engine::lifecycle;
use opys_engine::next::{self, PickQuery, PickResult};
use opys_engine::observe;
use opys_engine::project::Project;
use opys_engine::queue::{self, QueueQuery, QueueResult};
use opys_engine::refs;
use opys_engine::roles;
use opys_engine::store::Store;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::discover::Corpus;
use crate::flow::{self, FlowView};

/// Quiet period a filesystem burst must clear before it becomes one reload.
/// `opys sync` rewriting forty files is one event, not forty.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// What happened, for the WebSocket fan-out.
///
/// Payloads are counts, not documents: subscribers refetch what they need over
/// the API. A broadcast channel clones its message per receiver and buffers for
/// slow ones, so pushing whole corpora through it would cost memory
/// proportional to viewers times corpus size for data most of them already have.
///
/// On the wire this is `{"event": "corpus-reloaded", …}` — the discriminant is
/// kebab-case because that is the vocabulary the event stream and the write path write
/// against. Field names stay snake_case, matching every other JSON payload the
/// API emits (`verify_problems`, `is_primary`, `body_html`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    CorpusReloaded {
        cid: String,
        docs: usize,
        verify_problems: usize,
        ts: String,
    },
    CorpusAdded {
        cid: String,
    },
    CorpusRemoved {
        cid: String,
    },
    /// A write completed through the API. `action` is the request's
    /// own wire name (`set-status`, `close`), and `id` the document it touched.
    ///
    /// This is the acknowledgement, not the refresh: the write also trips the
    /// corpus watcher, so a `corpus-reloaded` follows once the debounce expires.
    /// A subscriber that reacts to both sees the result immediately and the new
    /// counts a moment later.
    ActionCompleted {
        cid: String,
        action: String,
        id: String,
    },
    /// A project group's refs moved, so its branch columns and its occupancy
    /// are new (ADR-0053). Keyed by the **group** key, not a cid: the branches
    /// of a repository belong to the project, not to any one worktree of it.
    ///
    /// Sent only when the ref reader's answer actually changed. A client that
    /// cannot attribute a frame to a corpus assumes it affects every view, so a
    /// frame per periodic re-read would be a node-wide refetch every minute.
    RefsChanged {
        project: String,
    },
}

/// One row of the board: everything a list view needs without opening a doc.
///
/// `PartialEq` so the ref reader can tell whether a re-read changed anything
/// before it broadcasts; see [`crate::refreader`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub status: String,
    pub title: String,
    pub tags: Vec<String>,
    /// Relative to the project root.
    pub path: String,
    pub updated: Option<String>,
    /// Whether anything live still blocks this document (ADR-0050 decision 3):
    /// a `blocked_by` entry that is not struck, that resolves, and whose
    /// target's status its type does not release at. Computed by the same
    /// [`refs::is_blocked`] behind `opys list --blocked`, so the board badge and
    /// the CLI cannot drift.
    pub blocked: bool,
    /// The ids of exactly those live blockers, for the badge's count and its
    /// tooltip. Ids rather than a count *and* a map, because a count that
    /// disagreed with the list would be the very drift this replaced: the full
    /// relation, titles and tombstones included, is one click away on the
    /// document view.
    pub live_blockers: Vec<String>,
    /// The mirror: how many live documents this one is blocking.
    pub blocks: usize,
    /// The `priority` frontmatter field, when it is an integer (ADR-0046:
    /// priority is an opt-in declared field, lower = higher). Lifted here
    /// because the board orders a column by it; anything non-integer is `None`,
    /// which sorts last exactly like an absent priority.
    pub priority: Option<i64>,
}

/// One document, opened.
///
/// The frontmatter keys a reader always wants — tags, `updated`, the three
/// relation maps — are lifted to the top level *as well as* staying in
/// [`DocView::fields`]. Lifting them here rather than in the HTTP layer keeps
/// one interpretation of frontmatter in the crate: the API handler serializes
/// this struct and adds nothing.
#[derive(Debug, Clone, Serialize)]
pub struct DocView {
    /// The roles this document currently waits on, interpreted by the engine.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub owner_override: bool,
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub status: String,
    /// This document's type's whole status *vocabulary*, minus its terminal
    /// statuses, which are reachable only through `close`.
    ///
    /// Here rather than in the client so nothing outside this crate has to read
    /// `opys.toml` — a UI that knew the status vocabulary would be a second
    /// interpretation of the config, drifting the moment a type is edited.
    /// Empty when the id's prefix matches no configured type, which is the same
    /// answer `type_name` gives.
    ///
    /// It answers "what statuses exist here", not "where may this document go":
    /// once the type declares a status graph that is
    /// [`DocView::allowed_targets`]'s question, and this field is the fallback
    /// for a type that declares none.
    pub allowed_statuses: Vec<String>,
    /// Where *this* document may go next: the edges its type declares out of its
    /// current status (ADR-0050 decision 1), minus terminal ones, or `null` when
    /// the type declares no graph at all and every status is reachable.
    ///
    /// Computed by the same engine helper `set-status`'s guard reads
    /// ([`lifecycle::allowed_targets`]), so a menu built from it cannot offer a
    /// move the node would then refuse. Terminal statuses are filtered out for
    /// the same reason `allowed_statuses` filters them: an edge into one says
    /// the document may be *closed* from here, and `close` — gated separately by
    /// [`DocView::closable`] — is the only door. A dead-end status therefore
    /// yields `[]`, which is the honest answer, not a missing one.
    pub allowed_targets: Option<Vec<String>>,
    /// Whether `close` would be accepted *from where this document stands*: its
    /// type has a terminal status at all, and — when the type declares a status
    /// graph — the current status has an edge into one. Without this the UI's
    /// close button would 422 on, say, an ADR no matter what the user did —
    /// [`DocView::allowed_statuses`] cannot express it, because a terminal
    /// status is precisely what it leaves out.
    pub closable: bool,
    /// Whether the type has a terminal status *at all* — the other half of
    /// [`DocView::closable`], which on its own conflates two refusals a reader
    /// must not be told apart wrongly: "never, for this type" and "not from
    /// here yet".
    ///
    /// A client cannot derive it from the fields above.
    /// [`DocView::allowed_targets`] is non-null whenever the type declares a
    /// *graph*, which says nothing about terminal statuses — the shipped
    /// `feature` type is exactly that shape (a graph, `terminal_statuses = []`),
    /// and so are this repo's own `feature` and `adr`. Only the node can tell
    /// the two apart, the way `close` does: empty `terminal_statuses` first,
    /// then the graph.
    pub has_terminal_status: bool,
    pub title: String,
    pub path: String,
    pub tags: Vec<String>,
    pub updated: Option<String>,
    /// `references`, as an id → title map. Values keep the `~~strikethrough~~`
    /// of a closed document's tombstone: that distinction is the point of the
    /// marker, so it is the caller's to render, not ours to strip.
    pub references: BTreeMap<String, String>,
    /// What blocks this document, id → title.
    pub blocked_by: BTreeMap<String, String>,
    /// Which of those entries still *hold* (ADR-0050 decision 3) — the rest are
    /// struck tombstones, dangling ids, or targets whose type releases at the
    /// status they have reached.
    ///
    /// The map above cannot answer it: whether an entry is live depends on the
    /// rest of the corpus and on the blocker type's `releases_blocks`, so a
    /// client deriving it would be a second interpretation of `opys.toml` —
    /// exactly what [`DocView::allowed_statuses`] exists to avoid.
    pub live_blockers: Vec<String>,
    /// Which non-struck `blocked_by` entries point at an id no document answers
    /// to — `verify`'s "does not resolve to a known document".
    ///
    /// A subset of the entries missing from `live_blockers`, and the reason that
    /// list alone cannot explain itself: an entry drops out either because its
    /// target released or because there is no target, and a view that renders
    /// both as "released" tells the reader a status transition happened to a
    /// document that does not exist.
    pub dangling_blockers: Vec<String>,
    /// Whether anything in `live_blockers` remains — the whole of what "blocked"
    /// means. There is no blocked status and no blocked flag on disk.
    pub blocked: bool,
    /// What this document blocks, id → title.
    pub blocks: BTreeMap<String, String>,
    /// Every frontmatter key, as JSON.
    pub fields: BTreeMap<String, serde_json::Value>,
    /// The type's declared custom fields (`[fields.*]` in `opys.toml`), set or
    /// not. Same reasoning as [`DocView::allowed_statuses`]: the UI's field
    /// editor needs the vocabulary — which keys exist, which are enums and of
    /// what, which are required — and reading `opys.toml` from a client would
    /// be a second interpretation of the config. Empty when the id's prefix
    /// matches no configured type.
    pub declared_fields: Vec<FieldView>,
    /// The headings of this type's `kind = "log"` sections, in declaration
    /// order — what the document view's entry box writes to, and which
    /// `--section` name it sends when there is more than one.
    ///
    /// Same reasoning as [`DocView::allowed_statuses`]: a client that worked
    /// out which sections are logs would be a second reading of `opys.toml`,
    /// drifting the moment a type is edited. Empty when the id's prefix matches
    /// no configured type, or when the type declares no log section — which is
    /// also how the UI knows not to offer the box at all.
    pub log_sections: Vec<String>,
    /// The markdown body, verbatim.
    pub body: String,
    /// Snapshot identity for source-location consumers; not a security digest.
    pub body_version: String,
    /// The body rendered. Raw HTML in the source stays escaped — comrak's
    /// `unsafe_` is off, and must stay off: bodies are user content.
    pub body_html: String,
}

/// One declared custom field of a document's type, as the field editor needs
/// it: enough to choose the right input (an enum gets a select of `values`, a
/// string gets text with `pattern` as the hint) and to know what must not be
/// removed (`required`). The constraint itself is still enforced server-side —
/// every field write is verify-gated — so this is affordance, not authority.
#[derive(Debug, Clone, Serialize)]
pub struct FieldView {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: &'static str,
    pub required: bool,
    /// Allowed values for an `enum` field; empty otherwise.
    pub values: Vec<String>,
    pub description: Option<String>,
    /// The regex a `string` value must fully match, when one is declared.
    pub pattern: Option<String>,
}

/// A declared role's metadata and default queue from one corpus snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct RoleQueue {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub queue: QueueResult,
}

/// Role lookup failures are resources missing from this project; other query
/// validation failures are malformed input.
pub enum QueueQueryError {
    UnknownRole,
    Invalid(String),
}

/// Which documents a caller wants. Every field is an equality filter; `None`
/// matches everything.
#[derive(Debug, Clone, Default)]
pub struct DocFilter {
    pub type_name: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
}

impl DocFilter {
    /// Whether a summary passes every set field.
    ///
    /// Public because one caller filters outside the actor: the union view asks
    /// each corpus for *everything* (it needs the unfiltered id set to spot an
    /// impending id collision) and applies the request's filter itself, so the
    /// two paths must agree on what a filter means.
    pub fn matches(&self, d: &DocSummary) -> bool {
        self.type_name.as_ref().is_none_or(|t| *t == d.type_name)
            && self.status.as_ref().is_none_or(|s| *s == d.status)
            && self.tag.as_ref().is_none_or(|t| d.tags.contains(t))
    }
}

/// A user SQL result: column labels and stringified rows.
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Per-corpus numbers for a project list: how much is there and how healthy it
/// is, without shipping any of it.
///
/// One request answers all four, because `/api/projects` asks every served
/// corpus on every call — two round trips each (docs then verify) that clone
/// every summary and every problem string would be a lot of work to produce
/// four integers.
#[derive(Debug, Clone, Serialize)]
pub struct CorpusStats {
    pub doc_count: usize,
    pub verify_problems: usize,
    /// When the numbers were computed; `None` if the corpus has never loaded.
    pub loaded_at: Option<String>,
    /// Why the most recent load attempt failed, if it did.
    pub load_error: Option<String>,
}

/// The corpus's health, as of the last successful load.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyStatus {
    /// True only when the corpus loaded and `verify` found nothing.
    pub ok: bool,
    pub problems: Vec<String>,
    /// Structured companions to `problems`, in the same order.
    pub findings: Vec<opys_engine::diagnostics::Finding>,
    /// Findings removed by a read-only preview of the actual automatic fixes.
    pub fixable_problems: Vec<String>,
    pub loaded_at: Option<String>,
    /// Why the most recent load attempt failed, if it did. Reads keep answering
    /// from the last good state, so this is how a caller learns the data is
    /// stale rather than absent.
    pub load_error: Option<String>,
}

/// Verification of a disposable body snapshot, never written to the store.
#[derive(Debug, Serialize)]
pub struct DraftVerification {
    pub body_version: String,
    pub findings: Vec<opys_engine::diagnostics::Finding>,
}

enum Message {
    /// The watcher saw something relevant; start (or extend) the quiet period.
    Fs,
    Req(Request),
}

enum Request {
    // The reads carry a `Result` rather than a bare answer so that "this corpus
    // never loaded" stays distinguishable from "there is nothing to show". An
    // empty list and a missing document are answers; a corpus whose config will
    // not parse has none, and saying so is what lets the API return a server
    // error instead of a misleading 200 or 404.
    Doc(String, SyncSender<Result<Option<DocView>>>),
    VerifyBody(
        String,
        String,
        SyncSender<Result<Option<DraftVerification>>>,
    ),
    Query(
        String,
        Vec<String>,
        SyncSender<Result<std::result::Result<QueryResult, String>>>,
    ),
    /// The first cached read waits for the initial load attempt. After that,
    /// published immutable reads never need to enter the actor's mailbox.
    Ready(SyncSender<()>),
    Verify(SyncSender<VerifyStatus>),
    /// Reload now and reply when done — the deterministic path for tests and
    /// for the manager after a registry change.
    Reload(SyncSender<()>),
    Shutdown,
}

/// The request-sending half of a corpus actor: `Clone + Send + Sync`, and cheap
/// (one `String`, one channel sender and one shared cache pointer).
///
/// This exists so a request handler never holds the manager's lock while it
/// blocks. Store-backed calls wait on a channel, while cached summaries,
/// health and pure builders read the last published generation immediately,
/// including during a reload. The manager's lock is dropped before either.
///
/// A client may outlive its [`CorpusHandle`] — a rescan can stop the corpus
/// while a request is in flight. Calls then fail with "corpus actor stopped"
/// rather than hanging.
#[derive(Clone)]
pub struct CorpusClient {
    /// Which corpus this talks to, for error messages and log lines.
    pub cid: String,
    tx: mpsc::Sender<Message>,
    published: Arc<Mutex<PublishedReads>>,
}

/// A handle to one corpus actor: the thread, the corpus it serves, and the
/// [`CorpusClient`] used to talk to it. The manager owns handles; everyone else
/// gets a client.
pub struct CorpusHandle {
    pub corpus: Corpus,
    client: CorpusClient,
    join: Option<JoinHandle<()>>,
}

/// One successful load's immutable inputs. The store stays on its actor thread;
/// only config, reconstructed documents and cached findings are shared. A
/// failed reload retains these exact inputs, just as it retains summaries.
#[derive(Clone)]
struct ReadSnapshot {
    prj: Arc<Project>,
    docs: Arc<Vec<Doc>>,
    findings: Arc<Vec<opys_engine::diagnostics::Finding>>,
}

/// A complete successful generation. No store is shared, and every large field
/// is behind an Arc so obtaining the generation only holds the publication
/// mutex long enough to clone pointers.
struct ReadGeneration {
    snapshot: ReadSnapshot,
    summaries: Arc<Vec<DocSummary>>,
    loaded_at: String,
    verify_problems: usize,
}

#[derive(Clone, Default)]
struct PublishedReads {
    generation: Option<Arc<ReadGeneration>>,
    load_error: Option<String>,
    initialized: bool,
    stopped: bool,
}

/// Clients can outlive their actor. Clear liveness on every exit, including an
/// unwind, so a retained cache never makes a stopped corpus look available.
struct PublishedLifetime(Arc<Mutex<PublishedReads>>);

impl Drop for PublishedLifetime {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).stopped = true;
    }
}

fn gone() -> OpysError {
    OpysError::Store("corpus actor stopped".to_string())
}

fn not_loaded(load_error: &Option<String>) -> OpysError {
    OpysError::Store(match load_error {
        Some(error) => format!("corpus is not loaded: {error}"),
        None => "corpus is not loaded".to_string(),
    })
}

impl CorpusClient {
    fn ask<T>(&self, make: impl FnOnce(SyncSender<T>) -> Request) -> Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.send(Message::Req(make(tx))).map_err(|_| gone())?;
        rx.recv().map_err(|_| gone())
    }

    /// Filtered document summaries, from the warm cache. Fails if the corpus has
    /// never loaded — an empty list would claim the inventory is empty.
    pub fn docs(&self, filter: DocFilter) -> Result<Vec<DocSummary>> {
        let published = self.read_published()?;
        let generation = published
            .generation
            .ok_or_else(|| not_loaded(&published.load_error))?;
        Ok(generation
            .summaries
            .iter()
            .filter(|doc| filter.matches(doc))
            .cloned()
            .collect())
    }

    /// One document with its rendered body, or `None` if this corpus has no
    /// such id. Fails if the corpus has never loaded — `None` would claim the
    /// document does not exist.
    pub fn doc(&self, id: &str) -> Result<Option<DocView>> {
        self.ask(|reply| Request::Doc(id.to_string(), reply))?
    }

    /// Validate an unsaved body against this corpus without changing any state.
    pub fn verify_body(&self, id: &str, body: &str) -> Result<Option<DraftVerification>> {
        self.ask(|reply| Request::VerifyBody(id.to_string(), body.to_string(), reply))?
    }

    /// Run a user query against the warm store. The inner `Err` is the user's
    /// SQL problem (a 400 for the API); the outer one is ours — the actor is
    /// gone, the corpus never loaded, the projections would not rebuild — and
    /// must not be reported as bad input.
    pub fn query(
        &self,
        sql: &str,
        params: &[String],
    ) -> Result<std::result::Result<QueryResult, String>> {
        self.ask(|reply| Request::Query(sql.to_string(), params.to_vec(), reply))?
    }

    /// The pick oracle over the warm store. The inner `Err` is the caller's
    /// query problem (a 400 for the API); the outer one is ours — the actor is
    /// gone, or the corpus never loaded.
    pub fn next(&self, q: PickQuery) -> Result<std::result::Result<PickResult, String>> {
        let snapshot = self.snapshot()?;
        if let Err(error) = q.validate(&snapshot.prj.pcfg) {
            return Ok(Err(error));
        }
        Ok(Ok(next::pick(
            &snapshot.prj,
            &snapshot.docs,
            &q,
            observe::now(),
        )))
    }

    /// A role queue using the actor's cached verify findings.
    pub fn queue(
        &self,
        q: QueueQuery,
    ) -> Result<std::result::Result<QueueResult, QueueQueryError>> {
        let snapshot = self.snapshot()?;
        if !roles::declared(&snapshot.prj.pcfg, &q.role) {
            return Ok(Err(QueueQueryError::UnknownRole));
        }
        if let Err(message) = q.validate(&snapshot.prj.pcfg) {
            return Ok(Err(QueueQueryError::Invalid(message)));
        }
        Ok(Ok(queue::build(
            &snapshot.prj,
            &snapshot.docs,
            &q,
            observe::now(),
            &snapshot.findings,
        )))
    }

    /// Every declared role and its queue, from one warm snapshot.
    pub fn roles(&self) -> Result<Vec<RoleQueue>> {
        let snapshot = self.snapshot()?;
        let now = observe::now();
        Ok(roles::names(&snapshot.prj.pcfg)
            .into_iter()
            .map(|name| RoleQueue {
                name: name.to_string(),
                description: roles::description(&snapshot.prj.pcfg, name).map(str::to_string),
                queue: queue::build(
                    &snapshot.prj,
                    &snapshot.docs,
                    &QueueQuery {
                        role: name.to_string(),
                        type_filter: None,
                        limit: next::DEFAULT_LIMIT,
                    },
                    now,
                    &snapshot.findings,
                ),
            })
            .collect())
    }

    /// The lifecycle graph over the warm store, with `limit` items per node.
    /// Fails only if the actor is gone or the corpus never loaded.
    pub fn flow(&self, limit: usize) -> Result<FlowView> {
        let snapshot = self.snapshot()?;
        Ok(flow::flow(&snapshot.prj, &snapshot.docs, &self.cid, limit))
    }

    fn snapshot(&self) -> Result<ReadSnapshot> {
        let published = self.read_published()?;
        published
            .generation
            .map(|generation| generation.snapshot.clone())
            .ok_or_else(|| not_loaded(&published.load_error))
    }

    fn read_published(&self) -> Result<PublishedReads> {
        let read = || {
            self.published
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        };
        let mut published = read();
        if published.stopped {
            return Err(gone());
        }
        if !published.initialized {
            // Match the old startup contract: the initial actor load finishes
            // before a caller gets either its data or its load error.
            self.ask(Request::Ready)?;
            published = read();
        }
        if published.stopped {
            return Err(gone());
        }
        Ok(published)
    }

    fn mark_stopped(&self) {
        self.published
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
    }

    /// Cached verify problems and the time they were computed.
    pub fn verify(&self) -> Result<VerifyStatus> {
        self.ask(Request::Verify)
    }

    /// Document count and health in one round trip.
    pub fn stats(&self) -> Result<CorpusStats> {
        let published = self.read_published()?;
        Ok(CorpusStats {
            doc_count: published
                .generation
                .as_ref()
                .map_or(0, |g| g.summaries.len()),
            verify_problems: published
                .generation
                .as_ref()
                .map_or(0, |g| g.verify_problems),
            loaded_at: published.generation.as_ref().map(|g| g.loaded_at.clone()),
            load_error: published.load_error,
        })
    }

    /// Force a reload and wait for it.
    pub fn reload(&self) -> Result<()> {
        self.ask(Request::Reload)
    }
}

impl CorpusHandle {
    /// Start an actor for `corpus`. The thread loads immediately, so the first
    /// read after this call answers from a warm store.
    pub fn spawn(
        corpus: Corpus,
        backend: Box<dyn Backend + Send>,
        events: broadcast::Sender<Event>,
    ) -> CorpusHandle {
        let (tx, rx) = mpsc::channel::<Message>();
        let watch_tx = tx.clone();
        let thread_corpus = corpus.clone();
        let published = Arc::new(Mutex::new(PublishedReads::default()));
        let actor_published = Arc::clone(&published);
        let join = std::thread::Builder::new()
            .name(format!("opys-corpus-{}", corpus.cid))
            .spawn(move || {
                let _lifetime = PublishedLifetime(Arc::clone(&actor_published));
                // Held for the life of the loop: dropping a notify watcher stops
                // it delivering.
                let _watcher = spawn_watcher(&thread_corpus, watch_tx);
                let mut actor = Actor {
                    corpus: thread_corpus,
                    backend,
                    events,
                    warm: None,
                    load_error: None,
                    published: actor_published,
                };
                actor.reload();
                actor.run(rx);
            })
            .expect("spawning a corpus thread");
        CorpusHandle {
            client: CorpusClient {
                cid: corpus.cid.clone(),
                tx,
                published,
            },
            corpus,
            join: Some(join),
        }
    }

    /// A cloneable talker to this actor, for callers who cannot hold a borrow of
    /// the manager (see [`CorpusClient`]).
    pub fn client(&self) -> CorpusClient {
        self.client.clone()
    }

    /// Filtered document summaries, from the warm cache.
    pub fn docs(&self, filter: DocFilter) -> Result<Vec<DocSummary>> {
        self.client.docs(filter)
    }

    /// One document with its rendered body, or `None` if this corpus has no
    /// such id.
    pub fn doc(&self, id: &str) -> Result<Option<DocView>> {
        self.client.doc(id)
    }

    /// Run a user query against the warm store. The inner `Err` is the user's
    /// SQL problem (a 4xx for the API); the outer one means the actor is gone.
    pub fn query(
        &self,
        sql: &str,
        params: &[String],
    ) -> Result<std::result::Result<QueryResult, String>> {
        self.client.query(sql, params)
    }

    /// The pick oracle over the warm store. The inner `Err` is the caller's
    /// query problem (a 4xx for the API); the outer one means the actor is gone.
    pub fn next(&self, q: PickQuery) -> Result<std::result::Result<PickResult, String>> {
        self.client.next(q)
    }

    /// A role queue over the warm store and cached verification findings.
    pub fn queue(
        &self,
        q: QueueQuery,
    ) -> Result<std::result::Result<QueueResult, QueueQueryError>> {
        self.client.queue(q)
    }

    /// All role declarations and their default queues from one warm snapshot.
    pub fn roles(&self) -> Result<Vec<RoleQueue>> {
        self.client.roles()
    }

    /// The lifecycle graph over the warm store. The `Err` means the actor is
    /// gone or the corpus never loaded.
    pub fn flow(&self, limit: usize) -> Result<FlowView> {
        self.client.flow(limit)
    }

    /// Cached verify problems and the time they were computed.
    pub fn verify(&self) -> Result<VerifyStatus> {
        self.client.verify()
    }

    /// Document count and health in one round trip.
    pub fn stats(&self) -> Result<CorpusStats> {
        self.client.stats()
    }

    /// Force a reload and wait for it.
    pub fn reload(&self) -> Result<()> {
        self.client.reload()
    }

    /// Stop the actor and wait for its thread.
    pub fn shutdown(mut self) {
        self.client.mark_stopped();
        let _ = self.client.tx.send(Message::Req(Request::Shutdown));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for CorpusHandle {
    fn drop(&mut self) {
        // An explicit message, not channel closure: the watcher callback owns a
        // cloned sender that lives *inside* the actor thread, and outstanding
        // clients own more, so the receiver never sees a disconnect on its own.
        // Deliberately no join here — a `Drop` that blocks on a reload in
        // progress would be a surprising place to spend 100 ms.
        self.client.mark_stopped();
        let _ = self.client.tx.send(Message::Req(Request::Shutdown));
    }
}

/// Everything the actor keeps between requests. Replaced wholesale on a
/// successful reload, never mutated piecemeal.
struct Warm {
    prj: Arc<Project>,
    store: Store,
    docs: Arc<Vec<Doc>>,
    summaries: Arc<Vec<DocSummary>>,
    problems: Vec<String>,
    findings: Arc<Vec<opys_engine::diagnostics::Finding>>,
    fixable_problems: Vec<String>,
    loaded_at: String,
}

struct Actor {
    corpus: Corpus,
    backend: Box<dyn Backend + Send>,
    events: broadcast::Sender<Event>,
    warm: Option<Warm>,
    load_error: Option<String>,
    published: Arc<Mutex<PublishedReads>>,
}

impl Actor {
    fn run(&mut self, rx: mpsc::Receiver<Message>) {
        // `Some(deadline)` means a filesystem burst is settling.
        let mut quiet_until: Option<Instant> = None;
        loop {
            let msg = match quiet_until {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    // Still settling: wait out the remainder, but let further
                    // events push the deadline back.
                    Some(remaining) => match rx.recv_timeout(remaining) {
                        Ok(m) => m,
                        Err(RecvTimeoutError::Timeout) => {
                            quiet_until = None;
                            self.reload();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                    None => {
                        quiet_until = None;
                        self.reload();
                        continue;
                    }
                },
                None => match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                },
            };
            match msg {
                Message::Fs => quiet_until = Some(Instant::now() + DEBOUNCE),
                Message::Req(Request::Shutdown) => break,
                Message::Req(req) => self.handle(req),
            }
        }
    }

    fn handle(&mut self, req: Request) {
        match req {
            Request::Doc(id, reply) => {
                let _ = reply.send(self.doc_view(&id));
            }
            Request::VerifyBody(id, body, reply) => {
                let _ = reply.send(self.verify_body(&id, body));
            }
            Request::Query(sql, params, reply) => {
                let _ = reply.send(self.run_query(&sql, &params));
            }
            Request::Ready(reply) => {
                let _ = reply.send(());
            }
            Request::Verify(reply) => {
                let _ = reply.send(VerifyStatus {
                    ok: self.load_error.is_none()
                        && self.warm.as_ref().is_some_and(|w| w.problems.is_empty()),
                    problems: self
                        .warm
                        .as_ref()
                        .map(|w| w.problems.clone())
                        .unwrap_or_default(),
                    findings: self
                        .warm
                        .as_ref()
                        .map(|w| w.findings.as_ref().clone())
                        .unwrap_or_default(),
                    loaded_at: self.warm.as_ref().map(|w| w.loaded_at.clone()),
                    fixable_problems: if self.load_error.is_none() {
                        self.warm
                            .as_ref()
                            .map(|w| w.fixable_problems.clone())
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    load_error: self.load_error.clone(),
                });
            }
            Request::Reload(reply) => {
                self.reload();
                let _ = reply.send(());
            }
            Request::Shutdown => {}
        }
    }

    /// The warm cache, or why there is none.
    ///
    /// Reads answer from the last good load, so the only way here is a corpus
    /// that has never had one. That is a state of the server, not of the
    /// caller's request, and every read says so the same way.
    fn warm(&mut self) -> Result<&mut Warm> {
        if self.warm.is_none() {
            return Err(not_loaded(&self.load_error));
        }
        Ok(self.warm.as_mut().expect("checked just above"))
    }

    fn doc_view(&mut self, id: &str) -> Result<Option<DocView>> {
        let warm = self.warm()?;
        let Some(dkey) = warm.store.dkey_opt(id)? else {
            return Ok(None);
        };
        let doc = warm.store.doc(dkey)?;
        // The summaries are this corpus's id → status index, already built by
        // the last load — the same one the board's badges came from.
        let corpus = refs::Corpus::new(
            &warm.prj.pcfg,
            warm.summaries
                .iter()
                .map(|s| (s.id.as_str(), s.status.as_str())),
        );
        Ok(Some(view(&warm.prj, &doc, &corpus)))
    }

    fn verify_body(&mut self, id: &str, body: String) -> Result<Option<DraftVerification>> {
        let warm = self.warm()?;
        let Some(dkey) = warm.store.dkey_opt(id)? else {
            return Ok(None);
        };
        let mut doc = warm.store.doc(dkey)?;
        doc.body = body;
        doc.title = opys_engine::body::title(&doc.body);
        let mut docs = warm.docs.as_ref().clone();
        for candidate in &mut docs {
            if candidate.path == doc.path {
                *candidate = doc.clone();
            }
        }
        let mut findings = verify::collect_findings(&warm.prj, &docs, Vec::new());
        findings.retain(|finding| finding.doc_id.as_deref() == Some(id));
        opys_engine::diagnostics::resolve_body(&doc, &mut findings);
        for finding in &mut findings {
            finding.path = finding.path.as_ref().map(|path| {
                path.strip_prefix(&warm.prj.root)
                    .unwrap_or(path)
                    .to_path_buf()
            });
        }
        Ok(Some(DraftVerification {
            body_version: opys_engine::diagnostics::body_version(&doc.body),
            findings,
        }))
    }

    /// Run user SQL. The outer error is ours (no warm store, projections would
    /// not rebuild); the inner one is the statement's, verbatim from the engine.
    fn run_query(
        &mut self,
        sql: &str,
        params: &[String],
    ) -> Result<std::result::Result<QueryResult, String>> {
        let warm = self.warm()?;
        // The derived projections (`fields`, `sections`, `blocks`) are what user
        // SQL is written against, and they are rebuilt rather than maintained.
        warm.store.refresh_projections(&warm.prj.pcfg)?;
        Ok(warm
            .store
            .run_user_query(sql, params)
            .map(|(columns, rows)| QueryResult { columns, rows }))
    }

    /// Load afresh and swap the cache in. A failure leaves the previous warm
    /// state answering reads and records why.
    fn reload(&mut self) {
        match self.load() {
            Ok(warm) => {
                let event = Event::CorpusReloaded {
                    cid: self.corpus.cid.clone(),
                    docs: warm.summaries.len(),
                    verify_problems: warm.problems.len(),
                    ts: warm.loaded_at.clone(),
                };
                self.warm = Some(warm);
                self.load_error = None;
                self.publish();
                // Err just means nobody is subscribed.
                let _ = self.events.send(event);
            }
            Err(e) => {
                self.load_error = Some(e.to_string());
                self.publish();
            }
        }
    }

    fn publish(&self) {
        // Build the next publication before locking: the store and all derived
        // work remain actor-owned, and readers never wait for a reload here.
        let generation = self.warm.as_ref().map(|warm| {
            Arc::new(ReadGeneration {
                snapshot: ReadSnapshot {
                    prj: Arc::clone(&warm.prj),
                    docs: Arc::clone(&warm.docs),
                    findings: Arc::clone(&warm.findings),
                },
                summaries: Arc::clone(&warm.summaries),
                loaded_at: warm.loaded_at.clone(),
                verify_problems: warm.problems.len(),
            })
        });
        let mut published = self.published.lock().unwrap_or_else(|p| p.into_inner());
        let previous = std::mem::replace(&mut published.generation, generation);
        published.load_error = self.load_error.clone();
        published.initialized = true;
        drop(published);
        // This can release the last reference to a whole corpus. Deallocate it
        // after unlocking, just as the replacement was built before locking.
        drop(previous);
    }

    fn load(&self) -> Result<Warm> {
        // `Project::open` searches *upward* for `opys.toml`. A corpus whose own
        // config has gone — a branch switch, a removed worktree — would
        // otherwise start serving the nearest enclosing project, which the user
        // never allowlisted (ADR-0031), under this corpus's cid. Refuse; the
        // manager's next tick retires the corpus properly, and until then reads
        // answer from the last good load rather than from someone else's
        // documents. `crate::action::perform` guards the write path the same way.
        if !self.corpus.root.join("opys.toml").is_file() {
            return Err(OpysError::Usage(format!(
                "{} is no longer an opys project",
                self.corpus.root.display()
            )));
        }
        let prj = Project::open(&self.corpus.root.to_string_lossy())?;
        if prj.root != self.corpus.root {
            return Err(OpysError::Usage(format!(
                "{} resolved to a different project ({})",
                self.corpus.root.display(),
                prj.root.display()
            )));
        }
        let (mut store, parse_errors) = self.backend.load(&prj)?;
        // THE RULE (see the module docs): the flock goes back immediately. A
        // warm store never writes, so it has no business holding it.
        drop(store.take_lock());

        let docs: Vec<Doc> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
        // Unparsable documents arrive as `parse_errors` and are already part of
        // what verify reports, so a broken file shows up as a problem rather
        // than taking the corpus down.
        let mut findings = verify::collect_findings(&prj, &docs, parse_errors);
        // The corpus was just read off the medium and nothing has written since,
        // so the spelling checks (format.md's quoted `history` keys) can run
        // against it too — the dashboard's verify dot then says what `opys
        // verify` says.
        findings.extend(verify::spelling_findings(&docs, self.backend.as_ref()));
        opys_engine::diagnostics::resolve(&prj, &docs, &mut findings, self.backend.as_ref());
        let problems = findings
            .iter()
            .map(|finding| finding.message.clone())
            .collect();
        // No displayed findings means there are no problems a preview could
        // remove. Avoid building a second store and re-verifying a clean
        // corpus; time observations are advertised through their own path.
        let fixable_problems = if findings.is_empty() {
            Vec::new()
        } else {
            verify::fixable_problems(&prj, &docs)?
        };
        // One index for the whole board: blocked is derived from the corpus, not
        // from a document on its own (ADR-0050 decision 3).
        let corpus = refs::Corpus::of_docs(&prj.pcfg, &docs);
        let summaries = docs
            .iter()
            .filter_map(|d| summary(&prj, d, &corpus))
            .collect();
        Ok(Warm {
            prj: Arc::new(prj),
            store,
            docs: Arc::new(docs),
            summaries: Arc::new(summaries),
            problems,
            findings: Arc::new(findings),
            fixable_problems,
            loaded_at: now_rfc3339(),
        })
    }
}

/// The document's type name, from its id prefix.
fn type_of(prj: &Project, id: &str) -> String {
    prj.pcfg
        .type_name_for_id(id)
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Path relative to the project root, for display and for links.
fn rel_path(prj: &Project, doc: &Doc) -> String {
    doc.path
        .strip_prefix(&prj.root)
        .unwrap_or(&doc.path)
        .to_string_lossy()
        .into_owned()
}

/// One board row from a parsed document. `pub(crate)` because the ref reader
/// summarises the documents it decoded out of git objects the same way, and a
/// second spelling of this would let a branch column and a worktree column
/// disagree about what `blocked` means.
pub(crate) fn summary(prj: &Project, doc: &Doc, corpus: &refs::Corpus<'_>) -> Option<DocSummary> {
    let id = doc.id()?.to_string();
    let live_blockers: Vec<String> = refs::live_blockers(doc, corpus)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    Some(DocSummary {
        type_name: type_of(prj, &id),
        status: doc.frontmatter.status().unwrap_or_default().to_string(),
        title: doc.title.clone(),
        tags: doc.frontmatter.tags().unwrap_or_default(),
        path: rel_path(prj, doc),
        updated: doc.frontmatter.get_str("updated").map(str::to_string),
        blocked: !live_blockers.is_empty(),
        live_blockers,
        blocks: live_relations(doc, refs::BLOCKS),
        priority: doc.frontmatter.get("priority").and_then(|v| v.as_i64()),
        id,
    })
}

/// How many entries of a relation map are not struck-through tombstones. The
/// mirror side (`blocks`) only: what makes a *blocker* live is
/// [`refs::live_blockers`], which also asks whether the target has released.
fn live_relations(doc: &Doc, field: &str) -> usize {
    refs::parse_in(&doc.frontmatter, field)
        .into_iter()
        .filter(|(_, title)| !refs::is_struck(title))
        .count()
}

fn view(prj: &Project, doc: &Doc, corpus: &refs::Corpus<'_>) -> DocView {
    let id = doc.id().unwrap_or_default().to_string();
    let mut fields = BTreeMap::new();
    for key in doc.frontmatter.keys() {
        if let Some(value) = doc.frontmatter.get(key) {
            // YAML and JSON agree on every shape frontmatter can hold; anything
            // that somehow does not convert is shown as its debug form rather
            // than dropped.
            let json = serde_json::to_value(value)
                .unwrap_or_else(|_| serde_json::Value::String(format!("{value:?}")));
            fields.insert(key.to_string(), json);
        }
    }
    let doc_type = prj
        .pcfg
        .type_name_for_id(&id)
        .and_then(|name| prj.pcfg.types.get(name));
    let status = doc.frontmatter.status().unwrap_or_default().to_string();
    let live_blockers: Vec<String> = refs::live_blockers(doc, corpus)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let dangling_blockers: Vec<String> = refs::parse_in(&doc.frontmatter, refs::BLOCKED_BY)
        .into_iter()
        .filter(|(id, title)| !refs::is_struck(title) && !corpus.resolves(id))
        .map(|(id, _)| id)
        .collect();
    DocView {
        owners: roles::effective_owners(&prj.pcfg, doc),
        owner_override: roles::is_override(&prj.pcfg, doc),
        type_name: type_of(prj, &id),
        allowed_statuses: doc_type
            .map(|t| {
                t.statuses
                    .iter()
                    .filter(|s| !t.terminal_statuses.contains(s))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        allowed_targets: doc_type.and_then(|t| {
            lifecycle::allowed_targets(t, &status).map(|targets| {
                targets
                    .into_iter()
                    .filter(|s| !t.terminal_statuses.contains(s))
                    .collect()
            })
        }),
        closable: doc_type
            .is_some_and(|t| !t.terminal_statuses.is_empty() && lifecycle::can_close(t, &status)),
        has_terminal_status: doc_type.is_some_and(|t| !t.terminal_statuses.is_empty()),
        log_sections: doc_type
            .map(|t| {
                t.sections
                    .iter()
                    .filter(|s| s.kind.is_log())
                    .map(|s| s.heading.clone())
                    .collect()
            })
            .unwrap_or_default(),
        declared_fields: doc_type
            .map(|t| {
                t.fields
                    .iter()
                    .map(|(name, spec)| FieldView {
                        name: name.clone(),
                        field_type: spec.field_type.as_str(),
                        required: spec.required,
                        values: if spec.field_type == opys_engine::config::FieldType::Role {
                            roles::names(&prj.pcfg)
                                .into_iter()
                                .map(str::to_string)
                                .collect()
                        } else {
                            spec.values.clone()
                        },
                        description: spec.description.clone(),
                        pattern: spec.pattern.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        title: doc.title.clone(),
        path: rel_path(prj, doc),
        tags: doc.frontmatter.tags().unwrap_or_default(),
        updated: doc.frontmatter.get_str("updated").map(str::to_string),
        references: relation(doc, refs::FIELD),
        blocked_by: relation(doc, refs::BLOCKED_BY),
        blocked: !live_blockers.is_empty(),
        live_blockers,
        dangling_blockers,
        blocks: relation(doc, refs::BLOCKS),
        fields,
        body_html: render_markdown(&doc.body),
        body: doc.body.clone(),
        body_version: opys_engine::diagnostics::body_version(&doc.body),
        status,
        id,
    }
}

/// How a document body is rendered.
///
/// GFM, because that is the dialect the corpus is written in: `checklist` is a
/// code-backed section kind, so nearly every document carries a `- [ ]` list,
/// and tables are used throughout. Plain CommonMark renders both as literal
/// text — `<li>[ ] …</li>` and a paragraph of pipes.
///
/// `render.unsafe_` stays off, and must: bodies are user content and the client
/// injects this string with `{@html}`. Raw HTML and `javascript:` hrefs are
/// filtered by that switch, which none of these extensions touches.
pub(crate) fn markdown_options() -> comrak::Options<'static> {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options
}

/// Markdown to HTML exactly as a document body is rendered, for the editor's
/// per-block previews (`POST /api/render`). One renderer for both, so a block
/// looks the same in the editor as in the document it came from.
pub fn render_markdown(markdown: &str) -> String {
    comrak::markdown_to_html(markdown, &markdown_options())
}

/// One relation map as id → title. An absent map is an empty one: a caller
/// iterating `references` should not have to special-case "the key is missing"
/// separately from "there are none".
fn relation(doc: &Doc, field: &str) -> BTreeMap<String, String> {
    refs::parse_in(&doc.frontmatter, field)
        .into_iter()
        .collect()
}

/// Watch the corpus's inventory directory and its `opys.toml`.
///
/// Returns `None` when a watcher cannot be established (an unsupported platform,
/// exhausted inotify watches). That degrades to a corpus that reloads only when
/// asked, which is worth strictly more than refusing to serve it.
fn spawn_watcher(corpus: &Corpus, tx: mpsc::Sender<Message>) -> Option<RecommendedWatcher> {
    let base = corpus.base.clone();
    let config = corpus.root.join("opys.toml");
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if is_change(&event, &base, &config) {
            // The actor debounces; this only has to be cheap and non-blocking.
            let _ = tx.send(Message::Fs);
        }
    })
    .ok()?;
    watcher.watch(&corpus.base, RecursiveMode::Recursive).ok()?;
    // Watch the config's parent, not its inode: editors and checkout tools can
    // replace the file by rename. A file watch would follow the old inode and
    // miss every later save. The callback filters this extra watch to the
    // config itself, so an unrelated markdown file at the root stays quiet.
    // A root inventory is already covered by the recursive watch above.
    if corpus.base != corpus.root {
        // Keep the useful inventory watch if the extra one cannot be installed;
        // explicit reloads still retry reading the config.
        let _ = watcher.watch(&corpus.root, RecursiveMode::NonRecursive);
    }
    Some(watcher)
}

/// Whether a filesystem event should cause a reload.
///
/// **Reads are not changes.** inotify reports opening a file, and a load opens
/// every document in the corpus — so counting `Access` events as changes makes
/// the corpus reload itself in a loop, forever, for as long as the server runs.
/// `a_quiet_corpus_does_not_reload_itself` in `tests/actor.rs` pins this.
fn is_change(event: &notify::Event, base: &Path, config: &Path) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event
        .paths
        .iter()
        .any(|p| p == config || (p.starts_with(base) && is_relevant(p)))
}

/// Whether a changed path should cause a reload. Editors write swap files and
/// the backend touches its lock file; neither is a document.
fn is_relevant(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with(".opys") || name.starts_with("opys-lock-") {
        return false;
    }
    name == "opys.toml" || path.extension().is_some_and(|e| e == "md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn snapshot_fixture() -> (tempfile::TempDir, CorpusHandle) {
        snapshot_fixture_with(Box::new(opys_backend_markdown_local::MarkdownLocal))
    }

    fn snapshot_fixture_with(
        backend: Box<dyn Backend + Send>,
    ) -> (tempfile::TempDir, CorpusHandle) {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let base = root.join("inventory");
        std::fs::create_dir(&base).unwrap();
        std::fs::write(root.join("opys.toml"),
            "base = \"inventory\"\n[types.note]\nprefix = \"NOTE\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n").unwrap();
        std::fs::write(base.join("NOTE-0001.md"),
            "---\nid: NOTE-0001\nstatus: open\nhistory:\n  \"2026-01-01T00:00:00.000000Z\": open\n---\n\n# First\n").unwrap();
        let corpus = Corpus {
            cid: "snapshot-test".into(),
            root,
            base,
            group: "snapshot-test".into(),
            branch: None,
            is_primary: true,
            error: None,
        };
        let (events, _) = broadcast::channel(16);
        let handle = CorpusHandle::spawn(corpus, backend, events);
        (temp, handle)
    }

    #[derive(Default)]
    struct BackendControl {
        gate: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
        extra_doc: bool,
        fail: bool,
    }

    struct GatedBackend(Arc<Mutex<BackendControl>>);

    impl Backend for GatedBackend {
        fn load(&self, prj: &Project) -> Result<(Store, Vec<String>)> {
            let (gate, extra_doc, fail) = {
                let mut control = self.0.lock().unwrap();
                (control.gate.take(), control.extra_doc, control.fail)
            };
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
            if fail {
                return Err(OpysError::Store("test reload failed".into()));
            }
            let (mut store, errors) = opys_backend_markdown_local::MarkdownLocal.load(prj)?;
            if extra_doc {
                let doc = Doc::parse(prj.base.join("NOTE-0002.md"),
                    "---\nid: NOTE-0002\nstatus: open\nhistory:\n  \"2026-01-01T00:00:00.000000Z\": open\n---\n\n# Second\n").unwrap();
                store.put_doc(&prj.pcfg, None, &doc)?;
            }
            Ok((store, errors))
        }

        fn flush(&self, prj: &Project, store: Store) -> Result<()> {
            opys_backend_markdown_local::MarkdownLocal.flush(prj, store)
        }

        fn load_docs(&self, prj: &Project) -> (Vec<Doc>, Vec<String>) {
            opys_backend_markdown_local::MarkdownLocal.load_docs(prj)
        }

        fn stored_text(&self, path: &Path) -> Option<String> {
            opys_backend_markdown_local::MarkdownLocal.stored_text(path)
        }
    }

    fn gate_reload(
        control: &Arc<Mutex<BackendControl>>,
        client: CorpusClient,
    ) -> (mpsc::Receiver<()>, SyncSender<()>, JoinHandle<Result<()>>) {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        control.lock().unwrap().gate = Some((entered, wait));
        let reloader = std::thread::spawn(move || client.reload());
        (ready, release, reloader)
    }

    fn open_query() -> PickQuery {
        PickQuery {
            status: "open".into(),
            type_filter: None,
            role: None,
            limit: 10,
        }
    }

    #[test]
    fn cached_reads_answer_during_reload_and_publish_one_complete_generation() {
        let control = Arc::new(Mutex::new(BackendControl::default()));
        let (_temp, handle) = snapshot_fixture_with(Box::new(GatedBackend(Arc::clone(&control))));
        let old = handle.client.snapshot().unwrap();
        let old_stats = handle.stats().unwrap();
        control.lock().unwrap().extra_doc = true;
        let (ready, release, reloader) = gate_reload(&control, handle.client());
        ready.recv_timeout(Duration::from_secs(3)).unwrap();

        let client = handle.client();
        let (done, result) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let docs = client.docs(DocFilter::default()).unwrap();
            let stats = client.stats().unwrap();
            let picked = client.next(open_query()).unwrap().unwrap();
            let graph = client.flow(5).unwrap();
            let snapshot = client.snapshot().unwrap();
            done.send((docs, stats, picked, graph, snapshot)).unwrap();
        });
        let during = result.recv_timeout(Duration::from_secs(1));
        // Release before asserting, so a regression cannot leave the backend
        // or reader thread waiting forever when the test fails.
        release.send(()).unwrap();
        reloader.join().unwrap().unwrap();
        reader.join().unwrap();
        let (docs, stats, picked, graph, snapshot) = during.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(stats.doc_count, 1);
        assert_eq!(stats.loaded_at, old_stats.loaded_at);
        assert!(stats.load_error.is_none());
        assert_eq!(picked.total, 1);
        assert_eq!(graph.nodes[0].total, 1);
        assert!(Arc::ptr_eq(&old.docs, &snapshot.docs));

        let fresh = handle.client.snapshot().unwrap();
        assert!(!Arc::ptr_eq(&old.docs, &fresh.docs));
        assert_eq!(fresh.docs.len(), 2);
        assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 2);
        assert_eq!(handle.stats().unwrap().doc_count, 2);
        assert_eq!(handle.next(open_query()).unwrap().unwrap().total, 2);

        // A failed reload only updates health; all data stays at the last good
        // generation, including while the failing backend is still blocked.
        control.lock().unwrap().fail = true;
        let (ready, release, reloader) = gate_reload(&control, handle.client());
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(Arc::ptr_eq(
            &fresh.docs,
            &handle.client.snapshot().unwrap().docs
        ));
        assert_eq!(handle.stats().unwrap().doc_count, 2);
        release.send(()).unwrap();
        reloader.join().unwrap().unwrap();
        let failed = handle.stats().unwrap();
        assert_eq!(failed.doc_count, 2);
        assert!(failed.load_error.unwrap().contains("test reload failed"));
        assert!(Arc::ptr_eq(
            &fresh.docs,
            &handle.client.snapshot().unwrap().docs
        ));
        assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 2);
        assert_eq!(handle.next(open_query()).unwrap().unwrap().total, 2);
        handle.shutdown();
    }

    #[test]
    fn cached_reads_keep_initial_load_errors_and_stopped_client_errors() {
        let control = Arc::new(Mutex::new(BackendControl {
            fail: true,
            ..Default::default()
        }));
        let (_temp, handle) = snapshot_fixture_with(Box::new(GatedBackend(control)));
        let client = handle.client();
        for error in [
            client.docs(DocFilter::default()).err().unwrap(),
            client.snapshot().err().unwrap(),
            client.next(open_query()).err().unwrap(),
        ] {
            assert_eq!(error.to_string(),
                "internal store error: corpus is not loaded: internal store error: test reload failed");
        }
        let stats = client.stats().unwrap();
        assert_eq!(stats.doc_count, 0);
        assert!(stats.loaded_at.is_none());
        assert!(stats.load_error.unwrap().contains("test reload failed"));
        handle.shutdown();
        for error in [
            client.docs(DocFilter::default()).err().unwrap(),
            client.stats().err().unwrap(),
            client.snapshot().err().unwrap(),
            client.next(open_query()).err().unwrap(),
        ] {
            assert!(
                error.to_string().contains("corpus actor stopped"),
                "{error}"
            );
        }
    }

    #[test]
    fn dropping_a_handle_invalidates_cached_clients_even_during_reload() {
        let control = Arc::new(Mutex::new(BackendControl::default()));
        let (_temp, handle) = snapshot_fixture_with(Box::new(GatedBackend(Arc::clone(&control))));
        let client = handle.client();
        assert_eq!(client.docs(DocFilter::default()).unwrap().len(), 1);
        let (ready, release, reloader) = gate_reload(&control, client.clone());
        ready.recv_timeout(Duration::from_secs(3)).unwrap();

        // Drop must not join a blocked load, and its cached clients must become
        // unavailable before the queued Shutdown can reach the actor loop.
        drop(handle);
        for error in [
            client.docs(DocFilter::default()).err().unwrap(),
            client.stats().err().unwrap(),
            client.snapshot().err().unwrap(),
        ] {
            assert!(
                error.to_string().contains("corpus actor stopped"),
                "{error}"
            );
        }
        release.send(()).unwrap();
        reloader.join().unwrap().unwrap();
        // The completed reload must preserve the stopped flag while publishing.
        assert!(client.docs(DocFilter::default()).is_err());
        assert!(client
            .reload()
            .unwrap_err()
            .to_string()
            .contains("corpus actor stopped"));
    }

    #[test]
    fn read_snapshots_reuse_inputs_and_swap_only_after_a_successful_reload() {
        let (_temp, handle) = snapshot_fixture();
        let first = handle.client.snapshot().unwrap();
        let same = handle.client.snapshot().unwrap();
        assert!(Arc::ptr_eq(&first.prj, &same.prj));
        assert!(Arc::ptr_eq(&first.docs, &same.docs));
        assert!(Arc::ptr_eq(&first.findings, &same.findings));
        assert_eq!(first.docs[0].title, "First");
        assert!(handle.verify().unwrap().fixable_problems.is_empty());

        let path = handle.corpus.base.join("NOTE-0001.md");
        let source = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, source.replace("# First", "# Second")).unwrap();
        handle.reload().unwrap();
        let fresh = handle.client.snapshot().unwrap();
        assert!(!Arc::ptr_eq(&first.docs, &fresh.docs));
        assert!(!Arc::ptr_eq(&first.prj, &fresh.prj));
        assert!(!Arc::ptr_eq(&first.findings, &fresh.findings));
        assert_eq!(fresh.docs[0].title, "Second");
        assert_eq!(
            first.docs[0].title, "First",
            "an in-flight read keeps its generation"
        );

        std::fs::write(handle.corpus.root.join("opys.toml"), "not = = toml").unwrap();
        handle.reload().unwrap();
        let last_good = handle.client.snapshot().unwrap();
        assert!(Arc::ptr_eq(&fresh.docs, &last_good.docs));
        assert!(Arc::ptr_eq(&fresh.prj, &last_good.prj));
        assert!(Arc::ptr_eq(&fresh.findings, &last_good.findings));
        assert!(handle.stats().unwrap().load_error.is_some());
        handle.shutdown();
    }

    #[test]
    fn a_paused_snapshot_consumer_does_not_block_cached_docs_or_stats() {
        let (_temp, handle) = snapshot_fixture();
        let client = handle.client();
        let (started, ready) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let builder = std::thread::spawn(move || {
            let snapshot = client.snapshot().unwrap();
            started.send(()).unwrap();
            wait.recv().unwrap();
            flow::flow(&snapshot.prj, &snapshot.docs, &client.cid, 5)
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();

        let client = handle.client();
        let (finished, result) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let docs = client.docs(DocFilter::default()).unwrap();
            let stats = client.stats().unwrap();
            finished.send((docs.len(), stats.doc_count)).unwrap();
        });
        let read = result.recv_timeout(Duration::from_secs(1));
        // Always let the consumer finish, including on a failed assertion.
        release.send(()).unwrap();
        let graph = builder.join().unwrap();
        reader.join().unwrap();
        assert_eq!(read.unwrap(), (1, 1));
        assert_eq!(graph.nodes[0].items[0].title, "First");
        handle.shutdown();
    }

    #[test]
    fn only_documents_and_config_are_relevant() {
        assert!(is_relevant(Path::new("/p/inventory/FEAT-0001.md")));
        assert!(is_relevant(Path::new("/p/opys.toml")));
        assert!(!is_relevant(Path::new("/p/inventory/.opys.lock")));
        assert!(!is_relevant(Path::new("/run/user/1000/opys-lock-abc-123")));
        assert!(!is_relevant(Path::new("/p/inventory/notes.txt")));
        assert!(!is_relevant(Path::new("/p/inventory/.FEAT-0001.md.swp")));
    }

    #[test]
    fn filters_are_conjunctive_and_none_matches_all() {
        let d = DocSummary {
            id: "FEAT-0001".into(),
            type_name: "feature".into(),
            status: "planned".into(),
            title: "T".into(),
            tags: vec!["server".into(), "core".into()],
            path: "inventory/FEAT-0001.md".into(),
            updated: None,
            blocked: false,
            live_blockers: Vec::new(),
            blocks: 0,
            priority: None,
        };
        assert!(DocFilter::default().matches(&d));
        assert!(DocFilter {
            type_name: Some("feature".into()),
            status: Some("planned".into()),
            tag: Some("core".into()),
        }
        .matches(&d));
        assert!(!DocFilter {
            type_name: Some("bug".into()),
            ..Default::default()
        }
        .matches(&d));
        assert!(!DocFilter {
            tag: Some("ui".into()),
            ..Default::default()
        }
        .matches(&d));
    }

    #[test]
    fn reads_are_not_changes() {
        use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind};
        use notify::{Event as N, EventKind};

        let doc = PathBuf::from("/p/inventory/FEAT-0001.md");
        let opened =
            N::new(EventKind::Access(AccessKind::Open(AccessMode::Any))).add_path(doc.clone());
        assert!(
            !is_change(
                &opened,
                Path::new("/p/inventory"),
                Path::new("/p/opys.toml")
            ),
            "opening a document is what loading does; it must not trigger another load"
        );
        let read = N::new(EventKind::Access(AccessKind::Read)).add_path(doc.clone());
        assert!(!is_change(
            &read,
            Path::new("/p/inventory"),
            Path::new("/p/opys.toml")
        ));

        let written =
            N::new(EventKind::Modify(ModifyKind::Data(DataChange::Any))).add_path(doc.clone());
        assert!(
            is_change(
                &written,
                Path::new("/p/inventory"),
                Path::new("/p/opys.toml")
            ),
            "a write is a change"
        );
        let created = N::new(EventKind::Create(CreateKind::File)).add_path(doc);
        assert!(is_change(
            &created,
            Path::new("/p/inventory"),
            Path::new("/p/opys.toml")
        ));

        let noise = N::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(PathBuf::from("/p/inventory/notes.txt"));
        assert!(
            !is_change(&noise, Path::new("/p/inventory"), Path::new("/p/opys.toml")),
            "only documents and the config matter"
        );
    }

    #[test]
    fn root_watch_ignores_unrelated_markdown_and_accepts_config_renames() {
        use notify::event::{ModifyKind, RenameMode};
        use notify::{Event as N, EventKind};

        let base = Path::new("/p/inventory");
        let config = Path::new("/p/opys.toml");
        let root_markdown =
            N::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("/p/README.md"));
        assert!(!is_change(&root_markdown, base, config));
        let staged =
            N::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("/p/opys.toml.next"));
        assert!(!is_change(&staged, base, config));
        let renamed = N::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/p/opys.toml.next"))
            .add_path(config.to_path_buf());
        assert!(is_change(&renamed, base, config));
    }

    #[test]
    fn swap_and_lock_paths_never_wake_the_actor() {
        // The lock file moved under $XDG_RUNTIME_DIR, but older inventories may
        // still carry one; either way it must not cause reload storms.
        for p in [
            PathBuf::from("/p/inventory/.opys.lock"),
            PathBuf::from("/tmp/opys-lock-_home_dan-0000"),
        ] {
            assert!(!is_relevant(&p), "{} should be ignored", p.display());
        }
    }
}
