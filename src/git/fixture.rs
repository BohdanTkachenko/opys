//! Test support: build a git repository through gix's write API alone.
//!
//! Nothing here spawns `git`, on purpose and not only for speed. The cases this
//! reader exists for cannot be produced by `git` on every machine anyway — a
//! **detached** worktree head is what a jj-colocated checkout leaves behind —
//! and a fixture that shelled out would inherit the developer's `init.
//! defaultBranch`, identity and `safe.directory`, making the same test pass here
//! and fail there. Every repository built here is isolated from user and system
//! config, names its branches explicitly, and stamps deterministic commit times.
//!
//! It lives in the library (rather than beside one test file) because the
//! engine's own tests, the binary's integration tests and the node's ref-reader
//! tests all build the same repositories. It is no part of the public contract: the declaration in
//! [`super`] carries `#[doc(hidden)]` and compiles only into the engine's own
//! test target or under the non-default `test-fixtures` feature, which nothing
//! but a dev-dependency turns on — so a release build, the published crate and
//! docs.rs never see it, and nothing here is covered by semver.

use std::path::{Path, PathBuf};

use gix::objs::tree::{Entry, EntryKind};
use gix::ObjectId;

/// The commit time of a fixture's first commit: `2026-02-01T00:00:00Z`, chosen
/// to sit *after* the pinned `OPYS_NOW` the integration tests use, so a seeded
/// timeline reads as commits that happened after the document was created
/// rather than as a pile of one-microsecond bumps.
pub const EPOCH: i64 = 1_769_904_000;

/// Seconds between consecutive fixture commits.
pub const STRIDE: i64 = 60;

/// The commit time of the `n`-th commit (0-based) of a fixture.
pub fn commit_secs(n: usize) -> i64 {
    EPOCH + STRIDE * n as i64
}

/// The lifecycle key `history --seed` writes for the `n`-th commit — the same
/// instant in the canonical spelling, so a test can assert the map exactly.
pub fn commit_key(n: usize) -> String {
    crate::history::format_key(
        time::OffsetDateTime::from_unix_timestamp(commit_secs(n)).expect("fixture time is valid"),
    )
}

/// The 8-hex abbreviation `opys history` prints for a commit — so a test can
/// pin the command's output line for line without naming gix itself.
pub fn short(oid: ObjectId) -> String {
    oid.to_hex_with_len(8).to_string()
}

/// A repository under construction.
pub struct Fixture {
    pub dir: PathBuf,
    pub repo: gix::Repository,
    commits: usize,
}

impl Fixture {
    /// Initialize a repository with a work tree at `dir`, which may already hold
    /// files (an `opys.toml` and an inventory, typically). Opened with
    /// [`gix::open::Options::isolated`], so no user, system or environment git
    /// configuration can reach the test.
    pub fn init(dir: impl AsRef<Path>) -> Fixture {
        let dir = dir.as_ref().to_path_buf();
        let repo = gix::ThreadSafeRepository::init_opts(
            &dir,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::isolated(),
        )
        .expect("init a fixture repository")
        .to_thread_local();
        Fixture {
            dir,
            repo,
            commits: 0,
        }
    }

    /// Write a file into the work tree, creating parent directories.
    pub fn write(&self, relpath: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
        let path = self.dir.join(relpath);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture dirs");
        }
        std::fs::write(path, contents).expect("write a fixture file");
    }

    /// Remove a file from the work tree.
    pub fn remove(&self, relpath: impl AsRef<Path>) {
        std::fs::remove_file(self.dir.join(relpath)).expect("remove a fixture file");
    }

    /// The commit a ref points at, if it exists.
    pub fn tip(&self, full_ref: &str) -> Option<ObjectId> {
        self.repo
            .try_find_reference(full_ref)
            .ok()
            .flatten()
            .and_then(|mut r| r.peel_to_id().ok())
            .map(|id| id.detach())
    }

    /// Snapshot the whole work tree onto `refs/heads/main`, as `git commit -a`
    /// would, with the current tip as its parent.
    pub fn commit(&mut self, author: &str, message: &str) -> ObjectId {
        let parents: Vec<ObjectId> = self.tip("refs/heads/main").into_iter().collect();
        self.commit_to("refs/heads/main", &parents, author, message)
    }

    /// Snapshot the whole work tree onto an arbitrary ref with explicit parents
    /// — how a branch that has no worktree, or a remote-tracking ref, is made.
    pub fn commit_to(
        &mut self,
        full_ref: &str,
        parents: &[ObjectId],
        author: &str,
        message: &str,
    ) -> ObjectId {
        let tree = self.write_tree(&self.dir);
        // A raw git time string, so commits are reproducible to the second
        // without touching the clock or the environment.
        let when = format!("{} +0000", commit_secs(self.commits));
        let email = format!("{author}@example.com");
        let sig = gix::actor::SignatureRef {
            name: author.into(),
            email: email.as_str().into(),
            time: when.as_str(),
        };
        let id = self
            .repo
            .commit_as(sig, sig, full_ref, message, tree, parents.to_vec())
            .expect("write a fixture commit")
            .detach();
        self.commits += 1;
        id
    }

    /// Point a ref at a commit (a branch with no worktree, a tag, a
    /// remote-tracking ref).
    pub fn set_ref(&self, full_ref: &str, oid: ObjectId) {
        self.repo
            .reference(
                full_ref,
                oid,
                gix::refs::transaction::PreviousValue::Any,
                "fixture",
            )
            .expect("write a fixture ref");
    }

    /// Point the **main** worktree's `HEAD` at a ref, as checking out a branch
    /// does. [`Fixture::commit`] still commits to `refs/heads/main`, so this is
    /// how a fixture whose main worktree sits somewhere other than the truth
    /// branch is built.
    pub fn set_head(&self, full_ref: &str) {
        std::fs::write(
            self.repo.common_dir().join("HEAD"),
            format!("ref: {full_ref}\n"),
        )
        .expect("write the main HEAD");
    }

    /// Detach the **main** worktree's `HEAD` onto a commit — what a
    /// jj-colocated checkout leaves behind in every workspace it manages.
    pub fn detach_head(&self, oid: ObjectId) {
        std::fs::write(self.repo.common_dir().join("HEAD"), format!("{oid}\n"))
            .expect("write the main HEAD");
    }

    /// A linked worktree with a **detached** head — the jj-colocated shape.
    /// `name` is the entry under `<git-dir>/worktrees/`; `checkout` is the
    /// directory the worktree claims (its name is what a column is labelled by).
    /// Both administrative files are written because gix lists a worktree only
    /// when its `gitdir` file exists.
    pub fn worktree_detached(&self, name: &str, checkout: impl AsRef<Path>, oid: ObjectId) {
        self.worktree_files(name, checkout.as_ref(), &format!("{oid}\n"));
    }

    /// A linked worktree whose head is **symbolic** — what plain `git worktree
    /// add` writes.
    pub fn worktree_on_ref(&self, name: &str, checkout: impl AsRef<Path>, full_ref: &str) {
        self.worktree_files(name, checkout.as_ref(), &format!("ref: {full_ref}\n"));
    }

    fn worktree_files(&self, name: &str, checkout: &Path, head: &str) {
        let admin = self.repo.common_dir().join("worktrees").join(name);
        std::fs::create_dir_all(&admin).expect("create the worktree admin dir");
        std::fs::write(admin.join("HEAD"), head).expect("write the worktree HEAD");
        std::fs::write(
            admin.join("gitdir"),
            format!("{}\n", checkout.join(".git").display()),
        )
        .expect("write the worktree gitdir");
    }

    /// Recursively write `dir` as a tree object, skipping `.git`. Empty
    /// directories are skipped, as git does.
    fn write_tree(&self, dir: &Path) -> ObjectId {
        let mut entries: Vec<Entry> = Vec::new();
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("read a fixture dir")
            .map(|e| e.expect("read a fixture dir entry").path())
            .collect();
        names.sort();
        for path in names {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == ".git" {
                continue;
            }
            if path.is_dir() {
                let oid = self.write_tree(&path);
                // `git` has no concept of an empty directory; writing one would
                // make the tree differ from what a real checkout produces.
                if oid == empty_tree(&self.repo) {
                    continue;
                }
                entries.push(Entry {
                    mode: EntryKind::Tree.into(),
                    filename: name.into(),
                    oid,
                });
            } else if path.is_file() {
                let data = std::fs::read(&path).expect("read a fixture file");
                let oid = self.repo.write_blob(data).expect("write a blob").detach();
                entries.push(Entry {
                    mode: EntryKind::Blob.into(),
                    filename: name.into(),
                    oid,
                });
            }
        }
        // `gix_object::Tree::write_to` asserts its entries are in git's tree
        // order (directories compare as if they ended in `/`), and a debug build
        // *panics* rather than erroring. `Entry`'s `Ord` is that order.
        entries.sort();
        self.repo
            .write_object(&gix::objs::Tree { entries })
            .expect("write a tree")
            .detach()
    }
}

/// The id of the empty tree, written once so it can be compared against.
fn empty_tree(repo: &gix::Repository) -> ObjectId {
    repo.write_object(&gix::objs::Tree { entries: vec![] })
        .expect("write the empty tree")
        .detach()
}
