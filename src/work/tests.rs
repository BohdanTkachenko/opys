use super::*;
use crate::{next, project_config::ProjectConfig};

fn project() -> Project {
    Project {
        root: "/no-such-project".into(),
        base: "/no-such-project/opys".into(),
        pcfg: toml::from_str::<ProjectConfig>(
            r#"
[types.task]
prefix = "TASK"
statuses = ["todo", "research", "ready", "done"]
default_status = "todo"
terminal_statuses = ["done"]
releases_blocks = ["research", "ready"]
claim_field = "claimed_by"
claim_ttl = "10m"
[types.task.fields.claimed_by]
type = "string"
[types.task.work_branches.spec]
ref = "spec/{id}"
from = "todo"
stages = ["research", "done"]
merged = "ready"
"#,
        )
        .unwrap(),
    }
}

fn doc(id: &str, status: &str, extra: &str) -> Doc {
    Doc::parse(
        format!("/no-such-project/opys/{id}.md").into(),
        &format!("---\nid: {id}\nstatus: {status}\n{extra}---\n\n# {id}\n"),
    )
    .unwrap()
}

fn survey(doc: Doc) -> git::Survey {
    git::Survey {
        refs: vec![git::RefState {
            reference: git::Ref {
                name: "refs/heads/spec/TASK-0001".into(),
                label: "spec/TASK-0001".into(),
                commit: "unused".into(),
                worktrees: vec![],
            },
            changed: Ok(git::Changed {
                docs: vec![doc],
                deleted: vec![],
                ..git::Changed::default()
            }),
            occupied: BTreeMap::new(),
        }],
        ..git::Survey::default()
    }
}

fn now() -> OffsetDateTime {
    crate::history::parse("2026-01-01T00:10:00Z").unwrap()
}
fn query(status: &str) -> next::PickQuery {
    next::PickQuery {
        status: status.into(),
        type_filter: None,
        role: None,
        limit: 10,
    }
}

#[test]
fn branch_statuses_never_release_main_blockers() {
    let prj = project();
    let main = vec![
        doc("TASK-0001", "todo", "blocks:\n  TASK-0002: Two\n"),
        doc("TASK-0002", "todo", "blocked_by:\n  TASK-0001: One\n"),
    ];
    let view = from_survey(
        &prj,
        &main,
        survey(doc("TASK-0001", "research", "blocks:\n  TASK-0002: Two\n")),
        now(),
    );
    assert_eq!(view.docs[0].status(), Some("research"));
    assert_eq!(
        next::pick_effective(&prj, &view, &query("research"), now()).total,
        1
    );
    let answer = next::pick_effective(&prj, &view, &query("todo"), now());
    assert_eq!(answer.total, 0);
    let why = next::explain_effective(&prj, &view, &query("todo"), "TASK-0002", now()).unwrap();
    assert!(matches!(why.why, next::Why::Blocked { .. }));
    assert_eq!(
        next::pick_effective(&prj, &view, &query("research"), now()).items[0].blocks,
        1
    );
}

#[test]
fn invalid_and_terminal_stages_leave_main_effective() {
    let prj = project();
    let main = vec![doc("TASK-0001", "todo", "")];
    for status in ["ready", "done"] {
        let view = from_survey(&prj, &main, survey(doc("TASK-0001", status, "")), now());
        assert_eq!(view.docs[0].status(), Some("todo"));
        assert!(view.refs.is_empty());
        if status == "ready" {
            assert_eq!(view.observations.len(), 1);
        }
    }
}

#[test]
fn unreadable_work_ref_blocks_only_its_document_even_with_a_valid_copy() {
    let prj = project();
    let main = vec![doc("TASK-0001", "todo", ""), doc("TASK-0002", "todo", "")];
    for failure in [
        Err("cannot read ref tree".to_string()),
        Ok(git::Changed {
            unreadable: BTreeMap::from([("TASK-0001".into(), "invalid YAML".into())]),
            ..git::Changed::default()
        }),
    ] {
        let mut surveyed = survey(doc("TASK-0001", "research", ""));
        let mut broken = surveyed.refs[0].clone();
        broken.reference.name = "refs/remotes/origin/spec/TASK-0001".into();
        broken.reference.label = "origin/spec/TASK-0001".into();
        broken.reference.commit = "different-tip".into();
        broken.changed = failure;
        surveyed.refs.push(broken);
        let view = from_survey(&prj, &main, surveyed, now());
        let waiting = next::pick_effective(&prj, &view, &query("todo"), now());
        assert_eq!(waiting.total, 1);
        assert_eq!(waiting.items[0].id, "TASK-0002");
        assert!(matches!(
            waiting.unavailable[0].why,
            next::Why::Unknown { .. }
        ));
        assert_eq!(
            next::pick_effective(&prj, &view, &query("research"), now()).total,
            0
        );
        assert!(view.observations.iter().any(|observation| {
            observation.id == "TASK-0001"
                && observation.target.is_none()
                && observation.to_string().contains("origin/spec/TASK-0001")
                && observation.to_string().contains("unreadable")
        }));
    }
}

#[test]
fn claim_age_boundary_unknown_and_future_timestamps() {
    let prj = project();
    for (stamp, age, expired) in [
        ("2026-01-01T00:00:00Z", Some(600), false),
        ("2025-12-31T23:59:59Z", Some(601), true),
        ("2025-12-31T23:59:59.999999Z", Some(600), true),
        ("not-a-date", None, false),
        ("2026-01-02T00:00:00Z", Some(0), false),
    ] {
        let main = vec![doc(
            "TASK-0001",
            "todo",
            &format!("claimed_by: agent-7\nupdated: '{stamp}'\n"),
        )];
        let claim = claim(&prj.pcfg, &main[0], now()).unwrap();
        assert_eq!(claim.age_seconds, age);
        let view = from_survey(&prj, &main, git::Survey::default(), now());
        assert_eq!(!view.observations.is_empty(), expired);
    }
    let missing = doc("TASK-0001", "todo", "claimed_by: agent-7\n");
    assert!(claim(&prj.pcfg, &missing, now())
        .unwrap()
        .age_seconds
        .is_none());
}

#[test]
fn without_declarations_effective_copies_do_not_open_git() {
    let mut prj = project();
    prj.pcfg = toml::from_str(
        r#"[types.task]
prefix = "TASK"
statuses = ["todo"]
default_status = "todo"
"#,
    )
    .unwrap();
    // No path exists to open: a reader invocation would return a notice.
    let main = vec![doc("TASK-0001", "todo", "")];
    let view = effective(&prj, &main, now());
    assert!(view.notices.is_empty());
    assert!(matches!(view.docs, Cow::Borrowed(_)));
    let result = next::pick(&prj, &main, &query("todo"), now());
    assert_eq!(result.total, 1);
    let json = serde_json::to_value(result).unwrap();
    assert!(json.get("unavailable").is_none());
    assert!(json["items"][0].get("ref").is_none());
    assert!(json["items"][0].get("claim").is_none());
}
