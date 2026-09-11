//! Command-line interface (clap derive).

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "opys",
    version,
    about = "File-based inventory of typed markdown documents"
)]
pub struct Cli {
    /// Where to start searching upward for `opys.toml` (the project root).
    /// Defaults to the current directory.
    #[arg(long, default_value = ".", global = true)]
    pub root: String,

    /// Skip the automatic sync (reconcile/linkify/relocate) after mutating commands.
    #[arg(long, global = true)]
    pub no_sync: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ListFormat {
    Table,
    Ids,
    Paths,
}

/// A rules-based editor that reads an always-on instruction file.
#[derive(Clone, Copy, ValueEnum)]
pub enum AgentTool {
    Cursor,
    Windsurf,
    Cline,
    Copilot,
    Kiro,
    /// Generate the rule file for every supported editor.
    All,
}

#[derive(Subcommand)]
pub enum Command {
    /// Bootstrap the inventory directory and config; print the CLAUDE.md snippet.
    Init,

    /// Create a document of a configured type with the next ID.
    New {
        /// Document type (configured in opys.toml; default `feature`).
        #[arg(long = "type", default_value = "feature")]
        type_name: String,
        #[arg(long)]
        title: String,
        /// Comma-separated, kebab-case (required when the type requires tags).
        #[arg(long, default_value = "")]
        tags: String,
        /// Comma-separated IDs this document references (e.g. linked features).
        #[arg(long, default_value = "")]
        features: String,
        /// Sets `<default_status>_reason` — only where the type declares that field.
        #[arg(long)]
        reason: Option<String>,
        /// Custom field key=value (repeatable).
        #[arg(long = "field")]
        field: Vec<String>,
    },

    /// Bulk-create documents of one type from a JSONL file (one JSON object per
    /// line), allocating the IDs as one block and syncing once. Run `verify` after.
    Import {
        /// Document type to create (configured in opys.toml; default `feature`).
        #[arg(long = "type", default_value = "feature")]
        type_name: String,
        /// Path to a `.jsonl` file. Each line is an object with `title` and
        /// `tags` (required), optional `status`/custom fields, and an optional
        /// `body` (markdown placed under the title heading).
        file: String,
    },

    /// Print a document.
    Show {
        id: String,
        /// After the document, list textual references to its id found in code
        /// (scans `[file_refs].roots`, matching the configured id formats).
        #[arg(long)]
        refs: bool,
    },

    /// Filtered listing.
    List {
        /// Restrict to one document type.
        #[arg(long = "type")]
        type_name: Option<String>,
        /// Filter by tag. Matches an exact tag, or any tag with this key
        /// (the head before `:` or `=`, e.g. `--tag area` matches
        /// `area:parsing` and `area=high`).
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        status: Option<String>,
        /// Filter by custom field: key=value (repeatable). Matches when the
        /// field equals the value (or, for list fields, contains it).
        #[arg(long = "field")]
        field: Vec<String>,
        /// Only documents something live still blocks: a `blocked_by` entry
        /// that is not struck through and whose target's status is not one its
        /// type releases at (`releases_blocks`).
        #[arg(long, conflicts_with = "unblocked")]
        blocked: bool,
        /// Only documents nothing live blocks — what to pick up next.
        #[arg(long)]
        unblocked: bool,
        #[arg(long, value_enum, default_value_t = ListFormat::Table)]
        format: ListFormat,
    },

    /// List the distinct tags in the inventory, sorted, one per line.
    Tags {
        /// List distinct tag keys (the head before `:` / `=`) instead of full
        /// tags — pairs with `list --tag <key>`.
        #[arg(long)]
        keys: bool,
    },

    /// Guarded status transition: the type's statuses, the edges its
    /// `[transitions]` map declares (when it declares any), and its rules. Takes
    /// several ids to move a batch at once (the same status/reason applied to
    /// each); a refused id aborts the batch.
    SetStatus {
        /// One or more ids: several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        status: String,
        /// Required when moving to wontfix.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Add/remove tags on one or more documents.
    Tag {
        /// One or more ids: several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        remove: Option<String>,
    },

    /// Append a dated entry to the document's `log` section.
    ///
    /// The entry is stamped with the current UTC time to the second, so a log
    /// reads as a timeline and `verify` can check that it does not go
    /// backwards. `TEXT` of `-` reads the entry from stdin (for a multi-line
    /// note, whose continuation lines are indented under the bullet).
    ///
    /// This is the one mutating command that takes a single id: the second
    /// positional is the text, and `-` there already means stdin, so an id list
    /// would be ambiguous.
    Log {
        /// The document to log to.
        id: String,
        /// The entry text, or `-` to read it from stdin.
        text: String,
        /// Which log section to append to. Required only for a type that
        /// declares more than one.
        #[arg(long)]
        section: Option<String>,
    },

    /// Delete one or more documents; each ID is logged and never reused.
    /// Inbound references are struck into `~~title~~` tombstones, like `close`.
    Retire {
        /// One or more ids: several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Optional context, echoed with the confirmation. The ledger records
        /// the id and title; git records the when and why.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Mark one or more documents as blocked by another, linking both
    /// directions. Status is untouched: blocked is derived from the live
    /// `blocked_by` entries (see `list --blocked`).
    Block {
        /// The blocked document(s): several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        /// The blocking document's ID.
        #[arg(long = "by")]
        by: String,
    },

    /// Remove a blocker link added by `block` from one or more documents.
    Unblock {
        /// One or more ids: several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long = "by")]
        by: String,
    },

    /// Integrity check (CI gate).
    ///
    /// Exit codes: 0 clean; 1 content problems; 2 an error (bad flags, IO, a
    /// missing config); 3 observations and nothing else — a document has sat in
    /// a status past its type's `ttl`, or entered one as often as `max_visits`
    /// allows. Gate CI on 1: most scripts gate on any nonzero code, which trips
    /// on 3 as well, and 3 is a corpus that was clean when it was written.
    Verify {
        /// Perform the fixable observations: move each stuck document to its
        /// `ttl` target and each overflowed one to its `then`, as ordinary
        /// transitions (history appended, the sync pass run, the layout
        /// relocation performed), then re-verify and exit on what remains.
        ///
        /// At most one fix per document per run; a fix the rules refuse is
        /// reported as a problem rather than aborting the pass; nothing is
        /// written when nothing was fixed; and no fixing happens at all while
        /// `opys config validate` reports a problem, since a fix is a move
        /// along the graph that check guards.
        #[arg(long)]
        fix: bool,
    },

    /// Reconcile references, linkify prose, and relocate docs to their layout path (after hand edits).
    Sync,

    /// Render the configured `[[stats]]` sections. Styled for a terminal;
    /// plain markdown when piped (or with `--plain` / `NO_COLOR`).
    Stats {
        /// Emit raw markdown instead of styled terminal output.
        #[arg(long)]
        plain: bool,
    },

    /// Run a SQL query over the inventory and print the result table. Read-only:
    /// every statement must be a SELECT, and nothing is ever written (bulk edits
    /// pipe `list --format ids` into a lifecycle command). Styled for a
    /// terminal; plain markdown when piped (or with `--plain` / `NO_COLOR`).
    ///
    /// Tables: docs(id, num, type, status, title, created, updated, body,
    /// path), tags(doc_id, seq, tag, key, value), relations(doc_id, field,
    /// seq, ref_id, ref_num, raw_value, title, struck),
    /// history(doc_id, seq, ts, status), fields(doc_id, key,
    /// value), sections(doc_id, heading, kind, items, unchecked),
    /// blocks(doc_id, seq, heading, text),
    /// retired(id, num, title).
    Query {
        /// The SQL to run (`-` reads it from stdin). Must be a SELECT.
        sql: String,
        /// Emit raw markdown instead of styled terminal output.
        #[arg(long)]
        plain: bool,
        /// Bind a value read from stdin to `$1` in the SQL — for large or
        /// multi-line values (e.g. a section body) without SQL-escaping;
        /// trailing whitespace is trimmed. Incompatible with `sql = -`.
        #[arg(long)]
        stdin: bool,
    },

    /// Reconstruct a document's lifecycle from git history: the status timeline
    /// across commits, following the file across relocations (e.g. into
    /// `_archived/`). Reads the repository in-process and decodes each revision
    /// through the document parser — no subprocess, no string scraping. A
    /// binary compiled without the `git` cargo feature still takes the command
    /// and answers by naming the feature it lacks.
    History {
        id: String,
        /// Write the document's `history` lifecycle map from that timeline, for
        /// a one-time migration of an inventory that predates the map (see
        /// docs/migration-history.md). Refuses to overwrite an existing map.
        #[arg(long)]
        seed: bool,
    },

    /// Finish a document of a type with a terminal status — and, where the type
    /// declares `[transitions]`, an edge into one from the document's current
    /// status: delete the file, strike its title in every referencing doc, and
    /// log the ID to `_retired.md` so it is never reused.
    Close {
        /// One or more ids: several positionals (`FEAT-0001 FEAT-0002`), a
        /// comma-separated list (`FEAT-0001,FEAT-0002`), or `-` to read the list from
        /// stdin (comma/space/newline-separated). Ids are applied in order in a
        /// single pass and any failure aborts the whole invocation — nothing is
        /// written.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Close even if a required checklist section has unchecked items.
        #[arg(long)]
        force: bool,
    },

    /// Resolve ID conflicts between branches: keep IDs that existed at the git
    /// merge-base with main/master and renumber the ones added on this branch.
    Renumber {
        /// The git ref to treat as the base (defaults to merge-base with main/master).
        #[arg(long)]
        base: Option<String>,
    },

    /// Strip struck-through (closed) references from every document.
    Cleanup,

    /// Project configuration (generate/inspect the universal opys.toml).
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Generate the always-on agent rule file for a rules-based editor
    /// (Cursor/Windsurf/Cline/Copilot/Kiro) from the canonical rule.
    AgentRules {
        #[arg(long, value_enum)]
        tool: AgentTool,
        /// Print to stdout instead of writing the file(s).
        #[arg(long)]
        stdout: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Generate the opinionated default opys.toml (never overwrites an existing one).
    Init,
    /// Parse opys.toml and check it is well-formed (exit 1 on problems).
    Validate,
}
