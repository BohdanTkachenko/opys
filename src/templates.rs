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
# Work-item IDs are always WI-NNNN (the prefix is fixed, not configurable).
pad = 4                # zero-padding width for the numeric part

# Additional statuses beyond todo | in-progress | blocked | done.
extra_statuses = []

# Body sections every work item must contain (verified; scaffolded by `new`).
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
- Start work: `opys work-item new --title "…" --features FEAT-0001`. Every work
  item must link at least one existing feature. Editing Tasks/Progress is a
  normal file edit.
- opys keeps cross-references in sync automatically: a feature's `references`
  map and a work item's `references` map are kept bidirectional and titled, and
  bare FEAT-/WI- mentions in prose are rewritten into markdown links.
- Finish: fold anything durable back into the feature (test plan, status, spec),
  then `opys work-item close WI-0001`. Close deletes the file and strikes the
  reference through in the feature as a tombstone — nothing else survives.
- Never put permanent docs in a work item, or implementation logs in a feature.
"#;
