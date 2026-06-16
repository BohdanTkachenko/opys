//! End-to-end tests driving the `opys` binary against temp project dirs.

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;

/// Build an `opys.toml` with the standard `feature` type (dir `features/`),
/// splicing in `[tests]`/`[report]` blocks and extra `[fields.*]` tables.
fn opys_cfg(tests: &str, report: &str, fields: &str) -> String {
    format!(
        "pad = 4\n{tests}{report}\n\
[types.feature]\n\
prefix = \"FEAT\"\n\
dir = \"features\"\n\
statuses = [\"planned\", \"partial\", \"implemented\", \"wontfix\"]\n\
default_status = \"planned\"\n\
tags_required = true\n\
[types.feature.fields.spec]\ntype = \"string\"\n\
[types.feature.fields.wontfix_reason]\ntype = \"string\"\n\
{fields}\
[[types.feature.sections]]\nheading = \"Test plan\"\nkind = \"test-plan\"\n\
[[types.feature.sections]]\nheading = \"Manual verification\"\nkind = \"manual\"\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"wontfix\" }}\nrequire_field = \"wontfix_reason\"\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"implemented\" }}\nrequire_checked_section = \"Test plan\"\n"
    )
}

/// The default feature-only config (no test-ref checking, plus the `ptyxis_ref`
/// custom field used across tests).
fn default_cfg() -> String {
    opys_cfg(
        "[tests]\nreference_check = \"none\"\n",
        "",
        "[types.feature.fields.ptyxis_ref]\ntype = \"string\"\n",
    )
}

/// Feature config with `enum`/`list` custom fields, for the enum/filter tests.
fn enum_cfg() -> String {
    opys_cfg(
        "[tests]\nreference_check = \"none\"\n",
        "",
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
statuses = ["todo", "in-progress", "blocked", "done"]
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
statuses = ["todo", "in-progress", "blocked", "done"]
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
statuses = ["todo", "in-progress", "blocked", "done"]
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
"#;

fn opys(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("opys").unwrap();
    cmd.arg("--root").arg(dir.path());
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
fn new_auto_syncs_index() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    dir.child("opys/INDEX.md")
        .assert(predicate::str::contains("FEAT-0001"));
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
    dir.child("opys/_retired.txt")
        .assert(predicate::str::contains("FEAT-0001"));

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
    let text = std::fs::read_to_string(dir.child("opys/_retired.txt").path()).unwrap();
    let p1 = text.find("FEAT-0001").unwrap();
    let p3 = text.find("FEAT-0003").unwrap();
    assert!(p1 < p3, "retired ledger not sorted: {text:?}");
}

#[test]
fn verify_passes_clean_and_flags_violations() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [osc, tabs]\n---\n\n# Clean feature\n",
        )
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
        .stderr(predicate::str::contains("is not lowercase kebab-case"))
        .stderr(predicate::str::contains(
            "unknown frontmatter field 'bogus'",
        ))
        .stderr(predicate::str::contains(
            "'## Test plan' needs at least one checked item",
        ));
}

#[test]
fn new_enforces_status_guards() {
    let dir = project();
    opys(&dir)
        .args([
            "new",
            "--title",
            "X",
            "--tags",
            "a",
            "--status",
            "implemented",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "'## Test plan' needs at least one checked item",
        ));
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a", "--status", "wontfix"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "field 'wontfix_reason' is required",
        ));
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
fn no_sync_skips_regeneration() {
    let dir = project();
    opys(&dir)
        .args(["--no-sync", "new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    dir.child("opys/INDEX.md")
        .assert(predicate::path::missing());
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
fn verify_checks_manual_item_shape() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n\n## Manual verification\n- Looks right — *manual: visual*\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains("manual item missing Setup"))
        .stderr(predicate::str::contains("missing numbered Steps"))
        .stderr(predicate::str::contains("missing Expect"));
}

#[test]
fn verify_ignores_prose_code_span_on_checked_item() {
    let dir = project_with(&opys_cfg(
        "[tests]\nsearch_paths = [\"src\"]\nreference_check = \"grep\"\n",
        "",
        "",
    ));
    dir.child("src/lib.rs")
        .write_str("fn sftp_uri_rewrites_to_ssh() {}\n")
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: implemented\ntags: [ssh]\n---\n\n# Sftp\n\n## Test plan\n- [x] sftp:// rewrites to `ssh -t exec $SHELL` not a path — `lib.rs::sftp_uri_rewrites_to_ssh`\n",
        )
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
        .stdout(predicate::str::contains(
            "imported 2 feature document(s): FEAT-0002..FEAT-0003",
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

#[test]
fn verify_extract_mode_resolves_real_tests() {
    let config = opys_cfg(
        "[tests]\nsearch_paths = [\"src\"]\nreference_check = \"extract\"\nname_pattern = \"fn\\\\s+(\\\\w+)\\\\s*\\\\(\"\n",
        "",
        "",
    );
    let dir = project_with(&config);
    dir.child("src/lib.rs")
        .write_str("fn real_test() {}\nfn another() {}\n")
        .unwrap();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: implemented\ntags: [a]\n---\n\n# Good\n\n## Test plan\n- [x] a — `mod::real_test`\n- [x] b — `src/lib.rs::another`\n")
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

#[test]
fn sync_regenerates_index() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [osc]\n---\n\n# One\n")
        .unwrap();
    opys(&dir).arg("sync").assert().success();
    dir.child("opys/INDEX.md")
        .assert(predicate::str::contains("FEAT-0001 [planned] (osc) One"));
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
        .stdout(predicate::str::contains("documents: 3"))
        .stdout(predicate::str::contains("feature: 3"))
        // planned 2/3 ≈ 67%, implemented 1/3 ≈ 33%
        .stdout(predicate::str::contains("planned"))
        .stdout(predicate::str::contains("67%"))
        .stdout(predicate::str::contains("parity").not());
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
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nreferences:\n  TASK-0099: ~~Ghost~~\n---\n\n# F\n",
        )
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
    let dir = project_with(&opys_cfg(
        "[tests]\nreference_check = \"none\"\n",
        "",
        "[types.feature.fields.bad]\ntype = \"enum\"\n",
    ));
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

#[test]
fn block_links_both_directions_and_sets_blocked_status() {
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

    // The blocked work item gained blocked_by and was auto-set to blocked.
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::str::contains("status: blocked"))
        .assert(predicate::str::contains("FEAT-0002: Beta"));
    // The blocker gained the reverse `blocks` edge.
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("blocks:"))
        .assert(predicate::str::contains("TASK-0003: Wire"));
    // No blocked_reason is needed — the blocker link satisfies the guard.
    opys(&dir).arg("verify").assert().success();

    // An item cannot block itself.
    opys(&dir)
        .args(["block", "FEAT-0001", "--by", "FEAT-0001"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot block itself"));
}

#[test]
fn unblock_removes_link_and_reverts_status() {
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
    // Both sides cleared, and the auto-blocked status reverted to in-progress.
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::str::contains("status: in-progress"))
        .assert(predicate::str::contains("blocked_by").not());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("blocks").not());
    opys(&dir).arg("verify").assert().success();

    // Unblocking an edge that does not exist is an error.
    opys(&dir)
        .args(["unblock", "TASK-0003", "--by", "FEAT-0002"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no blocker"));
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
        .assert(predicate::str::contains("kind = \"test-plan\""))
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
    // (The default opys.toml puts every type's docs in the shared `items/` dir.)
    dir.child("opys/items/FEAT-0001.md")
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
    dir.child("opys/items/FEAT-0001.md")
        .write_str(
            "---\nid: FEAT-0001\nstatus: archived\ntags: [a]\narchived_reason: removed in v3\n---\n\n# F\n",
        )
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
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\nticket: JIRA-42\n---\n\n# F\n")
        .unwrap();
    opys(&dir).arg("verify").assert().success();
}
