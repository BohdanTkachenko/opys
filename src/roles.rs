//! Declared roles and effective status ownership (ADR-0068).
//!
//! This is the only interpreter of the role declarations, status owner maps,
//! the per-document override and `escalate_to`. Consumers ask it for derived
//! answers instead of inferring workers from status or role names.

use std::collections::HashSet;

use crate::config::FieldType;
use crate::doc::Doc;
use crate::project_config::ProjectConfig;

/// The opt-in role field that overrides ownership for one status visit.
pub const OWNER_FIELD: &str = "owner";

/// Declared role names in stable alphabetical order.
pub fn names(pcfg: &ProjectConfig) -> Vec<&str> {
    pcfg.roles.keys().map(String::as_str).collect()
}

pub fn declared(pcfg: &ProjectConfig, role: &str) -> bool {
    pcfg.roles.contains_key(role)
}

pub fn description<'a>(pcfg: &'a ProjectConfig, role: &str) -> Option<&'a str> {
    pcfg.roles.get(role)?.description.as_deref()
}

/// The configured intervention recipient, if any. Validation reports a name
/// that is not declared; readers never invent a fallback recipient.
pub fn escalation_role(pcfg: &ProjectConfig) -> Option<&str> {
    pcfg.escalate_to.as_deref()
}

pub fn validate_role(pcfg: &ProjectConfig, role: &str) -> Result<(), String> {
    if declared(pcfg, role) {
        Ok(())
    } else {
        Err(format!(
            "unknown role {role:?} (declared roles: {})",
            legal_roles(pcfg)
        ))
    }
}

pub fn missing_role(pcfg: &ProjectConfig) -> String {
    format!(
        "a role is required (pass a role or set OPYS_ROLE; declared roles: {})",
        legal_roles(pcfg)
    )
}

fn legal_roles(pcfg: &ProjectConfig) -> String {
    let roles = names(pcfg);
    if roles.is_empty() {
        "none".to_string()
    } else {
        roles.join(", ")
    }
}

/// Owners of one declared status, preserving the configured preference order.
pub fn status_owners(pcfg: &ProjectConfig, type_name: &str, status: &str) -> Vec<String> {
    pcfg.types
        .get(type_name)
        .and_then(|t| t.owners.as_ref())
        .and_then(|owners| owners.get(status))
        .cloned()
        .unwrap_or_default()
}

/// Whether this type opted into the one-visit override field. An ordinary
/// string field called `owner` keeps its previous meaning on an unowned type.
pub fn override_enabled(pcfg: &ProjectConfig, type_name: &str) -> bool {
    pcfg.types
        .get(type_name)
        .and_then(|t| t.fields.get(OWNER_FIELD))
        .is_some_and(|f| f.field_type == FieldType::Role)
}

/// Whether the effective owners come from a set, role-typed override.
pub fn is_override(pcfg: &ProjectConfig, doc: &Doc) -> bool {
    override_value(pcfg, doc).is_some_and(serde_norway::Value::is_string)
}

/// The override's raw value, including a malformed value verify must report.
pub fn override_value<'a>(pcfg: &ProjectConfig, doc: &'a Doc) -> Option<&'a serde_norway::Value> {
    if doc
        .id()
        .and_then(|id| pcfg.type_name_for_id(id))
        .is_some_and(|name| override_enabled(pcfg, name))
    {
        doc.frontmatter.get(OWNER_FIELD)
    } else {
        None
    }
}

pub fn effective_owners(pcfg: &ProjectConfig, doc: &Doc) -> Vec<String> {
    if is_override(pcfg, doc) {
        return vec![doc.frontmatter.get_str(OWNER_FIELD).unwrap().to_string()];
    }
    let Some(type_name) = doc.id().and_then(|id| pcfg.type_name_for_id(id)) else {
        return Vec::new();
    };
    status_owners(pcfg, type_name, doc.status().unwrap_or_default())
}

pub fn waits_on(pcfg: &ProjectConfig, doc: &Doc, role: &str) -> bool {
    effective_owners(pcfg, doc)
        .iter()
        .any(|owner| owner == role)
}

/// Clear the opted-in override when the caller moves to a different status.
/// Invalid values are cleared too: they belong to the visit being left.
pub fn enter_status(pcfg: &ProjectConfig, doc: &mut Doc, retained: Option<&serde_norway::Value>) {
    if doc
        .id()
        .and_then(|id| pcfg.type_name_for_id(id))
        .is_some_and(|name| override_enabled(pcfg, name))
    {
        doc.frontmatter.remove(OWNER_FIELD);
        if let Some(value) = retained {
            doc.frontmatter.insert(OWNER_FIELD, value.clone());
        }
    }
}

/// A role field's value must be a string naming a declared role. The caller
/// attaches the resulting message to the field's existing verify location.
pub fn field_problem(pcfg: &ProjectConfig, value: &serde_norway::Value) -> Option<String> {
    match value.as_str() {
        Some(role) if declared(pcfg, role) => None,
        Some(role) => Some(format!(
            "value '{role}' is not a declared role (declared roles: {})",
            legal_roles(pcfg)
        )),
        None => Some(format!(
            "should be a declared role (declared roles: {})",
            legal_roles(pcfg)
        )),
    }
}

/// Every role-related config problem, collected rather than raised.
pub fn config_problems(pcfg: &ProjectConfig) -> Vec<String> {
    let mut problems = Vec::new();
    for role in names(pcfg) {
        let mut chars = role.chars();
        let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            problems.push(format!("role '{role}': name must match ^[a-z][a-z0-9-]*$"));
        }
    }
    if let Some(role) = escalation_role(pcfg) {
        if !declared(pcfg, role) {
            problems.push(format!(
                "escalate_to '{role}' is not a declared role (declared roles: {})",
                legal_roles(pcfg)
            ));
        }
    }
    for (name, t) in &pcfg.types {
        if let Some(owners) = &t.owners {
            for (status, roles) in owners {
                if !t.statuses.contains(status) {
                    problems.push(format!(
                        "type '{name}': owners key '{status}' is not a status (statuses: {})",
                        t.statuses.join(", ")
                    ));
                }
                if t.terminal_statuses.contains(status) {
                    problems.push(format!(
                        "type '{name}': owners key '{status}' is terminal (non-terminal statuses: {})",
                        t.statuses.iter().filter(|s| !t.terminal_statuses.contains(s)).cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                let mut seen = HashSet::new();
                for role in roles {
                    if !declared(pcfg, role) {
                        problems.push(format!(
                            "type '{name}': owners key '{status}' names undeclared role '{role}' (declared roles: {})",
                            legal_roles(pcfg)
                        ));
                    }
                    if !seen.insert(role) {
                        problems.push(format!(
                            "type '{name}': owners key '{status}' names role '{role}' more than once (declare each role once; declared roles: {})",
                            legal_roles(pcfg)
                        ));
                    }
                }
            }
            for status in &t.statuses {
                if !t.terminal_statuses.contains(status) && !owners.contains_key(status) {
                    problems.push(format!(
                        "type '{name}': owners is missing non-terminal status '{status}' (declare owners or [] for every non-terminal status: {})",
                        t.statuses.iter().filter(|s| !t.terminal_statuses.contains(s)).cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            if t.fields
                .get(OWNER_FIELD)
                .is_some_and(|f| f.field_type != FieldType::Role)
            {
                problems.push(format!(
                    "type '{name}': field '{OWNER_FIELD}' must have type = \"role\" when owners is declared"
                ));
            }
        }
        for (field, spec) in &t.fields {
            if spec.field_type == FieldType::Role && pcfg.roles.is_empty() {
                problems.push(format!(
                    "type '{name}': field '{field}' has type = \"role\" but no roles are declared (declare [roles.<name>])"
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{apply, set_status, verify};
    use crate::project::Project;
    use crate::store::{LoadedCorpus, Store};

    const CONFIG: &str = r#"
escalate_to = "human"
[roles.human]
description = "Reviews and decisions"
[roles.agent]
[types.task]
prefix = "TASK"
statuses = ["todo", "review", "settled", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.owners]
todo = ["agent", "human"]
review = "human"
settled = []
[types.task.fields.owner]
type = "role"
[types.task.fields.reviewer]
type = "role"
[types.task.ttl]
todo = { after = "1d", then = "review" }
[types.note]
prefix = "NOTE"
statuses = ["draft"]
default_status = "draft"
"#;

    fn config() -> ProjectConfig {
        toml::from_str(CONFIG).unwrap()
    }

    fn doc(id: &str, status: &str, owner: Option<&str>) -> Doc {
        let owner = owner.map(|r| format!("owner: {r}\n")).unwrap_or_default();
        Doc::parse(
            format!("/p/opys/{id}.md").into(),
            &format!(
                "---\nid: {id}\nstatus: {status}\ncreated: \"2026-01-01T00:00:00Z\"\nupdated: \"2026-01-01T00:00:00Z\"\nhistory:\n  \"2026-01-01T00:00:00.000000Z\": {status}\n{owner}---\n\n# Work\n"
            ),
        )
        .unwrap()
    }

    fn fixture() -> (tempfile::TempDir, Project, Store) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("opys.toml"), CONFIG).unwrap();
        std::fs::create_dir(dir.path().join("opys")).unwrap();
        let project = Project::open(&dir.path().to_string_lossy()).unwrap();
        let mut document = doc("TASK-0001", "todo", Some("human"));
        document.path = project.base.join("TASK-0001.md");
        let (store, errors) = Store::build(
            &project,
            LoadedCorpus {
                docs: vec![(document, None)],
                errors: Vec::new(),
                retired: Vec::new(),
                retired_legacy: false,
                retired_err: None,
            },
        )
        .unwrap();
        assert!(errors.is_empty());
        (dir, project, store)
    }

    fn current(store: &mut Store) -> Doc {
        let key = store.dkey_of("TASK-0001").unwrap();
        store.doc(key).unwrap()
    }

    #[test]
    fn role_tables_are_closed_and_names_are_validated() {
        let pcfg = config();
        assert!(pcfg.validate().is_empty(), "{:?}", pcfg.validate());
        assert_eq!(names(&pcfg), ["agent", "human"]);
        assert_eq!(description(&pcfg, "human"), Some("Reviews and decisions"));
        assert_eq!(status_owners(&pcfg, "task", "review"), ["human"]);
        let bad_key = CONFIG.replace("description =", "kind =");
        assert!(toml::from_str::<ProjectConfig>(&bad_key)
            .unwrap_err()
            .to_string()
            .contains("unknown field `kind`"));
        for name in ["Human", "-human", "1human", "human_name", ""] {
            let mut pcfg = config();
            pcfg.roles.insert(name.into(), Default::default());
            assert!(pcfg
                .validate()
                .iter()
                .any(|p| p.contains("must match ^[a-z][a-z0-9-]*$")));
        }
    }

    #[test]
    fn config_validation_collects_every_role_and_owners_problem() {
        let invalid = r#"
escalate_to = "missing"
[roles.human]
[types.task]
prefix = "TASK"
statuses = ["todo", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
[types.task.owners]
typo = "missing"
done = ["human", "human"]
review = []
[types.task.fields.owner]
type = "string"
"#;
        let pcfg: ProjectConfig = toml::from_str(invalid).unwrap();
        let problems = pcfg.validate();
        assert_eq!(problems.len(), 7, "{problems:?}");
        for expected in [
            "escalate_to 'missing'",
            "owners key 'typo' is not a status",
            "owners key 'done' is terminal",
            "names undeclared role 'missing'",
            "names role 'human' more than once",
            "missing non-terminal status 'todo'",
            "field 'owner' must have type = \"role\"",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(expected)),
                "{expected}: {problems:?}"
            );
        }
        let no_roles = r#"
[types.task]
prefix = "TASK"
statuses = ["todo"]
default_status = "todo"
[types.task.fields.reviewer]
type = "role"
"#;
        let no_roles: ProjectConfig = toml::from_str(no_roles).unwrap();
        assert!(no_roles
            .validate()
            .iter()
            .any(|p| p.contains("no roles are declared")));

        // An explicitly empty table opts in, so it still requires coverage.
        let empty = CONFIG.replace(
            "todo = [\"agent\", \"human\"]\nreview = \"human\"\nsettled = []",
            "",
        );
        let empty: ProjectConfig = toml::from_str(&empty).unwrap();
        assert_eq!(empty.validate().len(), 3);
        assert!(config().validate().is_empty(), "[] explicitly means nobody");
    }

    #[test]
    fn effective_owners_override_status_and_unowned_is_empty() {
        let pcfg = config();
        let overridden = doc("TASK-0001", "todo", Some("human"));
        assert_eq!(effective_owners(&pcfg, &overridden), ["human"]);
        assert!(is_override(&pcfg, &overridden));
        assert!(waits_on(&pcfg, &overridden, "human"));
        assert!(!waits_on(&pcfg, &overridden, "agent"));
        let plain = doc("TASK-0002", "todo", None);
        assert_eq!(effective_owners(&pcfg, &plain), ["agent", "human"]);
        assert!(!is_override(&pcfg, &plain));
        assert!(effective_owners(&pcfg, &doc("TASK-0003", "settled", None)).is_empty());
        assert!(effective_owners(&pcfg, &doc("NOTE-0004", "draft", None)).is_empty());
        let error = validate_role(&pcfg, "missing").unwrap_err();
        assert!(error.contains("declared roles: agent, human"), "{error}");
    }

    #[test]
    fn verify_collects_undeclared_and_wrong_shaped_role_fields() {
        let (_dir, project, _store) = fixture();
        let mut document = doc("TASK-0001", "todo", Some("missing"));
        document.frontmatter.set_str("reviewer", "human");
        let problems = verify::collect_findings(&project, &[document.clone()], Vec::new());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0]
            .message
            .contains("field 'owner' value 'missing' is not a declared role"));
        assert_eq!(problems[0].doc_id.as_deref(), Some("TASK-0001"));
        document.frontmatter.set_str("owner", "agent");
        document
            .frontmatter
            .insert("reviewer", serde_norway::Value::Bool(true));
        let problems = verify::collect_findings(&project, &[document], Vec::new());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0]
            .message
            .contains("field 'reviewer' should be a declared role"));
    }

    #[test]
    fn status_apply_and_verify_fixes_clear_only_previous_visit_override() {
        let (_dir, project, mut store) = fixture();
        set_status::core(&project, &mut store, "TASK-0001", "todo", None).unwrap();
        assert!(
            is_override(&project.pcfg, &current(&mut store)),
            "same status keeps owner"
        );
        set_status::core(&project, &mut store, "TASK-0001", "review", None).unwrap();
        assert!(current(&mut store).frontmatter.get(OWNER_FIELD).is_none());

        let (_dir, project, mut store) = fixture();
        apply::core(
            &project,
            &mut store,
            "TASK-0001",
            &apply::Apply {
                status: Some("review"),
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert!(current(&mut store).frontmatter.get(OWNER_FIELD).is_none());

        let (_dir, project, mut store) = fixture();
        let fields = [(OWNER_FIELD.to_string(), "agent".to_string())];
        apply::core(
            &project,
            &mut store,
            "TASK-0001",
            &apply::Apply {
                status: Some("review"),
                set_fields: &fields,
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            current(&mut store).frontmatter.get_str(OWNER_FIELD),
            Some("agent")
        );

        let (_dir, project, mut store) = fixture();
        let now = crate::history::parse("2026-01-03T00:00:00Z").unwrap();
        let observations = crate::observe::observations(&project, &[current(&mut store)], now);
        assert_eq!(observations.len(), 1);
        let fixes = crate::observe::apply_fixes(&project, &mut store, &observations);
        assert!(fixes.refused.is_empty(), "{:?}", fixes.refused);
        assert_eq!(fixes.applied.len(), 1);
        assert!(current(&mut store).frontmatter.get(OWNER_FIELD).is_none());
    }

    #[test]
    fn ordinary_owner_field_on_unowned_type_keeps_existing_semantics() {
        let (_dir, mut project, mut store) = fixture();
        project.pcfg = toml::from_str(&CONFIG.replace("type = \"role\"", "type = \"string\"").replace("[types.task.owners]\ntodo = [\"agent\", \"human\"]\nreview = \"human\"\nsettled = []\n", "")).unwrap();
        set_status::core(&project, &mut store, "TASK-0001", "review", None).unwrap();
        assert_eq!(
            current(&mut store).frontmatter.get_str(OWNER_FIELD),
            Some("human")
        );
        assert!(!is_override(&project.pcfg, &current(&mut store)));
    }

    #[test]
    fn target_rules_see_only_an_owner_written_for_the_new_visit() {
        let (_dir, mut project, mut store) = fixture();
        project.pcfg = toml::from_str(&format!(
            "{CONFIG}\n[[rules]]\nwhen = {{ type = \"task\", status = \"review\" }}\nrequire_field = \"owner\"\n"
        )).unwrap();
        let error =
            set_status::core(&project, &mut store, "TASK-0001", "review", None).unwrap_err();
        assert!(error.to_string().contains("owner"), "{error}");
        assert_eq!(current(&mut store).status(), Some("todo"));

        let fields = [(OWNER_FIELD.to_string(), "agent".to_string())];
        apply::core(
            &project,
            &mut store,
            "TASK-0001",
            &apply::Apply {
                status: Some("review"),
                set_fields: &fields,
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            current(&mut store).frontmatter.get_str(OWNER_FIELD),
            Some("agent")
        );
    }

    #[test]
    fn verify_fix_core_uses_the_same_override_clearing_transition() {
        let (_dir, project, mut store) = fixture();
        let key = store.dkey_of("TASK-0001").unwrap();
        let mut document = current(&mut store);
        // A clock-independent old visit, without changing the process clock
        // shared by the other unit tests.
        document
            .frontmatter
            .set_str("created", "2000-01-01T00:00:00Z");
        let history: serde_norway::Value =
            serde_norway::from_str("{\"2000-01-01T00:00:00.000000Z\": todo}").unwrap();
        document.frontmatter.insert(crate::history::FIELD, history);
        store.put_doc(&project.pcfg, Some(key), &document).unwrap();
        let fixes = verify::fix_core(&project, &mut store, None).unwrap();
        assert_eq!(fixes.applied.len(), 1);
        assert!(fixes.refused.is_empty(), "{:?}", fixes.refused);
        let document = current(&mut store);
        assert_eq!(document.status(), Some("review"));
        assert!(document.frontmatter.get(OWNER_FIELD).is_none());
    }
}
