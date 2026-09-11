//! Reading the inventory out of git, in-process — the branch reader of
//! [ADR-0102]: main is truth, every other ref is *progress*.
//!
//! One module answers three questions, all from git objects and never from a
//! spawned `git`: what a document's lifecycle looked like across commits (`opys
//! history <id>`, and the migration seed behind `--seed`), which refs hold the
//! inventory at all ([`refs`]), and which documents those refs hold at a status
//! main does not ([`occupancy`]).
//!
//! The lookup that makes all three cheap is the same one `opys history` has
//! always used: an opys document's **filename is its id** and never changes when
//! a status change relocates the file (into `_archived/`, say), so blobs are
//! found and diffed by *basename* within the inventory base subtree — exact,
//! threshold-free, and oblivious to where under the base opys put the file. A
//! path-keyed diff would report every relocation as a delete plus an add.
//!
//! Nothing here writes to the repository, and nothing here fails a command: a
//! project that is not a git repository — or a build without the `git` feature —
//! returns empty results plus exactly one notice for the caller to print once.
//! That is the degradation ADR-0102 asks for, and it is why the data types below
//! are defined unconditionally while only their gix-backed implementations sit
//! behind the feature.
//!
//! Not to be confused with [`crate::history`], the *lifecycle map* stored in
//! frontmatter. This module reconstructs history from commits; that one records
//! it as documents are written.
//!
//! [ADR-0102]: https://github.com/BohdanTkachenko/opys

use std::collections::BTreeMap;

use crate::doc::Doc;

#[cfg(feature = "git")]
mod reader;
#[cfg(feature = "git")]
pub use reader::{docs_changed_vs_main, occupancy, open, refs, timeline, truth_branch, Rev};

// Test support, not public API — hence both the `#[doc(hidden)]` and the gate.
// The module compiles into the engine's own test target, and into a consumer
// that explicitly asks for `test-fixtures` (only the `opys` binary's
// *dev*-dependency does, for `opys/tests/cli.rs`). A release build, a published
// crate and docs.rs therefore never carry it, so nothing here is semver surface.
#[cfg(all(feature = "git", any(test, feature = "test-fixtures")))]
#[doc(hidden)]
pub mod fixture;

#[cfg(all(test, feature = "git"))]
mod tests;

/// The notice a project that is not a git repository yields, once.
pub const NOT_A_REPOSITORY: &str =
    "not a git repository — branches are invisible; showing main only";

/// The notice a repository with no truth branch yields, once: with nothing to
/// compare against, every ref would look like the whole inventory.
pub const NO_TRUTH_BRANCH: &str =
    "no truth branch (`[git] main`, or `master`) — branches cannot be compared; \
     showing main only";

/// The notice a build without the `git` feature yields, once.
pub const NO_GIT_FEATURE: &str =
    "built without the `git` feature — branches are invisible; showing main only";

/// What `opys history` answers with in a build without the `git` feature.
///
/// The *reader* degrades to [`NO_GIT_FEATURE`]'s notice because it still has a
/// main-only answer to give. A command whose entire subject is the git timeline
/// has none: printing "no committed history found" would claim it looked. So it
/// names the missing feature and exits 2 — which is also why the subcommand
/// stays in the parser rather than being compiled out, since clap's
/// "unrecognized subcommand 'history'" never mentions the build at all.
pub const NO_GIT_FEATURE_COMMAND: &str =
    "built without the `git` cargo feature — `opys history` reads the git timeline, \
     and this binary cannot; rebuild without `--no-default-features`";

/// A commit that holds the inventory, and what to call it.
///
/// One `Ref` per *commit*, not per ref name: a jj-colocated worktree detaches
/// HEAD, so the same commit is routinely reachable both as `refs/heads/x` and as
/// a worktree head, and reporting it twice would double-count the work in
/// flight. [`refs`] merges them and picks the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// The fully qualified ref name (`refs/heads/feat-x`) when a ref points at
    /// the commit, else `worktrees/<checkout directory>` for a worktree head
    /// that no ref covers — the directory the worktree's `gitdir` file points
    /// at, which is what a human calls that worktree and what [`Ref::label`]
    /// uses, *not* its admin entry under `<git-dir>/worktrees/` (git lets the
    /// two differ). A commit carries one `name`, so when several worktrees are
    /// parked on it this is the first in sorted order; `label` names them all,
    /// and is the string to show.
    pub name: String,
    /// The commit id, full hex.
    pub commit: String,
    /// What a column of this ref is called: the branch name (its category
    /// prefix stripped) when a branch points at the commit, else the checkout
    /// directory names of the worktrees parked on it, sorted and joined.
    pub label: String,
}

/// What one ref holds that main does not.
///
/// `deleted` carries the ids whose document exists under the base on main and
/// not on the ref. It is reported rather than dropped — a caller building a
/// commit-level view wants it — but [`occupancy`] ignores it: a document a
/// branch deleted is not *occupied* by that branch, and treating a deletion as
/// occupancy would hold an id against a branch that has nothing to say about it.
#[derive(Debug, Clone, Default)]
pub struct Changed {
    pub docs: Vec<Doc>,
    pub deleted: Vec<String>,
}

/// One ref's claim on a document: which column, at what status, since when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupant {
    /// The [`Ref::label`] of the ref holding it.
    pub label: String,
    /// The status the document has on that ref.
    pub status: String,
    /// The ref copy's last lifecycle key, when it carries a readable history
    /// map — when the work in flight last moved.
    pub since: Option<String>,
}

/// Which refs hold which documents at a status main does not.
///
/// `notices` is what the caller prints, once, above whatever it renders: the
/// reader degrades rather than failing, so an unreadable repository (or a build
/// without the feature) shows up here and nowhere else.
#[derive(Debug, Clone, Default)]
pub struct Occupancy {
    pub by_doc: BTreeMap<String, Vec<Occupant>>,
    pub notices: Vec<String>,
}

/// Main-only fallback for a build without the `git` feature: no occupancy, and
/// exactly one notice saying why.
#[cfg(not(feature = "git"))]
pub fn occupancy(_prj: &crate::project::Project, _on_main: &[Doc]) -> Occupancy {
    Occupancy {
        by_doc: BTreeMap::new(),
        notices: vec![NO_GIT_FEATURE.to_string()],
    }
}
