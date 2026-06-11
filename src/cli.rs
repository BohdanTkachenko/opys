//! Command-line interface (clap derive).

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "opys", version, about = "File-based feature inventory manager")]
pub struct Cli {
    /// Project root.
    #[arg(long, default_value = ".", global = true)]
    pub root: String,

    /// Inventory base directory under the root (holds features/, views/,
    /// runbooks/). Absolute paths are used as-is. Env: OPYS_DIR.
    #[arg(long, default_value = "docs", env = "OPYS_DIR", global = true)]
    pub dir: String,

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

#[derive(Clone, Copy, ValueEnum)]
pub enum SchemaKind {
    /// JSON Schema for `_config.toml`.
    Config,
    /// JSON Schema for feature frontmatter, derived from the project's config.
    Frontmatter,
}

#[derive(Subcommand)]
pub enum Command {
    /// Bootstrap the inventory directory and config; print the CLAUDE.md snippet.
    Init,

    /// Create a feature file with the next ID.
    New {
        #[arg(long)]
        title: String,
        /// Comma-separated, kebab-case.
        #[arg(long)]
        tags: String,
        #[arg(long, default_value = "planned")]
        status: String,
        /// Required when creating directly as wontfix.
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
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        status: Option<String>,
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

    /// Emit a JSON Schema for editor/CI validation.
    Schema {
        #[arg(long, value_enum, default_value_t = SchemaKind::Config)]
        kind: SchemaKind,
        /// Write to file instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
}
