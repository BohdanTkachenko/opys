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
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [osc, area:parsing, priority=high]\n---\n\n# Clean\n",
        )
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
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n\n## Manual verification\n- Looks right\n",
        )
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
        .write_str(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n\n## Manual verification\n### Setup\n- a window\n\n### Procedure\n1. open it\n\n### Expectations\n- it renders\n",
        )
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
        .write_str(concat!(
            "---\nid: FEAT-0001\nstatus: implemented\ntags: [a]\n---\n\n",
            "# Cursor group\n\n",
            "## Code Pointers\n",
            "- `preferences/appearance.rs` — `cursor_group`: builds the group\n\n",
            "## Test plan\n",
            "- [x] exposes combos — `e2e::preferences::ocr::ocr_cursor_group_exposes_shape_and_blink_combos`\n",
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

/// `query --write` applies edit SQL when the result still passes verify.
#[test]
fn query_write_applies_valid_edits() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\", \"implemented\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    for n in ["0001", "0002", "0003"] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .write_str(&format!(
                "---\nid: FEAT-{n}\nstatus: planned\ntags: [x]\n---\n\n# F{n}\n"
            ))
            .unwrap();
    }
    opys(&dir)
        .args([
            "query",
            "--write",
            "UPDATE docs SET status = 'implemented' WHERE status = 'planned'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("3 updated"));
    for n in ["0001", "0002", "0003"] {
        dir.child(format!("opys/features/FEAT-{n}.md"))
            .assert(predicate::str::contains("status: implemented"));
    }
}

/// `query --write` refuses to write (and touches no files) when the result
/// would fail verify — the rules engine still gates SQL edits.
#[test]
fn query_write_gated_by_verify() {
    // wontfix requires a reason field (a [[rules]] guard).
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\", \"wontfix\"]\ndefault_status = \"planned\"\n\
[types.feature.fields.wontfix_reason]\ntype = \"string\"\n\
[[rules]]\nwhen = {{ type = \"feature\", status = \"wontfix\" }}\n\
require_field = \"wontfix_reason\"\n";
    let dir = project_with(&cfg.replace("{{", "{").replace("}}", "}"));
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "UPDATE docs SET status = 'wontfix' WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("refusing to write"))
        .stderr(predicate::str::contains("wontfix_reason"));
    // The file is untouched — the store mutation was never flushed.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
}

/// `query --write` allows only INSERT/UPDATE/DELETE: schema-level statements
/// (CREATE/DROP/ALTER TABLE) are rejected on the plan before anything runs, so
/// they can't reshape the authoritative tables the store and flush depend on.
/// A compound `UPDATE …; DROP TABLE docs` is rejected whole — no partial write.
#[test]
fn query_write_rejects_non_dml_statements() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n")
        .unwrap();
    for bad in [
        "CREATE TABLE evil (x INTEGER)",
        "DROP TABLE docs",
        "ALTER TABLE docs ADD COLUMN evil INTEGER",
        "UPDATE docs SET status = 'planned'; DROP TABLE docs",
        "SELECT 1",
    ] {
        opys(&dir)
            .args(["query", "--write", bad])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("allows only INSERT/UPDATE/DELETE"));
    }
    // The file was never touched.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("status: planned"));
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
    // Both sides cleared, and the auto-blocked status restored to the pre-block
    // status (the task was created at `todo`), with the bookkeeping field gone.
    dir.child("opys/work-items/TASK-0003.md")
        .assert(predicate::str::contains("status: todo"))
        .assert(predicate::str::contains("blocked_by").not())
        .assert(predicate::str::contains("blocked_from").not());
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

/// BUG-0033: unblock restores the exact pre-block status, not a hardcoded
/// `in-progress`.
#[test]
fn unblock_restores_pre_block_status() {
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
    // Advance the task, then block it — the pre-block status is `in-progress`.
    opys(&dir)
        .args(["set-status", "TASK-0002", "in-progress"])
        .assert()
        .success();
    opys(&dir)
        .args(["block", "TASK-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains("status: blocked"))
        .assert(predicate::str::contains("blocked_from: in-progress"));
    // Unblock restores in-progress (not todo/default), and clears the field.
    opys(&dir)
        .args(["unblock", "TASK-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    dir.child("opys/work-items/TASK-0002.md")
        .assert(predicate::str::contains("status: in-progress"))
        .assert(predicate::str::contains("blocked_from").not());
    opys(&dir).arg("verify").assert().success();
}

/// BUG-0033: a type with a `blocked` status but no `in-progress` unblocks back to
/// its default status (was left stuck at `blocked`, failing verify).
#[test]
fn unblock_without_in_progress_falls_back_to_default() {
    let cfg = format!(
        "{}\n{}",
        default_cfg(),
        "[types.job]\n\
prefix = \"JOB\"\n\
dir = \"work-items\"\n\
statuses = [\"open\", \"blocked\", \"done\"]\n\
default_status = \"open\"\n\
terminal_statuses = [\"done\"]\n\
[[types.job.sections]]\nheading = \"Tasks\"\nkind = \"checklist\"\nrequired = true\n\
[[rules]]\nwhen = { status = \"blocked\" }\n\
require_any = [{ link = \"blocked_by\" }]\n"
    );
    let dir = project_with(&cfg);
    opys(&dir)
        .args(["new", "--title", "Alpha", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--type", "job", "--title", "Do it"])
        .assert()
        .success();
    opys(&dir)
        .args(["block", "JOB-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    dir.child("opys/work-items/JOB-0002.md")
        .assert(predicate::str::contains("status: blocked"))
        .assert(predicate::str::contains("blocked_from: open"));
    opys(&dir)
        .args(["unblock", "JOB-0002", "--by", "FEAT-0001"])
        .assert()
        .success();
    dir.child("opys/work-items/JOB-0002.md")
        .assert(predicate::str::contains("status: open"));
    opys(&dir).arg("verify").assert().success();
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

#[test]
fn verify_rejects_malformed_timestamp_but_allows_absent() {
    let dir = project();
    // Absent timestamps (docs predating the fields) are allowed.
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n")
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
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n")
        .unwrap();
    opys(&dir).arg("sync").assert().success();
    // After a sync pass the doc carries both auto-maintained timestamps.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains("created:"))
        .assert(predicate::str::contains("updated:"));
    // And the backfilled values are valid RFC3339, so verify still passes.
    opys(&dir).arg("verify").assert().success();
}

// --- `opys history` (optional `history` feature) -------------------------------

/// Run a git command in `dir`, asserting success. Author identity is passed per
/// invocation so commits don't depend on ambient global git config.
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

#[cfg(feature = "history")]
#[test]
fn history_shows_status_timeline_with_authors() {
    let dir = project();
    git(&dir, &["init", "-q"]);

    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    git_commit(&dir, "Alice", "create feature");

    opys(&dir)
        .args(["set-status", "FEAT-0001", "partial"])
        .assert()
        .success();
    git_commit(&dir, "Bob", "move to partial");

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

#[cfg(feature = "history")]
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
    git(&dir, &["init", "-q"]);

    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    git_commit(&dir, "Alice", "create feature");
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());

    opys(&dir)
        .args(["set-status", "FEAT-0001", "archived"])
        .assert()
        .success();
    // The file physically moved into the `_archived/` subdir.
    dir.child("opys/features/_archived/FEAT-0001.md")
        .assert(predicate::path::exists());
    git_commit(&dir, "Bob", "archive feature");

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
#[cfg(feature = "history")]
#[test]
fn history_attributes_revision_to_the_introducing_commit() {
    let dir = project();
    git(&dir, &["init", "-q"]);
    opys(&dir)
        .args(["new", "--title", "Login", "--tags", "auth"])
        .assert()
        .success();
    git_commit(&dir, "Alice", "create feature");
    // Several unrelated commits on top, by a different author, touching other
    // files — the document blob is unchanged across all of them.
    for i in 0..3 {
        dir.child(format!("notes-{i}.txt")).write_str("x").unwrap();
        git_commit(&dir, "Mallory", "unrelated work");
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
fn space_separated_ids_are_rejected_not_silently_widened() {
    // Bulk is opt-in via commas. Space-separated ids (e.g. an unintended
    // `$(opys list --format ids)` expansion) must fail loudly rather than
    // operate on the whole set — the safety property of the single positional.
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    opys(&dir)
        .args(["close", "FEAT-0001", "FEAT-0002"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
    // Nothing was closed — both files are intact.
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::exists());
}

#[test]
fn bulk_is_best_effort_and_reports_failures() {
    let dir = project();
    for t in ["A", "B"] {
        opys(&dir)
            .args(["new", "--title", t, "--tags", "a"])
            .assert()
            .success();
    }
    // FEAT-0002 exists; FEAT-0099 does not. The valid one still moves, the
    // bad one is reported, and the command exits nonzero.
    opys(&dir)
        .args(["set-status", "FEAT-0002,FEAT-0099", "partial"])
        .assert()
        .failure()
        .code(2)
        .stdout(predicate::str::contains("FEAT-0002 -> partial"))
        .stderr(predicate::str::contains("FEAT-0099"))
        .stderr(predicate::str::contains("1 of 2 ids failed"));
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("status: partial"));
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

/// `query --write` DELETE of a referenced doc is a lifecycle removal: the file
/// is deleted, every inbound reference is struck to a `~~title~~` tombstone (so
/// nothing dangles and verify stays clean), and the id is reserved forever.
#[test]
fn query_write_delete_strikes_inbound_ref_and_reserves_id() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\", \"implemented\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [x]\n---\n\n# One\n")
        .unwrap();
    dir.child("opys/features/FEAT-0002.md")
        .write_str(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [x]\nreferences:\n  FEAT-0001: One\n---\n\n# Two\n",
        )
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "DELETE FROM docs WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 deleted"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("FEAT-0001: ~~One~~"));
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001"));
    opys(&dir).arg("verify").assert().success();
}

/// A DELETE of an *unreferenced* doc still reserves its id — striking inbound
/// refs alone wouldn't reserve a number nobody points at.
#[test]
fn query_write_delete_unreferenced_reserves_id() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [x]\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "DELETE FROM docs WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001"));
}

/// A reserved id is never handed back out: after deleting FEAT-0001, `new`
/// allocates FEAT-0002, not the freed number.
#[test]
fn query_write_delete_reserves_id_against_new_reuse() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [x]\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "DELETE FROM docs WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .success();
    opys(&dir)
        .args(["new", "--title", "Two", "--tags", "x"])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::missing());
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::exists());
}

/// `query --write` INSERT into docs auto-allocates the next id, defaults the
/// status, stamps timestamps, and scaffolds the type's required sections so the
/// new doc passes verify.
#[test]
fn query_write_insert_allocates_id_and_scaffolds() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n\
[[types.feature.sections]]\nheading = \"Test plan\"\nkind = \"checklist\"\nrequired = true\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# One\n\n## Test plan\n- [ ] a\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "INSERT INTO docs (type, title) VALUES ('feature', 'Auto')",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("inserted"));
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("id: FEAT-0002"));
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::str::contains("## Test plan"));
    opys(&dir).arg("verify").assert().success();
}

/// Multiple inserts in one statement get distinct sequential ids.
#[test]
fn query_write_insert_allocates_distinct_ids() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "INSERT INTO docs (type, title) VALUES ('feature', 'A'), ('feature', 'B')",
        ])
        .assert()
        .success();
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::exists());
    dir.child("opys/features/FEAT-0003.md")
        .assert(predicate::path::exists());
}

/// An INSERT with an unknown type is refused with no files changed.
#[test]
fn query_write_insert_rejects_unknown_type() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "INSERT INTO docs (type, title) VALUES ('nope', 'X')",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a known type"));
    dir.child("opys/features/FEAT-0002.md")
        .assert(predicate::path::missing());
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

/// `UPDATE blocks SET text` splices a section's content back into the body
/// byte-accurately (sibling sections untouched), gated by verify.
#[test]
fn query_write_blocks_splices_section_into_body() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# Login\n\n## Notes\nold note.\n\n## More\nkeep me.\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "UPDATE blocks SET text = 'new note.' WHERE doc_id = 'FEAT-0001' AND heading = 'Notes'",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated"));
    let f = dir.child("opys/features/FEAT-0001.md");
    f.assert(predicate::str::contains("## Notes\nnew note."));
    f.assert(predicate::str::contains("## More\nkeep me."));
}

/// `query --write --stdin` binds stdin to `$1`, so multi-line content with
/// quotes/percent signs needs no SQL escaping.
#[test]
fn query_write_binds_stdin_to_param() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# T\n\n## Notes\nold.\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "--stdin",
            "UPDATE blocks SET text = $1 WHERE doc_id = 'FEAT-0001' AND heading = 'Notes'",
        ])
        .write_stdin("first line\nline with 'quotes' and 50% off")
        .assert()
        .success();
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::str::contains(
            "## Notes\nfirst line\nline with 'quotes' and 50% off",
        ));
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
        .write_str(
            "---\nid: FEAT-0002\nstatus: planned\ntags: [a]\nreferences:\n  FEAT-0001: One\n---\n\n# Two\n",
        )
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

/// A raw `UPDATE docs SET path` may not relocate a file outside the inventory
/// base — the flush refuses the plan and nothing on disk changes.
#[test]
fn query_write_rejects_path_escape() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\"]\ndefault_status = \"planned\"\n";
    let dir = project_with(cfg);
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "--no-sync",
            "query",
            "--write",
            "UPDATE docs SET path = '../../evil/FEAT-0001.md' WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("not a plain relative path"));
    dir.child("opys/features/FEAT-0001.md")
        .assert(predicate::path::exists());
    assert!(!dir.path().parent().unwrap().join("evil").exists());
}

/// The `retired` ledger is not a DML surface: reservations are managed by the
/// lifecycle commands. It stays readable via plain `query`.
#[test]
fn query_write_rejects_retired_table_dml() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "FEAT-0001", "--reason", "dupe"])
        .assert()
        .success();
    opys(&dir)
        .args(["query", "--write", "DELETE FROM retired"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("may not modify 'retired'"));
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("FEAT-0001: X"));
    opys(&dir)
        .args(["query", "SELECT id FROM retired"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FEAT-0001"));
}

/// Load-snapshot columns (`orig_*`, `dkey`, `num`) are internal bookkeeping —
/// not writable through `query --write`.
#[test]
fn query_write_rejects_snapshot_columns() {
    let dir = project();
    dir.child("opys/features/FEAT-0001.md")
        .write_str("---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n---\n\n# One\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "UPDATE docs SET orig_text = '' WHERE id = 'FEAT-0001'",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("not writable"));
}

/// A positional `INSERT INTO docs VALUES (…)` would smuggle values into the
/// internal columns — an explicit (user-facing) column list is required.
#[test]
fn query_write_insert_docs_requires_column_list() {
    let dir = project();
    opys(&dir)
        .args([
            "query",
            "--write",
            "INSERT INTO docs VALUES (99, 'FEAT-0099', 99, 'feature', 'planned', 'X', \
             '', '', '', 'FEAT-0099.md', NULL, NULL, NULL)",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("explicit column list"));
    dir.child("opys/features/FEAT-0099.md")
        .assert(predicate::path::missing());
}

/// The write gate diffs verify problems keyed by doc id, not rendered path — an
/// edit that relocates a file must not make its pre-existing problems look new.
#[test]
fn query_write_relocation_keeps_preexisting_problems_keyed_by_id() {
    let cfg = "pad = 4\n\
[types.feature]\nprefix = \"FEAT\"\ndir = \"features\"\n\
statuses = [\"planned\", \"archived\"]\ndefault_status = \"planned\"\n\
status_dirs = { archived = \"_archived\" }\n";
    let dir = project_with(cfg);
    // Pre-existing problem: the live doc reuses a retired id. Its verify
    // message is path-prefixed, and the status change below moves the path.
    dir.child("opys/features/FEAT-0007.md")
        .write_str("---\nid: FEAT-0007\nstatus: planned\n---\n\n# Seven\n")
        .unwrap();
    dir.child("opys/_retired.txt")
        .write_str("FEAT-0007  # retired\n")
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "UPDATE docs SET status = 'archived' WHERE id = 'FEAT-0007'",
        ])
        .assert()
        .success();
    dir.child("opys/features/_archived/FEAT-0007.md")
        .assert(predicate::path::exists());
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

/// A `--write` DELETE of a relation row that was an id's only anchor (a
/// tombstone from the pre-ledger era) re-reserves the id in the ledger.
#[test]
fn query_write_deleting_a_tombstone_row_reserves_the_id() {
    let dir = project_with(SOLO_TASK_CFG);
    dir.child("opys/TASK-0002.md")
        .write_str(
            "---\nid: TASK-0002\nstatus: todo\nreferences:\n  TASK-0001: ~~Old one~~\n---\n\n# Two\n",
        )
        .unwrap();
    opys(&dir)
        .args([
            "query",
            "--write",
            "DELETE FROM relations WHERE ref_id = 'TASK-0001'",
        ])
        .assert()
        .success();
    dir.child("opys/TASK-0002.md")
        .assert(predicate::str::contains("TASK-0001").not());
    dir.child("opys/_retired.md")
        .assert(predicate::str::contains("TASK-0001: Old one"));
    opys(&dir)
        .args(["new", "--type", "task", "--title", "Next"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TASK-0003"));
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
        .open(base.join(".opys.lock"))
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
        .open(base.join(".opys.lock"))
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
