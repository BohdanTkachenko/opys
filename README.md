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

It pairs with the `feature-inventory` skill (under `skills/`), which
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

See `skills/feature-inventory/references/format.md` for the normative format
specification.

## The `feature-inventory` skill

This repo ships an agent skill that drives `opys` (authoring interviews, the
implementation workflow, retrieval discipline). It lives, once, in
[`skills/feature-inventory/`](skills/feature-inventory/) and is tool-agnostic —
the same `SKILL.md` works for every assistant; only the install directory
differs. To use it in a project, copy that folder into wherever your tool looks
for skills:

| Tool | Copy it to |
|---|---|
| Claude Code | `.claude/skills/feature-inventory/` (per-project) or `~/.claude/skills/` (all projects) |
| Cursor | `.cursor/skills/feature-inventory/` |
| Google Antigravity | `.agents/skills/feature-inventory/` |

```sh
git clone --depth 1 https://github.com/BohdanTkachenko/opys /tmp/opys
cp -r /tmp/opys/skills/feature-inventory <your-project>/.claude/skills/   # or .cursor/skills/ , .agents/skills/
```

The CLI itself is universal — any agent that can run a shell command can use
`opys`. For tools that read project instructions instead of skills, the
cross-tool standard is **AGENTS.md** (this repo ships one). The substance is the
same everywhere: `opys new/set-status/verify/...` for writes, `opys`/`rg` +
`docs/features/INDEX.md` for reads.

## License

Apache-2.0
