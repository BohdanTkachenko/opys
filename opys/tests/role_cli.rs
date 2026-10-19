//! Role-queue contracts, independent of the unchanged legacy CLI suite.
use assert_cmd::Command;
use assert_fs::{prelude::*, TempDir};
use serde_json::{json, Value};

const CFG: &str = r#"
base = "inventory"
escalate_to = "human"
[roles.human]
description = "Sign-offs"
[roles.agent]
[types.task]
prefix = "TASK"
statuses = ["review", "todo", "doing", "settled", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.owners]
review = "human"
todo = "agent"
doing = ["agent", "human"]
settled = []
[types.task.fields.owner]
type = "role"
[types.task.fields.priority]
type = "int"
[types.task.ttl]
doing = "1d"
[types.task.max_visits]
doing = {limit = 1, then = "review"}
[types.note]
prefix = "NOTE"
statuses = ["open", "review"]
default_status = "open"
[types.note.owners]
open = []
review = "human"
[types.work]
prefix = "WORK"
statuses = ["todo"]
default_status = "todo"
requires_link = {to = "note", min = 1}
[types.work.owners]
todo = "agent"
"#;
fn project() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml").write_str(CFG).unwrap();
    dir
}
fn command(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("opys").unwrap();
    cmd.current_dir(dir.path())
        .env_remove("OPYS_ROLE")
        .env("OPYS_NOW", "2026-10-19T12:00:00Z");
    cmd
}
fn doc(dir: &TempDir, id: &str, status: &str, extra: &str) {
    dir.child(format!("inventory/{id}.md")).write_str(&format!("---\nid: {id}\nstatus: {status}\ntags: [test]\ncreated: \"2026-10-01T00:00:00Z\"\nupdated: \"2026-10-01T00:00:00Z\"\nhistory:\n  \"2026-10-01T00:00:00.000000Z\": {status}\n{extra}---\n\n# Title {id}\n")).unwrap();
}
fn output(dir: &TempDir, args: &[&str]) -> Vec<u8> {
    command(dir)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone()
}
fn payload(dir: &TempDir, args: &[&str]) -> Value {
    serde_json::from_slice(&output(dir, args)).unwrap()
}

#[test]
fn queue_groups_follow_declarations_and_match_next_before_limits() {
    let dir = project();
    doc(&dir, "TASK-0001", "review", "priority: 20\n");
    doc(&dir, "TASK-0002", "review", "priority: 5\n");
    let queue = payload(&dir, &["queue", "human", "--json", "-n", "1"]);
    assert_eq!(
        queue["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| (g["type"].as_str().unwrap(), g["status"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        vec![("task", "review"), ("task", "doing"), ("note", "review")]
    );
    assert_eq!(queue["total"], 2);
    assert_eq!(queue["groups"][0]["total"], 2);
    for group in queue["groups"].as_array().unwrap() {
        let next = payload(
            &dir,
            &[
                "next",
                "--role",
                "human",
                "--status",
                group["status"].as_str().unwrap(),
                "--type",
                group["type"].as_str().unwrap(),
                "--json",
                "-n",
                "1",
            ],
        );
        let mut items = group["items"].clone();
        for item in items.as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("since");
            item.as_object_mut().unwrap().remove("owner_override");
        }
        assert_eq!(items, next["items"]);
    }
    let text = String::from_utf8(output(&dir, &["queue", "human"])).unwrap();
    assert!(text.contains("task · review"));
    assert!(!text.contains("task · doing"));
    assert!(!text.contains("note · review"));
    let empty = project();
    assert_eq!(
        output(&empty, &["queue", "agent"]),
        b"agent: nothing waiting\n"
    );
}

#[test]
fn override_routes_only_to_its_role_and_waiting_on_combines_filters() {
    let dir = project();
    doc(&dir, "TASK-0001", "review", "owner: agent\n");
    doc(&dir, "TASK-0002", "todo", "");
    doc(&dir, "TASK-0003", "settled", "");
    let queue = payload(&dir, &["queue", "agent", "--json"]);
    assert_eq!(queue["groups"][0]["status"], "review");
    assert_eq!(queue["groups"][0]["owned"], false);
    assert_eq!(queue["groups"][0]["items"][0]["owner_override"], true);
    let human = payload(&dir, &["queue", "human", "--json"]);
    assert_eq!(human["groups"][0]["items"], json!([]));
    assert_eq!(
        output(
            &dir,
            &[
                "list",
                "--waiting-on",
                "agent",
                "--status",
                "review",
                "--type",
                "task",
                "--format",
                "ids"
            ]
        ),
        b"TASK-0001\n"
    );
    assert_eq!(
        output(&dir, &["list", "--waiting-on", "human", "--format", "ids"]),
        b""
    );
    let explain = payload(
        &dir,
        &[
            "next",
            "--status",
            "review",
            "--role",
            "human",
            "--explain",
            "TASK-0001",
            "--json",
        ],
    );
    assert_eq!(explain["role"], "human");
    assert_eq!(explain["explain"]["reason"], "other-role");
    assert!(explain["explain"]["message"]
        .as_str()
        .unwrap()
        .contains("waits on agent, not human"));
    assert_eq!(
        payload(
            &dir,
            &["next", "--status", "review", "--role", "agent", "--json"]
        )["items"][0]["owners"],
        json!(["agent"])
    );
}

#[test]
fn role_defaults_usage_errors_and_read_only_queries() {
    let dir = project();
    doc(&dir, "TASK-0001", "todo", "");
    let path = dir.child("inventory/TASK-0001.md");
    let before = std::fs::read(path.path()).unwrap();
    let explicit = output(&dir, &["queue", "agent", "--json"]);
    command(&dir)
        .env("OPYS_ROLE", "agent")
        .args(["queue", "--json"])
        .assert()
        .success()
        .stdout(explicit);
    for args in [
        vec!["queue"],
        vec!["queue", "typo"],
        vec!["next", "--status", "todo", "--role", "typo"],
        vec!["list", "--waiting-on", "typo"],
    ] {
        let result = command(&dir)
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        let message = String::from_utf8(result).unwrap();
        assert!(
            message.contains("declared roles: agent, human"),
            "{message}"
        );
    }
    assert_eq!(std::fs::read(path.path()).unwrap(), before);
}

#[test]
fn attention_uses_every_shared_reason_and_total_deduplicates() {
    let dir = project();
    doc(
        &dir,
        "TASK-0001",
        "doing",
        "blocks:\n  TASK-0004: Title TASK-0004\n",
    ); // pickable + stuck + overflowed
    doc(&dir, "WORK-0002", "todo", ""); // orphan + verify requires_link problem
    doc(&dir, "TASK-0003", "review", "owner: unknown\n"); // role-field problem
    doc(
        &dir,
        "TASK-0004",
        "review",
        "blocked_by:\n  TASK-0001: Title TASK-0001\n",
    );
    dir.child("inventory/NOTE-0005.md")
        .write_str("---\nnot yaml: [\n---\n")
        .unwrap();
    doc(&dir, "TASK-0006", "review", ""); // pickable before limit, no attention
    let queue = payload(&dir, &["queue", "human", "--json", "-n", "0"]);
    assert_eq!(queue["total"], 4, "{queue:#}");
    assert_eq!(
        queue["attention"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["TASK-0001", "WORK-0002", "TASK-0003"]
    );
    let reasons = queue["attention"][0]["reasons"].as_array().unwrap();
    assert!(reasons.iter().any(|r| r["kind"] == "stuck"));
    assert!(reasons.iter().any(|r| r["kind"] == "overflowed"));
    let orphan_reasons = queue["attention"][1]["reasons"].as_array().unwrap();
    assert!(orphan_reasons.iter().any(|r| r["kind"] == "orphaned"));
    assert!(orphan_reasons.iter().any(|r| r["kind"] == "problem"));
    assert!(!queue["project_problems"].as_array().unwrap().is_empty());
    assert_eq!(
        queue["groups"][0]["blocked"][0]["blocked_by"],
        json!(["TASK-0001"])
    );
    let agent = payload(&dir, &["queue", "agent", "--json"]);
    assert_eq!(agent["attention"], json!([]));
    assert_eq!(agent["project_problems"], json!([]));
    assert_eq!(agent["groups"][2]["orphaned"][0]["id"], "WORK-0002");
    let filtered = payload(&dir, &["queue", "human", "--type", "work", "--json"]);
    assert_eq!(filtered["attention"].as_array().unwrap().len(), 1);
}

#[test]
fn no_roles_next_json_stays_byte_identical() {
    let dir = TempDir::new().unwrap();
    dir.child("opys.toml")
        .write_str(
            "[types.note]\nprefix = \"NOTE\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n",
        )
        .unwrap();
    // Empty payload pins the original whitespace and omitted extension keys.
    assert_eq!(output(&dir,&["next","--status","open","--json"]), b"{\n  \"schema\": \"opys.next/1\",\n  \"status\": \"open\",\n  \"type\": null,\n  \"limit\": 10,\n  \"total\": 0,\n  \"items\": [],\n  \"orphaned\": []\n}\n");
    dir.child("opys/NOTE-0001.md")
        .write_str("---\nid: NOTE-0001\nstatus: open\ntags: []\n---\n\n# Note\n")
        .unwrap();
    assert_eq!(String::from_utf8(output(&dir,&["next","--status","open","--json"])).unwrap(), "{\n  \"schema\": \"opys.next/1\",\n  \"status\": \"open\",\n  \"type\": null,\n  \"limit\": 10,\n  \"total\": 1,\n  \"items\": [\n    {\n      \"id\": \"NOTE-0001\",\n      \"type\": \"note\",\n      \"title\": \"Note\",\n      \"status\": \"open\",\n      \"path\": \"opys/NOTE-0001.md\",\n      \"tags\": [],\n      \"priority\": null,\n      \"effective_priority\": null,\n      \"blocks\": 0,\n      \"position\": 1,\n      \"observations\": []\n    }\n  ],\n  \"orphaned\": []\n}\n");
}

#[test]
fn override_groups_preserve_undeclared_statuses_and_since_falls_back_to_created() {
    let dir = project();
    doc(&dir, "TASK-0001", "undeclared", "owner: agent\n");
    let q = payload(&dir, &["queue", "agent", "--json"]);
    let group = q["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["status"] == "undeclared")
        .unwrap();
    assert_eq!(group["status"], "undeclared");
    assert_eq!(group["owned"], false);
    assert!(group.get("owners").is_none());
    assert_eq!(group["items"][0]["id"], "TASK-0001");
    assert_eq!(group["items"][0]["since"], "2026-10-01T00:00:00.000000Z");
    dir.child("inventory/TASK-0002.md").write_str("---\nid: TASK-0002\nstatus: todo\ntags: [test]\ncreated: \"2026-10-02T00:00:00Z\"\n---\n\n# Fallback\n").unwrap();
    doc(&dir, "TASK-0003", "settled", "");
    let q = payload(&dir, &["queue", "agent", "--json"]);
    assert_eq!(q["groups"][0]["items"][0]["since"], "2026-10-02T00:00:00Z");
    let explain = payload(
        &dir,
        &[
            "next",
            "--status",
            "settled",
            "--role",
            "agent",
            "--explain",
            "TASK-0003",
            "--json",
        ],
    );
    assert!(explain["explain"].get("owners").is_none());
    assert!(explain["explain"]["message"]
        .as_str()
        .unwrap()
        .contains("waits on nobody, not agent"));
}

#[test]
fn override_with_missing_status_is_visible_without_escalation_and_omits_missing_since() {
    let dir = project();
    dir.child("inventory/TASK-0001.md")
        .write_str("---\nid: TASK-0001\nowner: agent\ntags: [test]\n---\n\n# Repair me\n")
        .unwrap();
    let q = payload(&dir, &["queue", "agent", "--json"]);
    let group = q["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["status"] == "")
        .unwrap();
    assert_eq!(group["items"][0]["id"], "TASK-0001");
    assert!(group["items"][0].get("since").is_none());
    assert!(q["attention"].as_array().unwrap().is_empty());
}
