//! End-to-end tests driving the `opys` binary against temp project dirs.

use assert_cmd::Command;
use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;

const CONFIG: &str = r#"prefix = "VIK"
pad = 4
test_search_paths = ["src"]
test_reference_check = "none"
extra_statuses = []

[fields.ptyxis_ref]
type = "string"
required = false
"#;

/// A temp project with a VIK/no-grep config under the default docs/ layout.
fn project() -> TempDir {
    project_with(CONFIG)
}

fn project_with(config: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("docs/features/_config.toml")
        .write_str(config)
        .unwrap();
    dir
}

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
    dir.child("docs/features/_config.toml")
        .assert(predicate::path::exists());
    dir.child("docs/runbooks").assert(predicate::path::is_dir());
}

#[test]
fn init_does_not_overwrite_existing_config() {
    let dir = project();
    opys(&dir)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("already exists"));
    dir.child("docs/features/_config.toml")
        .assert(predicate::str::contains("prefix = \"VIK\""));
}

#[test]
fn new_allocates_next_id_and_requires_tags() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "osc,tabs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("VIK-0001.md"));
    dir.child("docs/features/VIK-0001.md")
        .assert(predicate::str::contains("# First"));

    opys(&dir)
        .args(["new", "--title", "Second", "--tags", "tabs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("VIK-0002.md"));

    // Empty tags rejected.
    opys(&dir)
        .args(["new", "--title", "Bad", "--tags", " , "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one tag"));
}

#[test]
fn new_auto_syncs_index_and_views() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    // Auto-sync regenerated the index and a by-tag view.
    dir.child("docs/features/INDEX.md")
        .assert(predicate::str::contains("VIK-0001"));
    dir.child("docs/views/by-tag/osc.md")
        .assert(predicate::path::exists());
}

#[test]
fn no_sync_skips_regeneration() {
    let dir = project();
    opys(&dir)
        .args(["--no-sync", "new", "--title", "First", "--tags", "osc"])
        .assert()
        .success();
    dir.child("docs/features/INDEX.md")
        .assert(predicate::path::missing());
}

#[test]
fn custom_dir_relocates_inventory() {
    let dir = TempDir::new().unwrap();
    dir.child("inventory/features/_config.toml")
        .write_str(CONFIG)
        .unwrap();
    opys(&dir)
        .args(["--dir", "inventory", "new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    dir.child("inventory/features/VIK-0001.md")
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
    dir.child("docs/features/VIK-0001.md")
        .assert(predicate::str::contains("ptyxis_ref: src/x.c"));
}

#[test]
fn set_status_implemented_requires_checked_item() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();

    opys(&dir)
        .args(["set-status", "VIK-0001", "implemented"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no checked test-plan item"));

    let path = dir.child("docs/features/VIK-0001.md");
    let mut text = std::fs::read_to_string(path.path()).unwrap();
    text.push_str("\n## Test plan\n- [x] does a thing — `mod::test_thing`\n");
    std::fs::write(path.path(), text).unwrap();

    opys(&dir)
        .args(["set-status", "VIK-0001", "implemented"])
        .assert()
        .success()
        .stdout(predicate::str::contains("VIK-0001 -> implemented"));
}

#[test]
fn set_status_wontfix_requires_reason() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["set-status", "VIK-0001", "wontfix"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("wontfix requires --reason"));
    opys(&dir)
        .args([
            "set-status",
            "VIK-0001",
            "wontfix",
            "--reason",
            "out of scope",
        ])
        .assert()
        .success();
    dir.child("docs/features/VIK-0001.md")
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
        .args(["tag", "VIK-0001", "--add", "b,c"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a, b, c"));
    opys(&dir)
        .args(["tag", "VIK-0001", "--remove", "a,b,c"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least one tag"));
}

#[test]
fn retire_deletes_and_reserves_id() {
    let dir = project();
    opys(&dir)
        .args(["new", "--title", "X", "--tags", "a"])
        .assert()
        .success();
    opys(&dir)
        .args(["retire", "VIK-0001", "--reason", "dupe"])
        .assert()
        .success();
    dir.child("docs/features/VIK-0001.md")
        .assert(predicate::path::missing());
    dir.child("docs/features/_retired.txt")
        .assert(predicate::str::contains("VIK-0001"));

    opys(&dir)
        .args(["new", "--title", "Y", "--tags", "a"])
        .assert()
        .success()
        .stdout(predicate::str::contains("VIK-0002.md"));
}

#[test]
fn verify_passes_clean_and_flags_violations() {
    let dir = project();
    dir.child("docs/features/VIK-0001.md")
        .write_str(
            "---\nid: VIK-0001\nstatus: planned\ntags: [osc, tabs]\n---\n\n# Clean feature\n",
        )
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK (1 features)"));

    dir.child("docs/features/VIK-0002.md")
        .write_str(
            "---\nid: VIK-0002\nstatus: implemented\ntags: [Bad_Tag]\nbogus: 1\n---\n\n# Broken\n",
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
            "implemented but no checked test-plan item",
        ));
}

#[test]
fn verify_checks_manual_item_shape() {
    let dir = project();
    dir.child("docs/features/VIK-0001.md")
        .write_str(
            "---\nid: VIK-0001\nstatus: planned\ntags: [a]\n---\n\n# F\n\n## Manual verification\n- Looks right — *manual: visual*\n",
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
fn verify_extract_mode_resolves_real_tests() {
    let config = r#"prefix = "VIK"
test_search_paths = ["src"]
test_reference_check = "extract"
test_name_pattern = "fn\\s+(\\w+)\\s*\\("
"#;
    let dir = project_with(config);
    dir.child("src/lib.rs")
        .write_str("fn real_test() {}\nfn another() {}\n")
        .unwrap();

    // Module ref + path ref to existing tests pass.
    dir.child("docs/features/VIK-0001.md")
        .write_str("---\nid: VIK-0001\nstatus: implemented\ntags: [a]\n---\n\n# Good\n\n## Test plan\n- [x] a — `mod::real_test`\n- [x] b — `src/lib.rs::another`\n")
        .unwrap();
    opys(&dir).arg("verify").assert().success();

    // Missing test name fails; wrong file fails.
    dir.child("docs/features/VIK-0002.md")
        .write_str("---\nid: VIK-0002\nstatus: implemented\ntags: [a]\n---\n\n# Bad\n\n## Test plan\n- [x] c — `mod::nope`\n- [x] d — `src/missing.rs::real_test`\n")
        .unwrap();
    opys(&dir)
        .arg("verify")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "test reference `mod::nope` not found",
        ))
        .stderr(predicate::str::contains(
            "test file `src/missing.rs` not found",
        ));
}

#[test]
fn sync_views_generates_and_prunes() {
    let dir = project();
    dir.child("docs/features/VIK-0001.md")
        .write_str("---\nid: VIK-0001\nstatus: planned\ntags: [osc]\n---\n\n# One\n")
        .unwrap();
    dir.child("docs/views/by-tag/stale.md")
        .write_str("old\n")
        .unwrap();

    opys(&dir).arg("sync-views").assert().success();
    dir.child("docs/features/INDEX.md")
        .assert(predicate::str::contains("VIK-0001 [planned] (osc) One"));
    dir.child("docs/views/by-tag/osc.md")
        .assert(predicate::path::exists());
    dir.child("docs/views/status/planned.md")
        .assert(predicate::path::exists());
    dir.child("docs/views/by-tag/stale.md")
        .assert(predicate::path::missing());
}

#[test]
fn report_parity_is_opt_in() {
    let dir = project(); // parity not set -> default off
    dir.child("docs/features/VIK-0001.md")
        .write_str("---\nid: VIK-0001\nstatus: planned\ntags: [a]\n---\n\n# A\n\n## Manual verification\n- check — *manual: visual*\n  - Setup: x\n  - Steps:\n    1. do\n  - Expect: ok\n")
        .unwrap();
    opys(&dir)
        .arg("report")
        .assert()
        .success()
        .stdout(predicate::str::contains("features: 1"))
        .stdout(predicate::str::contains(
            "manual items without automated coverage: 1",
        ))
        .stdout(predicate::str::contains("parity").not());

    // With parity enabled, the percentages appear.
    let dir2 = project_with("prefix = \"VIK\"\nparity = true\n");
    dir2.child("docs/features/VIK-0001.md")
        .write_str("---\nid: VIK-0001\nstatus: planned\ntags: [a]\n---\n\n# A\n")
        .unwrap();
    opys(&dir2)
        .arg("report")
        .assert()
        .success()
        .stdout(predicate::str::contains("parity (impl / all)"));
}

#[test]
fn manual_runbook_groups_and_flags_uncovered() {
    let dir = project();
    dir.child("docs/features/VIK-0001.md")
        .write_str(
            "---\nid: VIK-0001\nstatus: planned\ntags: [a]\n---\n\n# A\n\n## Manual verification\n- Check it — *manual: visual*\n  - Setup: monitor at 150%\n  - Steps:\n    1. open\n  - Expect: looks good\n",
        )
        .unwrap();
    dir.child("docs/features/VIK-0002.md")
        .write_str(
            "---\nid: VIK-0002\nstatus: wontfix\ntags: [a]\nwontfix_reason: x\n---\n\n# B\n\n## Manual verification\n- Skip me — *manual: visual*\n  - Setup: monitor at 150%\n  - Steps:\n    1. open\n  - Expect: nope\n",
        )
        .unwrap();
    opys(&dir)
        .arg("manual-runbook")
        .assert()
        .success()
        .stdout(predicate::str::contains("## Setup: monitor at 150%"))
        .stdout(predicate::str::contains("⚠ VIK-0001 — Check it"))
        .stdout(predicate::str::contains("Expect: looks good"))
        .stdout(predicate::str::contains("VIK-0002").not());
}

#[test]
fn schema_emits_config_and_frontmatter() {
    let dir = project();
    opys(&dir)
        .args(["schema", "--kind", "config"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opys project config"))
        .stdout(predicate::str::contains("test_reference_check"));

    // Frontmatter schema is derived from config: it knows the custom field
    // and forbids undeclared keys.
    opys(&dir)
        .args(["schema", "--kind", "frontmatter"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ptyxis_ref"))
        .stdout(predicate::str::contains("\"additionalProperties\": false"));
}
