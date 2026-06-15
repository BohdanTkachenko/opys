//! String templates emitted by `opys init` and `opys agent-rules`.

/// The canonical always-on agent rule, the single source for every per-editor
/// rule file `opys agent-rules` generates. Embedded at build time so there is
/// exactly one copy in the repo.
pub const AGENT_RULE: &str = include_str!("../skills/opys/agent-rule.md");

pub const DEFAULT_CONFIG: &str = r#"# opys feature inventory configuration
# Feature IDs are always FEAT-NNNN (the prefix is fixed, not configurable).
pad = 4                # zero-padding width for the numeric part

# Directories searched (recursively) when verifying that test references
# named in test plans actually exist in the codebase.
test_search_paths = ["src", "tests"]

# How test references in test plans are validated:
#   "grep"    - the test name must appear as a substring under test_search_paths
#   "extract" - extract real test names via test_name_pattern and resolve each
#               reference against that set (and the named file for path::name refs)
#   "none"    - skip existence checking (e.g. before any tests exist)
test_reference_check = "grep"

# Regex with ONE capture group that extracts a test name from source.
# Required when test_reference_check = "extract". Example for Rust:
# test_name_pattern = "fn\\s+(\\w+)\\s*\\("

# Additional statuses beyond planned | partial | implemented | wontfix.
extra_statuses = []

# Report feature-parity percentages. Enable only for parity projects
# (e.g. matching another product feature-for-feature).
parity = false

# Per-project custom frontmatter fields. Example:
# [fields.upstream_ref]
# type = "string"          # string | list | bool | int | enum
# required = false
# description = "Pointer into the upstream source establishing reference behavior"
#
# An enum field constrains the value to a declared set (filter with
# `opys list --field priority=high`):
# [fields.priority]
# type = "enum"
# values = ["low", "medium", "high"]
"#;

pub const CLAUDE_MD_SNIPPET: &str = r#"## Feature inventory

- The feature inventory lives in `docs/opys/features/`, one markdown file per
  feature. Source of truth is the feature files; `docs/opys/features/INDEX.md`
  and `docs/opys/views/` are generated — read them, never edit them.
- To find features: read `docs/opys/features/INDEX.md` first, then `rg` by
  tag/status, then read only the relevant feature files. Do not bulk-read the
  inventory.
- To create features or change status/tags, use `opys` (new, set-status, tag,
  retire); these regenerate INDEX.md/views automatically. Spec prose and
  test-plan edits are normal file edits — run `opys verify` before finishing.
- When implementing a feature: read its file fully; implement; add tests;
  check the matching test-plan items and append backticked test references;
  set status via the CLI.
- If a test plan's case enumeration looks incomplete, raise it — do not
  silently implement only the listed cases.
- Never record test results, dates, or completion claims in feature files.
- Track in-flight implementation work in *work items* (`opys work-item …`),
  not in feature files. Run `opys work-item init` to enable them.
"#;

pub const DEFAULT_WI_CONFIG: &str = r#"# work-item subsystem configuration
# Work items come in hardcoded types, each with its own fixed ID prefix:
#   task → TASK-NNNN   bug → BUG-NNNN   chore → CHORE-NNNN
# Pick one with `opys work-item new --type bug` (default: task).
pad = 4                # zero-padding width for the numeric part

# Additional statuses beyond todo | in-progress | blocked | done.
extra_statuses = []

# Body sections every work item must contain (verified; scaffolded by `new`).
# This is the shared baseline; some types add their own (e.g. bug → Reproduction).
required_sections = ["Tasks", "Progress"]

# Per-project custom frontmatter fields. Example:
# [fields.pr]
# type = "string"          # string | list | bool | int | enum
# required = false
# description = "Primary pull-request URL for this effort"
# (an enum field adds `values = [...]` and is filterable via `list --field`)
"#;

pub const WI_CLAUDE_MD_SNIPPET: &str = r#"## Work items

- Work items are the ephemeral companions to features: one markdown file per
  in-flight change in `docs/opys/work-items/`, holding `## Tasks` and a
  `## Progress` log (branch/commit/PR links). They are deleted on completion.
- Start work: `opys work-item new --type bug --title "…" --features FEAT-0001`
  (types: task/bug/chore → TASK-/BUG-/CHORE- ids; default task). Every work item
  must link at least one existing feature. Editing Tasks/Progress is a normal
  file edit.
- opys keeps cross-references in sync automatically: a feature's `references`
  map and a work item's `references` map are kept bidirectional and titled, and
  bare feature/work-item ID mentions in prose are rewritten into markdown links.
- Finish: fold anything durable back into the feature (test plan, status, spec),
  then `opys work-item close BUG-0001`. Close deletes the file and strikes the
  reference through in the feature as a tombstone — nothing else survives.
- Never put permanent docs in a work item, or implementation logs in a feature.
"#;

/// The opinionated default `opys.toml` written by `opys config init`. This is
/// the target shape for the upcoming universal typed-document engine; opys does
/// not consume it yet. A unit test below pins that it stays valid TOML.
pub const DEFAULT_OPYS_CONFIG: &str = r##"# opys.toml — universal typed-document engine config.
# Generated by `opys config init`. NOTE: opys does not read this file yet; it is
# the target config shape for the upcoming engine. Edit it to model your types.

pad = 4                              # zero-padding width for the numeric id part

# Test-reference resolution, used by sections of kind "test-plan".
[tests]
search_paths = ["src", "tests"]
reference_check = "grep"             # "grep" | "extract" | "none"
# name_pattern = "fn\\s+(\\w+)\\s*\\("   # required when reference_check = "extract"

[report]
parity = false                       # report feature-parity percentages

# ---------------------------------- feature ----------------------------------
# Permanent description of product behavior. A feature removed from the product
# becomes status "archived" (kept in the inventory), never deleted.
[types.feature]
prefix = "FEAT"
statuses = ["planned", "partial", "implemented", "wontfix", "archived"]
default_status = "planned"
terminal_statuses = []               # features are never closed/deleted
tags_required = true

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
kind = "test-plan"                   # checked items carry resolvable test refs

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

[[types.task.sections]]
heading = "Progress"
kind = "log"

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

[[types.bug.sections]]
heading = "Tasks"
kind = "checklist"

[[types.bug.sections]]
heading = "Progress"
kind = "log"

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

[[types.chore.sections]]
heading = "Progress"
kind = "log"

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
