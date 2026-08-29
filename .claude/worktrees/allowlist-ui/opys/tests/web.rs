//! `opys web` — the always-on node's command surface (ADR-0077, TASK-0075).
//!
//! Everything here runs against a fake `$HOME` *and* a fake `$XDG_CONFIG_HOME`.
//! Both, always: `config_home` prefers XDG, so setting only `HOME` would leave
//! the developer's real `~/.config/opys/server.toml` — and their real systemd
//! unit directory — in the blast radius of a test run.

use std::path::PathBuf;
use std::process::Output;

use assert_cmd::Command;
use tempfile::TempDir;

/// A whole machine's worth of state, in a temporary directory: the home, the
/// allowlist file it implies, and the systemd user directory beside it.
struct Fx {
    _dir: TempDir,
    home: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, because the registry canonicalizes what it stores and the
        // tests compare the two.
        let home = std::fs::canonicalize(dir.path()).unwrap();
        Fx { _dir: dir, home }
    }

    /// `opys` pointed at this fixture's home.
    fn opys(&self) -> Command {
        let mut cmd = Command::cargo_bin("opys").unwrap();
        cmd.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"));
        cmd
    }

    /// A directory under the fake home holding an `opys.toml`.
    fn project(&self, rel: &str) -> PathBuf {
        let path = self.plain_dir(rel);
        std::fs::write(path.join("opys.toml"), "base = \"inventory\"\n").unwrap();
        path
    }

    /// A directory under the fake home that is not a project.
    fn plain_dir(&self, rel: &str) -> PathBuf {
        let path = self.home.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn config(&self) -> PathBuf {
        self.home.join(".config/opys/server.toml")
    }

    fn config_text(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap_or_default()
    }

    fn write_config(&self, text: &str) {
        std::fs::create_dir_all(self.config().parent().unwrap()).unwrap();
        std::fs::write(self.config(), text).unwrap();
    }

    fn unit(&self) -> PathBuf {
        self.home.join(".config/systemd/user/opys-server.service")
    }
}

/// Exit code plus both streams, as text.
struct Run {
    code: i32,
    out: String,
    err: String,
}

impl From<Output> for Run {
    fn from(o: Output) -> Run {
        Run {
            code: o.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&o.stdout).into_owned(),
            err: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }
}

impl Run {
    /// Succeeded, and said `needle` on stdout.
    fn ok_saying(&self, needle: &str) -> &Run {
        assert_eq!(
            self.code, 0,
            "expected success, got {}: {}",
            self.code, self.err
        );
        assert!(
            self.out.contains(needle),
            "expected {needle:?} in stdout, got:\n{}",
            self.out
        );
        self
    }

    /// Failed as a usage error (exit 2), and said `needle` on stderr.
    fn failed_with(&self, needle: &str) -> &Run {
        assert_eq!(
            self.code, 2,
            "expected exit 2, got {}: {}",
            self.code, self.out
        );
        assert!(
            self.err.contains(needle),
            "expected {needle:?} in stderr, got:\n{}",
            self.err
        );
        self
    }

    fn lacks(&self, needle: &str) -> &Run {
        assert!(
            !self.out.contains(needle),
            "did not expect {needle:?} in stdout, got:\n{}",
            self.out
        );
        self
    }
}

fn run(cmd: &mut Command) -> Run {
    cmd.output().unwrap().into()
}

// ---------------------------------------------------------------- the surface

#[test]
fn web_is_listed_in_the_top_level_help() {
    let fx = Fx::new();
    let r = run(fx.opys().arg("--help"));
    r.ok_saying("web");
    assert!(
        r.out.contains("The always-on node"),
        "the `web` line should say what it is, got:\n{}",
        r.out
    );
}

#[test]
fn web_help_lists_the_documented_subcommands() {
    let fx = Fx::new();
    let r = run(fx.opys().args(["web", "--help"]));
    for sub in [
        "start",
        "add",
        "remove",
        "list",
        "scan",
        "install",
        "uninstall",
    ] {
        r.ok_saying(sub);
    }
}

/// clap propagates the inventory globals into `web`, where they mean nothing.
/// An invocation that relies on them is refused, not quietly obeyed.
#[test]
fn the_inherited_inventory_globals_are_refused() {
    let fx = Fx::new();
    run(fx.opys().args(["--root", "/tmp", "web", "list"]))
        .failed_with("`--root` and `--no-sync` are inventory flags; `web` takes neither");
    run(fx.opys().args(["web", "list", "--no-sync"])).failed_with("`web` takes neither");

    // …and a plain `web` invocation says nothing.
    let r = run(fx.opys().args(["web", "list"]));
    assert!(r.err.is_empty(), "unexpected stderr:\n{}", r.err);
}

/// `--root` is the spelling the spec, the ADR and muscle memory all reach for,
/// and clap hands it to the global. Silently scanning `$HOME` instead would
/// print a confident, correct-looking scan of the wrong tree.
#[test]
fn scan_refuses_the_inventory_root_flag_and_names_the_right_one() {
    let fx = Fx::new();
    let projects = fx.plain_dir("Projects");
    fx.project("Projects/a");
    let elsewhere = fx.project("other/b");

    let r = run(fx.opys().args(["web", "scan", "--root"]).arg(&projects));
    r.failed_with("the scan root is `opys web scan --under <PATH>`");
    assert!(
        !r.out.contains(&elsewhere.display().to_string()),
        "the home tree must not have been walked:\n{}",
        r.out
    );
}

// -------------------------------------------------------------------- add

#[test]
fn add_allowlists_a_project_and_is_idempotent() {
    let fx = Fx::new();
    let proj = fx.project("Projects/thing");

    let r = run(fx.opys().args(["web", "add"]).arg(&proj));
    r.ok_saying(&format!("added {}", proj.display()));
    r.ok_saying("a running node picks this up within a minute");
    assert!(
        fx.config_text().contains("~/Projects/thing"),
        "the entry should be stored tilde-relative, got:\n{}",
        fx.config_text()
    );

    let before = fx.config_text();
    run(fx.opys().args(["web", "add"]).arg(&proj))
        .ok_saying(&format!("already allowlisted: {}", proj.display()));
    assert_eq!(fx.config_text(), before, "a second add must not rewrite it");
}

/// The registry stores canonical paths, so a relative or symlinked path must
/// resolve to the same entry rather than a duplicate one.
#[test]
fn add_canonicalizes_before_recording() {
    let fx = Fx::new();
    let proj = fx.project("Projects/thing");

    run(fx.opys().args(["web", "add", "."]).current_dir(&proj)).ok_saying("added");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("already allowlisted");
    assert_eq!(
        fx.config_text().matches("path =").count(),
        1,
        "one entry, not two:\n{}",
        fx.config_text()
    );
}

#[test]
fn add_rejects_a_directory_that_is_not_a_project() {
    let fx = Fx::new();
    let plain = fx.plain_dir("Projects/empty");
    run(fx.opys().args(["web", "add"]).arg(&plain)).failed_with("has no opys.toml");
    assert_eq!(fx.config_text(), "", "nothing should have been written");
}

#[test]
fn add_rejects_a_path_that_does_not_exist() {
    let fx = Fx::new();
    run(fx.opys().args(["web", "add"]).arg(fx.home.join("nope")))
        .failed_with("No such file or directory");
}

/// `Registry::add` would happily record a *file* as a prefix entry, which then
/// sits in the allowlist as a permanent error. Refuse while the user can still
/// see what they typed.
#[test]
fn add_rejects_a_file() {
    let fx = Fx::new();
    let file = fx.home.join("notes.md");
    std::fs::write(&file, "hi\n").unwrap();
    run(fx.opys().args(["web", "add", "--prefix"]).arg(&file)).failed_with("not a directory");
}

#[test]
fn add_prefix_takes_a_plain_directory_and_serves_what_is_under_it() {
    let fx = Fx::new();
    let work = fx.plain_dir("work");
    let nested = fx.project("work/a/inner");

    run(fx.opys().args(["web", "add", "--prefix"]).arg(&work)).ok_saying("added");
    assert!(
        fx.config_text().contains("[[prefix]]"),
        "{}",
        fx.config_text()
    );

    let r = run(fx.opys().args(["web", "list"]));
    r.ok_saying("prefix");
    r.ok_saying(&nested.display().to_string());
}

#[test]
fn add_declines_a_project_a_prefix_already_covers() {
    let fx = Fx::new();
    let work = fx.plain_dir("work");
    let nested = fx.project("work/a/inner");
    run(fx.opys().args(["web", "add", "--prefix"]).arg(&work)).ok_saying("added");

    let before = fx.config_text();
    run(fx.opys().args(["web", "add"]).arg(&nested))
        .ok_saying("already served by the prefix entry ~/work");
    assert_eq!(fx.config_text(), before, "no redundant entry");
}

// ------------------------------------------------------------------- remove

#[test]
fn remove_drops_an_entry_and_is_idempotent() {
    let fx = Fx::new();
    let proj = fx.project("Projects/thing");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("added");

    run(fx.opys().args(["web", "remove"]).arg(&proj))
        .ok_saying(&format!("removed {}", proj.display()));
    assert!(
        !fx.config_text().contains("[[project]]"),
        "the emptied key should go:\n{}",
        fx.config_text()
    );

    run(fx.opys().args(["web", "remove"]).arg(&proj))
        .ok_saying(&format!("not in the allowlist: {}", proj.display()));
}

/// A project served through a prefix has no entry of its own, so `remove` finds
/// nothing. Saying "not in the allowlist" would be a lie — it *is* served.
#[test]
fn remove_names_the_prefix_that_is_actually_responsible() {
    let fx = Fx::new();
    let work = fx.plain_dir("work");
    let nested = fx.project("work/a/inner");
    run(fx.opys().args(["web", "add", "--prefix"]).arg(&work)).ok_saying("added");

    let r = run(fx.opys().args(["web", "remove"]).arg(&nested));
    r.ok_saying("served by the prefix entry ~/work");
    r.ok_saying("opys web remove ~/work");
}

/// The allowlist stores `~/…`, `list` prints it and `remove` suggests it, so
/// typing it back has to work — including quoted, in a variable, or inside an
/// `sh -c` string, where the shell does not expand it for us.
#[test]
fn a_tilde_path_resolves_on_the_way_in_as_well_as_out() {
    let fx = Fx::new();
    fx.project("Projects/thing");
    run(fx.opys().args(["web", "add", "~/Projects/thing"])).ok_saying("added");
    assert!(
        fx.config_text().contains("~/Projects/thing"),
        "{}",
        fx.config_text()
    );

    run(fx.opys().args(["web", "remove", "~/Projects/thing"])).ok_saying("removed");
    assert!(
        !fx.config_text().contains("Projects/thing"),
        "{}",
        fx.config_text()
    );
}

/// A directory that has since been deleted is exactly the entry a user most
/// wants gone, so `remove` must not insist the path still resolves.
#[test]
fn remove_works_after_the_directory_is_gone() {
    let fx = Fx::new();
    let proj = fx.project("Projects/gone");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("added");
    std::fs::remove_dir_all(&proj).unwrap();

    run(fx.opys().args(["web", "remove"]).arg(&proj)).ok_saying("removed");
    assert!(
        !fx.config_text().contains("Projects/gone"),
        "{}",
        fx.config_text()
    );
}

// --------------------------------------------------------------------- list

#[test]
fn list_on_a_fresh_install_says_how_to_add_something() {
    let fx = Fx::new();
    let r = run(fx.opys().args(["web", "list"]));
    r.ok_saying(&format!("allowlist: {}", fx.config().display()));
    r.ok_saying("bind:      127.0.0.1:6797 (default)");
    r.ok_saying("nothing allowlisted — add a project with: opys web add <path>");
}

#[test]
fn list_shows_each_entry_and_what_it_expands_to() {
    let fx = Fx::new();
    let proj = fx.project("Projects/thing");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("added");

    let r = run(fx.opys().args(["web", "list"]));
    r.ok_saying("project  ~/Projects/thing");
    r.ok_saying(&format!("-> {}", proj.display()));
    r.ok_saying("serving 1 corpus in 1 project:");
    r.ok_saying(&format!("thing  {}", proj.display()));
}

#[test]
fn list_reports_a_bind_that_came_from_the_allowlist_file() {
    let fx = Fx::new();
    fx.write_config("bind = \"0.0.0.0:9999\"\n");
    run(fx.opys().args(["web", "list"]))
        .ok_saying("bind:      0.0.0.0:9999 (from the allowlist file)");
}

/// An entry whose directory vanished stays visible with the reason attached —
/// "you allowlisted this and it is gone" beats forgetting it ever existed.
#[test]
fn list_keeps_a_broken_entry_visible_with_its_reason() {
    let fx = Fx::new();
    let proj = fx.project("Projects/gone");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("added");
    std::fs::remove_dir_all(&proj).unwrap();

    let r = run(fx.opys().args(["web", "list"]));
    r.ok_saying("project  ~/Projects/gone");
    r.ok_saying("-> error:");
    r.ok_saying("serving nothing: no project matched those entries");
}

// --------------------------------------------------------------------- scan

#[test]
fn scan_marks_what_is_allowlisted_and_adds_nothing() {
    let fx = Fx::new();
    let known = fx.project("Projects/known");
    let unknown = fx.project("Projects/unknown");
    run(fx.opys().args(["web", "add"]).arg(&known)).ok_saying("added");
    let before = fx.config_text();

    let r = run(fx.opys().args(["web", "scan"]));
    r.ok_saying(&format!("scanning {} (depth 10)", fx.home.display()));
    r.ok_saying(&format!("{}  (allowlisted)", known.display()));
    r.ok_saying(&unknown.display().to_string());
    r.ok_saying("scan never adds anything — allowlist one with:");
    r.ok_saying(&format!("opys web add {}", unknown.display()));
    assert_eq!(fx.config_text(), before, "scan must never write");
}

#[test]
fn scan_says_when_everything_found_is_already_allowlisted() {
    let fx = Fx::new();
    let only = fx.project("Projects/only");
    run(fx.opys().args(["web", "add"]).arg(&only)).ok_saying("added");
    run(fx.opys().args(["web", "scan"])).ok_saying("every project found is already allowlisted");
}

#[test]
fn scan_reports_an_empty_tree_rather_than_nothing() {
    let fx = Fx::new();
    let empty = fx.plain_dir("empty");
    run(fx.opys().args(["web", "scan", "--under"]).arg(&empty))
        .ok_saying(&format!("no projects found under {}", empty.display()));
}

#[test]
fn scan_honours_the_depth_bound() {
    let fx = Fx::new();
    let deep = fx.project("tree/a/b/c/deep");
    let r = run(fx
        .opys()
        .args(["web", "scan", "--under"])
        .arg(&fx.home)
        .args(["--depth", "2"]));
    r.ok_saying("(depth 2)");
    r.lacks(&deep.display().to_string());
}

/// `--root .` resolves through the same canonicalization the registry used, so
/// an allowlisted project is not reported as new.
#[test]
fn scan_canonicalizes_its_root_before_comparing() {
    let fx = Fx::new();
    let proj = fx.project("Projects/thing");
    run(fx.opys().args(["web", "add"]).arg(&proj)).ok_saying("added");

    run(fx
        .opys()
        .args(["web", "scan", "--under", "."])
        .current_dir(fx.home.join("Projects")))
    .ok_saying("(allowlisted)");
}

#[test]
fn scan_rejects_a_root_that_is_not_there() {
    let fx = Fx::new();
    run(fx
        .opys()
        .args(["web", "scan", "--under"])
        .arg(fx.home.join("nope")))
    .failed_with("No such file or directory");
}

/// A file resolves, so the "does it exist" check passes and the "is it a
/// directory" one has to catch it — otherwise the walk quietly finds nothing.
#[test]
fn scan_rejects_a_root_that_is_a_file() {
    let fx = Fx::new();
    let file = fx.home.join("notes.md");
    std::fs::write(&file, "hi\n").unwrap();
    run(fx.opys().args(["web", "scan", "--under"]).arg(&file)).failed_with("not a directory");
}

/// Without `$HOME` the default root expands to a literal `~`, which would
/// silently walk a relative directory of that name and report nothing.
#[test]
fn scan_without_a_root_or_a_home_is_an_error() {
    let fx = Fx::new();
    run(fx.opys().args(["web", "scan"]).env_remove("HOME"))
        .failed_with("$HOME is not set — pass --under <path>");
}

// ------------------------------------------------------- a broken allowlist

#[test]
fn a_malformed_allowlist_stops_every_subcommand() {
    let fx = Fx::new();
    fx.write_config("this is not = = toml\n");
    let proj = fx.project("Projects/thing");

    run(fx.opys().args(["web", "list"])).failed_with("TOML parse error");
    run(fx.opys().args(["web", "scan"])).failed_with("TOML parse error");
    run(fx.opys().args(["web", "add"]).arg(&proj)).failed_with("TOML parse error");
    run(fx.opys().args(["web", "remove"]).arg(&proj)).failed_with("TOML parse error");
}

/// The user hand-edited `project` into something that is not a list of entries.
/// Refusing beats clobbering whatever they meant.
#[test]
fn an_allowlist_key_edited_into_the_wrong_shape_is_refused() {
    let fx = Fx::new();
    fx.write_config("project = 7\n");
    let proj = fx.project("Projects/thing");
    run(fx.opys().args(["web", "add"]).arg(&proj))
        .failed_with("`project` is not a list of entries");
}

/// An entry this version cannot read is one the user asked for and would not
/// get. Dropping it silently makes `web list` say "nothing allowlisted" about a
/// file that plainly names a project — and leaves the typo in place forever,
/// because nothing ever complains about it.
#[test]
fn an_entry_of_the_wrong_shape_is_refused_rather_than_dropped() {
    let fx = Fx::new();

    fx.write_config("[project]\npath = \"~/Projects/alpha\"\n");
    run(fx.opys().args(["web", "list"])).failed_with("`project` is not a list of entries");

    fx.write_config("[[project]]\ndir = \"~/Projects/alpha\"\n");
    run(fx.opys().args(["web", "list"])).failed_with("a `project` entry has no `path` string");

    // A depth this version cannot read must not quietly widen to the default.
    fx.write_config("[[prefix]]\npath = \"~/work\"\ndepth = -3\n");
    run(fx.opys().args(["web", "list"])).failed_with("not a non-negative integer");
}

#[test]
fn an_unparseable_bind_is_fatal_rather_than_ignored() {
    let fx = Fx::new();
    fx.write_config("bind = \"not-an-address\"\n");
    run(fx.opys().args(["web", "list"])).failed_with("is not an address");
}

/// With neither variable set there is no default allowlist path to fall back
/// on, so the command has to say that rather than invent one.
#[test]
fn no_config_home_at_all_is_an_error() {
    let fx = Fx::new();
    run(fx
        .opys()
        .args(["web", "list"])
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME"))
    .failed_with("neither XDG_CONFIG_HOME nor HOME is set");
}

// -------------------------------------------------------------------- start

/// `web start` is wired to the real serve path, and a bind it cannot have is a
/// hard failure rather than a silent fallback to some other port.
#[test]
fn start_reports_a_port_it_cannot_have() {
    let fx = Fx::new();
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = held.local_addr().unwrap();

    let r = run(fx
        .opys()
        .args(["web", "start", "--bind"])
        .arg(addr.to_string())
        .timeout(std::time::Duration::from_secs(60)));
    r.failed_with(&format!("cannot bind {addr}"));
}

// ------------------------------------------------------------------ systemd

/// Whether this machine has the user manager the install path targets — the
/// same `sd_booted` test `systemd::unit_dir` makes. In a container, on WSL1 or
/// on a non-systemd distro there is nowhere to install a unit, and `install`
/// takes the manual-instructions branch these tests are not about.
#[cfg(target_os = "linux")]
fn systemd_booted() -> bool {
    std::path::Path::new("/run/systemd/system").is_dir()
}

/// The unit's exact shape, and the fact that installing it runs nothing.
#[cfg(target_os = "linux")]
#[test]
fn install_writes_the_unit_and_prints_the_enable_commands() {
    if !systemd_booted() {
        return;
    }
    let fx = Fx::new();
    let r = run(fx.opys().args(["web", "install"]));
    r.ok_saying(&format!("wrote {}", fx.unit().display()));
    r.ok_saying("systemctl --user daemon-reload && systemctl --user enable --now opys-server");
    r.ok_saying("http://127.0.0.1:6797");

    let text = std::fs::read_to_string(fx.unit()).unwrap();
    let exe = std::fs::canonicalize(assert_cmd::cargo::cargo_bin("opys")).unwrap();
    assert_eq!(
        text,
        format!(
            "[Unit]\n\
             Description=opys always-on node\n\
             Documentation=https://github.com/BohdanTkachenko/opys\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={} web start --bind 127.0.0.1:6797\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exe.display()
        )
    );
}

#[cfg(target_os = "linux")]
#[test]
fn install_takes_the_bind_from_the_flag_then_the_allowlist_file() {
    if !systemd_booted() {
        return;
    }
    let fx = Fx::new();
    fx.write_config("bind = \"0.0.0.0:9999\"\n");
    run(fx.opys().args(["web", "install"])).ok_saying("wrote");
    assert!(
        std::fs::read_to_string(fx.unit())
            .unwrap()
            .contains("--bind 0.0.0.0:9999\n"),
        "the file's bind should reach the unit"
    );

    run(fx
        .opys()
        .args(["web", "install", "--force", "--bind", "127.0.0.1:1234"]))
    .ok_saying("wrote");
    assert!(std::fs::read_to_string(fx.unit())
        .unwrap()
        .contains("--bind 127.0.0.1:1234\n"));
}

/// An install told which allowlist to read has to say so in `ExecStart`, or the
/// service listens on the address that file asked for while serving the
/// *default* file's projects: a live node, the right port, silently the wrong
/// documents. Pinned by running the unit's own command line.
#[cfg(target_os = "linux")]
#[test]
fn a_named_allowlist_reaches_the_unit_and_the_unit_runs() {
    if !systemd_booted() {
        return;
    }
    let fx = Fx::new();
    let proj = fx.project("work/notes");
    let alt = fx.home.join("alt/server.toml");
    std::fs::create_dir_all(alt.parent().unwrap()).unwrap();
    // A port already taken, so the node this unit starts fails fast at the bind
    // rather than running forever — proof it got that far with these arguments.
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = held.local_addr().unwrap();
    std::fs::write(
        &alt,
        format!("bind = \"{addr}\"\n\n[[project]]\npath = {:?}\n", proj),
    )
    .unwrap();

    let r = run(fx.opys().args(["web", "install", "--config"]).arg(&alt));
    r.ok_saying(&format!("the node will listen on http://{addr}"));

    let unit = std::fs::read_to_string(fx.unit()).unwrap();
    let exec = unit
        .lines()
        .find_map(|l| l.strip_prefix("ExecStart="))
        .expect("the unit has an ExecStart");
    assert_eq!(
        exec,
        format!(
            "{} web start --bind {addr} --config {}",
            std::fs::canonicalize(assert_cmd::cargo::cargo_bin("opys"))
                .unwrap()
                .display(),
            alt.display()
        )
    );

    // A relative `--config` is absolutized on the way in: systemd starts the
    // unit with the user's home as the working directory, not this shell's.
    run(fx
        .opys()
        .args(["web", "install", "--force", "--config", "alt/server.toml"])
        .current_dir(&fx.home))
    .ok_saying("wrote");
    assert!(
        std::fs::read_to_string(fx.unit())
            .unwrap()
            .contains(&format!("--config {}", alt.display())),
        "a relative --config must reach the unit as an absolute path"
    );

    // …and that exact command line is one the binary accepts.
    let argv: Vec<&str> = exec.split(' ').collect();
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..])
        .env("HOME", &fx.home)
        .env("XDG_CONFIG_HOME", fx.home.join(".config"))
        .timeout(std::time::Duration::from_secs(60));
    run(&mut cmd).failed_with(&format!("cannot bind {addr}"));
}

#[cfg(target_os = "linux")]
#[test]
fn install_refuses_to_overwrite_without_force() {
    if !systemd_booted() {
        return;
    }
    let fx = Fx::new();
    run(fx.opys().args(["web", "install"])).ok_saying("wrote");
    std::fs::write(fx.unit(), "# hand-tuned\n").unwrap();

    run(fx.opys().args(["web", "install"])).failed_with("already exists — pass --force");
    assert_eq!(
        std::fs::read_to_string(fx.unit()).unwrap(),
        "# hand-tuned\n",
        "the refused install must not have touched the file"
    );

    run(fx.opys().args(["web", "install", "--force"])).ok_saying("wrote");
    assert!(std::fs::read_to_string(fx.unit())
        .unwrap()
        .contains("ExecStart="));
}

#[cfg(target_os = "linux")]
#[test]
fn uninstall_removes_the_unit_and_prints_the_disable_commands() {
    if !systemd_booted() {
        return;
    }
    let fx = Fx::new();
    run(fx.opys().args(["web", "install"])).ok_saying("wrote");

    let r = run(fx.opys().args(["web", "uninstall"]));
    r.ok_saying(&format!("removed {}", fx.unit().display()));
    r.ok_saying("systemctl --user disable --now opys-server && systemctl --user daemon-reload");
    // Deleting the unit does not stop the service, so the command that does has
    // to come first — the order the user has to execute in.
    let disable = r.out.find("systemctl --user disable").unwrap();
    let removed = r.out.find("removed ").unwrap();
    assert!(
        disable < removed,
        "stop-first must be printed before the removal line, got:\n{}",
        r.out
    );
    assert!(!fx.unit().exists());

    run(fx.opys().args(["web", "uninstall"]))
        .ok_saying(&format!("no unit at {}", fx.unit().display()));
}

/// No place for a user unit is a fact about the machine, not a mistake the user
/// made: say how to run the node by hand, and succeed.
///
/// No `--config` here on purpose: with neither `$HOME` nor `$XDG_CONFIG_HOME`
/// there is no default allowlist path either, and resolving one before looking
/// for a unit directory would turn the documented exit-0 branch into `error:
/// neither XDG_CONFIG_HOME nor HOME is set`.
#[test]
fn install_without_a_systemd_directory_explains_and_exits_zero() {
    let fx = Fx::new();
    let r = run(fx
        .opys()
        .args(["web", "install"])
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME"));
    r.ok_saying("no systemd user unit directory here — nothing was installed");
    r.ok_saying("web start --bind 127.0.0.1:6797");
    assert!(!fx.unit().exists(), "nothing should have been written");
}

/// …and the command it prints is one that would serve what `install` just
/// reported on, rather than a line that reads right and runs against a
/// different allowlist.
#[test]
fn the_manual_command_carries_the_allowlist_it_reported_on() {
    let fx = Fx::new();
    let config = fx.home.join("server.toml");
    std::fs::write(&config, "bind = \"127.0.0.1:7777\"\n").unwrap();
    let r = run(fx
        .opys()
        .args(["web", "install", "--config"])
        .arg(&config)
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME"));
    r.ok_saying(&format!(
        "web start --bind 127.0.0.1:7777 --config {}",
        config.display()
    ));
}

#[test]
fn uninstall_without_a_systemd_directory_exits_zero() {
    let fx = Fx::new();
    run(fx
        .opys()
        .args(["web", "uninstall"])
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME"))
    .ok_saying("no systemd user unit directory here — there is nothing to remove");
}
