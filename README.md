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
*not* a task board — no sprints or assignees; priority exists only as an
opt-in declared int field (`[types.X.fields.priority]`) that the web UI's
board orders and reorders by.

Need a different lifecycle — an `epic`, an `adr`, a `risk`? Add a `[types.<name>]`
block to `opys.toml` and the whole tool (create, verify, index) works for
it. Durable knowledge → features; "what I'm doing right now" → a task/bug/chore.

It pairs with the `opys` skill (under `skills/`), which
documents the format and the authoring/implementation workflows for coding
agents.

## Install

```sh
cargo install opys                 # the CLI (what agents use)
```

Or build from source:

```sh
cargo build --release -p opys        # target/release/opys
```

### Use from another flake

The flake exposes `opys` as a package, an app, and an overlay, so other flakes
can consume the CLI without going through crates.io:

```nix
{
  inputs.opys.url = "github:BohdanTkachenko/opys";

  outputs = { nixpkgs, opys, ... }:
    let
      system = "x86_64-linux";
      # Either apply the overlay and use `pkgs.opys`…
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ opys.overlays.default ];
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        # …or reference the package directly: opys.packages.${system}.default
        packages = [ pkgs.opys ];
      };
    };
}
```

Or run it straight from the flake, no install:

```sh
nix run github:BohdanTkachenko/opys -- --help
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
opys list --tag area                        # exact tag, or any tag with key `area`
opys set-status FEAT-0001 implemented       # rejected unless a test item is checked
opys verify                                 # integrity check; nonzero exit on problems
opys stats                                  # configurable [[stats]] sections (default: status/coverage/tags)
opys tags                                   # distinct tags (--keys for just keys)

# Ephemeral work, linked to a feature (default types: task/bug/chore):
opys new --type bug --title "Survive profile switch" --features FEAT-0001
opys close BUG-0002                         # deletes the file; reference struck through

# Bulk: the mutating commands take a comma-separated id list, or `-` for stdin
opys set-status FEAT-0001,FEAT-0002 wontfix --reason "superseded"
opys list --type task --status done --format ids | opys close -
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
| `retire` | delete document(s); each ID is logged and never reused |
| `block` / `unblock` | record a directional blocker between documents |
| `close` / `cleanup` | finish document(s) of a type with a terminal status; strip struck refs |
| `verify` | full integrity check — wire into CI |
| `sync` | reconcile references, linkify prose, relocate docs to their layout path (for hand edits) |
| `stats` | render configured `[[stats]]` sections (each a SQL query over the corpus, shown as a table; default: status counts, coverage, tags) |
| `query "SELECT …"` | run a SQL query over the inventory (`-` reads it from stdin; `--stdin` binds stdin to `$1` for escape-free values) and print the result table; `--write` allows INSERT/UPDATE/DELETE, applied only if the edit introduces no new `verify` problem (else nothing is written). The `blocks` table decomposes bodies into `##` sections — `UPDATE blocks SET text = …` edits a section in place |
| `agent-rules --tool <editor>` | generate a rules-based editor's instruction file from the canonical rule |
| `web <start\|add\|remove\|list\|scan\|install\|uninstall>` | the always-on node: serve the allowlisted projects over HTTP — see [below](#the-always-on-node-opys-web) |

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

## The always-on node (`opys web`)

Every `opys` command so far is one shot: load the inventory, write, exit. The
**node** is that same engine kept warm — a long-lived local process that serves
the projects you allowlisted over HTTP, with a web dashboard, a typed API and a
live event stream. It is what you open when you want to see every project at
once instead of grepping one repo at a time. It ships inside the `opys` binary:
if you installed the CLI, you already have it.

### From nothing to a dashboard

**1. Look at the allowlist.** On a machine that has never run the node it is
empty, and an empty allowlist means the node would serve nothing at all:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

nothing allowlisted — add a project with: opys web add <path>
```

**2. Allowlist a project** — any directory holding an `opys.toml`:

```
$ opys web add ~/work/notes
added /home/dan/work/notes to /home/dan/.config/opys/server.toml
a running node picks this up within a minute
```

All that did was write two lines to `~/.config/opys/server.toml`. Nothing was
started, and nothing was contacted:

```toml
[[project]]
path = "~/work/notes"
```

`opys web list` now prints the allowlist as written, and under it what those
entries resolve to right now:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

  project  ~/work/notes  -> /home/dan/work/notes

serving 1 corpus in 1 project:
  notes  /home/dan/work/notes
```

**3. Start the node.** It runs in the foreground and `Ctrl-C` stops it; make it
a background service once you like it ([below](#run-it-as-a-service)):

```
$ opys web start
opys-server: serving 1 corpus from /home/dan/.config/opys/server.toml
opys-server listening on http://127.0.0.1:6797
```

**4. Open <http://127.0.0.1:6797>.** That is the dashboard.

### Why allowlisting is a separate step

This is the part that surprises people: `opys web start` takes no project paths,
and the node finds nothing by itself. It serves exactly the entries in
`~/.config/opys/server.toml` — a file only you write. Approving a project and
running the node are deliberately two different acts, because that file is the
security boundary. Two guarantees follow from it:

- **The node serves only what you allowlisted.** `opys web add` edits that file
  and never contacts a running node; the node re-reads the file on its own and
  picks up the change within a minute, no restart. So allowlisting is something
  you do at a terminal — never something a page open in your browser can do to
  you. Discovery only ever *suggests*: `opys web scan` prints candidates and has
  no way to add one.
- **The API is typed; the node cannot execute arbitrary commands.** Every write
  the dashboard makes is a named action with named arguments — `set-status`,
  `tag`, `block`, `unblock`, `close` — run through the same engine, the same
  inventory lock and the same write-time rules as the CLI. The request body is a
  closed set: there is no shell endpoint, no "run this opys command" endpoint,
  and no endpoint anywhere that accepts a filesystem path.

### More than one project

`opys web scan` walks your home directory (ten levels, skipping hidden, build,
vendor and cache directories), lists every project it finds and marks the ones
already allowlisted. It suggests and nothing more — the command cannot add
anything:

```
$ opys web scan
scanning /home/dan (depth 10)…
  /home/dan/Projects/opys
  /home/dan/Projects/opys-feature
  /home/dan/work/notes  (allowlisted)

scan never adds anything — allowlist one with:
  opys web add /home/dan/Projects/opys
```

Add them one `opys web add` at a time, or allowlist a whole tree with
`--prefix`, which covers everything ten levels below it — including projects you
create there later, found by the node's hourly rescan:

```
$ opys web add --prefix ~/Projects
added /home/dan/Projects to /home/dan/.config/opys/server.toml
a running node picks this up within a minute
```

One entry can serve several *corpora* — a corpus is one inventory: one
`opys.toml` and the documents under it. Sibling **git worktrees come along with
the project they belong to**, so allowlisting a repo covers every worktree of
it. Here two entries serve three corpora:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

  project  ~/work/notes           -> /home/dan/work/notes
  prefix   ~/Projects (depth 10)  -> /home/dan/Projects

serving 3 corpora in 2 projects:
  opys   /home/dan/Projects/opys  main  (primary)
  opys   /home/dan/Projects/opys-feature  feature/web
  notes  /home/dan/work/notes
```

`opys web remove <path>` takes an entry back out. A project reached *through* a
prefix has no entry of its own, so instead of pretending, the CLI names the
entry that is responsible:

```
$ opys web remove ~/Projects/opys
not allowlisted directly — served by the prefix entry ~/Projects
remove that entry instead: opys web remove ~/Projects
```

Start the node again (or leave it running and wait a minute) and it serves all
three:

```
$ opys web start
opys-server: serving 3 corpora from /home/dan/.config/opys/server.toml
opys-server listening on http://127.0.0.1:6797
```

### What the dashboard shows

The sidebar lists every project and the corpora inside it — labelled by git
branch, with the primary worktree marked, and a dot per corpus for its verify
state (clean, *N* problems, or not read yet). Pick one and you get:

- **the board** — every document in that corpus, in a column per status, with
  filters for type and tag, a text filter set from the omnibox, and drag and
  drop: onto another column to change status, within a column to set priority
  (an opt-in field; see ADR-0095). The keyboard drives it too — arrows move
  between columns and cards, Enter opens, Home/End jump within a column,
  PageUp/PageDown switch projects;
- **a document** — its frontmatter and rendered body, both edited in place:
  status, tags, blockers and custom fields on the panel, the markdown body by
  clicking into it, and close behind a confirmation. Every write is a typed
  action taking the same write path as the equivalent `opys` command, so a
  write the CLI would refuse — a status change whose rule is unmet, say — is
  refused here too, with the same message. Creating documents stays a CLI job.
- **the query console** — the same SQL over the corpus that `opys query` runs,
  read-only;
- **the union view** — every worktree of one project side by side, so you can
  see where two branches disagree about a document. It shows the drift and
  nothing else: nothing here merges anything, because git is the merger.

**Ctrl+P** (⌘P on a Mac) or `/` opens the omnibox from any view: a fuzzy
finder over the corpus's tickets — or every served corpus, from the home page
— that opens a ticket on Enter or, from a board, applies the text as its
filter.

Everything updates live: the node watches each inventory and pushes events over
a WebSocket, so an edit you make in your editor — or a write from `opys` in
another terminal — shows up in the browser without a reload.

The port is **6797**, and the node binds loopback only. There is no
authentication, so the bind address *is* the boundary; while it is on loopback
the node also refuses any request whose `Host` is not loopback and any
cross-origin request, so a page you happen to be visiting cannot drive it.
Widen it — `opys web start --bind 0.0.0.0:6797`, or a `bind = "…"` line at the
top of the allowlist file — only if you mean to, and put something in front of
it that authenticates.

### Run it as a service

`opys web install` writes a systemd **user** unit and prints the two commands
that turn it on. It never runs them — enabling a service on your session is your
decision, not a side effect of an install:

```
$ opys web install
wrote /home/dan/.config/systemd/user/opys-server.service

enable it with:
  systemctl --user daemon-reload && systemctl --user enable --now opys-server

the node will listen on http://127.0.0.1:6797
```

Run those two commands and the node comes up at login and restarts if it
crashes. A **user** service lives and dies with your session, so on a machine
you are not usually logged into — a headless box you reach over SSH — also run
`loginctl enable-linger $USER`, or the node stops the moment you disconnect.

The unit is static — `ExecStart=…/opys web start --bind 127.0.0.1:6797`,
pointing at the binary you ran `install` from — so it never needs touching again
when you allowlist another project. Two things *are* fixed at install time: the
address (resolved then from `--bind`, else the allowlist file's `bind`, else the
default) and the `--config` path if you passed one. Change either afterwards and
re-run `opys web install --force`; editing `bind` in the allowlist file alone
will not move a service whose unit already names an address. Installing over an
existing unit is refused unless you pass `--force`:

```
$ opys web install
error: /home/dan/.config/systemd/user/opys-server.service already exists — pass --force to overwrite it
```

`opys web uninstall` deletes the unit and prints the disable line first, because
that is the order you have to run it in — deleting a unit file does not stop the
service it started:

```
$ opys web uninstall
stop it first — removing the unit does not stop a running service:
  systemctl --user disable --now opys-server && systemctl --user daemon-reload

removed /home/dan/.config/systemd/user/opys-server.service
```

On a machine with no systemd user manager — a Mac, a container, WSL1, a distro
that boots something else — `install` prints how to run the node by hand and
exits 0. That is a fact about the machine, not an error, and nothing is written:
a unit file no service manager will ever read is worse than no unit at all.

**On NixOS or with home-manager, do not run `opys web install`** — declare the
service instead, so it is reproducible and survives a rebuild rather than living
as an untracked file in `~/.config`. It is the same unit either way, so all you
are doing is writing it down where your configuration can see it.

First make `pkgs.opys` exist by applying this flake's overlay in your
configuration (`opys` here is this flake, taken as an input — see
[Use from another flake](#use-from-another-flake)):

```nix
nixpkgs.overlays = [ opys.overlays.default ];
```

Then, in **home-manager**, where the attributes are the unit's own sections:

```nix
systemd.user.services.opys-server = {
  Unit.Description = "opys always-on node";
  Service = {
    ExecStart = "${pkgs.opys}/bin/opys web start --bind 127.0.0.1:6797";
    Restart = "on-failure";
  };
  Install.WantedBy = [ "default.target" ];
};
```

or, in a plain **NixOS** configuration, where `systemd.user.services` is a typed
submodule rather than a freeform unit — same service, different spelling:

```nix
systemd.user.services.opys-server = {
  description = "opys always-on node";
  wantedBy = [ "default.target" ];
  serviceConfig = {
    ExecStart = "${pkgs.opys}/bin/opys web start --bind 127.0.0.1:6797";
    Restart = "on-failure";
  };
};
```

On a headless box add `users.users.<you>.linger = true;` (NixOS) for the same
reason `loginctl enable-linger` exists above.

The allowlist stays yours to edit either way: `opys web add` writes it, and the
node picks the change up without a restart. (One caveat if you hand-edit
`~/.config/opys/server.toml`: `opys web add`/`remove` rewrite the file from its
parsed form, which preserves keys and values but drops comments.)

### The `web` subcommands

| Command | Purpose |
|---|---|
| `web start [--bind ADDR] [--config PATH]` | run the node in the foreground |
| `web add <PATH> [--prefix]` | allowlist a project, or a directory to search under |
| `web remove <PATH>` | drop an entry from the allowlist |
| `web list` | the allowlist, and the corpora it currently resolves to |
| `web scan [--under PATH] [--depth N]` | suggest projects; adds nothing, ever |
| `web install [--bind ADDR] [--force]` | write the systemd user unit; print how to enable it |
| `web uninstall` | remove the unit; print how to disable it |

Every one of them except `uninstall` also takes `--config <PATH>`, to work on an
allowlist file other than `~/.config/opys/server.toml`. `install` writes that
path into the unit's `ExecStart`, so the service it installs serves the file you
named rather than the default one.

(`web scan` spells its scan root `--under` rather than `--root`, because `opys`
already has a global `--root` for the inventory root and clap propagates a
global into every subcommand. `--root` and `--no-sync` mean nothing to `web`,
which refuses them rather than ignoring them — a scan of the wrong tree looks
exactly like a scan of the right one. The same surface is also available as
`opys-server web …` — one implementation, mounted by both binaries.)

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

Apache-2.0 — everything here, including the always-on node and its web UI.
Permanently, and for every crate in the workspace.
