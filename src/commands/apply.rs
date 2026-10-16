//! `opys apply` — a status change, field writes and a log entry as **one**
//! all-or-nothing write.
//!
//! [`crate::commands::apply_batch`] already makes a write all-or-nothing over
//! several *ids*; this module makes one all-or-nothing over several *actions*
//! on one document. The two compose: `run` drives the action batch under the id
//! batch, against one loaded store, and flushes once — after every action on
//! every id succeeded — so a refusal anywhere leaves the corpus byte-identical.
//!
//! **Why the general form and not an `opys claim` verb.** The composite write
//! an agent makes constantly is a claim (move to the working status, stamp the
//! field that records who took it, log that it was taken), but the same shape
//! is what releasing and completing need, and a `claim` verb would put
//! dispatcher vocabulary into a tracker that knows nothing about agents. The
//! field that records the claimant is an ordinary project-declared field
//! (`[types.task.fields.claimed_by]`), never a reserved key: opys stores who
//! claimed a document and nothing else about them (ADR-0027).
//!
//! **The order is fixed at set-fields → log → status → remove-fields, and is
//! not the caller's.** The two steps before the status are there because the
//! status step reads them; the removals are last because nothing reads them at
//! all.
//!
//! - `set_status::core_with` evaluates [`crate::rules`] against the document as
//!   it stands in the store, *before* appending the new history entry. So a
//!   `require_log_since_status` rule on the target status is satisfied by this
//!   very invocation's `--log` (the entry's stamp is at or after the last
//!   differing history key), and a `require_field` rule by this very
//!   invocation's `--field`. Write the status first and a claim into a guarded
//!   status becomes impossible in one command.
//! - `Store::append_history` must come after the `put_doc` that changed the
//!   status, which is `set_status`'s own contract — one more reason the status
//!   step follows both writes that feed it.
//!
//! **The verify gate is taken once around the whole batch**, not once per
//! field. [`edit::problems`] is snapshotted before the first step and
//! differenced by [`edit::refuse_new_problems`] after the last (the steps
//! themselves call the ungated [`edit::field_write`]), so every part of the
//! batch is judged against the document the batch *finishes* with — never
//! against a half-applied one at a status the invocation is on its way out of.
//! Under the natural claim config, which is what makes a claim mean anything at
//! all (`when = { type = "task", status = "doing" }`, `require_field =
//! "claimed_by"`), that is what keeps both halves of a hand-off one command:
//! releasing (`--status todo --remove-field claimed_by`) is not refused by the
//! `doing`-only rule the document is leaving, and handing on
//! (`--status review --field claimed_by=reviewer-3`) is not refused by a
//! `field_matches` rule scoped to `doing`. A write or a removal the *target*
//! status forbids is still refused by name: the target status is the one the
//! finished document holds. Only the status being left loses its say, and only
//! over the parts of the batch that leave with it.
//!
//! The printed line names the parts in a fixed reading order — status, fields
//! set, fields removed, then the log entry — rather than in execution order:
//! the status is what the batch hangs on and what a reader looks for first.
//!
//! **The race the fixed order makes decidable.** Two dispatchers read the same
//! document as pickable and both try to take it. The inventory flock only
//! *serializes* them; what decides which one is legitimate is the status write,
//! which here asserts a transition ([`set_status::Move::Transition`]): the
//! loser arrives after the winner's flush, finds the document already in the
//! working status, and is refused — with the transition guard's message, not
//! the lock's — before it has written a field, a log line or anything else.
//!
//! Field writes are gated by [`crate::commands::edit`], the one implementation
//! of the verify gate, so an undeclared key, a wrong type or a broken
//! enum/pattern constraint each come back with their own message and abort the
//! whole invocation — rather than through a second, weaker declared-field check
//! of this module's own. Bracketing the batch instead of each field also costs
//! two whole-corpus verify passes per invocation rather than two per field.

use crate::commands::{apply_batch, edit, log, maybe_sync, print_lines, resolve_ids, set_status};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::store::Store;
use crate::Ctx;

/// What one composite invocation asks of a single document.
///
/// Every part is optional, but not every combination is meaningful:
/// [`Apply::check`] refuses the ones that would silently drop a flag the caller
/// passed, and it is [`core`]'s first act so the node refuses them too.
#[derive(Default)]
pub struct Apply<'a> {
    /// `--status`: the transition to assert. The equal case is a refusal here,
    /// unlike `set-status` — see [`set_status::Move`].
    pub status: Option<&'a str>,
    /// `--reason`: written to `<status>_reason`, which the type must declare.
    pub reason: Option<&'a str>,
    /// `--field key=value`, in the order given.
    pub set_fields: &'a [(String, String)],
    /// `--remove-field key`, in the order given.
    pub remove_fields: &'a [String],
    /// `--log`: the entry text.
    pub log: Option<&'a str>,
    /// `--section`: which log section, for a type that declares more than one.
    pub section: Option<&'a str>,
}

impl Apply<'_> {
    /// Whether the invocation asks for anything at all.
    fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.set_fields.is_empty()
            && self.remove_fields.is_empty()
            && self.log.is_none()
    }

    /// The usage checks that do not need a corpus, in one place so both entry
    /// points make them: [`run`] before it even opens the project, and [`core`]
    /// for the node, whose typed action reaches the engine here.
    ///
    /// `--reason` and `--section` are *modifiers*, not actions: each is read by
    /// exactly one step, and passing one without that step would write nothing
    /// and say nothing. Everything else `apply` is handed badly it refuses, and
    /// `--section` in particular is checked against the type's sections by
    /// `log::resolve_section` — so swallowing it without `--log` would let a
    /// misspelling through that the same misspelling beside `--log` catches.
    pub fn check(&self) -> Result<()> {
        if self.is_empty() {
            return Err(usage(
                "apply needs something to do: pass --status, --field, --remove-field and/or --log",
            ));
        }
        if self.reason.is_some() && self.status.is_none() {
            return Err(usage(
                "--reason writes <status>_reason and needs --status: pass the status this \
                 reason is for",
            ));
        }
        if self.section.is_some() && self.log.is_none() {
            return Err(usage(
                "--section names the section --log appends to and needs --log",
            ));
        }
        Ok(())
    }
}

/// Apply every part of `spec` to one document inside the loaded store, in the
/// fixed order set-fields → log → status → remove-fields. Returns the line the
/// command prints (whose part order is the reading order, not the execution
/// order). Does not flush/print/sync: the caller flushes on `Ok` and drops the
/// store on `Err`, so a refused composite leaves no trace on disk.
pub fn core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    spec: &Apply<'_>,
    parse_errors: &[String],
) -> Result<String> {
    // The corpus-free usage checks, here rather than only in `run`, because the
    // node's typed action calls `core` directly and must refuse the same bodies
    // the CLI refuses — with the same words.
    spec.check()?;
    // Pre-flight, before anything is mutated: the loser of a claim race must be
    // told its transition is refused, rather than being handed whatever the
    // field or log step hit on the way there. `core_with` enforces it again.
    if let Some(status) = spec.status {
        set_status::check_move(prj, store, id, status, set_status::Move::Transition)?;
    }

    // One verify gate for the whole batch, not one per field: what has to hold
    // is the *finished* document, and a step judged in isolation is judged at a
    // status the invocation is on its way out of. See the module doc.
    let before = edit::problems(prj, store, parse_errors)?;

    let mut set_parts: Vec<String> = Vec::new();
    for (key, value) in spec.set_fields {
        edit::field_write(prj, store, id, key, Some(value))?;
        set_parts.push(format!("{key} set"));
    }
    let mut log_part: Option<String> = None;
    if let Some(text) = spec.log {
        let line = log::core(prj, store, id, text, spec.section)?;
        // `log::core` returns the whole line it would print, `<id> logged to
        // '## H'`; the id is this line's already, so only the tail joins here.
        // Reusing it keeps one source for the wording, including the
        // "(section created)" tail.
        log_part = Some(
            line.strip_prefix(&format!("{id} "))
                .unwrap_or(&line)
                .to_string(),
        );
    }
    if let Some(status) = spec.status {
        set_status::core_with(
            prj,
            store,
            id,
            status,
            spec.reason,
            set_status::Move::Transition,
        )?;
    }
    let mut remove_parts: Vec<String> = Vec::new();
    for key in spec.remove_fields {
        edit::field_write(prj, store, id, key, None)?;
        remove_parts.push(format!("{key} removed"));
    }
    // The gate closes here, over the document as the whole batch left it.
    edit::refuse_new_problems(prj, store, &before, parse_errors)?;

    let parts: Vec<String> = spec
        .status
        .map(|status| format!("-> {status}"))
        .into_iter()
        .chain(set_parts)
        .chain(remove_parts)
        .chain(log_part)
        .collect();
    Ok(format!("{id} {}", parts.join(", ")))
}

pub fn run(ctx: &Ctx, ids: &[String], spec: &Apply<'_>) -> Result<()> {
    // Refused before the corpus is even opened. `core` checks again, for the
    // node's sake; making it twice costs nothing and neither caller can skip it.
    spec.check()?;
    if spec.log == Some("-") {
        // `-` in the id list already means "read the list from stdin", and two
        // stdin consumers in one invocation is exactly the ambiguity that made
        // `opys log` take a single id. A multi-line entry still fits in a
        // single argument.
        return Err(usage(
            "--log does not read stdin (`-` is the id list's): pass the text as one argument, \
             or use `opys log <id> -`",
        ));
    }
    let prj = ctx.open()?;
    let ids = resolve_ids(ids)?;
    // The load's unparsable-document messages are the verify baseline the field
    // gate differences against; every other command drops them.
    let (mut store, parse_errors) = ctx.load(&prj)?;
    let lines = apply_batch(&ids, |id| core(&prj, &mut store, id, spec, &parse_errors))?;
    ctx.flush(&prj, store)?;
    print_lines(lines)?;
    maybe_sync(ctx, &prj);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use crate::store::LoadedCorpus;

    const CFG: &str = r#"
pad = 4
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.transitions]
todo = ["doing"]
doing = ["review", "todo"]
review = ["done", "doing"]
[types.task.fields.claimed_by]
type = "string"
[[types.task.sections]]
heading = "Progress"
kind = "log"
"#;

    /// [`CFG`] plus the rule that makes a claim mean anything: the working
    /// status requires the claimant field. Pairing the two is how a project
    /// stops a document from sitting in `doing` with nobody on it — and it is
    /// exactly the configuration under which the order of the removal step
    /// decides whether a release is expressible in one command.
    const GUARDED_CFG: &str = r#"
pad = 4
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.transitions]
todo = ["doing"]
doing = ["review", "todo"]
review = ["done", "doing"]
[types.task.fields.claimed_by]
type = "string"
[[types.task.sections]]
heading = "Progress"
kind = "log"
[[rules]]
when = { type = "task", status = "doing" }
require_field = "claimed_by"
"#;

    /// [`GUARDED_CFG`] plus a value constraint scoped to the same working
    /// status: while a task is `doing`, whoever holds it must be an agent.
    /// Handing it on to a reviewer is the mirror image of the release case —
    /// a *finished* state the status being left would refuse.
    const HANDOFF_CFG: &str = r#"
pad = 4
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.transitions]
todo = ["doing"]
doing = ["review", "todo"]
review = ["done", "doing"]
[types.task.fields.claimed_by]
type = "string"
[[types.task.sections]]
heading = "Progress"
kind = "log"
[[rules]]
when = { type = "task", status = "doing" }
require_field = "claimed_by"
[[rules]]
when = { type = "task", status = "doing" }
field_matches = { field = "claimed_by", pattern = "^agent-" }
"#;

    /// [`CFG`] with the `[types.task.transitions]` block removed: a type that
    /// declares no graph at all, for which [`crate::lifecycle::allows`] says
    /// yes to every pair. The equal-status refusal must not come from the map.
    const GRAPHLESS_CFG: &str = r#"
pad = 4
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.fields.claimed_by]
type = "string"
[[types.task.sections]]
heading = "Progress"
kind = "log"
"#;

    const DOC: &str = "---\nid: TASK-0001\nstatus: todo\ncreated: \"2026-01-02T03:04:05Z\"\n\
updated: \"2026-01-02T03:04:05Z\"\nhistory:\n  \"2026-01-02T03:04:05.000000Z\": todo\n\
---\n\n# Work\n\n## Progress\n";

    /// A project + store holding one `todo` task, built the way `store`'s own
    /// tests build one (no document filesystem, no inventory lock).
    ///
    /// The clock is deliberately *not* pinned: `OPYS_NOW` is process-wide and
    /// these tests share the process with every other unit test, so they assert
    /// the shape of a stamp rather than its value. The exact bytes a pinned
    /// clock produces are `opys/tests/cli.rs`'s to pin, one process per case.
    fn fixture() -> (tempfile::TempDir, Project, Store) {
        fixture_with(CFG)
    }

    fn fixture_with(cfg: &str) -> (tempfile::TempDir, Project, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("opys.toml"), cfg).unwrap();
        std::fs::create_dir_all(dir.path().join("opys")).unwrap();
        let prj = Project::open(&dir.path().to_string_lossy()).expect("open project");
        let doc = Doc::parse(prj.base.join("TASK-0001.md"), DOC).expect("parse");
        let (store, _) = Store::build(
            &prj,
            LoadedCorpus {
                docs: vec![(doc, None)],
                errors: Vec::new(),
                retired: Vec::new(),
                retired_legacy: false,
                retired_err: None,
            },
        )
        .expect("build");
        (dir, prj, store)
    }

    fn claim<'a>(fields: &'a [(String, String)]) -> Apply<'a> {
        Apply {
            status: Some("doing"),
            set_fields: fields,
            log: Some("claimed"),
            ..Apply::default()
        }
    }

    /// The three writes land as **one** write to the document's file: the flush
    /// plan is what "the file changes exactly once" actually means, and it is
    /// observable here in a way an mtime never is.
    #[test]
    fn the_three_writes_are_one_write_to_one_file() {
        let (_dir, prj, mut store) = fixture();
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        let line = core(&prj, &mut store, "TASK-0001", &claim(&fields), &[]).expect("the claim");
        assert_eq!(
            line,
            "TASK-0001 -> doing, claimed_by set, logged to '## Progress'"
        );

        let plan = store.flush_plan(&prj).expect("a plan");
        assert_eq!(plan.writes.len(), 1, "{:?}", plan.writes);
        assert!(plan.renames.is_empty(), "{:?}", plan.renames);
        assert!(plan.deletes.is_empty(), "{:?}", plan.deletes);
        let (_, text) = &plan.writes[0];
        assert!(text.contains("status: doing"), "{text}");
        assert!(text.contains("claimed_by: agent-7"), "{text}");
        let bullet = text
            .split_once("## Progress\n")
            .and_then(|(_, rest)| rest.lines().next())
            .expect("the log section carries the entry");
        assert!(bullet.starts_with("- 2"), "{text}");
        assert!(bullet.ends_with(" claimed"), "{text}");
    }

    /// The equal-status refusal, which is what makes a second claimant lose:
    /// it is the transition guard, it fires before any part of the batch runs,
    /// and it fires for a type whether or not it declares a graph.
    #[test]
    fn a_second_claim_is_refused_as_a_non_transition() {
        let (_dir, prj, mut store) = fixture();
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        core(&prj, &mut store, "TASK-0001", &claim(&fields), &[]).expect("the first claim");

        let fields = [("claimed_by".to_string(), "agent-9".to_string())];
        let err = core(&prj, &mut store, "TASK-0001", &claim(&fields), &[])
            .expect_err("the second claim is refused");
        let err = err.to_string();
        assert!(err.contains("it is already doing"), "{err}");
        assert!(err.contains("asserts a transition"), "{err}");

        // And it wrote nothing: the store still names the first claimant, with
        // one log entry.
        let plan = store.flush_plan(&prj).expect("a plan");
        let (_, text) = &plan.writes[0];
        assert!(text.contains("claimed_by: agent-7"), "{text}");
        assert_eq!(text.matches("\n- 2").count(), 1, "{text}");
    }

    /// The same refusal for a type that declares **no** `[transitions]` map, so
    /// a graph-less project cannot be claimed twice either. This is the case
    /// the map cannot answer: [`crate::lifecycle::allows`] returns true for
    /// every pair when `allowed_targets` is `None`, so the equal case has to be
    /// settled by [`set_status::Move`] *before* the map is consulted. Fold it
    /// back into `allows` and every other fixture here stays green while both
    /// claimants win.
    #[test]
    fn a_second_claim_is_refused_for_a_type_with_no_transition_graph() {
        let (_dir, prj, mut store) = fixture_with(GRAPHLESS_CFG);
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        core(&prj, &mut store, "TASK-0001", &claim(&fields), &[]).expect("the first claim");

        let fields = [("claimed_by".to_string(), "agent-9".to_string())];
        let err = core(&prj, &mut store, "TASK-0001", &claim(&fields), &[])
            .expect_err("the second claim is refused");
        let err = err.to_string();
        assert!(err.contains("it is already doing"), "{err}");
        assert!(err.contains("asserts a transition"), "{err}");

        // The first claimant survives, and the loser left no log entry.
        let plan = store.flush_plan(&prj).expect("a plan");
        let (_, text) = &plan.writes[0];
        assert!(text.contains("claimed_by: agent-7"), "{text}");
        assert_eq!(text.matches("\n- 2").count(), 1, "{text}");
    }

    /// An undeclared field aborts the whole invocation through the verify gate,
    /// even though the status pre-flight passed, and the gate names that one
    /// finding and nothing else.
    ///
    /// Nothing is asserted about the *store* here, and deliberately so: the
    /// batch-wide bracket runs every step into the store and then refuses,
    /// leaving it dirty for the caller to drop (the contract
    /// [`edit::field_core`] already had, one level up). That nothing reaches
    /// disk is `run`'s doing — it returns before `ctx.flush` — and is pinned
    /// end to end in `opys/tests/cli.rs`.
    #[test]
    fn an_undeclared_field_refuses_the_whole_batch() {
        let (_dir, prj, mut store) = fixture();
        let fields = [("nope".to_string(), "1".to_string())];
        let err = core(&prj, &mut store, "TASK-0001", &claim(&fields), &[])
            .expect_err("an undeclared field is refused");
        let err = err.to_string();
        assert!(err.contains("would introduce 1 verify problem(s)"), "{err}");
        assert!(err.contains("unknown frontmatter field 'nope'"), "{err}");
    }

    /// `--field key=value` is parsed by [`crate::commands::parse_key_values`],
    /// the same helper `list --field` filters with: one `key=value` grammar and
    /// one message for it, whether the pair is a filter or a write.
    #[test]
    fn field_arguments_want_key_equals_value() {
        assert_eq!(
            crate::commands::parse_key_values(&[" claimed_by = agent-7 ".to_string()]).unwrap(),
            vec![("claimed_by".to_string(), "agent-7".to_string())]
        );
        let err = crate::commands::parse_key_values(&["claimed_by".to_string()]).unwrap_err();
        assert!(err.to_string().contains("key=value"), "{err}");
    }

    /// `--reason` is read only by the status step and `--section` only by the
    /// log step, so passing either without its step is refused rather than
    /// dropped — the misspelled section is the sharp case, since the identical
    /// typo beside `--log` is caught by name.
    #[test]
    fn a_modifier_without_the_step_that_reads_it_is_a_usage_error() {
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        let err = Apply {
            reason: Some("because"),
            set_fields: &fields,
            ..Apply::default()
        }
        .check()
        .expect_err("--reason without --status");
        assert!(err.to_string().contains("--reason"), "{err}");

        let err = Apply {
            section: Some("Nope"),
            set_fields: &fields,
            ..Apply::default()
        }
        .check()
        .expect_err("--section without --log");
        assert!(err.to_string().contains("--section"), "{err}");

        // And an invocation that asks for nothing, which `core` refuses too —
        // the node's typed action reaches the engine there and nowhere else.
        let err = Apply::default()
            .check()
            .expect_err("a batch that asks for nothing");
        assert!(err.to_string().contains("needs something to do"), "{err}");
    }

    /// Releasing a claim is one command even when the status being *left*
    /// requires the field being cleared, which is how a claim is made
    /// enforceable in the first place. The removal is judged at the status the
    /// document is moving to, so the `doing`-only `require_field` no longer
    /// applies by the time it runs.
    #[test]
    fn a_release_clears_a_field_the_source_status_requires() {
        let (_dir, prj, mut store) = fixture_with(GUARDED_CFG);
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        core(&prj, &mut store, "TASK-0001", &claim(&fields), &[]).expect("the claim");

        let removals = ["claimed_by".to_string()];
        let line = core(
            &prj,
            &mut store,
            "TASK-0001",
            &Apply {
                status: Some("todo"),
                remove_fields: &removals,
                log: Some("handing back"),
                ..Apply::default()
            },
            &[],
        )
        .expect("the release");
        assert_eq!(
            line,
            "TASK-0001 -> todo, claimed_by removed, logged to '## Progress'"
        );

        let dkey = store.dkey_of("TASK-0001").unwrap();
        let doc = store.doc(dkey).unwrap();
        assert_eq!(doc.frontmatter.status(), Some("todo"));
        assert!(doc.frontmatter.get("claimed_by").is_none());
    }

    /// A hand-on writes a value the status being **left** forbids and the
    /// status being *entered* allows, in one command — the mirror image of the
    /// release above, and the reason the verify gate brackets the batch instead
    /// of each field. Gated per field, `claimed_by=reviewer-3` would be judged
    /// while the document still sat in `doing`, whose `field_matches` rule
    /// demands an `agent-` prefix, and the hand-on would be refused by a rule
    /// the finished document is not subject to.
    #[test]
    fn a_hand_on_writes_a_value_only_the_target_status_allows() {
        let (_dir, prj, mut store) = fixture_with(HANDOFF_CFG);
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        core(&prj, &mut store, "TASK-0001", &claim(&fields), &[]).expect("the claim");

        let fields = [("claimed_by".to_string(), "reviewer-3".to_string())];
        let line = core(
            &prj,
            &mut store,
            "TASK-0001",
            &Apply {
                status: Some("review"),
                set_fields: &fields,
                log: Some("handing on to review"),
                ..Apply::default()
            },
            &[],
        )
        .expect("the hand-on");
        assert_eq!(
            line,
            "TASK-0001 -> review, claimed_by set, logged to '## Progress'"
        );

        let dkey = store.dkey_of("TASK-0001").unwrap();
        let doc = store.doc(dkey).unwrap();
        assert_eq!(doc.frontmatter.status(), Some("review"));
        assert_eq!(doc.frontmatter.get_str("claimed_by"), Some("reviewer-3"));
    }

    /// And the *target* status keeps its say over that same write: claiming
    /// with a value `doing` forbids is refused by the rule's own message.
    #[test]
    fn the_target_status_still_refuses_a_value_it_forbids() {
        let (_dir, prj, mut store) = fixture_with(HANDOFF_CFG);
        let fields = [("claimed_by".to_string(), "reviewer-3".to_string())];
        let err = core(&prj, &mut store, "TASK-0001", &claim(&fields), &[])
            .expect_err("doing forbids the value");
        let err = err.to_string();
        assert!(err.contains("must match /^agent-/"), "{err}");
    }

    /// The target status still gates a removal: clearing the field `doing`
    /// requires while moving *into* `doing` is refused by the verify gate, and
    /// the document keeps the status it had.
    #[test]
    fn the_target_status_still_refuses_a_removal_it_forbids() {
        let (_dir, prj, mut store) = fixture_with(GUARDED_CFG);
        let fields = [("claimed_by".to_string(), "agent-7".to_string())];
        let removals = ["claimed_by".to_string()];
        let err = core(
            &prj,
            &mut store,
            "TASK-0001",
            &Apply {
                status: Some("doing"),
                set_fields: &fields,
                remove_fields: &removals,
                ..Apply::default()
            },
            &[],
        )
        .expect_err("the target status requires the field being removed");
        let err = err.to_string();
        assert!(err.contains("field 'claimed_by' is required"), "{err}");
    }
}
