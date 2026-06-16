//! `opys agent-rules` — generate a rules-based editor's always-on instruction
//! file from the single canonical rule (`templates::AGENT_RULE`). No per-editor
//! copies are kept in the repo; this writes the right file in the right place,
//! adding any host-specific frontmatter.

use std::path::Path;

use crate::cli::AgentTool;
use crate::error::{usage, Result};
use crate::templates::AGENT_RULE;
use crate::Ctx;

/// Cursor scopes the rule to inventory files via `globs`.
const CURSOR_FRONTMATTER: &str = "---\ndescription: opys feature inventory — operate the opys CLI when the project has a opys/ inventory.\nglobs: opys/**\nalwaysApply: false\n---\n\n";
/// Copilot path-specific instruction file, scoped with `applyTo`.
const COPILOT_FRONTMATTER: &str = "---\napplyTo: \"opys/**\"\n---\n\n";

/// (output path relative to the project root, host-specific frontmatter).
fn target(tool: AgentTool) -> (&'static str, &'static str) {
    match tool {
        AgentTool::Cursor => (".cursor/rules/opys.mdc", CURSOR_FRONTMATTER),
        AgentTool::Windsurf => (".windsurf/rules/opys.md", ""),
        AgentTool::Cline => (".clinerules/opys.md", ""),
        AgentTool::Copilot => (
            ".github/instructions/opys.instructions.md",
            COPILOT_FRONTMATTER,
        ),
        AgentTool::Kiro => (".kiro/steering/opys.md", ""),
        AgentTool::All => unreachable!("expanded before target()"),
    }
}

pub fn run(ctx: &Ctx, tool: AgentTool, stdout: bool) -> Result<()> {
    let tools = match tool {
        AgentTool::All => vec![
            AgentTool::Cursor,
            AgentTool::Windsurf,
            AgentTool::Cline,
            AgentTool::Copilot,
            AgentTool::Kiro,
        ],
        t => vec![t],
    };
    if stdout && tools.len() > 1 {
        return Err(usage("--stdout works with a single --tool, not 'all'"));
    }

    for t in tools {
        let (rel, frontmatter) = target(t);
        let content = format!("{frontmatter}{AGENT_RULE}");
        if stdout {
            print!("{content}");
        } else {
            let path = Path::new(&ctx.root).join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, content)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
