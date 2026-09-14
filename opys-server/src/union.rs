//! The worktree union view: one labeled table across a project's corpora
//! (TASK-0073).
//!
//! A project group is a repository's main worktree plus every sibling worktree
//! carrying the same inventory. Each is a separate corpus with its own copy of
//! the documents, and they drift: a task is `doing` on one branch and `done` on
//! another, a document exists only where it was written, two branches hand out
//! the same id number. This module merges those per-corpus views into one table
//! so the drift is *visible*.
//!
//! It is a view and nothing else. Nothing here merges, resolves, or writes —
//! git remains the only merger (ADR-0051), and the honest thing a node can do
//! is show a user which branch says what and let them go and merge it.
//!
//! [`union`] is a pure function over [`CorpusDocs`] pairs: no manager, no
//! actors, no filesystem, no HTTP. That is deliberate: the same merge with
//! different columns answers a more general question — any set of corpora that
//! drifted apart, however they were gathered — and it can only be reused that
//! way if it knows nothing about where the summaries came from.
//!
//! A corpus that could not be asked arrives as an `Err`, never as an empty
//! `Vec`. That distinction is the honesty of the whole table: "this branch has
//! nothing" and "we do not know what this branch has" produce the same blank
//! column, and only the first of them justifies telling the user that every
//! document is new on the other branch. Taking it as *input* is what makes the
//! second unrepresentable as the first — [`Column::error`], [`Cell::unknown`]
//! and the `only_in` denominator all derive from it.
//!
//! A worktree is not the only thing that can hold a copy of the inventory. A
//! branch nobody has checked out has no directory, no actor and no watcher, and
//! its documents exist only as git objects — so it arrives here as a
//! [`Branches`] value read by [`crate::refreader`], as a second kind of column
//! (ADR-0102 decision 5: the union function is pure over columns and takes them
//! unchanged). Branch columns merge exactly like worktree columns; the only
//! things that know the difference are [`Column::kind`] — a client must not link
//! a branch cell to a corpus that does not exist — and [`Row::occupants`], which
//! is the branch reader's own answer to "who else holds this document, and since
//! when".

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use opys_engine::git::Occupant;
use opys_engine::refs;
use serde::Serialize;

use crate::actor::DocSummary;
use crate::discover::{display_name, Corpus};

/// What one corpus had to say: its document summaries, or why it could not
/// answer (never loaded, stopped, still holding the inventory lock).
pub type CorpusDocs = (Corpus, Result<Vec<DocSummary>, String>);

/// One ref with no checkout, as a column heading.
///
/// Deliberately not a synthetic [`Corpus`]: a corpus is a directory holding
/// `opys.toml`, and faking a root for a branch that has none would put a path
/// that does not exist into [`labels`]'s disambiguation and into the client's
/// links. This is the honest shape — a key, a name, and the commit it was read
/// at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchColumn {
    /// The column key, in [`Cell::cid`] and [`Row::only_in`] where a corpus
    /// column puts its `cid`. `ref-<commit>`: unique because [`Branches`] holds
    /// one entry per *commit*, and unmistakable for a corpus id, which always
    /// ends in a 16-hex hash of its path.
    pub key: String,
    /// What to show above the column — the branch name, or the checkout
    /// directories, exactly as `opys_engine::git::Ref::label` chose it.
    pub label: String,
    /// The commit the column was read at, full hex. Shown on hover, and the
    /// thing to quote when a user goes to look at the branch themselves.
    pub commit: String,
}

/// What one ref held, or why it could not be read — the branch-side twin of
/// [`CorpusDocs`], and merged the same way.
pub type BranchDocs = (BranchColumn, Result<Vec<DocSummary>, String>);

/// Everything the node's ref reader last learned about a project's branches.
///
/// Held as a whole rather than as three loose arguments because the three are
/// one answer: the columns, the per-document occupancy behind them, and the
/// notices that say the reader could not look at all. An empty, notice-free
/// value is the honest "not read yet" and also the honest "nothing to say", and
/// [`Default`] is how a caller with no reader (every unit test below, a plain
/// non-git project) spells it.
#[derive(Debug, Clone, Default)]
pub struct Branches {
    pub columns: Vec<BranchDocs>,
    /// Document id → the refs holding it at a status main's copy does not.
    /// Restricted to the refs that became columns, so a row never carries a chip
    /// for a branch that is already a worktree column of its own.
    pub occupancy: BTreeMap<String, Vec<Occupant>>,
    /// Why there are no branch columns, when there are none for a reason:
    /// `git::NOT_A_REPOSITORY`, `git::NO_TRUTH_BRANCH`, `git::NO_GIT_FEATURE`.
    ///
    /// A notice and a [`Column::error`] are different states and must not look
    /// alike: a notice says *the reader could not look at branches at all*, an
    /// error says *this one branch could not be read* — which is why a failed
    /// ref keeps its column.
    pub notices: Vec<String>,
    /// When the reader last completed a pass, `None` before its first.
    pub read_at: Option<String>,
    /// The branch each served corpus's checkout is on, as of that pass, keyed
    /// by corpus root. Absent when the reader has no answer for it — no
    /// repository, or a pass that could not open one.
    ///
    /// Discovery records a corpus's branch when it first walks the allowlist and
    /// never again until the group is rebuilt, so a checkout that moves under a
    /// running node keeps the name it was found on. That was a quiet staleness
    /// until the reader gained a notice about *which* branch it is measuring
    /// against: the same screen then read `main (primary)` above a column and
    /// "no served worktree is on `main`" above the table. The reader re-resolves
    /// every checkout each pass anyway — this is that answer, published so the
    /// labels can be as fresh as the notice.
    pub branch_of: BTreeMap<PathBuf, Option<String>>,
}

impl Branches {
    /// Whether two passes say the same thing, ignoring when they were taken.
    ///
    /// The reader broadcasts only on a real change: the 60 s backstop otherwise
    /// wakes every open browser every minute forever, and the client treats an
    /// event it cannot attribute to a corpus as affecting every view.
    pub fn same_answer(&self, other: &Branches) -> bool {
        self.columns == other.columns
            && self.occupancy == other.occupancy
            && self.notices == other.notices
            && self.branch_of == other.branch_of
    }
}

/// One merged table: the corpora as columns, the union of their documents as
/// rows.
#[derive(Debug, Clone, Serialize)]
pub struct UnionView {
    /// The corpora, in the order they were given. [`union`] never reorders
    /// them: column order is the caller's to choose (discovery puts the main
    /// worktree first), and a table whose columns moved between requests would
    /// be unreadable.
    pub columns: Vec<Column>,
    /// Every document id that appears in at least one corpus, ordered by the
    /// numeric part of the id.
    pub rows: Vec<Row>,
    /// Why the table has no branch columns, when it has none for a reason —
    /// [`Branches::notices`], passed through. Rendered above the table, once,
    /// and never confused with a column that failed: see [`Column::error`].
    pub notices: Vec<String>,
}

/// What a column *is*, so a client can render one it cannot navigate to.
///
/// A worktree column's [`Column::cid`] addresses a served corpus and every cell
/// links to `/corpus/<cid>/doc/<id>`. A branch column's key addresses nothing —
/// there is no directory, no actor, and no document route — so a client that
/// linked its cells the same way would offer a table of 404s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnKind {
    /// A checked-out corpus the node serves.
    Worktree,
    /// A ref with no checkout, read out of git objects by the ref reader.
    Branch,
}

/// One corpus, as a column heading.
#[derive(Debug, Clone, Serialize)]
pub struct Column {
    /// The stable key: the corpus id for a worktree column, and
    /// [`BranchColumn::key`] for a branch one. Labels are made distinct within a
    /// view (see [`Column::label`]) but are still a human's string; this is the
    /// one to key on — and the one to check [`Column::kind`] against before
    /// treating it as something a document route will accept.
    pub cid: String,
    /// What to show above the column: the branch when git knows one, otherwise
    /// the corpus directory's name, with ` (primary)` appended for the main
    /// worktree.
    ///
    /// Guaranteed distinct within a view where the corpora's directories are:
    /// two columns whose branch name is the same are qualified with their
    /// directory. Nothing stops a repo from having two worktrees on what git
    /// reports as one branch — a jj-colocated repo detaches HEAD in *every*
    /// worktree, so the branch is inferred from the commit and two worktrees on
    /// the same commit infer the same name — and a "labeled" view whose columns
    /// are byte-identical is not labeled at all.
    pub label: String,
    /// Why this column is empty, when it is empty for a reason rather than
    /// because the worktree has no documents.
    ///
    /// Set from the `Err` side of this column's [`CorpusDocs`]. A corpus that
    /// failed to answer contributes no documents, and a column of blanks reads
    /// as "this branch deleted everything" when it means "we do not know" — so
    /// the merge is handed the failure rather than an empty list, and every
    /// derived claim below ([`Cell::unknown`], [`Row::only_in`],
    /// [`Row::differs`]) is computed as if this column were not there.
    /// A column that is a remote source rather than a local worktree inherits
    /// the same problem one layer out, where "did not answer" is routine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether this column is a served worktree or a ref with no checkout.
    pub kind: ColumnKind,
    /// The commit a branch column was read at; `None` for a worktree column,
    /// whose contents are files on disk rather than a fixed revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// One document across every corpus.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// The document id. The row exists because at least one corpus has it.
    pub id: String,
    /// The title to show. Taken from the primary corpus when that corpus has
    /// this document, otherwise from the first column that does — see
    /// [`union`], which explains why the primary cannot simply be assumed to
    /// exist. The other corpora's titles are on their own cells
    /// ([`Cell::title`]), because they can disagree.
    pub title: String,
    /// One cell per column, positionally aligned with [`UnionView::columns`].
    pub cells: Vec<Cell>,
    /// Whether the corpora that have this document disagree about its status or
    /// its title.
    ///
    /// False for a document only one corpus has: with a single observation
    /// there is nothing to disagree with. That absence is real and is reported
    /// by [`Row::only_in`], which a client renders differently — a row that is
    /// merely new on a branch must not light up the same way as a row two
    /// branches have taken in different directions.
    ///
    /// Computed only over the columns that answered. A column that did not is
    /// not evidence of agreement: its cell carries [`Cell::unknown`], and a
    /// client that wants to say "no drift" rather than "no drift among the
    /// branches we could reach" has to check for it.
    pub differs: bool,
    /// The corpora that have this document, listed *only* when at least one
    /// corpus that answered does not.
    ///
    /// Empty when every column has the row, rather than listing all of them:
    /// "only in" is not a claim you can make about a document that is
    /// everywhere, and a field populated on every row is a badge a client would
    /// have to learn to ignore. It is also empty for a single-corpus group, for
    /// the same reason — one column cannot be "only".
    ///
    /// Columns that failed to answer are excluded from the comparison
    /// altogether. A branch that is mid-reload must not turn every document in
    /// the group into "only on main" — that is a claim about what the other
    /// branch deleted, made from no information at all.
    pub only_in: Vec<String>,
    /// Whether some *other* document id shares this row's id number.
    ///
    /// Two branches that each allocated `0042` produce `FEAT-0042` and
    /// `TASK-0042`, which merge cleanly in git and collide in the inventory the
    /// moment the branches meet. Surfacing it here is the warning;
    /// `opys renumber` (FEAT-0017) is the repair.
    ///
    /// Deliberately *not* a property of the rows on screen: it comes from the
    /// `contested` set the caller derives with [`contested_numbers`] over every
    /// id the corpora hold, so a `?status=`/`?type=` filter that hides the other
    /// half of the pair cannot switch the warning off. The hazard is on disk
    /// either way. It can only miss a column that did not answer, whose ids are
    /// unknown.
    pub collision: bool,
    /// Which refs hold this document at a status main's copy does not, and
    /// since when (ADR-0102: branches are progress).
    ///
    /// Related to, but not derivable from, the branch cells beside it: a cell
    /// says what a ref holds, an occupant says that it *disagrees with main*,
    /// which is the claim an orchestrator acts on. The reader computes it off
    /// the same diff that built the columns, so the two cannot drift.
    ///
    /// Omitted from the JSON when empty, which is the common case: most rows are
    /// not in flight anywhere.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub occupants: Vec<Occupant>,
}

/// One document in one corpus.
#[derive(Debug, Clone, Serialize)]
pub struct Cell {
    /// The column this cell belongs to. Redundant with the cell's position, and
    /// kept anyway so a client can render a row without carrying the column list
    /// alongside it.
    pub cid: String,
    /// The document's status in this corpus, or `None` when this corpus does not
    /// have the document at all.
    ///
    /// `None` is the presence bit: absence is the single most important thing
    /// this view says, and giving it its own field would mean two ways to encode
    /// one fact. A document that *is* present but whose frontmatter carries no
    /// status is `Some("")` — an answer, unlike `None`. The one thing `None`
    /// does not mean is "unknown": that is [`Cell::unknown`], and it is a
    /// separate field precisely so `None` keeps meaning absent.
    pub status: Option<String>,
    /// The document's title in this corpus, `None` when it is not here.
    ///
    /// Carried per cell because [`Row::differs`] fires on title drift while
    /// [`Row::title`] holds only the primary's: without this, renaming a
    /// heading on a branch produces a row flagged as diverged whose every
    /// visible field is identical, and the branch's title appears nowhere in
    /// the response.
    pub title: Option<String>,
    /// The document's `updated` field in this corpus, when it has one. `None`
    /// for a document that is absent *and* for one that simply never recorded an
    /// update, because a client showing a timestamp treats both as "nothing to
    /// show"; [`Cell::status`] is what distinguishes them.
    pub updated: Option<String>,
    /// Whether this corpus never answered, so this cell says nothing.
    ///
    /// The blank cell of a column with an error is not an absence — see
    /// [`Column::error`]. A client must suppress "not on this branch" for these
    /// cells rather than render them like a real one.
    pub unknown: bool,
}

/// Merge per-corpus document summaries into one labeled table.
///
/// Pure and total. No corpora is an empty view rather than a panic or an error:
/// a group can be pruned to nothing between the moment its members are listed
/// and the moment they are asked, and an empty table is the truthful rendering
/// of "there is nothing to compare".
///
/// Row order is the numeric part of the id ([`refs::id_number`]), then the id
/// itself. The tiebreak is not decoration: ids that do not parse all share the
/// same sentinel number, and without it their order would depend on which
/// corpus happened to be walked first.
///
/// The primary corpus is preferred for a row's title, but only as a preference:
/// a group whose primary was pruned (its worktree removed while a sibling stays
/// served) has no primary column at all, and a document may exist only on a
/// branch. Both fall back to the first column that has the row.
///
/// `contested` is the set of id numbers claimed by more than one distinct id —
/// [`contested_numbers`] over the corpora's *unfiltered* summaries. It is a
/// separate argument so the warning survives a filtered view; see
/// [`Row::collision`].
///
/// `branches` is what the ref reader last read: columns for the refs nobody has
/// checked out, the occupancy behind them, and the notices that explain their
/// absence. They are appended after the worktree columns and merged identically
/// — a branch that added a document makes a row, a branch that deleted one is
/// reported by `only_in`, a branch that could not be read is a column with an
/// error. Passing them in rather than reading git here is ADR-0102 decision 5:
/// this function stays pure over its columns.
///
/// **The summaries are taken as given.** If the caller filtered them — the
/// `?status=` on the route does exactly that, per corpus, before merging — then
/// a document that is `open` here and `done` there was already dropped from the
/// second corpus and shows up as `only_in` the first. That is the specified
/// behaviour, and it means a filtered union answers "where does this match"
/// rather than "where does this exist".
pub fn union(corpora: &[CorpusDocs], branches: &Branches, contested: &BTreeSet<u64>) -> UnionView {
    let sources = sources(corpora, branches);
    let columns: Vec<Column> = sources
        .iter()
        .map(|source| Column {
            cid: source.cid.clone(),
            label: source.label.clone(),
            error: source.docs.as_ref().err().cloned(),
            kind: source.kind,
            commit: source.commit.clone(),
        })
        .collect();

    // Which columns spoke at all. Everything derived below counts only these:
    // a silent column is not a corpus with no documents.
    let answered: Vec<bool> = sources.iter().map(|s| s.docs.is_ok()).collect();
    let answered_count = answered.iter().filter(|ok| **ok).count();

    // id → one slot per column. A `BTreeMap` so the row set is already ordered
    // by id before the numeric sort below makes the tiebreak explicit.
    let mut slots: BTreeMap<&str, Vec<Option<&DocSummary>>> = BTreeMap::new();
    for (index, source) in sources.iter().enumerate() {
        for doc in source.docs.iter().flat_map(|docs| docs.iter()) {
            let row = slots
                .entry(doc.id.as_str())
                .or_insert_with(|| vec![None; sources.len()]);
            // One corpus carrying an id twice is a corpus `verify` already
            // rejects (`check_unique_numbers`). The first summary wins so the
            // view stays a table rather than growing a second row for a state
            // the user is being told about elsewhere.
            if row[index].is_none() {
                row[index] = Some(doc);
            }
        }
    }

    let primary = sources.iter().position(|source| source.is_primary);
    let mut rows: Vec<Row> = slots
        .into_iter()
        .map(|(id, row)| {
            let present: Vec<(usize, &DocSummary)> = row
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| slot.map(|doc| (index, doc)))
                .collect();
            let title = primary
                .and_then(|index| row.get(index).copied().flatten())
                .or_else(|| present.first().map(|(_, doc)| *doc))
                .map(|doc| doc.title.clone())
                .unwrap_or_default();
            // Everything present is compared against the first thing present,
            // which is enough: if they all match it, they all match each other.
            let differs = match present.split_first() {
                Some(((_, first), rest)) => rest
                    .iter()
                    .any(|(_, doc)| doc.status != first.status || doc.title != first.title),
                None => false,
            };
            let only_in = if present.len() < answered_count {
                present
                    .iter()
                    .map(|(index, _)| columns[*index].cid.clone())
                    .collect()
            } else {
                Vec::new()
            };
            let number = refs::id_number(id);
            Row {
                id: id.to_string(),
                title,
                cells: row
                    .iter()
                    .enumerate()
                    .map(|(index, slot)| Cell {
                        cid: columns[index].cid.clone(),
                        status: slot.map(|doc| doc.status.clone()),
                        title: slot.map(|doc| doc.title.clone()),
                        updated: slot.and_then(|doc| doc.updated.clone()),
                        unknown: !answered[index],
                    })
                    .collect(),
                differs,
                only_in,
                collision: number != u64::MAX && contested.contains(&number),
                occupants: branches.occupancy.get(id).cloned().unwrap_or_default(),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        refs::id_number(&a.id)
            .cmp(&refs::id_number(&b.id))
            .then_with(|| a.id.cmp(&b.id))
    });

    UnionView {
        columns,
        rows,
        notices: branches.notices.clone(),
    }
}

/// One column's inputs, whatever kind of thing produced it — the shape the merge
/// above works over, so a worktree and a branch cannot be treated differently by
/// accident.
struct Source<'a> {
    cid: String,
    label: String,
    kind: ColumnKind,
    commit: Option<String>,
    is_primary: bool,
    docs: Result<&'a [DocSummary], String>,
}

/// The worktree columns, then the branch columns, in the order given.
///
/// Branch labels are left exactly as git chose them. They are not folded into
/// [`labels`]'s deduplication because the two kinds are told apart on screen by
/// [`Column::kind`] rather than by their names, and a branch that a worktree has
/// checked out never reaches here — [`crate::refreader`] drops it, since the
/// checkout is already a column of its own.
fn sources<'a>(corpora: &'a [CorpusDocs], branches: &'a Branches) -> Vec<Source<'a>> {
    let mut out: Vec<Source<'a>> = labels(corpora)
        .into_iter()
        .zip(corpora)
        .map(|(label, (corpus, docs))| Source {
            cid: corpus.cid.clone(),
            label,
            kind: ColumnKind::Worktree,
            commit: None,
            is_primary: corpus.is_primary,
            docs: docs.as_deref().map_err(Clone::clone),
        })
        .collect();
    out.extend(branches.columns.iter().map(|(column, docs)| Source {
        cid: column.key.clone(),
        label: column.label.clone(),
        kind: ColumnKind::Branch,
        commit: Some(column.commit.clone()),
        // A branch is never the primary: that is a served worktree — the one
        // discovery put first — and it is where a row's title comes from.
        is_primary: false,
        docs: docs.as_deref().map_err(Clone::clone),
    }));
    out
}

/// The id numbers more than one *distinct* id claims across these columns.
///
/// Pure, and separate from [`union`] so the caller can compute it over the full
/// summaries and still hand [`union`] a filtered set: an impending id collision
/// is a fact about the corpora, not about what the user is currently looking at,
/// and a `?status=` that hides one half of the pair must not retract the
/// warning.
///
/// Branch columns count too, and must: two agents allocating `0042` on two
/// branches is exactly the hazard, and it is *most* likely to happen on a branch
/// nobody has checked out.
///
/// Distinct *ids*, not observations: the same document in three worktrees is
/// three sightings of one id and nothing is wrong with it. Ids whose number does
/// not parse are left out — they all collapse to one sentinel, so counting them
/// would flag every malformed id as colliding with every other one.
pub fn contested_numbers(corpora: &[CorpusDocs], branches: &Branches) -> BTreeSet<u64> {
    let mut per_number: BTreeMap<u64, BTreeSet<&str>> = BTreeMap::new();
    let seen = corpora
        .iter()
        .map(|(_, docs)| docs)
        .chain(branches.columns.iter().map(|(_, docs)| docs));
    for docs in seen {
        for doc in docs.iter().flatten() {
            let number = refs::id_number(&doc.id);
            if number != u64::MAX {
                per_number.entry(number).or_default().insert(&doc.id);
            }
        }
    }
    per_number
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(number, _)| number)
        .collect()
}

/// A column heading per corpus: the branch it has checked out, or the name of
/// its directory when git cannot say (a plain project, a genuinely detached
/// worktree), with the main worktree marked.
///
/// Computed for the whole set at once rather than per corpus, because a name
/// that two columns share is qualified with the corpus directory — see
/// [`Column::label`] for why that is not hypothetical.
fn labels(corpora: &[CorpusDocs]) -> Vec<String> {
    let names: Vec<String> = corpora
        .iter()
        .map(|(corpus, _)| {
            corpus
                .branch
                .clone()
                .unwrap_or_else(|| display_name(&corpus.root))
        })
        .collect();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for name in &names {
        *seen.entry(name.as_str()).or_default() += 1;
    }
    names
        .iter()
        .zip(corpora)
        .map(|(name, (corpus, _))| {
            let dir = display_name(&corpus.root);
            let name = if seen.get(name.as_str()).is_some_and(|n| *n > 1) && dir != *name {
                format!("{name} — {dir}")
            } else {
                name.clone()
            };
            if corpus.is_primary {
                format!("{name} (primary)")
            } else {
                name
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A corpus with just the fields the merge reads. `Corpus` has no `Default`
    /// and the rest of it (base, group, error) is discovery's business.
    fn corpus(dir: &str, branch: Option<&str>, is_primary: bool) -> Corpus {
        let root = PathBuf::from(format!("/tmp/{dir}"));
        Corpus {
            cid: format!("cid-{dir}"),
            base: root.join("inventory"),
            root,
            group: "g".into(),
            branch: branch.map(str::to_string),
            is_primary,
            error: None,
        }
    }

    fn doc(id: &str, status: &str, title: &str) -> DocSummary {
        DocSummary {
            id: id.into(),
            type_name: "task".into(),
            status: status.into(),
            title: title.into(),
            tags: Vec::new(),
            path: format!("inventory/{id}.md"),
            updated: None,
            blocked: false,
            live_blockers: Vec::new(),
            blocks: 0,
            priority: None,
        }
    }

    /// One corpus that answered, spelled the way the handler spells it.
    fn ok(corpus: Corpus, docs: Vec<DocSummary>) -> CorpusDocs {
        (corpus, Ok(docs))
    }

    /// The whole route's shape in one call: the collision set derived from the
    /// same (here unfiltered) summaries the merge is given.
    fn merged(corpora: Vec<CorpusDocs>) -> UnionView {
        merged_with(corpora, &Branches::default())
    }

    /// The same, with what the ref reader had to say.
    fn merged_with(corpora: Vec<CorpusDocs>, branches: &Branches) -> UnionView {
        let contested = contested_numbers(&corpora, branches);
        union(&corpora, branches, &contested)
    }

    /// One branch column, spelled the way the reader spells it.
    fn branch(label: &str, docs: Result<Vec<DocSummary>, String>) -> BranchDocs {
        (
            BranchColumn {
                key: format!("ref-{label}"),
                label: label.into(),
                commit: "0".repeat(40),
            },
            docs,
        )
    }

    /// The row for an id, or a panic naming what was actually there.
    fn row<'a>(view: &'a UnionView, id: &str) -> &'a Row {
        view.rows
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no row for {id}: {:?}", ids(view)))
    }

    fn ids(view: &UnionView) -> Vec<&str> {
        view.rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn no_corpora_is_an_empty_view() {
        let view = merged(Vec::new());
        assert!(view.columns.is_empty());
        assert!(view.rows.is_empty(), "nothing to compare is not an error");
    }

    #[test]
    fn a_single_corpus_group_is_the_trivial_one_column_view() {
        let view = merged(vec![ok(
            corpus("proj", Some("main"), true),
            vec![doc("TASK-0001", "todo", "One")],
        )]);
        assert_eq!(view.columns.len(), 1);
        let r = row(&view, "TASK-0001");
        assert_eq!(r.cells.len(), 1, "one cell per column");
        assert_eq!(r.cells[0].status.as_deref(), Some("todo"));
        assert!(!r.cells[0].unknown, "the one corpus answered");
        assert!(!r.differs, "one observation cannot disagree with itself");
        assert!(
            r.only_in.is_empty(),
            "one column cannot be the only one: {:?}",
            r.only_in
        );
        assert!(!r.collision);
    }

    #[test]
    fn labels_prefer_the_branch_and_mark_the_primary() {
        let view = merged(vec![
            ok(corpus("proj", Some("main"), true), Vec::new()),
            ok(corpus("proj-feature", Some("feature/x"), false), Vec::new()),
            ok(corpus("plain", None, false), Vec::new()),
        ]);
        let labels: Vec<&str> = view.columns.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["main (primary)", "feature/x", "plain"]);
        assert!(
            view.columns.iter().all(|c| c.error.is_none()),
            "every corpus answered"
        );
    }

    /// git reports a jj-colocated worktree as detached, so the branch is
    /// inferred from the commit — and two worktrees on one commit infer the same
    /// name. A table whose headings are byte-identical is not labeled.
    #[test]
    fn columns_that_would_share_a_label_are_qualified_by_their_directory() {
        let view = merged(vec![
            ok(corpus("proj", Some("main"), true), Vec::new()),
            ok(corpus("proj-wt1", Some("main"), false), Vec::new()),
            ok(corpus("proj-wt2", Some("main"), false), Vec::new()),
        ]);
        let labels: Vec<&str> = view.columns.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "main — proj (primary)",
                "main — proj-wt1",
                "main — proj-wt2"
            ]
        );
        let mut distinct: Vec<&str> = labels.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), labels.len(), "{labels:?}");
    }

    #[test]
    fn a_document_absent_from_one_corpus_gets_an_empty_cell_and_only_in() {
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![
                    doc("TASK-0001", "todo", "One"),
                    doc("TASK-0002", "doing", "Two"),
                ],
            ),
        ]);

        let everywhere = row(&view, "TASK-0001");
        assert!(everywhere.only_in.is_empty(), "present in both columns");
        assert!(!everywhere.differs);

        let branch_only = row(&view, "TASK-0002");
        assert_eq!(branch_only.only_in, ["cid-proj-feature"]);
        assert_eq!(
            branch_only.cells[0].status, None,
            "an absent document has no status"
        );
        assert!(
            !branch_only.cells[0].unknown,
            "main answered; the document really is not there"
        );
        assert_eq!(branch_only.cells[0].cid, "cid-proj");
        assert_eq!(branch_only.cells[1].status.as_deref(), Some("doing"));
        assert!(
            !branch_only.differs,
            "new on a branch is not the same signal as drifted between branches"
        );
    }

    #[test]
    fn a_status_drift_sets_differs() {
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "doing", "One")],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0001", "done", "One")],
            ),
        ]);
        let r = row(&view, "TASK-0001");
        assert!(r.differs, "doing here, done there");
        assert!(r.only_in.is_empty(), "it is in both, it just disagrees");
        assert_eq!(r.cells[0].status.as_deref(), Some("doing"));
        assert_eq!(r.cells[1].status.as_deref(), Some("done"));
    }

    #[test]
    fn a_title_drift_sets_differs_and_both_titles_are_readable() {
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0001", "todo", "One, renamed")],
            ),
        ]);
        let r = row(&view, "TASK-0001");
        assert!(r.differs, "a renamed heading is drift too");
        assert_eq!(r.title, "One", "the primary names the row");
        // Without the per-cell title the row is flagged as diverged and every
        // visible field is identical.
        assert_eq!(r.cells[0].title.as_deref(), Some("One"));
        assert_eq!(
            r.cells[1].title.as_deref(),
            Some("One, renamed"),
            "the branch's title has to be somewhere: {:?}",
            r.cells
        );
    }

    #[test]
    fn three_corpora_differ_when_any_one_of_them_does() {
        let agree = doc("TASK-0001", "todo", "One");
        let view = merged(vec![
            ok(corpus("a", Some("main"), true), vec![agree.clone()]),
            ok(corpus("b", Some("b"), false), vec![agree.clone()]),
            ok(
                corpus("c", Some("c"), false),
                vec![doc("TASK-0001", "done", "One")],
            ),
        ]);
        assert!(
            row(&view, "TASK-0001").differs,
            "the odd one out is still drift"
        );
    }

    #[test]
    fn the_title_falls_back_to_the_first_column_that_has_the_row() {
        // The primary exists but does not carry this document.
        let view = merged(vec![
            ok(corpus("proj", Some("main"), true), Vec::new()),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0001", "todo", "Written on the branch")],
            ),
        ]);
        assert_eq!(row(&view, "TASK-0001").title, "Written on the branch");

        // A pruned group can lose its primary altogether.
        let view = merged(vec![
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0001", "todo", "First column")],
            ),
            ok(
                corpus("proj-other", Some("other"), false),
                vec![doc("TASK-0001", "todo", "Second column")],
            ),
        ]);
        assert_eq!(
            row(&view, "TASK-0001").title,
            "First column",
            "no primary means the leftmost column names the row"
        );
    }

    /// The primary is not always column zero — discovery puts it first today,
    /// and a caller assembling columns some other way need not. The rule is
    /// "the primary names the row", not "the first column does".
    #[test]
    fn the_primary_names_the_row_wherever_it_sits() {
        let view = merged(vec![
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0001", "todo", "Branch title")],
            ),
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "todo", "Primary title")],
            ),
        ]);
        assert_eq!(row(&view, "TASK-0001").title, "Primary title");
    }

    #[test]
    fn two_ids_sharing_a_number_flag_both_rows() {
        let shared = doc("TASK-0007", "todo", "Fine");
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("FEAT-0042", "todo", "Mine"), shared.clone()],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0042", "todo", "Theirs"), shared.clone()],
            ),
        ]);
        assert!(row(&view, "FEAT-0042").collision, "both branches took 0042");
        assert!(row(&view, "TASK-0042").collision);
        // The control is deliberately in *both* corpora: a collision is two
        // distinct ids on one number, never two sightings of one id, and
        // counting sightings would flag every shared document in the group.
        assert!(
            !row(&view, "TASK-0007").collision,
            "one document in two worktrees is not a collision"
        );
    }

    /// The filters run per corpus before the merge, so the other half of a
    /// contested number can be missing from the rows entirely. The warning is a
    /// fact about the corpora and survives it.
    #[test]
    fn a_collision_survives_a_filter_that_hid_the_other_half() {
        let corpora = vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("NOTE-0004", "open", "Mine")],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0004", "todo", "Theirs")],
            ),
        ];
        let contested = contested_numbers(&corpora, &Branches::default());
        // What the handler passes after filtering: only the branch's row.
        let filtered = vec![
            ok(corpus("proj", Some("main"), true), Vec::new()),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![doc("TASK-0004", "todo", "Theirs")],
            ),
        ];
        let view = union(&filtered, &Branches::default(), &contested);
        assert_eq!(ids(&view), ["TASK-0004"]);
        assert!(
            row(&view, "TASK-0004").collision,
            "a filter is not evidence that the other id stopped existing"
        );
    }

    #[test]
    fn unparsable_ids_do_not_collide_with_each_other() {
        let view = merged(vec![ok(
            corpus("proj", Some("main"), true),
            vec![
                doc("not-an-id", "todo", "A"),
                doc("also_broken", "todo", "B"),
                doc("TASK-0001", "todo", "C"),
            ],
        )]);
        assert!(
            view.rows.iter().all(|r| !r.collision),
            "every malformed id shares one sentinel number; that is not a collision"
        );
    }

    #[test]
    fn rows_are_ordered_by_the_numeric_id_part() {
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![
                    doc("TASK-0010", "todo", "Ten"),
                    doc("NOTE-0002", "todo", "Two"),
                    // Unpadded: lexicographically "TASK-10" precedes "TASK-9".
                    doc("TASK-9", "todo", "Nine, unpadded"),
                    doc("zzz-broken", "todo", "Last"),
                ],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![
                    // Lexicographically after AAA-0002, numerically before it.
                    doc("ZZZ-0001", "todo", "One"),
                    doc("AAA-0002", "todo", "Also two"),
                    doc("TASK-0009", "todo", "Nine"),
                    doc("aaa-broken", "todo", "Also last"),
                ],
            ),
        ]);
        assert_eq!(
            ids(&view),
            [
                "ZZZ-0001",
                "AAA-0002",
                "NOTE-0002",
                "TASK-0009",
                "TASK-9",
                "TASK-0010",
                // Unparsable ids sort last, among themselves by id — not by
                // whichever corpus was walked first.
                "aaa-broken",
                "zzz-broken",
            ],
            "numeric order, not lexicographic, and not per-corpus order"
        );
    }

    #[test]
    fn every_row_has_one_cell_per_column_in_column_order() {
        let view = merged(vec![
            ok(
                corpus("a", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            ),
            ok(corpus("b", Some("b"), false), Vec::new()),
            ok(
                corpus("c", Some("c"), false),
                vec![doc("TASK-0002", "todo", "Two")],
            ),
        ]);
        let cids: Vec<&str> = view.columns.iter().map(|c| c.cid.as_str()).collect();
        for r in &view.rows {
            let cells: Vec<&str> = r.cells.iter().map(|c| c.cid.as_str()).collect();
            assert_eq!(cells, cids, "cells align positionally with columns");
        }
    }

    #[test]
    fn a_cell_carries_the_updated_stamp_of_its_own_corpus() {
        let mut newer = doc("TASK-0001", "todo", "One");
        newer.updated = Some("2026-08-24T00:00:00Z".into());
        let view = merged(vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            ),
            ok(
                corpus("proj-feature", Some("feature/x"), false),
                vec![newer],
            ),
        ]);
        let r = row(&view, "TASK-0001");
        assert_eq!(r.cells[0].updated, None, "no stamp in the main worktree");
        assert_eq!(
            r.cells[1].updated.as_deref(),
            Some("2026-08-24T00:00:00Z"),
            "each cell reports its own corpus"
        );
    }

    /// The whole reason the merge is handed a `Result`: a corpus that could not
    /// answer must not read as a corpus with nothing in it.
    #[test]
    fn a_corpus_that_could_not_answer_is_unknown_rather_than_empty() {
        let corpora = vec![
            ok(
                corpus("proj", Some("main"), true),
                vec![
                    doc("NOTE-0001", "open", "Shared"),
                    doc("TASK-0002", "doing", "Drifted"),
                ],
            ),
            (
                corpus("proj-feature", Some("feature/x"), false),
                Err("corpus is not loaded: bad config".to_string()),
            ),
        ];
        let view = union(
            &corpora,
            &Branches::default(),
            &contested_numbers(&corpora, &Branches::default()),
        );

        assert_eq!(view.columns[0].error, None);
        assert_eq!(
            view.columns[1].error.as_deref(),
            Some("corpus is not loaded: bad config"),
            "the merge sets the reason itself"
        );

        for id in ["NOTE-0001", "TASK-0002"] {
            let r = row(&view, id);
            assert!(
                r.only_in.is_empty(),
                "a silent branch is no evidence that {id} is main's alone: {:?}",
                r.only_in
            );
            assert!(!r.cells[0].unknown);
            assert!(r.cells[1].unknown, "the blank cell means 'we do not know'");
            assert_eq!(r.cells[1].status, None);
        }
    }

    /// With every column silent there is nothing to say, and nothing is said.
    #[test]
    fn no_column_answering_is_an_empty_table_not_an_empty_inventory() {
        let corpora = vec![
            (
                corpus("proj", Some("main"), true),
                Err("corpus is busy".to_string()),
            ),
            (
                corpus("proj-feature", Some("feature/x"), false),
                Err("corpus is busy".to_string()),
            ),
        ];
        let view = union(
            &corpora,
            &Branches::default(),
            &contested_numbers(&corpora, &Branches::default()),
        );
        assert_eq!(view.columns.len(), 2);
        assert!(view.rows.is_empty(), "no observations, no rows");
        assert!(view.columns.iter().all(|c| c.error.is_some()));
    }

    /// Three columns, one silent: the two that answered are still compared to
    /// each other, and the silent one neither confirms nor denies.
    #[test]
    fn a_silent_column_is_left_out_of_only_in_but_the_rest_still_compare() {
        let corpora = vec![
            ok(
                corpus("a", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            ),
            ok(corpus("b", Some("b"), false), Vec::new()),
            (
                corpus("c", Some("c"), false),
                Err("corpus is busy".to_string()),
            ),
        ];
        let view = union(
            &corpora,
            &Branches::default(),
            &contested_numbers(&corpora, &Branches::default()),
        );
        let r = row(&view, "TASK-0001");
        assert_eq!(
            r.only_in,
            ["cid-a"],
            "b answered and does not have it; c said nothing"
        );
        assert!(r.cells[2].unknown);
    }

    /// The whole point of a branch column: a ref nobody has checked out is a
    /// column beside the worktrees, and the row says who holds it and since
    /// when.
    #[test]
    fn a_branch_with_no_worktree_is_a_column_of_its_own() {
        let branches = Branches {
            columns: vec![branch(
                "feat-x",
                Ok(vec![
                    doc("TASK-0001", "doing", "One"),
                    doc("TASK-0002", "todo", "Two"),
                ]),
            )],
            occupancy: BTreeMap::from([(
                "TASK-0001".to_string(),
                vec![Occupant {
                    label: "feat-x".into(),
                    status: "doing".into(),
                    since: Some("2026-02-01T00:01:00.000000Z".into()),
                }],
            )]),
            notices: vec!["packed-refs could not be read".to_string()],
            ..Branches::default()
        };
        let view = merged_with(
            vec![ok(
                corpus("proj", Some("main"), true),
                vec![
                    doc("TASK-0001", "todo", "One"),
                    doc("TASK-0002", "todo", "Two"),
                ],
            )],
            &branches,
        );

        assert_eq!(
            view.columns
                .iter()
                .map(|c| (c.label.as_str(), c.kind))
                .collect::<Vec<_>>(),
            [
                ("main (primary)", ColumnKind::Worktree),
                ("feat-x", ColumnKind::Branch),
            ]
        );
        // A branch column names the commit it was read at; a worktree is files.
        assert_eq!(
            view.columns[1].commit.as_deref(),
            Some("0".repeat(40).as_str())
        );
        assert!(view.columns[0].commit.is_none());

        let r = row(&view, "TASK-0001");
        assert_eq!(r.cells[1].status.as_deref(), Some("doing"));
        assert!(r.differs, "main says todo, the branch says doing");
        assert_eq!(r.occupants.len(), 1);
        assert_eq!(r.occupants[0].label, "feat-x");
        // A row nothing is doing carries no occupants at all.
        assert!(row(&view, "TASK-0002").occupants.is_empty());

        // And the wire form, because that is the contract the dashboard keys
        // off: it renders a branch cell as text rather than a document link on
        // `kind === "branch"`, so the *spelling* of the discriminant is load
        // bearing — a rename here turns every branch cell into a 404.
        let wire = serde_json::to_value(&view).expect("the view serializes");
        assert_eq!(wire["columns"][0]["kind"], "worktree");
        assert_eq!(
            wire["columns"][0].get("commit"),
            None,
            "a worktree is files"
        );
        assert_eq!(wire["columns"][1]["kind"], "branch");
        assert_eq!(wire["columns"][1]["cid"], "ref-feat-x");
        assert_eq!(wire["columns"][1]["commit"], "0".repeat(40));
        assert_eq!(
            wire["rows"][0]["occupants"],
            serde_json::json!([{
                "label": "feat-x",
                "status": "doing",
                "since": "2026-02-01T00:01:00.000000Z",
            }])
        );
        assert_eq!(
            wire["rows"][1].get("occupants"),
            None,
            "an empty list is omitted, not rendered as a badge"
        );
        assert_eq!(
            wire["notices"],
            serde_json::json!(["packed-refs could not be read"]),
            "the reader's notices are the view's, above the table"
        );
    }

    /// The trap the overlay exists to avoid, pinned from the merge's side: a
    /// branch column carries the *whole* inventory it holds, so a document the
    /// branch left alone is not reported as missing from it.
    #[test]
    fn a_branch_that_changed_one_document_does_not_look_like_it_deleted_the_rest() {
        let view = merged_with(
            vec![ok(
                corpus("proj", Some("main"), true),
                vec![
                    doc("TASK-0001", "todo", "One"),
                    doc("TASK-0002", "todo", "Two"),
                ],
            )],
            &Branches {
                columns: vec![branch(
                    "feat-x",
                    Ok(vec![
                        doc("TASK-0001", "doing", "One"),
                        doc("TASK-0002", "todo", "Two"),
                    ]),
                )],
                ..Branches::default()
            },
        );
        for r in &view.rows {
            assert!(r.only_in.is_empty(), "{}: {:?}", r.id, r.only_in);
        }
        // And a document the branch really did delete *is* reported.
        let view = merged_with(
            vec![ok(
                corpus("proj", Some("main"), true),
                vec![
                    doc("TASK-0001", "todo", "One"),
                    doc("TASK-0002", "todo", "Two"),
                ],
            )],
            &Branches {
                columns: vec![branch("feat-x", Ok(vec![doc("TASK-0001", "todo", "One")]))],
                ..Branches::default()
            },
        );
        assert_eq!(row(&view, "TASK-0002").only_in, ["cid-proj"]);
    }

    /// A ref the reader could not diff keeps its column. The alternative — a
    /// column that quietly disappears — reads as a branch that never existed,
    /// and the same `unknown`/`only_in` exclusions that protect a silent
    /// worktree must protect it.
    #[test]
    fn a_branch_that_could_not_be_read_is_a_column_with_a_reason() {
        let view = merged_with(
            vec![ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0001", "todo", "One")],
            )],
            &Branches {
                columns: vec![branch("feat-x", Err("git: bad object".into()))],
                ..Branches::default()
            },
        );

        assert_eq!(view.columns.len(), 2, "the column is still there");
        assert_eq!(view.columns[1].error.as_deref(), Some("git: bad object"));
        let r = row(&view, "TASK-0001");
        assert!(r.cells[1].unknown, "nothing is claimed about this branch");
        assert!(
            r.only_in.is_empty(),
            "a branch that did not answer is no evidence of absence: {:?}",
            r.only_in
        );
    }

    /// "No branches to read" and "one branch could not be read" are different
    /// states: the first is a notice with no columns, the second a column with
    /// an error. Conflating them would tell a user with no repository that a
    /// branch broke.
    #[test]
    fn no_repository_is_a_notice_rather_than_a_broken_column() {
        let view = merged_with(
            vec![ok(
                corpus("proj", None, true),
                vec![doc("TASK-0001", "todo", "One")],
            )],
            &Branches {
                notices: vec![opys_engine::git::NOT_A_REPOSITORY.to_string()],
                ..Branches::default()
            },
        );
        assert_eq!(view.columns.len(), 1, "no branch columns at all");
        assert_eq!(view.notices, [opys_engine::git::NOT_A_REPOSITORY]);
        assert!(view.columns.iter().all(|c| c.error.is_none()));
    }

    /// The hazard ADR-0102 names, on the column where it is most likely: two
    /// agents on two branches minting the same number. A branch nobody checked
    /// out must not be able to hide it.
    #[test]
    fn an_id_minted_only_on_a_branch_still_contests_its_number() {
        let view = merged_with(
            vec![ok(
                corpus("proj", Some("main"), true),
                vec![doc("TASK-0042", "todo", "Ours")],
            )],
            &Branches {
                columns: vec![branch(
                    "feat-x",
                    Ok(vec![
                        doc("TASK-0042", "todo", "Ours"),
                        doc("FEAT-0042", "planned", "Theirs"),
                    ]),
                )],
                ..Branches::default()
            },
        );
        assert!(row(&view, "TASK-0042").collision);
        assert!(row(&view, "FEAT-0042").collision);
    }
}
