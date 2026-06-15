//! Command-line interface (clap derive).

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "opys", version, about = "File-based feature inventory manager")]
pub struct Cli {
    /// Project root.
    #[arg(long, default_value = ".", global = true)]
    pub root: String,

    /// Inventory base directory under the root (holds features/, work-items/,
    /// views/, runbooks/). Absolute paths are used as-is. Env: OPYS_DIR.
    #[arg(long, default_value = "docs/opys", env = "OPYS_DIR", global = true)]
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

/// A hardcoded work-item type (selects the ID prefix and per-type rules).
#[derive(Clone, Copy, ValueEnum)]
pub enum WiType {
    Task,
    Bug,
    Chore,
}

impl WiType {
    /// The type name, matching `config::WorkItemType::name`.
    pub fn name(self) -> &'static str {
        match self {
            WiType::Task => "task",
            WiType::Bug => "bug",
            WiType::Chore => "chore",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SchemaKind {
    /// JSON Schema for `_config.toml`.
    Config,
    /// JSON Schema for feature frontmatter, derived from the project's config.
    Frontmatter,
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

    /// Emit a JSON Schema for editor/CI validation.
    Schema {
        #[arg(long, value_enum, default_value_t = SchemaKind::Config)]
        kind: SchemaKind,
        /// Write to file instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },

    /// Manage ephemeral work items (implementation tracking) linked to features.
    #[command(alias = "wi", subcommand)]
    WorkItem(WorkItemCommand),

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
pub enum WorkItemCommand {
    /// Scaffold the work-items/ directory and config.
    Init,

    /// Create a work item with the next ID, linked to one or more features.
    New {
        #[arg(long)]
        title: String,
        /// Work-item type, selecting the ID prefix (task→TASK, bug→BUG,
        /// chore→CHORE) and any per-type required sections.
        #[arg(long = "type", value_enum, default_value_t = WiType::Task)]
        wi_type: WiType,
        /// Comma-separated existing feature IDs (at least one required).
        #[arg(long)]
        features: String,
        #[arg(long, default_value = "todo")]
        status: String,
        /// Comma-separated, kebab-case (optional for work items).
        #[arg(long, default_value = "")]
        tags: String,
        /// Required when creating directly as blocked.
        #[arg(long)]
        reason: Option<String>,
        /// Custom field key=value (repeatable).
        #[arg(long = "field")]
        field: Vec<String>,
    },

    /// Print a work-item file.
    Show { id: String },

    /// Filtered listing.
    List {
        /// Only items linked to this feature ID.
        #[arg(long)]
        feature: Option<String>,
        /// Only items of this type (task/bug/chore).
        #[arg(long = "type", value_enum)]
        wi_type: Option<WiType>,
        #[arg(long)]
        status: Option<String>,
        /// Filter by custom field: key=value (repeatable). Matches when the
        /// field equals the value (or, for list fields, contains it).
        #[arg(long = "field")]
        field: Vec<String>,
        #[arg(long, value_enum, default_value_t = ListFormat::Table)]
        format: ListFormat,
    },

    /// Guarded status transition (todo | in-progress | blocked + extras).
    SetStatus {
        id: String,
        status: String,
        /// Required when moving to blocked.
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

    /// Complete a work item: delete the file and strike its title in every
    /// referencing doc (the struck reference reserves the ID forever).
    Close {
        id: String,
        /// Close even if some `## Tasks` items are unchecked.
        #[arg(long)]
        force: bool,
    },

    /// Strip struck-through (completed) work-item references from all docs.
    Cleanup,
}
