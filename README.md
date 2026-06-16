# opys

File-based inventory of typed markdown documents for human + AI codebases — one
markdown file per document, verified in CI.

`opys` manages a version-controlled inventory of *what a product does*: one
markdown file per document, each with YAML frontmatter (stable ID, status,
tags) and an optional body (spec prose, a test plan, manual-verification
procedures). The document **types** — their ID prefixes, statuses, fields,
required sections, and validation rules — are configured in one
`opys.toml`. The default config ships a permanent **feature** type
(`FEAT-NNNN`) plus ephemeral **task/bug/chore** types (`TASK-`/`BUG-`/`CHORE-NNNN`)
for in-flight work, deleted on `close`. Writes go through the CLI so invariants
hold at write time and parallel agents don't collide; reads are plain `grep` +
targeted file reads. A `verify` subcommand is the CI gate. It is deliberately
*not* a task board — no sprints, assignees, or priorities.

Need a different lifecycle — an `epic`, an `adr`, a `risk`? Add a `[types.<name>]`
block to `opys.toml` and the whole tool (create, verify, index) works for
it. Durable knowledge → features; "what I'm doing right now" → a task/bug/chore.

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

`opys.toml` lives at the **project root** — opys finds it by searching upward
from the current directory (like git or Cargo). It declares a `base` directory
(default `opys/`, relative to the root) so the inventory stays out of the
repo root: the document files, flat at `opys/` by default (the path is rendered
from a configurable `[layout]` template — see the spec). A document's type is its
ID prefix.

## Quick start

```sh
opys init                                   # bootstrap opys.toml + opys/
# edit opys.toml: types, statuses, fields, sections, rules

opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --status planned
opys set-status FEAT-0001 implemented       # rejected unless a test item is checked
opys verify                                 # integrity check; nonzero exit on problems
opys stats                                  # per-type status counts + percentages

# Ephemeral work, linked to a feature (default types: task/bug/chore):
opys new --type bug --title "Survive profile switch" --features FEAT-0001
opys close BUG-0002                         # deletes the file; reference struck through
```

Mutating commands (`new`, `set-status`, `tag`, `retire`, `block`, `close`,
`cleanup`) reconcile cross-references, linkify prose, and relocate documents to
their canonical layout path (e.g. an archived doc moves into `_archived/`)
automatically; pass `--no-sync` to skip, or run `opys sync` after editing files
by hand.

## Commands

| Command | Purpose |
|---|---|
| `init` | bootstrap `opys.toml` + `opys/`, print a CLAUDE.md snippet |
| `config <init\|validate>` | generate / validate the universal `opys.toml` |
| `new --type <T>` | allocate the next ID and write a skeleton document of type `T` (auto-syncs) |
| `import --type <T>` | bulk-create documents of type `T` from a JSONL file (sequential IDs, one sync) |
| `show` / `list` | retrieval (`--type`, `--tag`, `--status`, `--format table\|ids\|paths`) |
| `set-status` | guarded transitions, enforced by the type's configured rules |
| `tag` | add/remove tags (`--add a,b --remove c`) |
| `retire` | delete a document; its ID is logged and never reused |
| `block` / `unblock` | record a directional blocker between two documents |
| `close` / `cleanup` | finish a document of a type with a terminal status; strip struck refs |
| `verify` | full integrity check — wire into CI |
| `sync` | reconcile references, linkify prose, relocate docs to their layout path (for hand edits) |
| `stats` | per-type status counts + percentages, coverage gaps |
| `agent-rules --tool <editor>` | generate a rules-based editor's instruction file from the canonical rule |

A feature file looks like (the `references` map is auto-maintained — a work
item links back, and a closed one leaves a struck-through tombstone):

```markdown
---
id: FEAT-0421
status: implemented
tags: [osc, tabs]
references:
  TASK-0042: Make tab title survive profile switch
---

# Tab title follows OSC 0/2 sequence

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title_updates`
- [ ] Invalid UTF-8 in title payload — uncovered
```

See `skills/opys/references/format.md` for the normative document format and the
`opys.toml` config reference.

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
project has a `opys/` inventory) for rules-based editors: `opys` *generates*
it from one canonical rule (`skills/opys/agent-rule.md`), so there
are no duplicate files to keep in sync. Run it in your project:

```sh
opys agent-rules --tool cursor     # or windsurf | cline | copilot | kiro | all
opys agent-rules --tool copilot --stdout   # print instead of writing
```

It writes the right file in the right place (`.cursor/rules/opys.mdc`,
`.windsurf/rules/…`, `.clinerules/…`, `.github/instructions/…`,
`.kiro/steering/…`) with any host-specific frontmatter.

The skill folder carries the normative spec (`references/format.md`), so one
folder brings everything.

The CLI itself is universal — any agent that can run a shell command can use
`opys`. For tools that read project instructions instead of skills, the
cross-tool standard is **AGENTS.md** (this repo ships one). The substance is the
same everywhere: `opys new --type/set-status/close/verify ...` for writes,
`opys list`/`rg` for reads.

## License

Apache-2.0
