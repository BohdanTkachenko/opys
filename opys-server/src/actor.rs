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
use opys_engine::refs;
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
    Docs(DocFilter, SyncSender<Result<Vec<DocSummary>>>),
    Doc(String, SyncSender<Result<Option<DocView>>>),
    Query(
        String,
        Vec<String>,
        SyncSender<Result<std::result::Result<QueryResult, String>>>,
    ),
    /// Same double `Result` as [`Request::Query`]: the inner `Err` is the
    /// caller's bad `status`/`type` (a 400), the outer one is ours (a 500).
    Next(
        PickQuery,
        SyncSender<Result<std::result::Result<PickResult, String>>>,
    ),
    /// The lifecycle graph, with the per-node item cap. A single `Result`,
    /// unlike [`Request::Next`]: the only thing the caller supplies is a
    /// number, and every status in the answer comes from the config itself, so
    /// there is no user vocabulary left to refuse.
    Flow(usize, SyncSender<Result<FlowView>>),
    Verify(SyncSender<VerifyStatus>),
    Stats(SyncSender<CorpusStats>),
    /// Reload now and reply when done — the deterministic path for tests and
    /// for the manager after a registry change.
    Reload(SyncSender<()>),
    Shutdown,
}

/// The request-sending half of a corpus actor: `Clone + Send + Sync`, and cheap
/// (one `String` and one channel sender).
///
/// This exists so a request handler never holds the manager's lock while it
/// blocks. Every call below waits on a channel, and the actor may be halfway
/// through a reload; a handler that kept the lock for that would stall the
/// periodic passes and every other request behind one slow corpus. The pattern
/// is: lock, clone the client, drop the lock, then block on the client.
///
/// A client may outlive its [`CorpusHandle`] — a rescan can stop the corpus
/// while a request is in flight. Calls then fail with "corpus actor stopped"
/// rather than hanging.
#[derive(Clone)]
pub struct CorpusClient {
    /// Which corpus this talks to, for error messages and log lines.
    pub cid: String,
    tx: mpsc::Sender<Message>,
}

/// A handle to one corpus actor: the thread, the corpus it serves, and the
/// [`CorpusClient`] used to talk to it. The manager owns handles; everyone else
/// gets a client.
pub struct CorpusHandle {
    pub corpus: Corpus,
    client: CorpusClient,
    join: Option<JoinHandle<()>>,
}

fn gone() -> OpysError {
    OpysError::Store("corpus actor stopped".to_string())
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
        self.ask(|reply| Request::Docs(filter, reply))?
    }

    /// One document with its rendered body, or `None` if this corpus has no
    /// such id. Fails if the corpus has never loaded — `None` would claim the
    /// document does not exist.
    pub fn doc(&self, id: &str) -> Result<Option<DocView>> {
        self.ask(|reply| Request::Doc(id.to_string(), reply))?
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
        self.ask(|reply| Request::Next(q, reply))?
    }

    /// The lifecycle graph over the warm store, with `limit` items per node.
    /// Fails only if the actor is gone or the corpus never loaded.
    pub fn flow(&self, limit: usize) -> Result<FlowView> {
        self.ask(|reply| Request::Flow(limit, reply))?
    }

    /// Cached verify problems and the time they were computed.
    pub fn verify(&self) -> Result<VerifyStatus> {
        self.ask(Request::Verify)
    }

    /// Document count and health in one round trip.
    pub fn stats(&self) -> Result<CorpusStats> {
        self.ask(Request::Stats)
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
        let join = std::thread::Builder::new()
            .name(format!("opys-corpus-{}", corpus.cid))
            .spawn(move || {
                // Held for the life of the loop: dropping a notify watcher stops
                // it delivering.
                let _watcher = spawn_watcher(&thread_corpus, watch_tx);
                let mut actor = Actor {
                    corpus: thread_corpus,
                    backend,
                    events,
                    warm: None,
                    load_error: None,
                };
                actor.reload();
                actor.run(rx);
            })
            .expect("spawning a corpus thread");
        CorpusHandle {
            client: CorpusClient {
                cid: corpus.cid.clone(),
                tx,
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
        let _ = self.client.tx.send(Message::Req(Request::Shutdown));
    }
}

/// Everything the actor keeps between requests. Replaced wholesale on a
/// successful reload, never mutated piecemeal.
struct Warm {
    prj: Project,
    store: Store,
    summaries: Vec<DocSummary>,
    problems: Vec<String>,
    findings: Vec<opys_engine::diagnostics::Finding>,
    fixable_problems: Vec<String>,
    loaded_at: String,
}

struct Actor {
    corpus: Corpus,
    backend: Box<dyn Backend + Send>,
    events: broadcast::Sender<Event>,
    warm: Option<Warm>,
    load_error: Option<String>,
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
            Request::Docs(filter, reply) => {
                let out = self.warm().map(|w| {
                    w.summaries
                        .iter()
                        .filter(|d| filter.matches(d))
                        .cloned()
                        .collect()
                });
                let _ = reply.send(out);
            }
            Request::Doc(id, reply) => {
                let _ = reply.send(self.doc_view(&id));
            }
            Request::Query(sql, params, reply) => {
                let _ = reply.send(self.run_query(&sql, &params));
            }
            Request::Next(q, reply) => {
                let _ = reply.send(self.run_next(&q));
            }
            Request::Flow(limit, reply) => {
                let _ = reply.send(self.run_flow(limit));
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
                        .map(|w| w.findings.clone())
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
            Request::Stats(reply) => {
                let _ = reply.send(CorpusStats {
                    doc_count: self.warm.as_ref().map_or(0, |w| w.summaries.len()),
                    verify_problems: self.warm.as_ref().map_or(0, |w| w.problems.len()),
                    loaded_at: self.warm.as_ref().map(|w| w.loaded_at.clone()),
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
            let why = match &self.load_error {
                Some(e) => format!("corpus is not loaded: {e}"),
                None => "corpus is not loaded".to_string(),
            };
            return Err(OpysError::Store(why));
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

    /// Answer the pick oracle from the warm store.
    ///
    /// The documents are reconstructed per request, exactly as [`run_query`]
    /// rebuilds the projections per request: [`next::pick`] needs whole
    /// documents (relation maps and all) and the summaries the board is served
    /// from carry no `blocks` map, so effective priority cannot come from them.
    ///
    /// [`run_query`]: Actor::run_query
    fn run_next(&mut self, q: &PickQuery) -> Result<std::result::Result<PickResult, String>> {
        let warm = self.warm()?;
        if let Err(msg) = q.validate(&warm.prj.pcfg) {
            return Ok(Err(msg));
        }
        let docs: Vec<Doc> = warm.store.all_docs()?.into_iter().map(|(_, d)| d).collect();
        Ok(Ok(next::pick(&warm.prj, &docs, q, observe::now())))
    }

    /// Answer the flow graph from the warm store.
    ///
    /// The documents are reconstructed **once** and every node is computed from
    /// that one set — the alternative, one `/next` request per status, would
    /// reconstruct the whole corpus a dozen times over and still could not
    /// answer the blocked count, which is not part of a [`PickResult`].
    ///
    /// The cid is cloned before [`Actor::warm`] borrows `self` mutably.
    fn run_flow(&mut self, limit: usize) -> Result<FlowView> {
        let cid = self.corpus.cid.clone();
        let warm = self.warm()?;
        let docs: Vec<Doc> = warm.store.all_docs()?.into_iter().map(|(_, d)| d).collect();
        Ok(flow::flow(&warm.prj, &docs, &cid, limit))
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
                // Err just means nobody is subscribed.
                let _ = self.events.send(event);
            }
            Err(e) => self.load_error = Some(e.to_string()),
        }
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
        let fixable_problems = verify::fixable_problems(&prj, &docs)?;
        // One index for the whole board: blocked is derived from the corpus, not
        // from a document on its own (ADR-0050 decision 3).
        let corpus = refs::Corpus::of_docs(&prj.pcfg, &docs);
        let summaries = docs
            .iter()
            .filter_map(|d| summary(&prj, d, &corpus))
            .collect();
        Ok(Warm {
            prj,
            store,
            summaries,
            problems,
            findings,
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
                        values: spec.values.clone(),
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
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if is_change(&event) {
            // The actor debounces; this only has to be cheap and non-blocking.
            let _ = tx.send(Message::Fs);
        }
    })
    .ok()?;
    watcher.watch(&corpus.base, RecursiveMode::Recursive).ok()?;
    // The config sits outside the inventory directory, so it needs its own
    // watch. Not fatal if it fails — config edits are rare and the periodic
    // re-check catches them.
    let _ = watcher.watch(&corpus.root.join("opys.toml"), RecursiveMode::NonRecursive);
    Some(watcher)
}

/// Whether a filesystem event should cause a reload.
///
/// **Reads are not changes.** inotify reports opening a file, and a load opens
/// every document in the corpus — so counting `Access` events as changes makes
/// the corpus reload itself in a loop, forever, for as long as the server runs.
/// `a_quiet_corpus_does_not_reload_itself` in `tests/actor.rs` pins this.
fn is_change(event: &notify::Event) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|p| is_relevant(p))
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
            !is_change(&opened),
            "opening a document is what loading does; it must not trigger another load"
        );
        let read = N::new(EventKind::Access(AccessKind::Read)).add_path(doc.clone());
        assert!(!is_change(&read));

        let written =
            N::new(EventKind::Modify(ModifyKind::Data(DataChange::Any))).add_path(doc.clone());
        assert!(is_change(&written), "a write is a change");
        let created = N::new(EventKind::Create(CreateKind::File)).add_path(doc);
        assert!(is_change(&created));

        let noise = N::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(PathBuf::from("/p/inventory/notes.txt"));
        assert!(!is_change(&noise), "only documents and the config matter");
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
