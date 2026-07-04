use clap::Parser;

use opys::cli::{Cli, Command};
use opys::Ctx;
use opys_backend_markdown_local::MarkdownLocal;

fn main() {
    let Cli {
        root,
        no_sync,
        command,
    } = Cli::parse();

    // The TUI lives in its own crate; intercept it here so the core library
    // stays UI-free. Everything else goes through opys::run with the injected
    // markdown-local backend.
    let result = match command {
        Command::Tui { dir } => {
            let ctx = Ctx {
                root: dir.unwrap_or(root),
                no_sync,
                backend: Box::new(MarkdownLocal),
            };
            opys_tui::run(&ctx)
        }
        command => opys::run(
            Cli {
                root,
                no_sync,
                command,
            },
            Box::new(MarkdownLocal),
        ),
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
