//! String templates emitted by `opys init`.

pub const DEFAULT_CONFIG: &str = r#"# feature-inventory project configuration
prefix = "FEAT"        # feature ID prefix -> FEAT-0001
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
# type = "string"          # string | list | bool | int
# required = false
# description = "Pointer into the upstream source establishing reference behavior"
"#;

pub const CLAUDE_MD_SNIPPET: &str = r#"## Feature inventory

- The feature inventory lives in `docs/features/`, one markdown file per
  feature. Source of truth is the feature files; `docs/features/INDEX.md` and
  `docs/views/` are generated — read them, never edit them.
- To find features: read `docs/features/INDEX.md` first, then `rg` by
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
"#;
