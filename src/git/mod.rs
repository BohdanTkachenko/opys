//! Reading the inventory out of git, in-process — the branch reader of
//! [ADR-0053]: main is truth, every other ref is *progress*.
//!
//! One module answers three questions, all from git objects and never from a
//! spawned `git`: what a document's lifecycle looked like across commits (`opys
//! history <id>`, and the migration seed behind `--seed`), which refs hold the
//! inventory at all ([`refs`]), and which documents those refs hold at a status
//! main does not ([`occupancy`]).
//!
//! [`survey`] is the one-pass form of the last two, for a long-lived caller: it
//! returns every ref with its diff *and* its occupancy off a single walk, plus
//! the git directory to watch for the next change. The node's ref reader is
//! built on it, so the dashboard's branch columns and the CLI's occupancy come
//! from the same code rather than from two readers that can disagree.
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
//! That is the degradation ADR-0053 asks for, and it is why the data types below
//! are defined unconditionally while only their gix-backed implementations sit
//! behind the feature.
//!
//! Not to be confused with [`crate::history`], the *lifecycle map* stored in
//! frontmatter. This module reconstructs history from commits; that one records
//! it as documents are written.
//!
//! [ADR-0053]: https://github.com/BohdanTkachenko/opys

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::doc::Doc;

#[cfg(feature = "git")]
mod reader;
#[cfg(feature = "git")]
pub use reader::{
    docs_changed_vs_main, occupancy, open, refs, survey, survey_with, timeline, truth_branch, Rev,
};

// Test support, not public API — hence both the `#[doc(hidden)]` and the gate.
// The module compiles into the engine's own test target, and into a consumer
// that explicitly asks for `test-fixtures` — only two *dev*-dependencies do:
// the `opys` binary's, for `opys/tests/cli.rs`, and `opys-server`'s, for
// `opys-server/tests/ref_reader.rs`. A release build, a published crate and
// docs.rs therefore never carry it, so nothing here is semver surface.
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
    /// The checkout directories of the linked worktrees parked on this commit,
    /// sorted; empty when no worktree is.
    ///
    /// Kept rather than folded into [`Ref::label`] because "is this ref already
    /// checked out somewhere" is not answerable from the label: a branch that a
    /// worktree sits on is labelled by the branch, byte-identically to one that
    /// no worktree touches. The node's ref reader needs exactly that
    /// distinction — it adds a union column per ref that has *no* checkout,
    /// since a checkout is already a corpus column of its own.
    pub worktrees: Vec<PathBuf>,
}

/// One checkout of a repository: where it is, and what its `HEAD` names.
///
/// The list [`survey`] returns includes the repository's **main** worktree,
/// which `gix::Repository::worktrees` deliberately does not. A caller asking
/// "which of the directories I serve is sitting on this commit" has to be able
/// to answer it for the main one too — it is the usual answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The work directory. A served corpus is this directory, or something
    /// under it when the project is nested inside the repository.
    pub root: PathBuf,
    /// The branch this checkout is on, shortened the way [`Ref::label`] is
    /// (`main`).
    ///
    /// The branch `HEAD` names when `HEAD` is symbolic, and otherwise the
    /// branch whose tip the detached head is parked on — because a detached
    /// head is *every* checkout of a jj-colocated repository, and answering
    /// `None` for all of them would say nothing about the one kind of
    /// repository this reader mostly runs against. `None` only when the commit
    /// is no branch's tip (a genuine detached checkout), or when `HEAD` could
    /// not be read at all.
    pub branch: Option<String>,
    /// The commit `HEAD` resolves to, full hex. `None` for an unborn head, or
    /// one that cannot be read.
    pub commit: Option<String>,
}

/// The truth branch, as [`survey_with`]'s caller is handed it: main is the
/// baseline every other ref is measured against (ADR-0053).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truth {
    /// Fully qualified: `refs/heads/main`.
    pub name: String,
    /// Short, as a column would be labelled: `main`.
    pub short: String,
    /// The commit it points at, full hex.
    pub commit: String,
}

impl Checkout {
    /// Whether this checkout is sitting on the truth branch.
    ///
    /// By name when `HEAD` is symbolic, and by **commit** when it is not: a
    /// jj-colocated repository detaches every head, and a detached head parked
    /// on main's commit is a checkout of main by every meaning that matters to
    /// a caller looking for the working copy to measure branches against.
    pub fn is_on(&self, truth: &Truth) -> bool {
        self.branch.as_deref() == Some(truth.short.as_str())
            || self.commit.as_deref() == Some(truth.commit.as_str())
    }
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
///
/// `Serialize` because it is wire data: the node's union view carries the
/// occupants of every row, and the shape a reader sees must be the shape the
/// engine computed rather than a mirror struct that can drift from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// One ref, everything the reader learned about it in a single pass.
///
/// The pair of `changed` and `occupied` is deliberate: they answer two different
/// questions off one diff. `changed` is *what this ref holds that main does not*
/// — enough to build a column — and `occupied` is the subset whose **status**
/// differs from main's, which is what a row's occupancy chip says. Computing
/// them together is the reason [`survey`] exists: the diff is the expensive part
/// and it is walked once.
///
/// A ref whose diff failed keeps its entry, with the reason in `changed`. A
/// caller rendering columns must show it as a column that could not answer, not
/// drop it: a vanished column reads as a branch that never existed.
#[derive(Debug, Clone)]
pub struct RefState {
    pub reference: Ref,
    pub changed: std::result::Result<Changed, String>,
    /// The documents this ref holds at a status main's copy does not, keyed by
    /// document id. Empty when the diff failed.
    pub occupied: BTreeMap<String, Occupant>,
}

/// One pass of the branch reader: every ref, what it holds, and where to watch
/// for the next change.
///
/// `notices` carries **whole-reader** states only — not a repository, no truth
/// branch, no `git` feature, a ref listing that failed. A single ref that could
/// not be diffed is *not* a notice: it stays in `refs` as a [`RefState`] with an
/// `Err`, so the caller can render it as a column with a reason. The two must
/// not be conflated, because "one branch is unreadable" and "there are no
/// branches to read" justify entirely different things on screen.
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub refs: Vec<RefState>,
    pub notices: Vec<String>,
    /// The repository's common git directory — where the refs, `packed-refs`
    /// and `worktrees/*/HEAD` that determine this answer live, so a caller can
    /// watch it and re-read on a change.
    ///
    /// `None` when there is nothing to watch: no repository here, or a build
    /// without the `git` feature. A long-lived caller takes that as "one answer,
    /// forever" and starts no watcher — unless [`Survey::retry`] says this pass
    /// simply could not tell.
    pub git_dir: Option<PathBuf>,
    /// Every checkout of the repository — the main worktree and each linked
    /// one — with the branch and commit its `HEAD` names.
    ///
    /// Empty when there is no repository. It is what lets a caller serving
    /// directories decide which of them is *main's* working copy (the baseline
    /// for everything below) and which refs are already on screen as a
    /// checkout, without asking a `git` subprocess and without trusting a ref
    /// label — several branches can point at one commit, and only one name of
    /// them survives into [`Ref::label`].
    pub checkouts: Vec<Checkout>,
    /// Whether the reason there is no [`Survey::git_dir`] might not last.
    ///
    /// A repository that was *discovered* and then could not be opened — a
    /// half-written `.git/config`, a permissions blip, an extension gix does
    /// not know — is a different state from "this directory is in no
    /// repository", and only the first is worth trying again. `false` whenever
    /// `git_dir` is set, for a plain non-repository, and for a build without
    /// the `git` feature: those answers do not change on their own.
    pub retry: bool,
}

/// Main-only fallback for a build without the `git` feature: nothing read, and
/// exactly one notice saying why.
#[cfg(not(feature = "git"))]
pub fn survey(_prj: &crate::project::Project, _on_main: &BTreeMap<String, String>) -> Survey {
    Survey {
        notices: vec![NO_GIT_FEATURE.to_string()],
        ..Survey::default()
    }
}

/// Main-only fallback for a build without the `git` feature. The callback is
/// never invoked: there is no truth branch to hand it, and nothing to compare.
#[cfg(not(feature = "git"))]
pub fn survey_with<F>(_prj: &crate::project::Project, _on_main: F) -> Survey
where
    F: FnOnce(&Truth, &[Checkout]) -> Option<BTreeMap<String, String>>,
{
    Survey {
        notices: vec![NO_GIT_FEATURE.to_string()],
        ..Survey::default()
    }
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
