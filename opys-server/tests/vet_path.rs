//! The browser-facing path rules (ADR-0082, FEAT-0083).
//!
//! This is the entire security boundary for UI-driven allowlisting, so it is
//! tested as one: every rule, and every way each rule has of being wrong.

use std::path::Path;

use opys_server::registry::vet_ui_path;

/// Run `f` with `HOME` set to `home`. The tests are serialized by a mutex
/// because `set_var` is process-wide.
fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", home) };
    let out = f();
    match prev {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    out
}

fn home() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("Projects/thing")).unwrap();
    t
}

#[test]
fn an_ordinary_project_under_home_is_accepted() {
    let h = home();
    with_home(h.path(), || {
        let got = vet_ui_path(&h.path().join("Projects/thing").display().to_string()).unwrap();
        assert!(got.ends_with("Projects/thing"), "{got:?}");
        // `~` is expanded, so the browser can send the short form.
        assert!(vet_ui_path("~/Projects/thing").is_ok());
    });
}

#[test]
fn a_path_outside_home_is_refused() {
    let h = home();
    let outside = tempfile::tempdir().unwrap();
    with_home(h.path(), || {
        let err = vet_ui_path(&outside.path().display().to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside your home directory"), "{err}");
        // And it says how to do it anyway, since the file remains the escape hatch.
        assert!(err.contains("allowlist file"), "{err}");
    });
}

/// The rule the lexical check alone would miss: a link whose *name* is innocent.
#[test]
fn a_symlink_inside_home_that_lands_outside_is_refused() {
    let h = home();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), h.path().join("Projects/escape")).unwrap();
    with_home(h.path(), || {
        let err = vet_ui_path(&h.path().join("Projects/escape").display().to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside your home directory"), "{err}");
    });
}

/// `..` is resolved before comparison, so climbing out fails on where it lands.
#[test]
fn dot_dot_that_climbs_out_is_refused() {
    let h = home();
    with_home(h.path(), || {
        let raw = h.path().join("Projects/../../..").display().to_string();
        let err = vet_ui_path(&raw).unwrap_err().to_string();
        assert!(err.contains("outside your home directory"), "{err}");
    });
}

#[test]
fn hidden_directories_are_refused_at_any_depth() {
    let h = home();
    std::fs::create_dir_all(h.path().join(".ssh")).unwrap();
    std::fs::create_dir_all(h.path().join("Projects/.secret/inner")).unwrap();
    with_home(h.path(), || {
        for p in [
            h.path().join(".ssh"),
            h.path().join("Projects/.secret"),
            // Not just the leaf: a hidden ancestor counts.
            h.path().join("Projects/.secret/inner"),
        ] {
            let err = vet_ui_path(&p.display().to_string())
                .unwrap_err()
                .to_string();
            assert!(err.contains("hidden directory"), "{p:?}: {err}");
        }
    });
}

/// A home directory that is itself reached through a symlink must not reject
/// everything: both sides are canonicalized before comparing.
#[test]
fn a_symlinked_home_still_accepts_its_own_children() {
    let real = home();
    let link_parent = tempfile::tempdir().unwrap();
    let linked_home = link_parent.path().join("home-link");
    std::os::unix::fs::symlink(real.path(), &linked_home).unwrap();
    with_home(&linked_home, || {
        assert!(
            vet_ui_path(&linked_home.join("Projects/thing").display().to_string()).is_ok(),
            "a symlinked HOME must not reject its own children"
        );
    });
}

#[test]
fn nonsense_is_refused_without_a_panic() {
    let h = home();
    with_home(h.path(), || {
        for raw in ["", "   ", "~/does-not-exist", "/nope/nope"] {
            assert!(vet_ui_path(raw).is_err(), "{raw:?} should be refused");
        }
        // A file is not a project directory.
        let f = h.path().join("Projects/file.txt");
        std::fs::write(&f, "x").unwrap();
        let err = vet_ui_path(&f.display().to_string())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
    });
}
