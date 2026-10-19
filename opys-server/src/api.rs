//! The API — reads and the event stream, and writes.
//!
//! Everything is JSON, there is no auth (a loopback bind is the boundary in M1),
//! and every failure is `{"error": "<message>"}` with 400 for bad input, 404 for
//! an unknown corpus or project group (and, on the read routes, an unknown
//! document), 422 for a write the corpus refused — including a write naming an
//! id that does not exist, which is one member of the "the CLI would have exited
//! 2" class rather than a routing miss — 503 for a write the node was too busy
//! to attempt, and 500 for anything internal. That includes the failures axum
//! would otherwise render as `text/plain` — a rejected body, a malformed query
//! string, an unroutable path — because a client that parses one error shape
//! should never meet a second.
//!
//! **No endpoint executes commands or accepts a filesystem path** (ADR-0031).
//! Corpora are addressed by their opaque `cid`, and the set of them comes from
//! the allowlist file the user owns — the route surface here is exhaustive. The
//! one endpoint that writes takes a closed enum of the mutating commands and
//! their arguments, never anything shell- or path-shaped; see [`crate::action`].
//!
//! A loopback bind keeps other machines out, but not the user's own browser:
//! any page they visit can `fetch` a localhost URL, and the same-origin policy
//! does not apply to a WebSocket handshake at all. [`AppState::check_local`]
//! closes both — see its docs — and is the one thing here that answers 403.
//!
//! Two rules shape the code below:
//!
//! - The manager lock is a [`std::sync::Mutex`], and a background pass can hold
//!   it for seconds (a rescan walks the filesystem and joins actor threads). No
//!   handler takes it on the reactor: the lock, the clone and the blocking call
//!   all happen inside one [`tokio::task::spawn_blocking`].
//! - Actor calls block on a channel, so they run on the blocking pool. A read
//!   that lands mid-reload waits out the whole load; on the reactor that would
//!   stall unrelated requests.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use opys_engine::commands::now_rfc3339;
use opys_engine::error::OpysError;
use opys_engine::next::{PickQuery, PickResult, DEFAULT_LIMIT};
use opys_engine::queue::{QueueQuery, QueueResult};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::action::{Action, ActionError};
use crate::actor::{
    CorpusClient, CorpusHandle, CorpusStats, DocFilter, DocSummary, DocView, DraftVerification,
    Event, QueryResult, QueueQueryError, VerifyStatus,
};
use crate::discover::Corpus;
use crate::flow::{FlowView, DEFAULT_NODE_LIMIT};
use crate::manager::Manager;
use crate::registry;
use crate::role_queues::{self, ProjectQueues, RoleView, RolesView};
use crate::union::{contested_numbers, union, Branches, CorpusDocs, UnionView};

/// How often the server pings an idle WebSocket client.
const PING_EVERY: Duration = Duration::from_secs(30);

/// How many pings may go unanswered before the client is dropped. At
/// [`PING_EVERY`] that is a minute and a half of silence from a peer that has
/// stopped reading — long enough to survive a suspended laptop's first hiccup,
/// short enough that dead sockets do not accumulate.
const MISSED_PINGS_ALLOWED: u32 = 2;

/// How long the project list waits on any one corpus.
///
/// A corpus mid-reload can sit on the inventory flock for `OPYS_LOCK_TIMEOUT_MS`
/// (10 s by default). The landing page must not wait that out — and must not
/// wait out several of them in turn — so a corpus that does not answer in time
/// is reported as busy and the rest of the list goes out.
const STATS_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the union view waits on any one corpus of a group.
///
/// Same reasoning as [`STATS_TIMEOUT`], and the same two seconds: a worktree
/// that is mid-reload must not hold up the columns that are ready, and several
/// of them must not add up. A column that misses the deadline is labelled busy
/// rather than silently rendered empty — see [`crate::union::Column::error`].
const UNION_TIMEOUT: Duration = Duration::from_secs(2);

/// The WebSocket liveness settings.
///
/// Constants everywhere else; a field here only so a test can drive the ping /
/// missed-pong path at millisecond scale instead of needing a minute and a half
/// of wall clock.
#[derive(Clone, Copy, Debug)]
struct PingConfig {
    every: Duration,
    missed_allowed: u32,
}

impl Default for PingConfig {
    fn default() -> PingConfig {
        PingConfig {
            every: PING_EVERY,
            missed_allowed: MISSED_PINGS_ALLOWED,
        }
    }
}

/// Everything a handler needs. Cheap to clone: two pointers and a timestamp.
#[derive(Clone)]
pub struct AppState {
    /// The live corpus actors. See the module docs for the locking rule.
    pub manager: Arc<Mutex<Manager>>,
    /// The fan-out every WebSocket client subscribes to.
    pub events: broadcast::Sender<Event>,
    /// When this process started, RFC3339. Reported by `/api/health` so a client
    /// can tell "the node is up" from "the node restarted under me".
    pub started: String,
    /// Where the node listens, when it is known. `None` means "assume the
    /// loopback default", which is the safe way to be wrong.
    bind: Option<SocketAddr>,
    /// Whether the first rescan has finished. The node binds before it scans
    /// so an empty project list early on means "not yet looked",
    /// not "nothing allowlisted" — and only this tells the two apart.
    scanned: Option<Arc<AtomicBool>>,
    ping: PingConfig,
}

impl AppState {
    /// Wrap a manager and its event channel for serving.
    pub fn new(manager: Arc<Mutex<Manager>>, events: broadcast::Sender<Event>) -> AppState {
        AppState {
            manager,
            events,
            started: now_rfc3339(),
            bind: None,
            scanned: None,
            ping: PingConfig::default(),
        }
    }

    /// Tell the state where the node listens, which is what decides whether the
    /// `Host` guard applies (see [`AppState::check_local`]).
    pub fn with_bind(mut self, bind: SocketAddr) -> AppState {
        self.bind = Some(bind);
        self
    }

    /// Share the flag the startup rescan sets when it finishes.
    ///
    /// Absent means "assume scanned": a state built without one (every test that
    /// wires the manager directly) has no pending first pass to wait for.
    pub fn with_scanned(mut self, scanned: Arc<AtomicBool>) -> AppState {
        self.scanned = Some(scanned);
        self
    }

    /// Ping WebSocket clients this often instead of every 30 s. For tests.
    pub fn with_ping_interval(mut self, every: Duration) -> AppState {
        self.ping.every = every;
        self
    }

    /// Whether a request may be served at all.
    ///
    /// A loopback bind stops other machines, not other *origins*: a page the
    /// user visits can point a `fetch` at `127.0.0.1`, and a WebSocket
    /// handshake is not subject to the same-origin policy in the first place,
    /// so `ws://127.0.0.1:6797/api/events` is readable by any site unless the
    /// server itself refuses. DNS rebinding is the same story for the rest of
    /// the API: an attacker's name resolving to 127.0.0.1 makes their page
    /// same-origin with the node. Both attacks have to send a header we can
    /// check — a browser always sends `Host`, and always sends `Origin` on a
    /// cross-origin request and on every WebSocket handshake.
    ///
    /// So: the `Host` must name loopback (rebinding cannot forge that, because
    /// the browser sends the attacker's own name), and an `Origin`, if present,
    /// must be loopback or the very host the request was addressed to. A node
    /// deliberately bound to a non-loopback address has opted out of the first
    /// check — it is presumably reached by a name we cannot know — but keeps the
    /// second, which is what a reverse proxy in front of it satisfies anyway.
    fn check_local(&self, uri: &Uri, headers: &HeaderMap) -> Result<(), ApiError> {
        // HTTP/2 puts the authority in the URI; HTTP/1.1 in the header.
        let host = uri
            .host()
            .map(str::to_owned)
            .or_else(|| header_str(headers, HOST).map(str::to_owned));
        let guarded = self.bind.is_none_or(|addr| addr.ip().is_loopback());
        if guarded {
            if let Some(host) = host.as_deref().filter(|h| !is_loopback_host(h)) {
                return Err(ApiError::forbidden(format!(
                    "refusing a request for {host:?}: this node serves loopback only"
                )));
            }
        }
        if let Some(origin) = header_str(headers, ORIGIN) {
            let same_site = origin_host(origin).is_some_and(|o| {
                is_loopback_host(o)
                    || host
                        .as_deref()
                        .is_some_and(|h| host_only(h).eq_ignore_ascii_case(o))
            });
            if !same_site {
                return Err(ApiError::forbidden(format!(
                    "refusing a cross-origin request from {origin:?}"
                )));
            }
        }
        Ok(())
    }
}

/// One header, when it is present and text.
fn header_str(headers: &HeaderMap, name: axum::http::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The host part of an authority: `[::1]:6797` → `::1`, `localhost:6797` →
/// `localhost`, `127.0.0.1` → `127.0.0.1`.
fn host_only(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        // An IPv6 literal, which is the only bracketed form.
        return rest.split(']').next().unwrap_or(rest);
    }
    if authority.parse::<IpAddr>().is_ok() {
        // A bare `::1` — not legal in a `Host`, but cheap to accept.
        return authority;
    }
    authority.split(':').next().unwrap_or(authority)
}

/// Whether an authority names this machine.
fn is_loopback_host(authority: &str) -> bool {
    let host = host_only(authority);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The host of an `Origin` header. `None` for anything that is not a real
/// origin — notably the literal `null` a sandboxed frame sends.
fn origin_host(origin: &str) -> Option<&str> {
    let (_scheme, rest) = origin.split_once("://")?;
    let host = host_only(rest);
    (!host.is_empty()).then_some(host)
}

/// Take the manager lock, ignoring poisoning.
///
/// A panic inside one background pass must not turn every later request into a
/// 500: the manager's own state stays consistent because each pass either
/// completes or leaves a corpus out, and the alternative — a node that answers
/// nothing until restarted — is strictly worse.
///
/// **Only ever called inside a blocking task.** See the module docs.
fn lock(manager: &Mutex<Manager>) -> MutexGuard<'_, Manager> {
    manager.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every route the node serves. Kept as one function so tests can drive it with
/// `tower::ServiceExt::oneshot` instead of binding a socket.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/projects", get(projects))
        // Markdown in, HTML out: the document editor's block previews. Pure —
        // it reads no corpus and no file, so it needs no cid.
        .route("/api/render", post(render))
        // Allowlist management from the browser (ADR-0035). Every path these
        // accept goes through `registry::vet_ui_path` first; none of them can
        // reach outside `$HOME` or into a hidden directory.
        .route("/api/setup", get(setup).post(save_setup))
        .route("/api/suggestions", get(suggestions))
        .route("/api/allowlist", post(allowlist))
        // axum 0.8 path captures are `{name}`; the 0.7 `:name` form panics.
        .route("/api/group/{key}/union", get(group_union))
        // Project-scoped, not corpus-scoped, and deliberately so: pickability
        // is evaluated on **main** (ADR-0053), so the caller names the project
        // and the node picks its primary worktree rather than making the client
        // guess which of a group's corpora is the truth. `{key}` is the same
        // key `/api/projects` returns.
        .route("/api/projects/{key}/next", get(pick))
        // The lifecycle graph of one project, answered from the same primary
        // corpus and for the same reason (FEAT-0060).
        .route("/api/projects/{key}/flow", get(flow))
        .route("/api/projects/{key}/queue/{role}", get(queue))
        .route("/api/roles", get(roles))
        .route("/api/roles/{role}", get(role))
        .route("/api/corpus/{cid}/docs", get(docs))
        .route("/api/corpus/{cid}/doc/{docid}", get(doc))
        .route("/api/corpus/{cid}/doc/{docid}/verify", post(verify_body))
        .route("/api/corpus/{cid}/query", post(query))
        .route("/api/corpus/{cid}/verify", get(verify))
        .route("/api/corpus/{cid}/action", post(action))
        // `any`, not `get`: that is what enables WebSockets over HTTP/2, where
        // the handshake is a CONNECT rather than a GET.
        .route("/api/events", any(events))
        // The web UI, compiled into the binary (ADR-0037). The SPA is hash
        // routed, so `/` is the only document path there ever is and every
        // other view is a fragment the server never sees.
        .route("/", get(ui_index))
        .route("/ui/{*path}", get(ui_asset))
        // The installable-app files (FEAT-0064). One segment only, and the
        // handler serves nothing outside `assets::ROOT_FILES`, so every other
        // root path still gets the JSON 404 below.
        .route("/{file}", get(ui_root_file))
        // Routing-level failures answer in the same shape as everything else;
        // axum's own would be an empty body with no content type.
        .fallback(no_route)
        .method_not_allowed_fallback(wrong_method)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Refuse anything that is not addressed to this node — before it reaches a
/// route, so `/api/events` is covered too (it is the one endpoint a foreign page
/// could otherwise read).
async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Err(refusal) = state.check_local(request.uri(), request.headers()) {
        return refusal.into_response();
    }
    next.run(request).await
}

/// A failure, in the one shape every endpoint uses.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    /// The caller sent something unusable — malformed JSON, rejected SQL.
    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, message)
    }

    /// The request was well formed but is not allowed to be answered: it came
    /// from another origin, or was addressed to a name that is not this node.
    pub fn forbidden(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, message)
    }

    /// No such corpus, or no such document in it.
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, message)
    }

    /// The corpus refused a write: an unknown status, a terminal status reached
    /// without `close`, an unchecked test-plan item, an id that resolves to
    /// nothing. The message is the engine's own, which is the text the CLI
    /// prints before exiting 2.
    ///
    /// Every [`OpysError`] the write cycle can raise lands here, including the
    /// two that are arguably the node's rather than the caller's (a failed disk
    /// write, an internal store bug). That is deliberate: "the CLI would have
    /// exited 2" is one outcome a client can present as one thing, and splitting
    /// it across 422 and 500 would make a caller parse two shapes to learn the
    /// same fact — the write did not happen, here is why. Reads keep the older
    /// rule, where an engine error really is the state of the node ([`ApiError`]'s
    /// `From<OpysError>`, which is a 500 and must not be reached from a write).
    pub fn unprocessable(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    /// The node could not attempt the write, but nothing about the request was
    /// wrong: another `opys` invocation held the inventory lock past the
    /// timeout. A retry is the right response, which is precisely what a 422
    /// would not have said.
    pub fn unavailable(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    /// Our fault, not the caller's.
    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// This binary cannot answer that route at all — it was built without the
    /// `web-ui` feature, so there is no bundle to serve. Deliberately not a 404:
    /// the route exists and the URL is right, and telling a confused operator
    /// "not found" would send them looking for a typo instead of at their build.
    pub fn not_implemented(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, message)
    }

    fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl From<OpysError> for ApiError {
    /// Engine errors reaching a handler are internal by construction: bad input
    /// is caught before the actor is asked, and a corpus that vanished mid-request
    /// or never loaded is the state of the node rather than something the caller
    /// did.
    fn from(e: OpysError) -> ApiError {
        ApiError::internal(e.to_string())
    }
}

impl From<JsonRejection> for ApiError {
    /// Every way a JSON body can be rejected is the caller's mistake — missing
    /// field, wrong content type, oversized — so all of them are 400 and not
    /// axum's assorted 415/422/413, which are outside the contract. The message
    /// already says which kind.
    fn from(rejection: JsonRejection) -> ApiError {
        ApiError::bad_request(rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    /// A query string that will not deserialize (a repeated `?type=`, say) is
    /// bad input, and must not escape as axum's `text/plain`.
    fn from(rejection: QueryRejection) -> ApiError {
        ApiError::bad_request(rejection.body_text())
    }
}

impl From<WebSocketUpgradeRejection> for ApiError {
    /// A plain `GET /api/events` — a browser address bar, a health probe —
    /// deserves the same JSON as everything else.
    fn from(rejection: WebSocketUpgradeRejection) -> ApiError {
        ApiError::bad_request(rejection.body_text())
    }
}

/// Nothing is routed here.
async fn no_route(uri: Uri) -> ApiError {
    ApiError::not_found(format!("no such route: {}", uri.path()))
}

/// The path exists, the method does not.
async fn wrong_method(uri: Uri) -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        format!("method not allowed for {}", uri.path()),
    )
}

/// The SPA shell, for `GET /`.
///
/// Served unconditionally and revalidated on every load: its URL never changes,
/// so an upgraded node that let a browser cache it would keep showing the
/// previous UI. The hashed assets it pulls in are cached forever instead — see
/// [`crate::assets::cache_control`].
async fn ui_index() -> Response {
    match crate::assets::index() {
        Some(asset) => serve(asset),
        // Two different failures share this arm, and they get different answers.
        // An empty table is a deliberate `--no-default-features` build and is
        // the node working as configured; a populated table with no shell is a
        // broken build product, which is ours to own.
        None if !crate::assets::embedded() => ApiError::not_implemented(
            "this opys-server was built without the `web-ui` feature, so it serves no web UI — \
             its API is unaffected. Rebuild with default features to get the dashboard.",
        )
        .into_response(),
        None => ApiError::internal(format!(
            "the embedded web UI has no {}",
            crate::assets::INDEX
        ))
        .into_response(),
    }
}

/// One bundled asset, for `GET /ui/…`.
///
/// A miss answers the same JSON 404 as any other unrouted path. That is right
/// even for a browser expecting a script: the only way to ask for an asset that
/// is not in the table is to ask for one this build never emitted, and a JSON
/// body is what the rest of the node says.
async fn ui_asset(Path(path): Path<String>) -> Response {
    match crate::assets::get(&format!("ui/{path}")) {
        Some(asset) => serve(asset),
        None if !crate::assets::embedded() => ApiError::not_implemented(
            "this opys-server was built without the `web-ui` feature, so it serves no web UI",
        )
        .into_response(),
        None => ApiError::not_found(format!("no such asset: /ui/{path}")).into_response(),
    }
}

/// One of the root-level bundle files — the manifest, its icons, the service
/// worker — for `GET /{file}`. Anything not on [`crate::assets::ROOT_FILES`]
/// is the same 404 an unrouted path gets, so this route widens nothing.
async fn ui_root_file(Path(file): Path<String>, uri: Uri) -> Response {
    if !crate::assets::ROOT_FILES.contains(&file.as_str()) {
        return no_route(uri).await.into_response();
    }
    match crate::assets::get(&file) {
        Some(asset) => serve(asset),
        None if !crate::assets::embedded() => ApiError::not_implemented(
            "this opys-server was built without the `web-ui` feature, so it serves no web UI",
        )
        .into_response(),
        None => ApiError::internal(format!("the embedded web UI has no {file}")).into_response(),
    }
}

/// One embedded file as a response. `Bytes::from_static` — the bundle lives in
/// the binary's rodata, so serving it copies nothing.
fn serve(asset: crate::assets::Asset) -> Response {
    (
        [
            (CONTENT_TYPE, asset.content_type),
            (CACHE_CONTROL, asset.cache_control),
        ],
        Bytes::from_static(asset.bytes),
    )
        .into_response()
}

/// Resolve `cid` and run one blocking actor call, both off the reactor.
///
/// The manager lock is taken *inside* the blocking task on purpose: a rescan can
/// hold it across a filesystem walk and an actor join, and a reactor thread
/// parked on that stalls every other request, the accept loop and the WebSocket
/// pumps along with it.
async fn with_corpus<T, F>(state: &AppState, cid: &str, call: F) -> Result<T, ApiError>
where
    F: FnOnce(CorpusClient) -> opys_engine::error::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let manager = Arc::clone(&state.manager);
    let cid = cid.to_string();
    tokio::task::spawn_blocking(move || {
        // An owned client, so the lock is gone before the call blocks.
        let client = lock(&manager).get(&cid).map(CorpusHandle::client);
        let client = client.ok_or_else(|| ApiError::not_found(format!("no such corpus: {cid}")))?;
        call(client).map_err(ApiError::from)
    })
    .await
    // A panic in the actor call would otherwise abort the response task and
    // hang up the connection with no explanation.
    .map_err(|e| ApiError::internal(format!("corpus task failed: {e}")))?
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    version: &'static str,
    started: String,
    /// False only in the window between binding the socket and the first
    /// rescan finishing. An empty `/api/projects` while this is false means the
    /// node has not looked yet.
    scanned: bool,
}

/// Liveness, plus enough to notice a restart.
async fn health(State(state): State<AppState>) -> Json<Health> {
    let scanned = state
        .scanned
        .as_ref()
        .is_none_or(|f| f.load(Ordering::Acquire));
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        started: state.started,
        scanned,
    })
}

/// The allowlist as the setup screen needs it.
#[derive(Serialize)]
struct Setup {
    /// Whether the allowlist file exists at all. False means nobody has ever
    /// been asked — which is what triggers onboarding. An *empty* file is a
    /// decision already made, and does not.
    configured: bool,
    writable: bool,
    read_only_reason: Option<String>,
    mode: registry::ScanMode,
    /// Where suggestion scans start, resolved: the configured root, else
    /// `~/Projects` when it exists, else `$HOME`.
    scan_root: String,
    /// Shown so the user can see the boundary they are working inside.
    home: String,
    /// What is allowlisted now, as written in the file.
    entries: Vec<EntryOut>,
    /// The file itself, so the UI can name it when it says "edit this by hand".
    path: String,
}

#[derive(Serialize)]
struct EntryOut {
    path: String,
    kind: &'static str,
    /// Present when the entry does not resolve — a project that moved away.
    error: Option<String>,
}

/// The root a scan should start from, given the registry.
///
/// `~/Projects` when it exists, because a narrower root is a shorter walk and a
/// shorter list, and most people keep their work in one place. `$HOME`
/// otherwise. An explicit `scan_root` always wins.
fn default_scan_root(reg: &registry::Registry) -> PathBuf {
    if let Some(root) = &reg.scan_root {
        return root.clone();
    }
    let home = registry::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let projects = home.join("Projects");
    if projects.is_dir() {
        projects
    } else {
        home
    }
}

fn read_registry(state: &AppState) -> Result<(PathBuf, registry::Registry), ApiError> {
    let path = {
        let manager = lock(&state.manager);
        manager.registry_path().to_path_buf()
    };
    let reg = registry::Registry::load_from(&path)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((path, reg))
}

async fn setup(State(state): State<AppState>) -> Result<Json<Setup>, ApiError> {
    let (path, reg) = read_registry(&state)?;
    let home = registry::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let read_only_reason = reg.read_only_reason();
    Ok(Json(Setup {
        configured: path.exists(),
        writable: read_only_reason.is_none(),
        read_only_reason,
        mode: reg.mode,
        scan_root: default_scan_root(&reg).display().to_string(),
        home: home.display().to_string(),
        entries: reg
            .entries
            .iter()
            .map(|e| EntryOut {
                path: e.raw_path.clone(),
                kind: e.kind.key(),
                error: e.error.clone(),
            })
            .collect(),
        path: path.display().to_string(),
    }))
}

/// What onboarding submits.
#[derive(Deserialize)]
struct SetupIn {
    mode: String,
    /// Optional: omitted leaves the resolved default in place rather than
    /// pinning it, so a home directory that later grows a `Projects` is picked
    /// up.
    #[serde(default)]
    scan_root: Option<String>,
}

async fn save_setup(
    State(state): State<AppState>,
    body: Result<Json<SetupIn>, JsonRejection>,
) -> Result<Json<Setup>, ApiError> {
    let Json(input) = body?;
    let mode = match input.mode.as_str() {
        "off" => registry::ScanMode::Off,
        "suggest" => registry::ScanMode::Suggest,
        other => {
            return Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown mode {other:?} (expected `off` or `suggest`)"),
            ))
        }
    };
    // The scan root is a path from the browser like any other, so it is vetted
    // the same way: a setup screen is not a way around the rules.
    let root = match input.scan_root.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(raw) => Some(
            registry::vet_ui_path(raw)
                .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?,
        ),
        None => None,
    };

    let (path, current) = read_registry(&state)?;
    if let Some(reason) = current.read_only_reason() {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, reason));
    }
    let _lock = registry::lock(&path)
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    // Re-read under the lock: another writer may have moved between the check
    // and the edit.
    let mut reg = registry::Registry::load_from(&path)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    reg.set_mode(mode);
    reg.set_scan_root(root.as_deref());
    reg.save()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    drop(_lock);
    setup(State(state)).await
}

#[derive(Serialize)]
struct SuggestionOut {
    path: String,
    name: String,
    already_allowlisted: bool,
}

/// Projects the scan found that are not allowlisted yet.
///
/// **Paths only.** No document count, no verify dot, no title: rendering any of
/// those means opening the project, and opening it reads whatever its
/// `opys.toml` points `base` at. Keeping a person between "found" and "opened"
/// is the entire reason there is no auto-add mode.
async fn suggestions(State(state): State<AppState>) -> Result<Json<Vec<SuggestionOut>>, ApiError> {
    let (_, reg) = read_registry(&state)?;
    if reg.mode == registry::ScanMode::Off {
        return Ok(Json(Vec::new()));
    }
    let root = default_scan_root(&reg);
    // The walk blocks, so it never runs on the reactor.
    let found = tokio::task::spawn_blocking(move || {
        crate::discover::suggest(&root, registry::DEFAULT_DEPTH, &reg)
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        found
            .into_iter()
            .filter(|s| !s.already_allowlisted)
            .map(|s| SuggestionOut {
                path: s.path.display().to_string(),
                name: s.name,
                already_allowlisted: s.already_allowlisted,
            })
            .collect(),
    ))
}

/// Add or remove one allowlist entry.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum AllowlistIn {
    Add {
        path: String,
        /// `project` (default) or `prefix`.
        #[serde(default)]
        kind: Option<String>,
    },
    Remove {
        path: String,
    },
}

async fn allowlist(
    State(state): State<AppState>,
    body: Result<Json<AllowlistIn>, JsonRejection>,
) -> Result<Json<Setup>, ApiError> {
    let Json(input) = body?;
    let unprocessable =
        |e: OpysError| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, e.to_string());

    let (path, current) = read_registry(&state)?;
    if let Some(reason) = current.read_only_reason() {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, reason));
    }
    let _lock = registry::lock(&path)
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    let mut reg = registry::Registry::load_from(&path)
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match input {
        AllowlistIn::Add { path: raw, kind } => {
            // Vetted before anything else touches it.
            let dir = registry::vet_ui_path(&raw).map_err(unprocessable)?;
            let kind = match kind.as_deref() {
                None | Some("project") => registry::EntryKind::Project,
                Some("prefix") => registry::EntryKind::Prefix,
                Some(other) => {
                    return Err(ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        format!("unknown kind {other:?} (expected `project` or `prefix`)"),
                    ))
                }
            };
            reg.add(&dir, kind).map_err(unprocessable)?;
        }
        AllowlistIn::Remove { path: raw } => {
            // Removal takes the path as written, not as vetted: an entry that no
            // longer resolves, or one added by hand from outside `$HOME`, must
            // still be removable from the UI that is showing it.
            let target = registry::expand_tilde(raw.trim());
            if !reg.remove(&target).map_err(unprocessable)? {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    format!("{raw:?} is not in the allowlist"),
                ));
            }
        }
    }
    reg.save()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    drop(_lock);
    // Browser additions/removals must update the roster immediately rather
    // than waiting for the filesystem watcher's next maintenance tick.
    let manager = Arc::clone(&state.manager);
    tokio::task::spawn_blocking(move || lock(&manager).rescan())
        .await
        .map_err(|e| {
            ApiError::internal(format!(
                "allowlist saved, but refreshing projects failed: {e}"
            ))
        })?
        .map_err(|e| {
            ApiError::internal(format!(
                "allowlist saved, but refreshing projects failed: {e}"
            ))
        })?;
    setup(State(state)).await
}

/// Replace a corpus's discovery-time branch with the one its group's ref reader
/// last resolved, when the reader has an answer for it.
///
/// Discovery reads the branch once, when it expands the allowlist, and the
/// manager rebuilds a group only on a rescan — so between rescans a checkout
/// that moved keeps the name it was found on, in the sidebar and in the union's
/// column headings. The ref reader re-resolves every checkout on every pass
/// (and broadcasts when the answer moves), so its answer is the fresh one.
///
/// A corpus the reader has no entry for is left alone: no repository, or a pass
/// that could not open one, is not evidence that the branch changed.
fn freshen_branch(corpus: &mut Corpus, branches: &Branches) {
    if let Some(branch) = branches.branch_of.get(&corpus.root) {
        corpus.branch = branch.clone();
    }
}

/// One project (a repository and its worktrees) as the UI presents it.
#[derive(Serialize)]
struct ProjectOut {
    key: String,
    name: String,
    corpora: Vec<CorpusOut>,
    /// How many branch columns the union view would add — refs with no checkout,
    /// as the group's ref reader last counted them (ADR-0053).
    ///
    /// Here so a client can decide whether a comparison exists to offer. A
    /// repository with one worktree and three branches has plenty to compare and
    /// would otherwise look, from this payload alone, like a lone directory.
    branches: usize,
}

/// One inventory, with the cached numbers for it.
///
/// A bespoke struct rather than [`crate::discover::Corpus`]'s own `Serialize`:
/// that one carries an internal `group` key, and it would serialize `root`/`base`
/// as paths — a non-UTF-8 path then fails mid-response and degrades to a 500 with
/// a plain-text body, breaking the error contract in the one case it matters.
#[derive(Serialize)]
struct CorpusOut {
    cid: String,
    root: String,
    base: String,
    branch: Option<String>,
    is_primary: bool,
    /// `None` until a load has succeeded — a corpus whose config will not parse
    /// reports its `error` and no counts, rather than a misleading zero.
    doc_count: Option<usize>,
    verify_problems: Option<usize>,
    loaded_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// The allowlisted projects, each with its corpora and their cached counts.
async fn projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectOut>>, ApiError> {
    let manager = Arc::clone(&state.manager);
    // One lock, taken off the reactor, held for three cheap clones: the group
    // list, a branch count per group, and a client per corpus. The count is a
    // read through the `Arc` the ref reader published — never a copy of the
    // columns, which carry main's whole inventory apiece.
    let (groups, branches, clients) = tokio::task::spawn_blocking(move || {
        let manager = lock(&manager);
        let mut groups = manager.groups().to_vec();
        let branches: BTreeMap<String, usize> = groups
            .iter_mut()
            .map(|g| {
                let read = manager.branches_for(&g.key);
                // The branch each corpus is *now* on, not the one discovery
                // recorded when it first walked the allowlist; see
                // `Branches::branch_of`.
                for corpus in &mut g.corpora {
                    freshen_branch(corpus, &read);
                }
                (g.key.clone(), read.columns.len())
            })
            .collect();
        let clients: Vec<CorpusClient> = manager
            .cids()
            .iter()
            .filter_map(|cid| manager.get(cid).map(CorpusHandle::client))
            .collect();
        (groups, branches, clients)
    })
    .await
    .map_err(|e| ApiError::internal(format!("listing projects failed: {e}")))?;

    let stats = gather_stats(clients).await;
    let out = groups
        .into_iter()
        .map(|group| ProjectOut {
            branches: branches.get(&group.key).copied().unwrap_or_default(),
            key: group.key,
            name: group.name,
            corpora: group
                .corpora
                .into_iter()
                .map(|corpus| {
                    let numbers = stats.get(&corpus.cid);
                    let ok = numbers.and_then(|r| r.as_ref().ok());
                    // Counts only mean something once a load has succeeded.
                    let loaded = ok.filter(|s| s.loaded_at.is_some());
                    let unreachable = match numbers {
                        Some(Err(e)) => Some(e.clone()),
                        // A rescan stopped this corpus between the two steps
                        // above. Rare, and better said than silently blank.
                        None => Some("corpus is not being served".to_string()),
                        Some(Ok(_)) => None,
                    };
                    CorpusOut {
                        cid: corpus.cid,
                        root: corpus.root.display().to_string(),
                        base: corpus.base.display().to_string(),
                        branch: corpus.branch,
                        is_primary: corpus.is_primary,
                        doc_count: loaded.map(|s| s.doc_count),
                        verify_problems: loaded.map(|s| s.verify_problems),
                        loaded_at: ok.and_then(|s| s.loaded_at.clone()),
                        // Most specific reason first: discovery could not read
                        // the config, the last load failed, the actor is gone.
                        error: corpus
                            .error
                            .or_else(|| ok.and_then(|s| s.load_error.clone()))
                            .or(unreachable),
                    }
                })
                .collect(),
        })
        .collect();
    Ok(Json(out))
}

/// Every corpus's cached numbers, gathered in parallel under one deadline.
///
/// One task per corpus rather than a loop inside a single task: these are cache
/// reads, but a corpus that is mid-reload answers only when the load finishes,
/// and in a loop each such corpus would add its whole wait to the response.
async fn gather_stats(clients: Vec<CorpusClient>) -> BTreeMap<String, Result<CorpusStats, String>> {
    let deadline = tokio::time::Instant::now() + STATS_TIMEOUT;
    let tasks: Vec<(String, _)> = clients
        .into_iter()
        .map(|client| {
            (
                client.cid.clone(),
                tokio::task::spawn_blocking(move || client.stats()),
            )
        })
        .collect();
    let mut out = BTreeMap::new();
    for (cid, task) in tasks {
        // All the tasks are already running, so one shared deadline bounds the
        // whole gather rather than each wait in turn.
        let stats = match tokio::time::timeout_at(deadline, task).await {
            Ok(Ok(Ok(stats))) => Ok(stats),
            Ok(Ok(Err(e))) => Err(e.to_string()),
            Ok(Err(e)) => Err(format!("corpus task failed: {e}")),
            Err(_) => Err("corpus is busy".to_string()),
        };
        out.insert(cid, stats);
    }
    out
}

/// The `?type=&status=&tag=` filters, each optional and AND-combined.
#[derive(Debug, Deserialize)]
struct DocQuery {
    #[serde(rename = "type")]
    type_name: Option<String>,
    status: Option<String>,
    tag: Option<String>,
}

impl DocQuery {
    fn into_filter(self) -> DocFilter {
        // `?status=` with no value is what a UI sends when its dropdown is
        // cleared, and it means "no filter" — not "match the empty status",
        // which would return nothing.
        fn set(value: Option<String>) -> Option<String> {
            value.filter(|s| !s.is_empty())
        }
        DocFilter {
            type_name: set(self.type_name),
            status: set(self.status),
            tag: set(self.tag),
        }
    }
}

/// Document summaries from a corpus's warm cache.
async fn docs(
    State(state): State<AppState>,
    Path(cid): Path<String>,
    filters: Result<Query<DocQuery>, QueryRejection>,
) -> Result<Json<Vec<DocSummary>>, ApiError> {
    let Query(filters) = filters?;
    let filter = filters.into_filter();
    Ok(Json(
        with_corpus(&state, &cid, move |client| client.docs(filter)).await?,
    ))
}

/// The merged, labeled view across one project group's corpora.
///
/// A group is a repository's worktrees, so this is the one endpoint that answers
/// "what does each branch say about this document" — presentation only. The node
/// never merges or resolves anything; git remains the only merger (ADR-0027).
///
/// The filters apply **per corpus, before merging**, which is what makes
/// `?status=` mean "where does this document match" rather than "where does it
/// exist": a task that is `doing` on main and `done` on a branch is dropped from
/// the branch's column and reported as `only_in` main. [`crate::union::union`]
/// says the same thing at more length; a client offering the filter should say
/// it to the user.
///
/// The one thing a filter does *not* reshape is the collision warning. Each
/// corpus is asked for its whole summary set, the contested id numbers are
/// derived from that, and the filter is applied here — so a `?type=` that hides
/// one half of a contested number cannot retract the warning on the other half.
/// An impending id collision is a fact about the corpora, not about the current
/// view. Filtering in this handler rather than in the actor costs one pass over
/// summaries that are cloned out of the warm cache anyway.
///
/// 404 only for a group nobody is serving. A group of one corpus is a valid,
/// trivial, one-column view — a project with no worktrees is not an error, and a
/// UI that can render the union should not need a second code path for it.
///
/// A member that cannot answer — never loaded, stopped by a rescan mid-request,
/// still holding the inventory lock past [`UNION_TIMEOUT`] — does not fail the
/// request. One broken branch is precisely the divergence this view exists to
/// show, and refusing the whole group because of it would hide the other
/// columns. The failure is handed to the merge as such, so the column carries
/// the reason and no row claims the branch deleted anything.
///
/// The columns for refs **nobody has checked out** come from the group's ref
/// reader ([`crate::refreader`]), which read them in the background off a git
/// watcher. Taking them from its published snapshot rather than reading git here
/// is what keeps this handler free of a tree walk: the answer depends on the
/// refs, not on the request, so it is computed once per ref change and cloned
/// per request. The request's filter is applied to those columns exactly as it
/// is to a worktree's.
async fn group_union(
    State(state): State<AppState>,
    Path(key): Path<String>,
    filters: Result<Query<DocQuery>, QueryRejection>,
) -> Result<Json<UnionView>, ApiError> {
    let Query(filters) = filters?;
    let filter = filters.into_filter();
    let manager = Arc::clone(&state.manager);
    let wanted = key.clone();
    // One lock, taken off the reactor, held for the group's clone and a client
    // per member. `None` means no such group; an inner `None` means that member
    // stopped between the group being listed and its client being taken.
    let (members, branches) = tokio::task::spawn_blocking(move || {
        let manager = lock(&manager);
        let group = manager.groups().iter().find(|g| g.key == wanted)?;
        // The reader's last snapshot, shared rather than copied, taken under the
        // same lock. It is never computed here: the reader owns the git walk.
        let branches = manager.branches_for(&wanted);
        let members: Vec<(Corpus, Option<CorpusClient>)> = group
            .corpora
            .iter()
            .map(|c| {
                let client = manager.get(&c.cid).map(CorpusHandle::client);
                let mut c = c.clone();
                freshen_branch(&mut c, &branches);
                (c, client)
            })
            .collect();
        Some((members, branches))
    })
    .await
    .map_err(|e| ApiError::internal(format!("building the union view failed: {e}")))?
    .ok_or_else(|| ApiError::not_found(format!("no such project group: {key}")))?;

    // Unfiltered, because the collision warning is derived from every id the
    // corpora and the branches hold; the request's filter is applied below.
    let gathered = gather_docs(members, DocFilter::default()).await;
    let contested = contested_numbers(&gathered, &branches);
    let rows: Vec<CorpusDocs> = gathered
        .into_iter()
        .map(|(corpus, docs)| {
            // A corpus that failed stays a failure: an empty column would say
            // "this branch has nothing", which is the one thing we do not know.
            let docs = docs.map(|docs| docs.into_iter().filter(|d| filter.matches(d)).collect());
            (corpus, docs)
        })
        .collect();
    // The one place the snapshot is copied, and only the parts the filter
    // reshapes: the reader's `Arc` is shared with every other reader of it.
    let branches = Branches {
        columns: branches
            .columns
            .iter()
            // Same rule as a corpus column, for the same reason: a branch that
            // could not be read keeps its error rather than becoming empty.
            .map(|(column, docs)| {
                let docs = match docs {
                    Ok(docs) => Ok(docs.iter().filter(|d| filter.matches(d)).cloned().collect()),
                    Err(e) => Err(e.clone()),
                };
                (column.clone(), docs)
            })
            .collect(),
        occupancy: branches.occupancy.clone(),
        notices: branches.notices.clone(),
        read_at: branches.read_at.clone(),
        // Already applied to the columns above, by `freshen_branch`; carried so
        // the reshaped value still says what the reader read.
        branch_of: branches.branch_of.clone(),
    };
    Ok(Json(union(&rows, &branches, &contested)))
}

/// Every member's summaries — those matching `filter` — gathered in parallel
/// under one deadline. A member that could not answer comes back as the `Err`
/// side, never as an empty list; see [`crate::union::Column::error`].
///
/// Order is preserved: it becomes the column order, and discovery puts the main
/// worktree first. One task per corpus for the same reason as [`gather_stats`] —
/// a member waiting out a reload would otherwise add its whole wait to the
/// response, once per member.
async fn gather_docs(
    members: Vec<(Corpus, Option<CorpusClient>)>,
    filter: DocFilter,
) -> Vec<CorpusDocs> {
    let deadline = tokio::time::Instant::now() + UNION_TIMEOUT;
    let tasks: Vec<(Corpus, Option<_>)> = members
        .into_iter()
        .map(|(corpus, client)| {
            let started = client.map(|client| {
                let filter = filter.clone();
                tokio::task::spawn_blocking(move || client.docs(filter))
            });
            (corpus, started)
        })
        .collect();
    let mut out = Vec::with_capacity(tasks.len());
    for (corpus, task) in tasks {
        let docs = match task {
            // A rescan stopped this corpus between the two steps above. Rare,
            // and better said than silently blank.
            None => Err("corpus is not being served".to_string()),
            // All the tasks are already running, so one shared deadline bounds
            // the whole gather rather than each wait in turn.
            Some(task) => match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok(Ok(docs))) => Ok(docs),
                Ok(Ok(Err(e))) => Err(e.to_string()),
                Ok(Err(e)) => Err(format!("corpus task failed: {e}")),
                Err(_) => Err("corpus is busy".to_string()),
            },
        };
        out.push((corpus, docs));
    }
    out
}

/// One document, with its frontmatter, relations and rendered body.
async fn doc(
    State(state): State<AppState>,
    Path((cid, docid)): Path<(String, String)>,
) -> Result<Json<DocView>, ApiError> {
    let wanted = docid.clone();
    with_corpus(&state, &cid, move |client| client.doc(&wanted))
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no such document: {docid}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftBody {
    body: String,
}

/// The same semantic verifier as the save gate, over a disposable body. This
/// read does not take the inventory write lock, flush, sync, or publish events.
async fn verify_body(
    State(state): State<AppState>,
    Path((cid, docid)): Path<(String, String)>,
    body: Result<Json<DraftBody>, JsonRejection>,
) -> Result<Json<DraftVerification>, ApiError> {
    let Json(body) = body?;
    let wanted = docid.clone();
    with_corpus(&state, &cid, move |client| {
        client.verify_body(&wanted, &body.body)
    })
    .await?
    .map(Json)
    .ok_or_else(|| ApiError::not_found(format!("no such document: {docid}")))
}

/// Markdown blocks to render, in order.
#[derive(Debug, Deserialize)]
struct RenderBody {
    markdown: Vec<String>,
}

#[derive(Serialize)]
struct Rendered {
    html: Vec<String>,
}

/// The most the editor ever sends at once: every block of a large document.
/// Bounds are the cost of a pure endpoint, which anything on the allowed
/// origin can call: no unbounded render on the reactor.
const RENDER_MAX_BLOCKS: usize = 5_000;
const RENDER_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Render markdown blocks with the node's own options (`actor::render_markdown`),
/// so the editor's preview of a block is the document view's rendering of it.
async fn render(body: Result<Json<RenderBody>, JsonRejection>) -> Result<Json<Rendered>, ApiError> {
    let Json(body) = body?;
    let bytes: usize = body.markdown.iter().map(String::len).sum();
    if body.markdown.len() > RENDER_MAX_BLOCKS || bytes > RENDER_MAX_BYTES {
        return Err(ApiError::bad_request(format!(
            "too much to render: {} blocks, {bytes} bytes (at most {RENDER_MAX_BLOCKS} and {RENDER_MAX_BYTES})",
            body.markdown.len()
        )));
    }
    let html = tokio::task::spawn_blocking(move || {
        body.markdown
            .iter()
            .map(|md| crate::actor::render_markdown(md))
            .collect()
    })
    .await
    .map_err(|e| ApiError::internal(format!("render task failed: {e}")))?;
    Ok(Json(Rendered { html }))
}

/// A user SQL query. `params` fills the statement's `$n` placeholders.
#[derive(Debug, Deserialize)]
struct QueryBody {
    sql: String,
    #[serde(default)]
    params: Vec<String>,
}

/// Run read-only SQL over a corpus's store.
async fn query(
    State(state): State<AppState>,
    Path(cid): Path<String>,
    body: Result<Json<QueryBody>, JsonRejection>,
) -> Result<Json<QueryResult>, ApiError> {
    let Json(body) = body?;
    // The outer error is the node's (no warm store, projections would not
    // rebuild) and is a 500; only the inner one is about the statement.
    let result = with_corpus(&state, &cid, move |client| {
        client.query(&body.sql, &body.params)
    })
    .await?;
    // SELECT-only is enforced by the engine's plan guard, which is the single
    // place that decision lives. Whatever it says goes back verbatim: it names
    // the statement kind it refused, which is more useful than a rewrite.
    result.map(Json).map_err(ApiError::bad_request)
}

/// The `?status=&type=&n=` query of the pick endpoint.
///
/// `status` is required — unlike [`DocQuery`]'s optional filters, it is the
/// question itself, and there is no sensible default for it. A missing one is a
/// [`QueryRejection`] (400) before this is ever built.
#[derive(Debug, Deserialize)]
struct NextQuery {
    status: String,
    #[serde(rename = "type")]
    type_name: Option<String>,
    role: Option<String>,
    n: Option<usize>,
}

/// The pick oracle for one project, from its **primary** corpus.
///
/// `{key}` is a project-group key, the same string `/api/projects` emits and
/// `/api/group/{key}/union` accepts. A group is a repository and its worktrees;
/// `next` must answer from exactly one of them, and ADR-0053 says which — main
/// is the truth for pickability, so this is the main worktree (falling back to
/// the group's first corpus when discovery marked none, which happens only
/// outside git).
///
/// The body is [`PickResult`], serialized from the very same engine type the
/// CLI's `--json` prints; `opys-server/tests/api.rs` pins that they agree.
///
/// Named `pick` rather than `next` because [`axum::middleware::Next`] is in
/// scope here and a handler of that name beside it would read as the same
/// thing.
async fn pick(
    State(state): State<AppState>,
    Path(key): Path<String>,
    query: Result<Query<NextQuery>, QueryRejection>,
) -> Result<Json<PickResult>, ApiError> {
    let Query(query) = query?;
    if query.status.is_empty() {
        return Err(ApiError::bad_request("next: ?status= is required"));
    }
    let cid = primary_cid(&state, &key).await?;

    let q = PickQuery {
        status: query.status,
        // `?type=` with no value is a cleared dropdown, not the empty type.
        type_filter: query.type_name.filter(|s| !s.is_empty()),
        role: query.role.filter(|s| !s.is_empty()),
        limit: query.n.unwrap_or(DEFAULT_LIMIT),
    };
    let result = with_corpus(&state, &cid, move |client| client.next(q)).await?;
    result.map(Json).map_err(ApiError::bad_request)
}

/// The corpus a project-keyed route answers from: the group's **primary**
/// worktree, falling back to its first corpus when discovery marked none (which
/// happens only outside git).
///
/// Shared by the routes under `/api/projects/{key}/…` so they cannot come
/// to disagree about which worktree is the truth — or about the wording of the
/// 404, which `opys-server/tests/api.rs` asserts on.
async fn primary_cid(state: &AppState, key: &str) -> Result<String, ApiError> {
    let manager = Arc::clone(&state.manager);
    let wanted = key.to_string();
    // The manager lock, taken off the reactor, held only for the cid lookup.
    tokio::task::spawn_blocking(move || {
        let manager = lock(&manager);
        let group = manager.groups().iter().find(|g| g.key == wanted)?;
        let corpus = group
            .corpora
            .iter()
            .find(|c| c.is_primary)
            .or_else(|| group.corpora.first())?;
        Some(corpus.cid.clone())
    })
    .await
    .map_err(|e| ApiError::internal(format!("resolving the project failed: {e}")))?
    .ok_or_else(|| ApiError::not_found(format!("no such project: {key}")))
}

/// The `?n=` query of the flow endpoint: how many items each node lists.
#[derive(Debug, Deserialize)]
struct FlowQuery {
    n: Option<usize>,
}

/// The lifecycle graph of one project, from its **primary** corpus.
///
/// `{key}` is a project-group key, as for [`pick`], and for the same reason: the
/// nodes carry `next::pick`'s answer per status, and pickability is evaluated on
/// main (ADR-0053).
///
/// One call answers the whole view — the statuses, the edges the `transitions`
/// maps declare, and the documents on each node — because the alternative is a
/// request per status over a corpus the node would reconstruct each time, and
/// because the blocked count a node greys out is not part of a `PickResult` at
/// all.
///
/// Nothing here can be bad input but a malformed `?n=`, which the
/// [`QueryRejection`] turns into a 400: every status in the answer comes from
/// the project's own config.
async fn flow(
    State(state): State<AppState>,
    Path(key): Path<String>,
    query: Result<Query<FlowQuery>, QueryRejection>,
) -> Result<Json<FlowView>, ApiError> {
    let Query(query) = query?;
    let cid = primary_cid(&state, &key).await?;
    let limit = query.n.unwrap_or(DEFAULT_NODE_LIMIT);
    Ok(Json(
        with_corpus(&state, &cid, move |client| client.flow(limit)).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct QueueOptions {
    #[serde(rename = "type")]
    type_name: Option<String>,
    n: Option<usize>,
}

/// The CLI's exact pretty-printed queue, including its final newline.
async fn queue(
    State(state): State<AppState>,
    Path((key, role)): Path<(String, String)>,
    query: Result<Query<QueueOptions>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let cid = primary_cid(&state, &key).await?;
    let wanted = role.clone();
    let result = with_corpus(&state, &cid, move |client| {
        client.queue(QueueQuery {
            role: wanted,
            type_filter: query.type_name.filter(|s| !s.is_empty()),
            limit: query.n.unwrap_or(DEFAULT_LIMIT),
        })
    })
    .await?;
    let payload = result.map_err(|error| match error {
        QueueQueryError::UnknownRole => {
            ApiError::not_found(format!("project {key} does not declare role {role}"))
        }
        QueueQueryError::Invalid(error) => ApiError::bad_request(error),
    })?;
    queue_response(&payload)
}

fn queue_response(payload: &QueueResult) -> Result<Response, ApiError> {
    let mut body = serde_json::to_string_pretty(payload)
        .map_err(|error| ApiError::internal(format!("serializing queue failed: {error}")))?;
    body.push('\n');
    Ok(([(CONTENT_TYPE, "application/json")], body).into_response())
}

async fn roles(State(state): State<AppState>) -> Result<Json<RolesView>, ApiError> {
    Ok(Json(role_queues::roles(gather_role_queues(&state).await?)))
}

async fn role(
    State(state): State<AppState>,
    Path(role): Path<String>,
) -> Result<Json<RoleView>, ApiError> {
    Ok(Json(role_queues::role(
        gather_role_queues(&state).await?,
        role,
    )))
}

/// Clone primary clients under one manager lock, then ask every project under
/// one deadline. A slow reload cannot delay all other project queues in turn.
async fn gather_role_queues(state: &AppState) -> Result<Vec<ProjectQueues>, ApiError> {
    let manager = Arc::clone(&state.manager);
    let projects = tokio::task::spawn_blocking(move || {
        let manager = lock(&manager);
        manager
            .groups()
            .iter()
            .map(|group| {
                let client = group
                    .corpora
                    .iter()
                    .find(|corpus| corpus.is_primary)
                    .or_else(|| group.corpora.first())
                    .and_then(|corpus| manager.get(&corpus.cid))
                    .map(CorpusHandle::client);
                (group.key.clone(), group.name.clone(), client)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| ApiError::internal(format!("listing role queues failed: {error}")))?;
    let deadline = tokio::time::Instant::now() + UNION_TIMEOUT;
    let tasks: Vec<_> = projects
        .into_iter()
        .map(|(key, label, client)| {
            let task = client.map(|client| tokio::task::spawn_blocking(move || client.roles()));
            (key, label, task)
        })
        .collect();
    let mut projects = Vec::with_capacity(tasks.len());
    for (key, label, task) in tasks {
        let queues = match task {
            None => Err("corpus is not being served".to_string()),
            Some(task) => match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok(Ok(queues))) => Ok(queues),
                Ok(Ok(Err(error))) => Err(error.to_string()),
                Ok(Err(error)) => Err(format!("corpus task failed: {error}")),
                Err(_) => Err("corpus is busy".to_string()),
            },
        };
        projects.push(ProjectQueues { key, label, queues });
    }
    Ok(projects)
}

/// The corpus's cached verify problems.
///
/// A superset of the documented `{problems, loaded_at}`: `ok` and `load_error`
/// come along because "verify found nothing" and "the corpus would not load at
/// all" are different states, and a client showing a green tick needs to tell
/// them apart.
async fn verify(
    State(state): State<AppState>,
    Path(cid): Path<String>,
) -> Result<Json<VerifyStatus>, ApiError> {
    Ok(Json(
        with_corpus(&state, &cid, |client| client.verify()).await?,
    ))
}

/// A completed write.
#[derive(Serialize)]
struct ActionOk {
    ok: bool,
    /// The document the action touched: allocated by `new`, echoed by the rest.
    id: String,
    /// The line the CLI prints for the same command. For display, not parsing.
    message: String,
    /// Present only when the write landed but the auto-sync pass did not run —
    /// the CLI's `note: skipped sync` on stderr, which a headless node has
    /// nowhere else to put. The write is still authoritative; relation maps and
    /// prose links are not being maintained until the reason is fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_skipped: Option<String>,
}

/// How a refused write is answered, per [`ActionError`].
///
/// Three statuses rather than one, because a client has to act on them
/// differently: 404 stop, 503 retry, 422 the write is invalid — show the user.
/// Only the last carries the engine's own text.
fn refusal(cid: &str, e: ActionError) -> ApiError {
    match e {
        ActionError::Gone => ApiError::not_found(format!("corpus is no longer available: {cid}")),
        busy @ ActionError::Busy => ApiError::unavailable(busy.to_string()),
        ActionError::Refused(e) => ApiError::unprocessable(e.to_string()),
    }
}

/// Perform one write against a corpus.
///
/// The body is a closed enum of the mutating commands
/// ([`crate::action::Action`]); an unknown action or an undeclared field fails
/// deserialization and comes back as a 400, which is what makes this endpoint
/// incapable of executing anything the node did not implement.
///
/// The write does **not** go through the corpus actor, and that is the point.
/// The actor owns a warm store loaded in the past without the inventory lock;
/// flushing it would clobber every CLI write made since. So the blocking task
/// below borrows nothing from the actor's state, runs a fresh CLI-identical
/// cycle against the files, and only *then* asks the actor to reload.
///
/// That last step is not the watcher's job to do alone. Nothing else in the node
/// ever reloads an actor — `refresh` only drops corpora that vanished, `rescan`
/// skips cids it already serves — so a corpus whose watcher never started (an
/// inventory directory created after the corpus was allowlisted) or died (the
/// directory replaced wholesale by a branch switch; inotify on a network mount
/// or WSL `/mnt`) would answer reads from a cache frozen at the last watched
/// change, *permanently*, while happily reporting each write as a success.
/// Asking explicitly costs one load and closes read-your-own-write as well: the
/// 200 goes out after the cache reflects the write. The cycle's flock is
/// released by then, so the reload cannot deadlock against it.
///
/// Worst case that parks a blocking-pool thread for `OPYS_LOCK_TIMEOUT_MS`
/// (10 s by default) waiting on the inventory lock, then answers 503. There is
/// no deadline on top of it deliberately: a timeout that abandoned a half-written
/// cycle would be worse than a slow answer, and a client should expect the wait.
async fn action(
    State(state): State<AppState>,
    Path(cid): Path<String>,
    body: Result<Json<Action>, JsonRejection>,
) -> Result<Json<ActionOk>, ApiError> {
    let Json(request) = body?;
    let name = request.name().to_string();
    let manager = Arc::clone(&state.manager);
    let events = state.events.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        // Owned copies, so the manager lock is gone well before the cycle
        // blocks: a rescan can hold it for seconds, and the cycle itself can
        // wait out the inventory lock for ten.
        let target = {
            let manager = lock(&manager);
            manager.get(&cid).map(|handle| {
                (
                    handle.corpus.root.clone(),
                    handle.client(),
                    manager.backend(),
                )
            })
        };
        let (root, client, make_backend) =
            target.ok_or_else(|| ApiError::not_found(format!("no such corpus: {cid}")))?;
        let backend = make_backend();
        let done = crate::action::perform(&root, backend.as_ref(), &request);
        // Gone and Busy are the two outcomes that provably touched nothing:
        // neither ever reached the flush. Everything else reloads, including
        // `Refused` — a refused *core* now leaves the bytes as they were, but
        // the same variant also carries a failed flush and a failed load, and a
        // flush that fails partway leaves files the cache has not seen. The
        // cheap wrong answer here is one spare reload; the expensive one is a
        // dashboard serving documents that moved.
        if !matches!(done, Err(ActionError::Gone | ActionError::Busy)) {
            if let Ok(outcome) = &done {
                // Broadcast here rather than after the `.await` below: a client
                // that hangs up mid-cycle cannot cancel `spawn_blocking`, so the
                // write still lands, and an acknowledgement nobody sends is a
                // write nobody hears about. Err only means nobody is subscribed.
                let _ = events.send(Event::ActionCompleted {
                    cid: cid.clone(),
                    action: name,
                    id: outcome.id.clone(),
                });
            }
            let _ = client.reload();
        }
        // Not `?` on the engine error: the blanket `From<OpysError>` is a 500,
        // and here every refusal is the caller's answer rather than the node's.
        done.map_err(|e| refusal(&cid, e))
    })
    .await
    // A panic mid-cycle would otherwise abort the response task and hang up the
    // connection with no explanation.
    .map_err(|e| ApiError::internal(format!("action task failed: {e}")))??;

    Ok(Json(ActionOk {
        ok: true,
        id: outcome.id,
        message: outcome.message,
        sync_skipped: outcome.sync_skipped,
    }))
}

/// The greeting frame. Keyed on `type` rather than the `event` tag the broadcast
/// payloads use: it is connection metadata, not something that happened.
#[derive(Serialize)]
struct Hello {
    #[serde(rename = "type")]
    kind: &'static str,
    version: &'static str,
}

/// The event stream: one `hello`, then every broadcast event as JSON.
async fn events(
    State(state): State<AppState>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return ApiError::from(rejection).into_response(),
    };
    // Subscribe before the handshake completes, not inside the callback: events
    // published in that window would otherwise reach nobody, and a client that
    // connects in response to an action would miss its own result.
    let receiver = state.events.subscribe();
    let ping = state.ping;
    upgrade.on_upgrade(move |socket| pump(socket, receiver, ping))
}

/// One frame, with a deadline.
///
/// `send` parks indefinitely on a peer that has stopped draining its socket, and
/// a parked send means nothing else in the loop below runs — including the ping
/// timer, which exists for exactly that peer. A write that misses the deadline
/// is treated as a dead client.
async fn send(socket: &mut WebSocket, message: Message, within: Duration) -> bool {
    matches!(
        tokio::time::timeout(within, socket.send(message)).await,
        Ok(Ok(()))
    )
}

/// Forward broadcast events to one client until it stops keeping up.
///
/// A single task drives both directions. Each `select!` branch awaits a
/// cancel-safe future — a broadcast receive, an interval tick, a socket read
/// that buffers inside the codec — while the sends happen in the branch bodies,
/// where nothing can cancel them mid-frame.
async fn pump(mut socket: WebSocket, mut events: broadcast::Receiver<Event>, config: PingConfig) {
    let hello = Hello {
        kind: "hello",
        version: env!("CARGO_PKG_VERSION"),
    };
    let Ok(hello) = serde_json::to_string(&hello) else {
        return;
    };
    if !send(&mut socket, Message::text(hello), config.every).await {
        return;
    }

    let mut ping = tokio::time::interval(config.every);
    // Not the default `Burst`: after any stall longer than two periods — a
    // suspended laptop, a starved runtime — the missed ticks would fire
    // back-to-back and drop a perfectly live client before it could answer even
    // one of them. `Delay` gives a resumed pump a full period again.
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick of an interval fires immediately; a ping on connect would
    // be noise.
    ping.tick().await;
    let mut unanswered: u32 = 0;

    loop {
        tokio::select! {
            event = events.recv() => {
                // Both `Closed` and `Lagged` drop this client. Lagging means the
                // buffer overran while it was not reading: its view is already
                // incomplete, and retrying would spin. Reconnecting refetches.
                let Ok(event) = event else { break };
                let Ok(json) = serde_json::to_string(&event) else { continue };
                if !send(&mut socket, Message::text(json), config.every).await {
                    break;
                }
            }
            _ = ping.tick() => {
                if unanswered >= config.missed_allowed {
                    break;
                }
                unanswered += 1;
                if !send(&mut socket, Message::Ping(Bytes::new()), config.every).await {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    // Any frame proves the peer is alive, but a pong is the one
                    // we asked for. (Client pings are answered by the protocol
                    // layer, below this loop.)
                    Some(Ok(Message::Pong(_))) => unanswered = 0,
                    Some(Ok(_)) => {}
                    // A close frame or a broken socket: nothing left to do.
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
    send(&mut socket, Message::Close(None), config.every).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A corpus's branch is discovery's answer from whenever it last walked the
    /// allowlist; the ref reader's is from its last pass. The union's heading
    /// and the sidebar must show the second, or a checkout that moved reads
    /// `main (primary)` above the reader's own "no served worktree is on
    /// `main`".
    #[test]
    fn a_corpus_shows_the_branch_the_ref_reader_last_resolved() {
        let corpus = |dir: &str, branch: Option<&str>| Corpus {
            cid: format!("cid-{dir}"),
            root: PathBuf::from(format!("/tmp/{dir}")),
            base: PathBuf::from(format!("/tmp/{dir}/inventory")),
            group: "g".into(),
            branch: branch.map(str::to_string),
            is_primary: true,
            error: None,
        };
        let branches = Branches {
            branch_of: [
                (PathBuf::from("/tmp/moved"), Some("topic".to_string())),
                (PathBuf::from("/tmp/detached"), None),
            ]
            .into_iter()
            .collect(),
            ..Branches::default()
        };

        let mut moved = corpus("moved", Some("main"));
        freshen_branch(&mut moved, &branches);
        assert_eq!(moved.branch.as_deref(), Some("topic"), "the fresh answer");

        // The reader looked and found no branch: that overwrites too, or the
        // heading keeps a name nothing is on.
        let mut detached = corpus("detached", Some("main"));
        freshen_branch(&mut detached, &branches);
        assert_eq!(detached.branch, None);

        // But a corpus the reader has no entry for is left alone: no
        // repository, or a pass that could not open one, is not evidence that
        // anything moved.
        let mut unknown = corpus("unknown", Some("main"));
        freshen_branch(&mut unknown, &branches);
        assert_eq!(unknown.branch.as_deref(), Some("main"));
    }

    #[test]
    fn empty_query_values_mean_no_filter() {
        let filter = DocQuery {
            type_name: Some(String::new()),
            status: Some("open".into()),
            tag: Some("server".into()),
        }
        .into_filter();
        assert_eq!(filter.type_name, None, "`?type=` must not filter on \"\"");
        assert_eq!(filter.status.as_deref(), Some("open"));
        assert_eq!(filter.tag.as_deref(), Some("server"), "`?tag=` is plumbed");
    }

    /// One shape for every failure, whatever produced it — a client parsing our
    /// errors should never need a second code path.
    #[tokio::test]
    async fn errors_render_as_one_json_shape() {
        let cases = [
            (ApiError::bad_request("nope"), StatusCode::BAD_REQUEST),
            (ApiError::forbidden("elsewhere"), StatusCode::FORBIDDEN),
            (ApiError::not_found("gone"), StatusCode::NOT_FOUND),
            (
                ApiError::from(OpysError::Store("boom".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (error, expected) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), expected);
            let bytes = http_body_util::BodyExt::collect(response.into_body())
                .await
                .unwrap()
                .to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(body["error"].is_string(), "{body}");
        }
    }

    #[test]
    fn loopback_is_recognised_in_every_spelling() {
        for host in [
            "localhost",
            "localhost:6797",
            "LOCALHOST:6797",
            "127.0.0.1",
            "127.0.0.1:6797",
            "127.1.2.3:6797",
            "[::1]:6797",
            "::1",
        ] {
            assert!(is_loopback_host(host), "{host} is this machine");
        }
        for host in [
            "evil.attacker.test",
            "evil.attacker.test:6797",
            "10.0.0.5:6797",
            "127.0.0.1.attacker.test",
            "[2001:db8::1]:6797",
            "",
        ] {
            assert!(!is_loopback_host(host), "{host} is not this machine");
        }
    }

    #[test]
    fn an_origin_is_its_host_and_null_is_not_one() {
        assert_eq!(origin_host("http://localhost:5173"), Some("localhost"));
        assert_eq!(origin_host("https://evil.example"), Some("evil.example"));
        assert_eq!(origin_host("http://[::1]:6797"), Some("::1"));
        assert_eq!(origin_host("null"), None, "a sandboxed frame is not local");
        assert_eq!(origin_host("http://"), None);
    }
}
