//! Filesystem behavior of the markdown-local backend: load → mutate → flush,
//! verifying real files are written, relocated, deleted, and the ledger migrated.

use opys_backend_markdown_local::MarkdownLocal;
use opys_core::backend::Backend;
use opys_core::project::Project;

const CFG: &str = r#"
pad = 4
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented", "archived"]
default_status = "planned"
status_dirs = { archived = "_archived" }
[types.task]
prefix = "TASK"
statuses = ["todo", "done"]
default_status = "todo"
terminal_statuses = ["done"]
"#;

fn project_with(docs: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("opys.toml"), CFG).unwrap();
    let base = dir.path().join("opys");
    std::fs::create_dir_all(&base).unwrap();
    for (rel, text) in docs {
        let p = base.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let prj = Project::open(&dir.path().to_string_lossy()).expect("open project");
    (dir, prj)
}

/// Flush right after load must not modify a single byte on disk — even a
/// deliberately non-canonical file, since the doc did not logically change.
#[test]
fn load_then_flush_is_a_noop() {
    let docs: &[(&str, &str)] = &[
        (
            "FEAT-0001.md",
            "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
        ),
        (
            "FEAT-0002.md",
            "---\nstatus: planned\nid: \"FEAT-0002\"\n---\n# B\n",
        ),
    ];
    let (_t, prj) = project_with(docs);
    let before: Vec<String> = docs
        .iter()
        .map(|(rel, _)| std::fs::read_to_string(prj.base.join(rel)).unwrap())
        .collect();
    let (s, _) = MarkdownLocal.load(&prj).unwrap();
    MarkdownLocal.flush(&prj, s).unwrap();
    for ((rel, _), b) in docs.iter().zip(&before) {
        let after = std::fs::read_to_string(prj.base.join(rel)).unwrap();
        assert_eq!(*b, after, "{rel} changed on a no-op flush");
    }
}

/// put_doc + set_canonical_path relocates on flush; delete_doc removes the file;
/// retire_id rewrites the ledger sorted by number, migrating a legacy ledger.
#[test]
fn flush_applies_writes_renames_deletes_and_ledger() {
    let docs: &[(&str, &str)] = &[
        (
            "FEAT-0001.md",
            "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
        ),
        (
            "FEAT-0002.md",
            "---\nid: FEAT-0002\nstatus: planned\n---\n\n# B\n",
        ),
        (
            "FEAT-0003.md",
            "---\nid: FEAT-0003\nstatus: planned\n---\n\n# C\n",
        ),
    ];
    let (_t, prj) = project_with(docs);
    std::fs::write(
        prj.base.join("_retired.txt"),
        "FEAT-0009  # retired 2026-01-01: old\n",
    )
    .unwrap();
    let (mut s, _) = MarkdownLocal.load(&prj).unwrap();

    let k1 = s.dkey_of("FEAT-0001").unwrap();
    let mut d1 = s.doc(k1).unwrap();
    d1.frontmatter.set_str("status", "archived");
    s.put_doc(&prj.pcfg, Some(k1), &d1).unwrap();
    s.set_canonical_path(&prj.pcfg, k1).unwrap();

    let k2 = s.dkey_of("FEAT-0002").unwrap();
    s.delete_doc(k2).unwrap();
    let k3 = s.dkey_of("FEAT-0003").unwrap();
    s.retire_id("FEAT-0003", "C").unwrap();
    s.delete_doc(k3).unwrap();

    MarkdownLocal.flush(&prj, s).unwrap();

    assert!(!prj.base.join("FEAT-0001.md").exists());
    let archived = prj.base.join("_archived/FEAT-0001.md");
    assert!(archived.exists(), "archived doc not relocated");
    assert!(std::fs::read_to_string(&archived)
        .unwrap()
        .contains("status: archived"));
    assert!(!prj.base.join("FEAT-0002.md").exists());
    assert!(!prj.base.join("FEAT-0003.md").exists());
    assert!(!prj.base.join("_retired.txt").exists());
    let ledger = std::fs::read_to_string(prj.base.join("_retired.md")).unwrap();
    assert!(ledger.contains("FEAT-0003: C"), "ledger = {ledger}");
    assert!(ledger.contains("FEAT-0009"), "ledger = {ledger}");
}
