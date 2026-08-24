//! One actor thread per corpus, owning a warm [`Store`] (TASK-0070).
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
use opys_engine::project::Project;
use opys_engine::refs;
use opys_engine::store::Store;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::discover::Corpus;

/// Quiet period a filesystem burst must clear before it becomes one reload.
/// `opys sync` rewriting forty files is one event, not forty.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// What happened, for the WebSocket fan-out in TASK-0071.
///
/// Payloads are counts, not documents: subscribers refetch what they need over
/// the API. A broadcast channel clones its message per receiver and buffers for
/// slow ones, so pushing whole corpora through it would cost memory
/// proportional to viewers times corpus size for data most of them already have.
///
/// On the wire this is `{"event": "corpus-reloaded", …}` — the discriminant is
/// kebab-case because that is the vocabulary TASK-0071 and TASK-0072 write
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
    /// A write completed through the API (TASK-0072). `action` is the request's
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
}

/// One row of the board: everything a list view needs without opening a doc.
#[derive(Debug, Clone, Serialize)]
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
    /// Every status `set-status` will accept for this document's type: the
    /// type's declared statuses minus its terminal ones, which are reachable
    /// only through `close`.
    ///
    /// Here rather than in the client so nothing outside this crate has to read
    /// `opys.toml` — a UI that knew the status vocabulary would be a second
    /// interpretation of the config, drifting the moment a type is edited.
    /// Empty when the id's prefix matches no configured type, which is the same
    /// answer `type_name` gives.
    pub allowed_statuses: Vec<String>,
    /// Whether `close` is even possible: a type with no terminal status has
    /// nothing to close *to*, and the engine refuses. Without this the UI's
    /// close button would 422 on, say, an ADR no matter what the user did —
    /// [`DocView::allowed_statuses`] cannot express it, because a terminal
    /// status is precisely what it leaves out.
    pub closable: bool,
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
    /// What this document blocks, id → title.
    pub blocks: BTreeMap<String, String>,
    /// Every frontmatter key, as JSON.
    pub fields: BTreeMap<String, serde_json::Value>,
    /// The markdown body, verbatim.
    pub body: String,
    /// The body rendered. Raw HTML in the source stays escaped — comrak's
    /// `unsafe_` is off, and must stay off: bodies are user content.
    pub body_html: String,
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
            Request::Verify(reply) => {
                let _ = reply.send(VerifyStatus {
                    ok: self.load_error.is_none()
                        && self.warm.as_ref().is_some_and(|w| w.problems.is_empty()),
                    problems: self
                        .warm
                        .as_ref()
                        .map(|w| w.problems.clone())
                        .unwrap_or_default(),
                    loaded_at: self.warm.as_ref().map(|w| w.loaded_at.clone()),
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
        Ok(Some(view(&warm.prj, &doc)))
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
        // never allowlisted (ADR-0077), under this corpus's cid. Refuse; the
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
        let problems = verify::collect_problems(&prj, &docs, parse_errors);
        let summaries = docs.iter().filter_map(|d| summary(&prj, d)).collect();
        Ok(Warm {
            prj,
            store,
            summaries,
            problems,
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

fn summary(prj: &Project, doc: &Doc) -> Option<DocSummary> {
    let id = doc.id()?.to_string();
    Some(DocSummary {
        type_name: type_of(prj, &id),
        status: doc.frontmatter.status().unwrap_or_default().to_string(),
        title: doc.title.clone(),
        tags: doc.frontmatter.tags().unwrap_or_default(),
        path: rel_path(prj, doc),
        updated: doc.frontmatter.get_str("updated").map(str::to_string),
        id,
    })
}

fn view(prj: &Project, doc: &Doc) -> DocView {
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
    DocView {
        type_name: type_of(prj, &id),
        status: doc.frontmatter.status().unwrap_or_default().to_string(),
        allowed_statuses: doc_type
            .map(|t| {
                t.statuses
                    .iter()
                    .filter(|s| !t.terminal_statuses.contains(s))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        closable: doc_type.is_some_and(|t| !t.terminal_statuses.is_empty()),
        title: doc.title.clone(),
        path: rel_path(prj, doc),
        tags: doc.frontmatter.tags().unwrap_or_default(),
        updated: doc.frontmatter.get_str("updated").map(str::to_string),
        references: relation(doc, refs::FIELD),
        blocked_by: relation(doc, refs::BLOCKED_BY),
        blocks: relation(doc, refs::BLOCKS),
        fields,
        body_html: comrak::markdown_to_html(&doc.body, &markdown_options()),
        body: doc.body.clone(),
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
fn markdown_options() -> comrak::Options<'static> {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options
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
