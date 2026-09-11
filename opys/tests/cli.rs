//! End-to-end tests driving the `opys` binary against temp project dirs.

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;

/// Build an `opys.toml` with the standard `feature` type (dir `features/`). The
/// `Test plan` section is a `checklist` carrying a grep-style check (a checked
/// item must end in a `mod::name` ref whose name is found under `src`/`tests`).
/// Extra `[fields.*]` tables are spliced in via `fields`.
fn opys_cfg(fields: &str) -> String {
    format!(
        "pad = 4\n\
[types.feature]\n\
prefix = \"FEAT\"\n\
dir = \"features\"\n\
statuses = [\"planned\", \"partial\", \"implemented\", \"wontfix\"]\n\
default_status = \"planned\"\n\
tags_required = true\n\
[types.feature.fields.spec]\ntype = \"string\"\n\
[types.feature.fields.wontfix_reason]\ntype = \"string\"\n\
{fields}\
[[types.feature.sections]]\nheading = \"Test plan\"\nkind = \"checklist\"\n\
[[types.feature.sections.checks]]\n\
pattern = '`(?P<ref>[^`]*::(?P<name>[^`]+))`'\n\
roots = [\"src\", \"tests\"]\n\
must_match = '${{name}}'\n\
scope = \"checked\"\n\
message = \"test reference `${{ref}}` not found\"\n\
[[types.feature.sections]]\nheading = \"Manual verification\"\nkind = \"structured\"\n\
structure = '''\n### @setup Setup\n  - +@items\n### @procedure Procedure\n  1. +@steps\n### @expect Expectations\n  - +@checks\n'''\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"wontfix\" }}\nrequire_field = \"wontfix_reason\"\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"implemented\" }}\nrequire_checked_section = \"Test plan\"\n{STATS_BLOCK}"
    )
}

/// The standard `[[stats]]` sections (status-by-type, tags, coverage), mirroring
/// the shipped default config. Appended to every `opys_cfg` project so the
/// `stats` command has sections to render. Each is a SQL query over the corpus
/// view (tables `docs`, `tags`, `sections`, `fields`).
const STATS_BLOCK: &str = r#"
[[stats]]
name = "Status by type"
sql = '''
SELECT type, status, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) /
             (SELECT COUNT(*) FROM docs d2 WHERE d2.type = d1.type)) AS pct
FROM docs d1
GROUP BY type, status
ORDER BY type, status
'''

[[stats]]
name = "Tags by key"
sql = '''
SELECT key, value, COUNT(DISTINCT doc_id) AS docs
FROM tags
WHERE value IS NOT NULL
GROUP BY key, value
ORDER BY key, value
'''

[[stats]]
name = "Section coverage"
sql = '''
SELECT heading, kind, SUM(items) AS items, SUM(unchecked) AS uncovered
FROM sections
GROUP BY heading, kind
ORDER BY heading
'''
"#;

/// The default feature-only config (plus the `ptyxis_ref` custom field used
/// across tests).
fn default_cfg() -> String {
    opys_cfg("[types.feature.fields.ptyxis_ref]\ntype = \"string\"\n")
}

/// Feature config with `enum`/`list` custom fields, for the enum/filter tests.
fn enum_cfg() -> String {
    opys_cfg(
        "[types.feature.fields.priority]\ntype = \"enum\"\nvalues = [\"low\", \"high\"]\n\
[types.feature.fields.area]\ntype = \"list\"\n",
    )
}

/// A temp project whose `opys.toml` is the default feature-only config.
fn project() -> TempDir {
    project_with(&default_cfg())
}

/// A temp project whose `opys.toml` is exactly `opys_toml`.
fn project_with(opys_toml: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml").write_str(opys_toml).unwrap();
    dir
}

/// Append the task/bug/chore types (dir `work-items/`) to the project's config.
fn enable_work_items(dir: &TempDir) {
    let path = dir.path().join("opys.toml");
    let mut s = std::fs::read_to_string(&path).unwrap();
    s.push_str(WORK_ITEM_TYPES);
    std::fs::write(&path, s).unwrap();
}

const WORK_ITEM_TYPES: &str = r#"
[types.task]
prefix = "TASK"
dir = "work-items"
statuses = ["todo", "in-progress", "blocked", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
requires_link = { to = "feature", min = 1 }
[types.task.fields.blocked_reason]
type = "string"
[[types.task.sections]]
heading = "Tasks"
kind = "checklist"
required = true
[[types.task.sections]]
heading = "Progress"
kind = "log"
required = true

[types.bug]
prefix = "BUG"
dir = "work-items"
statuses = ["todo", "in-progress", "blocked", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
requires_link = { to = "feature", min = 1 }
[types.bug.fields.blocked_reason]
type = "string"
[[types.bug.sections]]
heading = "Reproduction"
kind = "prose"
required = true
[[types.bug.sections]]
heading = "Tasks"
kind = "checklist"
required = true
[[types.bug.sections]]
heading = "Progress"
kind = "log"
required = true

[types.chore]
prefix = "CHORE"
dir = "work-items"
statuses = ["todo", "in-progress", "blocked", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
requires_link = { to = "feature", min = 1 }
[types.chore.fields.blocked_reason]
type = "string"
[[types.chore.sections]]
heading = "Tasks"
kind = "checklist"
required = true
[[types.chore.sections]]
heading = "Progress"
kind = "log"
required = true

[[rules]]
when = { status = "blocked" }
require_any = [{ field = "blocked_reason" }, { link = "blocked_by" }]

[[rules]]
when = { type = "task", status = "review" }
require_log_since_status = "Progress"

[[rules]]
when = { type = "bug", status = "review" }
require_log_since_status = "Progress"

[[rules]]
when = { type = "chore", status = "review" }
require_log_since_status = "Progress"
"#;

/// The instant every [`opys`] invocation is pinned to. Every `new` now writes a
/// `history` entry, whose key is UTC with microsecond precision — an unpinned
/// clock would make the byte expectations time-dependent. Tests that need time
/// to advance set their own, later, `OPYS_NOW` (a later `.env` wins).
const PINNED_NOW: &str = "2026-01-02T03:04:05Z";
/// The `history` key of a document created at [`PINNED_NOW`].
const PINNED_KEY: &str = "2026-01-02T03:04:05.000000Z";

/// The `history:` block for a hand-written fixture document: one entry at
/// `status`, keyed to [`PINNED_NOW`] (which is also what a fixture carrying
/// `created: "2026-01-02T03:04:05Z"` needs, since verify checks the two agree).
fn hist(status: &str) -> String {
    format!("history:\n  \"{PINNED_KEY}\": {status}\n")
}

fn opys(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("opys").unwrap();
    cmd.arg("--root").arg(dir.path());
    cmd.env("OPYS_NOW", PINNED_NOW);
    cmd
}

#[test]
fn init_bootstraps_and_prints_snippet() {
    let dir = TempDir::new().unwrap();
    opys(&dir)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Feature inventory"))
        .stdout(predicate::str::contains("opys verify"));
    dir.child("opys.toml").assert(predicate::path::exists());
}

#[test]
fn new_allocates_next_id_and_requires_tags() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "osc,tabs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001.md"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("# First"));

    opys(&dir)
        .args(["new", "--title", "Second", "--tags", "tabs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002.md"));

    opys(&dir)
        .args(["new", "--title", "Bad", "--tags", " , "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one tag"));
}

#[test]
fn new_writes_doc_and_no_index() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());
    // opys no longer generates an index.
    dir.child("opys/INDEX.md")
        .assert(predicate::path::missing());
}

#[test]
fn new_scaffolds_structured_section_from_mdprism() {
    let dir = project_with(
        "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
default_status = \"planned\"\n\
[[types.feature.sections]]\nheading = \"Manual verification\"\nkind = \"structured\"\nrequired = true\n\
structure = '''\n### @setup Setup\n  - +@items\n### @procedure Procedure\n  1. +@steps\n'''\n",
    );
    opys(&dir).args(["new", "--title", "X"]).assert().success();
    let body = std::fs::read_to_string(dir.child("opys/FEAT-0001.md").path()).unwrap();
    assert!(body.contains("## Manual verification"), "{body}");
    // mdprism scaffold emits the structure's sub-headings and a placeholder item.
    assert!(body.contains("### Setup"), "{body}");
    assert!(body.contains("### Procedure"), "{body}");
}

#[test]
fn flat_layout_is_the_default() {
    // With no `dir`/`status_dirs`/`[layout]`, documents live flat at the base.
    let dir = project_with(
        "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
default_status = \"planned\"\ntags_required = true\n",
    );
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    dir.child("opys/FEAT-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn base_relocates_inventory() {
    // The `base` key moves the inventory; opys.toml stays at the root.
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml")
        .write_str(&format!("base = \"inventory\"\n{}", default_cfg()))
        .unwrap();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    dir.child("inventory/features/FEAT-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn finds_config_by_searching_upward() {
    // opys.toml at the root is found from a nested working directory.
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    let mut cmd = Command::cargo_bin("opys").unwrap();
    cmd.arg("--root")
        .arg(dir.child("opys/features").path())
        .args(["list", "--format", "ids"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));
}

#[test]
fn set_status_implemented_requires_checked_item() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();

    opys(&dir)
        .args(["set-status", "FEAT-0001", "implemented"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "'## Test plan' needs at least one checked item",
        ));

    let path = dir.child("opys/features/FEAT-0001.md");
    let mut text = std::fs::read_to_string(path.path()).unwrap();
    text.push_str("\n## Test plan\n- [x] does a thing — `mod::test_thing`\n");
    std::fs::write(path.path(), text).unwrap();

    opys(&dir)
        .args(["set-status", "FEAT-0001", "implemented"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 -> implemented"));
}

#[test]
fn retire_deletes_and_reserves_id() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "dupe"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001: X"));

    opys(&dir)
        .args(["new", "--title", "Y", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002.md"));
}

#[test]
fn retired_ledger_is_sorted_by_number() {
    let dir = project();
    for t in ["A", "B", "C"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // Retire out of order; the ledger must come out ascending.
    opys(&dir)
        .args(["retire", "FEAT-0003", "--reason", "x"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "x"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.child("opys/_retired.md").path()).unwrap();
    let p1 = text.find("FEAT-0001").unwrap();
    let p3 = text.find("FEAT-0003").unwrap();
    assert!(p1 < p3, "retired ledger not sorted: {text:?}");
}

#[test]
fn verify_passes_clean_and_flags_violations() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [osc, tabs]\n{}---\n\n# Clean feature\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (1 documents)"));

    dir.child("opys/features/FEAT-0002.md")
        .write_str(
            "---\nid: FEAT-0002\nstatus: implemented\ntags: [Bad_Tag]\nbogus: 1\n---\n\n# Broken\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("must be lowercase kebab-case"))
        .stderr(predicate::str::contains(
            "unknown frontmatter field 'bogus'",
        ))
        .stderr(predicate::str::contains(
            "'## Test plan' needs at least one checked item",
        ));
}

#[test]
fn verify_accepts_namespaced_and_keyed_tags_but_rejects_duplicate_keys() {
    let dir = project();
    // Colon-namespaced and key=value tags are accepted.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [osc, area:parsing, priority=high]\n{}---\n\n# Clean\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();

    // The same key=value key twice on one document is rejected.
    dir.child("opys/features/FEAT-0002.md")
        .write_str(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [priority=high, priority=low]\n---\n\n# Dup\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tag key 'priority' is set more than once",
        ));
}

/// The write-time rules bite whatever seeds a document at a status. `new` no
/// longer takes one (ADR-0099 decision 5), so `import` — the one seam that does
/// — is where they are exercised.
#[test]
fn import_enforces_status_guards() {
    let dir = project();
    let jsonl = dir.child("seed.jsonl");
    jsonl
        .write_str("{\"title\": \"X\", \"tags\": [\"a\"], \"status\": \"implemented\"}\n")
        .unwrap();
    opys(&dir)
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "'## Test plan' needs at least one checked item",
        ));
    jsonl
        .write_str("{\"title\": \"X\", \"tags\": [\"a\"], \"status\": \"wontfix\"}\n")
        .unwrap();
    opys(&dir)
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "field 'wontfix_reason' is required",
        ));
}

/// `--status` is gone from `new` (ADR-0099 decision 5): a document starts at its
/// type's initial status, and `import` is the one seam that seeds elsewhere. An
/// unknown flag rather than a silently ignored one, so a stale script is told.
#[test]
fn new_rejects_the_removed_status_flag() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a", "--status", "wontfix"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument '--status'"));
}

/// `--reason` survives `--status`'s removal, but it can now only ever write
/// `<default_status>_reason`. Where the type does not declare that field the
/// write would be frontmatter the next `verify` rejects, so it is refused up
/// front — the same bargain `set-status` strikes.
#[test]
fn new_refuses_a_reason_with_nowhere_to_go() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a", "--reason", "because"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--reason has nowhere to go: 'planned_reason' is not a declared field",
        ));
    // …and it lands where the field *is* declared.
    let cfg = opys_cfg("[types.feature.fields.planned_reason]\ntype = \"string\"\n");
    let dir = project_with(&cfg);
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a", "--reason", "because"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(text.contains("planned_reason: because"), "{text}");
}

#[test]
fn init_does_not_overwrite_existing_config() {
    let dir = project();
    opys(&dir)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
}

#[test]
fn no_sync_still_writes_the_doc() {
    let dir = project();
    opys(&dir)
        .args(["--no-sync", "new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn new_coerces_custom_field_types() {
    let dir = project();
    opys(&dir)
        .args([
            "new",
            "--title",
            "X",
            "--tags",
            "a",
            "--field",
            "ptyxis_ref=src/x.c",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("ptyxis_ref: src/x.c"));
}

#[test]
fn set_status_wontfix_requires_reason() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "FEAT-0001", "wontfix"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "field 'wontfix_reason' is required",
        ));
    opys(&dir)
        .args([
            "set-status",
            "FEAT-0001",
            "wontfix",
            "--reason",
            "out of scope",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("wontfix_reason: out of scope"));
}

#[test]
fn tag_keeps_at_least_one() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["tag", "FEAT-0001", "--add", "b,c"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a, b, c"));
    opys(&dir)
        .args(["tag", "FEAT-0001", "--remove", "a,b,c"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one tag"));
}

#[test]
fn verify_checks_structured_section_against_mdprism() {
    let dir = project();
    // A "## Manual verification" present but missing the required sub-sections
    // declared by the section's mdprism `structure`.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}---\n\n# F\n\n## Manual verification\n- Looks right\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Manual verification"))
        .stderr(predicate::str::contains("setup"))
        .stderr(predicate::str::contains("procedure"));

    // A document matching the structure passes.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}---\n\n# F\n\n## Manual verification\n### Setup\n- a window\n\n### Procedure\n1. open it\n\n### Expectations\n- it renders\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn verify_ignores_prose_code_span_on_checked_item() {
    let dir = project_with(&opys_cfg(""));
    dir.child("src/lib.rs")
        .write_str("fn sftp_uri_rewrites_to_ssh() {}\n")
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: implemented\ntags: [ssh]\n{}---\n\n# Sftp\n\n## Test plan\n- [x] sftp:// rewrites to `ssh -t exec $SHELL` not a path — `lib.rs::sftp_uri_rewrites_to_ssh`\n",
            hist("implemented")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn verify_hints_at_unquoted_colon() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: wontfix\ntags: [a]\nwontfix_reason: MVP scope: containers\n---\n\n# F\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("quote the whole value"));
}

#[test]
fn import_bulk_creates_sequential_ids_and_syncs_once() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "Zero", "--tags", "a"])
        .assert()
        .success();
    let jsonl = dir.child("import.jsonl");
    jsonl
        .write_str(
            "{\"title\": \"One\", \"tags\": [\"osc\"], \"ptyxis_ref\": \"src/a.c\"}\n\
             {\"title\": \"Two\", \"tags\": [\"tabs\"], \"status\": \"partial\"}\n",
        )
        .unwrap();
    opys(&dir)
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .success()
        // The whole line, newline included: the dense path must not grow a
        // ` step M` suffix (that belongs to an interleaved block alone).
        .stdout(predicate::str::contains(
            "imported 2 feature document(s): FEAT-0002..FEAT-0003\n",
        ));
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("ptyxis_ref: src/a.c"));
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn import_creates_documents_of_any_type() {
    let cfg = format!(
        "{}\n[types.note]\nprefix = \"NOTE\"\ndir = \"notes\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n",
        default_cfg()
    );
    let dir = project_with(&cfg);
    let jsonl = dir.child("notes.jsonl");
    jsonl
        .write_str("{\"title\": \"A note\", \"tags\": [\"x\"]}\n")
        .unwrap();
    opys(&dir)
        .args(["import", "--type", "note", jsonl.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "imported 1 note document(s): NOTE-0001",
        ));
    dir.child("opys/notes/NOTE-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn import_is_transactional_on_bad_record() {
    let dir = project();
    let jsonl = dir.child("bad.jsonl");
    jsonl
        .write_str("{\"title\": \"Good\", \"tags\": [\"a\"]}\n{\"title\": \"NoTags\"}\n")
        .unwrap();
    opys(&dir)
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("import aborted"))
        .stderr(predicate::str::contains("line 2"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
}

/// BUG-0032: import rejects a terminal status (parity with `new`/`set-status`).
#[test]
fn import_rejects_terminal_status() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    let jsonl = dir.child("tasks.jsonl");
    jsonl
        .write_str(
            "{\"title\": \"Done thing\", \"status\": \"done\", \"references\": {\"FEAT-0001\": \"Alpha\"}}\n",
        )
        .unwrap();
    opys(&dir)
        .args(["import", "--type", "task", jsonl.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("terminal"));
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::path::missing());
}

/// BUG-0032: import scaffolds required sections a record omits, so it lands
/// verify-clean instead of red.
#[test]
fn import_scaffolds_missing_required_sections() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    let jsonl = dir.child("tasks.jsonl");
    jsonl
        .write_str("{\"title\": \"Wire it\", \"references\": {\"FEAT-0001\": \"Alpha\"}}\n")
        .unwrap();
    opys(&dir)
        .args(["import", "--type", "task", jsonl.path().to_str().unwrap()])
        .assert()
        .success();
    // The required Tasks + Progress sections were scaffolded, so verify is green.
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains("## Tasks"))
        .assert(predicate::str::contains("## Progress"));
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn verify_corpus_grep_check_resolves_test_refs() {
    // The default `Test plan` check greps the test name (the part after the
    // last `::`) under src/tests; a real name passes, a bogus one fails.
    let dir = project_with(&opys_cfg(""));
    dir.child("src/lib.rs")
        .write_str("fn real_test() {}\nfn another() {}\n")
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!("---\nid: FEAT-0001\nstatus: implemented\ntags: [a]\n{}---\n\n# Good\n\n## Test plan\n- [x] a — `mod::real_test`\n- [x] b — `src/lib.rs::another`\n", hist("implemented")))
        .unwrap();
    opys(&dir).arg("verify").assert().success();

    dir.child("opys/features/FEAT-0002.md")
        .write_str("---\nid: FEAT-0002\nstatus: implemented\ntags: [a]\n---\n\n# Bad\n\n## Test plan\n- [x] c — `mod::nope`\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "test reference `mod::nope` not found",
        ));
}

/// A feature config (modeled on the real FEAT-0207) with a `Code Pointers`
/// prose section validated by a `file`+`must_match` `scope = "all"` check, next
/// to the default `Test plan` corpus-grep check (with a multi-`::` ref).
const CODE_POINTERS_CFG: &str = r#"pad = 4
[types.feature]
prefix = "FEAT"
dir = "features"
statuses = ["planned", "implemented"]
default_status = "planned"
tags_required = true

[[types.feature.sections]]
heading = "Code Pointers"
kind = "prose"
[[types.feature.sections.checks]]
pattern = '`(?P<file>[^`]+\.rs)` — `(?P<sym>[^`]+)`'
file = "file"
roots = ["src"]
must_match = '${sym}'
scope = "all"
message = "`${sym}` not found in `${file}`"

[[types.feature.sections]]
heading = "Test plan"
kind = "checklist"
[[types.feature.sections.checks]]
pattern = '`(?P<ref>[^`]*::(?P<name>[^`]+))`'
roots = ["src", "tests"]
must_match = '${name}'
scope = "checked"
message = "test reference `${ref}` not found"
"#;

#[test]
fn verify_code_pointers_file_and_symbol_check() {
    let dir = project_with(CODE_POINTERS_CFG);
    dir.child("src/preferences/appearance.rs")
        .write_str("fn cursor_group() {}\nfn appearance_page() {}\n")
        .unwrap();
    // The multi-`::` test ref resolves to the last segment, found under tests/.
    dir.child("tests/e2e.rs")
        .write_str("fn ocr_cursor_group_exposes_shape_and_blink_combos() {}\n")
        .unwrap();

    // A doc whose Code Pointers line points at a real (file, symbol) pair, and
    // whose checked Test plan item carries a resolvable multi-`::` ref, passes.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            concat!(
                "---\nid: FEAT-0001\nstatus: implemented\ntags: [a]\n{}---\n\n",
                "# Cursor group\n\n",
                "## Code Pointers\n",
                "- `preferences/appearance.rs` — `cursor_group`: builds the group\n\n",
                "## Test plan\n",
                "- [x] exposes combos — `e2e::preferences::ocr::ocr_cursor_group_exposes_shape_and_blink_combos`\n",
            ),
            hist("implemented")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();

    // A wrong symbol fails with the check's custom message.
    dir.child("opys/features/FEAT-0002.md")
        .write_str(concat!(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [a]\n---\n\n",
            "# Bad symbol\n\n",
            "## Code Pointers\n",
            "- `preferences/appearance.rs` — `nonexistent_fn`: nope\n",
        ))
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "`nonexistent_fn` not found in `preferences/appearance.rs`",
        ));
}

#[test]
fn verify_code_pointers_missing_file_fails() {
    let dir = project_with(CODE_POINTERS_CFG);
    dir.child("src/preferences/appearance.rs")
        .write_str("fn cursor_group() {}\n")
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(concat!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n",
            "# Missing file\n\n",
            "## Code Pointers\n",
            "- `preferences/missing.rs` — `cursor_group`: points nowhere\n",
        ))
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "file 'preferences/missing.rs' not found",
        ));
}

/// A config whose `feature` type sends `archived` docs to a `_archived/`
/// segment. `layout` is the `[layout]` table body (empty for the flat default);
/// `feature_extra` adds lines to the `[types.feature]` table (e.g. a `dir`).
fn archived_cfg(layout: &str, feature_extra: &str) -> String {
    format!(
        "pad = 4\n{layout}\
[types.feature]\nprefix = \"FEAT\"\n{feature_extra}\
statuses = [\"planned\", \"archived\"]\ndefault_status = \"planned\"\n\
tags_required = true\nstatus_dirs = {{ archived = \"_archived\" }}\n\
[types.feature.fields.archived_reason]\ntype = \"string\"\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"archived\" }}\nrequire_field = \"archived_reason\"\n"
    )
}

#[test]
fn archived_status_relocates_into_subdir_and_back() {
    let dir = project_with(&archived_cfg("", ""));
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    dir.child("opys/FEAT-0001.md")
        .assert(predicate::path::exists());

    // Archiving moves the file into the status_dirs subdir; it stays in inventory.
    opys(&dir)
        .args(["set-status", "FEAT-0001", "archived", "--reason", "gone"])
        .assert()
        .success();
    dir.child("opys/_archived/FEAT-0001.md")
        .assert(predicate::path::exists());
    dir.child("opys/FEAT-0001.md")
        .assert(predicate::path::missing());
    opys(&dir).arg("verify").assert().success();
    opys(&dir)
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));

    // Moving off `archived` relocates it back to the base.
    opys(&dir)
        .args(["set-status", "FEAT-0001", "planned"])
        .assert()
        .success();
    dir.child("opys/FEAT-0001.md")
        .assert(predicate::path::exists());
    dir.child("opys/_archived/FEAT-0001.md")
        .assert(predicate::path::missing());
}

#[test]
fn layout_template_order_is_configurable() {
    // `{status}/{type}/{id}.md` groups by status first; empty status collapses.
    let cfg = archived_cfg(
        "[layout]\npath = \"{status}/{type}/{id}.md\"\n",
        "dir = \"features\"\n",
    );
    let dir = project_with(&cfg);
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());

    opys(&dir)
        .args(["set-status", "FEAT-0001", "archived", "--reason", "gone"])
        .assert()
        .success();
    dir.child("opys/_archived/features/FEAT-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn config_validate_flags_bad_layout() {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml")
        .write_str(
            "[layout]\npath = \"{type}/{status}\"\n\
[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
default_status = \"planned\"\nstatus_dirs = { ghost = \"x\" }\n",
        )
        .unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "layout.path must contain the {id} placeholder",
        ))
        .stderr(predicate::str::contains(
            "status_dirs key 'ghost' is not a status",
        ));
}

#[test]
fn sync_reconciles_and_writes_no_index() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [osc]\n---\n\n# One\n")
        .unwrap();
    // A stale index from an older version is cleaned up, not regenerated.
    dir.child("opys/INDEX.md").write_str("old\n").unwrap();
    opys(&dir).arg("sync").assert().success();
    dir.child("opys/INDEX.md")
        .assert(predicate::path::missing());
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());
}

#[test]
fn stats_reports_per_status_percentages() {
    let dir = project();
    for (n, status) in [
        ("0001", "planned"),
        ("0002", "planned"),
        ("0003", "implemented"),
    ] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: {status}\ntags: [a]\n---\n\n# F\n"
            ))
            .unwrap();
    }
    opys(&dir)
        .arg("stats")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Status by type"))
        // A markdown table; planned 2/3 ≈ 67%, implemented 1/3 ≈ 33%.
        .stdout(predicate::str::contains("| feature | planned | 2 | 67 |"))
        .stdout(predicate::str::contains(
            "| feature | implemented | 1 | 33 |",
        ));
}

#[test]
fn stats_reports_coverage_by_real_section_heading() {
    let dir = project();
    // Two features with the default `Test plan` checklist: 4 items total, 2
    // unchecked. Coverage is labeled by the real heading, not a hardcoded name.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# A\n\n## Test plan\n- [x] one `m::a`\n- [ ] two\n",
        )
        .unwrap();
    dir.child("opys/features/FEAT-0002.md")
        .write_str(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [a]\n---\n\n# B\n\n## Test plan\n- [x] ok `m::b`\n- [ ] todo\n",
        )
        .unwrap();
    opys(&dir)
        .arg("stats")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Section coverage"))
        // Real heading + kind, aggregated across both features (4 items, 2 uncovered).
        .stdout(predicate::str::contains(
            "| Test plan | checklist | 4 | 2 |",
        ))
        .stdout(predicate::str::contains("test-plan items").not());
}

#[test]
fn stats_groups_keyed_tags() {
    let dir = project();
    // Two docs share the `area` key with different values; one repeats a key
    // with two values (counts once for the key, distinct per value). `priority`
    // uses the `=` form. `osc` is a plain tag (excluded — it has no value).
    let docs = [
        ("0001", "[area:parsing, area:rendering, priority=high, osc]"),
        ("0002", "[area:parsing, osc]"),
        ("0003", "[priority=low]"),
    ];
    for (n, tags) in docs {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: planned\ntags: {tags}\n---\n\n# F\n"
            ))
            .unwrap();
    }
    opys(&dir)
        .arg("stats")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Tags by key"))
        // `area/parsing` on 2 docs (COUNT DISTINCT doc_id), `area/rendering` on 1.
        .stdout(predicate::str::contains("| area | parsing | 2 |"))
        .stdout(predicate::str::contains("| area | rendering | 1 |"))
        // `priority` via the `=` form, keyed.
        .stdout(predicate::str::contains("| priority | high | 1 |"))
        // Plain tag `osc` has no value, so the keyed tally omits it.
        .stdout(predicate::str::contains("osc").not());
}

/// A minimal project-defined `[[stats]]` section renders its SQL as a table.
#[test]
fn stats_renders_a_custom_section() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"Totals\"\n\
sql = '''SELECT COUNT(*) AS docs FROM docs'''\n";
    let dir = project_with(cfg);
    for n in ["0001", "0002"] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!("---\nid: FEAT-{n}\nstatus: planned\n---\n\n# F\n"))
            .unwrap();
    }
    opys(&dir)
        .arg("stats")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Totals"))
        .stdout(predicate::str::contains("| docs |"))
        .stdout(predicate::str::contains("| 2 |"));
}

/// A stat with a `template` renders each result row through it instead of a
/// table (`{column}` substitution), for single-row summaries and custom lists.
#[test]
fn stats_renders_a_row_template() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\",\"implemented\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"By status\"\n\
sql = '''SELECT status, COUNT(*) AS c FROM docs GROUP BY status ORDER BY status'''\n\
template = '''- {status}: {c}'''\n";
    let dir = project_with(cfg);
    for (n, status) in [
        ("0001", "planned"),
        ("0002", "planned"),
        ("0003", "implemented"),
    ] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: {status}\n---\n\n# F\n"
            ))
            .unwrap();
    }
    opys(&dir)
        .arg("stats")
        .assert()
        .success()
        .stdout(predicate::str::contains("## By status"))
        // Templated rows, not a markdown table.
        .stdout(predicate::str::contains("- implemented: 1"))
        .stdout(predicate::str::contains("- planned: 2"))
        .stdout(predicate::str::contains("| status |").not());
}

// --- opys query -------------------------------------------------------------

/// `opys query` runs a SELECT over the live corpus store and prints a table.
#[test]
fn query_selects_over_the_corpus() {
    let dir = project();
    for (n, status, tags) in [
        ("0001", "planned", "[osc, area:parsing]"),
        ("0002", "implemented", "[ui]"),
        ("0003", "planned", "[osc]"),
    ] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: {status}\ntags: {tags}\n---\n\n# F{n}\n\n\
                 ## Test plan\n- [x] a `m::t`\n- [ ] b\n"
            ))
            .unwrap();
    }
    opys(&dir)
        .args([
            "query",
            "SELECT status, COUNT(*) AS n FROM docs GROUP BY status ORDER BY status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("| status | n |"))
        .stdout(predicate::str::contains("| implemented | 1 |"))
        .stdout(predicate::str::contains("| planned | 2 |"));
    // Joins across tables work; tags are split into key/value.
    opys(&dir)
        .args([
            "query",
            "SELECT d.id FROM docs d JOIN tags t ON t.doc_id = d.id \
             WHERE t.tag = 'osc' ORDER BY d.id",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("| FEAT-0001 |"))
        .stdout(predicate::str::contains("| FEAT-0003 |"));
    // The derived sections projection is queryable.
    opys(&dir)
        .args([
            "query",
            "SELECT SUM(items) AS items, SUM(unchecked) AS open FROM sections \
             WHERE heading = 'Test plan'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("| 6 | 3 |"));
}

/// `opys query` rejects anything that is not a SELECT — before executing, so a
/// `DELETE …; SELECT 1` compound cannot mutate the store either. Exit code 2
/// (a usage error), never 1 (reserved for verify).
#[test]
fn query_rejects_non_select_statements() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n")
        .unwrap();
    for bad in [
        "DELETE FROM docs",
        "DROP TABLE docs",
        "UPDATE docs SET status = 'x'",
        "DELETE FROM docs; SELECT 1",
    ] {
        opys(&dir).args(["query", bad]).assert().code(2);
    }
    // And the files were never touched.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
}

/// `-` reads the SQL from stdin (composes with heredocs/pipes).
#[test]
fn query_reads_sql_from_stdin() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n")
        .unwrap();
    opys(&dir)
        .args(["query", "-"])
        .write_stdin("SELECT id FROM docs")
        .assert()
        .success()
        .stdout(predicate::str::contains("| FEAT-0001 |"));
}

/// `verify`/`config validate` reject a `template` referencing an unknown column
/// (checked against the query's columns, so it fails even with zero rows).
#[test]
fn verify_flags_a_template_unknown_column() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"BadTmpl\"\n\
sql = '''SELECT COUNT(*) AS n FROM docs'''\n\
template = '''{n} docs, {nope} missing'''\n";
    let dir = project_with(cfg);
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("stats 'BadTmpl'"))
        .stderr(predicate::str::contains("unknown column '{nope}'"));
}

/// `verify` reports a stat whose `sql` does not parse/execute.
#[test]
fn verify_flags_a_broken_stats_query() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"Bad\"\nsql = '''SELECT FROM WHERE'''\n";
    let dir = project_with(cfg);
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("stats 'Bad'"))
        .stderr(predicate::str::contains("query failed"));
}

/// `verify` reports a stat whose `sql` references an unknown column. GlueSQL
/// only resolves the column when there are rows to evaluate, so this surfaces
/// against the real (populated) corpus rather than the empty config-time check.
#[test]
fn verify_flags_a_stats_query_with_unknown_column() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"BadCol\"\nsql = '''SELECT nope FROM docs'''\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# F\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("stats 'BadCol'"))
        .stderr(predicate::str::contains("query failed"));
}

/// A stat whose `sql` is not a SELECT is rejected — stats are read-only views.
#[test]
fn verify_flags_a_non_select_stat() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[stats]]\nname = \"Mutate\"\nsql = '''DELETE FROM docs'''\n";
    let dir = project_with(cfg);
    for n in ["0001", "0002"] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!("---\nid: FEAT-{n}\nstatus: planned\n---\n\n# F\n"))
            .unwrap();
    }
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("stats 'Mutate'"))
        .stderr(predicate::str::contains("must end in a SELECT"));
}

#[test]
fn tags_lists_distinct_tags_and_keys() {
    let dir = project();
    let docs = [
        ("0001", "[area:parsing, priority=high, osc]"),
        ("0002", "[area:rendering, osc]"),
    ];
    for (n, tags) in docs {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: planned\ntags: {tags}\n---\n\n# F\n"
            ))
            .unwrap();
    }
    // Full tags: distinct, sorted, deduped (osc appears once).
    opys(&dir)
        .arg("tags")
        .assert()
        .success()
        .stdout("area:parsing\narea:rendering\nosc\npriority=high\n");
    // Keys collapse the value forms down to their key.
    opys(&dir)
        .args(["tags", "--keys"])
        .assert()
        .success()
        .stdout("area\nosc\npriority\n");
}

// --- Work items -----------------------------------------------------------

#[test]
fn new_rejects_unconfigured_type() {
    let dir = project();
    opys(&dir)
        .args(["new", "--type", "task", "--title", "X"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown type"));
}

#[test]
fn work_item_new_requires_existing_feature_link() {
    let dir = project();
    enable_work_items(&dir);
    // No feature exists yet.
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "X",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("FEAT-0001 does not exist"));
    // Empty link rejected (the type's requires_link rule).
    opys(&dir)
        .args(["new", "--type", "task", "--title", "X", "--features", " , "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("doc(s) of type 'feature'"));
}

#[test]
fn work_item_new_auto_links_feature_bidirectionally() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Auth login", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire login",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0002.md"));

    // Work item references the feature...
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains("FEAT-0001: Auth login"));
    // ...and the feature gained the reverse reference automatically.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("TASK-0002: Wire login"));
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (2 documents)"));
}

#[test]
fn ref_title_auto_syncs_on_rename() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Old name", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Work",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    let fpath = dir.child("opys/features/FEAT-0001.md");
    let text = std::fs::read_to_string(fpath.path())
        .unwrap()
        .replace("# Old name", "# New name");
    std::fs::write(fpath.path(), text).unwrap();
    opys(&dir).arg("sync").assert().success();

    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains("FEAT-0001: New name"));
}

#[test]
fn body_refs_are_linkified_idempotently() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Auth login", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Work",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    let wpath = dir.child("opys/work-items/TASK-0002.md");
    let mut text = std::fs::read_to_string(wpath.path()).unwrap();
    text.push_str("\nSee FEAT-0001 and `FEAT-0001` literal.\n");
    std::fs::write(wpath.path(), text).unwrap();

    opys(&dir).arg("sync").assert().success();
    let once = std::fs::read_to_string(wpath.path()).unwrap();
    assert!(
        once.contains("[FEAT-0001 — Auth login](../features/FEAT-0001.md) and `FEAT-0001` literal"),
        "linkify failed: {once}"
    );

    opys(&dir).arg("sync").assert().success();
    let twice = std::fs::read_to_string(wpath.path()).unwrap();
    assert_eq!(once, twice, "linkify is not idempotent");
}

#[test]
fn verify_flags_nested_markdown_links() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [osc]\n---\n\n# Corrupted\n\n\
             See [[FEAT-0002 — [Light] mode](FEAT-0002.md) — [Light] mode](FEAT-0002.md).\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: nested markdown link in body",
        ));
}

#[test]
fn linkify_is_idempotent_for_bracketed_titles() {
    // Regression: a linkified label containing `[…]` used to be re-matched on
    // every sync, nesting the link one level deeper each time.
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args([
            "new",
            "--title",
            "[Light] / [Dark] dual-variant support",
            "--tags",
            "core",
        ])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Work",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    let wpath = dir.child("opys/work-items/TASK-0002.md");
    let mut text = std::fs::read_to_string(wpath.path()).unwrap();
    text.push_str("\nSee FEAT-0001 for the variants.\n");
    std::fs::write(wpath.path(), text).unwrap();

    opys(&dir).arg("sync").assert().success();
    let once = std::fs::read_to_string(wpath.path()).unwrap();
    assert!(
        once.contains(
            "[FEAT-0001 — [Light] / [Dark] dual-variant support](../features/FEAT-0001.md)"
        ),
        "linkify failed: {once}"
    );
    assert!(!once.contains("[["), "nested link after first sync: {once}");

    opys(&dir).arg("sync").assert().success();
    let twice = std::fs::read_to_string(wpath.path()).unwrap();
    assert_eq!(once, twice, "linkify is not idempotent");
}

#[test]
fn work_item_close_requires_all_tasks_checked() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "W",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    opys(&dir)
        .args(["close", "TASK-0002"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unchecked items remain"));
    opys(&dir)
        .args(["close", "TASK-0002", "--force"])
        .assert()
        .success();
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::path::missing());
}

#[test]
fn work_item_close_strikes_ref_reserves_id_and_cleanup_strips() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "First",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["close", "TASK-0002", "--force"])
        .assert()
        .success();

    // The reference becomes a struck-through tombstone.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("TASK-0002: ~~First~~"));
    // verify accepts the struck (closed) reference.
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (1 documents)"));
    // The ID is reserved — next continues the global sequence at TASK-0003.
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Second",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003.md"));

    // cleanup removes the struck tombstone.
    opys(&dir).args(["cleanup"]).assert().success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("~~First~~").not());
}

#[test]
fn verify_fails_on_dangling_unstruck_reference() {
    let dir = project();
    enable_work_items(&dir);
    // A reference to a non-existent, non-struck id is an error.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nreferences:\n  TASK-0099: Ghost\n---\n\n# F\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "reference 'TASK-0099' does not resolve",
        ));

    // The same reference, struck through, is an accepted tombstone.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}references:\n  TASK-0099: ~~Ghost~~\n---\n\n# F\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn work_item_set_status_rejects_done_and_requires_blocked_reason() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "W",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    opys(&dir)
        .args(["set-status", "TASK-0002", "done"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("opys close"));
    opys(&dir)
        .args(["set-status", "TASK-0002", "blocked"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires one of"));
    opys(&dir)
        .args([
            "set-status",
            "TASK-0002",
            "blocked",
            "--reason",
            "waiting on upstream",
        ])
        .assert()
        .success();
}

#[test]
fn work_item_verify_flags_missing_required_section() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    // Hand-written work item missing the ## Progress section (TASK-0002 so its
    // number doesn't collide with FEAT-0001 under the global id sequence).
    dir.child("opys/work-items/TASK-0002.md")
        .write_str(
            "---\nid: TASK-0002\nstatus: todo\nreferences:\n  FEAT-0001: F\n---\n\n# W\n\n## Tasks\n- [ ] do it\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "missing required '## Progress' section",
        ));
}

#[test]
fn verify_ignores_work_items_when_not_configured() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success();
    // No work-items config: the success line never mentions work items.
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (1 documents)"))
        .stdout(predicate::str::contains("work items").not());
}

#[test]
fn agent_rules_generates_editor_files() {
    let dir = TempDir::new().unwrap();
    // A single tool writes its conventional file with host frontmatter + body.
    opys(&dir)
        .args(["agent-rules", "--tool", "cursor"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".cursor/rules/opys.mdc"));
    dir.child(".cursor/rules/opys.mdc")
        .assert(predicate::str::contains("globs: opys/**"))
        .assert(predicate::str::contains("# opys document inventory"));

    // --stdout prints instead of writing; --tool all is rejected with --stdout.
    opys(&dir)
        .args(["agent-rules", "--tool", "kiro", "--stdout"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# opys document inventory"));
    opys(&dir)
        .args(["agent-rules", "--tool", "all", "--stdout"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("single --tool"));

    // `all` writes every editor's file.
    opys(&dir)
        .args(["agent-rules", "--tool", "all"])
        .assert()
        .success();
    dir.child(".github/instructions/opys.instructions.md")
        .assert(predicate::str::contains("applyTo: \"opys/**\""));
    dir.child(".clinerules/opys.md")
        .assert(predicate::path::exists());
}

#[test]
fn enum_field_validates_against_declared_values() {
    let dir = project_with(&enum_cfg());
    // A value in the set passes verify.
    opys(&dir)
        .args([
            "new",
            "--title",
            "Ok",
            "--tags",
            "a",
            "--field",
            "priority=high",
        ])
        .assert()
        .success();
    opys(&dir).arg("verify").assert().success();

    // An out-of-set value is written (custom fields are verify-checked, like
    // every other field type) and rejected by verify with a precise message.
    opys(&dir)
        .args([
            "new",
            "--title",
            "Bad",
            "--tags",
            "b",
            "--field",
            "priority=urgent",
        ])
        .assert()
        .success();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "field 'priority' value 'urgent' is not one of: low, high",
        ));
}

#[test]
fn enum_field_requires_declared_values() {
    let dir = project_with(&opys_cfg("[types.feature.fields.bad]\ntype = \"enum\"\n"));
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "field 'bad' is enum but declares no values",
        ));
}

#[test]
fn list_filters_by_custom_field() {
    let dir = project_with(&enum_cfg());
    opys(&dir)
        .args([
            "new",
            "--title",
            "Alpha",
            "--tags",
            "a",
            "--field",
            "priority=high",
            "--field",
            "area=[ui, cli]",
        ])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--title",
            "Beta",
            "--tags",
            "b",
            "--field",
            "priority=low",
            "--field",
            "area=[cli]",
        ])
        .assert()
        .success();

    // Scalar equality.
    opys(&dir)
        .args(["list", "--field", "priority=high", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"))
        .stdout(predicate::str::contains("FEAT-0002").not());
    // List membership.
    opys(&dir)
        .args(["list", "--field", "area=ui", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"))
        .stdout(predicate::str::contains("FEAT-0002").not());
    // Multiple filters are ANDed.
    opys(&dir)
        .args([
            "list",
            "--field",
            "area=cli",
            "--field",
            "priority=low",
            "--format",
            "ids",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002"))
        .stdout(predicate::str::contains("FEAT-0001").not());
    // A non-matching value yields nothing.
    opys(&dir)
        .args(["list", "--field", "priority=none", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-").not());
    // A malformed filter is a usage error.
    opys(&dir)
        .args(["list", "--field", "priority"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("expects key=value"));
}

#[test]
fn list_filters_by_tag_key() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "A", "--tags", "area:parsing"])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--title", "B", "--tags", "area=cli"])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--title", "C", "--tags", "osc"])
        .assert()
        .success();

    // A bare key matches both the colon and the equals forms.
    opys(&dir)
        .args(["list", "--tag", "area", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"))
        .stdout(predicate::str::contains("FEAT-0002"))
        .stdout(predicate::str::contains("FEAT-0003").not());
    // The exact value still matches just that one.
    opys(&dir)
        .args(["list", "--tag", "area:parsing", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"))
        .stdout(predicate::str::contains("FEAT-0002").not());
}

#[test]
fn work_item_list_filters_by_custom_field() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success();
    for (title, area) in [("W1", "ui"), ("W2", "cli")] {
        opys(&dir)
            .args([
                "new",
                "--type",
                "task",
                "--title",
                title,
                "--features",
                "FEAT-0001",
                "--field",
                &format!("area={area}"),
            ])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["list", "--field", "area=ui", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0002"))
        .stdout(predicate::str::contains("TASK-0003").not());
}

/// `block` writes the two relation maps and nothing else (ADR-0099 decision 3).
/// The work-item fixture still *declares* a `blocked` status — a project may —
/// and the task stays at `todo` regardless: there is no auto-set left to fire.
#[test]
fn block_links_both_directions_without_touching_status() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--title", "Beta", "--tags", "b"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();

    opys(&dir)
        .args(["block", "TASK-0003", "--by", "FEAT-0002"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003 blocked by FEAT-0002"));

    // The blocked work item gained `blocked_by` and kept the status it had.
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::str::contains("status: todo"))
        .assert(predicate::str::contains("FEAT-0002: Beta"))
        .assert(predicate::str::contains("blocked_from").not());
    // The blocker gained the reverse `blocks` edge.
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("blocks:"))
        .assert(predicate::str::contains("TASK-0003: Wire"));
    opys(&dir).arg("verify").assert().success();
    // Blocked is derived from that entry: this fixture's `feature` type declares
    // no `releases_blocks`, so a live `FEAT-0002` still holds.
    opys(&dir)
        .args(["list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("TASK-0003\n");

    // An item cannot block itself.
    opys(&dir)
        .args(["block", "FEAT-0001", "--by", "FEAT-0001"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot block itself"));
}

/// `unblock` is the mirror: it removes the entry from both sides and leaves the
/// status where it found it. The document stops being blocked because the entry
/// is gone, not because anything set a status.
#[test]
fn unblock_removes_the_link_from_both_sides() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--title", "Beta", "--tags", "b"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["block", "TASK-0003", "--by", "FEAT-0002"])
        .assert()
        .success();

    opys(&dir)
        .args(["unblock", "TASK-0003", "--by", "FEAT-0002"])
        .assert()
        .success();
    // Both sides cleared; the status is the one the task was created at.
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::str::contains("status: todo"))
        .assert(predicate::str::contains("blocked_by").not())
        .assert(predicate::str::contains("blocked_from").not());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("blocks").not());
    opys(&dir).arg("verify").assert().success();
    opys(&dir)
        .args(["list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("");

    // Unblocking an edge that does not exist is an error.
    opys(&dir)
        .args(["unblock", "TASK-0003", "--by", "FEAT-0002"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no blocker"));
}

/// BUG-0036: `--reason` is rejected when `<status>_reason` is not a declared
/// field, instead of writing an unknown field that the next verify rejects.
#[test]
fn set_status_reason_requires_declared_field() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "A", "--tags", "a"])
        .assert()
        .success();
    // `partial_reason` is not declared -> reject, write nothing.
    opys(&dir)
        .args(["set-status", "FEAT-0001", "partial", "--reason", "half"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("partial_reason"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
    opys(&dir).arg("verify").assert().success();
    // `wontfix_reason` IS declared -> accepted, and the corpus stays green.
    opys(&dir)
        .args([
            "set-status",
            "FEAT-0001",
            "wontfix",
            "--reason",
            "out of scope",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: wontfix"))
        .assert(predicate::str::contains("wontfix_reason"));
    opys(&dir).arg("verify").assert().success();
}

/// BUG-0034: agent-rules resolves the project root (like every other command),
/// so running from a nested subdir writes at the root, not the cwd.
#[test]
fn agent_rules_writes_to_project_root_from_subdir() {
    let dir = project();
    let deep = dir.path().join("sub/deep");
    std::fs::create_dir_all(&deep).unwrap();
    Command::cargo_bin("opys")
        .unwrap()
        .arg("--root")
        .arg(&deep)
        .args(["agent-rules", "--tool", "cursor"])
        .assert()
        .success();
    // Lands at the project root, not under sub/deep/.
    dir.child(".cursor/rules/opys.mdc")
        .assert(predicate::path::exists());
    assert!(!deep.join(".cursor/rules/opys.mdc").exists());
}

#[test]
fn close_strikes_blocker_and_reserves_id() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    // The feature is blocked by the work item.
    opys(&dir)
        .args(["block", "FEAT-0001", "--by", "TASK-0002"])
        .assert()
        .success();

    // Closing the blocker strikes its blocked_by entry into a tombstone.
    opys(&dir)
        .args(["close", "TASK-0002", "--force"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("TASK-0002: ~~Wire~~"));
    opys(&dir).arg("verify").assert().success();

    // The struck blocker reserves the id: the next work item is TASK-0003.
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Next",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003.md"));

    // Unblocking a struck (closed) blocker is tolerated and clears the blocker
    // tombstone (the separate `references` tombstone is untouched — that is
    // `cleanup`'s job).
    opys(&dir)
        .args(["unblock", "FEAT-0001", "--by", "TASK-0002"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("blocked_by").not());
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn wi_new_type_selects_prefix_and_sections() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success();

    // Default type is task (and the work item continues the global sequence).
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "General",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0002.md"));

    // --type bug → BUG- prefix and a scaffolded ## Reproduction section.
    opys(&dir)
        .args([
            "new",
            "--type",
            "bug",
            "--title",
            "Crash",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("BUG-0003.md"));
    dir.child("opys/work-items/BUG-0003.md")
        .assert(predicate::str::contains("## Reproduction"))
        .assert(predicate::str::contains("## Tasks"))
        .assert(predicate::str::contains("## Progress"));

    // --type chore → CHORE-.
    opys(&dir)
        .args([
            "new",
            "--type",
            "chore",
            "--title",
            "Tidy",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("CHORE-0004.md"));

    opys(&dir).arg("verify").assert().success();
}

#[test]
fn wi_bug_requires_reproduction_section() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success();
    // Hand-written bug missing the per-type ## Reproduction section (BUG-0002 so
    // its number doesn't collide with FEAT-0001 under the global id sequence).
    dir.child("opys/work-items/BUG-0002.md")
        .write_str(
            "---\nid: BUG-0002\nstatus: todo\nreferences:\n  FEAT-0001: F\n---\n\n# B\n\n## Tasks\n- [ ] x\n\n## Progress\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "missing required '## Reproduction' section",
        ));
}

#[test]
fn ids_share_one_global_increasing_sequence() {
    let dir = project();
    enable_work_items(&dir);
    // Features and every work-item type draw from one increasing sequence, so a
    // number never repeats across prefixes.
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001.md"));
    for (ty, want) in [
        ("bug", "BUG-0002"),
        ("task", "TASK-0003"),
        ("chore", "CHORE-0004"),
    ] {
        opys(&dir)
            .args([
                "new",
                "--type",
                ty,
                "--title",
                "w",
                "--features",
                "FEAT-0001",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(format!("{want}.md")));
    }
    // A new feature also continues the same sequence.
    opys(&dir)
        .args(["new", "--title", "G", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0005.md"));
    // Closing reserves the number globally; the next id skips past it.
    opys(&dir)
        .args(["close", "BUG-0002", "--force"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "bug",
            "--title",
            "w2",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("BUG-0006.md"));
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn wi_list_filters_by_type() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "bug",
            "--title",
            "B",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "T",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["list", "--type", "bug", "--format", "ids"])
        .assert()
        .success()
        .stdout(predicate::str::contains("BUG-0002"))
        .stdout(predicate::str::contains("TASK-0003").not());
}

#[test]
fn body_links_work_item_type_prefixes() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Auth", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "bug",
            "--title",
            "Crash",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    // A bare BUG-0002 mention in the feature body is linkified on sync.
    let fpath = dir.child("opys/features/FEAT-0001.md");
    let mut text = std::fs::read_to_string(fpath.path()).unwrap();
    text.push_str("\nSee BUG-0002 for the regression.\n");
    std::fs::write(fpath.path(), text).unwrap();
    opys(&dir).arg("sync").assert().success();
    let out = std::fs::read_to_string(fpath.path()).unwrap();
    assert!(
        out.contains("[BUG-0002 — Crash](../work-items/BUG-0002.md)"),
        "bug mention not linkified: {out}"
    );
}

#[test]
fn config_init_generates_opys_toml() {
    let dir = TempDir::new().unwrap();
    opys(&dir)
        .args(["config", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opys.toml"));
    dir.child("opys.toml")
        .assert(predicate::str::contains("[types.feature]"))
        .assert(predicate::str::contains("prefix = \"FEAT\""))
        .assert(predicate::str::contains(
            "[[types.feature.sections.checks]]",
        ))
        .assert(predicate::str::contains("[[rules]]"));

    // Re-running refuses to overwrite.
    opys(&dir)
        .args(["config", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
}

#[test]
fn config_validate_accepts_the_generated_default() {
    let dir = TempDir::new().unwrap();
    opys(&dir).args(["config", "init"]).assert().success();
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config: OK (4 types"));
}

#[test]
fn config_validate_names_the_id_slot_under_a_modulus() {
    // The slot this process allocates in is invisible until a document is
    // created, so `config validate` is where an orchestrator reads it back.
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml").write_str(&modulus_cfg(4)).unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .env("OPYS_ID_SLOT", "2")
        .assert()
        .success()
        .stdout(predicate::str::contains("config: OK"))
        .stdout(predicate::str::contains(
            "ids: slot 2 of 4 (from OPYS_ID_SLOT)",
        ));
    // Without the variable the line still names the slot, hashed from the
    // machine, and says where it came from.
    opys(&dir)
        .args(["config", "validate"])
        .env_remove("OPYS_ID_SLOT")
        .env("USER", "agent")
        .env("HOSTNAME", "box")
        .assert()
        .success()
        .stdout(predicate::str::contains("(from the hash of agent@box)"));
    // A dense project says nothing new.
    dir.child("opys.toml").write_str(&default_cfg()).unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .env("OPYS_ID_SLOT", "2")
        .assert()
        .success()
        .stdout(predicate::str::contains("ids: slot").not());
}

#[test]
fn config_validate_refuses_an_unusable_slot_before_reporting_ok() {
    // The config is fine and the environment is not: that is the allocating
    // commands' usage error (exit 2), and `OK` must not be printed first.
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml").write_str(&modulus_cfg(4)).unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .env("OPYS_ID_SLOT", "9")
        .assert()
        .failure()
        .code(2)
        .stdout(predicate::str::contains("config: OK").not())
        .stderr(predicate::str::contains(
            "OPYS_ID_SLOT=9 is not a slot below the [ids] modulus 4",
        ));
}

#[test]
fn config_validate_flags_a_broken_config() {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml")
        .write_str(
            "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\ndefault_status = \"nope\"\n\n[types.bug]\nprefix = \"FEAT\"\nstatuses = [\"todo\"]\ndefault_status = \"todo\"\n\n[[rules]]\nwhen = { type = \"ghost\" }\nrequire_field = \"x\"\n",
        )
        .unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "default_status 'nope' not in statuses",
        ))
        .stderr(predicate::str::contains("already used by type"))
        .stderr(predicate::str::contains(
            "when.type 'ghost' is not a defined type",
        ));
}

#[test]
fn config_validate_requires_the_file() {
    let dir = TempDir::new().unwrap();
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("run `opys config init`"));
}

#[test]
fn verify_enforces_opys_rules_when_present() {
    // The generated default config has an `archived ⇒ archived_reason` rule.
    let dir = TempDir::new().unwrap();
    opys(&dir).args(["config", "init"]).assert().success();

    // An archived feature with no archived_reason — the opys.toml rule fires.
    // (The default opys.toml sends archived features to `opys/_archived/`.)
    dir.child("opys/_archived/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: archived\ntags: [a]\n---\n\n# F\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: field 'archived_reason' is required",
        ));

    // Supplying the reason satisfies both the legacy checks and the rule.
    dir.child("opys/_archived/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: archived\ntags: [a]\narchived_reason: removed in v3\n{}---\n\n# F\n",
            hist("archived")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn verify_surfaces_a_broken_opys_config() {
    let dir = project();
    dir.child("opys.toml")
        .write_str("[types.x]\nprefix = \"lower\"\nstatuses = [\"a\"]\ndefault_status = \"a\"\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("opys.toml:"))
        .stderr(predicate::str::contains("must match"));
}

#[test]
fn verify_enforces_field_pattern_from_opys_config() {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml")
        .write_str(
            "[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\nstatuses = [\"planned\"]\ndefault_status = \"planned\"\ntags_required = true\n\n[types.feature.fields.ticket]\ntype = \"string\"\npattern = '^JIRA-[0-9]+$'\n",
        )
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nticket: nope\n---\n\n# F\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: field 'ticket' must match",
        ));

    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}ticket: JIRA-42\n---\n\n# F\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

#[test]
fn new_stamps_created_and_updated_and_set_status_bumps_updated() {
    let dir = project();
    // `OPYS_NOW` pins the clock so the stamped timestamps are deterministic.
    opys(&dir)
        .env("OPYS_NOW", "2026-06-16T14:30:00Z")
        .args(["new", "--title", "First", "--tags", "a"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(
            "created: \"2026-06-16T14:30:00Z\"",
        ))
        .assert(predicate::str::contains(
            "updated: \"2026-06-16T14:30:00Z\"",
        ));

    // A later status change refreshes `updated` but leaves `created` untouched.
    opys(&dir)
        .env("OPYS_NOW", "2026-07-01T09:00:00Z")
        .args(["set-status", "FEAT-0001", "partial"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(
            "created: \"2026-06-16T14:30:00Z\"",
        ))
        .assert(predicate::str::contains(
            "updated: \"2026-07-01T09:00:00Z\"",
        ));
}

/// `new` writes exactly one history entry, at the type's default status, keyed
/// to the same instant as `created` (verify checks the two agree).
#[test]
fn new_writes_one_history_entry_at_the_default_status() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "a"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(
        text.contains(&format!("history:\n  \"{PINNED_KEY}\": planned\n")),
        "{text}"
    );
    assert_eq!(text.matches("\": planned").count(), 1, "{text}");
    assert!(
        text.contains(&format!("created: \"{PINNED_NOW}\"")),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// `set-status` appends; two transitions under one pinned clock land one
/// microsecond apart, in order, and the last entry is the current status.
#[test]
fn set_status_appends_history_bumping_a_same_instant_key() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "FEAT-0001", "partial"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "FEAT-0001", "planned"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    // One pinned instant, three writes: 0µs, +1µs, +2µs — distinct and ordered.
    assert!(
        text.contains(
            "history:\n  \
             \"2026-01-02T03:04:05.000000Z\": planned\n  \
             \"2026-01-02T03:04:05.000001Z\": partial\n  \
             \"2026-01-02T03:04:05.000002Z\": planned\n"
        ),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// The key is UTC even when the clock is not: an `OPYS_NOW` carrying an offset
/// is converted, and `created` (which keeps the offset) still agrees with it.
#[test]
fn a_clock_with_an_offset_still_yields_a_utc_history_key() {
    let dir = project();
    opys(&dir)
        .env("OPYS_NOW", "2026-06-16T10:30:00-04:00")
        .args(["new", "--title", "First", "--tags", "a"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(
        text.contains("created: \"2026-06-16T10:30:00-04:00\""),
        "{text}"
    );
    assert!(
        text.contains("history:\n  \"2026-06-16T14:30:00.000000Z\": planned\n"),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// `import` seeds one entry at the status the record was given.
#[test]
fn import_seeds_one_history_entry_at_the_given_status() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    let jsonl = dir.child("tasks.jsonl");
    jsonl
        .write_str(
            "{\"title\": \"Wire it\", \"status\": \"in-progress\", \
             \"references\": {\"FEAT-0001\": \"Alpha\"}}\n",
        )
        .unwrap();
    opys(&dir)
        .args(["import", "--type", "task", jsonl.path().to_str().unwrap()])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/work-items/TASK-0002.md")).unwrap();
    assert!(
        text.contains(&format!("history:\n  \"{PINNED_KEY}\": in-progress\n")),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// Blocked is derived, so `block`/`unblock` are not status changes and neither
/// appends a history entry (ADR-0099 decision 3). The map stays a faithful
/// record of the lifecycle precisely because only real transitions reach it —
/// and the task ends where it started even though its type declares `blocked`.
#[test]
fn block_and_unblock_change_neither_status_nor_history() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["block", "TASK-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    let blocked = std::fs::read_to_string(dir.path().join("opys/work-items/TASK-0002.md")).unwrap();
    assert!(blocked.contains("status: todo"), "{blocked}");
    assert!(
        blocked.contains(&format!("history:\n  \"{PINNED_KEY}\": todo\n")),
        "{blocked}"
    );
    // One entry, the one `new` wrote: `block` added none.
    assert_eq!(blocked.matches("\": ").count(), 1, "{blocked}");

    opys(&dir)
        .args(["unblock", "TASK-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/work-items/TASK-0002.md")).unwrap();
    assert!(text.contains("status: todo"), "{text}");
    assert!(
        text.contains(&format!("history:\n  \"{PINNED_KEY}\": todo\n")),
        "{text}"
    );
    assert_eq!(text.matches("\": ").count(), 1, "{text}");
    opys(&dir).arg("verify").assert().success();
}

/// A document written before `history` existed keeps its migration handle. A
/// status change on it seeds the *first* entry from `created`, at the status
/// being left, so the transition lands second and the corpus stays green.
/// Appending straight onto it would key the first entry to the wall clock,
/// contradicting `created` — a red corpus the migration could neither find (it
/// greps `missing history`) nor fix, since nothing hand-edits the map.
#[test]
fn a_status_change_seeds_history_on_a_document_that_predates_the_map() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n\
             created: \"2024-01-05T10:00:00-04:00\"\n\
             updated: \"2024-01-05T10:00:00-04:00\"\n---\n\n# A\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("FEAT-0001: missing history"));

    opys(&dir)
        .args(["set-status", "FEAT-0001", "partial"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(
        text.contains(
            "history:\n  \
             \"2024-01-05T14:00:00.000000Z\": planned\n  \
             \"2026-01-02T03:04:05.000000Z\": partial\n"
        ),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// A migrated document may carry `history` and no `created` (the procedure says
/// so). Both `touch` paths — `Store::touch` behind `tag`, and the frontmatter
/// `touch` behind `set-status` — then backfill `created` from the first history
/// key, never from the wall clock: `verify` requires the two to denote the same
/// second, so stamping "now" would turn a clean corpus red on a successful write.
#[test]
fn a_write_backfills_created_from_the_first_history_key() {
    let dir = project();
    for id in ["FEAT-0001", "FEAT-0002"] {
        dir.child(format!("opys/features/{id}.md"))
            .write_str(&format!(
                "---\nid: {id}\nstatus: planned\ntags: [core]\nhistory:\n  \
                 \"2026-01-01T00:00:00.000000Z\": planned\n---\n\n# Doc {id_num}\n",
                id_num = &id[5..]
            ))
            .unwrap();
    }
    opys(&dir).arg("verify").assert().success();

    opys(&dir)
        .args(["tag", "FEAT-0001", "--add", "x"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "FEAT-0002", "partial"])
        .assert()
        .success();
    opys(&dir).arg("verify").assert().success();
    for id in ["FEAT-0001", "FEAT-0002"] {
        let text =
            std::fs::read_to_string(dir.path().join(format!("opys/features/{id}.md"))).unwrap();
        assert!(
            text.contains("created: \"2026-01-01T00:00:00.000000Z\""),
            "{text}"
        );
    }
}

/// The four history checks, each on its own document.
#[test]
fn verify_flags_a_botched_history_map() {
    let dir = project();
    // (a) no history at all.
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# A\n")
        .unwrap();
    // (b) the last entry is not the current status.
    dir.child("opys/features/FEAT-0002.md")
        .write_str(&format!(
            "---\nid: FEAT-0002\nstatus: partial\ntags: [a]\n{}---\n\n# B\n",
            hist("planned")
        ))
        .unwrap();
    // (c) keys that do not strictly increase, and a key that is not UTC.
    dir.child("opys/features/FEAT-0003.md")
        .write_str(
            "---\nid: FEAT-0003\nstatus: partial\ntags: [a]\nhistory:\n  \
             \"2026-01-02T03:04:05.000000Z\": planned\n  \
             \"2026-01-01T00:00:00.000000Z\": partial\n---\n\n# C\n",
        )
        .unwrap();
    dir.child("opys/features/FEAT-0004.md")
        .write_str(
            "---\nid: FEAT-0004\nstatus: planned\ntags: [a]\nhistory:\n  \
             \"2026-01-02T03:04:05.000000+00:00\": planned\n---\n\n# D\n",
        )
        .unwrap();
    // …and one that *is* UTC but not fixed width. Accepting it would be a trap:
    // the serializer sorts the block lexically-by-instant, so a narrower key
    // beside a fractional one in the same second would be reordered by the next
    // unrelated write, turning a blessed corpus red.
    dir.child("opys/features/FEAT-0006.md")
        .write_str(
            "---\nid: FEAT-0006\nstatus: planned\ntags: [a]\nhistory:\n  \
             \"2026-01-02T03:04:05Z\": planned\n---\n\n# F\n",
        )
        .unwrap();
    // …and a map whose *entries* are wrong-typed. One good pair beside one bad
    // one used to read as a valid map (`entries` filter-mapped the bad pair
    // away) while the store kept the whole value opaque — verify blessed it and
    // the next `set-status` destroyed it.
    dir.child("opys/features/FEAT-0007.md")
        .write_str(
            "---\nid: FEAT-0007\nstatus: planned\ntags: [a]\nhistory:\n  \
             \"2026-01-02T03:04:05.000000Z\": planned\n  \
             \"2026-01-03T00:00:00.000000Z\": 5\n---\n\n# G\n",
        )
        .unwrap();
    dir.child("opys/features/FEAT-0008.md")
        .write_str(
            "---\nid: FEAT-0008\nstatus: planned\ntags: [a]\nhistory:\n  \
             5: planned\n---\n\n# H\n",
        )
        .unwrap();
    // …and a value that is not a mapping at all but still carries entries: the
    // YAML list a hand-written migration produces. It used to be reported as
    // `missing history` — telling the operator to *add* a block that is already
    // there — and then silently replaced by the next `set-status`.
    dir.child("opys/features/FEAT-0009.md")
        .write_str(
            "---\nid: FEAT-0009\nstatus: planned\ntags: [a]\nhistory:\n  \
             - \"2026-01-02T03:04:05.000000Z: planned\"\n  \
             - \"2026-02-02T03:04:05.000000Z: partial\"\n---\n\n# I\n",
        )
        .unwrap();
    // (d) `created` and the first key are different seconds.
    dir.child("opys/features/FEAT-0005.md")
        .write_str(&format!(
            "---\nid: FEAT-0005\nstatus: planned\ntags: [a]\ncreated: \"2026-05-05T00:00:00Z\"\n{}---\n\n# E\n",
            hist("planned")
        ))
        .unwrap();

    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("FEAT-0001: missing history"))
        .stderr(predicate::str::contains(
            "FEAT-0002: history's last entry (planned) does not match status (partial)",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0003: history keys must strictly increase",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0004: history key '2026-01-02T03:04:05.000000+00:00' is not a canonical \
             UTC timestamp (RFC3339 with 'Z' and exactly six fractional digits, \
             e.g. 2026-06-16T14:30:00.000000Z)",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0005: 'created' (2026-05-05T00:00:00Z) does not match the first history entry",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0006: history key '2026-01-02T03:04:05Z' is not a canonical UTC timestamp",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0007: history entry '2026-01-03T00:00:00.000000Z' does not record a status \
             as a string",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0008: a history key is not a string",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0009: 'history' is a sequence, not a timestamp→status mapping",
        ));
}

/// History keys are **quoted** in the file, and `verify` enforces the spelling.
///
/// It cannot do that from the parsed map: a YAML 1.2 reader (`serde_norway`,
/// and so opys) resolves a bare `2026-…Z` to the identical string, so an
/// unquoted key satisfies `history::is_map` and every other history check. A
/// YAML 1.1 reader does not — it resolves the key as a *timestamp* — so a corpus
/// written that way is not the string→string map format.md specifies for the
/// other implementations that read it. `verify` therefore reads the stored text
/// (`verify::spelling_problems`, through `Backend::stored_text`), which is also
/// the only check the hand-written migration of docs/migration-history.md has:
/// nothing else reports the spelling, and the next unrelated write silently
/// re-quotes the file, so the mistake would otherwise surface as a surprise diff
/// long after the migration.
#[test]
fn verify_flags_a_history_key_written_unquoted() {
    let dir = project();
    let bare = format!(
        "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nhistory:\n  \
         {PINNED_KEY}: planned\n---\n\n# A\n"
    );
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&bare)
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(format!(
            "FEAT-0001: history key '{PINNED_KEY}' is written unquoted (a bare RFC3339 \
             scalar is read as a timestamp, not a string, by a YAML 1.1 reader); \
             quote it: \"{PINNED_KEY}\": <status>"
        )));

    // The canonical spelling — the one opys itself writes — is what clears it,
    // and it is the only difference between the two files.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&bare.replace(&format!("  {PINNED_KEY}:"), &format!("  \"{PINNED_KEY}\":")))
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}

/// A `history` map opys cannot read is **data**: reported, never rewritten.
///
/// The shape rule is all-or-nothing (`history::is_map`), and `verify`,
/// `store::decompose` and `history::entries` all use it. When they disagreed —
/// `entries` reading the well-formed subset of a map the store kept opaque in
/// `fm_fields` — `verify` printed OK, and the next `set-status` exited 0 while
/// destroying every entry and leaving the corpus red. Now every append path
/// refuses and the bytes on disk do not move.
#[test]
fn a_botched_history_is_reported_and_no_write_touches_it() {
    let dir = project();
    enable_work_items(&dir);
    // One entry well formed, one not: the old reader saw a valid map here.
    let feat = "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n\
                created: \"2026-01-01T00:00:00Z\"\nhistory:\n  \
                \"2026-01-01T00:00:00.000000Z\": planned\n  \
                \"2026-02-01T00:00:00.000000Z\": null\n---\n\n# A\n";
    dir.child("opys/features/FEAT-0001.md")
        .write_str(feat)
        .unwrap();
    let task = "---\nid: TASK-0002\nstatus: todo\ntags: [a]\n\
                created: \"2026-01-01T00:00:00Z\"\nhistory:\n  \
                \"2026-01-01T00:00:00.000000Z\": 5\nreferences:\n  FEAT-0001: A\n---\n\n\
                # T\n\n## Tasks\n- [ ] x\n\n## Progress\n- none\n";
    dir.child("opys/TASK-0002.md").write_str(task).unwrap();
    // …and a value that is not a mapping at all: the YAML list a hand-written
    // migration script produces. The shape rule covers every shape, not just
    // mappings — this one was reported as `missing history` and then replaced
    // wholesale by the next `set-status`, which exited 0.
    let seq = "---\nid: FEAT-0003\nstatus: planned\ntags: [a]\n\
               created: \"2026-01-01T00:00:00Z\"\nhistory:\n  \
               - \"2026-01-01T00:00:00.000000Z: planned\"\n  \
               - \"2026-02-01T00:00:00.000000Z: partial\"\n---\n\n# C\n";
    dir.child("opys/features/FEAT-0003.md")
        .write_str(seq)
        .unwrap();

    // Reported, naming the document and the entry — not blessed, and not
    // silently reduced to `missing history`.
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: history entry '2026-02-01T00:00:00.000000Z' does not record a status \
             as a string; opys will not rewrite a lifecycle map it cannot read",
        ))
        .stderr(predicate::str::contains(
            "TASK-0002: history entry '2026-01-01T00:00:00.000000Z' does not record a status",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0003: 'history' is a sequence, not a timestamp→status mapping",
        ));

    // Every write path that would append refuses, before touching anything.
    // `block`/`unblock` are not on this list: they record a relation, never a
    // status, so they append nothing and have nothing to refuse (asserted at the
    // end of this test).
    for args in [
        vec!["set-status", "FEAT-0001", "partial"],
        vec!["set-status", "FEAT-0003", "partial"],
    ] {
        opys(&dir)
            .args(&args)
            .arg("--no-sync")
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("repair the lifecycle map by hand"));
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap(),
        feat,
        "a refused write must not move a byte of the map"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("opys/TASK-0002.md")).unwrap(),
        task
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("opys/features/FEAT-0003.md")).unwrap(),
        seq,
        "a sequence carries entries too — a refused write must not replace it"
    );

    // A write that does *not* record a status leaves the map alone too — and
    // keeps its keys quoted, rather than surrendering the block to the generic
    // YAML emitter, which writes bare RFC3339 keys a YAML 1.1 reader resolves
    // as timestamps.
    opys(&dir)
        .args(["tag", "FEAT-0001", "--add", "z", "--no-sync"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(
        text.contains(
            "history:\n  \"2026-01-01T00:00:00.000000Z\": planned\n  \
             \"2026-02-01T00:00:00.000000Z\": null\n"
        ),
        "{text}"
    );

    // `block` is the same kind of write: it records a relation, not a status,
    // so it runs on a document whose map opys cannot read — and leaves the map
    // exactly as it found it.
    opys(&dir)
        .args(["block", "TASK-0002", "--by", "FEAT-0001", "--no-sync"])
        .assert()
        .success();
    let after = std::fs::read_to_string(dir.path().join("opys/TASK-0002.md")).unwrap();
    assert!(after.contains("status: todo"), "{after}");
    assert!(after.contains("FEAT-0001: A"), "{after}");
    assert!(
        after.contains("history:\n  \"2026-01-01T00:00:00.000000Z\": 5\n"),
        "{after}"
    );
}

/// A control character in a history value used to be written as a raw line
/// break inside a double-quoted scalar (`format_string` quotes but does not
/// escape), so a *successful* command turned a parseable document into one
/// nothing could read — wedging `sync` for the whole corpus.
#[test]
fn a_control_character_in_history_survives_a_rewrite() {
    let dir = project();
    let text = "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nhistory:\n  \
                \"2026-01-02T03:04:05.000000Z\": \"open\\n---\\n\\n# gone\"\n---\n\n# A\n";
    dir.child("opys/features/FEAT-0001.md")
        .write_str(text)
        .unwrap();
    opys(&dir)
        .args(["tag", "FEAT-0001", "--add", "z"])
        .assert()
        .success();
    let after = std::fs::read_to_string(dir.path().join("opys/features/FEAT-0001.md")).unwrap();
    assert!(
        after.contains("\"2026-01-02T03:04:05.000000Z\": \"open\\n---\\n\\n# gone\""),
        "{after}"
    );
    // Still parseable: the only complaint is the (legitimate) status mismatch.
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid frontmatter YAML").not());
    opys(&dir).arg("sync").assert().success();
}

/// `import` copies a record's unrecognised keys into the frontmatter verbatim,
/// so it must refuse `history` by name — otherwise a record plants a lifecycle
/// map opys did not write, which `seed` then refuses to replace.
#[test]
fn import_refuses_a_record_carrying_history() {
    let dir = project();
    let jsonl = dir.child("rec.jsonl");
    jsonl
        .write_str(
            "{\"title\": \"Planted\", \"tags\": [\"a\"], \
             \"history\": {\"2026-01-01T00:00:00.000000Z\": \"planned\"}}\n",
        )
        .unwrap();
    opys(&dir)
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "history is auto-maintained — remove it from the record",
        ));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
}

/// `history` is readable from `query` but never a write surface: raw DML
/// against the table is refused. (The field-level refusal — the server's
/// `set-field` action — is pinned in `opys-server/tests/actions.rs`.)
#[test]
fn history_is_queryable_but_never_writable() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["query", "SELECT ts, status FROM history"])
        .assert()
        .success()
        .stdout(predicate::str::contains(PINNED_KEY));
    // `opys query` is read-only since ADR-0100, so the lifecycle map has no raw
    // write path at all: the flag is gone and a DELETE is refused on the plan.
    opys(&dir)
        .args(["query", "--write", "DELETE FROM history"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument '--write'"));
    opys(&dir)
        .args(["query", "DELETE FROM history"])
        .assert()
        .code(2);
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(PINNED_KEY));
}

#[test]
fn verify_rejects_malformed_timestamp_but_allows_absent() {
    let dir = project();
    // Absent timestamps (docs predating the fields) are allowed. With no
    // `created` to agree with, any well-formed history key is accepted.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}---\n\n# F\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("verify").assert().success();

    // A malformed `created` is a content problem (exit 1).
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\ncreated: not-a-date\n---\n\n# F\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: 'created' must be an RFC3339 datetime",
        ));
}

#[test]
fn sync_backfills_missing_timestamps_from_mtime() {
    let dir = project();
    // A document predating both the timestamps and the lifecycle map: sync
    // cannot invent history (only new/set-status/import write it), so the one
    // remaining problem afterwards is `missing history`.
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n")
        .unwrap();
    opys(&dir).arg("sync").assert().success();
    // After a sync pass the doc carries both auto-maintained timestamps.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("created:"))
        .assert(predicate::str::contains("updated:"));
    // And the backfilled values are valid RFC3339: verify complains about the
    // absent history and about nothing else.
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("verify: 1 problem(s)"))
        .stderr(predicate::str::contains("missing history"))
        .stderr(predicate::str::contains("must be an RFC3339 datetime").not());
}

/// A document that has history but no `created` takes `created` from the first
/// history key, not from the file's mtime — the two have to denote the same
/// instant, and the lifecycle map is the better record.
#[test]
fn sync_backfills_created_from_the_first_history_entry() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n{}---\n\n# F\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir).arg("sync").assert().success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(format!(
            "created: \"{PINNED_KEY}\""
        )));
    opys(&dir).arg("verify").assert().success();
}

// --- `opys history` (the `git` feature, on by default) -------------------------

/// A git repository built in the project dir through gix's write API alone.
///
/// [`Fixture`](opys_engine::git::fixture::Fixture) is the engine's shared test
/// support, so the binary's tests and the reader's own unit tests exercise one
/// definition of "a repository with a branch, or a detached worktree head" —
/// and neither spawns `git`, whose default branch name, identity and
/// `safe.directory` come from whichever machine the tests happen to run on.
#[cfg(feature = "git")]
fn repo(dir: &TempDir) -> opys_engine::git::fixture::Fixture {
    opys_engine::git::fixture::Fixture::init(dir.path())
}

/// A document with no lifecycle map — what an inventory that predates `history`
/// looks like, and the only thing `--seed` will write to.
#[cfg(feature = "git")]
fn unseeded_doc(status: &str) -> String {
    format!(
        "---\nid: FEAT-0001\nstatus: {status}\ntags: [auth]\n\
created: \"{PINNED_NOW}\"\nupdated: \"{PINNED_NOW}\"\n---\n\n# Login\n"
    )
}

/// Run a git command in `dir`, asserting success. Author identity is passed per
/// invocation so commits don't depend on ambient global git config. Still a
/// subprocess because its one remaining caller tests `opys renumber`, which
/// shells out to `git merge-base` itself.
fn git(dir: &TempDir, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .current_dir(dir.path())
        .args(args)
        .status()
        .expect("git available")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Stage everything and commit as the given author.
fn git_commit(dir: &TempDir, author: &str, message: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "-c",
            &format!("user.email={author}@example.com"),
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

#[cfg(feature = "git")]
#[test]
fn history_shows_status_timeline_with_authors() {
    let dir = project();
    let mut fx = repo(&dir);

    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    fx.commit("Alice", "create feature");

    opys(&dir)
        .args(["set-status", "FEAT-0001", "partial"])
        .assert()
        .success();
    fx.commit("Bob", "move to partial");

    // Newest-first timeline: both revisions, their statuses, and per-commit authors.
    opys(&dir)
        .args(["history", "FEAT-0001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 revisions"))
        .stdout(predicate::str::contains("planned"))
        .stdout(predicate::str::contains("partial"))
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob"));
}

#[cfg(feature = "git")]
#[test]
fn history_follows_a_relocation_without_leaking_paths() {
    // A config whose `archived` status relocates the file into an `_archived/`
    // subdir, so the doc moves on disk between commits.
    let cfg = "pad = 4\n\
[types.feature]\n\
prefix = \"FEAT\"\n\
dir = \"features\"\n\
status_dirs = { archived = \"_archived\" }\n\
statuses = [\"planned\", \"partial\", \"archived\"]\n\
default_status = \"planned\"\n\
tags_required = true\n\
[[types.feature.sections]]\nheading = \"Notes\"\nkind = \"prose\"\n";
    let dir = project_with(cfg);
    let mut fx = repo(&dir);

    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    fx.commit("Alice", "create feature");
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());

    opys(&dir)
        .args(["set-status", "FEAT-0001", "archived"])
        .assert()
        .success();
    // The file physically moved into the `_archived/` subdir.
    dir.child("opys/features/_archived/FEAT-0001.md")
        .assert(predicate::path::exists());
    fx.commit("Bob", "archive feature");

    // History still spans both revisions despite the move — and never surfaces
    // the on-disk path (taxonomy is opys's concern, not the user's).
    opys(&dir)
        .args(["history", "FEAT-0001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 revisions"))
        .stdout(predicate::str::contains("planned"))
        .stdout(predicate::str::contains("archived"))
        .stdout(predicate::str::contains("_archived").not());
}

/// BUG-0030: a revision is attributed to the commit that introduced it, not the
/// newest commit while the content was still current.
#[cfg(feature = "git")]
#[test]
fn history_attributes_revision_to_the_introducing_commit() {
    let dir = project();
    let mut fx = repo(&dir);
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    fx.commit("Alice", "create feature");
    // Several unrelated commits on top, by a different author, touching other
    // files — the document blob is unchanged across all of them.
    for i in 0..3 {
        dir.child(format!("notes-{i}.txt")).write_str("x").unwrap();
        fx.commit("Mallory", "unrelated work");
    }
    // The single revision is credited to Alice (who introduced it), never to the
    // later Mallory commits that merely carried the same content forward.
    opys(&dir)
        .args(["history", "FEAT-0001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 revision,"))
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Mallory").not());
}

/// The command's output is a fixed-width table, and other tools read it: pin it
/// line for line, not by `contains`.
#[cfg(feature = "git")]
#[test]
fn history_output_is_pinned_line_for_line() {
    let dir = project();
    let mut fx = repo(&dir);
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    let commit = fx.commit("Alice", "create feature");

    let short = opys_engine::git::fixture::short(commit);
    let expected = format!(
        "History of FEAT-0001 (1 revision, newest first):\n  \
         {short}  2026-02-01  Alice  {:<14}  create feature\n",
        "planned"
    );
    opys(&dir)
        .args(["history", "FEAT-0001"])
        .assert()
        .success()
        .stdout(expected);
}

/// The migration path of `docs/migration-history.md` §2: a document that
/// predates the lifecycle map gets one built from what git actually recorded.
#[cfg(feature = "git")]
#[test]
fn history_seed_writes_the_lifecycle_map_from_the_git_timeline() {
    let dir = project();
    let mut fx = repo(&dir);
    let doc = dir.child("opys/features/FEAT-0001.md");
    doc.write_str(&unseeded_doc("planned")).unwrap();
    fx.commit("Alice", "create feature");
    doc.write_str(&unseeded_doc("partial")).unwrap();
    fx.commit("Bob", "move to partial");

    // The state the migration starts from.
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains("FEAT-0001: missing history"));

    // A *later* clock than the one the document was written under: a seed
    // reconstructs the past, so `updated` must come out untouched below rather
    // than stamped with this instant (docs/migration-history.md §2, on which
    // §4's round-trip probe rests). Under the shared `PINNED_NOW` a stray
    // `touch` would rewrite `updated` to the identical string and hide itself.
    opys(&dir)
        .args(["history", "--seed", "FEAT-0001"])
        .env("OPYS_NOW", "2026-03-04T05:06:07Z")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "seeded 2 history entries from git",
        ));

    doc.assert(predicate::str::contains(format!(
        "updated: \"{PINNED_NOW}\""
    )));

    // One entry per status the document held. The first is keyed from `created`
    // (verify requires the two to denote the same second — a document is never
    // committed in the second it was created); the second is the commit that
    // introduced `partial`, in UTC with six digits, quoted.
    let second = opys_engine::git::fixture::commit_key(1);
    doc.assert(predicate::str::contains(format!(
        "history:\n  \"{PINNED_KEY}\": planned\n  \"{second}\": partial\n"
    )));
    opys(&dir).arg("verify").assert().success();
}

/// The ordinary mid-migration state: the working copy is ahead of the last
/// commit. That status is recorded last, at the wall clock, bumped past the
/// entry it collides with — because verify requires the last entry to match
/// `status`.
#[cfg(feature = "git")]
#[test]
fn history_seed_records_an_uncommitted_status_last() {
    let dir = project();
    let mut fx = repo(&dir);
    let doc = dir.child("opys/features/FEAT-0001.md");
    doc.write_str(&unseeded_doc("planned")).unwrap();
    fx.commit("Alice", "create feature");
    doc.write_str(&unseeded_doc("partial")).unwrap();

    opys(&dir)
        .args(["history", "--seed", "FEAT-0001"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "seeded 2 history entries from git",
        ));

    doc.assert(predicate::str::contains(format!(
        "history:\n  \"{PINNED_KEY}\": planned\n  \"2026-01-02T03:04:05.000001Z\": partial\n"
    )));
    opys(&dir).arg("verify").assert().success();
}

/// History cannot be rebuilt from ground truth, so a map that is already there
/// is data: `--seed` refuses rather than replacing it, and writes nothing.
#[cfg(feature = "git")]
#[test]
fn history_seed_refuses_to_overwrite_an_existing_map() {
    let dir = project();
    let mut fx = repo(&dir);
    // `new` seeds the map itself, so every document opys created has one.
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    fx.commit("Alice", "create feature");

    opys(&dir)
        .args(["history", "--seed", "FEAT-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "FEAT-0001 already has a lifecycle map (1 entry); --seed refuses to overwrite it",
        ));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(format!(
            "history:\n  \"{PINNED_KEY}\": planned\n"
        )));
}

/// The second refusal of `docs/migration-history.md` §2: a `history` that is
/// present but is not a lifecycle map is repaired by hand, never overwritten.
/// It is the one refusal that fires before the store is consulted for a
/// timeline at all, so the document must come back byte-identical.
#[cfg(feature = "git")]
#[test]
fn history_seed_refuses_a_botched_lifecycle_map() {
    let dir = project();
    // A repository with a commit, so the refusal cannot be mistaken for the
    // "no committed revision" one below.
    let mut fx = repo(&dir);
    let doc = dir.child("opys/features/FEAT-0001.md");
    // A YAML *sequence* where the timestamp→status mapping belongs — what a
    // hand edit or an older exporter leaves behind.
    let botched = format!(
        "---\nid: FEAT-0001\nstatus: planned\ntags: [auth]\n\
created: \"{PINNED_NOW}\"\nhistory:\n  - planned\n  - partial\n\
updated: \"{PINNED_NOW}\"\n---\n\n# Login\n"
    );
    doc.write_str(&botched).unwrap();
    fx.commit("Alice", "create feature");

    opys(&dir)
        .args(["history", "--seed", "FEAT-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "'history' is a sequence, not a timestamp→status mapping",
        ))
        .stderr(predicate::str::contains("repair the lifecycle map by hand"));

    // Nothing was written: the refusal precedes every mutation.
    assert_eq!(std::fs::read_to_string(doc.path()).unwrap(), botched);
}

/// Git can only show what was committed. With nothing committed there is no
/// timeline to seed from, and inventing one from the wall clock would be worse
/// than the `created` fallback the migration document describes.
#[cfg(feature = "git")]
#[test]
fn history_seed_refuses_when_git_has_no_committed_revision() {
    let dir = project();
    let _fx = repo(&dir);
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&unseeded_doc("planned"))
        .unwrap();

    opys(&dir)
        .args(["history", "--seed", "FEAT-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "git has no committed revision carrying a status",
        ));
}

/// A project that is no git repository at all: the *reader* degrades to
/// main-only plus one notice (`git::tests::a_non_repository_yields_empty_
/// occupancy_and_a_single_notice`), but the **command** whose whole subject is
/// the committed timeline has nothing to print, so it says why and exits 2
/// rather than reporting an empty history it never looked for. The path it
/// names is the project root without the interior `.` the default `--root`
/// leaves behind — a message that reads as a paste-able path.
#[cfg(feature = "git")]
#[test]
fn history_outside_a_repository_says_there_is_none_and_exits_2() {
    let dir = project();
    // Unseeded, so `--seed` reaches the repository check instead of refusing on
    // the lifecycle map it would otherwise overwrite.
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&unseeded_doc("planned"))
        .unwrap();

    let root = dir.path().display().to_string();
    for args in [
        vec!["history", "FEAT-0001"],
        vec!["history", "--seed", "FEAT-0001"],
    ] {
        opys(&dir)
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(format!(
                "git history: {root} is not in a git repository — there is no \
                 committed timeline to read"
            )));
    }

    // The same run with the *default* `--root .`, where the discovered root
    // carries an interior `.`: the message still names a plain path.
    Command::cargo_bin("opys")
        .unwrap()
        .current_dir(dir.path())
        .env("OPYS_NOW", PINNED_NOW)
        .args(["history", "FEAT-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(format!(
            "git history: {root} is not"
        )));
}

/// Built without the `git` feature, `history` is still a subcommand — and its
/// answer names the feature. Compiling the variant out instead left clap saying
/// "unrecognized subcommand 'history'; a similar subcommand exists: 'list'",
/// which tells a user their command does not exist rather than that their
/// binary was built without the half that implements it. Runs only in the
/// `--no-default-features` CI job, which is the build it describes.
#[cfg(not(feature = "git"))]
#[test]
fn history_without_the_git_feature_names_the_missing_feature() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();

    opys(&dir)
        .args(["history", "FEAT-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("`git` cargo feature"))
        .stderr(predicate::str::contains("--no-default-features"))
        .stderr(predicate::str::contains("unrecognized subcommand").not());

    // And it is documented, not merely accepted: `--help` lists it.
    opys(&dir)
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("history"));
}

#[test]
fn bulk_set_status_moves_several_features_with_a_comma_list() {
    let dir = project();
    for t in ["A", "B", "C"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // One command, comma-separated ids — same status applied to each.
    opys(&dir)
        .args(["set-status", "FEAT-0001,FEAT-0002,FEAT-0003", "partial"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 -> partial"))
        .stdout(predicate::str::contains("FEAT-0002 -> partial"))
        .stdout(predicate::str::contains("FEAT-0003 -> partial"));
    for n in 1..=3 {
        dir.child(format!("opys/features/FEAT-000{n}.md"))
            .assert(predicate::str::contains("status: partial"));
    }
}

#[test]
fn bulk_tag_takes_a_comma_list() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["tag", "FEAT-0001,FEAT-0002", "--add", "x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 tags: a, x"))
        .stdout(predicate::str::contains("FEAT-0002 tags: a, x"));
}

#[test]
fn bulk_retire_reserves_every_id() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["retire", "FEAT-0001,FEAT-0002", "--reason", "dupes"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::missing());
    // Both numbers are reserved — the next new doc is FEAT-0003.
    opys(&dir)
        .args(["new", "--title", "C", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0003.md"));
}

#[test]
fn bulk_reads_ids_from_stdin_with_dash() {
    let dir = project();
    for t in ["A", "B", "C"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // `-` reads the list from stdin; newline separation (as `list --format ids`
    // emits) is accepted, alongside commas and spaces.
    opys(&dir)
        .args(["set-status", "-", "partial"])
        .write_stdin("FEAT-0001\nFEAT-0002 FEAT-0003\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 -> partial"))
        .stdout(predicate::str::contains("FEAT-0002 -> partial"))
        .stdout(predicate::str::contains("FEAT-0003 -> partial"));
    for n in 1..=3 {
        dir.child(format!("opys/features/FEAT-000{n}.md"))
            .assert(predicate::str::contains("status: partial"));
    }
}

#[test]
fn bulk_stdin_pipe_from_list_closes_the_selection() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Feature", "--tags", "a"])
        .assert()
        .success();
    for t in ["One", "Two"] {
        opys(&dir)
            .args([
                "new",
                "--type",
                "task",
                "--title",
                t,
                "--features",
                "FEAT-0001",
            ])
            .assert()
            .success();
    }
    // The canonical pipe: select ids, close them. Done with a real shell so the
    // pipe and `-` work end to end.
    let close = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{bin} --root {root} list --type task --format ids | {bin} --root {root} close - --force",
            bin = assert_cmd::cargo::cargo_bin("opys").display(),
            root = dir.path().display(),
        ))
        .output()
        .unwrap();
    assert!(
        close.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&close.stderr)
    );
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::path::missing());
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::path::missing());
}

/// The bulk-edit pipe the docs ship verbatim: README, format.md, SKILL.md, the
/// web query console and FEAT-0013 all print this exact line, and
/// `docs_ship_a_bulk_pipe_that_runs` pins them to it.
const DOC_BULK_PIPE: &str = "opys list --type task --status review --format ids | opys close -";

/// Every file that writes the bulk-edit story for a human. All of them are
/// checked by `docs_ship_a_bulk_pipe_that_runs`.
const BULK_DOCS: [&str; 5] = [
    "README.md",
    "skills/opys/references/format.md",
    "skills/opys/SKILL.md",
    "opys-server/ui/src/Query.svelte",
    "inventory/FEAT-0013.md",
];

/// The documented pipe has to work against the *shipped default* config, not
/// just a test fixture. It used to select on `--status done`, which no live
/// document can ever hold (`done` is terminal for task/bug/chore and reached
/// only through `close`, which deletes the file), so the producer was always
/// empty and the example always exited 2 with `expected at least one id`.
///
/// It now selects on `review`, and that is not cosmetic: the shipped task graph
/// puts the only edges into `done` on `review` and `user-review`, so those are
/// the two statuses `close` may be run from at all.
#[test]
fn documented_bulk_pipe_runs_under_the_default_config() {
    let dir = TempDir::new().unwrap();
    opys(&dir).arg("init").assert().success();
    opys(&dir)
        .args(["new", "--title", "Feature", "--tags", "a"])
        .assert()
        .success();
    for t in ["One", "Two"] {
        opys(&dir)
            .args([
                "new",
                "--type",
                "task",
                "--title",
                t,
                "--features",
                "FEAT-0001",
            ])
            .assert()
            .success();
    }
    for n in [2, 3] {
        let path = dir.path().join(format!("opys/TASK-000{n}.md"));
        // The work is done: check the scaffolded `## Tasks` item, so `close`
        // has no unchecked-item reason to refuse the batch.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("- [ ]", "- [x]");
        std::fs::write(&path, text).unwrap();
    }
    // `todo -> in-progress -> review`: the shipped graph has no shortcut, and
    // `review` is where an edge into `done` (so, `close`) first exists.
    opys(&dir)
        .args(["set-status", "TASK-0002", "TASK-0003", "in-progress"])
        .assert()
        .success();
    // The shipped config also demands a checkpoint before a handoff into
    // `review` (`require_log_since_status`), which is what a real user does.
    for id in ["TASK-0002", "TASK-0003"] {
        opys(&dir)
            .args(["log", id, "ready for review"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["set-status", "TASK-0002", "TASK-0003", "review"])
        .assert()
        .success();

    // Run the documented line itself, with only `opys` swapped for the built
    // binary and this project's root.
    let script = DOC_BULK_PIPE.replace(
        "opys ",
        &format!(
            "{bin} --root {root} ",
            bin = assert_cmd::cargo::cargo_bin("opys").display(),
            root = dir.path().display(),
        ),
    );
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`{script}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir.child("opys/TASK-0002.md")
        .assert(predicate::path::missing());
    dir.child("opys/TASK-0003.md")
        .assert(predicate::path::missing());
}

/// Every place the bulk-edit story is written for a human ships the one pipe
/// that runs (above), and no example anywhere in those files names a status the
/// shipped default config cannot reach — the trap that produced both halves of
/// this guard. `done` is the default config's only terminal status: it belongs
/// to task/bug/chore, where `set-status` refuses it, and `feature` does not have
/// it at all, so an example naming it is a hard failure rather than an
/// illustration. That covers the producer side (`--status done --format ids`
/// selects nothing) and the consumer side (`set-status … done` is refused).
#[test]
fn docs_ship_a_bulk_pipe_that_runs() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    for rel in BULK_DOCS {
        let text = std::fs::read_to_string(repo.join(rel)).unwrap();
        assert!(
            text.contains(DOC_BULK_PIPE),
            "{rel} does not ship the canonical bulk pipe `{DOC_BULK_PIPE}`"
        );
        assert!(
            !text.contains("--status done --format ids"),
            "{rel} selects a bulk batch on a terminal status, which matches nothing"
        );
        for (n, line) in text.lines().enumerate() {
            let names_done = line
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .any(|w| w == "done");
            assert!(
                !(line.contains("set-status") && names_done),
                "{rel}:{} teaches `set-status` with the terminal status `done`, \
                 which the default config always refuses: {line}",
                n + 1
            );
        }
    }
}

/// Every file that writes opys' behaviour down for a human, plus the whole
/// dogfooded inventory: `no_doc_promises_the_stripped_fix_flag` reads all of
/// them.
const BEHAVIOUR_DOCS: [&str; 7] = [
    "README.md",
    "CLAUDE.md",
    "AGENTS.md",
    "skills/opys/SKILL.md",
    "skills/opys/agent-rule.md",
    "skills/opys/references/format.md",
    "src/templates.rs",
];

/// Drop the parts of a line that can only *name* another document: a relation
/// map / retired-ledger entry (`  ADR-0101: 'title'`) and any inline
/// `[ID — title](ID.md)` link. Both carry the referenced document's title
/// verbatim, so a title containing a withdrawn flag is not a promise of it.
fn without_titled_refs(line: &str) -> String {
    let after_indent = line.trim_start();
    let is_relation_entry = line.starts_with("  ")
        && after_indent
            .split_once(": ")
            .is_some_and(|(id, _)| is_doc_id(id.trim_start_matches("- ")));
    if is_relation_entry {
        return String::new();
    }
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](") else {
            break;
        };
        let Some(close) = rest[open + mid..].find(')') else {
            break;
        };
        let label = &rest[open + 1..open + mid];
        if is_doc_id(label.split(' ').next().unwrap_or("")) {
            out.push_str(&rest[..open]);
            rest = &rest[open + mid + close..];
        } else {
            out.push_str(&rest[..open + mid]);
            rest = &rest[open + mid..];
        }
    }
    out.push_str(rest);
    out
}

/// A `PREFIX-NNNN` document id, the shape `refs::id_number` parses.
fn is_doc_id(word: &str) -> bool {
    word.split_once('-').is_some_and(|(prefix, num)| {
        !prefix.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && prefix.starts_with(|c: char| c.is_ascii_uppercase())
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
    })
}

/// The `---`-fenced frontmatter of a document, empty for a file that has none.
fn frontmatter_of(text: &str) -> &str {
    text.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---").map(|(fm, _)| fm))
        .unwrap_or("")
}

/// `verify --fix` was withdrawn: the clock-driven half of the time-aware verify
/// moved to kobzar, and the flag does not exist. Two shapes of mention survive
/// legitimately — a document tagged `pending-move:kobzar` keeps the withdrawn
/// decision text *and* the note recording the withdrawal, and any file may
/// carry a titled reference to one of those documents, whose title happens to
/// contain the flag. Everything else promises a flag the binary refuses, which
/// is exactly what ADR-0099's decision 2 did: it went on listing
/// `verify --fix` among the writers of `history` after the strip, under a scope
/// note claiming every part of the decision was implemented.
#[test]
fn no_doc_promises_the_stripped_fix_flag() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut files: Vec<std::path::PathBuf> =
        BEHAVIOUR_DOCS.iter().map(|rel| repo.join(rel)).collect();
    let mut inventory: Vec<std::path::PathBuf> = std::fs::read_dir(repo.join("inventory"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    inventory.sort();
    assert!(
        inventory.len() > 10,
        "the dogfooded inventory should have been read, found {inventory:?}"
    );
    files.append(&mut inventory);

    for path in files {
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // A document that carries the marking is where the move is written up.
        if frontmatter_of(&text)
            .lines()
            .any(|l| l.starts_with("tags:") && l.contains("pending-move:kobzar"))
        {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            assert!(
                !without_titled_refs(line).contains("--fix"),
                "{}:{} names `--fix`, a flag `verify` no longer has; if this \
                 records the move to kobzar it belongs in a document tagged \
                 `pending-move:kobzar`: {line}",
                path.strip_prefix(&repo).unwrap_or(&path).display(),
                n + 1,
            );
        }
    }
}

/// The minimal `[types.task]` header the two `format.md` fragments below are
/// written against: the statuses their own text names, the conventional
/// `default_status`, and the one terminal status. Splicing it on is the
/// completion a reader makes, and it is where a fragment's own contradictions
/// (a `default_status` with no outgoing edge, a `then` nothing can reach)
/// surface.
const FORMAT_MD_TASK_HEADER: &str = "pad = 4\n[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"in-progress\", \"review\", \"stuck\", \"user-review\", \"done\"]\n\
default_status = \"todo\"\nterminal_statuses = [\"done\"]\n";

/// …and the shorter one, for the status-graph example, which names four.
const FORMAT_MD_GRAPH_HEADER: &str = "pad = 4\n[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"in-progress\", \"review\", \"user-review\", \"done\"]\n\
default_status = \"todo\"\nterminal_statuses = [\"done\"]\n";

/// The `toml` blocks in format.md are the configs readers copy, and the
/// normative file's own subject is what `config validate` accepts — yet two
/// successive rounds of new validator rules (`default_status` needs an outgoing
/// edge; a `max_visits` budget needs its escalation edges) each left a
/// documented example failing that very command. Feed them to the validator so
/// the next rule cannot.
#[test]
fn the_config_examples_in_format_md_pass_config_validate() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let md = std::fs::read_to_string(repo.join("skills/opys/references/format.md")).unwrap();
    // The first fenced `toml` block after a heading. A moved or renamed heading
    // fails loudly here rather than quietly pinning nothing.
    let block = |heading: &str| -> String {
        let from = md
            .find(heading)
            .unwrap_or_else(|| panic!("format.md no longer has the heading `{heading}`"));
        let open = from
            + md[from..]
                .find("```toml\n")
                .unwrap_or_else(|| panic!("no toml block under `{heading}`"))
            + "```toml\n".len();
        let close = open + md[open..].find("```").expect("unterminated toml block");
        md[open..close].to_string()
    };
    for (what, cfg) in [
        (
            "the `opys.toml` configuration reference",
            block("## Configuration: `opys.toml`"),
        ),
        (
            "the status-graph example",
            format!("{FORMAT_MD_GRAPH_HEADER}{}", block("### The status graph")),
        ),
        (
            "the time-aware example",
            format!(
                "{FORMAT_MD_TASK_HEADER}{}",
                block("### Time-aware observations")
            ),
        ),
    ] {
        let dir = project_with(&cfg);
        let out = opys(&dir).args(["config", "validate"]).output().unwrap();
        assert!(
            out.status.success(),
            "{what} in format.md does not pass `opys config validate`:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Rust ignores SIGPIPE, so an unguarded `println!` panics (exit 101, backtrace
/// on stderr) as soon as the reader of a pipe goes away — and `opys list
/// --format ids` is the documented producer of exactly such a pipe
/// (`| head -1`, `| fzf`, `| grep -q`). Every command must stop quietly instead.
#[test]
fn commands_stop_quietly_when_the_reader_closes_the_pipe() {
    let dir = project();
    for t in ["A", "B", "C"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    for args in [
        vec!["list", "--format", "ids"],
        vec!["list"],
        vec!["tags"],
        vec!["show", "FEAT-0001"],
        vec!["verify"],
        vec!["query", "SELECT id FROM docs"],
        vec!["tag", "FEAT-0001", "--add", "b"],
        // This corpus has no conflicting ids, so `renumber` prints its one line
        // and returns before it would go looking for a git base.
        vec!["renumber"],
    ] {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("opys"))
            .arg("--root")
            .arg(dir.path())
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Close the read end before the child has loaded the project, let alone
        // printed: every write it then attempts fails with EPIPE.
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains("panicked"), "`opys {args:?}` panicked: {err}");
        assert!(
            out.status.success(),
            "`opys {args:?}` exited {:?}: {err}",
            out.status.code()
        );
    }
}

#[test]
fn bulk_dedupes_repeated_ids() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "A", "--tags", "a"])
        .assert()
        .success();
    // A repeated id is collapsed — retire runs once, not twice (the second
    // pass would fail on the already-deleted file).
    opys(&dir)
        .args(["retire", "FEAT-0001,FEAT-0001", "--reason", "x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("retired FEAT-0001"));
}

/// Several ids as plain positionals — the shell-friendly form, equivalent to a
/// comma list (`opys list --format ids` output substituted with `$(…)` lands
/// here).
#[test]
fn bulk_takes_several_positional_ids() {
    let dir = project();
    for t in ["A", "B", "C"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args([
            "set-status",
            "FEAT-0001",
            "FEAT-0002",
            "FEAT-0003",
            "partial",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 -> partial"))
        .stdout(predicate::str::contains("FEAT-0003 -> partial"));
    for n in 1..=3 {
        dir.child(format!("opys/features/FEAT-000{n}.md"))
            .assert(predicate::str::contains("status: partial"));
    }
    // Mixed forms in one invocation: positionals and a comma list together.
    opys(&dir)
        .args(["tag", "FEAT-0001", "FEAT-0002,FEAT-0003", "--add", "x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002 tags: a, x"));
}

/// Where an option sits around a variadic id list. `--reason` may **lead** the
/// ids or **trail** the whole thing, with the ids given as positionals or read
/// from stdin as `-`; the README ships the trailing form and an agent piping
/// into `set-status` naturally reaches for the leading one. Both work only
/// because clap parses options anywhere in the argument list, and nothing else
/// in the suite pins the leading form: adding `trailing_var_arg`,
/// `allow_hyphen_values`, or another positional to one of these commands would
/// silently break it. (The one order that does *not* parse is an option wedged
/// between the ids and `set-status`'s trailing `STATUS` — asserted last, so the
/// usage line keeps naming the right order.)
#[test]
fn bulk_options_may_lead_or_trail_the_id_list() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // Back to a live status between the forms, so each one is a real transition.
    let replant = |dir: &TempDir| {
        opys(dir)
            .args(["set-status", "FEAT-0001", "FEAT-0002", "planned"])
            .assert()
            .success();
    };

    // Leading: the option comes before the ids.
    opys(&dir)
        .args([
            "set-status",
            "--reason",
            "leading",
            "FEAT-0001",
            "FEAT-0002",
            "wontfix",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002 -> wontfix"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("wontfix_reason: leading"));

    // Trailing: the option comes after the ids *and* the status (the README's
    // form).
    replant(&dir);
    opys(&dir)
        .args([
            "set-status",
            "FEAT-0001",
            "FEAT-0002",
            "wontfix",
            "--reason",
            "trailing",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("wontfix_reason: trailing"));

    // Both orders again with the ids on stdin — the `list --format ids | …`
    // shape, which is where an option has to share the line with `-`.
    replant(&dir);
    opys(&dir)
        .args(["set-status", "--reason", "piped-leading", "-", "wontfix"])
        .write_stdin("FEAT-0001\nFEAT-0002\n")
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("wontfix_reason: piped-leading"));

    replant(&dir);
    opys(&dir)
        .args(["set-status", "-", "wontfix", "--reason", "piped-trailing"])
        .write_stdin("FEAT-0001,FEAT-0002")
        .assert()
        .success();
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("wontfix_reason: piped-trailing"));

    // The accepted limitation: an option *between* the ids and the status is a
    // parse error, and the usage line it prints spells out the order that works.
    replant(&dir);
    opys(&dir)
        .args(["set-status", "FEAT-0001", "--reason", "middle", "wontfix"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("<IDS>... <STATUS>"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
}

/// A batch is all-or-nothing: one bad id aborts the whole invocation
/// with exit 2, the ids that already applied in memory are dropped un-flushed,
/// and nothing is announced on stdout for a write that never happened.
#[test]
fn bulk_is_all_or_nothing() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // The bad id is second, so the first really was applied to the store before
    // the abort — the proof is that it never reaches disk.
    opys(&dir)
        .args([
            "set-status",
            "FEAT-0001",
            "FEAT-9999",
            "FEAT-0002",
            "partial",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("FEAT-9999"))
        .stderr(predicate::str::contains("nothing was written"))
        .stdout(predicate::str::contains("-> partial").not());
    for n in 1..=2 {
        dir.child(format!("opys/features/FEAT-000{n}.md"))
            .assert(predicate::str::contains("status: planned"));
    }
}

/// `tag -` reads its ids from stdin, the `list --format ids` pipe shape.
#[test]
fn bulk_tag_reads_ids_from_stdin() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["tag", "-", "--add", "x"])
        .write_stdin("FEAT-0001\nFEAT-0002\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001 tags: a, x"))
        .stdout(predicate::str::contains("FEAT-0002 tags: a, x"));
}

/// `retire -` reads its ids from stdin.
#[test]
fn bulk_retire_reads_ids_from_stdin() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["retire", "-", "--reason", "dupes"])
        .write_stdin("FEAT-0001\nFEAT-0002\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("retired FEAT-0001"))
        .stdout(predicate::str::contains("retired FEAT-0002"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::missing());
}

/// `close -` reads its ids from stdin (the shell pipe itself is covered by
/// `bulk_stdin_pipe_from_list_closes_the_selection`).
#[test]
fn bulk_close_reads_ids_from_stdin() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Feature", "--tags", "a"])
        .assert()
        .success();
    for t in ["One", "Two"] {
        opys(&dir)
            .args([
                "new",
                "--type",
                "task",
                "--title",
                t,
                "--features",
                "FEAT-0001",
            ])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["close", "-", "--force"])
        .write_stdin("TASK-0002 TASK-0003\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("closed TASK-0002"))
        .stdout(predicate::str::contains("closed TASK-0003"));
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::path::missing());
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::path::missing());
}

/// `block -` and `unblock -` read their ids from stdin, so a selection can be
/// blocked (and released) in one invocation each.
#[test]
fn bulk_block_and_unblock_read_ids_from_stdin() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Feature", "--tags", "a"])
        .assert()
        .success();
    for t in ["One", "Two"] {
        opys(&dir)
            .args([
                "new",
                "--type",
                "task",
                "--title",
                t,
                "--features",
                "FEAT-0001",
            ])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["block", "-", "--by", "FEAT-0001"])
        .write_stdin("TASK-0002,TASK-0003\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0002 blocked by FEAT-0001"))
        .stdout(predicate::str::contains("TASK-0003 blocked by FEAT-0001"));
    for n in [2, 3] {
        dir.child(format!("opys/work-items/TASK-000{n}.md"))
            .assert(predicate::str::contains("FEAT-0001: Feature"))
            .assert(predicate::str::contains("status: todo"));
    }
    opys(&dir)
        .args(["list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("TASK-0002\nTASK-0003\n");
    opys(&dir)
        .args(["unblock", "-", "--by", "FEAT-0001"])
        .write_stdin("TASK-0002\nTASK-0003\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "TASK-0002 no longer blocked by FEAT-0001",
        ));
    for n in [2, 3] {
        dir.child(format!("opys/work-items/TASK-000{n}.md"))
            .assert(predicate::str::contains("blocked_by").not())
            .assert(predicate::str::contains("status: todo"));
    }
}

#[test]
fn show_refs_lists_code_references_to_the_id() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    // A code mention of the id, plus a near-miss that must NOT match (word
    // boundary: FEAT-00010 is a different id).
    dir.child("src/login.rs")
        .write_str("// implements FEAT-0001\nlet other = \"FEAT-00010\";\n")
        .unwrap();

    opys(&dir)
        .args(["show", "FEAT-0001", "--refs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("file references"))
        .stdout(predicate::str::contains("src/login.rs:1:"))
        .stdout(predicate::str::contains("implements FEAT-0001"))
        // The longer id on line 2 is not a reference to FEAT-0001.
        .stdout(predicate::str::contains("src/login.rs:2:").not());
}

#[test]
fn show_refs_reports_none_when_unreferenced() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    opys(&dir)
        .args(["show", "FEAT-0001", "--refs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("file references"))
        .stdout(predicate::str::contains("(none)"));
}

#[test]
fn show_refs_honors_configured_formats() {
    // A format for the compact `feat_1` style (prefix lowercased, unpadded num).
    let cfg = format!(
        "{}\n[file_refs]\nroots = [\"src\"]\nformats = [{{ template = \"{{prefix_lower}}_{{num}}\" }}]\n",
        default_cfg()
    );
    let dir = project_with(&cfg);
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    dir.child("src/handler.rs")
        .write_str("// see feat_1 for the contract\n")
        .unwrap();

    opys(&dir)
        .args(["show", "FEAT-0001", "--refs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("src/handler.rs:1:"))
        .stdout(predicate::str::contains("see feat_1"));
}

#[test]
fn renumber_warns_about_file_references_with_sed_suggestion() {
    let dir = project();
    // Two documents sharing the numeric id 1 (a cross-branch collision). No git
    // base is resolvable, so renumber keeps the first (FEAT-0001) and renumbers
    // the rest (TASK-0001 → TASK-0002).
    dir.child("opys/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [auth]\n---\n# Login\n")
        .unwrap();
    dir.child("opys/TASK-0001.md")
        .write_str("---\nid: TASK-0001\nstatus: todo\ntags: []\n---\n# Do it\n")
        .unwrap();
    // A code reference to the id that will be renumbered.
    dir.child("src/worker.rs")
        .write_str("// part of TASK-0001\nrun(\"TASK-0001\");\n")
        .unwrap();

    opys(&dir)
        .args(["--no-sync", "renumber"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0001 → TASK-0002"))
        .stderr(predicate::str::contains(
            "file reference(s) still point at a renumbered id",
        ))
        .stderr(predicate::str::contains("src/worker.rs:1:"))
        .stderr(predicate::str::contains(
            "sed -i 's/\\bTASK-0001\\b/TASK-0002/g' src/worker.rs",
        ));
}

/// BUG-0035: a base document relocated on the branch is resolved by id (not its
/// current path), so it is *kept* and the genuinely new id is renumbered — even
/// when the new id sorts first.
#[test]
fn renumber_keeps_a_relocated_base_document() {
    let cfg = "pad = 4\n\
[types.note]\n\
prefix = \"NOTE\"\n\
status_dirs = { archived = \"_archived\" }\n\
statuses = [\"open\", \"archived\"]\n\
default_status = \"open\"\n\
[types.feature]\n\
prefix = \"FEAT\"\n\
statuses = [\"planned\"]\n\
default_status = \"planned\"\n";
    let dir = project_with(cfg);
    git(&dir, &["init", "-q", "-b", "main"]);
    // Base commit: NOTE-0001 lives at opys/NOTE-0001.md.
    opys(&dir)
        .args(["new", "--type", "note", "--title", "A note"])
        .assert()
        .success();
    git_commit(&dir, "Base", "add note");
    // On the branch, archive it — the file relocates into _archived/.
    opys(&dir)
        .args(["set-status", "NOTE-0001", "archived"])
        .assert()
        .success();
    dir.child("opys/_archived/NOTE-0001.md")
        .assert(predicate::path::exists());
    // A genuinely new doc collides on the number 1 (and sorts before NOTE).
    dir.child("opys/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [x]\n---\n\n# New\n")
        .unwrap();

    opys(&dir)
        .args(["--no-sync", "renumber", "--base", "main"])
        .assert()
        .success()
        // The relocated base doc is kept; the new id is the one renumbered.
        .stdout(predicate::str::contains("FEAT-0001 → FEAT-0002"))
        .stdout(predicate::str::contains("NOTE-0001 →").not());
    dir.child("opys/_archived/NOTE-0001.md")
        .assert(predicate::path::exists());
}

/// A pre-0.12 plaintext `_retired.txt` is migrated to `_retired.md` on the next
/// write (here a no-op `sync`), and the reserved id survives the migration.
#[test]
fn retired_ledger_migrates_from_legacy_txt() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# One\n")
        .unwrap();
    dir.child("opys/_retired.txt")
        .write_str("FEAT-0007  # retired 2026-01-01: old\n")
        .unwrap();
    opys(&dir).arg("sync").assert().success();
    // The legacy file is gone; the markdown ledger holds the reserved id.
    dir.child("opys/_retired.txt")
        .assert(predicate::path::missing());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0007"));
    // Still reserved and queryable after migration.
    opys(&dir)
        .args(["query", "SELECT id FROM retired WHERE id = 'FEAT-0007'"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0007"));
}

/// The `blocks` projection decomposes each body into one row per `##` section
/// (plus the preamble), for querying into document bodies.
#[test]
fn query_blocks_exposes_body_sections() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\n---\n\n# Login\n\nIntro.\n\n## Notes\nBeware the edge case.\n",
        )
        .unwrap();
    // Sections are addressable by heading; retrieval + single-line content match.
    opys(&dir)
        .args([
            "query",
            "SELECT doc_id FROM blocks WHERE heading = 'Notes' AND text LIKE '%edge%'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));
    // The preamble is seq 0 with an empty heading.
    opys(&dir)
        .args([
            "query",
            "SELECT text FROM blocks WHERE doc_id = 'FEAT-0001' AND seq = 0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Login"));
}

/// `query --stdin` binds stdin to `$1`, so a multi-line value with
/// quotes/percent signs can be matched without SQL escaping.
#[test]
fn query_binds_stdin_to_param() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\n---\n\n# T\n\n\
             ## Notes\nfirst line\nline with 'quotes' and 50% off\n",
        )
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--stdin",
            "SELECT doc_id FROM blocks WHERE heading = 'Notes' AND text = $1",
        ])
        .write_stdin("first line\nline with 'quotes' and 50% off\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));
}

/// `--write` is gone (ADR-0100): `opys query` is read-only, so clap rejects the
/// flag outright rather than the engine refusing the statement later.
#[test]
fn query_rejects_the_removed_write_flag() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n")
        .unwrap();
    opys(&dir)
        .args(["query", "--write", "UPDATE docs SET status = 'x'"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument \'--write\'"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
}

/// A minimal closable type for the ledger-on-close tests: no links, no
/// required sections, so a doc can be closed with nothing referencing it.
const SOLO_TASK_CFG: &str = "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"done\"]\ndefault_status = \"todo\"\n\
terminal_statuses = [\"done\"]\n";

/// Closing a doc nobody references must still reserve its id — there is no
/// tombstone in any relation map, so the retired ledger is the only record.
#[test]
fn close_unreferenced_doc_reserves_id() {
    let dir = project_with(SOLO_TASK_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Solo"])
        .assert()
        .success();
    opys(&dir).args(["close", "TASK-0001"]).assert().success();
    dir.child("opys/TASK-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("TASK-0001: Solo"));
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Next"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0002"));
}

/// Closing a doc whose own maps carry the only tombstone of an earlier close
/// must reserve that id too — the tombstone is destroyed with the file.
#[test]
fn close_reserves_tombstones_carried_by_the_closed_doc() {
    let dir = project_with(SOLO_TASK_CFG);
    // Pre-fix state: TASK-0001 was closed while only TASK-0002 referenced it —
    // its number is anchored solely by this struck entry (no ledger).
    dir.child("opys/TASK-0002.md")
        .write_str(
            "---\nid: TASK-0002\nstatus: todo\nreferences:\n  TASK-0001: ~~Old one~~\n---\n\n# Two\n",
        )
        .unwrap();
    opys(&dir).args(["close", "TASK-0002"]).assert().success();
    let ledger = std::fs::read_to_string(dir.child("opys/_retired.md").path()).unwrap();
    assert!(ledger.contains("TASK-0001: Old one"), "{ledger:?}");
    assert!(ledger.contains("TASK-0002: Two"), "{ledger:?}");
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Next"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003"));
}

/// `cleanup` stripping the last tombstone of a dead id must first reserve it in
/// the ledger — covers docs closed before close itself wrote the ledger.
#[test]
fn cleanup_reserves_stripped_tombstone_ids() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "First",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["close", "TASK-0002", "--force"])
        .assert()
        .success();
    // Simulate the pre-ledger-on-close era: only the tombstone reserves the id.
    std::fs::remove_file(dir.child("opys/_retired.md").path()).unwrap();
    opys(&dir).args(["cleanup"]).assert().success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("~~First~~").not());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("TASK-0002: First"));
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Second",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003"));
}

/// A present-but-unparseable `_retired.md` is an error, never an empty ledger:
/// allocation refuses to run (exit 2) and the file is left byte-identical.
#[test]
fn corrupt_ledger_blocks_allocation_and_stays_untouched() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "dupe"])
        .assert()
        .success();
    let broken = "--\nbroken ledger\n";
    dir.child("opys/_retired.md").write_str(broken).unwrap();

    opys(&dir)
        .args(["new", "--title", "Y", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unreadable retired ledger"));
    // Nothing was created and the ledger was not rewritten from the empty read.
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::missing());
    let after = std::fs::read_to_string(dir.child("opys/_retired.md").path()).unwrap();
    assert_eq!(after, broken);
}

/// `verify` reports a corrupt ledger as a content problem (exit 1, alongside
/// everything else), not a hard error.
#[test]
fn verify_flags_corrupt_ledger() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# One\n")
        .unwrap();
    dir.child("opys/_retired.md")
        .write_str("--\nbroken ledger\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("unreadable retired ledger"));
}

/// `retire` strikes every inbound reference into a tombstone (like `close`), so
/// a successful retire leaves the corpus passing verify.
#[test]
fn retire_strikes_inbound_references_and_verify_stays_green() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# One\n")
        .unwrap();
    dir.child("opys/features/FEAT-0002.md")
        .write_str(&format!(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [a]\n{}references:\n  FEAT-0001: One\n---\n\n# Two\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "dupe"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("FEAT-0001: ~~One~~"));
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001: One"));
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (1 documents)"));
}

/// `--reason` is optional: the ledger records the id and title; git records the
/// when and why.
#[test]
fn retire_reason_is_optional() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("retired FEAT-0001"));
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001: X"));
}

/// The legacy plaintext ledger gets the same protection as `_retired.md`: a
/// present-but-unreadable `_retired.txt` blocks allocation instead of reading
/// as empty (which would also let the migration delete it).
#[test]
fn corrupt_legacy_ledger_blocks_allocation_and_survives() {
    let dir = project();
    // Invalid UTF-8 makes the read fail deterministically on every platform.
    std::fs::create_dir_all(dir.child("opys").path()).unwrap();
    std::fs::write(dir.child("opys/_retired.txt").path(), b"FEAT-0007 \xff\n").unwrap();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unreadable retired ledger"));
    // The legacy file was neither migrated away nor rewritten.
    dir.child("opys/_retired.txt")
        .assert(predicate::path::exists());
    dir.child("opys/_retired.md")
        .assert(predicate::path::missing());
}

/// Retiring a doc that another doc *requires* a link to succeeds but warns —
/// the struck tombstone no longer satisfies the `requires_link` rule.
#[test]
fn retire_warns_when_striking_a_required_link() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "F", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "First",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "dupe"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "must reference at least 1 doc(s) of type 'feature'",
        ));
}

/// A `dir = "."` layout renders paths with a leading `./` segment — harmless,
/// and must not trip the flush containment check.
#[test]
fn layout_dot_dir_still_flushes() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \".\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    opys(&dir).args(["new", "--title", "X"]).assert().success();
    dir.child("opys/FEAT-0001.md")
        .assert(predicate::path::exists());
}

// ------------------------------------------------------------------ lock

/// FEAT-0021: concurrent `new` invocations serialize on the inventory lock and
/// never allocate the same id — six racing processes yield six documents with
/// six distinct numbers, and the corpus still verifies (duplicate numbers
/// would make `verify` exit 1).
#[test]
fn concurrent_new_allocates_distinct_ids() {
    let dir = project();
    let root = dir.path().to_path_buf();
    let handles: Vec<_> = (0..6)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                let mut cmd = Command::cargo_bin("opys").unwrap();
                cmd.arg("--root").arg(&root);
                cmd.args(["new", "--title", &format!("Doc {i}"), "--tags", "core"]);
                cmd.assert().success();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let files = std::fs::read_dir(dir.path().join("opys").join("features"))
        .unwrap()
        .count();
    assert_eq!(files, 6);
    opys(&dir).arg("verify").assert().success();
}

/// FEAT-0021: while another process holds the lock, an invocation waits
/// (retries) instead of failing, and proceeds as soon as the holder releases.
#[test]
fn lock_contention_waits_then_succeeds() {
    use fs4::fs_std::FileExt as _;
    let dir = project();
    let base = dir.path().join("opys");
    std::fs::create_dir_all(&base).unwrap();
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(opys_backend_markdown_local::lock_path(&base).unwrap())
        .unwrap();
    assert!(holder.try_lock_exclusive().unwrap());

    let root = dir.path().to_path_buf();
    let waiter = std::thread::spawn(move || {
        let mut cmd = Command::cargo_bin("opys").unwrap();
        cmd.arg("--root").arg(&root);
        cmd.env("OPYS_LOCK_TIMEOUT_MS", "5000");
        cmd.args(["new", "--title", "Waited", "--tags", "core"]);
        cmd.assert().success();
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(holder); // closing the fd releases the flock
    waiter.join().unwrap();
}

/// FEAT-0021: contention past the timeout surfaces as a real error (exit 2)
/// naming the inventory lock, not a hang or a corrupt write.
#[test]
fn lock_timeout_is_a_clear_error() {
    use fs4::fs_std::FileExt as _;
    let dir = project();
    let base = dir.path().join("opys");
    std::fs::create_dir_all(&base).unwrap();
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(opys_backend_markdown_local::lock_path(&base).unwrap())
        .unwrap();
    assert!(holder.try_lock_exclusive().unwrap());

    opys(&dir)
        .env("OPYS_LOCK_TIMEOUT_MS", "150")
        .args(["new", "--title", "Blocked", "--tags", "core"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("inventory lock"));
}

/// FEAT-0021: an invocation that fails mid-command releases the lock (the
/// flock dies with its file handle — stale locks cannot exist), so the next
/// invocation proceeds without waiting out any timeout.
#[test]
fn failed_command_releases_the_lock() {
    let dir = project();
    opys(&dir)
        .args(["set-status", "FEAT-9999", "implemented"])
        .assert()
        .failure()
        .code(2);
    opys(&dir)
        .env("OPYS_LOCK_TIMEOUT_MS", "500")
        .args(["new", "--title", "After the failure", "--tags", "core"])
        .assert()
        .success();
}

/// `base` names a directory *inside* the project, and nothing else. An escaping
/// base aims every read and write the tool performs at another directory, which
/// has no legitimate use — so it is a hard error (exit 2), not a `verify`
/// finding, and it fires before any command can act on it.
#[test]
fn base_may_not_escape_the_project_root() {
    let dir = project_with(&format!("base = \"../victim\"\n{}", default_cfg()));
    std::fs::create_dir_all(dir.path().parent().unwrap().join("victim")).ok();

    opys(&dir)
        .args([
            "new", "--type", "feature", "--title", "escaped", "--tags", "x",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must live inside the project"));

    // The same refusal on a read path, so nothing loads the corpus either.
    opys(&dir)
        .arg("list")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must live inside the project"));
}

/// An absolute `base` escapes just as effectively as `..` — `Path::join` with an
/// absolute path discards the root entirely — so it is refused the same way.
#[test]
fn base_may_not_be_absolute() {
    let dir = project_with(&format!("base = \"/tmp\"\n{}", default_cfg()));
    opys(&dir)
        .arg("list")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must live inside the project"));
}

/// The check must not cost the ordinary case: a nested base under the root is
/// still fine, and `config validate` names the problem when it is not.
#[test]
fn a_nested_base_is_fine_and_config_validate_reports_an_escaping_one() {
    let ok = project_with(&format!("base = \"docs/inventory\"\n{}", default_cfg()));
    opys(&ok).arg("verify").assert().success();

    let bad = project_with(&format!("base = \"../victim\"\n{}", default_cfg()));
    opys(&bad)
        .args(["config", "validate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must live inside the project"));
}

// ---------------------------------------------------------------------------
// Interleaved id sequences (ADR-0103): `[ids] modulus`, `OPYS_ID_SLOT`.
// ---------------------------------------------------------------------------

/// The default feature config plus `[ids] modulus = M` (appended: the table
/// must follow every top-level key, or it would swallow `pad`).
fn modulus_cfg(modulus: u64) -> String {
    format!("{}\n[ids]\nmodulus = {modulus}\n", default_cfg())
}

/// A project under `modulus` seeded with a hand-written FEAT-0001, so two
/// projects start from an identical corpus regardless of slot.
fn seeded_modulus_project(modulus: u64) -> TempDir {
    let dir = project_with(&modulus_cfg(modulus));
    dir.child("opys/features/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [seed]\n{}---\n\n# One\n",
            hist("planned")
        ))
        .unwrap();
    dir
}

/// Create `n` documents with `slot`, returning the created file names in order.
fn new_in_slot(dir: &TempDir, slot: &str, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let out = opys(dir)
                .env("OPYS_ID_SLOT", slot)
                .args(["new", "--title", &format!("Doc {i}"), "--tags", "a"])
                .assert()
                .success()
                .get_output()
                .stdout
                .clone();
            let path = String::from_utf8(out).unwrap();
            std::path::Path::new(path.trim())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// With no modulus configured, allocation is the dense sequence and a stray
/// `OPYS_ID_SLOT` — even one that would be invalid under any modulus — is not
/// read at all.
#[test]
fn new_without_a_modulus_ignores_the_slot_env() {
    let dir = project();
    opys(&dir)
        .env("OPYS_ID_SLOT", "not a slot")
        .args(["new", "--title", "One", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001.md"));
    opys(&dir)
        .env_remove("OPYS_ID_SLOT")
        .args(["new", "--title", "Two", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0002.md"));
}

/// Two corpora that start identical and allocate in different slots produce
/// disjoint, congruent ids — and merging one's new files into the other passes
/// `verify` (the number-uniqueness check is the invariant interleaving keeps).
#[test]
fn interleaved_slots_allocate_disjoint_ids_across_corpora() {
    let a = seeded_modulus_project(2);
    let b = seeded_modulus_project(2);
    assert_eq!(
        new_in_slot(&a, "0", 3),
        ["FEAT-0002.md", "FEAT-0004.md", "FEAT-0006.md"]
    );
    assert_eq!(
        new_in_slot(&b, "1", 3),
        ["FEAT-0003.md", "FEAT-0005.md", "FEAT-0007.md"]
    );
    for name in ["FEAT-0003.md", "FEAT-0005.md", "FEAT-0007.md"] {
        std::fs::copy(
            b.path().join("opys/features").join(name),
            a.path().join("opys/features").join(name),
        )
        .unwrap();
    }
    opys(&a).arg("verify").assert().success();
    opys(&a).arg("sync").assert().success();
    opys(&a).arg("verify").assert().success();
    // The merged corpus keeps allocating above everything it now holds.
    assert_eq!(new_in_slot(&a, "0", 1), ["FEAT-0008.md"]);
}

/// The same slot on two identical corpora allocates the same ids — the
/// misconfiguration `renumber` exists to repair.
#[test]
fn interleaved_same_slot_allocates_the_same_ids() {
    let a = seeded_modulus_project(2);
    let b = seeded_modulus_project(2);
    let expected = ["FEAT-0003.md", "FEAT-0005.md", "FEAT-0007.md"];
    assert_eq!(new_in_slot(&a, "1", 3), expected);
    assert_eq!(new_in_slot(&b, "1", 3), expected);
}

/// A slot at or above the modulus, or not an integer, is a usage error (exit
/// 2) naming both the slot and the modulus; nothing is written.
#[test]
fn interleaved_rejects_a_slot_outside_the_modulus() {
    let dir = seeded_modulus_project(2);
    opys(&dir)
        .env("OPYS_ID_SLOT", "2")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("OPYS_ID_SLOT=2"))
        .stderr(predicate::str::contains("modulus 2"))
        .stderr(predicate::str::contains("valid slots: 0 to 1"));
    opys(&dir)
        .env("OPYS_ID_SLOT", "abc")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("OPYS_ID_SLOT=\"abc\""))
        .stderr(predicate::str::contains("modulus 2"));
    // Set but empty is the orchestrator's unassigned `$SLOT`, not "unset": it
    // must fail rather than silently land in the hash slot.
    opys(&dir)
        .env("OPYS_ID_SLOT", "")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("OPYS_ID_SLOT=\"\""))
        .stderr(predicate::str::contains("modulus 2"));
    for name in ["FEAT-0002.md", "FEAT-0003.md"] {
        dir.child("opys/features")
            .child(name)
            .assert(predicate::path::missing());
    }
}

/// The slot is resolved *before* the inventory lock is taken
/// (`Ctx::id_source`), so a bad `OPYS_ID_SLOT` is reported as a bad slot even
/// while another process holds the lock — never as a lock timeout, and without
/// waiting one out. Deterministic: the holder never lets go, so a resolution
/// that had moved after the load could only end in the timeout message.
#[test]
fn a_bad_slot_is_refused_before_the_inventory_lock() {
    use fs4::fs_std::FileExt as _;
    let dir = seeded_modulus_project(2);
    let base = dir.path().join("opys");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(opys_backend_markdown_local::lock_path(&base).unwrap())
        .unwrap();
    assert!(holder.try_lock_exclusive().unwrap());

    opys(&dir)
        .env("OPYS_ID_SLOT", "5")
        .env("OPYS_LOCK_TIMEOUT_MS", "150")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("OPYS_ID_SLOT=5"))
        .stderr(predicate::str::contains("modulus 2"))
        .stderr(predicate::str::contains("inventory lock").not());
    drop(holder); // closing the fd releases the flock
    dir.child("opys/features/FEAT-0003.md")
        .assert(predicate::path::missing());
}

/// `modulus` above 2^32 is a config problem too: a larger stride could walk
/// ids past the store's 64-bit numbers, where they would wrap and be reused.
#[test]
fn modulus_above_the_cap_is_a_config_problem() {
    let dir = project_with(&modulus_cfg(4_294_967_297));
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "ids.modulus must be at most 4294967296 (got 4294967297)",
        ));
    opys(&dir)
        .env("OPYS_ID_SLOT", "0")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "[ids] modulus must be at most 4294967296 (got 4294967297)",
        ));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
}

/// `modulus` below 2 is a config problem (`config validate` and `verify` list
/// it, exit 1) and every allocating command refuses to run (exit 2).
#[test]
fn modulus_below_two_is_a_config_problem() {
    let dir = project_with(&modulus_cfg(1));
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "ids.modulus must be at least 2 (got 1)",
        ));
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "opys.toml: ids.modulus must be at least 2",
        ));
    opys(&dir)
        .env("OPYS_ID_SLOT", "0")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "[ids] modulus must be at least 2 (got 1)",
        ));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
}

/// `[ids]` accepts only `modulus`. A misspelled key is a TOML parse error —
/// a hard error (exit 2) from every command, `config validate` included, not
/// a project that silently allocates densely.
#[test]
fn a_misspelled_ids_key_is_a_config_error() {
    let dir = project_with(&format!("{}\n[ids]\nmodulos = 16\n", default_cfg()));
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("opys.toml"))
        .stderr(predicate::str::contains(
            "unknown field `modulos`, expected `modulus`",
        ));
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unknown field `modulos`"));
    opys(&dir)
        .env("OPYS_ID_SLOT", "0")
        .args(["new", "--title", "Nope", "--tags", "a"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unknown field `modulos`"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
}

/// `import` reserves the batch as one block that steps by the modulus, so
/// every imported id stays in the slot.
#[test]
fn import_under_a_modulus_steps_by_the_modulus() {
    let dir = seeded_modulus_project(4);
    let jsonl = dir.child("import.jsonl");
    jsonl
        .write_str(
            "{\"title\": \"One\", \"tags\": [\"osc\"]}\n\
             {\"title\": \"Two\", \"tags\": [\"tabs\"], \"status\": \"partial\"}\n\
             {\"title\": \"Three\", \"tags\": [\"x\"]}\n",
        )
        .unwrap();
    opys(&dir)
        .env("OPYS_ID_SLOT", "1")
        .args(["import", jsonl.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "imported 3 feature document(s): FEAT-0005..FEAT-0013 step 4\n",
        ));
    for name in ["FEAT-0005.md", "FEAT-0009.md", "FEAT-0013.md"] {
        dir.child("opys/features")
            .child(name)
            .assert(predicate::path::exists());
    }
    dir.child("opys/features/FEAT-0009.md")
        .assert(predicate::str::contains("# Two"));
    opys(&dir).arg("verify").assert().success();
    // The next `new` in the same slot continues past the block.
    assert_eq!(new_in_slot(&dir, "1", 1), ["FEAT-0017.md"]);
}

/// Without `OPYS_ID_SLOT` the slot is FNV-1a of `user@host` modulo M: pinned
/// here for `alice@box` (hash `0xa1f84a6896be4a72`, slot 2 of 4) so it can
/// never drift silently — and with it the fallback chains that decide which
/// `user` and which `host`: `USER` then `USERNAME`, `HOSTNAME` then
/// `COMPUTERNAME`, in that order, with a set-but-blank value counting as unset.
#[test]
fn interleaved_slot_falls_back_to_a_hash_of_user_at_host() {
    let dir = seeded_modulus_project(4);
    for expected in ["FEAT-0002.md", "FEAT-0006.md"] {
        opys(&dir)
            .env_remove("OPYS_ID_SLOT")
            .env("USER", "alice")
            .env("HOSTNAME", "box")
            .args(["new", "--title", "Hashed", "--tags", "a"])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
    // USERNAME stands in when USER is unset; the slot is the same identity.
    opys(&dir)
        .env_remove("OPYS_ID_SLOT")
        .env_remove("USER")
        .env("USERNAME", "alice")
        .env("HOSTNAME", "box")
        .args(["new", "--title", "Hashed", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0010.md"));
    // COMPUTERNAME stands in when HOSTNAME is unset, ahead of `/etc/hostname`
    // — which is also what keeps this case off the test machine's own name.
    opys(&dir)
        .env_remove("OPYS_ID_SLOT")
        .env_remove("HOSTNAME")
        .env("USER", "alice")
        .env("COMPUTERNAME", "box")
        .args(["new", "--title", "Hashed", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0014.md"));
    // The chains have an order, not a set: with both members of each pair set
    // it is USER and HOSTNAME that decide, so the identity is still `alice@box`
    // (slot 2). Every way of swapping an `or_else` lands somewhere else —
    // `mallory@box` in slot 0, `alice@other` in 1, `mallory@other` in 3 — so
    // none of them can produce this id.
    opys(&dir)
        .env_remove("OPYS_ID_SLOT")
        .env("USER", "alice")
        .env("USERNAME", "mallory")
        .env("HOSTNAME", "box")
        .env("COMPUTERNAME", "other")
        .args(["new", "--title", "Hashed", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0018.md"));
    // Set but blank counts as unset on every step of both chains: a whitespace
    // USER/HOSTNAME falls through to USERNAME/COMPUTERNAME rather than hashing
    // `  @  `.
    opys(&dir)
        .env_remove("OPYS_ID_SLOT")
        .env("USER", "  ")
        .env("USERNAME", "alice")
        .env("HOSTNAME", "  ")
        .env("COMPUTERNAME", "box")
        .args(["new", "--title", "Hashed", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0022.md"));
}

/// A read never allocates, so it never resolves a slot: even an unusable
/// `OPYS_ID_SLOT` leaves `query`, `list` and `verify` working. (The
/// fails-before-the-load half of this property moved to
/// `a_bad_slot_is_refused_before_the_inventory_lock` when `query --write`
/// went away with ADR-0100.)
#[test]
fn reads_ignore_an_unusable_id_slot() {
    let dir = seeded_modulus_project(4);
    for args in [
        vec!["query", "SELECT id FROM docs"],
        vec!["list"],
        vec!["verify"],
    ] {
        opys(&dir)
            .env("OPYS_ID_SLOT", "9")
            .args(&args)
            .assert()
            .success();
    }
    opys(&dir)
        .env("OPYS_ID_SLOT", "9")
        .args(["query", "SELECT id FROM docs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));
}

/// `renumber` allocates replacement ids through the source too, so the repair
/// lands in this allocator's slot (and cannot collide with another slot).
#[test]
fn renumber_under_a_modulus_stays_in_the_slot() {
    let dir = project_with(&modulus_cfg(2));
    dir.child("opys/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [auth]\n---\n# Login\n")
        .unwrap();
    dir.child("opys/TASK-0001.md")
        .write_str("---\nid: TASK-0001\nstatus: todo\ntags: []\n---\n# Do it\n")
        .unwrap();
    // max 1, slot 1 of 2: the first congruent number above it is 3.
    opys(&dir)
        .env("OPYS_ID_SLOT", "1")
        .args(["--no-sync", "renumber"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0001 → TASK-0003"));
}

// ---------------------------------------------------------------------------
// `opys log` — FEAT-0105
// ---------------------------------------------------------------------------

/// A project with a feature (FEAT-0001) and a task (TASK-0002) linked to it.
fn work_item_project() -> TempDir {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "core"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire it",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    dir
}

fn read(dir: &TempDir, rel: &str) -> String {
    std::fs::read_to_string(dir.child(rel).path()).unwrap()
}

/// The entry lands in the declared log section and **nothing else in the file
/// moves** — asserted byte-for-byte, including the blank line separating the
/// log section from the one after it.
#[test]
fn log_appends_a_dated_bullet_and_changes_nothing_else() {
    let dir = work_item_project();
    let path = "opys/work-items/TASK-0002.md";
    // A section after the log one, so the separator between them is at stake.
    let seeded = format!("{}\n## Notes\nkeep me\n", read(&dir, path));
    std::fs::write(dir.child(path).path(), &seeded).unwrap();

    opys(&dir)
        .args(["log", "TASK-0002", "first thing"])
        .assert()
        .success()
        .stdout("TASK-0002 logged to '## Progress'\n");
    let once = seeded.replace(
        "## Progress\n",
        &format!("## Progress\n- {PINNED_NOW} first thing\n"),
    );
    assert_eq!(read(&dir, path), once);

    // A second entry goes under the first, still without touching the rest.
    opys(&dir)
        .env("OPYS_NOW", "2026-03-04T05:06:07Z")
        .args(["log", "TASK-0002", "second thing"])
        .assert()
        .success();
    assert_eq!(
        read(&dir, path),
        once.replace(
            "first thing\n",
            "first thing\n- 2026-03-04T05:06:07Z second thing\n",
        )
        .replace(
            &format!("updated: \"{PINNED_NOW}\""),
            "updated: \"2026-03-04T05:06:07Z\"",
        )
    );
    opys(&dir).arg("verify").assert().success();
}

/// A type with no `log` section has nowhere to put the entry: exit 2, naming
/// the type rather than inventing a section.
#[test]
fn log_on_a_type_without_a_log_section_exits_2() {
    let dir = work_item_project();
    opys(&dir)
        .args(["log", "FEAT-0001", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "type 'feature' declares no log section",
        ));
    // And an id whose prefix is nobody's.
    opys(&dir)
        .args(["log", "NOPE-0001", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unrecognized id prefix"));
}

/// Two log sections: `--section` picks one, and leaving it out is an error that
/// names both.
#[test]
fn log_needs_a_section_when_the_type_declares_two() {
    let cfg = format!(
        "{}\n[types.note]\nprefix = \"NOTE\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n\
[[types.note.sections]]\nheading = \"Progress\"\nkind = \"log\"\nrequired = true\n\
[[types.note.sections]]\nheading = \"Decisions\"\nkind = \"log\"\nrequired = true\n",
        default_cfg()
    );
    let dir = project_with(&cfg);
    opys(&dir)
        .args(["new", "--type", "note", "--title", "Scratch"])
        .assert()
        .success();
    opys(&dir)
        .args(["log", "NOTE-0001", "which one?"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "type 'note' has 2 log sections (Progress, Decisions) — pass --section",
        ));
    opys(&dir)
        .args([
            "log",
            "NOTE-0001",
            "chose the simple thing",
            "--section",
            "Decisions",
        ])
        .assert()
        .success()
        .stdout("NOTE-0001 logged to '## Decisions'\n");
    let text = read(&dir, "opys/NOTE-0001.md");
    assert!(
        text.contains(&format!(
            "## Decisions\n- {PINNED_NOW} chose the simple thing\n"
        )),
        "{text}"
    );
    // The other log section is untouched.
    assert!(text.contains("## Progress\n\n"), "{text}");
    // A `--section` that names a section of another kind is still refused.
    opys(&dir)
        .args(["log", "NOTE-0001", "x", "--section", "Nowhere"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("is not a log section"));
}

/// `-` means stdin only when it is the whole argument: an entry that merely
/// contains a dash is text. Nothing should be read from stdin here, so the
/// test gives the child no stdin to read.
#[test]
fn log_treats_a_dash_inside_text_as_text() {
    let dir = work_item_project();
    opys(&dir)
        .args(["log", "TASK-0002", " - still a note"])
        .assert()
        .success();
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains(format!(
            "- {PINNED_NOW} - still a note"
        )));
}

/// `-` reads the entry from stdin; continuation lines are indented two spaces
/// under the bullet so the whole note is one entry.
#[test]
fn log_indents_a_multi_line_entry_from_stdin() {
    let dir = work_item_project();
    opys(&dir)
        .args(["log", "TASK-0002", "-"])
        .write_stdin("chose the flock\nbecause a lease file can go stale\n\nrejected: a lock dir\n")
        .assert()
        .success();
    let text = read(&dir, "opys/work-items/TASK-0002.md");
    assert!(
        text.contains(&format!(
            "## Progress\n- {PINNED_NOW} chose the flock\n  because a lease file can go stale\n\n  rejected: a lock dir\n"
        )),
        "{text}"
    );
    // One entry, not four.
    opys(&dir)
        .args([
            "query",
            "SELECT items FROM sections WHERE doc_id = 'TASK-0002' AND kind = 'log'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1"));
    opys(&dir).arg("verify").assert().success();

    // Empty text is refused rather than writing a bare stamp.
    opys(&dir)
        .args(["log", "TASK-0002", "-"])
        .write_stdin("   \n\n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs some text"));
}

/// The write is an ordinary mutating command: `updated` moves and the sync pass
/// runs over the entry, so a bare id in it becomes a link.
#[test]
fn log_bumps_updated_and_linkifies_a_bare_id() {
    let dir = work_item_project();
    opys(&dir)
        .env("OPYS_NOW", "2026-05-06T07:08:09Z")
        .args(["log", "TASK-0002", "blocked on FEAT-0001 landing"])
        .assert()
        .success();
    let text = read(&dir, "opys/work-items/TASK-0002.md");
    assert!(text.contains("updated: \"2026-05-06T07:08:09Z\""), "{text}");
    assert!(
        text.contains("[FEAT-0001 — Alpha](../features/FEAT-0001.md) landing"),
        "{text}"
    );

    // `--no-sync` is honoured: the mention stays bare.
    opys(&dir)
        .env("OPYS_NOW", "2026-05-06T07:08:10Z")
        .args(["--no-sync", "log", "TASK-0002", "still on FEAT-0001"])
        .assert()
        .success();
    let text = read(&dir, "opys/work-items/TASK-0002.md");
    assert!(text.contains("still on FEAT-0001\n"), "{text}");
}

/// A section the type declares but the document lacks is created at the end of
/// the body rather than losing the entry.
#[test]
fn log_creates_the_declared_section_when_the_document_lacks_it() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "core"])
        .assert()
        .success();
    dir.child("opys/work-items/TASK-0002.md")
        .write_str(&format!(
            "---\nid: TASK-0002\nstatus: todo\ncreated: \"{PINNED_NOW}\"\nupdated: \"{PINNED_NOW}\"\n{}references:\n  FEAT-0001: Alpha\n---\n\n# Hand written\n\n## Tasks\n- [ ] do it\n",
            hist("todo")
        ))
        .unwrap();
    opys(&dir)
        .args(["log", "TASK-0002", "starting"])
        .assert()
        .success()
        .stdout("TASK-0002 logged to '## Progress' (section created)\n");
    let text = read(&dir, "opys/work-items/TASK-0002.md");
    assert!(
        text.ends_with(&format!(
            "## Tasks\n- [ ] do it\n\n## Progress\n- {PINNED_NOW} starting\n"
        )),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// verify's two log checks: an undated bullet and a stamp that goes backwards.
/// Two entries sharing a stamp are legal — the check is non-decreasing, not
/// strictly increasing.
#[test]
fn verify_flags_an_undated_and_a_backwards_log_entry() {
    let dir = project();
    enable_work_items(&dir);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "core"])
        .assert()
        .success();
    let write = |progress: &str| {
        dir.child("opys/work-items/TASK-0002.md")
            .write_str(&format!(
                "---\nid: TASK-0002\nstatus: todo\ncreated: \"{PINNED_NOW}\"\nupdated: \"{PINNED_NOW}\"\n{}references:\n  FEAT-0001: Alpha\n---\n\n# Hand written\n\n## Tasks\n- [ ] do it\n\n## Progress\n{progress}",
                hist("todo")
            ))
            .unwrap();
    };

    write("- 2026-08-24: legacy dated form\n");
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "TASK-0002: section 'Progress': undated log entry '2026-08-24: legacy dated form'",
        ));

    write("- 2026-01-02T03:04:05Z later\n- 2026-01-01T00:00:00Z earlier\n");
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
        "log entries must not go backwards ('2026-01-01T00:00:00Z' follows '2026-01-02T03:04:05Z')",
    ));

    // Equal stamps are fine: two `opys log` calls in one second, or a migration
    // dating a document's bullets from its `created`.
    write("- 2026-01-02T03:04:05Z one\n- 2026-01-02T03:04:05Z two\n");
    opys(&dir).arg("verify").assert().success();
}

/// The shipped `require_log_since_status` rule: `review` needs a checkpoint,
/// and a `review → in-progress → review` bounce needs a fresh one each time.
#[test]
fn review_refuses_without_a_fresh_log_entry_and_accepts_with_one() {
    let dir = work_item_project();
    // Nothing logged yet.
    opys(&dir)
        .env("OPYS_NOW", "2026-02-01T00:00:00Z")
        .args(["set-status", "TASK-0002", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "'## Progress' has no log entry since the status last changed",
        ));

    opys(&dir)
        .env("OPYS_NOW", "2026-02-01T01:00:00Z")
        .args(["log", "TASK-0002", "ready for a look"])
        .assert()
        .success();
    opys(&dir)
        .env("OPYS_NOW", "2026-02-01T02:00:00Z")
        .args(["set-status", "TASK-0002", "review"])
        .assert()
        .success();
    // The verdict is the same after the transition's own history entry lands.
    opys(&dir).arg("verify").assert().success();

    // Bounce back out and in: the entry that satisfied the first visit is stale
    // for the second.
    opys(&dir)
        .env("OPYS_NOW", "2026-02-02T00:00:00Z")
        .args(["set-status", "TASK-0002", "in-progress"])
        .assert()
        .success();
    opys(&dir)
        .env("OPYS_NOW", "2026-02-02T01:00:00Z")
        .args(["set-status", "TASK-0002", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no log entry since"));
    opys(&dir)
        .env("OPYS_NOW", "2026-02-02T02:00:00Z")
        .args(["log", "TASK-0002", "second look, addressed the comments"])
        .assert()
        .success();
    opys(&dir)
        .env("OPYS_NOW", "2026-02-02T03:00:00Z")
        .args(["set-status", "TASK-0002", "review"])
        .assert()
        .success();
    opys(&dir).arg("verify").assert().success();
}

// ---------------------------------------------------------------------------
// The status graph: `[types.X.transitions]` (ADR-0099 decision 1)
// ---------------------------------------------------------------------------

/// A `task` type with a declared status graph. `todo` is the initial status and
/// only `review` may close; `blocked` is a deliberate dead end, so a document
/// that reaches it can go nowhere without a config change — the shape the engine
/// has to be able to express.
const GRAPH_CFG: &str = "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"doing\", \"review\", \"blocked\", \"done\"]\n\
default_status = \"todo\"\n\
terminal_statuses = [\"done\"]\n\
[types.task.transitions]\n\
todo = [\"doing\"]\n\
doing = [\"review\", \"todo\"]\n\
review = [\"done\", \"doing\"]\n";

/// A type that declares no graph keeps the any-to-any rule byte-identically —
/// the promise that makes `transitions` additive for every config that predates
/// it. (The rest of this file is the wide version of this test: every fixture
/// here declares no graph.)
#[test]
fn a_type_without_transitions_accepts_any_status_change() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    for status in ["partial", "planned", "partial"] {
        opys(&dir)
            .args(["set-status", "FEAT-0001", status])
            .assert()
            .success()
            .stdout(predicate::str::contains(format!("FEAT-0001 -> {status}")));
    }
    opys(&dir).arg("verify").assert().success();
}

/// An undeclared move is refused before anything is written, exit 2, naming
/// where the document stands and where it may go.
#[test]
fn set_status_refuses_an_undeclared_edge() {
    let dir = project_with(GRAPH_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Work"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "TASK-0001", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot move TASK-0001 from todo to review: type 'task' declares no such transition \
             (from todo: doing)",
        ));
    // Nothing was written: the document is still where it was, with one entry.
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(text.contains("status: todo"), "{text}");
    assert_eq!(text.matches("\": ").count(), 1, "{text}");
    // And the declared edge goes through.
    opys(&dir)
        .args(["set-status", "TASK-0001", "doing"])
        .assert()
        .success();
    opys(&dir).arg("verify").assert().success();
}

/// A status that is not a key in a non-empty map is a dead end, not a wildcard:
/// half a graph must not leave every unlisted status wide open.
#[test]
fn set_status_refuses_every_move_out_of_a_dead_end() {
    let dir = project_with(GRAPH_CFG);
    dir.child("opys/TASK-0001.md")
        .write_str(&format!(
            "---\nid: TASK-0001\nstatus: blocked\ncreated: \"{PINNED_NOW}\"\n{}---\n\n# Work\n",
            hist("blocked")
        ))
        .unwrap();
    opys(&dir)
        .args(["set-status", "TASK-0001", "doing"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot move TASK-0001 from blocked to doing: type 'task' declares no such transition \
             (no transitions out of blocked)",
        ));
}

/// Setting a document to the status it already holds has always been a legal
/// no-op that appends an entry. A graph must not turn it into a refusal for want
/// of a self-edge nobody would think to declare — and the resulting equal pair
/// must stay verify-clean, or a successful write would leave a red corpus.
#[test]
fn set_status_to_the_current_status_survives_a_graph() {
    let dir = project_with(GRAPH_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Work"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "TASK-0001", "todo"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert_eq!(text.matches("\": todo").count(), 2, "{text}");
    opys(&dir).arg("verify").assert().success();
}

/// `close` is the door to a terminal status, so the graph gates it too: without
/// this the terminal edges a type declares would mean nothing.
#[test]
fn close_needs_an_edge_into_a_terminal_status() {
    let dir = project_with(GRAPH_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Work"])
        .assert()
        .success();
    opys(&dir)
        .args(["close", "TASK-0001"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot close TASK-0001 from todo: type 'task' declares no transition from there to \
             a terminal status (done) — only review can close",
        ));
    // `--force` is about unchecked checklist items; it does not open the door.
    opys(&dir)
        .args(["close", "TASK-0001", "--force"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("only review can close"));
    for status in ["doing", "review"] {
        opys(&dir)
            .args(["set-status", "TASK-0001", status])
            .assert()
            .success();
    }
    opys(&dir).args(["close", "TASK-0001"]).assert().success();
    dir.child("opys/TASK-0001.md")
        .assert(predicate::path::missing());
}

/// The graph is a CI check as well as a write-time guard: a hand-edited document
/// whose history records a move the type does not declare is a verify problem —
/// and the very same document is clean under a config that declares no graph.
#[test]
fn verify_flags_a_history_pair_that_is_not_a_declared_edge() {
    let doc = format!(
        "---\nid: TASK-0001\nstatus: review\ncreated: \"{PINNED_NOW}\"\n\
         history:\n  \"{PINNED_KEY}\": todo\n  \"2026-01-02T03:04:06.000000Z\": review\n\
         ---\n\n# Work\n"
    );
    let dir = project_with(GRAPH_CFG);
    dir.child("opys/TASK-0001.md").write_str(&doc).unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "TASK-0001: history records todo -> review, which type 'task' does not declare as a \
             transition (from todo: doing)",
        ));

    // The same bytes under a graph-less type: accepted, byte-identically to the
    // world before transitions existed.
    let no_graph = GRAPH_CFG
        .split("[types.task.transitions]")
        .next()
        .expect("the config starts before the transitions table");
    let dir = project_with(no_graph);
    dir.child("opys/TASK-0001.md").write_str(&doc).unwrap();
    opys(&dir).arg("verify").assert().success();
}

/// `import` seeds a document wherever its record says, and leaves no marker
/// saying it did — so verify checks *moves*, never where a document entered the
/// graph. An imported document that starts at a non-initial status is clean.
#[test]
fn an_imported_document_seeded_off_the_initial_status_verifies() {
    let dir = project_with(GRAPH_CFG);
    let jsonl = dir.child("tasks.jsonl");
    jsonl
        .write_str("{\"title\": \"Seeded mid-flight\", \"status\": \"review\"}\n")
        .unwrap();
    opys(&dir)
        .args(["import", "--type", "task", jsonl.path().to_str().unwrap()])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(
        text.contains(&format!("history:\n  \"{PINNED_KEY}\": review\n")),
        "{text}"
    );
    opys(&dir).arg("verify").assert().success();
}

/// A graph the config cannot mean is a config problem, reported by `verify` and
/// `config validate` like every other one — not a hard error at open time, which
/// would change the exit code of every command over a typo.
#[test]
fn config_validate_flags_a_broken_transitions_map() {
    let dir = project_with(
        "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"doing\", \"done\"]\ndefault_status = \"todo\"\n\
terminal_statuses = [\"done\"]\n\
[types.task.transitions]\n\
doing = [\"shipped\"]\n\
done = [\"todo\"]\n",
    );
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "transition 'doing' -> 'shipped' targets a status the type does not declare",
        ))
        .stderr(predicate::str::contains(
            "terminal status 'done' must have no outgoing transitions",
        ))
        .stderr(predicate::str::contains(
            "default_status 'todo' has no outgoing transitions",
        ));
}

// ---------------------------------------------------------------------------
// Time-aware verify: `[types.X.ttl]`, `[types.X.max_visits]`, exit 3
// (ADR-0101 / FEAT-0108)
// ---------------------------------------------------------------------------

/// A `task` type with both observations configured, a graph that declares each
/// configured `then` *and* the edge back from it (the recovery edge
/// `config validate` requires), the escalation edge `doing -> user-review` (the
/// budget's refusal fires on the *entry* into `review`, so every status that may
/// enter it has to be able to take the escape), and a `_stuck/` directory, so
/// the intended moves are all legal ones.
const TTL_CFG: &str = "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"doing\", \"review\", \"stuck\", \"user-review\", \"done\"]\n\
default_status = \"todo\"\n\
terminal_statuses = [\"done\"]\n\
status_dirs = { stuck = \"_stuck\" }\n\
[types.task.transitions]\n\
todo = [\"doing\"]\n\
doing = [\"review\", \"todo\", \"stuck\", \"user-review\"]\n\
review = [\"done\", \"doing\", \"user-review\"]\n\
stuck = [\"doing\", \"todo\"]\n\
user-review = [\"review\", \"doing\"]\n\
[types.task.ttl]\n\
doing = { after = \"10m\", then = \"stuck\" }\n\
[types.task.max_visits]\n\
review = { limit = 3, then = \"user-review\" }\n";

/// The history key of the entry `set-status` writes at [`PINNED_NOW`], one
/// microsecond past the `new` that seeded the document at the same instant.
const SECOND_KEY: &str = "2026-01-02T03:04:05.000001Z";
/// Well past the 10m `ttl` of `TTL_CFG`'s `doing`.
const LATE_NOW: &str = "2026-01-02T03:20:00Z";

/// A `TTL_CFG` project holding one task that entered `doing` at [`PINNED_NOW`].
fn ttl_project() -> TempDir {
    let dir = project_with(TTL_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Work"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "TASK-0001", "doing"])
        .assert()
        .success();
    dir
}

/// A document past its status's `ttl` is an *observation*, not a problem: its
/// own block, its own exit code, and nothing on the `verify: OK` line's stream.
#[test]
fn a_stuck_document_exits_three() {
    let dir = ttl_project();
    // Nothing has elapsed yet, and the boundary is strict: exactly `after` past
    // the entry is not stuck.
    opys(&dir).arg("verify").assert().success();
    opys(&dir)
        .env("OPYS_NOW", "2026-01-02T03:14:05.000001Z")
        .arg("verify")
        .assert()
        .success();
    opys(&dir)
        .env("OPYS_NOW", "2026-01-02T03:14:05.000002Z")
        .arg("verify")
        .assert()
        .code(3);
    opys(&dir)
        .env("OPYS_NOW", LATE_NOW)
        .arg("verify")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("verify: OK").not())
        .stderr(predicate::str::contains("verify: 1 observation(s)"))
        .stderr(predicate::str::contains(format!(
            "TASK-0001: stuck in doing since {SECOND_KEY} (ttl 10m) — configured target: stuck"
        )));
    // Verify wrote nothing: the observation names where the config intends the
    // document to go, and moving it there is nobody in opys's job (ADR-0101,
    // "Pending move to kobzar").
    dir.child("opys/TASK-0001.md")
        .assert(predicate::path::exists());
    dir.child("opys/_stuck/TASK-0001.md")
        .assert(predicate::path::missing());
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(text.contains("status: doing"), "{text}");
}

/// Observations never change the exit-code class of content problems: a corpus
/// with both is exit 1, and both blocks are printed.
#[test]
fn a_problem_beside_a_stuck_document_exits_one() {
    let dir = ttl_project();
    dir.child("opys/TASK-0002.md")
        .write_str(&format!(
            "---\nid: TASK-0002\nstatus: todo\ncreated: \"{PINNED_NOW}\"\nbogus: 1\n{}---\n\n# Other\n",
            hist("todo")
        ))
        .unwrap();
    opys(&dir)
        .env("OPYS_NOW", LATE_NOW)
        .arg("verify")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("verify: 1 problem(s)"))
        .stderr(predicate::str::contains(
            "TASK-0002: unknown frontmatter field 'bogus'",
        ))
        .stderr(predicate::str::contains("verify: 1 observation(s)"))
        .stderr(predicate::str::contains("TASK-0001: stuck in doing"));
}

/// The visit budget: `limit = 3` allows three entries into `review`, the third
/// *is* the limit — `set-status` refuses a fourth naming `then`, and verify
/// names the document as overflowed with that same `then` as its configured
/// target.
#[test]
fn the_entry_past_max_visits_is_refused_and_the_document_is_named_as_overflowed() {
    let dir = project_with(TTL_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Ping pong"])
        .assert()
        .success();
    for status in ["doing", "review", "doing", "review", "doing", "review"] {
        opys(&dir)
            .args(["set-status", "TASK-0001", status])
            .assert()
            .success();
    }
    // The fourth entry is refused — by the budget, not the graph: the document
    // is *at* `review`, and a move to the current status has always been legal.
    opys(&dir)
        .args(["set-status", "TASK-0001", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot set TASK-0001 to review: type 'task' allows 3 visit(s) to review and this \
             document has had 3 — max_visits sends it to user-review instead",
        ));
    // …and the same document is what verify names, at any clock.
    opys(&dir)
        .arg("verify")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "TASK-0001: 3 visits to review reaches its limit of 3 — configured target: \
             user-review",
        ));
    // Verify only *names* the target; taking it is a `set-status` somebody else
    // decides to run.
    opys(&dir)
        .args(["set-status", "TASK-0001", "user-review"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(text.contains("status: user-review"), "{text}");
    assert_eq!(text.matches("\": review").count(), 3, "{text}");
    // The budget is spent for good: `user-review -> review` is a declared edge,
    // and the guard still refuses it. `then` is where a human takes over.
    opys(&dir)
        .args(["set-status", "TASK-0001", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "max_visits sends it to user-review",
        ));
}

/// The same bytes are clean or stuck depending only on `OPYS_NOW`: no wall
/// clock reaches the observation, so one commit verifies the same way twice.
#[test]
fn observations_follow_opys_now_not_the_wall_clock() {
    let dir = ttl_project();
    for (now, code) in [
        ("2025-01-01T00:00:00Z", 0),
        (PINNED_NOW, 0),
        ("2026-01-02T03:13:00Z", 0),
        (LATE_NOW, 3),
        ("2099-01-01T00:00:00Z", 3),
    ] {
        opys(&dir)
            .env("OPYS_NOW", now)
            .arg("verify")
            .assert()
            .code(code);
    }
}

/// A `ttl` written as a bare duration names no target at all: the project has
/// asked to be told, and nothing more. The line says so, and the exit code is
/// the same `3` — an observation with a target is no more actionable by opys
/// than one without, because opys acts on neither.
#[test]
fn a_bare_duration_ttl_is_observed_with_no_configured_target() {
    let dir = project_with(
        "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"idle\", \"done\"]\n\
default_status = \"todo\"\nterminal_statuses = [\"done\"]\n\
[types.task.transitions]\n\
todo = [\"idle\"]\n\
idle = [\"todo\", \"done\"]\n\
[types.task.ttl]\n\
idle = \"10m\"\n",
    );
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Idling"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "TASK-0001", "idle"])
        .assert()
        .success();
    opys(&dir)
        .env("OPYS_NOW", LATE_NOW)
        .arg("verify")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!(
            "TASK-0001: stuck in idle since {SECOND_KEY} (ttl 10m) — no configured target"
        )));
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(text.contains("status: idle"), "{text}");
}

/// The move a `ttl` names must be safe to apply under a running agent, so the
/// type has to declare the way back from where it sends the document
/// (ADR-0101 decision 6).
#[test]
fn config_validate_flags_a_ttl_target_with_no_way_back() {
    let dir = project_with(
        "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"doing\", \"stuck\", \"done\"]\ndefault_status = \"todo\"\n\
terminal_statuses = [\"done\"]\n\
[types.task.transitions]\n\
todo = [\"doing\"]\n\
doing = [\"stuck\", \"done\"]\n\
[types.task.ttl]\n\
doing = { after = \"10m\", then = \"stuck\" }\n\
[types.task.max_visits]\n\
doing = { limit = 2, then = \"done\" }\n",
    );
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "type 'task': ttl 'doing' -> 'stuck' has no way back ('stuck' declares no transition \
             to 'doing' — the recovery edge a move under a running agent relies on)",
        ))
        .stderr(predicate::str::contains(
            "type 'task': max_visits 'doing' -> 'done' targets a terminal status",
        ));
}

/// The observation block is byte-stable — the same corpus, the same clock, the
/// same bytes, twice — and it is a block of its own, after the problems.
#[test]
fn the_observations_block_is_byte_stable_and_separate_from_the_problems() {
    let dir = project_with(TTL_CFG);
    for title in ["Stuck one", "Ping pong"] {
        opys(&dir)
            .args(["new", "--type", "task", "--title", title])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["set-status", "TASK-0001", "doing"])
        .assert()
        .success();
    for status in ["doing", "review", "doing", "review", "doing", "review"] {
        opys(&dir)
            .args(["set-status", "TASK-0002", status])
            .assert()
            .success();
    }
    let expected = format!(
        "verify: 2 observation(s)\n  \
         TASK-0001: stuck in doing since {SECOND_KEY} (ttl 10m) — configured target: stuck\n  \
         TASK-0002: 3 visits to review reaches its limit of 3 — configured target: user-review\n"
    );
    let run = || {
        let out = opys(&dir)
            .env("OPYS_NOW", LATE_NOW)
            .arg("verify")
            .assert()
            .code(3)
            .get_output()
            .stderr
            .clone();
        String::from_utf8(out).unwrap()
    };
    assert_eq!(run(), expected);
    assert_eq!(run(), expected);

    // A problem does not merge into the block: it keeps its own heading, first.
    dir.child("opys/TASK-0003.md")
        .write_str(&format!(
            "---\nid: TASK-0003\nstatus: todo\ncreated: \"{PINNED_NOW}\"\nbogus: 1\n{}---\n\n# Other\n",
            hist("todo")
        ))
        .unwrap();
    let out = opys(&dir)
        .env("OPYS_NOW", LATE_NOW)
        .arg("verify")
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with("verify: 1 problem(s)\n  TASK-0003:"),
        "{out}"
    );
    assert!(out.ends_with(&expected), "{out}");
}

/// The visit budget counts entries into the *target* status, so it refuses a
/// move from anywhere — while [`observe::overflow`] keys on the status the
/// document *holds*, so verify has nothing to observe here at all. That
/// asymmetry is only safe because `config validate` requires the
/// **escalation edge**: the `then` the refusal advertises has to be a legal move
/// from every status that may enter the budgeted one.
#[test]
fn a_spent_budget_refuses_re_entry_from_elsewhere_and_the_escape_is_a_legal_move() {
    let dir = project_with(TTL_CFG);
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Ping pong"])
        .assert()
        .success();
    for status in [
        "doing", "review", "doing", "review", "doing", "review", "doing",
    ] {
        opys(&dir)
            .args(["set-status", "TASK-0001", status])
            .assert()
            .success();
    }
    // Three visits to `review`, and the document now stands at `doing`.
    opys(&dir)
        .args(["set-status", "TASK-0001", "review"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot set TASK-0001 to review: type 'task' allows 3 visit(s) to review and this \
             document has had 3 — max_visits sends it to user-review instead",
        ));
    // Not an observation: the budgeted status is not the one it holds.
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stderr(predicate::str::contains("observation(s)").not());
    // The escape the refusal named is a move the document can actually make.
    opys(&dir)
        .args(["set-status", "TASK-0001", "user-review"])
        .assert()
        .success();
    let text = std::fs::read_to_string(dir.path().join("opys/TASK-0001.md")).unwrap();
    assert!(text.contains("status: user-review"), "{text}");
}

/// …and a graph that does not declare that edge is a config problem, not a trap
/// discovered on the day the budget runs out.
#[test]
fn config_validate_flags_a_max_visits_escape_a_predecessor_cannot_take() {
    let dir = project_with(
        "pad = 4\n\
[types.task]\nprefix = \"TASK\"\n\
statuses = [\"todo\", \"doing\", \"review\", \"user-review\", \"done\"]\n\
default_status = \"todo\"\nterminal_statuses = [\"done\"]\n\
[types.task.transitions]\n\
todo = [\"doing\"]\n\
doing = [\"review\", \"todo\"]\n\
review = [\"doing\", \"user-review\", \"done\"]\n\
user-review = [\"review\", \"doing\"]\n\
[types.task.max_visits]\n\
review = { limit = 3, then = \"user-review\" }\n",
    );
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "type 'task': max_visits 'review' -> 'user-review': a document at 'doing' may enter \
             'review' but not 'user-review' — once the budget is spent the refusal would name \
             an escape it cannot take",
        ));
}

// ---------------------------------------------------------------------------
// Derived blocked: `releases_blocks` and `list --blocked` (ADR-0099 decision 3)
// ---------------------------------------------------------------------------

/// A permanent `feature` that releases what it blocks once it is `implemented`,
/// and an ephemeral `task` that declares nothing — it releases by construction,
/// because `close` deletes it and strikes every entry pointing at it.
const RELEASE_CFG: &str = r#"pad = 4
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented"]
default_status = "planned"
releases_blocks = ["implemented"]
[types.task]
prefix = "TASK"
statuses = ["todo", "done"]
default_status = "todo"
terminal_statuses = ["done"]
"#;

/// Write `<id>.md` titled `title` at `status`, optionally carrying a
/// `blocked_by` map, with the one history entry `verify` wants. Hand-written
/// rather than driven through `new`, so a struck tombstone and a released
/// blocker can both be set up without closing anything.
///
/// The frontmatter is *canonical* — keys in serialization order, `created` and
/// `updated` both present — and the title is never the document's own id, so a
/// sync pass over this fixture has nothing to backfill, restamp or linkify.
/// That is what lets a test assert one of these files is byte-identical after a
/// write elsewhere in the corpus.
fn blocker_doc(dir: &TempDir, id: &str, title: &str, status: &str, blocked_by: &str) {
    dir.child(format!("opys/{id}.md"))
        .write_str(&format!(
            "---\nid: {id}\nstatus: {status}\ncreated: \"{PINNED_NOW}\"\n\
             updated: \"{PINNED_NOW}\"\n{blocked_by}{}---\n\n# {title}\n",
            hist(status)
        ))
        .unwrap();
}

/// The shapes a `blocked_by` entry can have, end to end through `list`. Only a
/// live, non-struck entry whose target has not released still blocks.
#[test]
fn list_blocked_reads_the_live_blockers() {
    let dir = project_with(RELEASE_CFG);
    blocker_doc(&dir, "FEAT-0001", "Engine", "implemented", "");
    blocker_doc(&dir, "FEAT-0002", "Parser", "planned", "");
    // Blocked by a feature that has released.
    blocker_doc(
        &dir,
        "TASK-0003",
        "Wire the engine",
        "todo",
        "blocked_by:\n  FEAT-0001: Engine\n",
    );
    // Blocked by a feature that has not.
    blocker_doc(
        &dir,
        "TASK-0004",
        "Wire the parser",
        "todo",
        "blocked_by:\n  FEAT-0002: Parser\n",
    );
    // Blocked by a *closed* task: the struck tombstone reserves the id and
    // nothing more.
    blocker_doc(
        &dir,
        "TASK-0005",
        "Follow up",
        "todo",
        "blocked_by:\n  TASK-0009: '~~Gone~~'\n",
    );
    // Blocked by an id no document answers to: there is nothing left to wait
    // for (the dangling entry is a separate `verify` finding).
    blocker_doc(
        &dir,
        "TASK-0006",
        "Loose end",
        "todo",
        "blocked_by:\n  TASK-0008: Vanished\n",
    );

    opys(&dir)
        .args(["--no-sync", "list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("TASK-0004\n");
    opys(&dir)
        .args(["--no-sync", "list", "--unblocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("FEAT-0001\nFEAT-0002\nTASK-0003\nTASK-0005\nTASK-0006\n");
    // The filter composes with the others, and with neither flag nothing is cut.
    opys(&dir)
        .args([
            "--no-sync",
            "list",
            "--type",
            "task",
            "--unblocked",
            "--format",
            "ids",
        ])
        .assert()
        .success()
        .stdout("TASK-0003\nTASK-0005\nTASK-0006\n");
    opys(&dir)
        .args(["--no-sync", "list", "--format", "ids"])
        .assert()
        .success()
        .stdout("FEAT-0001\nFEAT-0002\nTASK-0003\nTASK-0004\nTASK-0005\nTASK-0006\n");
}

/// A blocker releases by *moving*, not by anyone editing the document it blocks:
/// the entry stays exactly where it is and stops counting.
#[test]
fn a_blocker_releases_when_it_reaches_a_releasing_status() {
    let dir = project_with(RELEASE_CFG);
    blocker_doc(&dir, "FEAT-0001", "Engine", "planned", "");
    blocker_doc(
        &dir,
        "TASK-0002",
        "Wire the engine",
        "todo",
        "blocked_by:\n  FEAT-0001: Engine\n",
    );
    opys(&dir)
        .args(["--no-sync", "list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("TASK-0002\n");

    let blocked_path = dir.path().join("opys/TASK-0002.md");
    let before = std::fs::read_to_string(&blocked_path).unwrap();
    opys(&dir)
        .args(["set-status", "FEAT-0001", "implemented"])
        .assert()
        .success();
    opys(&dir)
        .args(["list", "--blocked", "--format", "ids"])
        .assert()
        .success()
        .stdout("");
    // Nothing was written to the blocked document — not a cached `blocked:`,
    // not a status, not a history entry. Byte-identity is the whole claim: the
    // release happened on the blocker's side, and the reading of an untouched
    // relation changed with it. (The blocker itself does move: the sync pass
    // adds its reverse `blocks` entry.)
    let after = std::fs::read_to_string(&blocked_path).unwrap();
    assert_eq!(before, after, "the blocked document was rewritten");
    assert!(after.contains("FEAT-0001: Engine"), "{after}");
    opys(&dir).arg("verify").assert().success();
}

/// The two flags answer opposite questions, so asking both is a usage error clap
/// refuses before a document is loaded.
#[test]
fn list_blocked_and_unblocked_are_mutually_exclusive() {
    let dir = project_with(RELEASE_CFG);
    opys(&dir)
        .args(["list", "--blocked", "--unblocked"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

/// `releases_blocks` naming a status the type cannot hold is dead config, and
/// dead config here has no symptom: every blocker of that type would simply stay
/// live forever. `config validate` names it like any other config problem.
#[test]
fn config_validate_flags_a_releases_blocks_non_status() {
    let dir = project_with(
        "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\n\
statuses = [\"planned\", \"implemented\"]\ndefault_status = \"planned\"\n\
releases_blocks = [\"shipped\"]\n",
    );
    opys(&dir)
        .args(["config", "validate"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "type 'feature': releases_blocks 'shipped' is not a status",
        ));
}

/// `blocked_from` and `blocked_reason` were the two frontmatter keys the old
/// blocked *status* needed. Neither is reserved any more, and the shipped config
/// declares neither, so under it they are ordinary unknown keys — which is the
/// signal a corpus still carrying them has not been migrated.
#[test]
fn blocked_from_and_blocked_reason_are_unknown_keys_under_the_shipped_config() {
    let dir = TempDir::new().unwrap();
    opys(&dir).arg("init").assert().success();
    dir.child("opys/FEAT-0001.md")
        .write_str(&format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n\
             blocked_from: todo\nblocked_reason: waiting\n{}---\n\n# A\n",
            hist("planned")
        ))
        .unwrap();
    opys(&dir)
        .args(["--no-sync", "verify"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "FEAT-0001: unknown frontmatter field 'blocked_from' \
             (declare it in type 'feature' [fields.blocked_from])",
        ))
        .stderr(predicate::str::contains(
            "FEAT-0001: unknown frontmatter field 'blocked_reason'",
        ));
}

/// The shipped work-item graph, in the two facts a user meets first: `blocked`
/// is not a status any more, and `close` works only from `review`/`user-review`
/// — which is why the documented bulk pipe selects on `review`.
#[test]
fn the_shipped_work_item_graph_closes_only_from_review() {
    let dir = TempDir::new().unwrap();
    opys(&dir).arg("init").assert().success();
    opys(&dir)
        .args(["new", "--title", "Feature", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args([
            "new",
            "--type",
            "task",
            "--title",
            "Wire",
            "--features",
            "FEAT-0001",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "TASK-0002", "blocked"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "unknown status \"blocked\" for type 'task' \
             (allowed: todo, in-progress, review, user-review, done)",
        ));
    opys(&dir)
        .args(["close", "TASK-0002"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("review, user-review"));
}
