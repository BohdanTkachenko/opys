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

## Quick start

```sh
opys init                                   # bootstrap features/ + _config.toml
# edit features/_config.toml: prefix, test_search_paths, custom [fields.*]

opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --status planned
opys set-status VIK-0001 implemented        # rejected unless a test item is checked
opys verify                                 # integrity check; nonzero exit on problems
opys sync-views                             # regenerate INDEX.md + views/
opys report                                 # parity % and coverage gaps
opys manual-runbook --out runbooks/release-0.3.md
```

## Commands

| Command | Purpose |
|---|---|
| `init` | bootstrap `features/_config.toml`, print a CLAUDE.md snippet |
| `new` | allocate the next ID and write a skeleton feature file |
| `show` / `list` | retrieval (`--tag`, `--status`, `--format table\|ids\|paths`) |
| `set-status` | guarded transitions (wontfix needs a reason; implemented needs a checked test item) |
| `tag` | add/remove tags (`--add a,b --remove c`) |
| `retire` | delete a feature; its ID is logged and never reused |
| `verify` | full integrity check — wire into CI |
| `sync-views` | regenerate `features/INDEX.md` and `views/` |
| `report` | counts, parity %, coverage gaps |
| `manual-runbook` | aggregate manual items into an executable checklist |

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

## License

Apache-2.0
