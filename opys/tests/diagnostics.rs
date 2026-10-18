use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::{
    backend::Backend,
    commands::{edit, verify},
    diagnostics,
    doc::Doc,
    Project,
};

const CONFIG: &str = r#"
base = "inventory"
[types.note]
prefix = "NOTE"
statuses = ["open", "closed"]
default_status = "open"
tags_required = true
[types.note.fields.risk]
type = "int"
[[types.note.sections]]
heading = "Checks"
kind = "checklist"
[[types.note.sections.checks]]
pattern = '(?P<symbol>missing)'
must_match = '${symbol}'
roots = ["src"]
scope = "checked"
message = "unknown symbol ${symbol}"
"#;

const TEXT: &str = "---\n# Preserve this handwritten comment\nid: NOTE-0001\nstatus: wrong\ntags:\n  - okay\n  - Bad\nrisk: invalid\nextra: true\nhistory:\n  \"2026-01-01T00:00:00.000000Z\": open\n---\n\n# Unicode 🦀\n\n## Checks\n- [x] missing\n- [x] λ 🦀 missing\n";

#[test]
fn malformed_history_points_to_the_bad_entry_instead_of_its_container() {
    for entry in [
        "\"not a timestamp\": open",
        "\"2026-01-02T00:00:00.000000Z\": 42",
    ] {
        let text = format!("---\nid: NOTE-0001\nstatus: open\ntags: [okay]\nhistory:\n  \"2026-01-01T00:00:00.000000Z\": open\n  {entry}\n---\n# Note\n");
        let dir = fixture(CONFIG, &text);
        let prj = Project::open(&dir.path().to_string_lossy()).unwrap();
        let (mut store, errors) = MarkdownLocal.load(&prj).unwrap();
        let docs: Vec<_> = store
            .all_docs()
            .unwrap()
            .into_iter()
            .map(|(_, doc)| doc)
            .collect();
        drop(store.take_lock());
        let mut findings = verify::collect_findings(&prj, &docs, errors);
        diagnostics::resolve(&prj, &docs, &mut findings, &MarkdownLocal);
        let history = findings
            .iter()
            .find(|finding| finding.message.contains("history"))
            .unwrap();
        assert_eq!(history.range.as_ref().unwrap().start.line, 7);
    }
}

fn fixture(config: &str, text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("inventory")).unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("opys.toml"), config).unwrap();
    std::fs::write(dir.path().join("inventory/NOTE-0001.md"), text).unwrap();
    dir
}

#[test]
fn cli_prints_stored_file_lines_and_columns_and_remains_read_only() {
    let dir = fixture(CONFIG, TEXT);
    let result = assert_cmd::Command::cargo_bin("opys")
        .unwrap()
        .arg("--root")
        .arg(dir.path())
        .arg("verify")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stderr.contains("inventory/NOTE-0001.md:4:1: NOTE-0001: invalid status"),
        "{stderr}"
    );
    assert!(
        stderr.contains("inventory/NOTE-0001.md:7:5: NOTE-0001: tag 'Bad'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("inventory/NOTE-0001.md:9:1: NOTE-0001: unknown frontmatter field 'extra'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("inventory/NOTE-0001.md:17:7: NOTE-0001: unknown symbol missing"),
        "{stderr}"
    );
    assert!(
        stderr.contains("inventory/NOTE-0001.md:18:11: NOTE-0001: unknown symbol missing"),
        "{stderr}"
    );
    assert!(result.stdout.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inventory/NOTE-0001.md")).unwrap(),
        TEXT
    );
}

#[test]
fn duplicate_messages_have_distinct_ranges_and_the_gate_ignores_line_movements() {
    let dir = fixture(CONFIG, TEXT);
    let prj = Project::open(&dir.path().to_string_lossy()).unwrap();
    let (mut store, parse_errors) = MarkdownLocal.load(&prj).unwrap();
    let docs: Vec<Doc> = store
        .all_docs()
        .unwrap()
        .into_iter()
        .map(|(_, doc)| doc)
        .collect();
    let mut findings = verify::collect_findings(&prj, &docs, parse_errors.clone());
    diagnostics::resolve(&prj, &docs, &mut findings, &MarkdownLocal);
    let symbols: Vec<_> = findings
        .iter()
        .filter(|f| f.message.contains("unknown symbol"))
        .collect();
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0].message, symbols[1].message);
    assert_ne!(symbols[0].range, symbols[1].range);
    assert_eq!(
        symbols[0].body_version.as_ref(),
        Some(&diagnostics::body_version(&docs[0].body))
    );
    assert!(symbols[0]
        .excerpt
        .as_ref()
        .unwrap()
        .text
        .contains("missing"));
    edit::body_core(
        &prj,
        &mut store,
        "NOTE-0001",
        &format!("New prose\n\n{}", docs[0].body),
        &parse_errors,
    )
    .unwrap();

    // A file changed since the snapshot must not receive stale annotations.
    std::fs::write(
        dir.path().join("inventory/NOTE-0001.md"),
        TEXT.replace("# Unicode", "# Changed Unicode"),
    )
    .unwrap();
    let mut stale = verify::collect_findings(&prj, &docs, vec![]);
    diagnostics::resolve(&prj, &docs, &mut stale, &MarkdownLocal);
    assert!(stale.iter().all(|f| f.range.is_none()));
}

#[test]
fn missing_content_has_an_insertion_point_and_yaml_parse_errors_include_the_fence() {
    let config = format!("{CONFIG}\n[[types.note.sections]]\nheading = \"Required\"\nkind = \"prose\"\nrequired = true\n");
    let text = "---\nid: NOTE-0001\nstatus: open\ntags: [okay]\n---\n\n# Note\n";
    let dir = fixture(&config, text);
    let prj = Project::open(&dir.path().to_string_lossy()).unwrap();
    let (mut store, errors) = MarkdownLocal.load(&prj).unwrap();
    let docs: Vec<_> = store
        .all_docs()
        .unwrap()
        .into_iter()
        .map(|(_, doc)| doc)
        .collect();
    drop(store.take_lock());
    let mut findings = verify::collect_findings(&prj, &docs, errors);
    diagnostics::resolve(&prj, &docs, &mut findings, &MarkdownLocal);
    let history = findings
        .iter()
        .find(|f| f.message.contains("missing history"))
        .unwrap()
        .range
        .as_ref()
        .unwrap();
    assert_eq!(history.start.line, 5);
    assert_eq!(history.start, history.end);
    let missing = findings
        .iter()
        .find(|f| f.message.contains("missing required"))
        .unwrap()
        .range
        .as_ref()
        .unwrap();
    assert_eq!(missing.start.line, 8);
    assert_eq!(missing.start, missing.end);

    std::fs::write(
        dir.path().join("inventory/NOTE-0001.md"),
        "---\nid: NOTE-0001\nstatus: open\ninvalid: text: bad\n---\n# Note\n",
    )
    .unwrap();
    let (mut store, errors) = MarkdownLocal.load(&prj).unwrap();
    drop(store.take_lock());
    let mut findings = verify::collect_findings(&prj, &[], errors);
    diagnostics::resolve(&prj, &[], &mut findings, &MarkdownLocal);
    assert_eq!(findings[0].range.as_ref().unwrap().start.line, 4);
}
