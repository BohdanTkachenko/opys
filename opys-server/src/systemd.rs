//! The systemd user unit: writing it, removing it, and knowing when there is
//! nowhere to put it.
//!
//! `opys web install` never runs `systemctl`. It writes one file and prints the
//! two commands that activate it, so the user sees exactly what is about to
//! happen to their session — and so the same code is safe to run in a test, in a
//! container, and on a machine that has no systemd at all.
//!
//! Nothing here reads the environment except [`unit_dir`]: [`install`] and
//! [`uninstall`] take the directory to work in, which is what lets the tests
//! drive them against a temporary directory.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use opys_engine::error::{usage, Result};

use crate::registry;

/// The unit's file name. One name whichever binary installs it, so a second
/// install finds the first one instead of shadowing it.
pub const UNIT_NAME: &str = "opys-server.service";

/// The directory systemd creates as PID 1 and nothing else does — the canonical
/// `sd_booted()` test. Checked because "Linux" and "runs systemd" are different
/// facts: a container, WSL1, or an OpenRC distro has a config home and no
/// service manager to read a unit out of it.
const SYSTEMD_MARKER: &str = "/run/systemd/system";

/// Where user units live: `$XDG_CONFIG_HOME/systemd/user`, falling back to
/// `~/.config/systemd/user`.
///
/// `None` means "this machine has no place for a user unit" — a platform
/// without systemd, a Linux that is not systemd-booted, or an environment with
/// neither `XDG_CONFIG_HOME` nor `HOME` set. That is not a failure; it is the
/// branch where the CLI prints how to run the node by hand.
pub fn unit_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") || !Path::new(SYSTEMD_MARKER).is_dir() {
        return None;
    }
    registry::config_home()
        .ok()
        .map(|dir| dir.join("systemd").join("user"))
}

/// Where the unit file goes inside a unit directory.
pub fn unit_path(dir: &Path) -> PathBuf {
    dir.join(UNIT_NAME)
}

/// Whether something is already sitting at the unit path — a file, or a symlink
/// of any kind, including a dangling one.
pub fn is_installed(dir: &Path) -> bool {
    std::fs::symlink_metadata(unit_path(dir)).is_ok()
}

/// The unit file's contents, for `exe` listening on `bind` and serving `config`.
///
/// `config` is `Some` only when the user named an allowlist file explicitly:
/// baking the resolved default in would pin the service to a path it would have
/// found anyway, while a `--config` the unit did not carry would leave the
/// service listening on that file's port and serving a different file's
/// projects.
///
/// Pure — no environment, no filesystem — so the shape the user gets is pinned
/// by a test rather than by a live install.
pub fn unit_text(exe: &Path, bind: SocketAddr, config: Option<&Path>) -> String {
    let config = match config {
        Some(path) => format!(" --config {}", path.display()),
        None => String::new(),
    };
    format!(
        "[Unit]\n\
         Description=opys always-on node\n\
         Documentation=https://github.com/BohdanTkachenko/opys\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} web start --bind {bind}{config}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display()
    )
}

/// Write the unit into `dir`, returning where it landed.
///
/// Refuses to overwrite an existing unit unless `force`: both binaries install
/// the same file name, and an install that silently replaced a hand-tuned unit
/// would be the kind of surprise a service manager must never spring.
pub fn install(
    dir: &Path,
    exe: &Path,
    bind: SocketAddr,
    config: Option<&Path>,
    force: bool,
) -> Result<PathBuf> {
    let path = unit_path(dir);
    // `symlink_metadata`, not `exists()`: a *dangling* symlink at the unit path
    // does not "exist", and writing through it would land the unit wherever the
    // link points while this reported the unit path. A link of any kind counts
    // as already installed, and --force replaces the link rather than following
    // it into somebody's dotfile source.
    match std::fs::symlink_metadata(&path) {
        Ok(_) if !force => {
            return Err(usage(format!(
                "{} already exists — pass --force to overwrite it",
                path.display()
            )))
        }
        Ok(_) => {
            std::fs::remove_file(&path).map_err(|e| usage(format!("{}: {e}", path.display())))?
        }
        Err(_) => {}
    }
    // Every filesystem error here names the path it was reaching for: a bare
    // errno leaves the user guessing which of the unit dir, the unit file and
    // the allowlist was at fault.
    std::fs::create_dir_all(dir).map_err(|e| usage(format!("{}: {e}", dir.display())))?;
    std::fs::write(&path, unit_text(exe, bind, config))
        .map_err(|e| usage(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// Remove the unit from `dir`. `None` means there was nothing to remove, which
/// is a normal outcome rather than an error.
pub fn uninstall(dir: &Path) -> Result<Option<PathBuf>> {
    let path = unit_path(dir);
    if !is_installed(dir) {
        return Ok(None);
    }
    std::fs::remove_file(&path).map_err(|e| usage(format!("{}: {e}", path.display())))?;
    Ok(Some(path))
}

/// The path of the running executable, canonicalized.
///
/// Baked into `ExecStart`, so it has to be the real file rather than whatever
/// symlink the user happened to invoke: systemd starts the unit long after the
/// shell that ran `install` is gone.
pub fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind() -> SocketAddr {
        "127.0.0.1:6797".parse().unwrap()
    }

    #[test]
    fn unit_text_is_exactly_the_documented_shape() {
        let text = unit_text(Path::new("/home/u/.cargo/bin/opys"), bind(), None);
        assert_eq!(
            text,
            "[Unit]\n\
             Description=opys always-on node\n\
             Documentation=https://github.com/BohdanTkachenko/opys\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart=/home/u/.cargo/bin/opys web start --bind 127.0.0.1:6797\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        );
    }

    /// An install told which allowlist to read has to say so in `ExecStart`.
    /// Otherwise the service listens on the address that file asked for while
    /// serving the *default* file's projects — a live node, the right port, and
    /// silently the wrong set of documents.
    #[test]
    fn a_named_allowlist_reaches_exec_start() {
        let text = unit_text(
            Path::new("/bin/opys"),
            bind(),
            Some(Path::new("/home/u/alt/server.toml")),
        );
        assert!(
            text.contains(
                "ExecStart=/bin/opys web start --bind 127.0.0.1:6797 \
                 --config /home/u/alt/server.toml\n"
            ),
            "got: {text}"
        );
    }

    /// Whichever binary writes the unit, the `ExecStart` it writes is a command
    /// that binary can parse — `opys-server` mounts `web` too for this reason.
    #[test]
    fn exec_start_uses_the_web_start_form() {
        let text = unit_text(Path::new("/usr/bin/opys-server"), bind(), None);
        assert!(
            text.contains("ExecStart=/usr/bin/opys-server web start --bind 127.0.0.1:6797\n"),
            "got: {text}"
        );
    }

    #[test]
    fn install_writes_then_refuses_to_overwrite_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("systemd/user");
        let path = install(&dir, Path::new("/bin/opys"), bind(), None, false).unwrap();
        assert_eq!(path, dir.join(UNIT_NAME));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            unit_text(Path::new("/bin/opys"), bind(), None)
        );

        let err = install(&dir, Path::new("/bin/other"), bind(), None, false).unwrap_err();
        assert!(err.to_string().contains("--force"), "got: {err}");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("/bin/opys"),
            "the refused install must not have touched the file"
        );

        install(&dir, Path::new("/bin/other"), bind(), None, true).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("/bin/other"));
    }

    /// A symlink farm (stow, chezmoi, a rolled-back home-manager generation)
    /// can leave a dangling link at the unit path. It does not `exists()`, so an
    /// `exists()` guard would write *through* it — the unit landing in some
    /// other directory while the CLI printed the unit path.
    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_unit_path_counts_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("systemd/user");
        std::fs::create_dir_all(&dir).unwrap();
        let elsewhere = tmp.path().join("elsewhere/stray.service");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, unit_path(&dir)).unwrap();

        let err = install(&dir, Path::new("/bin/opys"), bind(), None, false).unwrap_err();
        assert!(err.to_string().contains("--force"), "got: {err}");
        assert!(!elsewhere.exists(), "nothing may be written through a link");

        // …and --force replaces the link rather than following it.
        install(&dir, Path::new("/bin/opys"), bind(), None, true).unwrap();
        assert!(!elsewhere.exists(), "the unit must land in the unit dir");
        assert!(std::fs::read_to_string(unit_path(&dir))
            .unwrap()
            .contains("ExecStart="));
    }

    #[test]
    fn uninstall_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("systemd/user");
        install(&dir, Path::new("/bin/opys"), bind(), None, false).unwrap();
        assert!(is_installed(&dir));
        assert_eq!(uninstall(&dir).unwrap(), Some(dir.join(UNIT_NAME)));
        assert_eq!(uninstall(&dir).unwrap(), None, "nothing left to remove");
        assert!(!is_installed(&dir));
    }

    /// A filesystem refusal has to name the file it was refused: `Permission
    /// denied (os error 13)` alone leaves three candidate paths in play.
    #[test]
    fn a_write_failure_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        // A *file* where the unit directory should be: create_dir_all fails.
        let dir = tmp.path().join("user");
        std::fs::write(&dir, "not a directory\n").unwrap();
        let err = install(&dir, Path::new("/bin/opys"), bind(), None, false).unwrap_err();
        assert!(
            err.to_string().contains(&dir.display().to_string()),
            "got: {err}"
        );
    }
}
