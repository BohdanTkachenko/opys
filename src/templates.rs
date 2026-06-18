//! String templates emitted by `opys init` and `opys agent-rules`.

/// The canonical always-on agent rule, the single source for every per-editor
/// rule file `opys agent-rules` generates. Embedded at build time so there is
/// exactly one copy in the repo.
pub const AGENT_RULE: &str = include_str!("../skills/opys/agent-rule.md");

pub const CLAUDE_MD_SNIPPET: &str = r#"## Feature inventory

- opys manages a file-based inventory of typed documents under `opys/`,
  one markdown file per document; the document *types* (their ID prefixes,
  statuses, fields, required sections, and validation rules) are configured in
  `opys.toml`.
- To find documents: `rg` by tag/status or `opys list`, then read only the
  relevant files. Do not bulk-read the inventory.
- To create or change documents, use `opys` (`new --type`, set-status, tag,
  retire, block, close). Body prose and section edits are normal file edits —
  run `opys verify` before finishing.
- When implementing a feature: read its file fully; implement; add tests; check
  the matching test-plan items and append backticked test references; set status
  via the CLI. If a test plan's case enumeration looks incomplete, raise it.
- Track ephemeral implementation work in work-item-style types (e.g.
  `opys new --type bug --features FEAT-0001`), and `opys close` them when done.
  Never record test results, dates, or completion claims in documents.
"#;

/// The opinionated default `opys.toml` written by `opys init` / `opys config
/// init`. A unit test below pins that it stays valid TOML.
pub const DEFAULT_OPYS_CONFIG: &str = r##"# opys.toml — the opys document-inventory config. Lives at the project root;
# opys finds it by searching upward from the current directory.

base = "opys"                        # inventory dir (relative to this file)
pad = 4                              # zero-padding width for the numeric id part

# On-disk layout. Each document's path (under `base`) is this template with
# {type} → the type's `dir`, {status} → the type's `status_dirs[status]`, and
# {id} → PREFIX-NNNN. Both segments are empty by default, so documents live flat
# at the base (e.g. opys/FEAT-0001.md). Empty segments collapse, so the order is
# free — e.g. "{status}/{type}/{id}.md" groups by status first.
[layout]
path = "{type}/{status}/{id}.md"

# ---------------------------------- feature ----------------------------------
# Permanent description of product behavior. A feature removed from the product
# becomes status "archived" (kept in the inventory), never deleted.
[types.feature]
prefix = "FEAT"
statuses = ["planned", "partial", "implemented", "wontfix", "archived"]
default_status = "planned"
terminal_statuses = []               # features are never closed/deleted
tags_required = true
status_dirs = { archived = "_archived" }   # archived features move to opys/_archived/

[types.feature.fields.spec]
type = "string"
pattern = '^\S.*$'                   # non-empty, single line
description = "Path to long-form shared spec material"

[types.feature.fields.wontfix_reason]
type = "string"

[types.feature.fields.archived_reason]
type = "string"

[[types.feature.sections]]
heading = "Test plan"
kind = "checklist"                   # structure: checkbox items

# Each checked item must carry a resolvable test reference. `pattern` parses a
# `mod::name` backtick span; `must_match` greps the test name under `roots`.
[[types.feature.sections.checks]]
pattern = '`(?P<ref>[^`]*::(?P<name>[^`]+))`'
roots = ["src", "tests"]
must_match = '${name}'               # ${group} = the regex-escaped capture
scope = "checked"                    # "all" (every line) | "checked" (checked items)
message = "test reference `${ref}` not found"

[[types.feature.sections]]
heading = "Manual verification"
kind = "manual"                      # items need Setup / Steps / Expect

# ----------------------------------- task ------------------------------------
# Ephemeral implementation work. Deleted on `close`.
[types.task]
prefix = "TASK"
statuses = ["todo", "in-progress", "blocked", "done"]
default_status = "todo"
terminal_statuses = ["done"]         # "done" reached only via `close`
tags_required = false
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

# ------------------------------------ bug ------------------------------------
# Like task, plus a required Reproduction section.
[types.bug]
prefix = "BUG"
statuses = ["todo", "in-progress", "blocked", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
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

# ----------------------------------- chore -----------------------------------
# Maintenance/tooling work with no user-facing behavior change.
[types.chore]
prefix = "CHORE"
statuses = ["todo", "in-progress", "blocked", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
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

# ------------------------------ validation rules -----------------------------
# Each rule: an optional `when { type?, status? }` + one assertion. Closed set:
# require_field, field_matches, require_section, require_checked_section,
# require_link, require_any.

[[rules]]
when = { type = "feature", status = "wontfix" }
require_field = "wontfix_reason"

[[rules]]
when = { type = "feature", status = "archived" }
require_field = "archived_reason"

[[rules]]
when = { type = "feature", status = "implemented" }
require_checked_section = "Test plan"

[[rules]]
when = { status = "blocked" }
require_any = [{ field = "blocked_reason" }, { link = "blocked_by" }]
"##;

#[cfg(test)]
mod tests {
    /// The shipped default config must always be valid TOML, so `config init`
    /// can never emit a file that fails to parse.
    #[test]
    fn default_opys_config_is_valid_toml() {
        toml::from_str::<toml::Value>(super::DEFAULT_OPYS_CONFIG)
            .expect("DEFAULT_OPYS_CONFIG must be valid TOML");
    }
}
