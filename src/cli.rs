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
        /// Defaults to the type's `default_status`.
        #[arg(long, default_value = "")]
        status: String,
        /// Comma-separated IDs this document references (e.g. linked features).
        #[arg(long, default_value = "")]
        features: String,
        /// Sets `<status>_reason` (e.g. wontfix/blocked/archived).
        #[arg(long)]
        reason: Option<String>,
        /// Custom field key=value (repeatable).
        #[arg(long = "field")]
        field: Vec<String>,
    },

    /// Bulk-create documents of one type from a JSONL file (one JSON object per
    /// line), allocating sequential IDs and syncing once. Run `verify` after.
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

    /// Guarded status transition. The id may be a comma-separated list to move
    /// several documents at once (same status/reason applied to each).
    SetStatus {
        /// One id, a comma-separated list (e.g. `FEAT-1,FEAT-2`), or `-` to read
        /// the list from stdin (comma/space/newline-separated).
        ids: String,
        status: String,
        /// Required when moving to wontfix.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Add/remove tags on one or more documents.
    Tag {
        /// One id, a comma-separated list (e.g. `FEAT-1,FEAT-2`), or `-` to read
        /// the list from stdin (comma/space/newline-separated).
        ids: String,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        remove: Option<String>,
    },

    /// Delete one or more documents; each ID is logged and never reused.
    Retire {
        /// One id, a comma-separated list (e.g. `FEAT-1,FEAT-2`), or `-` to read
        /// the list from stdin (comma/space/newline-separated).
        ids: String,
        #[arg(long)]
        reason: String,
    },

    /// Mark one or more documents as blocked by another, linking both
    /// directions. Each blocked document is auto-set to `blocked` if its type
    /// has that status.
    Block {
        /// The blocked document's id, a comma-separated list, or `-` for stdin.
        ids: String,
        /// The blocking document's ID.
        #[arg(long = "by")]
        by: String,
    },

    /// Remove a blocker link added by `block` from one or more documents.
    Unblock {
        /// One id, a comma-separated list (e.g. `FEAT-1,FEAT-2`), or `-` to read
        /// the list from stdin (comma/space/newline-separated).
        ids: String,
        #[arg(long = "by")]
        by: String,
    },

    /// Integrity check (CI gate).
    Verify,

    /// Reconcile references, linkify prose, and relocate docs to their layout path (after hand edits).
    Sync,

    /// Per-type status breakdown (counts + percentages) and coverage stats.
    Stats,

    /// Reconstruct a document's lifecycle from git history: the status timeline
    /// across commits, following the file across relocations (e.g. into
    /// `_archived/`). Reads the repository in-process and decodes each revision
    /// through the document parser — no subprocess, no string scraping.
    /// Requires the optional `history` build feature.
    #[cfg(feature = "history")]
    History { id: String },

    /// Finish a document of a type with a terminal status: delete the file and
    /// strike its title in every referencing doc (the struck reference reserves
    /// the ID forever).
    Close {
        /// One id, a comma-separated list (e.g. `FEAT-1,FEAT-2`), or `-` to read
        /// the list from stdin (comma/space/newline-separated).
        ids: String,
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

    /// Launch the interactive terminal UI: a live board over the inventory that
    /// updates as documents change on disk.
    Tui {
        /// Project directory to open (where to search upward for opys.toml).
        /// Overrides `--root`; defaults to the current directory.
        dir: Option<String>,
    },

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
