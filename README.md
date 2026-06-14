# opys

File-based feature inventory for human + AI codebases — one markdown file per
feature, verified in CI.

`opys` manages a version-controlled inventory of *what a product does*: one
markdown file per feature, each with YAML frontmatter (stable ID, status,
tags) and an optional body (spec prose, a test plan, manual-verification
procedures). Writes go through the CLI so invariants hold at write time and
parallel agents don't collide; reads are plain `grep` + targeted file reads.
A `verify` subcommand is the CI gate. It is deliberately *not* a task board —
no sprints, assignees, or priorities.

It also tracks **work items** — ephemeral, per-change companion files (a task
checklist, a progress log, branch/PR links) that link to the feature(s) they
change and are deleted on completion. Work items are content, not scheduling, so
the not-a-task-board stance still holds; they keep in-flight implementation
state out of the permanent feature files.

It pairs with the `opys` skill (under `skills/`), which
documents the format and the authoring/implementation workflows for coding
agents.

## Install

```sh
cargo install opys
```

Or build from source:

```sh
cargo build --release   # target/release/opys
```

The inventory lives under a base directory (default `docs/opys/`, configurable
with `--dir`/`OPYS_DIR`), so it stays out of the repo root: `docs/opys/features/`
(config + feature files + `INDEX.md`), `docs/opys/work-items/` (optional),
`docs/opys/views/`, `docs/opys/runbooks/`. Feature IDs are always `FEAT-NNNN`
and work-item IDs `WI-NNNN` — the prefixes are fixed.

## Quick start

```sh
opys init                                   # bootstrap docs/opys/features/ + _config.toml
# edit docs/opys/features/_config.toml: test_search_paths, custom [fields.*]

opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --status planned
opys set-status FEAT-0001 implemented       # rejected unless a test item is checked
opys verify                                 # integrity check; nonzero exit on problems
opys report                                 # status, coverage gaps (parity if enabled)
opys manual-runbook --out docs/opys/runbooks/release-0.3.md
opys schema --kind frontmatter              # JSON Schema for editor/CI validation

# Work items (optional): ephemeral, per-change tracking linked to a feature.
opys work-item init                         # enable the subsystem
opys work-item new --title "Survive profile switch" --features FEAT-0001
opys work-item close WI-0001                # deletes the file; reference struck through
```

Mutating commands (`new`, `set-status`, `tag`, `retire`, and the `work-item …`
mutators) reconcile cross-references, linkify prose, and regenerate
`INDEX.md`/`views/` automatically; pass `--no-sync` to skip, or run `sync-views`
after editing files by hand.

## Commands

| Command | Purpose |
|---|---|
| `init` | bootstrap `docs/features/_config.toml`, print a CLAUDE.md snippet |
| `new` | allocate the next ID and write a skeleton feature file (auto-syncs) |
| `import` | bulk-create features from a JSONL file (sequential IDs, one sync) — for migrations |
| `show` / `list` | retrieval (`--tag`, `--status`, `--format table\|ids\|paths`) |
| `set-status` | guarded transitions (wontfix needs a reason; implemented needs a checked test item) |
| `tag` | add/remove tags (`--add a,b --remove c`) |
| `retire` | delete a feature; its ID is logged and never reused |
| `verify` | full integrity check — wire into CI |
| `sync-views` | regenerate `INDEX.md` and `views/` (for hand edits) |
| `report` | status counts, coverage gaps, opt-in parity % |
| `manual-runbook` | aggregate manual items into an executable checklist |
| `schema` | emit a JSON Schema for `_config.toml` or feature frontmatter |
| `work-item <init\|new\|show\|list\|set-status\|tag\|close\|cleanup>` | manage ephemeral work items linked to features (alias `wi`) |
| `agent-rules --tool <editor>` | generate a rules-based editor's instruction file from the canonical rule |

A feature file looks like (the `references` map is auto-maintained — a work
item links back, and a closed one leaves a struck-through tombstone):

```markdown
---
id: FEAT-0421
status: implemented
tags: [osc, tabs]
references:
  WI-0042: Make tab title survive profile switch
---

# Tab title follows OSC 0/2 sequence

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title_updates`
- [ ] Invalid UTF-8 in title payload — uncovered
```

See `skills/opys/references/format.md` for the normative feature
format and `references/work-items.md` for work items.

## The `opys` skill

This repo doubles as a multi-agent plugin that drives `opys` (authoring
interviews, the implementation workflow, retrieval discipline). The skill lives,
once, in [`skills/opys/`](skills/opys/) and is
tool-agnostic; the repo also ships per-agent manifests so most tools can install
it natively. (The `opys` binary itself is a prerequisite — `cargo install opys`.)

**Native plugin/extension install:**

| Agent | Install |
|---|---|
| Claude Code | `/plugin marketplace add BohdanTkachenko/opys` then `/plugin install opys@opys` |
| Codex | `codex plugin marketplace add BohdanTkachenko/opys`, then install via `/plugins` |
| Gemini CLI | `gemini extensions install https://github.com/BohdanTkachenko/opys` |
| pi | `pi install git:github.com/BohdanTkachenko/opys` |
| opencode | add `"instructions": ["…/agent-rule.md"]` (see `opencode.json`) |

**Copy the skill folder** (conditional, fullest content) for tools that read a
skills directory:

| Tool | Copy `skills/opys/` to |
|---|---|
| Claude Code | `.claude/skills/opys/` (or `~/.claude/skills/`) |
| Cursor | `.cursor/skills/opys/` |
| Google Antigravity | `.agents/skills/opys/` |

```sh
git clone --depth 1 https://github.com/BohdanTkachenko/opys /tmp/opys
cp -r /tmp/opys/skills/opys <your-project>/.claude/skills/   # or .cursor/skills/ , .agents/skills/
```

**Always-on rule file** (a short, self-gating pointer — activates only when the
project has a `docs/opys/` inventory) for rules-based editors: `opys` *generates*
it from one canonical rule (`skills/opys/agent-rule.md`), so there
are no duplicate files to keep in sync. Run it in your project:

```sh
opys agent-rules --tool cursor     # or windsurf | cline | copilot | kiro | all
opys agent-rules --tool copilot --stdout   # print instead of writing
```

It writes the right file in the right place (`.cursor/rules/opys.mdc`,
`.windsurf/rules/…`, `.clinerules/…`, `.github/instructions/…`,
`.kiro/steering/…`) with any host-specific frontmatter.

The skill folder carries both normative specs (`references/format.md` and
`references/work-items.md`), so one folder brings everything.

The CLI itself is universal — any agent that can run a shell command can use
`opys`. For tools that read project instructions instead of skills, the
cross-tool standard is **AGENTS.md** (this repo ships one). The substance is the
same everywhere: `opys new/set-status/verify/work-item ...` for writes,
`opys`/`rg` + `docs/opys/features/INDEX.md` for reads.

## License

Apache-2.0
