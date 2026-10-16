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
  log, apply, retire, block, close). Body prose and section edits are normal file
  edits, with one exception: a `kind = "log"` section (typically `## Progress`)
  holds dated entries, so append to it with `opys log <id> "…"`, never by hand.
  `opys apply <id> --status … --field … --log "…"` performs a status change,
  field writes and a log entry as one all-or-nothing batch — how a document is
  claimed, since a second claimant is refused and writes nothing.
  Run `opys verify` before finishing.
- When implementing a feature: read its file fully; implement; add tests; check
  the matching test-plan items and append backticked test references; set status
  via the CLI. If a test plan's case enumeration looks incomplete, raise it.
- Track ephemeral implementation work in work-item-style types (e.g.
  `opys new --type bug --features FEAT-0001`), and `opys close` them when the
  work lands — move one along its type's status graph first (the shipped work
  items close from `review`).
  Never record test results, dates, or completion claims in documents.
"#;

/// The opinionated default `opys.toml` written by `opys init` / `opys config
/// init`. A unit test below pins that it stays valid TOML.
pub const DEFAULT_OPYS_CONFIG: &str = r##"# opys.toml — the opys document-inventory config. Lives at the project root;
# opys finds it by searching upward from the current directory.

base = "opys"                        # inventory dir (relative to this file)
pad = 4                              # zero-padding width for the numeric id part

# Interleaved ids for multi-branch work: with a modulus M each allocator owns a
# slot in 0..M (OPYS_ID_SLOT, else a stable hash of user@host) and draws only
# numbers congruent to it, so branches never collide. Absent: dense sequence.
# [ids]
# modulus = 16

# Reading the inventory out of git: which branch is truth (everything else is
# work in flight, diffed against it) and which refs count. Patterns are matched
# against fully qualified ref names, so `*` crosses `/`; the default is local
# heads only, and remote-tracking refs are opt-in because keeping them fetched
# is your job. Linked worktree heads are always read. A project that is not a
# git repository degrades to main-only.
# [git]
# main = "main"                      # falls back to "master" when absent
# branches = ["refs/heads/*"]        # add "refs/remotes/*" to include remotes

# On-disk layout. Each document's path (under `base`) is this template with
# {type} → the type's `dir`, {status} → the type's `status_dirs[status]`, and
# {id} → PREFIX-NNNN. Both segments are empty by default, so documents live flat
# at the base (e.g. opys/FEAT-0001.md). Empty segments collapse, so the order is
# free — e.g. "{status}/{type}/{id}.md" groups by status first.
[layout]
path = "{type}/{status}/{id}.md"

# How document ids are mentioned in code, for `opys show <id> --refs` and the
# warning `opys renumber` prints when a renumbered id is still referenced. Each
# format renders an id with the placeholders {id} (the full PREFIX-NNNN),
# {prefix}/{prefix_lower}, {num} (unpadded), and {padded} (zero-padded to `pad`);
# `word = false` matches a substring instead of a whole word. With no formats
# configured, only the canonical {id} is scanned.
[file_refs]
roots = ["src", "tests"]             # where to scan (project-root relative)
formats = [
    { template = "{id}" },               # FEAT-0001
    { template = "{prefix}{num}" },       # FEAT1
    { template = "{prefix_lower}_{num}" }, # feat_1
]

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

# Blocked is derived, never stored: a `blocked_by` entry pointing here stops
# blocking once this feature reaches one of these statuses. Only meaningful on a
# type that is never closed — an ephemeral one releases by construction, because
# `close` deletes it and strikes every entry pointing at it.
releases_blocks = ["implemented", "wontfix", "archived"]

# The status graph: a status → the statuses `set-status` may move it to. Omit
# the table entirely and the type keeps the any-to-any rule. Declared, it is the
# whole truth: a status that is not a key here is a dead end, and a terminal
# status may have no outgoing edges (it is reached only via `close`).
[types.feature.transitions]
planned     = ["partial", "implemented", "wontfix"]
partial     = ["planned", "implemented", "wontfix"]
implemented = ["partial", "archived"]      # back to partial when it regresses
wontfix     = ["planned"]                  # reconsidered
archived    = ["planned"]                  # un-archived

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

# A `structured` section: its content shape is an mdprism schema (the body
# portion of the DSL — see docs/structure-dsl-spec.md). This reproduces the
# classic manual-QA shape with Setup / Procedure / Expectations sub-sections;
# the format is yours to change per type.
[[types.feature.sections]]
heading = "Manual verification"
kind = "structured"
structure = '''
### @setup Setup
  - +@items
### @procedure Procedure
  1. +@steps
### @expect Expectations
  - +@checks
'''

# ----------------------------------- task ------------------------------------
# Ephemeral implementation work. Deleted on `close`.
[types.task]
prefix = "TASK"
statuses = ["todo", "in-progress", "review", "user-review", "done"]
default_status = "todo"
terminal_statuses = ["done"]         # "done" reached only via `close`
tags_required = false
requires_link = { to = "feature", min = 1 }

# There is no `blocked` status: a task is blocked because something live blocks
# it (`opys block`, then `opys list --blocked`), never because a flag says so.
[types.task.transitions]
todo        = ["in-progress"]
in-progress = ["review", "todo"]
review      = ["done", "in-progress", "user-review"]
user-review = ["done", "in-progress"]

# Who currently holds the document, for a project whose work is picked up by
# more than one agent. An ordinary declared field: opys stores the name and
# knows nothing else about agents. Taking the document is then one atomic
# command — `opys apply TASK-0007 --status in-progress --field claimed_by=…
# --log "picked up"` — and a second claimant is refused, because `apply
# --status` asserts a transition (format.md, "Composite writes").
# [types.task.fields.claimed_by]
# type = "string"

# Time-aware observations (ADR-0052). `ttl` is how long a document may sit in a
# status, `max_visits` how often it may enter one; `verify` reports both apart
# from content problems and exits 3 when they are all it found. Uncommenting
# these two lines observes only — a bare duration has no fix:
# [types.task.ttl]
# in-progress = "2d"                 # one or more <n><unit>: s/m/h/d/w/y
#
# A *fixable* entry names where `verify --fix` sends the document, so the target
# has to be a status a document can also come back from. That needs statuses
# this default config does not ship — add them first, e.g.:
#   statuses = ["todo", "in-progress", "review", "stuck", "user-review", "done"]
#   [types.task.ttl]
#   in-progress = { after = "2d", then = "stuck" }
#   [types.task.max_visits]
#   review = { limit = 3, then = "user-review" }    # the coder/reviewer loop
#   [types.task.transitions]           # replaces the map above: the fix edge,
#   todo = ["in-progress"]             # the way back and the escape must all
#   in-progress = ["review", "stuck", "user-review"]   # exist, or `config
#   stuck = ["in-progress"]            # validate` says so.  <- recovery edge
#   review = ["in-progress", "user-review", "done"]
#   user-review = ["review", "in-progress"]
# `in-progress` names `user-review` as well because a spent budget refuses the
# *entry* into `review`: the escape the refusal advertises has to be reachable
# from wherever the document stands.
# The example has no `blocked` status because there is no such thing any more:
# blocked is *derived* from a live `blocked_by` entry, so it is a relation and
# never a status a document sits at (format.md, "Blocked is derived"). It is
# not a useful `then` either — a target is a status a document can be moved to,
# and nothing can move a document into being blocked except blocking it.

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
statuses = ["todo", "in-progress", "review", "user-review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
requires_link = { to = "feature", min = 1 }

[types.bug.transitions]
todo        = ["in-progress"]
in-progress = ["review", "todo"]
review      = ["done", "in-progress", "user-review"]
user-review = ["done", "in-progress"]

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
statuses = ["todo", "in-progress", "review", "user-review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
requires_link = { to = "feature", min = 1 }

[types.chore.transitions]
todo        = ["in-progress"]
in-progress = ["review", "todo"]
review      = ["done", "in-progress", "user-review"]
user-review = ["done", "in-progress"]

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
# require_link, require_any, require_log_since_status.

[[rules]]
when = { type = "feature", status = "wontfix" }
require_field = "wontfix_reason"

[[rules]]
when = { type = "feature", status = "archived" }
require_field = "archived_reason"

[[rules]]
when = { type = "feature", status = "implemented" }
require_checked_section = "Test plan"

# A handoff needs a checkpoint: moving a work item into `review` requires a
# `## Progress` entry stamped at or after the moment its status last changed, so
# whoever picks it up reads what happened rather than guessing. Append one with
# `opys log <id> "…"`.
[[rules]]
when = { type = "task", status = "review" }
require_log_since_status = "Progress"

[[rules]]
when = { type = "bug", status = "review" }
require_log_since_status = "Progress"

[[rules]]
when = { type = "chore", status = "review" }
require_log_since_status = "Progress"

# ---------------------------------- stats ------------------------------------
# `opys stats` renders these sections. Each stat is a single SQL query over an
# in-memory relational view of the corpus; its result set is rendered as a
# markdown table headed by `name`. The tables (all rebuilt on every run,
# read-only):
#   docs(id, num, type, status, title, created, updated)
#   tags(doc_id, tag, key, value)    -- one row per tag; value NULL for a plain tag
#   sections(doc_id, heading, kind, items, unchecked)   -- countable sections only
#   fields(doc_id, key, value)       -- one row per declared field (per element for lists)
# Any SQL GlueSQL supports works (GROUP BY, subqueries, CASE, JOIN, …). Edit freely.
# Add an optional `template` to render each result row through it (`{column}`
# placeholders) instead of a table — handy for single-row summaries. Omit it for
# the default table (these three are naturally tabular, so they use tables).

[[stats]]
name = "Status by type"
sql = '''
SELECT type, status, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) /
             (SELECT COUNT(*) FROM docs d2 WHERE d2.type = d1.type)) AS pct
FROM docs d1
GROUP BY type, status
ORDER BY type, status
'''

[[stats]]
name = "Tags by key"
sql = '''
SELECT key, value, COUNT(DISTINCT doc_id) AS docs
FROM tags
WHERE value IS NOT NULL
GROUP BY key, value
ORDER BY key, value
'''

[[stats]]
name = "Section coverage"
sql = '''
SELECT heading, kind, SUM(items) AS items, SUM(unchecked) AS uncovered
FROM sections
GROUP BY heading, kind
ORDER BY heading
'''
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

    /// The default config must also pass self-validation — in particular every
    /// shipped `[[stats]]` query must compile as jq and every template must
    /// parse as an mdprism schema.
    #[test]
    fn default_opys_config_validates() {
        let cfg: crate::project_config::ProjectConfig =
            toml::from_str(super::DEFAULT_OPYS_CONFIG).expect("parses as ProjectConfig");
        let problems = cfg.validate();
        assert!(problems.is_empty(), "default config problems: {problems:?}");
    }

    /// The commented sketch under the `task` type is a recipe a user applies
    /// verbatim: it replaces the type's `statuses` line and appends the four
    /// tables below it. Uncomment it, apply it, and it must validate — a config
    /// example that `config validate` rejects is worse than no example.
    ///
    /// It must also keep clear of `blocked`, which is not a status at all:
    /// blocked is derived from a live `blocked_by` entry (format.md, "Blocked
    /// is derived"). A sketch declaring it would put a status in the graph that
    /// nothing can ever move a document into, and no config check would say
    /// so — `config validate` only sees a status like any other.
    #[test]
    fn the_commented_task_sketch_applies_and_validates() {
        // The sketch is the run of `#   `-indented lines after the "add them
        // first" line; the deeper indent is what separates it from the prose.
        let marker = "# this default config does not ship — add them first, e.g.:\n";
        let start = super::DEFAULT_OPYS_CONFIG
            .find(marker)
            .expect("the task sketch's lead-in line")
            + marker.len();
        let sketch: String = super::DEFAULT_OPYS_CONFIG[start..]
            .lines()
            .take_while(|l| l.starts_with("#   "))
            .map(|l| format!("{}\n", &l[4..]))
            .collect();
        let statuses = sketch
            .lines()
            .find(|l| l.starts_with("statuses = "))
            .expect("the sketch replaces the type's statuses line")
            .to_string();
        assert!(
            !sketch.contains("blocked"),
            "the sketch must not declare a `blocked` status — blocked is derived from a \
             live `blocked_by` entry, never a status a document sits at: {sketch}"
        );

        // Apply it: swap the `task` type's statuses line for the sketch's, drop
        // the shipped `[types.task.transitions]` table the sketch replaces, and
        // append the rest at the end of the file (the sketch's own tables are
        // fully qualified, so where they land does not matter).
        let rest: String = sketch
            .lines()
            .filter(|l| !l.starts_with("statuses = "))
            .map(|l| format!("{l}\n"))
            .collect();
        let task_at = super::DEFAULT_OPYS_CONFIG
            .find("[types.task]")
            .expect("the task type");
        // The sketch redeclares the graph, so the shipped table has to go or TOML
        // sees a duplicate key. Cut it from its header to the blank line ending it.
        let shipped_graph = super::DEFAULT_OPYS_CONFIG
            .find("[types.task.transitions]")
            .expect("the shipped task graph");
        let shipped_graph_end = shipped_graph
            + super::DEFAULT_OPYS_CONFIG[shipped_graph..]
                .find("\n\n")
                .expect("a blank line after the shipped task graph")
            + 2;
        let old_statuses_at = task_at
            + super::DEFAULT_OPYS_CONFIG[task_at..]
                .find("statuses = ")
                .expect("the task type's statuses line");
        let old_statuses_end = old_statuses_at
            + super::DEFAULT_OPYS_CONFIG[old_statuses_at..]
                .find('\n')
                .expect("a newline after it");
        assert!(
            old_statuses_end < shipped_graph,
            "the sketch splice assumes the statuses line precedes the graph"
        );
        let applied = format!(
            "{}{statuses}{}{}\n{rest}",
            &super::DEFAULT_OPYS_CONFIG[..old_statuses_at],
            &super::DEFAULT_OPYS_CONFIG[old_statuses_end..shipped_graph],
            &super::DEFAULT_OPYS_CONFIG[shipped_graph_end..],
        );

        let cfg: crate::project_config::ProjectConfig =
            toml::from_str(&applied).expect("the applied sketch parses as ProjectConfig");
        let problems = cfg.validate();
        assert!(
            problems.is_empty(),
            "the shipped task sketch does not validate: {problems:?}\n{applied}"
        );
    }
}
