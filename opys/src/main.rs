use clap::Parser;

use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::cli::Cli;

fn main() {
    match opys_engine::run(Cli::parse(), Box::new(MarkdownLocal)) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
