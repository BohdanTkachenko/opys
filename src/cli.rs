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

    /// Skip the automatic INDEX.md/views regeneration after mutating commands.
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

    /// Bulk-create features from a JSONL file (one JSON object per line),
    /// allocating sequential IDs and syncing once. Run `verify` afterwards.
    Import {
        /// Path to a `.jsonl` file. Each line is an object with `title` and
        /// `tags` (required), optional `status`/`spec`/custom fields, and an
        /// optional `body` (markdown placed under the title heading).
        file: String,
    },

    /// Print a feature file.
    Show { id: String },

    /// Filtered listing.
    List {
        /// Restrict to one document type.
        #[arg(long = "type")]
        type_name: Option<String>,
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

    /// Guarded status transition.
    SetStatus {
        id: String,
        status: String,
        /// Required when moving to wontfix.
        #[arg(long)]
        reason: Option<String>,
    },

    /// Add/remove tags.
    Tag {
        id: String,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        remove: Option<String>,
    },

    /// Delete a feature; its ID is never reused.
    Retire {
        id: String,
        #[arg(long)]
        reason: String,
    },

    /// Mark an item (FEAT/WI) as blocked by another, linking both directions.
    /// A blocked work item is auto-set to `blocked` status.
    Block {
        /// The blocked item's ID (a feature or work-item id).
        id: String,
        /// The blocking item's ID.
        #[arg(long = "by")]
        by: String,
    },

    /// Remove a blocker link added by `block`.
    Unblock {
        id: String,
        #[arg(long = "by")]
        by: String,
    },

    /// Integrity check (CI gate).
    Verify,

    /// Regenerate INDEX.md and views/.
    SyncViews,

    /// Progress, coverage, and (optionally) parity stats.
    Report,

    /// Aggregate manual items into a runbook.
    ManualRunbook {
        /// Write to file (e.g. runbooks/release-0.3.md).
        #[arg(long)]
        out: Option<String>,
        /// Runbook title suffix.
        #[arg(long)]
        name: Option<String>,
    },

    /// Finish a document of a type with a terminal status: delete the file and
    /// strike its title in every referencing doc (the struck reference reserves
    /// the ID forever).
    Close {
        id: String,
        /// Close even if a required checklist section has unchecked items.
        #[arg(long)]
        force: bool,
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
