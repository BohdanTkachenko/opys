//! Role endpoints driven through a real node and compared with the real CLI.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const NOW: &str = "2026-10-19T12:00:00Z";
const CONFIG: &str = r#"
base = "inventory"
escalate_to = "human"
[roles.human]
description = "Sign-offs"
[roles.agent]
description = "Implementation"
[types.task]
prefix = "TASK"
statuses = ["todo", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
[types.task.owners]
todo = "agent"
review = "human"
[types.task.fields.owner]
type = "role"
[types.note]
prefix = "NOTE"
statuses = ["review"]
default_status = "review"
tags_required = false
[types.note.owners]
review = ["human", "agent"]
"#;

fn project(root: &Path, config: &str) -> PathBuf {
    std::fs::create_dir_all(root.join("inventory")).unwrap();
    std::fs::write(root.join("opys.toml"), config).unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn doc(root: &Path, id: &str, status: &str, extra: &str) {
    std::fs::write(
        root.join("inventory").join(format!("{id}.md")),
        format!(
            "---\nid: {id}\nstatus: {status}\ncreated: \"2026-10-17T12:00:00Z\"\n\
             updated: \"2026-10-17T12:00:00Z\"\nhistory:\n  \"2026-10-17T12:00:00.000000Z\": {status}\n\
             {extra}---\n\n# Document {id}\n\nBody.\n"
        ),
    )
    .unwrap();
}

struct Node {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Node {
    fn start(home: &Path, roots: &[&Path]) -> Self {
        let config = home.join("server.toml");
        let allowlist: String = roots
            .iter()
            .map(|root| format!("[[project]]\npath = {:?}\n", root.display().to_string()))
            .collect();
        std::fs::write(&config, allowlist).unwrap();
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let child = Command::new(assert_cmd::cargo::cargo_bin!("opys"))
            .args(["web", "start", "--bind", &addr.to_string(), "--config"])
            .arg(config)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OPYS_NOW", NOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut node = Self { child, addr };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok((200, body)) = node.try_request("GET", "/api/projects", None) {
                let groups: Value = serde_json::from_slice(&body).unwrap();
                if groups
                    .as_array()
                    .is_some_and(|groups| groups.len() == roots.len())
                {
                    return node;
                }
            }
            assert!(
                node.child.try_wait().unwrap().is_none(),
                "node stopped during startup"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("node did not serve its projects before the startup deadline");
    }

    fn try_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> std::io::Result<(u16, Vec<u8>)> {
        let mut stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let body = body.map(Value::to_string).unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len(),
        )?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;
        let split = bytes
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let header = std::str::from_utf8(&bytes[..split]).unwrap();
        let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
        // Every route used here returns a concrete JSON body with Content-Length.
        assert!(!header
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"));
        Ok((status, bytes[split + 4..].to_vec()))
    }

    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Vec<u8>) {
        self.try_request(method, path, body).unwrap()
    }

    fn get(&self, path: &str) -> (u16, Value) {
        let (status, body) = self.request("GET", path, None);
        (status, serde_json::from_slice(&body).unwrap())
    }

    fn groups(&self) -> Vec<Value> {
        let (status, groups) = self.get("/api/projects");
        assert_eq!(status, 200, "{groups}");
        groups.as_array().unwrap().clone()
    }

    fn project(&self, root: &Path) -> (String, String) {
        for group in self.groups() {
            for corpus in group["corpora"].as_array().unwrap() {
                if corpus["root"].as_str() == root.to_str() {
                    return (
                        group["key"].as_str().unwrap().to_string(),
                        corpus["cid"].as_str().unwrap().to_string(),
                    );
                }
            }
        }
        panic!("project {} was not served", root.display());
    }
}

fn cli(root: &Path, args: &[&str]) -> Vec<u8> {
    let result = Command::new(assert_cmd::cargo::cargo_bin!("opys"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("OPYS_NOW", NOW)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn project_queue_matches_cli_bytes_and_validates_role_and_options() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(&tmp.path().join("project"), CONFIG);
    doc(&root, "TASK-0001", "review", "");
    doc(&root, "TASK-0002", "todo", "owner: human\n");
    doc(&root, "TASK-0003", "review", "owner: agent\nbogus: value\n");
    let node = Node::start(tmp.path(), &[&root]);
    let (key, _) = node.project(&root);
    for (query, args) in [
        ("", vec!["queue", "human", "--json"]),
        (
            "?type=task&n=1",
            vec!["queue", "human", "--json", "--type", "task", "-n", "1"],
        ),
        ("?n=0", vec!["queue", "human", "--json", "-n", "0"]),
    ] {
        let (status, bytes) = node.request(
            "GET",
            &format!("/api/projects/{key}/queue/human{query}"),
            None,
        );
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
        assert_eq!(
            bytes,
            cli(&root, &args),
            "node and CLI queue must be byte-identical"
        );
    }
    let (status, body) = node.get(&format!("/api/projects/{key}/queue/missing"));
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].as_str().unwrap().contains(&key));
    assert!(body["error"].as_str().unwrap().contains("missing"));
    for suffix in ["?type=missing", "?n=bad"] {
        let (status, body) = node.get(&format!("/api/projects/{key}/queue/human{suffix}"));
        assert_eq!(status, 400, "{body}");
    }
    let (status, body) = node.get("/api/projects/missing/queue/human");
    assert_eq!(status, 404, "{body}");
    let (status, body) = node.get(&format!(
        "/api/projects/{key}/next?status=review&role=missing"
    ));
    assert_eq!(status, 400, "{body}");
    let (status, body) = node.get(&format!(
        "/api/projects/{key}/next?status=review&role=human"
    ));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["role"], "human");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], "TASK-0001");
}

#[test]
fn role_endpoints_merge_exact_names_preserve_order_and_keep_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let first = project(&tmp.path().join("first"), CONFIG);
    doc(&first, "TASK-0001", "review", "");
    let second_config = CONFIG
        .replace("[roles.agent]\ndescription = \"Implementation\"\n", "")
        .replace("todo = \"agent\"", "todo = []")
        .replace("review = [\"human\", \"agent\"]", "review = \"human\"")
        .replace("Sign-offs", "Review");
    let second = project(&tmp.path().join("second"), &second_config);
    doc(&second, "TASK-0002", "review", "");
    let unowned = project(&tmp.path().join("unowned"), "base = \"inventory\"\n");
    let broken = project(&tmp.path().join("broken"), "base = = invalid\n");
    let node = Node::start(tmp.path(), &[&first, &second, &unowned, &broken]);
    let (first_key, _) = node.project(&first);
    let (second_key, _) = node.project(&second);
    let (broken_key, _) = node.project(&broken);
    let (status, body) = node.get("/api/roles");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["schema"], "opys.roles/1");
    let roles = body["roles"].as_array().unwrap();
    assert_eq!(
        roles
            .iter()
            .map(|role| role["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["agent", "human"]
    );
    let human = &roles[1];
    assert_eq!(human["total"], 2, "{body}");
    let totals: Vec<_> = human["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| project["total"].as_u64().unwrap())
        .collect();
    assert_eq!(totals, [1, 1]);
    assert_eq!(human["projects"][0]["description"], "Sign-offs");
    assert_eq!(human["projects"][1]["description"], "Review");
    assert_eq!(body["errors"][0]["key"], broken_key);
    assert!(body["errors"][0].get("total").is_none());
    let (status, body) = node.get("/api/roles/human");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["schema"], "opys.role/1");
    assert_eq!(body["role"], "human");
    let projects = body["projects"].as_array().unwrap();
    let keys: Vec<_> = projects
        .iter()
        .map(|project| project["key"].as_str().unwrap())
        .collect();
    let expected: Vec<_> = node
        .groups()
        .iter()
        .filter_map(|project| {
            let key = project["key"].as_str().unwrap();
            [first_key.as_str(), second_key.as_str(), broken_key.as_str()]
                .contains(&key)
                .then(|| key.to_string())
        })
        .collect();
    assert_eq!(keys, expected);
    let failed = projects
        .iter()
        .find(|project| project["key"] == broken_key)
        .unwrap();
    assert!(failed.get("error").is_some());
    assert!(failed.get("queue").is_none());
    let (_, agent) = node.get("/api/roles/agent");
    assert_eq!(
        agent["projects"].as_array().unwrap().len(),
        2,
        "the second project does not declare agent; the broken project stays an error: {agent}"
    );
    let (_, unknown) = node.get("/api/roles/unknown");
    assert_eq!(
        unknown["projects"].as_array().unwrap().len(),
        1,
        "only the unreadable project remains unknown: {unknown}"
    );
}

#[test]
fn flow_and_doc_expose_owners_and_status_action_clears_override() {
    let tmp = tempfile::tempdir().unwrap();
    let root = project(&tmp.path().join("project"), CONFIG);
    doc(&root, "TASK-0001", "todo", "owner: human\n");
    let node = Node::start(tmp.path(), &[&root]);
    let (key, cid) = node.project(&root);
    let (status, flow) = node.get(&format!("/api/projects/{key}/flow"));
    assert_eq!(status, 200, "{flow}");
    let review = flow["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["status"] == "review")
        .unwrap();
    assert_eq!(review["owners"], json!(["human", "agent"]));
    let todo = flow["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["status"] == "todo")
        .unwrap();
    assert_eq!(todo["owners"], json!(["agent"]));
    let (status, view) = node.get(&format!("/api/corpus/{cid}/doc/TASK-0001"));
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["owners"], json!(["human"]));
    assert_eq!(view["owner_override"], true);
    let owner = view["declared_fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "owner")
        .unwrap();
    assert_eq!(owner["type"], "role");
    assert_eq!(owner["values"], json!(["agent", "human"]));
    let (status, body) = node.request(
        "POST",
        &format!("/api/corpus/{cid}/action"),
        Some(&json!({"action":"set-status", "id":"TASK-0001", "status":"review"})),
    );
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let text = std::fs::read_to_string(root.join("inventory/TASK-0001.md")).unwrap();
    assert!(
        !text.contains("owner:"),
        "status action must remove the override: {text}"
    );
    let (_, view) = node.get(&format!("/api/corpus/{cid}/doc/TASK-0001"));
    assert_eq!(view["owners"], json!(["human"]));
    assert!(
        view.get("owner_override").is_none(),
        "false override flag is omitted: {view}"
    );
    assert!(view["fields"].get("owner").is_none());
}

#[cfg(feature = "git")]
#[test]
fn project_queue_uses_primary_worktree() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("skipping project_queue_uses_primary_worktree: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = project(&tmp.path().join("primary"), CONFIG);
    doc(&root, "TASK-0001", "review", "");
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init"]);
    git(&["add", "-A"]);
    git(&["commit", "-m", "fixture"]);
    let branch = tmp.path().join("branch");
    git(&[
        "worktree",
        "add",
        "-b",
        "feature/roles",
        branch.to_str().unwrap(),
    ]);
    doc(&branch, "TASK-0001", "todo", "");
    let node = Node::start(tmp.path(), &[&branch]);
    let (key, _) = node.project(&branch);
    let (status, body) = node.request("GET", &format!("/api/projects/{key}/queue/human"), None);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, cli(&root, &["queue", "human", "--json"]));
    assert_ne!(body, cli(&branch, &["queue", "human", "--json"]));
}
