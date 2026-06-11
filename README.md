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

It pairs with the `feature-inventory` skill (under `.claude/skills/`), which
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

The inventory lives under a base directory (default `docs/`, configurable with
`--dir`/`OPYS_DIR`), so it stays out of the repo root: `docs/features/`
(config + feature files + `INDEX.md`), `docs/views/`, `docs/runbooks/`.

## Quick start

```sh
opys init                                   # bootstrap docs/features/ + _config.toml
# edit docs/features/_config.toml: prefix, test_search_paths, custom [fields.*]

opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --status planned
opys set-status VIK-0001 implemented        # rejected unless a test item is checked
opys verify                                 # integrity check; nonzero exit on problems
opys report                                 # status, coverage gaps (parity if enabled)
opys manual-runbook --out docs/runbooks/release-0.3.md
opys schema --kind frontmatter              # JSON Schema for editor/CI validation
```

Mutating commands (`new`, `set-status`, `tag`, `retire`) regenerate
`INDEX.md`/`views/` automatically; pass `--no-sync` to skip, or run `sync-views`
after editing files by hand.

## Commands

| Command | Purpose |
|---|---|
| `init` | bootstrap `docs/features/_config.toml`, print a CLAUDE.md snippet |
| `new` | allocate the next ID and write a skeleton feature file (auto-syncs) |
| `show` / `list` | retrieval (`--tag`, `--status`, `--format table\|ids\|paths`) |
| `set-status` | guarded transitions (wontfix needs a reason; implemented needs a checked test item) |
| `tag` | add/remove tags (`--add a,b --remove c`) |
| `retire` | delete a feature; its ID is logged and never reused |
| `verify` | full integrity check — wire into CI |
| `sync-views` | regenerate `INDEX.md` and `views/` (for hand edits) |
| `report` | status counts, coverage gaps, opt-in parity % |
| `manual-runbook` | aggregate manual items into an executable checklist |
| `schema` | emit a JSON Schema for `_config.toml` or feature frontmatter |

A feature file looks like:

```markdown
---
id: VIK-0421
status: implemented
tags: [osc, tabs]
---

# Tab title follows OSC 0/2 sequence

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title_updates`
- [ ] Invalid UTF-8 in title payload — uncovered
```

See `.claude/skills/feature-inventory/references/format.md` for the normative
format specification.

## The `feature-inventory` skill

This repo ships a Claude Code skill that drives `opys` (authoring interviews,
the implementation workflow, retrieval discipline). This repo also doubles as a
single-plugin marketplace, so installing it is a one-liner:

```text
/plugin marketplace add BohdanTkachenko/opys
/plugin install feature-inventory@opys
```

Then invoke it with `/feature-inventory`. Alternatively, drop the skill into any
project (or `~/.claude/skills/` for all projects) by copying the directory:

```sh
git clone https://github.com/BohdanTkachenko/opys /tmp/opys \
  && cp -r /tmp/opys/.claude/skills/feature-inventory ~/.claude/skills/
```

## Other agent tools

The `opys` CLI is plain and universal — any agent or editor that can run a shell
command can use it; nothing is Claude-specific. Only the *skill wrapper* is
Claude Code's format, so this repo ships the same guidance in two other tools'
native formats. Copy the file into your own project to use it there:

- **Cursor** — [`.cursor/rules/feature-inventory.mdc`](.cursor/rules/feature-inventory.mdc).
  Drop it in your repo's `.cursor/rules/`; it activates contextually (and on
  `docs/features/**`). Cursor 2.5+ also has a plugin marketplace (`/add-plugin`).
- **Google Antigravity** — [`.agents/skills/feature-inventory.md`](.agents/skills/feature-inventory.md).
  Place it in your repo's `.agents/skills/`; Antigravity auto-registers it as a
  slash command.

For any other tool, the cross-tool standard is **AGENTS.md** (this repo ships
one). The substance is identical everywhere: `opys new/set-status/verify/...`
for writes, `opys`/`rg` + `docs/features/INDEX.md` for reads, per
`references/format.md`.

## License

Apache-2.0
