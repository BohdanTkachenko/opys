use clap::Parser;

use opys::cli::Cli;

fn main() {
    let cli = Cli::parse();
    match opys::run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
