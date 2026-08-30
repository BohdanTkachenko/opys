//! The write path: one CLI-identical cycle per request (TASK-0072).
//!
//! The node is a *remote hand on the local write path*, never a second write
//! authority (ADR-0051). Two rules follow, and both are load-bearing:
//!
//! - **Never write through a warm store.** The corpus actors hold stores loaded
//!   in the past and *without* the inventory lock (that is the whole point of
//!   `actor.rs`'s one rule); flushing one would silently overwrite every `opys`
//!   invocation made since. So every action opens its own [`Project`], loads its
//!   own [`Store`] — which is what takes the FEAT-0021 flock — and flushes that.
//!   Nothing in here touches an actor; catching the warm cache up afterwards is
//!   the caller's job (`api::action` asks the actor to reload once the flock is
//!   back, rather than trusting the watcher to notice).
//! - **The request body is a closed enum.** [`Action`] is the entire vocabulary
//!   a client has. Nothing here accepts a filesystem path, a command line, or
//!   even a field it does not recognise: `deny_unknown_fields` turns anything
//!   else into a deserialization failure before this module is reached, and that
//!   is the no-arbitrary-execution guarantee (ADR-0077), not a nicety.
//! - **The corpus root is a boundary.** The allowlist is the whole security
//!   boundary (ADR-0077), and `Project::open`'s upward search would walk
//!   straight through it; [`perform`] refuses to write anywhere but the corpus
//!   it was addressed. See its docs.
//!
//! What [`perform`] reproduces is not "roughly what the CLI does" but the exact
//! call sequence of `src/commands/*.rs`, including the one place those commands
//! disagree with each other. See its docs.

use std::path::Path;

use opys_engine::backend::Backend;
use opys_engine::commands::{block, close, edit, new, set_status, sync, tag};
use opys_engine::error::OpysError;
use opys_engine::project::Project;
use opys_engine::store::Store;
use serde::Deserialize;

/// What a client is told when the inventory lock could not be had.
///
/// Deliberately says nothing about *where*: the engine's own message names the
/// inventory directory and the lock file under `$XDG_RUNTIME_DIR`, and this is
/// the one endpoint whose premise is that it never traffics in filesystem paths
/// (ADR-0077).
const BUSY: &str = "the inventory is busy — another opys invocation is holding the lock; retry";

/// The result of asking the node to write.
pub type Attempt = std::result::Result<ActionOutcome, ActionError>;

/// Everything a client may ask the node to write — and nothing else.
///
/// One variant per mutating CLI command, carrying that command's arguments in
/// the same shape. Absent optional fields become the same values clap hands the
/// core (an empty `tags`, an empty `status`, `None` for a reason), so an action
/// and the corresponding `opys` invocation reach the engine identically; in
/// particular the *empty* status is deliberate, because resolving a type's
/// default status is `new::core`'s job and duplicating it here would be a second
/// copy of a rule that can change.
///
/// `type` and `title` are the exception: the CLI defaults `--type` to `feature`
/// for interactive convenience, but a client that forgets the field deserves to
/// be told so rather than to have a document appear under a type the project may
/// not even declare.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    /// Create a document. `tags`, `features` are comma-separated lists.
    New {
        #[serde(rename = "type")]
        type_name: String,
        title: String,
        #[serde(default)]
        tags: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        features: String,
        reason: Option<String>,
    },
    /// Move a document to another status, with an optional `<status>_reason`.
    SetStatus {
        id: String,
        status: String,
        reason: Option<String>,
    },
    /// Add and/or remove tags; both are comma-separated lists.
    Tag {
        id: String,
        add: Option<String>,
        remove: Option<String>,
    },
    /// Record that `id` is blocked by `by`, writing both directions.
    Block { id: String, by: String },
    /// Remove that blocker link from both sides.
    Unblock { id: String, by: String },
    /// Reach the type's terminal status: delete the document and strike it
    /// through wherever it is referenced.
    Close {
        id: String,
        #[serde(default)]
        force: bool,
    },
    /// Replace the document's whole markdown body (the web UI's edit-in-place).
    /// Verify-gated in the engine: the edit lands only if it introduces no new
    /// verify problems, and a refusal flushes nothing.
    EditBody { id: String, body: String },
    /// Set one custom frontmatter field, verify-gated like `edit-body` — the
    /// closed-frontmatter invariant, declared types, and enum/pattern
    /// constraints are all the gate's findings, refused with their own
    /// messages. `value` is parsed as the CLI parses `--field key=value`:
    /// a YAML scalar if it reads as one, a bare string otherwise.
    ///
    /// Removal is deliberately its own action rather than a nullable `value`
    /// here: with an `Option`, a client that *forgot* the field would silently
    /// delete one instead of being told.
    SetField {
        id: String,
        key: String,
        value: String,
    },
    /// Remove one custom frontmatter field, verify-gated like `set-field` (so
    /// removing a required field is refused by name).
    RemoveField { id: String, key: String },
}

impl Action {
    /// The wire name of this action — what arrived in the `action` key, and what
    /// the `action-completed` event reports. Spelled out rather than derived so
    /// the event vocabulary cannot drift from the request vocabulary silently.
    pub fn name(&self) -> &'static str {
        match self {
            Action::New { .. } => "new",
            Action::SetStatus { .. } => "set-status",
            Action::Tag { .. } => "tag",
            Action::Block { .. } => "block",
            Action::Unblock { .. } => "unblock",
            Action::Close { .. } => "close",
            Action::EditBody { .. } => "edit-body",
            Action::SetField { .. } => "set-field",
            Action::RemoveField { .. } => "remove-field",
        }
    }
}

/// Why an action did not happen.
///
/// Three outcomes, because a caller has to act on them differently and one
/// status code cannot say which is which: the corpus is gone (stop asking), the
/// node was busy (retry, unchanged), the corpus refused (the write is invalid —
/// tell the user).
#[derive(Debug)]
pub enum ActionError {
    /// The corpus is not an opys project at the path it was allowlisted at any
    /// more. Nothing was written, and nothing will be until it comes back or the
    /// manager retires it.
    Gone,
    /// Another writer held the inventory lock past `OPYS_LOCK_TIMEOUT_MS`.
    /// Nothing was written; the same request will very likely work in a moment.
    Busy,
    /// The corpus refused the write — an unknown status, a terminal status
    /// reached without `close`, an id that resolves to nothing. This is the
    /// class the CLI exits 2 on, and the message is the line it prints.
    Refused(OpysError),
}

impl ActionError {
    /// Classify a failure from the load that takes the inventory flock.
    ///
    /// Contention is retryable and its message carries absolute paths, so it
    /// must not be delivered as a permanent refusal. There is no [`OpysError`]
    /// variant for it yet — adding one is an engine change — so the backend's
    /// own wording is what identifies it. `the_lock_timeout_is_recognised` pins
    /// this against the backend's message drifting.
    fn from_load(e: OpysError) -> ActionError {
        match &e {
            OpysError::Usage(m) if m.contains("waiting for the inventory lock") => {
                ActionError::Busy
            }
            _ => ActionError::Refused(e),
        }
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::Gone => f.write_str("this corpus is no longer an opys project"),
            ActionError::Busy => f.write_str(BUSY),
            ActionError::Refused(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ActionError::Refused(e) => Some(e),
            _ => None,
        }
    }
}

/// What a completed action reports back.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    /// The document the action touched: allocated by `new`, echoed by the rest.
    pub id: String,
    /// The line the CLI prints for the same command. Callers show it; nothing
    /// parses it. (`new`'s is the created document's path, made relative to the
    /// project root the way every other path this crate emits is — an absolute
    /// one means nothing on the machine reading it.)
    pub message: String,
    /// Why the auto-sync pass was skipped, when it was.
    ///
    /// The write itself still happened and is authoritative; what did not happen
    /// is the reconcile/linkify/relocate pass, because some *other* document in
    /// the corpus will not parse (or the lock went away between the two cycles).
    /// The CLI says so on stderr and exits 0; the node is headless, so the
    /// response is the only place it can say so at all.
    pub sync_skipped: Option<String>,
}

impl ActionOutcome {
    fn new(id: &str, message: String) -> ActionOutcome {
        ActionOutcome {
            id: id.to_string(),
            message,
            sync_skipped: None,
        }
    }
}

/// Run one action against the corpus rooted at `root`, exactly as the CLI would.
///
/// The sequence is the one in `src/commands/*.rs`, verbatim: open the project,
/// load a store (which takes the inventory lock), call the print-free core,
/// flush (which releases it), then run the auto-sync pass. That last step is its
/// own second load/flush cycle rather than a pass over the store above, and the
/// difference is visible on disk: `sync::pass` backfills a missing
/// `created`/`updated` from the file's mtime, so folding it into the first cycle
/// would read the *pre-write* mtime and write different bytes.
///
/// The one place the six commands disagree with each other is what happens when
/// a core refuses. `new::run` propagates immediately — nothing is flushed, no
/// sync runs. The other five capture the error, flush and sync anyway, and
/// return it last, so a refused write still leaves the corpus reconciled. That
/// asymmetry is not tidied up here: it is observable whenever the corpus was not
/// already in sync, and the point of this module is that the same write through
/// the API and through the CLI leaves the same bytes.
///
/// Blocking, and possibly for a while — the load waits out `OPYS_LOCK_TIMEOUT_MS`
/// (10 s by default) if another writer holds the inventory lock, which is
/// exactly what a concurrent `opys` invocation would do. Callers run this on a
/// blocking pool.
///
/// **`root` is a boundary, not a starting point.** `Project::open` searches
/// *upward* for `opys.toml`, which is the CLI's convenience and would be this
/// module's escape hatch: a corpus whose own config has gone (a branch switch, a
/// removed worktree, a rename) still resolves for up to a minute, because
/// `Manager::refresh` only retires it on the next tick — and the cycle would
/// then land in the nearest *enclosing* project, which the user never
/// allowlisted. Nested projects are the normal shape a `[[prefix]]` entry
/// produces, so this is not exotic. The allowlist is ADR-0077's whole security
/// boundary, so the walk is refused rather than inherited.
pub fn perform(root: &Path, backend: &dyn Backend, action: &Action) -> Attempt {
    // The exact predicate `Manager::refresh` uses to decide a corpus is gone.
    if !root.join("opys.toml").is_file() {
        return Err(ActionError::Gone);
    }
    let prj = Project::open(&root.to_string_lossy()).map_err(ActionError::Refused)?;
    // Belt and braces: `root` is canonical (`discover::Corpus`), so a project
    // rooted anywhere else means the config vanished between the check above and
    // the open, or a symlink resolved out of the corpus.
    if prj.root.as_path() != root {
        return Err(ActionError::Gone);
    }
    // Unparsable documents come back here as messages rather than as a failure,
    // and the CLI drops them at exactly this point: they are the corpus's
    // problem, reported by `verify`, not this write's.
    let (mut store, parse_errors) = backend.load(&prj).map_err(ActionError::from_load)?;
    match action {
        Action::New {
            type_name,
            title,
            tags,
            status,
            features,
            reason,
        } => {
            // The trailing `&[]` is the CLI's repeatable `--field key=value`,
            // which the action vocabulary deliberately does not expose.
            let doc = new::core(
                &prj,
                &mut store,
                type_name,
                title,
                tags,
                status,
                features,
                reason.as_deref(),
                &[],
            )
            .map_err(ActionError::Refused)?;
            // The CLI prints the created document's path, so that is the
            // message — relative to the project root, like every other document
            // path the API emits; the id is reported separately and stays bare.
            let mut outcome = ActionOutcome::new(
                doc.id().unwrap_or_default(),
                doc.path
                    .strip_prefix(&prj.root)
                    .unwrap_or(&doc.path)
                    .to_string_lossy()
                    .into_owned(),
            );
            backend.flush(&prj, store).map_err(ActionError::Refused)?;
            outcome.sync_skipped = auto_sync(&prj, backend);
            Ok(outcome)
        }
        Action::SetStatus { id, status, reason } => {
            let done = set_status::core(&prj, &mut store, id, status, reason.as_deref())
                .map(|()| ActionOutcome::new(id, format!("{id} -> {status}")))
                .map_err(ActionError::Refused);
            finish(&prj, backend, store, done)
        }
        Action::Tag { id, add, remove } => {
            let done = tag::core(&prj, &mut store, id, add.as_deref(), remove.as_deref())
                .map(|tags| ActionOutcome::new(id, format!("{id} tags: {}", tags.join(", "))))
                .map_err(ActionError::Refused);
            finish(&prj, backend, store, done)
        }
        Action::Block { id, by } => {
            let done = block::block_core(&prj, &mut store, id, by)
                .map(|()| ActionOutcome::new(id, format!("{id} blocked by {by}")))
                .map_err(ActionError::Refused);
            finish(&prj, backend, store, done)
        }
        Action::Unblock { id, by } => {
            let done = block::unblock_core(&prj, &mut store, id, by)
                .map(|()| ActionOutcome::new(id, format!("{id} no longer blocked by {by}")))
                .map_err(ActionError::Refused);
            finish(&prj, backend, store, done)
        }
        Action::EditBody { id, body } => {
            // Like `new`, not like the other five: a refused edit propagates
            // *before* the flush, because the whole point of the verify gate is
            // that a rejected body never reaches disk.
            edit::body_core(&prj, &mut store, id, body, &parse_errors)
                .map_err(ActionError::Refused)?;
            let mut outcome = ActionOutcome::new(id, format!("{id} body updated (verified)"));
            backend.flush(&prj, store).map_err(ActionError::Refused)?;
            outcome.sync_skipped = auto_sync(&prj, backend);
            Ok(outcome)
        }
        Action::SetField { id, key, value } => {
            // Verify-gated like `edit-body`: a refusal propagates before the
            // flush, so a rejected field never reaches disk.
            edit::field_core(&prj, &mut store, id, key, Some(value), &parse_errors)
                .map_err(ActionError::Refused)?;
            let mut outcome = ActionOutcome::new(id, format!("{id} {key} set (verified)"));
            backend.flush(&prj, store).map_err(ActionError::Refused)?;
            outcome.sync_skipped = auto_sync(&prj, backend);
            Ok(outcome)
        }
        Action::RemoveField { id, key } => {
            edit::field_core(&prj, &mut store, id, key, None, &parse_errors)
                .map_err(ActionError::Refused)?;
            let mut outcome = ActionOutcome::new(id, format!("{id} {key} removed (verified)"));
            backend.flush(&prj, store).map_err(ActionError::Refused)?;
            outcome.sync_skipped = auto_sync(&prj, backend);
            Ok(outcome)
        }
        Action::Close { id, force } => {
            let done = close::core(&prj, &mut store, id, *force)
                .map(|()| {
                    ActionOutcome::new(
                        id,
                        format!("closed {id} (deleted; references struck through)"),
                    )
                })
                .map_err(ActionError::Refused);
            finish(&prj, backend, store, done)
        }
    }
}

/// The tail every command except `new` runs: flush and sync happen even when the
/// core refused, and only then is the core's verdict returned. A failure to
/// flush masks that verdict, which is also what the CLI's `ctx.flush(…)?` does.
fn finish(prj: &Project, backend: &dyn Backend, store: Store, attempted: Attempt) -> Attempt {
    backend.flush(prj, store).map_err(ActionError::Refused)?;
    let skipped = auto_sync(prj, backend);
    attempted.map(|mut outcome| {
        outcome.sync_skipped = skipped;
        outcome
    })
}

/// The auto-sync pass — reconcile relations, linkify prose, relocate documents
/// onto their canonical layout paths — run best-effort, as `maybe_sync` runs it.
/// Returns why it was skipped, when it was.
///
/// `sync::run` refuses the whole pass when *any* document in the corpus fails to
/// parse, which is a pre-existing broken file rather than anything this write
/// did. The CLI answers that with a note on stderr and still exits 0. A node
/// that reported it as a *failure* would be telling a client its write was
/// rejected after the write had already reached disk, and the retry would apply
/// it twice — but not reporting it at all is a different mistake: the pass that
/// maintains relation maps and prose links stopped running and the only channel
/// this node has is the response. So: still a success, with the reason attached.
fn auto_sync(prj: &Project, backend: &dyn Backend) -> Option<String> {
    // Through the same classifier as the first load, so a lock timeout here
    // reports as "busy" rather than handing the client a lock-file path.
    sync::run(prj, backend)
        .err()
        .map(|e| ActionError::from_load(e).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: serde_json::Value) -> std::result::Result<Action, String> {
        serde_json::from_value(json).map_err(|e| e.to_string())
    }

    /// **The no-arbitrary-execution test.** The vocabulary is closed on both
    /// axes — an action nobody implemented and a field nobody declared are both
    /// refused before any code runs — and that is the whole guardrail, so it is
    /// pinned here as well as over HTTP.
    #[test]
    fn the_vocabulary_is_closed() {
        let unknown_action = parse(serde_json::json!({"action": "exec", "cmd": "rm -rf /"}))
            .expect_err("an action nobody implemented is not deserializable");
        assert!(
            unknown_action.contains("unknown variant"),
            "{unknown_action}"
        );

        let unknown_field = parse(serde_json::json!({
            "action": "close", "id": "NOTE-0001", "path": "/etc/passwd"
        }))
        .expect_err("a field nobody declared is not deserializable");
        assert!(unknown_field.contains("unknown field"), "{unknown_field}");

        let missing_tag = parse(serde_json::json!({"id": "NOTE-0001"}))
            .expect_err("a body with no action at all");
        assert!(missing_tag.contains("action"), "{missing_tag}");
    }

    /// Absent optionals must reach the cores as the values clap hands them, not
    /// as something this crate invented — an empty status especially, since
    /// resolving a type's default is the engine's job.
    #[test]
    fn absent_optionals_become_the_cli_defaults() {
        let action = parse(serde_json::json!({
            "action": "new", "type": "note", "title": "Only the required fields"
        }))
        .expect("type and title are enough");
        let Action::New {
            type_name,
            title,
            tags,
            status,
            features,
            reason,
        } = action
        else {
            panic!("deserialized as the wrong variant");
        };
        assert_eq!(type_name, "note");
        assert_eq!(title, "Only the required fields");
        assert_eq!(tags, "", "`--tags` defaults to empty, not to a made-up tag");
        assert_eq!(
            status, "",
            "the type's default status is `new::core`'s call"
        );
        assert_eq!(features, "");
        assert_eq!(reason, None);

        let close = parse(serde_json::json!({"action": "close", "id": "NOTE-0001"}))
            .expect("force is optional");
        assert!(matches!(close, Action::Close { force: false, .. }));
    }

    /// Why removal is its own action: a `set-field` with no `value` must be a
    /// request error, not a silent field deletion.
    #[test]
    fn set_field_without_a_value_is_rejected_not_a_removal() {
        let missing =
            parse(serde_json::json!({"action": "set-field", "id": "N-1", "key": "priority"}))
                .expect_err("value is required");
        assert!(missing.contains("value"), "{missing}");
    }

    /// Lock contention is retryable and its message names the inventory
    /// directory and the lock file; both facts make it the one load failure that
    /// must not be delivered as a permanent, path-carrying refusal. The input
    /// here is the backend's real wording, so this fails if that drifts.
    #[test]
    fn the_lock_timeout_is_recognised() {
        let timed_out = OpysError::Usage(
            "timed out after 300 ms waiting for the inventory lock for /home/dan/p/inventory \
             (/run/user/1000/opys-lock-_home_dan_p_inventory-d77af5c8d2baff08) — another opys \
             invocation is holding it; raise OPYS_LOCK_TIMEOUT_MS to wait longer"
                .to_string(),
        );
        let busy = ActionError::from_load(timed_out);
        assert!(matches!(busy, ActionError::Busy), "{busy:?}");
        let said = busy.to_string();
        assert!(!said.contains('/'), "the reply must carry no paths: {said}");

        // Everything else the load can fail with stays the caller's answer.
        let refused = ActionError::from_load(OpysError::NotFound {
            id: "NOTE-0001".into(),
        });
        assert!(matches!(refused, ActionError::Refused(_)), "{refused:?}");
        let corrupt = ActionError::from_load(OpysError::Usage("corrupt ledger line".into()));
        assert!(matches!(corrupt, ActionError::Refused(_)), "{corrupt:?}");
    }

    /// `Project::open` searches upward, so a corpus that has lost its config
    /// would otherwise be written to through its parent. Nothing is opened, no
    /// lock is taken, no id is allocated.
    #[test]
    fn a_corpus_without_its_own_config_is_gone_not_climbed() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path();
        std::fs::write(outer.join("opys.toml"), "base = \"inventory\"\n").unwrap();
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();

        let refusal = perform(
            &inner,
            &opys_backend_markdown_local::MarkdownLocal,
            &Action::Tag {
                id: "NOTE-0001".into(),
                add: Some("x".into()),
                remove: None,
            },
        )
        .expect_err("a corpus with no opys.toml of its own cannot be written to");
        assert!(matches!(refusal, ActionError::Gone), "{refusal:?}");
        assert!(
            !outer.join("inventory").exists(),
            "the enclosing project must not have been touched at all"
        );
    }

    /// The wire spelling of every action, in both directions: the tag that
    /// selects a variant and the name the `action-completed` event carries.
    #[test]
    fn every_action_names_itself_as_it_arrived() {
        let cases = [
            (
                serde_json::json!({"action": "new", "type": "note", "title": "T"}),
                "new",
            ),
            (
                serde_json::json!({"action": "set-status", "id": "N-1", "status": "open"}),
                "set-status",
            ),
            (
                serde_json::json!({"action": "tag", "id": "N-1", "add": "a"}),
                "tag",
            ),
            (
                serde_json::json!({"action": "block", "id": "N-1", "by": "N-2"}),
                "block",
            ),
            (
                serde_json::json!({"action": "unblock", "id": "N-1", "by": "N-2"}),
                "unblock",
            ),
            (serde_json::json!({"action": "close", "id": "N-1"}), "close"),
            (
                serde_json::json!({"action": "set-field", "id": "N-1", "key": "k", "value": "v"}),
                "set-field",
            ),
            (
                serde_json::json!({"action": "remove-field", "id": "N-1", "key": "k"}),
                "remove-field",
            ),
        ];
        for (body, name) in cases {
            let action = parse(body.clone()).unwrap_or_else(|e| panic!("{body}: {e}"));
            assert_eq!(action.name(), name, "{body}");
        }
    }
}
