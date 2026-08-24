//! The `web` subcommand surface (ADR-0077), and the one implementation of it.
//!
//! Mounted twice — as `opys web …` by the CLI and as `opys-server web …` by the
//! node's own binary — from this single enum and this single [`dispatch`]. The
//! two entry points are argument plumbing and nothing else, so there is no
//! second copy of any behaviour to drift.
//!
//! `add` and `remove` edit the allowlist file and nothing else. They never speak
//! to a running node, which is what keeps filesystem paths out of the HTTP
//! surface entirely (ADR-0052) — the node watches the file instead. And `scan`
//! cannot add anything: it is handed a `&Registry`, so the type system, not
//! discipline, is what stops it.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use opys_engine::error::{usage, Result};

use crate::discover;
use crate::registry::{self, EntryKind, Registry, DEFAULT_DEPTH};
use crate::serve;
use crate::systemd;

/// Where the node listens and which allowlist it serves.
///
/// ADR-0077: no project paths. The allowlist file is the only way in, so
/// approving a project and running the node stay separate acts.
///
/// `about`/`long_about` are reset because this struct is *flattened* into
/// `opys-server`'s root parser, and a flattened `Args`'s doc comment would
/// otherwise replace that command's own description in `--help`.
#[derive(Args)]
#[command(about = None, long_about = None)]
pub struct StartArgs {
    /// Address to listen on. Overrides `bind` in the allowlist file; the default
    /// is 127.0.0.1:6797.
    #[arg(long)]
    pub bind: Option<SocketAddr>,

    /// Allowlist file to serve from, instead of the default
    /// `$XDG_CONFIG_HOME/opys/server.toml`.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// What the installed systemd unit should say, and whether to replace one.
#[derive(Args)]
pub struct InstallArgs {
    /// Address to bake into the unit's `ExecStart`. Defaults the same way
    /// `web start` does: the allowlist file's `bind`, then 127.0.0.1:6797.
    #[arg(long)]
    pub bind: Option<SocketAddr>,

    /// Overwrite an existing unit file.
    #[arg(long)]
    pub force: bool,

    /// Allowlist file to read `bind` from.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

/// The always-on node's command surface.
#[derive(Subcommand)]
pub enum WebCommand {
    /// Run the node: serve the allowlisted projects over HTTP.
    Start(StartArgs),

    /// Allowlist a project directory (or, with --prefix, everything under one).
    Add {
        /// The project directory, or the directory to search under.
        path: PathBuf,

        /// Allowlist every project found beneath PATH, rather than PATH itself.
        #[arg(long)]
        prefix: bool,

        /// Allowlist file to edit.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },

    /// Drop a path from the allowlist.
    Remove {
        /// The path exactly as it was added (or anything resolving to it).
        path: PathBuf,

        /// Allowlist file to edit.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },

    /// Show the allowlist and what it expands to.
    List {
        /// Allowlist file to read.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },

    /// Suggest projects to allowlist. Never adds anything.
    Scan {
        /// Directory to search under. Defaults to your home directory.
        ///
        /// Spelled `--under`, not `--root`, and it has to be: `opys` declares a
        /// *global* `--root` (the inventory root), and clap propagates a global
        /// through the whole command tree — id and long name both — so a second
        /// `--root` anywhere beneath it is either a duplicate long option or a
        /// value of the wrong type arriving at the wrong level.
        #[arg(long, value_name = "PATH")]
        under: Option<PathBuf>,

        /// How many directory levels below the root to look, counted the same
        /// way a prefix entry's depth is. Defaults to 10.
        #[arg(long)]
        depth: Option<usize>,

        /// Allowlist file to compare against.
        #[arg(long, value_name = "PATH")]
        config: Option<PathBuf>,
    },

    /// Write the systemd user unit (prints how to enable it; runs nothing).
    Install(InstallArgs),

    /// Remove the systemd user unit.
    Uninstall,
}

/// Run one `web` subcommand, returning its exit code.
///
/// Every line the surface prints is printed here, so `opys web list` and
/// `opys-server web list` are the same output by construction.
pub fn dispatch(command: WebCommand) -> Result<i32> {
    match command {
        WebCommand::Start(args) => start(args),
        WebCommand::Add {
            path,
            prefix,
            config,
        } => add(&path, prefix, config.as_deref()),
        WebCommand::Remove { path, config } => remove(&path, config.as_deref()),
        WebCommand::List { config } => list(config.as_deref()),
        WebCommand::Scan {
            under,
            depth,
            config,
        } => scan(under.as_deref(), depth, config.as_deref()),
        WebCommand::Install(args) => install(args),
        WebCommand::Uninstall => uninstall(),
    }
}

/// `web start` — serve until the process is stopped.
fn start(args: StartArgs) -> Result<i32> {
    let config = serve::config_for(args.config.as_deref())?;
    let bind = serve::bind_address(args.bind, &config)?;
    serve::blocking(config, bind)?;
    Ok(0)
}

/// Expand a leading `~` in a path the user typed.
///
/// The allowlist file stores `~/…` and `web list`/`web remove` print it back,
/// so `opys web remove '~/work'` — the exact line `remove` suggests — has to
/// resolve. Unquoted it is the shell's job; quoted, in a variable, or inside an
/// `sh -c` string it is ours. Input and storage go through the same expansion.
fn user_path(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(s) if s.starts_with('~') => registry::expand_tilde(s),
        _ => path.to_path_buf(),
    }
}

/// Canonicalize a path the user typed, insisting it is a directory.
///
/// Done before the registry sees it: `Registry::add` will happily record a
/// prefix entry pointing at a *file*, which then sits in the allowlist as a
/// permanent error. Better to refuse at the point the user can still fix it.
fn allowlistable_dir(path: &Path) -> Result<PathBuf> {
    let path = &user_path(path);
    let canon =
        std::fs::canonicalize(path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    if !canon.is_dir() {
        return Err(usage(format!(
            "{}: not a directory — allowlist the directory that holds opys.toml",
            canon.display()
        )));
    }
    Ok(canon)
}

/// `web add` — record a project (or a prefix) in the allowlist file.
fn add(path: &Path, prefix: bool, config: Option<&Path>) -> Result<i32> {
    let config = serve::config_for(config)?;
    // Load, mutate and save are one edit: two `add`s racing would otherwise each
    // load the same pre-state and the second save would drop the first's entry.
    let _lock = registry::lock(&config)?;
    let mut registry = Registry::load_from(&config)?;
    let target = allowlistable_dir(path)?;
    let kind = if prefix {
        EntryKind::Prefix
    } else {
        EntryKind::Project
    };

    if registry
        .entries
        .iter()
        .any(|e| e.kind == kind && e.path == target)
    {
        println!("already allowlisted: {}", target.display());
        return Ok(0);
    }
    // A project already reached through a prefix needs no entry of its own; a
    // second one would only be something else to remove later.
    if kind == EntryKind::Project {
        if let Some(entry) = registry.entry_covering(&target) {
            println!(
                "already served by the {} entry {} in {}",
                entry.kind.key(),
                entry.raw_path,
                config.display()
            );
            return Ok(0);
        }
    }

    if registry.add(&target, kind)? {
        registry.save()?;
        println!("added {} to {}", target.display(), config.display());
        println!("a running node picks this up within a minute");
    }
    Ok(0)
}

/// `web remove` — drop a path from the allowlist file.
fn remove(path: &Path, config: Option<&Path>) -> Result<i32> {
    let config = serve::config_for(config)?;
    let _lock = registry::lock(&config)?;
    let mut registry = Registry::load_from(&config)?;
    // Not `allowlistable_dir`: an entry whose directory has since been deleted
    // is exactly the one a user most wants to remove.
    let path = user_path(path);
    let target = std::fs::canonicalize(&path).unwrap_or(path);

    if registry.remove(&target)? {
        registry.save()?;
        println!("removed {} from {}", target.display(), config.display());
        return Ok(0);
    }
    match registry.entry_covering(&target) {
        Some(entry) => {
            println!(
                "not allowlisted directly — served by the {} entry {}",
                entry.kind.key(),
                entry.raw_path
            );
            println!(
                "remove that entry instead: opys web remove {}",
                entry.raw_path
            );
        }
        None => println!("not in the allowlist: {}", target.display()),
    }
    Ok(0)
}

/// `n thing` / `n things`, for a count that is usually small.
fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// `web list` — the allowlist as written, then what it currently resolves to.
fn list(config: Option<&Path>) -> Result<i32> {
    let config = serve::config_for(config)?;
    let registry = Registry::load_from(&config)?;
    let bind = serve::resolve_bind(None, registry.bind.as_deref())?;

    println!("allowlist: {}", config.display());
    let source = if registry.bind.is_some() {
        "from the allowlist file"
    } else {
        "default"
    };
    println!("bind:      {bind} ({source})");
    println!();

    if registry.entries.is_empty() {
        println!("nothing allowlisted — add a project with: opys web add <path>");
        return Ok(0);
    }
    // The entry column is padded to its own widest row, so the `->` line up and
    // a long path in one entry does not shift every other one.
    let written: Vec<String> = registry
        .entries
        .iter()
        .map(|entry| match entry.kind {
            EntryKind::Prefix => format!("{} (depth {})", entry.raw_path, entry.depth),
            EntryKind::Project => entry.raw_path.clone(),
        })
        .collect();
    let width = written.iter().map(String::len).max().unwrap_or(0);
    for (entry, written) in registry.entries.iter().zip(&written) {
        let resolved = match &entry.error {
            Some(err) => format!("error: {err}"),
            None => entry.path.display().to_string(),
        };
        println!(
            "  {:<7}  {written:<width$}  -> {resolved}",
            entry.kind.key()
        );
    }
    println!();

    let groups = discover::expand(&registry);
    let corpora: usize = groups.iter().map(|g| g.corpora.len()).sum();
    if corpora == 0 {
        println!("serving nothing: no project matched those entries");
        return Ok(0);
    }
    println!(
        "serving {} in {}:",
        plural(corpora, "corpus", "corpora"),
        plural(groups.len(), "project", "projects")
    );
    let width = groups.iter().map(|g| g.name.len()).max().unwrap_or(0);
    for group in &groups {
        for corpus in &group.corpora {
            let mut line = format!("  {:<width$}  {}", group.name, corpus.root.display());
            // The branch and the primary marker are what distinguish worktrees of
            // one project from each other; on a lone corpus they say nothing, so
            // they are left off rather than padded out with a placeholder.
            if let Some(branch) = &corpus.branch {
                line.push_str("  ");
                line.push_str(branch);
            }
            if group.corpora.len() > 1 && corpus.is_primary {
                line.push_str("  (primary)");
            }
            println!("{line}");
            if let Some(err) = &corpus.error {
                println!("    error: {err}");
            }
        }
    }
    Ok(0)
}

/// `web scan` — suggestions only. Adds nothing, by construction.
fn scan(under: Option<&Path>, depth: Option<usize>, config: Option<&Path>) -> Result<i32> {
    let config = serve::config_for(config)?;
    let registry = Registry::load_from(&config)?;

    let (root, default_depth) = match under {
        Some(path) => (path.to_path_buf(), DEFAULT_DEPTH),
        None => {
            // `suggest_default` answers `~`, which without `$HOME` is a literal
            // relative path — a silent scan of the wrong tree. Refuse instead.
            if registry::home_dir().is_none() {
                return Err(usage(
                    "$HOME is not set — pass --under <path> to say where to scan",
                ));
            }
            discover::suggest_default(&registry)
        }
    };
    // Canonicalize before comparing: allowlist entries are canonical, so
    // `--under .` or a symlinked home would otherwise report an allowlisted
    // project as new.
    let root =
        std::fs::canonicalize(&root).map_err(|e| usage(format!("{}: {e}", root.display())))?;
    if !root.is_dir() {
        return Err(usage(format!("{}: not a directory", root.display())));
    }
    let depth = depth.unwrap_or(default_depth);

    println!("scanning {} (depth {depth})…", root.display());
    let found = discover::suggest(&root, depth, &registry);
    if found.is_empty() {
        println!("no projects found under {}", root.display());
        return Ok(0);
    }
    for suggestion in &found {
        let mark = if suggestion.already_allowlisted {
            "  (allowlisted)"
        } else {
            ""
        };
        println!("  {}{mark}", suggestion.path.display());
    }
    println!();
    match found.iter().find(|s| !s.already_allowlisted) {
        Some(next) => {
            println!("scan never adds anything — allowlist one with:");
            println!("  opys web add {}", next.path.display());
        }
        None => println!("every project found is already allowlisted"),
    }
    Ok(0)
}

/// `web install` — write the unit, print how to enable it, run nothing.
fn install(args: InstallArgs) -> Result<i32> {
    let exe = systemd::current_exe()?;
    let dir = systemd::unit_dir();
    // Resolved *after* `unit_dir`, and tolerantly when there is no unit
    // directory: an environment with neither `$HOME` nor `$XDG_CONFIG_HOME` has
    // no default allowlist path, and that is precisely one of the environments
    // the print-and-succeed branch exists for. Failing here would make the
    // documented branch unreachable in the case it was written for.
    let config = match (serve::config_for(args.config.as_deref()), dir.is_some()) {
        (Ok(path), _) => Some(path),
        (Err(e), true) => return Err(e),
        (Err(_), false) => None,
    };
    let bind = match &config {
        Some(path) => serve::bind_address(args.bind, path)?,
        None => serve::resolve_bind(args.bind, None)?,
    };
    // Only an allowlist the user *named* is baked into the command. The default
    // is left to resolve itself, so the unit stays self-describing — but a
    // `--config` that reached `bind` and not `ExecStart` would leave the service
    // on that file's port serving a different file's projects.
    //
    // Absolute, because neither consumer runs from this shell's directory:
    // systemd starts the unit with the user's home as the working directory, and
    // the printed manual command is one the user pastes somewhere else. Not
    // canonicalized — the file need not exist yet.
    let named = config
        .as_deref()
        .filter(|_| args.config.is_some())
        .map(|path| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));

    let Some(dir) = dir else {
        return Ok(no_systemd(&exe, bind, named.as_deref()));
    };
    let path = systemd::install(&dir, &exe, bind, named.as_deref(), args.force)?;
    println!("wrote {}", path.display());
    println!();
    println!("enable it with:");
    println!("  systemctl --user daemon-reload && systemctl --user enable --now opys-server");
    println!();
    println!("the node will listen on http://{bind}");
    Ok(0)
}

/// `web uninstall` — remove the unit, print how to forget it.
fn uninstall() -> Result<i32> {
    let Some(dir) = systemd::unit_dir() else {
        println!("no systemd user unit directory here — there is nothing to remove");
        return Ok(0);
    };
    if !systemd::is_installed(&dir) {
        println!("no unit at {}", systemd::unit_path(&dir).display());
        return Ok(0);
    }
    // Printed *before* the removal line, because that is the order the user has
    // to execute in: deleting a unit file does not stop the service it started,
    // and a node left running still holds the port the next install wants.
    println!("stop it first — removing the unit does not stop a running service:");
    println!("  systemctl --user disable --now opys-server && systemctl --user daemon-reload");
    println!();
    match systemd::uninstall(&dir)? {
        Some(path) => println!("removed {}", path.display()),
        None => println!("no unit at {}", systemd::unit_path(&dir).display()),
    }
    Ok(0)
}

/// Nowhere to install a unit: say how to run the node by hand and succeed.
///
/// Exit 0 on purpose. "This machine does not use systemd" is a fact about the
/// machine, not a mistake the user made, and a non-zero exit here would fail
/// every setup script run on a Mac.
///
/// The printed command carries whatever `--config` the user passed, so it is a
/// line they can paste and have serve what `install` just reported on.
fn no_systemd(exe: &Path, bind: SocketAddr, config: Option<&Path>) -> i32 {
    let config = match config {
        Some(path) => format!(" --config {}", path.display()),
        None => String::new(),
    };
    println!("no systemd user unit directory here — nothing was installed");
    println!();
    println!("run the node yourself with:");
    println!("  {} web start --bind {bind}{config}", exe.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// A parser standing in for either binary's `web` mount point.
    #[derive(Parser)]
    #[command(name = "web")]
    struct Harness {
        #[command(subcommand)]
        command: WebCommand,
    }

    fn parse(args: &[&str]) -> WebCommand {
        Harness::parse_from(args).command
    }

    #[test]
    fn the_surface_is_the_seven_documented_subcommands() {
        let names: Vec<String> = Harness::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .filter(|n| n != "help")
            .collect();
        assert_eq!(
            names,
            [
                "start",
                "add",
                "remove",
                "list",
                "scan",
                "install",
                "uninstall"
            ]
        );
    }

    #[test]
    fn start_takes_no_roots_only_bind_and_config() {
        let WebCommand::Start(args) = parse(&[
            "web",
            "start",
            "--bind",
            "0.0.0.0:1234",
            "--config",
            "/tmp/server.toml",
        ]) else {
            panic!("expected start");
        };
        assert_eq!(
            args.bind.map(|b| b.to_string()).as_deref(),
            Some("0.0.0.0:1234")
        );
        assert_eq!(args.config, Some(PathBuf::from("/tmp/server.toml")));
        assert!(
            Harness::try_parse_from(["web", "start", "/tmp/a"]).is_err(),
            "a project path is not an argument (ADR-0077)"
        );
    }

    #[test]
    fn add_defaults_to_a_project_entry_and_opts_into_a_prefix() {
        let WebCommand::Add { path, prefix, .. } = parse(&["web", "add", "/tmp/proj"]) else {
            panic!("expected add");
        };
        assert_eq!(path, PathBuf::from("/tmp/proj"));
        assert!(!prefix);

        let WebCommand::Add { prefix, .. } = parse(&["web", "add", "/tmp/work", "--prefix"]) else {
            panic!("expected add");
        };
        assert!(prefix);
    }

    /// The guarantee is structural, not a matter of care: `scan` has no path to
    /// a mutable registry, so it cannot allowlist anything.
    #[test]
    fn scan_takes_a_root_and_a_depth_and_nothing_that_writes() {
        let WebCommand::Scan { under, depth, .. } =
            parse(&["web", "scan", "--under", "/tmp", "--depth", "3"])
        else {
            panic!("expected scan");
        };
        assert_eq!(under, Some(PathBuf::from("/tmp")));
        assert_eq!(depth, Some(3));
    }

    /// The scan root cannot be `--root`: the CLI's global `--root` owns that
    /// name in every subcommand. Pinned so nobody "fixes" the spelling back.
    #[test]
    fn the_scan_root_is_under_not_root() {
        assert!(Harness::try_parse_from(["web", "scan", "--root", "/tmp"]).is_err());
    }

    #[test]
    fn install_takes_bind_and_force() {
        let WebCommand::Install(args) =
            parse(&["web", "install", "--bind", "0.0.0.0:1", "--force"])
        else {
            panic!("expected install");
        };
        assert_eq!(
            args.bind.map(|b| b.to_string()).as_deref(),
            Some("0.0.0.0:1")
        );
        assert!(args.force);
    }
}
