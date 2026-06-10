//! String templates emitted by `opys init`.

pub const DEFAULT_CONFIG: &str = r#"# feature-inventory project configuration
prefix = "FEAT"        # feature ID prefix -> FEAT-0001
pad = 4                # zero-padding width for the numeric part

# Directories searched (recursively) when verifying that test references
# named in test plans actually exist in the codebase.
test_search_paths = ["src", "tests"]

# "grep": every test reference must appear somewhere under test_search_paths.
# "none": skip existence checking (e.g. before any tests exist).
test_reference_check = "grep"

# Additional statuses beyond planned | partial | implemented | wontfix.
extra_statuses = []

# Per-project custom frontmatter fields. Example:
# [fields.upstream_ref]
# type = "string"          # string | list | bool | int
# required = false
# description = "Pointer into the upstream source establishing reference behavior"
"#;

pub const CLAUDE_MD_SNIPPET: &str = r#"## Feature inventory

- The feature inventory lives in `features/`, one markdown file per feature.
  Source of truth is the feature files; `features/INDEX.md` and `views/` are
  generated — read them, never edit them.
- To find features: read `features/INDEX.md` first, then `rg` by tag/status,
  then read only the relevant feature files. Do not bulk-read `features/`.
- To create features or change status/tags, use `opys` (new, set-status, tag,
  retire). Spec prose and test-plan edits are normal file edits, but run
  `opys verify` before finishing.
- When implementing a feature: read its file fully; implement; add tests;
  check the matching test-plan items and append backticked test references;
  set status via the CLI.
- If a test plan's case enumeration looks incomplete, raise it — do not
  silently implement only the listed cases.
- Never record test results, dates, or completion claims in feature files.
"#;
