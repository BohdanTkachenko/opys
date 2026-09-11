# Agent Instructions

This file provides guidance to AI coding assistants (Claude Code and any other AGENTS.md-aware tool) when working with code in this repository.

## What this is

`opys` is a Rust CLI that manages a **file-based inventory of typed markdown
documents**: one markdown file per document, each with `---`-fenced YAML
frontmatter (a stable `PREFIX-NNNN` id, status, tags, relation maps) plus a
markdown body. All writes go through the CLI so invariants hold at write time and
parallel agents don't collide; reads are plain `grep` + targeted file reads.
`verify` is the CI gate. The inventory base dir defaults to `opys/`. It is
deliberately *not* a task board — no sprints or assignees; priority exists
only as an opt-in declared field the board can order by (ADR-0095).

Everything is driven by **one config, `opys.toml`** (parsed into `ProjectConfig`,
`src/project_config.rs`), which lives at the **project root** — `Project::open`
finds it by searching upward from the cwd (`find_root`), and it declares the
inventory `base` (default `opys/`, relative to the root). The config
declares document **types**, each with an id `prefix`, an optional `dir` and
per-status `status_dirs` (the `{type}`/`{status}` segments of the configurable
`[layout]` path template — both empty by default, so docs live flat at `base`),
its own `statuses`
(plus `default_status` / `terminal_statuses`), `[fields.*]` (custom frontmatter
fields, with optional regex `pattern`), and required `sections` (each a
code-backed *kind*: prose/log/checklist/structured — the `structured` kind's
content shape is config-driven via a `structure` (an `mdprism` schema, the
inlined `mdprism` module, `src/mdprism/`) — with optional config-driven
`checks`), plus a list of
conditional `[[rules]]` (`when {type?, status?}` + one assertion). **A document's
type is its id prefix.** There is no hardcoded type set: the default config ships
a permanent `feature` type plus ephemeral `task`/`bug`/`chore` types (deleted on
`close`), but a project can add `epic`, `adr`, `risk`, … and the whole tool
(create, verify, index) works for them. The engine that runs the rules is
`src/rules.rs` (`rules::evaluate`).

The tool ships alongside the tool-agnostic `opys` skill in `skills/opys/`. The
normative spec lives at `skills/opys/references/format.md` — consult it before
changing parsing, serialization, or `verify` semantics, and keep code ↔
format.md in sync. The README explains how users copy that one folder into their
tool's skills directory (`.claude/skills/`, `.cursor/skills/`, `.agents/skills/`).

## Development Environment

This project uses a Nix flake with a devShell (`flake.nix`) and direnv
(`.envrc`), which provide the Rust toolchain (`cargo`, `rustc`, `clippy`,
`rustfmt`, `rust-analyzer`).

To add a new tool, add it to `devPackages` in the devShell in `flake.nix` and
run `refresh`. Do not use `nix run` or `nix shell` for project tooling — keep
everything in the devShell. Use `nix run` only for one-off commands that don't
belong in the devShell permanently.

## Build / test / lint

The CI that gates merges (`.github/workflows/ci.yml`) runs exactly:

```sh
ui-build                                    # opys-server/ui/dist is generated, not committed
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # warnings are errors
cargo test --all
cargo clippy --workspace --all-targets --all-features -- -D warnings   # + the `history` alias
cargo test --all --all-features
cargo package --list -p opys-server --allow-dirty       # the bundle is in the tarball
cargo build --workspace --all-targets       # also built on MSRV 1.88 — don't use newer std APIs
```

plus a second job for the no-Node path, which must keep passing on its own:

```sh
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --all --no-default-features
```

**`ui-build` comes first, and a fresh checkout will not compile without it**
(ADR-0086): `opys-server/ui/dist` is gitignored, and `build.rs` embeds it. Two
ways out — build the bundle, or drop the `web-ui` feature with
`--no-default-features`, which is the supported path for working on the Rust
without a Node toolchain. Never make the missing-bundle case degrade silently to
a UI-less binary; `build.rs` fails on purpose.

The MSRV (`rust-version` in `Cargo.toml`) is set by the dependency tree's floor,
not just our own code — reproduce the CI floor build locally with **`msrv`** (a
devShell command that runs `cargo build --workspace --all-targets` under a pinned Rust
toolchain via `rust-overlay`), so a dependency raising the minimum is caught here
rather than on the first push.

Run a single test:

```sh
cargo test --test cli new_allocates_next_id_and_requires_tags   # one integration test
cargo test --lib frontmatter::                                  # unit tests in a module
```

## Architecture

The repo is a Cargo **workspace** of four crates: **`opys-engine`** (the core
library, at the repo root — the model, config, rules, SQL store, and command
implementations, plus the [`Backend`] storage trait; lib name `opys_engine`);
**`opys-backend-markdown-local`** (the default `Backend` impl: one markdown file
per document on the local filesystem — it owns all corpus filesystem I/O,
walking and parsing documents on load, executing the store's `FlushPlan` on
flush, and holding the **exclusive inventory lock** from load through flush, so
parallel invocations serialize instead of colliding — a flock on a per-inventory
file under `$XDG_RUNTIME_DIR` (or the OS temp dir), named by the canonicalized
base path + hash, so nothing untracked ever appears in the user's repo;
contention retries until `OPYS_LOCK_TIMEOUT_MS`, default 10 s, and the OS
releases the flock with the process, so stale locks cannot exist); and
**`opys`** (the binary — this is what `cargo install opys` yields); and
**`opys-server`** (the always-on node of FEAT-0058: watcher, HTTP/WS API, and
embedded web UI over the allowlisted inventories, ADR-0077). The binary
(`opys/src/main.rs`) owns the top-level parser — `opys_engine::cli::Command`
flattened in with `#[command(flatten)]`, plus a `web` variant carrying
`opys_server::cli::WebCommand` — and dispatches each half to its crate: engine
commands rebuild `opys_engine::cli::Cli` and call `opys_engine::run` (injecting
the `MarkdownLocal` backend), `web` calls `opys_server::cli::dispatch`. Both map
the exit code the same way. **`opys-engine` must never depend on
`opys-server`**: the engine is the library every consumer embeds, and it does not
pull in axum or tokio — joining the two surfaces is the binary's job. A
`#[cfg(test)] mod tests` in `opys/src/main.rs` compares the duplicated root
metadata (name, about, `--root`, `--no-sync`, the subcommand list) against
`opys_engine::cli::Cli::command()`, because nothing else in the build would
notice it drifting.
(The former `opys-tui` terminal board was retired per ADR-0050 — the web UI over
the always-on node replaces it; the crate lives on in git history.) All four
crates publish to crates.io together, in dependency order. Commands never touch
the storage medium directly — they load/flush through the injected
`Box<dyn Backend>` on `Ctx`, so the medium is swappable.

**License (ADR-0080):** the whole workspace is Apache-2.0 — the CLI, the engine,
the storage backends, and the node with its embedded web UI. Permanently, and
for every crate here.

**Exit-code contract (important):** `verify` returns `1` when it finds content
problems and `3` when all it found are *observations* (a document past its
type's `ttl`, or at its `max_visits` limit — ADR-0101: not content problems, and
never mixed into the problem list); every other command returns `0` on success.
Real failures (bad flags, IO, missing config) surface as `OpysError` and the
binary maps them to exit `2` — the binary passes any other code through
unchanged, so `3` needed no mapping. Crucially, **content problems found by
`verify` are not `OpysError`s** — they are collected into a `Vec<String>` and
printed, so verify can report *all* problems at once rather than aborting on the
first. `verify` itself never writes, in any mode.

Layering, roughly outermost-in:

- `src/cli.rs` + `src/lib.rs` — `Cli`/`Command` enums, the dispatch `match`,
  and the `Ctx` struct (root dir, inventory `dir`, `no_sync` flag).
- `src/commands/` — one module per subcommand, each a `run(ctx, …)` fn.
  `commands/mod.rs` holds shared helpers: `today()`, the clock family
  (`now_rfc3339`/`now_utc_micros`/`now_utc_seconds` — the last is the log-entry
  stamp), `split_csv()`, and `maybe_sync()` (the auto-sync hook: reconcile +
  linkify + relocate).
- `src/project_config.rs` — `ProjectConfig` (the parsed `opys.toml`): the `types`
  map of `DocType` (prefix, dir, status_dirs, statuses, fields, sections, the
  `requires_link` shorthand), the `[layout]` path template, and the `[[rules]]`
  list, plus `type_name_for_id`, `doc_relpath` (renders a doc's canonical path),
  and config self-validation (`validate`). The sole config.
- `src/rules.rs` — `rules::evaluate(prj, type, status, fm, body, doc_ids)`: runs
  the applicable `[[rules]]` (plus the type-level `requires_link` shorthand) and
  returns one problem per failed assertion. Called at every write point and by
  `verify`. `require_log_since_status` is the one assertion that reads the
  evaluated *status*: `since_instant` resolves it to the last `history` entry
  whose status differs, which is what makes the write-time answer (before the
  new entry is appended) and the verify-time answer agree.
- `src/store/` — **the internal working representation.** Per CLI invocation the
  corpus is loaded and decomposed into an in-memory GlueSQL database
  (`Store::open`); commands run SQL over it (plus reconstructed `Doc`s for the
  Rust-side invariants — rules, linkify, mdprism); `flush` writes changed docs
  back (create/rename/relocate/delete + retired-ledger rewrite). Markdown files
  remain the durable storage. Authoritative tables `docs`/`tags`/`relations`/
  `history`/`fm_fields`/`retired` (every frontmatter key has exactly one home; arbitrary
  YAML round-trips through `fm_fields.value_yaml` — the invariant is
  `reconstruct(decompose(doc)).to_text() == doc.to_text()`); derived
  `fields`/`sections`/`blocks` (the `[[stats]]`/`opys query` contract) are
  rebuilt by `refresh_projections`; `blocks(doc_id, seq, heading, text)` decomposes
  each body into its `##` sections, for querying into prose. All three
  projections are read-only. ID allocation goes through the `ids::IdSource`
  seam, resolved by `Ctx::id_source` / `ids::resolve` from the config and never
  named by a command — `SequenceMax` (one SQL `MAX` across docs/relations/
  retired) by default, or under `[ids] modulus = M` an `Interleaved` slot
  (`OPYS_ID_SLOT`, else FNV-1a of `user@host` mod `M`) that draws only
  congruent numbers, so allocators on different branches cannot collide
  (ADR-0103). Callers that build several documents before storing any
  (`import`, `renumber`) take one `block`, which steps by the source's stride. Internal SQL uses `$n` parameters and JOINs only — never
  `IN (subquery)`/FROM-subqueries/`UNION` (GlueSQL executes/​rejects those
  badly). `commands/query.rs` exposes user SQL, read-only: every statement is
  plan-guarded to SELECT and the command never flushes (ADR-0100 — writes are
  lifecycle commands, and a bulk edit pipes `list --format ids` into one).
  The "no worse than before" verify gate has one owner, `commands/edit.rs`
  (the web UI's edit-body/set-field/remove-field actions).
- `src/project.rs` — `Project` ties the on-disk layout to `pcfg`. `Project::open`
  requires `<root>/opys.toml`. Owns the canonical-path helper (`doc_path`), the
  retired-ledger reader (`retired_ids`), and — for the still-file-based
  `history` path — `find` over an already-loaded doc set. Id allocation is not
  here: every allocator is `Ctx::id_source` → `ids::resolve`, reserving against
  the store. Document discovery/parsing lives in the backend crate
  (`load_docs`). Mutating commands go through the store.
- `src/doc.rs` / `src/frontmatter.rs` / `src/body.rs` — the parse layer. `Doc` is
  the single document struct (`{path, frontmatter, body, title}`; type derived
  from the id prefix). `frontmatter` parses YAML with `serde_norway` and
  re-serializes canonically; `body` extracts the title, sections
  (`section`/`sections`/`section_spans`), checkbox items (`checklist_items`) and
  log entries (`log_entries`), and `apply_section_edits` splices new content
  into a section byte-accurately — `commands/log.rs` is its one owner
  (FEAT-0105/ADR-0100).
- `src/refs.rs` — the uniform relation maps (`references`/`blocked_by`/`blocks`),
  ID→title: parse/serialize (sorted by item number), strikethrough tombstone
  helpers, `id_number`. Also the derived-blocked predicate — `Corpus` (config +
  an id→status index, built once per command), `live_blockers` and `is_blocked`
  — which `list --blocked` and the node both read rather than reimplement, plus
  `live_blocks`, the same question from the blocker's side (not struck, target
  resolves, and the document itself has not released) that ordering counts.
- `src/history.rs` — the `history` lifecycle map (UTC RFC3339 with six
  fractional digits → status): key formatting, `is_canonical_key` (the exact
  spelling `verify` demands), the same-instant `+1µs` bump, and the
  `seed`/`append`/`record` writers. Distinct from `src/git/`, which reconstructs
  a timeline *from git*, and from `commands::history`, the command over it.
- `src/git/` — the in-process git reader (ADR-0102, feature `git`, on by
  default): `open` (a non-repository is `None`, never an error), `truth_branch`
  (`[git] main`, falling back to `master`), `refs` (branch tips selected by the
  `[git] branches` globs plus every linked worktree head from
  `<git-dir>/worktrees/*/HEAD`, deduplicated by commit, labelled by branch else
  by checkout directory), `docs_changed_vs_main` (a tree diff under the base,
  keyed by the id-named **basename** so a relocation is not a delete plus an add,
  decoding only the blobs that differ), `occupancy` (which refs hold a document
  at a status main's on-disk copy does not — the caller supplies main's loaded
  docs, because corpus I/O is the backend's job), and `timeline` (what `opys
  history <id>` prints and `--seed` writes). `fixture` builds test repositories
  through gix's write API, so the reader's own tests and the binary's `history`
  tests never spawn `git` — pinned by
  `git::tests::nothing_in_the_reader_spawns_a_process`. The repository is not
  free of subprocesses: `opys/tests/cli.rs` keeps a `git` helper for
  `renumber_keeps_a_relocated_base_document` (because `opys renumber` shells out
  to `git merge-base` itself), and `opys-server`'s worktree tests and
  `discover.rs` spawn it too — which is why `flake.nix` still lists `pkgs.git`
  in `nativeCheckInputs`. `fixture` is `#[doc(hidden)]` and
  gated `any(test, feature = "test-fixtures")`, so it compiles only into the
  engine's own tests and into whoever asks for that non-default feature (only
  `opys`'s *dev*-dependency does, for `opys/tests/cli.rs`) — never into the
  published crate's API. The feature does not imply `git`, so that edge cannot
  smuggle gix into a `--no-default-features` build. The data types and a
  main-only fallback are compiled unconditionally, so a `--no-default-features`
  build answers "empty, plus one notice" rather than not compiling. The
  `history` **command** is the one thing that does not degrade: it is parsed in
  every build, but a binary without the feature answers it with
  `git::NO_GIT_FEATURE_COMMAND` (exit 2) instead of an empty timeline it never
  looked for — and so does a project that is no git repository. Only the reader
  promises "main-only and nothing fails".
- `src/lifecycle.rs` — the status graph: `allowed_targets`/`allows`/`can_close`
  over a type's `[transitions]` map, plus the one wording of the "…may move to"
  phrase. The single reader of that map, so `set-status`, `close`, `verify` and
  the node's `DocView` cannot disagree about declared order, duplicates, or what
  an absent map means (any-to-any) versus an absent *key* (a dead end).
- `src/order.rs` — the ordering chain (ADR-0099 decision 4): the `OrderKey`
  enum, `chain` (a type's `order_by`, or the default `["priority", "blocks",
  "id"]` with the priority key *dropped* on a type declaring no `int priority`,
  since priority is opt-in per ADR-0095), `effective_priority` (the smallest
  number over a document and everything it transitively still blocks, walked
  with a visited set so a cycle terminates) and `compare`. The single reader of
  `order_by`, so `list --sort`, `next` and the flow view cannot order the same
  corpus three ways. Unprioritised sorts **last** — `Option`'s own `Ord` is the
  other way round, so every optional key goes through one helper here. The web
  UI board is the deliberate exception: it orders a column by the raw
  `priority` field, because that is the number drag-to-reorder writes.
- `src/observe.rs` — the time-aware observations (ADR-0101): `override_str()`
  (the one reader of the `OPYS_NOW` override) and `now()` (the override parsed
  as an instant, which `commands::now_utc_micros`/`today` go through — while
  `now_rfc3339` returns `override_str()` verbatim, so an offset survives),
  `parse_duration`, the `stuck`/`overflow` predicates over a type's
  `[ttl]`/`[max_visits]` and `observations` (the one function `verify`, `next`
  and the node call, so the three cannot disagree). Read-only throughout: an
  observation names the target the type's config declares and nothing here
  performs the move (ADR-0101, "Pending move to kobzar" — acting on staleness
  needs to know whether the agent holding the document is still alive, which
  only a dispatcher knows).
- `src/links.rs` — the auto-sync engine: `reconcile`/`reconcile_blockers`
  (bidirectional, title-fresh relation maps between live docs) and `linkify`
  (bare `PREFIX-NNNN` mentions in prose → markdown links, skipping code; the
  prefix regex is built by `ref_re` from the live type prefixes). Driven by
  `commands/sync.rs`, which `maybe_sync` calls.
- `src/config.rs` — just the shared `FieldSpec` / `FieldType` / `TestRefCheck`
  the engine config reuses.
- `src/file_refs.rs` — scan source files for *textual* mentions of an id
  (`FEAT-0001`, `feat_1`, …) per the `[file_refs]` config (`render` a format
  template for an id, search under `roots`). Powers `show --refs` and the
  post-`renumber` reference warning (with `sed_fix` suggestions). Distinct from
  `refs.rs` (relation maps between documents).

### Invariants enforced on disk (the point of the tool)

All status/section/link guards are *config*, enforced by one engine
(`rules::evaluate`) at every write point and re-checked by `verify`.

- **IDs**: each type has a `prefix` (validated `^[A-Z][A-Z0-9]*$`, unique across
  types); ids are drawn from a *single global, monotonically increasing
  sequence* — never reused, never duplicated across prefixes. The store's
  `max_doc_num` (one SQL `MAX` each over `docs`, `relations`, `retired`) takes
  the max over every live doc, the retired ledger (`<base>/_retired.md`), *and*
  every relation map (struck or not), so a closed doc's tombstone still
  reserves its number; the one allocator, the `IdSource` handed out by
  `Ctx::id_source` / `ids::resolve`, allocates above it — `SequenceMax` one
  past, `Interleaved` the smallest number in its slot past.
  `retire` appends to the (sorted) ledger; `verify` rejects reuse *and* any two
  live docs sharing a number (`check_unique_numbers`). With `[ids] modulus = M`
  set, an allocator takes only numbers congruent to its slot (ADR-0103) — the
  same invariants, with gaps of `M - 1`; with no modulus, allocation is dense
  and `OPYS_ID_SLOT` is not read.
- **References** (`references` map): auto-reconciled on every write
  (`links::reconcile`) — bidirectional between live docs, titles refreshed, sorted
  by number. A closed doc leaves a struck-through (`~~title~~`) tombstone.
  `verify` fails on a non-struck id that resolves to nothing, or a type whose
  `requires_link` is unmet; drift / missing reverse links are auto-fixed, not
  gated. Bare ID mentions in body prose are linkified (`links::linkify`),
  skipping code spans/fences.
- **Blockers** (`blocked_by` / `blocks` maps): a directional relation on the same
  ID→title machinery. `opys block <id> --by <id>` / `unblock` write `blocked_by`
  on the blocked side and the inverse `blocks` on the blocker — and nothing else:
  no status, no history entry. **Blocked is derived, never stored** (ADR-0099
  decision 3): a document is blocked iff it holds a live `blocked_by` entry — not
  struck, resolving to a document that exists, and that document's status not in
  its own type's `releases_blocks`. A permanent type declares that set (a feature
  releases at `implemented`/`wontfix`/`archived`); an ephemeral one releases by
  construction, because `close` deletes it and strikes the entry. The one
  predicate is `refs::live_blockers` / `refs::is_blocked` over a `refs::Corpus`
  (config + id→status), read by `list --blocked`/`--unblocked`, the board badge
  and the document view alike. There is no `blocked` status, `blocked_from` or
  `blocked_reason`; everything that blocks is a document.
  `refs::RELATION_FIELDS` drives close/cleanup/verify/id-reservation uniformly.
- **Ordering** (`order_by`, ADR-0099 decision 4): a type's chain of tiebreaks —
  `priority` (effective, ascending, unprioritised last), `blocks` (live count,
  descending), `id` (ascending), `updated` (newest first) — defaulting to
  `["priority", "blocks", "id"]`. Effective priority is transitive over live
  `blocks` entries, so a task at 30 holding up one at 1 sorts as a 1. An
  unknown key is a parse error (the set is closed); an *explicit* chain naming
  `priority` on a type with no `int priority` field is a `validate` problem,
  while the implicit default just drops the key. `list --sort` applies it,
  grouped by type.
- **Lifecycle history**: every status change is appended to the reserved
  `history` map (`new` seeds it from `created`, `set-status`/`import` write it;
  `block`/`unblock` change no status so they write nothing; nothing hand-edits
  it). Keys are UTC RFC3339 with *exactly*
  six fractional digits — `verify` rejects any other width, because only fixed
  width makes lexical order chronological, and the serializer sorts the block —
  and a colliding key is bumped one microsecond. A status change on a document
  that predates the map seeds its first entry from `created` at the status being
  left, so the write lands second and never contradicts `created`. `verify`
  checks presence, the last entry against `status`, strictly increasing
  canonical keys, `created` against the first key, and every consecutive pair
  against the type's status graph — and never repairs, since history cannot be
  rebuilt from ground truth. Two silences in the pair check are deliberate: an
  equal pair is skipped (a same-status `set-status` is a legal no-op that
  appends), and the first entry is never checked against `default_status`
  (`import` may seed anywhere and leaves no marker).
- **Status lifecycle**: each type declares its own `statuses`, `default_status`,
  `terminal_statuses` and, optionally, `transitions` — a status → the statuses it
  may move to (ADR-0099). Without it the type keeps any-to-any, byte-identically;
  with it `set-status` refuses an undeclared edge (exit 2, naming the legal
  targets), `close` refuses unless the current status has an edge into a terminal
  one, and `verify` checks every consecutive `history` pair. A status absent from
  a non-empty map is a dead end; a terminal status may have no outgoing edges. The
  map is read only through `lifecycle::allowed_targets`, so the four call sites
  cannot drift. A terminal status is reached only via `close` (`set-status`
  rejects it), and `new` has no `--status` at all — a document starts at its
  type's initial status and `import` is the one seam that seeds elsewhere. The
  conditional guards (e.g. feature `wontfix`⇒`wontfix_reason`,
  `archived`⇒`archived_reason`, `implemented`⇒a checked `## Test plan` item) are
  `[[rules]]`, enforced at write time and by `verify`. "Removed from the product"
  is just a status (e.g. `archived`), never a deletion.
- **Time-aware observations** (ADR-0101): a type may also declare `[ttl]` (a
  status → how long a document may hold it) and `[max_visits]` (a status →
  `{ limit, then }`). A document past its `ttl` is *stuck*; one whose history
  records `limit` entries at its current status is *overflowed*. The two trip at
  the same count but not in the same place: the observation counts the status the
  document *holds* (the one the configured `then` moves it out of), while the
  guard counts the status being *entered*, so `set-status` refuses the move from
  anywhere, naming `then`. Neither is a content problem: `verify` prints them in
  their own block and exits `3` when they are all it found. `config validate`
  requires each `then` to be a declared, non-terminal status and — where the
  type declares transitions — the edge to it, the edge back (the recovery edge a
  move under a running agent relies on) and, for `max_visits`, the *escalation
  edge* to `then` from every status that may enter the budgeted one, so the
  refusal never advertises an illegal move.

  **opys observes; it does not act.** A `then` is recorded *intent* — where the
  project wants a stale document to go — and nothing in opys performs that move
  (ADR-0101's "Pending move to kobzar": deciding a document is really abandoned
  needs to know whether the agent holding it is still alive, which only a
  dispatcher knows, and ADR-0051 keeps files and git the only truth). The
  observation line ends `— configured target: <status>` (or `— no configured
  target` for a bare-duration `ttl`), and `verify` writes nothing. The config
  checks above still earn their keep: they catch a `then` nothing could *ever*
  move the document to, when the config is written rather than on the day the
  threshold trips. The visit budget is a different thing and stays enforced —
  `set-status` refusing an entry is a refusal counting visits, not a write off
  the clock.
- **Section checks** (`[[types.X.sections.checks]]`): a universal, config-driven
  validation attachable to any section. A `pattern` regex parses each line into
  named capture groups; an optional `file` group (a path that must exist under
  `roots`) and/or `must_match` regex (built from `${group}` substitutions, matched
  in that file or the corpus) assert the parsed reference resolves. `scope`
  (`all`/`checked`) selects the lines. The default config reproduces the old
  test-plan grep with one such check on the `Test plan` `checklist`; the engine is
  `run_check` in `commands/verify.rs`. (There is no longer a `test-plan` kind or a
  `[tests]` block.)
- **Sections**: a type's `sections` each declare a `kind` (prose/log/checklist/
  structured), optional `checks`, and `required`. `verify` checks a required
  section is present, runs each section's `checks`, and validates a `structured`
  section's content against its `structure` (an `mdprism` schema) via
  `mdprism::Schema::validate`; `new` scaffolds the required ones (a `structured`
  section from `mdprism::Schema::scaffold`).
- **Log sections** (`kind = "log"`): a dated timeline, written only by
  `opys log` (`commands/log.rs`), which stamps each entry `- <RFC3339 UTC
  seconds> text`. `verify` reports an undated bullet and a stamp that goes
  *backwards* — non-strict, so two entries may share a second. The
  `require_log_since_status` rule makes an entry a precondition of a transition;
  the shipped config puts it on the move into `review` for task/bug/chore.
- **Frontmatter is closed**: only the reserved keys (`id`/`status`/`tags`/
  `history` + `references`/`blocked_by`/`blocks`) plus the doc type's declared `[fields.*]`
  are allowed; unknown keys fail `verify`. Declared fields are type-checked
  (`check_custom_fields`); a `type = "enum"` field constrains the value to its
  `values`, a `pattern` constrains a string, and `list --field key=value` filters
  on any of them.

### Auto-sync — no generated artifacts

opys generates no view files (no `INDEX.md`); slice the inventory live with
`opys list` or `opys query`. Mutating commands (`new`, `set-status`, `tag`,
`log`, `retire`, `block`, `close`, `cleanup`) call `maybe_sync` →
`commands/sync::run` automatically unless `--no-sync` is passed. That pass
reconciles relations, linkifies prose, and **relocates each document to its
canonical layout path** (the store's `flush` renames a file when a status change
or `[layout]` edit moved it, e.g. into `_archived/`); it refuses to run if any
document fails to parse (run `verify` first). `opys sync` runs the same pass
after hand edits (and deletes any stale `INDEX.md` left by older versions).

### Frontmatter serialization

`frontmatter::serialize` emits canonical output: the five core fields of
`ORDER` (`id`, `status`, `tags`, `created`, `updated`) first, in that order, then
the remaining keys alphabetically; flat scalars and scalar lists inline
(`tags: [a, b]`), complex values as block YAML. `format_string` quotes
only when needed for unambiguous round-tripping. The unit tests in
`frontmatter.rs` pin this exact output — update them deliberately when changing
formatting.

**`history` is the one exception** — it takes a dedicated `render_history` path
instead of the block-map one, because `serde_norway`'s block emitter writes the
timestamp keys *unquoted* and a bare RFC3339 scalar is resolved as a datetime by
a YAML 1.1 reader. That path always quotes the key, sorts entries by *parsed
instant* rather than by spelling, and escapes control characters itself
(`format_string` quotes but does not escape — its callers are guaranteed
control-char-free by `inline_scalar`, and a raw newline inside a quoted scalar
would make the document unparseable). It takes every non-empty mapping, not just
the canonical ones, so opys never emits the unquoted form — a key that is not a
string costs only itself its quoting; only a *composite* key or value falls
through to the generic emitter. The frontmatter unit tests pin that output too.

Quoting is the one history invariant `verify` cannot check from the parsed map:
opys reads YAML 1.2, where the bare and quoted spellings are the same string. So
it reads the document as stored, through `Backend::stored_text` (the seam exists
so verify does not read files behind the backend's back) — `history::unquoted_keys`
finds the bare keys and `verify::spelling_problems` reports them. That pass runs
only where the medium's bytes *are* the subject: the `verify` command and the
node's per-corpus problem list, never `collect_problems`, which also judges
documents that exist only in memory.

### The always-on node (`opys web`)

`opys-server` is a library as well as a binary, and the CLI mounts its whole
surface as `opys web` (ADR-0077). The surface *and* its one implementation live
in `opys-server/src/cli.rs` — `WebCommand` + `dispatch`, mounted twice (by
`opys/src/main.rs` and by `opys-server/src/main.rs`) so the two entry points are
argument plumbing and nothing else. Serving is `opys-server/src/serve.rs`
(`serve::blocking` owns the tokio runtime, which is what lets a plain `fn main`
start the node without a tokio dependency of its own); the unit file is
`opys-server/src/systemd.rs`.

The **user-facing** documentation is the README's "The always-on node (`opys
web`)" section — the walkthrough from an empty allowlist to a dashboard, plus
the systemd and NixOS/home-manager notes. It is the only place these commands
are written up for humans, and its transcripts are real command output: if you
change what a `web` subcommand prints, re-run it and paste the new output rather
than editing the block by hand.

- `opys web start` runs the node; `add`/`remove`/`list`/`scan` manage the
  **allowlist** at `~/.config/opys/server.toml`. **No endpoint accepts a
  filesystem path** (ADR-0077): `add` edits the file and never speaks to a
  running node, which watches the file instead. `scan` only suggests — it is
  handed a `&Registry`, so it *cannot* add anything.
- Writes over the API are typed actions (`action.rs`) that reproduce the CLI's
  call sequence exactly — its own `Project`/`Store`, the inventory flock, the
  same write-time rules — never a flush through a warm store. The request body
  is a closed enum, so the node cannot execute arbitrary commands, and it serves
  only what was allowlisted.
- `opys web install` writes `~/.config/systemd/user/opys-server.service` and
  *prints* the `systemctl --user` commands rather than running them; it refuses
  to overwrite without `--force`, and exits 0 with manual instructions where
  there is no systemd user manager — which means *systemd-booted*
  (`/run/systemd/system`), not merely Linux, or a container would get a unit
  nothing reads. A `--config` the user passed is baked into `ExecStart` (an
  install that read `bind` from a file the unit does not name would serve the
  default allowlist on that file's port); the resolved default is not, so the
  ordinary unit stays self-describing. `uninstall` prints the disable command
  *before* the removal line: deleting a unit file does not stop the service.
  Tests drive all of it over a fake `$HOME` **and** `$XDG_CONFIG_HOME` —
  `registry::config_home` prefers XDG, so setting only one leaks into the
  developer's real config.
- `web scan`'s scan root is spelled `--under`, not `--root`: the CLI's global
  `--root` propagates into every subcommand, so that name is taken tree-wide.
  `opys/src/main.rs` *refuses* `--root`/`--no-sync` under `web` (exit 2, naming
  `--under`) rather than warning — ignoring them yields a confident scan of the
  home directory that looks exactly like a scan of the right tree.
- A prefix entry's `depth` counts **project directories**, one level shallower
  than the `opys.toml` the walk reaches: `discover::scan_projects` owns the `+1`
  and is pinned against `registry::Entry::covers` by a test. When those two
  disagree, `add` refuses to allowlist projects the node never serves.
- The dashboard (`opys-server/ui/`, embedded by `assets.rs`) is a sidebar of
  projects/corpora — each with a verify dot — over four views: board, document,
  query console, and the worktree union. Its writes are the same typed actions;
  there is no create-document view, so `opys new` stays a CLI job. The document
  view's log entry box reads `DocView::log_sections`, so the client never has to
  work out which sections are logs.

`opys/tests/web.rs` covers the surface end to end. `opys/tests/cli.rs` is the
byte-identity pin for every pre-existing command and must keep passing untouched.

## Conventions

- Errors that should abort a command are `OpysError` (`src/error.rs`); content
  problems for `verify` are pushed onto an error `Vec` instead. Keep that
  distinction — don't turn verify findings into hard errors.
- Reach for the inline-scalar / block-YAML split in `frontmatter.rs` rather than
  formatting YAML by hand elsewhere.
- Integration tests (`tests/cli.rs`) drive the built binary with `assert_cmd`
  over a `tempfile` project; unit tests live next to the code they cover.

## Multi-agent packaging

The repo is also a multi-agent plugin for the `opys` skill. The
`skills/opys/` folder is the conditional skill (shipped by the
Claude Code plugin in `.claude-plugin/`, the Codex plugin in `.codex-plugin/`,
the Gemini extension `gemini-extension.json`, the opencode `opencode.json`, and
the pi extension `pi-extension/` + root `package.json`).

The always-on rule has **one** source: `skills/opys/agent-rule.md`.
There are deliberately no committed per-editor copies — `opys agent-rules --tool
<editor>` (`commands/agent_rules.rs`) generates them on demand from that file,
which is embedded in the binary via `templates::AGENT_RULE` (`include_str!`) and
also referenced by the Gemini/opencode manifests and read by the pi extension.
Edit the rule in one place; everything else derives from it.

## Feature inventory (dogfooding)

This repo tracks its own features with opys: the inventory lives under
`inventory/` (configured in `opys.toml` at the root; `opys/` was taken by the
binary crate). To find documents: `rg` by tag/status or `opys list`, then read
only the relevant files. To create or change documents, use the CLI (`new
--type`, `set-status`, `tag`, `log`, `retire`, `block`, `close`); body prose
edits are normal file edits, but a `## Progress` entry goes through `opys log`
(it is a `log` section, so its bullets must be dated) — run `opys verify` before
finishing. Known code-review
findings are filed as `bug`/`chore` docs linked to their feature.

This repository allocates ids densely today — there is no `[ids]` table in its
`opys.toml`; `[ids] modulus` (ADR-0103) is the switch to flip when the inventory
goes multi-branch.

Product and architecture decisions are `adr` docs (`new --type adr`;
Context/Decision/Consequences sections, statuses proposed → accepted →
superseded). Consult the accepted ADRs before changing anything they settled,
and file a new ADR (or supersede one) when a direction changes rather than
editing an accepted decision in place.
