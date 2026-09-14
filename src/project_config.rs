//! The universal typed-document config (`opys.toml`) — the sole
//! source of truth for the engine.
//!
//! A project declares its own document **types** (each with a prefix, dir,
//! statuses, fields, and required sections) plus a list of conditional
//! **validation rules**. `Project::open` loads this; every command reads it, and
//! `verify` enforces it via [`crate::rules`]. Reuses
//! `config::FieldSpec`/`FieldType`/`TestRefCheck`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;

use crate::config::{FieldSpec, FieldType};
use crate::error::{usage, OpysError, Result};
use crate::refs;

/// Why this `base` may not be used, or `None` if it is a fine relative path.
///
/// Purely lexical, so it works on a base that does not exist yet and can be
/// reported by `validate` as well as enforced by
/// [`ProjectConfig::resolve_base`]. `.` is allowed: an inventory *at* the
/// project root is inside it.
fn base_escape(base: &str) -> Option<&'static str> {
    use std::path::Component;
    if base.is_empty() {
        return Some("is empty");
    }
    let p = Path::new(base);
    if p.is_absolute() {
        return Some("is an absolute path");
    }
    for c in p.components() {
        match c {
            Component::ParentDir => return Some("climbs out of the project with `..`"),
            Component::Prefix(_) | Component::RootDir => return Some("is an absolute path"),
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    None
}

fn default_pad() -> usize {
    4
}
fn default_base() -> String {
    DEFAULT_BASE.to_string()
}
fn default_roots() -> Vec<String> {
    vec![".".to_string()]
}
fn default_min() -> usize {
    1
}
fn default_true() -> bool {
    true
}
fn default_layout_path() -> String {
    DEFAULT_LAYOUT_PATH.to_string()
}
fn default_git_main() -> String {
    DEFAULT_TRUTH_BRANCH.to_string()
}
fn default_git_branches() -> Vec<String> {
    vec![DEFAULT_BRANCH_PATTERN.to_string()]
}

/// Placeholder names in a layout template that aren't one of `type`/`status`/`id`.
fn unknown_placeholders(template: &str) -> Vec<&str> {
    let known = ["type", "status", "id"];
    placeholder_names(template)
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect()
}

/// Every distinct `{name}` placeholder in `template`, in order of first
/// appearance.
fn placeholder_names(template: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            let name = &rest[..close];
            if !out.contains(&name) {
                out.push(name);
            }
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
    out
}

/// Inventory base directory (the documents and `_retired.txt`), relative to the
/// project root that holds `opys.toml`; the config `base` default.
pub const DEFAULT_BASE: &str = "opys";

/// Directory (under the inventory base) for a type that declares no explicit
/// `dir`. Empty by default → documents live flat at the base.
pub const DEFAULT_DOC_DIR: &str = "";

/// The branch a project calls truth when `[git] main` says nothing. When
/// `refs/heads/main` does not resolve, the reader falls back to `master`.
pub const DEFAULT_TRUTH_BRANCH: &str = "main";

/// The default `[git] branches` pattern: local heads only, so remote-tracking
/// refs are opt-in.
pub const DEFAULT_BRANCH_PATTERN: &str = "refs/heads/*";

/// Default file-path template (relative to the base). Both the `{type}` and
/// `{status}` segments are empty by default, so this collapses to a flat
/// `PREFIX-NNNN.md` at the base.
pub const DEFAULT_LAYOUT_PATH: &str = "{type}/{status}/{id}.md";

/// The on-disk layout: a single path template rendered per document. The
/// `{type}` placeholder resolves to the type's `dir`, `{status}` to the type's
/// `status_dirs[status]` (both empty by default), and `{id}` to `PREFIX-NNNN`.
/// Empty segments collapse, so the order of segments is freely configurable.
#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    #[serde(default = "default_layout_path")]
    pub path: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            path: default_layout_path(),
        }
    }
}

/// Id allocation (ADR-0103). With `modulus = M` set, each allocator owns a
/// slot in `0..M` (from `OPYS_ID_SLOT`, else a stable hash of `user@host`) and
/// draws only numbers congruent to it, so allocators on different branches
/// cannot collide. Absent, allocation is the dense global sequence.
///
/// Unlike the rest of the config, this table rejects unknown keys at parse
/// time (a hard `OpysError::Toml`, exit 2, from every command): a misspelled
/// `modulus` would otherwise leave the project silently dense, and
/// `Project::open` never runs [`ProjectConfig::validate`], so a validate-time
/// report could not stop `new` from allocating.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ids {
    #[serde(default)]
    pub modulus: Option<u64>,
}

/// Reading the inventory out of git (ADR-0102): which branch is *truth*, and
/// which refs count as work in flight.
///
/// `main` names the truth branch — where `next` evaluates pickability and
/// completion, and the baseline every other ref is diffed against. When
/// `refs/heads/<main>` does not resolve, the reader falls back to `master`, so
/// a repository that never renamed its default branch needs no config at all.
///
/// `branches` is a list of glob patterns matched against **fully qualified** ref
/// names with git's own wildmatch, in which `*` crosses `/` (so `refs/heads/*`
/// covers `refs/heads/feat/x`, the way a refspec does). It selects which refs
/// the reader reads, and defaults to local heads only: a remote-tracking ref is
/// opt-in because keeping it fetched is the user's job, and a stale
/// `refs/remotes/*` would report work in flight that ended days ago. Linked
/// worktree heads are read whatever the patterns say — they are not ref names,
/// so no pattern could select them.
///
/// Like [`Ids`], this table rejects unknown keys at parse time (a hard
/// `OpysError::Toml`, exit 2): a misspelled `main` would silently change what
/// "truth" means, and `Project::open` never runs [`ProjectConfig::validate`],
/// so a validate-time report could not stop the reader from using the wrong
/// baseline.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Git {
    #[serde(default = "default_git_main")]
    pub main: String,
    #[serde(default = "default_git_branches")]
    pub branches: Vec<String>,
}

impl Default for Git {
    fn default() -> Self {
        Git {
            main: default_git_main(),
            branches: default_git_branches(),
        }
    }
}

/// A built-in section behavior a type's section opts into. The validator and
/// scaffold for each kind are compiled code (closed set, not extensible from
/// config) — this is the guardrail that keeps the engine opinionated. The
/// `structured` kind is the configurable one: its content shape is declared by
/// the section's `structure` (an [`mdprism`] schema; see
/// `docs/structure-dsl-spec.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionKind {
    Prose,
    Log,
    Checklist,
    Structured,
}

impl SectionKind {
    /// Whether "≥1 checked item" is meaningful for this kind.
    pub fn is_checkable(self) -> bool {
        matches!(self, SectionKind::Checklist)
    }

    /// Whether this section holds dated log entries — written by `opys log`,
    /// parsed by [`crate::body::log_entries`], checked by `verify`.
    pub fn is_log(self) -> bool {
        matches!(self, SectionKind::Log)
    }
}

/// Which lines of a section a [`SectionCheck`] validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckScope {
    /// Every line in the section; lines not matching `pattern` are skipped.
    #[default]
    All,
    /// Only checked checklist items; a checked item with no `pattern` match is
    /// itself an error.
    Checked,
}

/// A universal, config-driven content check attached to a section. `pattern`
/// parses a line into named capture groups; `file` (a group name) and/or
/// `must_match` (a regex built from `${group}` substitutions) then assert the
/// captured reference points at something real.
#[derive(Debug, Clone, Deserialize)]
pub struct SectionCheck {
    /// Regex parsing one line into named groups. A line that does not match is
    /// prose (skipped); a match is validated.
    pub pattern: String,
    /// Optional capture-group name holding a file path to open (resolved against
    /// `roots`). Its presence is what makes the check file-scoped vs corpus-wide.
    #[serde(default)]
    pub file: Option<String>,
    /// Directories the `file` path / corpus resolve against (project-root
    /// relative). Defaults to `["."]`.
    #[serde(default = "default_roots")]
    pub roots: Vec<String>,
    /// Optional regex that must match in the opened file (or, when `file` is
    /// unset, in the concatenated corpus under `roots`). `${group}` is replaced
    /// by the regex-escaped capture from `pattern`.
    #[serde(default)]
    pub must_match: Option<String>,
    /// Which lines to validate. Defaults to `all`.
    #[serde(default)]
    pub scope: CheckScope,
    /// Optional custom failure message; `${group}` is replaced by the raw
    /// capture from `pattern`.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SectionSpec {
    pub heading: String,
    pub kind: SectionKind,
    /// Whether the section must be present (verify enforces it; `new` scaffolds it).
    #[serde(default)]
    pub required: bool,
    /// Universal content checks run over this section at `verify` time.
    #[serde(default)]
    pub checks: Vec<SectionCheck>,
    /// The content shape for a `structured` section: an [`mdprism`] schema (the
    /// body portion of the DSL). Required for `structured`, rejected otherwise.
    #[serde(default)]
    pub structure: Option<String>,
}

/// `requires_link = { to = "feature", min = 1 }` — a type must reference ≥`min`
/// docs of type `to`.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkReq {
    pub to: String,
    #[serde(default = "default_min")]
    pub min: usize,
}

/// A `[types.<name>.ttl]` value: `in-progress = "10m"` (observed only) or
/// `in-progress = { after = "10m", then = "stuck" }` (observed, and naming where
/// the project intends a stuck document to go — opys reports that target and
/// never performs the move).
///
/// Deserialized by hand rather than as an untagged enum so both spellings land
/// in one shape and a malformed table names the field it is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ttl {
    /// How long the document may sit in the status, e.g. `"10m"`, `"2d"`,
    /// `"1h30m"`. Unparsed here — see [`crate::observe::parse_duration`].
    pub after: String,
    /// Where the project intends a stuck document to go, when the config says.
    /// Recorded intent, reported by `verify`; opys itself never moves it.
    pub then: Option<String>,
}

impl<'de> Deserialize<'de> for Ttl {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::{Error as _, MapAccess, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ttl;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a duration string, or a table { after = \"10m\", then = \"stuck\" }")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Ttl, E> {
                Ok(Ttl {
                    after: s.to_string(),
                    then: None,
                })
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> std::result::Result<Ttl, M::Error> {
                let (mut after, mut then) = (None, None);
                while let Some(key) = m.next_key::<String>()? {
                    match key.as_str() {
                        "after" => after = Some(m.next_value::<String>()?),
                        "then" => then = Some(m.next_value::<String>()?),
                        // Unknown keys are ignored, as everywhere else in this
                        // config: a misspelled one leaves the entry observe-only,
                        // which `verify` shows, rather than taking every command
                        // down with a parse error.
                        _ => {
                            m.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Ttl {
                    after: after.ok_or_else(|| M::Error::missing_field("after"))?,
                    then,
                })
            }
        }
        d.deserialize_any(V)
    }
}

/// A `[types.<name>.max_visits]` value: `review = { limit = 3, then = "user-review" }`.
///
/// `limit = N` means N entries into the status are allowed; the document that
/// has made its N-th is *at* its limit — `verify` reports it and `set-status`
/// refuses another entry, so `then` is where it goes next.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MaxVisits {
    pub limit: usize,
    pub then: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocType {
    pub prefix: String,
    /// Directory under the inventory base holding this type's files (defaults to
    /// the type name). The legacy adapter sets `features`/`work-items`.
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub statuses: Vec<String>,
    /// Per-status subdirectory (the `{status}` layout segment). A status absent
    /// from the map contributes an empty segment. E.g. `archived = "_archived"`.
    #[serde(default)]
    pub status_dirs: BTreeMap<String, String>,
    #[serde(default)]
    pub default_status: String,
    #[serde(default)]
    pub terminal_statuses: Vec<String>,
    /// `[types.<name>.transitions]` — the legal status moves: a status → the
    /// statuses `set-status` may move it to (ADR-0099 decision 1).
    ///
    /// Absent or empty, the type keeps the any-to-any rule byte-identically —
    /// that is what makes this additive for every existing config. When it is
    /// non-empty it is the whole truth: a status *absent* from the map has no
    /// outgoing edges at all (a dead end, which is legal and sometimes the
    /// point — `superseded` is where an ADR stops). `close` still owns the
    /// terminal statuses, so a declared edge into one says "this status may
    /// close", not "`set-status` may write it".
    ///
    /// Read it through [`crate::lifecycle::allowed_targets`], never directly:
    /// that is the one place declared order and de-duplication are applied, and
    /// the one place `set-status`, `close`, `verify` and the node agree.
    #[serde(default)]
    pub transitions: BTreeMap<String, Vec<String>>,
    /// `releases_blocks` — the statuses of *this* type at which a document of it
    /// stops blocking whatever it blocks (ADR-0099 decision 3).
    ///
    /// Blocked is derived, never stored: a `blocked_by` entry is live while its
    /// target still holds. Empty (the default) means a document of this type
    /// blocks at every status it can hold, which is the right answer for an
    /// **ephemeral** type — it releases by construction, because `close` deletes
    /// it and strikes every inbound entry, and a struck entry is never live. On
    /// such a type the key is accepted but redundant. It earns its keep on a
    /// **permanent** type, which is never closed and would otherwise block
    /// forever: a feature releases at `implemented`, `wontfix`, `archived`.
    ///
    /// Read it through [`crate::refs::Corpus`], never directly — that is the one
    /// place `list --blocked`, the board badge and the document view agree on
    /// what "live" means.
    #[serde(default)]
    pub releases_blocks: Vec<String>,
    /// `[types.<name>] order_by` — the tiebreak chain documents of this type are
    /// put in intentional order by (ADR-0099 decision 4), e.g.
    /// `["priority", "blocks", "id"]`.
    ///
    /// Absent or empty is *not* "unordered": it is the implicit
    /// [`crate::order::DEFAULT_ORDER_BY`], with the `priority` key dropped on a
    /// type that declares no `int` field of that name — priority is opt-in
    /// (ADR-0095) and the default may not demand it. Naming `priority`
    /// explicitly on such a type is a [`ProjectConfig::validate`] problem, since
    /// the author asked for something the type cannot answer.
    ///
    /// Read it through [`crate::order::chain`], never directly: that is the one
    /// place the default and its carve-out live, so `list --sort` and everything
    /// after it cannot order the same corpus two ways.
    #[serde(default)]
    pub order_by: Vec<crate::order::OrderKey>,
    /// `[types.<name>.ttl]` — how long a document may sit in a status before
    /// `verify` calls it **stuck** (ADR-0101 decision 1). A status maps either to
    /// a bare duration (observed only) or to `{ after, then }` (observed, with
    /// `then` naming where the project intends the document to go).
    ///
    /// The duration stays a string here on purpose: a typo must be a
    /// `validate` problem (exit 1), never a parse error that changes the exit
    /// code of every command. Parse it with [`crate::observe::parse_duration`].
    #[serde(default)]
    pub ttl: BTreeMap<String, Ttl>,
    /// `[types.<name>.max_visits]` — how often a document may enter a status
    /// before `verify` calls it **overflowed** and `set-status` refuses another
    /// entry (ADR-0101 decision 1). Read through [`crate::observe`].
    #[serde(default)]
    pub max_visits: BTreeMap<String, MaxVisits>,
    #[serde(default)]
    pub tags_required: bool,
    #[serde(default)]
    pub requires_link: Option<LinkReq>,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldSpec>,
    #[serde(default)]
    pub sections: Vec<SectionSpec>,
}

impl DocType {
    /// The `{type}` layout segment for this type: its `dir`, or the default
    /// (empty → flat at the base).
    pub fn resolved_dir(&self) -> &str {
        self.dir.as_deref().unwrap_or(DEFAULT_DOC_DIR)
    }

    /// The `{status}` layout segment for the given status (empty if unmapped).
    pub fn status_dir(&self, status: &str) -> &str {
        self.status_dirs.get(status).map_or("", String::as_str)
    }
}

/// A rule's match guard. Both fields optional: omitting both means "always".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct When {
    #[serde(default, rename = "type")]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Applies only to documents carrying this tag (exact tag or tag key — see
    /// [`Frontmatter::has_tag`](crate::frontmatter::Frontmatter::has_tag)).
    #[serde(default)]
    pub tag: Option<String>,
}

/// One term of a `require_any` (exactly one of the four is set).
#[derive(Debug, Clone, Deserialize)]
pub struct AnyTerm {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
    /// Holds when the document carries this tag (exact tag or tag key).
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldMatch {
    pub field: String,
    pub pattern: String,
}

/// A conditional validation rule: a `when` guard plus exactly one assertion
/// from the closed set below.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub require_field: Option<String>,
    #[serde(default)]
    pub require_section: Option<String>,
    #[serde(default)]
    pub require_checked_section: Option<String>,
    #[serde(default)]
    pub require_link: Option<LinkReq>,
    #[serde(default)]
    pub require_any: Option<Vec<AnyTerm>>,
    #[serde(default)]
    pub field_matches: Option<FieldMatch>,
    /// The heading of a `log` section that must carry an entry stamped at or
    /// after the last `history` entry whose status *differs* from the status
    /// this rule guards (the first key when every entry holds it). Shipped on
    /// the move into `review`, so a handoff without a checkpoint is refused.
    /// See [`crate::rules`] for why that definition — rather than "the last
    /// history key" — makes the write-time and verify-time verdicts agree.
    #[serde(default)]
    pub require_log_since_status: Option<String>,
}

impl Rule {
    /// How many assertions this rule sets (must be exactly one).
    fn assertion_count(&self) -> usize {
        [
            self.require_field.is_some(),
            self.require_section.is_some(),
            self.require_checked_section.is_some(),
            self.require_link.is_some(),
            self.require_any.is_some(),
            self.field_matches.is_some(),
            self.require_log_since_status.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }
}

/// `[file_refs]` — how document ids are mentioned in code, for `opys show <id>
/// --refs` and the post-`renumber` reference warning. This is about textual
/// mentions of an id (e.g. `FEAT-0123`) in source files, distinct from the
/// `references` relation maps *between documents* (see [`crate::refs`]).
#[derive(Debug, Clone, Deserialize)]
pub struct FileRefs {
    /// Directories to scan, project-root relative. Defaults to the whole project
    /// (`["."]`). The inventory `base`, `.git`, `target`, `node_modules`, and
    /// hidden directories are always skipped.
    #[serde(default = "default_roots")]
    pub roots: Vec<String>,
    /// The textual forms an id may take in code. When empty, a single canonical
    /// `{id}` word-boundary format is assumed (see [`FileRefs::effective_formats`]).
    #[serde(default)]
    pub formats: Vec<RefFormat>,
}

impl Default for FileRefs {
    fn default() -> Self {
        FileRefs {
            roots: default_roots(),
            formats: Vec::new(),
        }
    }
}

impl FileRefs {
    /// The configured formats, or a single canonical `{id}` word-boundary format
    /// when none are configured (so the feature works without any config).
    pub fn effective_formats(&self) -> Vec<RefFormat> {
        if self.formats.is_empty() {
            vec![RefFormat {
                template: "{id}".to_string(),
                word: true,
            }]
        } else {
            self.formats.clone()
        }
    }
}

/// One way a document id may appear in code. `template` is rendered for a given
/// id with these placeholders: `{id}` (the full `PREFIX-NNNN`), `{prefix}` /
/// `{prefix_lower}`, `{num}` (the unpadded number), and `{padded}` (zero-padded
/// to `pad`). With `word` (the default) the match must fall on word boundaries;
/// set `word = false` to match anywhere (substring).
#[derive(Debug, Clone, Deserialize)]
pub struct RefFormat {
    pub template: String,
    #[serde(default = "default_true")]
    pub word: bool,
}

/// The placeholder names `{id}` / `{prefix}` / `{prefix_lower}` / `{num}` /
/// `{padded}` that a [`RefFormat`] template may use.
pub const REF_PLACEHOLDERS: [&str; 5] = ["id", "prefix", "prefix_lower", "num", "padded"];

/// One custom stats section for `opys stats`. `sql` is a single SQL query run
/// against an in-memory relational view of the corpus (tables `docs`, `tags`,
/// `sections`, `fields` — see [`crate::commands::stats`]). Without `template`
/// the result set renders as a markdown table headed by `name`; with `template`,
/// each result row is rendered through it (`{column}` placeholders, `{{`/`}}` for
/// literal braces) and the rendered rows are joined under the `name` heading.
#[derive(Debug, Clone, Deserialize)]
pub struct StatSpec {
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub template: Option<String>,
}

/// The `[types.*]` table: the declared document types, keyed by name *and* in
/// the order the TOML declared them.
///
/// Lookup is a `BTreeMap` and every read goes through [`Deref`], so this behaves
/// as the map it always was. What it adds is [`TypeMap::index_of`] — the
/// position of a type in the config, which is how `list --sort` groups a mixed
/// listing (a `BTreeMap` alone could only offer type-*name* order, which is an
/// order the config never asked for). Declaration order is the order serde
/// visits the entries in, which for TOML is document order.
#[derive(Debug, Clone, Default)]
pub struct TypeMap {
    by_name: BTreeMap<String, DocType>,
    /// The type names in declaration order — the same set as `by_name`'s keys.
    order: Vec<String>,
}

impl TypeMap {
    /// The position of `name` among the declared types, or `None` if no type
    /// goes by that name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.order.iter().position(|n| n == name)
    }

    /// The type names in declaration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// Consume the map, yielding the types in declaration order.
    pub fn into_values(self) -> impl Iterator<Item = DocType> {
        let mut by_name = self.by_name;
        self.order
            .into_iter()
            .filter_map(move |n| by_name.remove(&n))
    }
}

/// Iteration is the map's — by name, the order every other reader (validation,
/// `verify`, error messages) wants for a stable, alphabetical report. Only
/// `--sort`'s grouping asks for declaration order, and it asks by name through
/// [`TypeMap::index_of`].
impl<'a> IntoIterator for &'a TypeMap {
    type Item = (&'a String, &'a DocType);
    type IntoIter = std::collections::btree_map::Iter<'a, String, DocType>;

    fn into_iter(self) -> Self::IntoIter {
        self.by_name.iter()
    }
}

impl std::ops::Deref for TypeMap {
    type Target = BTreeMap<String, DocType>;

    fn deref(&self) -> &Self::Target {
        &self.by_name
    }
}

impl<'de> Deserialize<'de> for TypeMap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;

        impl<'de> serde::de::Visitor<'de> for V {
            type Value = TypeMap;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a table of document types")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut m: M,
            ) -> std::result::Result<TypeMap, M::Error> {
                let mut out = TypeMap::default();
                while let Some((name, t)) = m.next_entry::<String, DocType>()? {
                    if out.by_name.insert(name.clone(), t).is_none() {
                        out.order.push(name);
                    }
                }
                Ok(out)
            }
        }

        d.deserialize_map(V)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    /// Inventory base directory, relative to the project root (the dir holding
    /// `opys.toml`). Defaults to `opys`.
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_pad")]
    pub pad: usize,
    #[serde(default)]
    pub layout: Layout,
    /// Interleaved id allocation; see [`Ids`].
    #[serde(default)]
    pub ids: Ids,
    /// Which branch is truth and which refs the branch reader reads; see
    /// [`Git`].
    #[serde(default)]
    pub git: Git,
    #[serde(default)]
    pub types: TypeMap,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Custom stats sections rendered by `opys stats`. Each is a single `sql`
    /// query over the corpus view, rendered as a markdown table.
    #[serde(default)]
    pub stats: Vec<StatSpec>,
    /// How document ids are mentioned in code, for `show --refs` and the
    /// post-`renumber` reference warning. See [`FileRefs`].
    #[serde(default)]
    pub file_refs: FileRefs,
}

impl ProjectConfig {
    /// Load `opys.toml`, or a usage error pointing at `config init` when absent.
    pub fn load(path: &Path) -> Result<ProjectConfig> {
        if !path.exists() {
            return Err(usage(format!(
                "no opys.toml at {} — run `opys config init`",
                path.display()
            )));
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|source| OpysError::Toml {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The distinct directories (under the base) that hold documents — the set
    /// generic discovery scans. Multiple types may share a dir (assigned by id
    /// prefix at load).
    pub fn doc_dirs(&self) -> Vec<&str> {
        let mut dirs: Vec<&str> = self.types.values().map(DocType::resolved_dir).collect();
        dirs.sort_unstable();
        dirs.dedup();
        dirs
    }

    /// The directory holding the type whose prefix matches `id` (the default dir
    /// when the prefix matches no type).
    pub fn dir_for_id(&self, id: &str) -> &str {
        self.type_name_for_id(id)
            .and_then(|n| self.types.get(n))
            .map(DocType::resolved_dir)
            .unwrap_or(DEFAULT_DOC_DIR)
    }

    /// The inventory base as an absolute path under `root`, or an error.
    ///
    /// `base` is documented as *relative to the project root*, and an inventory
    /// that lives outside its own project has no meaning: every path the tool
    /// writes is `base`-relative, so a `base` that climbs out simply aims the
    /// whole corpus somewhere else. There is no use for it, and allowing it lets
    /// a config decide where the tool reads and writes — which matters most for
    /// a caller that did not author the config, like the node opening a project
    /// it was handed. So it is refused outright rather than made configurable.
    ///
    /// Two checks, because one is not enough. Lexically, `base` must be relative
    /// and free of `..`; that alone would still be fooled by a symlink, so when
    /// the directory already exists the resolved path is canonicalized and
    /// required to sit under the canonicalized root. A base that does not exist
    /// yet (the `init` path) gets the lexical check only — there is no symlink to
    /// resolve, and it will be checked the next time it is opened.
    ///
    /// Returns `root.join(base)` uncanonicalized, so paths the tool prints are
    /// spelled the way the user spelled them.
    pub fn resolve_base(&self, root: &Path) -> Result<PathBuf> {
        if let Some(problem) = base_escape(&self.base) {
            return Err(usage(format!(
                "opys.toml: base {:?} {problem} — the inventory must live inside the project (at or below {})",
                self.base,
                root.display()
            )));
        }
        let joined = root.join(&self.base);
        // Only meaningful once it exists; `canonicalize` fails otherwise.
        if joined.exists() {
            let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let real_base = std::fs::canonicalize(&joined).unwrap_or_else(|_| joined.clone());
            if !real_base.starts_with(&real_root) {
                return Err(usage(format!(
                    "opys.toml: base {:?} resolves to {}, outside the project at {} — the inventory must live inside the project",
                    self.base,
                    real_base.display(),
                    real_root.display()
                )));
            }
        }
        Ok(joined)
    }

    /// The canonical file path for a document, relative to the inventory base:
    /// the `layout.path` template with `{type}`/`{status}`/`{id}` substituted and
    /// empty path segments collapsed. The `{type}`/`{status}` segments come from
    /// the type matching `id`'s prefix (empty for an unknown prefix).
    pub fn doc_relpath(&self, id: &str, status: &str) -> PathBuf {
        let t = self.type_name_for_id(id).and_then(|n| self.types.get(n));
        let type_seg = t.map_or(DEFAULT_DOC_DIR, DocType::resolved_dir);
        let status_seg = t.map_or("", |t| t.status_dir(status));
        let rendered = self
            .layout
            .path
            .replace("{type}", type_seg)
            .replace("{status}", status_seg)
            .replace("{id}", id);
        // Collapse empty segments so unset `{type}`/`{status}` don't leave `//`.
        rendered.split('/').filter(|seg| !seg.is_empty()).collect()
    }

    /// The name of the type whose prefix matches `id`'s prefix, if any. (A doc's
    /// type is derived from its ID prefix — the ID is the single source of truth.)
    pub fn type_name_for_id(&self, id: &str) -> Option<&str> {
        let prefix = id.split_once('-')?.0;
        self.types
            .iter()
            .find(|(_, t)| t.prefix == prefix)
            .map(|(name, _)| name.as_str())
    }

    /// Check the config is well-formed, returning all problems (empty == OK).
    /// These are content problems (verify-style), not hard errors.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.types.is_empty() {
            errs.push("no document types defined ([types.<name>])".into());
        }

        // The same rule `resolve_base` enforces at open time, reported here so
        // `config validate` names it before anything tries to use it.
        if let Some(problem) = base_escape(&self.base) {
            errs.push(format!(
                "base {:?} {problem} — the inventory must live inside the project",
                self.base
            ));
        }

        // The same bounds `ids::resolve` enforces before any allocation: a
        // modulus of 0 or 1 would leave one slot (or none) for every allocator,
        // and one above `ids::MAX_MODULUS` strides ids past the store's `i64`.
        if let Some(m) = self.ids.modulus {
            if m < 2 {
                errs.push(format!(
                    "ids.modulus must be at least 2 (got {m}) — interleaving needs at least two slots"
                ));
            } else if m > crate::ids::MAX_MODULUS {
                errs.push(format!(
                    "ids.modulus must be at most {} (got {m}) — ids must stay within 64-bit numbers",
                    crate::ids::MAX_MODULUS
                ));
            }
        }

        // A truth branch is the baseline every ref is diffed against; an empty
        // name resolves to nothing, which would silently make every branch look
        // like the whole inventory.
        if self.git.main.trim().is_empty() {
            errs.push("git.main must name a branch (e.g. \"main\")".into());
        }
        // An empty `branches` list is legal — it reads no branch refs at all,
        // leaving worktree heads — but an empty *pattern* matches nothing and is
        // always a mistake.
        if self.git.branches.iter().any(|p| p.trim().is_empty()) {
            errs.push(
                "git.branches: empty pattern — name a ref glob (e.g. \"refs/heads/*\") \
                 or drop the entry"
                    .into(),
            );
        }

        // The layout template must place each document at a unique path: it must
        // contain `{id}` and use only the known placeholders.
        if !self.layout.path.contains("{id}") {
            errs.push("layout.path must contain the {id} placeholder".into());
        }
        for unknown in unknown_placeholders(&self.layout.path) {
            errs.push(format!(
                "layout.path: unknown placeholder '{{{unknown}}}' (use type/status/id)"
            ));
        }

        let prefix_re = Regex::new(r"^[A-Z][A-Z0-9]*$").unwrap();
        let type_names: HashSet<&str> = self.types.keys().map(String::as_str).collect();
        let mut seen_prefix: BTreeMap<&str, &str> = BTreeMap::new();

        for (name, t) in &self.types {
            if !prefix_re.is_match(&t.prefix) {
                errs.push(format!(
                    "type '{name}': prefix '{}' must match ^[A-Z][A-Z0-9]*$",
                    t.prefix
                ));
            }
            if let Some(prev) = seen_prefix.insert(&t.prefix, name) {
                errs.push(format!(
                    "type '{name}': prefix '{}' already used by type '{prev}'",
                    t.prefix
                ));
            }
            if t.statuses.is_empty() {
                errs.push(format!("type '{name}': statuses must be non-empty"));
            }
            if t.default_status.is_empty() {
                errs.push(format!("type '{name}': default_status is required"));
            } else if !t.statuses.contains(&t.default_status) {
                errs.push(format!(
                    "type '{name}': default_status '{}' not in statuses",
                    t.default_status
                ));
            }
            for s in &t.terminal_statuses {
                if !t.statuses.contains(s) {
                    errs.push(format!(
                        "type '{name}': terminal_status '{s}' not in statuses"
                    ));
                }
            }
            for s in t.status_dirs.keys() {
                if !t.statuses.contains(s) {
                    errs.push(format!(
                        "type '{name}': status_dirs key '{s}' is not a status"
                    ));
                }
            }
            // `releases_blocks` (ADR-0099 decision 3). A status the type cannot
            // hold releases nothing, so the entry is dead config — and the
            // silent failure mode is the bad one: every blocker of that type
            // stays live forever with no symptom to chase.
            for s in &t.releases_blocks {
                if !t.statuses.contains(s) {
                    errs.push(format!(
                        "type '{name}': releases_blocks '{s}' is not a status"
                    ));
                }
            }
            // `order_by` (ADR-0099 decision 4). Both problems are about an
            // *explicitly* declared chain: the implicit default silently drops
            // the priority key on a type without the field, because priority is
            // opt-in (ADR-0095) and the shipped config declares none.
            let mut seen_key: HashSet<&str> = HashSet::new();
            for key in &t.order_by {
                if !seen_key.insert(key.as_str()) {
                    errs.push(format!(
                        "type '{name}': order_by lists '{}' more than once",
                        key.as_str()
                    ));
                }
            }
            if t.order_by.contains(&crate::order::OrderKey::Priority)
                && !crate::order::has_priority_field(t)
            {
                errs.push(format!(
                    "type '{name}': order_by names 'priority' but the type declares no \
                     [types.{name}.fields.priority] with type = \"int\" (ADR-0095: priority \
                     is an opt-in declared field)"
                ));
            }
            // `transitions` (ADR-0099 decision 1). A map that names a status the
            // type does not declare is dead config: the edge can never fire, and
            // the status it guards is unreachable or unleavable in a way nobody
            // intended. A status absent from a non-empty map is *not* an error —
            // it is a dead end, which is a legitimate shape.
            for (from, targets) in &t.transitions {
                if !t.statuses.contains(from) {
                    errs.push(format!(
                        "type '{name}': transitions key '{from}' is not a status"
                    ));
                }
                // A terminal status is reached only through `close`, which
                // deletes the document — there is nothing left to move on.
                if t.terminal_statuses.contains(from) {
                    errs.push(format!(
                        "type '{name}': terminal status '{from}' must have no outgoing \
                         transitions (it is reached only via `close`, which deletes)"
                    ));
                }
                for to in targets {
                    if !t.statuses.contains(to) {
                        errs.push(format!(
                            "type '{name}': transition '{from}' -> '{to}' targets a status \
                             the type does not declare"
                        ));
                    }
                }
            }
            // The initial status must be able to go somewhere, or every document
            // of the type is stuck the moment `new` creates it. What matters is
            // the outgoing *edge*, not the key: `todo = []` is a key and still
            // leaves the whole corpus stranded.
            if !t.transitions.is_empty()
                && !t.default_status.is_empty()
                && t.transitions
                    .get(&t.default_status)
                    .is_none_or(|targets| targets.is_empty())
            {
                errs.push(format!(
                    "type '{name}': default_status '{}' has no outgoing transitions (every \
                     document starts there, so it would have nowhere to go)",
                    t.default_status
                ));
            }
            // `ttl` / `max_visits` (ADR-0101 decision 1): the two time-aware
            // observations `verify` reports. Both are keyed by a status and both
            // may name a `then` the project intends the document to move to, so
            // one routine checks both.
            for (from, ttl) in &t.ttl {
                validate_observation_key(name, t, "ttl", from, &mut errs);
                if crate::observe::parse_duration(&ttl.after).is_none() {
                    errs.push(format!(
                        "type '{name}': ttl '{from}': '{}' is not a duration (one or more \
                         <integer><unit> groups, units s/m/h/d/w/y — e.g. \"10m\", \"2d\", \
                         \"1h30m\")",
                        ttl.after
                    ));
                }
                if let Some(then) = &ttl.then {
                    validate_observation_target(name, t, "ttl", from, then, &mut errs);
                }
            }
            for (from, mv) in &t.max_visits {
                validate_observation_key(name, t, "max_visits", from, &mut errs);
                if mv.limit < 1 {
                    errs.push(format!(
                        "type '{name}': max_visits '{from}': limit must be at least 1 (got {})",
                        mv.limit
                    ));
                }
                if validate_observation_target(name, t, "max_visits", from, &mv.then, &mut errs) {
                    validate_escalation_edges(name, t, from, &mv.then, &mut errs);
                }
            }
            if let Some(lr) = &t.requires_link {
                if !type_names.contains(lr.to.as_str()) {
                    errs.push(format!(
                        "type '{name}': requires_link.to '{}' is not a defined type",
                        lr.to
                    ));
                }
            }
            for (fname, spec) in &t.fields {
                if spec.field_type == FieldType::Enum && spec.values.is_empty() {
                    errs.push(format!(
                        "type '{name}' field '{fname}': enum declares no values"
                    ));
                }
                if let Some(p) = &spec.pattern {
                    if Regex::new(p).is_err() {
                        errs.push(format!(
                            "type '{name}' field '{fname}': pattern is not a valid regex"
                        ));
                    }
                }
            }
            let mut seen_section: HashSet<&str> = HashSet::new();
            for sec in &t.sections {
                if !seen_section.insert(sec.heading.as_str()) {
                    errs.push(format!(
                        "type '{name}': duplicate section heading '{}'",
                        sec.heading
                    ));
                }
                for (ci, chk) in sec.checks.iter().enumerate() {
                    validate_check(name, &sec.heading, sec.kind, ci + 1, chk, &mut errs);
                }
                validate_structure(name, sec, &mut errs);
            }
        }

        for (i, rule) in self.rules.iter().enumerate() {
            self.validate_rule(i + 1, rule, &type_names, &mut errs);
        }

        self.validate_file_refs(&mut errs);
        self.validate_stats(&mut errs);
        errs
    }

    /// Validate `[[stats]]`: each `sql` must run against the corpus schema. We
    /// execute it over an empty corpus, which surfaces parse errors and unknown
    /// columns/tables at config time (a valid query just returns no rows).
    fn validate_stats(&self, errs: &mut Vec<String>) {
        let empty = serde_json::json!([]);
        for s in &self.stats {
            if let Err(e) = crate::commands::stats::render_stat(s, &empty) {
                errs.push(e);
            }
        }
    }

    /// Validate `[file_refs].formats`: every placeholder must be known, and each
    /// template must include a number placeholder (`{id}`/`{num}`/`{padded}`) so
    /// two ids sharing a prefix can't render to the same text.
    fn validate_file_refs(&self, errs: &mut Vec<String>) {
        for (i, f) in self.file_refs.formats.iter().enumerate() {
            for ph in placeholder_names(&f.template) {
                if !REF_PLACEHOLDERS.contains(&ph) {
                    errs.push(format!(
                        "file_refs.formats[{i}]: unknown placeholder '{{{ph}}}' (use {})",
                        REF_PLACEHOLDERS.join("/")
                    ));
                }
            }
            let numbered = ["{id}", "{num}", "{padded}"]
                .iter()
                .any(|p| f.template.contains(p));
            if !numbered {
                errs.push(format!(
                    "file_refs.formats[{i}]: template '{}' must include a number placeholder ({{id}}, {{num}}, or {{padded}})",
                    f.template
                ));
            }
        }
    }

    fn validate_rule(&self, n: usize, r: &Rule, types: &HashSet<&str>, errs: &mut Vec<String>) {
        let tag = format!("rule #{n}");
        match r.assertion_count() {
            1 => {}
            0 => errs.push(format!("{tag}: has no assertion")),
            _ => errs.push(format!("{tag}: has more than one assertion")),
        }

        // `when` guard resolves.
        if let Some(t) = &r.when.doc_type {
            if !types.contains(t.as_str()) {
                errs.push(format!("{tag}: when.type '{t}' is not a defined type"));
            } else if let Some(s) = &r.when.status {
                if !self.types[t].statuses.contains(s) {
                    errs.push(format!(
                        "{tag}: when.status '{s}' is not a status of type '{t}'"
                    ));
                }
            }
        }

        // `require_link.to` is a type.
        if let Some(lr) = &r.require_link {
            if !types.contains(lr.to.as_str()) {
                errs.push(format!(
                    "{tag}: require_link.to '{}' is not a defined type",
                    lr.to
                ));
            }
        }

        // `require_any` terms: exactly one key each; a `link` is a relation field.
        if let Some(terms) = &r.require_any {
            if terms.is_empty() {
                errs.push(format!("{tag}: require_any is empty"));
            }
            for term in terms {
                let count = [
                    term.field.is_some(),
                    term.link.is_some(),
                    term.section.is_some(),
                    term.tag.is_some(),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                if count != 1 {
                    errs.push(format!(
                        "{tag}: each require_any term needs exactly one of field/link/section/tag"
                    ));
                }
                if let Some(l) = &term.link {
                    if !refs::RELATION_FIELDS.contains(&l.as_str()) {
                        errs.push(format!(
                            "{tag}: require_any link '{l}' is not a relation field (references/blocked_by/blocks)"
                        ));
                    }
                }
            }
        }

        // Field/section assertions are resolvable against the named type.
        if let Some(t) = &r.when.doc_type {
            if let Some(dt) = self.types.get(t) {
                if let Some(f) = &r.require_field {
                    if !dt.fields.contains_key(f) {
                        errs.push(format!(
                            "{tag}: require_field '{f}' is not a field of type '{t}'"
                        ));
                    }
                }
                if let Some(sec) = &r.require_section {
                    if !dt.sections.iter().any(|s| &s.heading == sec) {
                        errs.push(format!(
                            "{tag}: require_section '{sec}' is not a section of type '{t}'"
                        ));
                    }
                }
                if let Some(sec) = &r.require_checked_section {
                    match dt.sections.iter().find(|s| &s.heading == sec) {
                        None => errs.push(format!(
                            "{tag}: require_checked_section '{sec}' is not a section of type '{t}'"
                        )),
                        Some(s) if !s.kind.is_checkable() => errs.push(format!(
                            "{tag}: require_checked_section '{sec}' targets a non-checklist section"
                        )),
                        _ => {}
                    }
                }
                if let Some(sec) = &r.require_log_since_status {
                    match dt.sections.iter().find(|s| &s.heading == sec) {
                        None => errs.push(format!(
                            "{tag}: require_log_since_status '{sec}' is not a section of type '{t}'"
                        )),
                        Some(s) if !s.kind.is_log() => errs.push(format!(
                            "{tag}: require_log_since_status '{sec}' targets a non-log section"
                        )),
                        _ => {}
                    }
                }
            }
        }

        // A `field_matches.pattern` must always compile.
        if let Some(fm) = &r.field_matches {
            if Regex::new(&fm.pattern).is_err() {
                errs.push(format!("{tag}: field_matches.pattern is not a valid regex"));
            }
            if let Some(t) = &r.when.doc_type {
                if let Some(dt) = self.types.get(t) {
                    if !dt.fields.contains_key(&fm.field) {
                        errs.push(format!(
                            "{tag}: field_matches.field '{}' is not a field of type '{t}'",
                            fm.field
                        ));
                    }
                }
            }
        }
    }
}

/// The key of a `[types.X.ttl]` / `[types.X.max_visits]` entry must be a status
/// the type declares, or the entry can never fire.
fn validate_observation_key(
    type_name: &str,
    t: &DocType,
    kind: &str,
    from: &str,
    errs: &mut Vec<String>,
) {
    if !t.statuses.contains(&from.to_string()) {
        errs.push(format!(
            "type '{type_name}': {kind} key '{from}' is not a status"
        ));
    }
}

/// The `then` of an observation is where the project intends the document to go
/// (ADR-0101 decision 6). opys never performs that move — but a target nothing
/// could ever move the document to is config the project cannot mean, so it is
/// checked here rather than discovered on the day the threshold trips:
///
/// - a declared, non-terminal status other than the key itself (a terminal one
///   is reached only through `close`, which `set-status` refuses outright, so
///   the intended move could never be made);
/// - when the type declares a status graph, the edge `from -> then` must exist,
///   because the intended move is an ordinary `set-status`; and the **recovery
///   edge** `then -> from` must exist too, so a document moved there under a
///   running agent is one `set-status` away from where it was.
///
/// A type with no graph is exempt from the last two: it allows every move
/// already, which is what [`crate::lifecycle::allows`] answers.
///
/// Returns whether `then` is usable at all, so a caller can skip the further
/// checks that only make sense for a target that resolves.
fn validate_observation_target(
    type_name: &str,
    t: &DocType,
    kind: &str,
    from: &str,
    then: &str,
    errs: &mut Vec<String>,
) -> bool {
    if !t.statuses.contains(&then.to_string()) {
        errs.push(format!(
            "type '{type_name}': {kind} '{from}' -> '{then}' targets a status the type does \
             not declare"
        ));
        return false;
    }
    if then == from {
        errs.push(format!(
            "type '{type_name}': {kind} '{from}' -> '{then}' must move the document somewhere \
             else"
        ));
        return false;
    }
    if t.terminal_statuses.iter().any(|s| s == then) {
        errs.push(format!(
            "type '{type_name}': {kind} '{from}' -> '{then}' targets a terminal status (it is \
             reached only via `close`, which deletes)"
        ));
        return false;
    }
    if t.transitions.is_empty() {
        return true;
    }
    let mut ok = true;
    if !crate::lifecycle::allows(t, from, then) {
        errs.push(format!(
            "type '{type_name}': {kind} '{from}' -> '{then}' is not a declared transition \
             (the move the config names must be one)"
        ));
        ok = false;
    }
    if !crate::lifecycle::allows(t, then, from) {
        errs.push(format!(
            "type '{type_name}': {kind} '{from}' -> '{then}' has no way back ('{then}' declares \
             no transition to '{from}' — the recovery edge a move under a running agent relies \
             on)"
        ));
        ok = false;
    }
    ok
}

/// The **escalation edges** of a `max_visits` budget, checked only for that kind
/// (ADR-0101 decision 6).
///
/// A `ttl` fires only while the document holds the key status, so its `then` is
/// only ever entered from there. A visit budget is different: `set-status`
/// refuses the *entry* into `from`, which happens from any status with an edge
/// into it, and the refusal tells the caller to go to `then` instead. That
/// advice has to be a legal move from wherever the document stands, or the
/// document is barred from `from` for good with the advertised escape out of
/// reach — and verify sees nothing to report either, because
/// [`crate::observe::overflow`] keys on the status the document *holds*.
///
/// So every predecessor of `from` must also declare an edge to `then`. `then`
/// itself is exempt (a document already there needs no move), and so is `from`
/// (the configured edge `from -> then` is checked above).
fn validate_escalation_edges(
    type_name: &str,
    t: &DocType,
    from: &str,
    then: &str,
    errs: &mut Vec<String>,
) {
    if t.transitions.is_empty() {
        return;
    }
    for (pred, targets) in &t.transitions {
        if pred == then || pred == from || !targets.iter().any(|s| s == from) {
            continue;
        }
        if !crate::lifecycle::allows(t, pred, then) {
            errs.push(format!(
                "type '{type_name}': max_visits '{from}' -> '{then}': a document at '{pred}' \
                 may enter '{from}' but not '{then}' — once the budget is spent the refusal \
                 would name an escape it cannot take"
            ));
        }
    }
}

/// Validate a section's `structure`: only a `structured` section may declare it,
/// and then it is required and must parse as an [`mdprism`] schema.
fn validate_structure(type_name: &str, sec: &SectionSpec, errs: &mut Vec<String>) {
    let tag = format!("type '{type_name}' section '{}'", sec.heading);
    if sec.kind != SectionKind::Structured {
        if sec.structure.is_some() {
            errs.push(format!(
                "{tag}: 'structure' is only allowed on a 'structured' section"
            ));
        }
        return;
    }
    match &sec.structure {
        None => errs.push(format!("{tag}: a 'structured' section needs a 'structure'")),
        Some(src) => {
            if let Err(e) = crate::mdprism::parse_schema(src) {
                errs.push(format!("{tag}: invalid structure ({e})"));
            }
        }
    }
}

/// Validate one [`SectionCheck`]: its `pattern` compiles, `file` / every
/// `${group}` reference name real capture groups, at least one of `file` /
/// `must_match` is set, `must_match` compiles, and `scope = "checked"` only
/// targets a checklist section.
fn validate_check(
    type_name: &str,
    heading: &str,
    kind: SectionKind,
    n: usize,
    chk: &SectionCheck,
    errs: &mut Vec<String>,
) {
    let tag = format!("type '{type_name}' section '{heading}' check #{n}");
    let names: HashSet<String> = match Regex::new(&chk.pattern) {
        Ok(re) => re
            .capture_names()
            .flatten()
            .map(|s| s.to_string())
            .collect(),
        Err(_) => {
            errs.push(format!("{tag}: pattern is not a valid regex"));
            return;
        }
    };

    if let Some(f) = &chk.file {
        if !names.contains(f) {
            errs.push(format!(
                "{tag}: file '{f}' is not a named capture group of pattern"
            ));
        }
    }
    if chk.file.is_none() && chk.must_match.is_none() {
        errs.push(format!("{tag}: needs at least one of file / must_match"));
    }

    let group_re = Regex::new(r"\$\{(\w+)\}").unwrap();
    for tmpl in [chk.must_match.as_deref(), chk.message.as_deref()]
        .into_iter()
        .flatten()
    {
        for c in group_re.captures_iter(tmpl) {
            let g = &c[1];
            if !names.contains(g) {
                errs.push(format!(
                    "{tag}: ${{{g}}} is not a named capture group of pattern"
                ));
            }
        }
    }
    if let Some(mm) = &chk.must_match {
        let probe = group_re.replace_all(mm, "x");
        if Regex::new(&probe).is_err() {
            errs.push(format!("{tag}: must_match is not a valid regex"));
        }
    }
    if chk.scope == CheckScope::Checked && !kind.is_checkable() {
        errs.push(format!(
            "{tag}: scope = \"checked\" requires a checklist section"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::DEFAULT_OPYS_CONFIG;

    #[test]
    fn default_config_validates_clean() {
        let cfg: ProjectConfig = toml::from_str(DEFAULT_OPYS_CONFIG).unwrap();
        let problems = cfg.validate();
        assert!(
            problems.is_empty(),
            "default config has problems: {problems:?}"
        );
        assert_eq!(cfg.types.len(), 4);
    }

    /// Declaration order survives the parse, and it is not name order: the
    /// types below are declared in an order a `BTreeMap` would reshuffle, and
    /// `index_of` reports the order the file is written in. `list --sort`
    /// groups a mixed listing by exactly this, so an alphabetising map would
    /// silently reorder every mixed board.
    #[test]
    fn types_remember_the_order_the_config_declares_them_in() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.zeta]
prefix = "Z"
statuses = ["open"]
default_status = "open"

[types.alpha]
prefix = "A"
statuses = ["open"]
default_status = "open"

[types.mid]
prefix = "M"
statuses = ["open"]
default_status = "open"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.types.names().collect::<Vec<_>>(),
            ["zeta", "alpha", "mid"]
        );
        assert_eq!(cfg.types.index_of("zeta"), Some(0));
        assert_eq!(cfg.types.index_of("alpha"), Some(1));
        assert_eq!(cfg.types.index_of("mid"), Some(2));
        assert_eq!(cfg.types.index_of("nosuch"), None);
        // Still the map it always was: lookup by name, keys alphabetical.
        assert_eq!(cfg.types["alpha"].prefix, "A");
        assert_eq!(
            cfg.types.keys().map(String::as_str).collect::<Vec<_>>(),
            ["alpha", "mid", "zeta"]
        );
        assert_eq!(
            cfg.types
                .clone()
                .into_values()
                .map(|t| t.prefix)
                .collect::<Vec<_>>(),
            ["Z", "A", "M"]
        );
    }

    /// The shipped config leads with `feature`, and the repo's own dogfooding
    /// config puts `adr` second — both orders the file states rather than the
    /// alphabet's.
    #[test]
    fn the_default_config_declares_feature_first() {
        let cfg: ProjectConfig = toml::from_str(DEFAULT_OPYS_CONFIG).unwrap();
        assert_eq!(cfg.types.names().next(), Some("feature"));
    }

    /// An explicitly declared chain is held to what the type can answer: the
    /// `priority` key needs the `int` field ADR-0095 defines, and a key listed
    /// twice is a chain whose author meant something else.
    #[test]
    fn flags_an_order_by_the_type_cannot_answer() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "done"]
default_status = "planned"
order_by = ["priority", "id", "id"]

[types.feature.fields.priority]
type = "enum"
values = ["low", "high"]
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(
            joined.contains(
                "type 'feature': order_by names 'priority' but the type declares no \
                 [types.feature.fields.priority] with type = \"int\""
            ),
            "{joined}"
        );
        assert!(
            joined.contains("type 'feature': order_by lists 'id' more than once"),
            "{joined}"
        );
    }

    /// The implicit default is *not* held to the same rule: it drops the
    /// priority key on a type without the field. That carve-out is what keeps
    /// every config that never heard of priority — the shipped one included —
    /// valid, which `default_config_validates_clean` pins from the other side.
    #[test]
    fn the_implicit_default_chain_never_demands_a_priority_field() {
        let cfg: ProjectConfig = toml::from_str(
            "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
             default_status = \"planned\"\n",
        )
        .unwrap();
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
        let t = cfg.types.get("feature").unwrap();
        assert_eq!(
            crate::order::chain(t),
            vec![crate::order::OrderKey::Blocks, crate::order::OrderKey::Id]
        );
    }

    /// Every name a `transitions` map uses must be a status the type declares,
    /// no terminal status may lead anywhere (it is reached only via `close`,
    /// which deletes), and the status every document starts at must lead
    /// somewhere — otherwise the type's whole corpus is stuck the moment `new`
    /// runs.
    #[test]
    fn flags_broken_transitions_maps() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "partial", "done"]
default_status = "planned"
terminal_statuses = ["done"]

[types.feature.transitions]
partial = ["shipped", "done"]
shipped = ["partial"]
done = ["planned"]
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(
            joined.contains("type 'feature': transitions key 'shipped' is not a status"),
            "{joined}"
        );
        assert!(
            joined.contains(
                "type 'feature': transition 'partial' -> 'shipped' targets a status the type \
                 does not declare"
            ),
            "{joined}"
        );
        assert!(
            joined.contains(
                "type 'feature': terminal status 'done' must have no outgoing transitions"
            ),
            "{joined}"
        );
        assert!(
            joined.contains("type 'feature': default_status 'planned' has no outgoing transitions"),
            "{joined}"
        );
    }

    /// Both `ttl` spellings land in one shape: a bare duration is observed and
    /// nothing more, a table names where the project intends the document to
    /// go. The duration stays unparsed here — a typo is a `validate` problem,
    /// not a parse error that would change the exit code of every command.
    #[test]
    fn ttl_accepts_a_bare_duration_and_a_table() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "stuck"]
default_status = "todo"

[types.task.ttl]
todo = "2d"
doing = { after = "10m", then = "stuck" }

[types.task.max_visits]
doing = { limit = 3, then = "stuck" }
"#,
        )
        .unwrap();
        let t = &cfg.types["task"];
        assert_eq!(
            t.ttl["todo"],
            Ttl {
                after: "2d".to_string(),
                then: None
            }
        );
        assert_eq!(
            t.ttl["doing"],
            Ttl {
                after: "10m".to_string(),
                then: Some("stuck".to_string())
            }
        );
        assert_eq!(
            t.max_visits["doing"],
            MaxVisits {
                limit: 3,
                then: "stuck".to_string()
            }
        );
    }

    /// Every way a `ttl`/`max_visits` entry can be config the project cannot
    /// mean, including the recovery edge a move under a running agent relies on
    /// (ADR-0101 decision 6).
    #[test]
    fn flags_broken_ttl_and_max_visits_maps() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "stuck", "done"]
default_status = "todo"
terminal_statuses = ["done"]

[types.task.transitions]
todo = ["doing"]
doing = ["review", "stuck"]
review = ["doing", "done"]
stuck = ["todo"]

[types.task.ttl]
idle = "10m"
todo = "ten minutes"
doing = { after = "10m", then = "stuck" }
review = { limit = 1, after = "1d", then = "todo" }

[types.task.max_visits]
doing = { limit = 2, then = "review" }
review = { limit = 0, then = "done" }
stuck = { limit = 2, then = "stuck" }
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        for want in [
            "type 'task': ttl key 'idle' is not a status",
            "type 'task': ttl 'todo': 'ten minutes' is not a duration",
            // `doing -> stuck` is declared, but `stuck` only goes back to `todo`.
            "type 'task': ttl 'doing' -> 'stuck' has no way back ('stuck' declares no \
             transition to 'doing'",
            "type 'task': ttl 'review' -> 'todo' is not a declared transition",
            "type 'task': max_visits 'review': limit must be at least 1 (got 0)",
            "type 'task': max_visits 'review' -> 'done' targets a terminal status",
            "type 'task': max_visits 'stuck' -> 'stuck' must move the document somewhere else",
            // The configured edge `doing -> review` and the way back both
            // exist, but `todo` may enter `doing` and could not take the escape.
            "type 'task': max_visits 'doing' -> 'review': a document at 'todo' may enter \
             'doing' but not 'review' — once the budget is spent the refusal would name an \
             escape it cannot take",
        ] {
            assert!(joined.contains(want), "missing {want:?} in\n{joined}");
        }
        // An unknown key inside the table is ignored, not a parse error: the
        // entry is simply observe-only, which verify shows.
        let cfg: ProjectConfig = toml::from_str(
            "[types.task]\nprefix = \"TASK\"\nstatuses = [\"todo\"]\n\
             default_status = \"todo\"\n[types.task.ttl]\n\
             todo = { after = \"1d\", thenn = \"todo\" }\n",
        )
        .unwrap();
        assert_eq!(cfg.types["task"].ttl["todo"].then, None);
    }

    /// A key is not an edge: a `default_status` that is present in the map but
    /// leads nowhere strands every document of the type just as surely as one
    /// the map never mentions, so it is the same problem with the same message.
    #[test]
    fn flags_an_initial_status_whose_only_entry_is_empty() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]

[types.task.transitions]
todo = []
doing = ["todo", "done"]
"#,
        )
        .unwrap();
        let problems = cfg.validate();
        assert_eq!(
            problems,
            vec![
                "type 'task': default_status 'todo' has no outgoing transitions (every document \
                 starts there, so it would have nowhere to go)"
                    .to_string()
            ],
        );
    }

    /// The two shapes that look like mistakes and are not: a status absent from
    /// the map (a dead end — `superseded` is where an ADR stops), and a type with
    /// no map at all (the any-to-any rule every config predating ADR-0099 has).
    #[test]
    fn a_dead_end_and_a_missing_map_are_both_legal() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.adr]
prefix = "ADR"
statuses = ["proposed", "accepted", "superseded"]
default_status = "proposed"

[types.adr.transitions]
proposed = ["accepted", "superseded"]
accepted = ["superseded"]

[types.note]
prefix = "NOTE"
statuses = ["open", "shut"]
default_status = "open"
"#,
        )
        .unwrap();
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
    }

    /// `releases_blocks` naming a status the type cannot hold is dead config
    /// with no symptom — every blocker of that type would simply stay live — so
    /// validate names it. An empty set is legal and means "never releases",
    /// which is exactly what an ephemeral type wants.
    #[test]
    fn flags_a_releases_blocks_status_the_type_does_not_declare() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented"]
default_status = "planned"
releases_blocks = ["implemented", "shipped"]

[types.task]
prefix = "TASK"
statuses = ["todo", "done"]
default_status = "todo"
terminal_statuses = ["done"]
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.validate(),
            vec!["type 'feature': releases_blocks 'shipped' is not a status".to_string()],
        );
        assert!(cfg.types["task"].releases_blocks.is_empty());
    }

    #[test]
    fn git_defaults_to_main_and_local_heads_and_rejects_a_typo() {
        // No `[git]` table at all: the reader still knows what truth is and
        // which refs count, and remote-tracking refs stay opt-in.
        let cfg: ProjectConfig = toml::from_str(
            "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
             default_status = \"planned\"\n",
        )
        .unwrap();
        assert_eq!(cfg.git.main, "main");
        assert_eq!(cfg.git.branches, ["refs/heads/*"]);

        // A misspelled key is a parse error, not a silently ignored table: it
        // would otherwise change what "truth" means with nothing to report it
        // (`Project::open` never runs `validate`).
        let err = toml::from_str::<ProjectConfig>(
            "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
             default_status = \"planned\"\n[git]\nmian = \"trunk\"\n",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `mian`"), "{err}");
        // The message names the two keys the table accepts, which is what
        // format.md's `[git]` section promises.
        assert!(err.contains("`main`"), "{err}");
        assert!(err.contains("`branches`"), "{err}");
    }

    #[test]
    fn flags_git_table_problems() {
        let cfg: ProjectConfig = toml::from_str(
            "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\n\
             default_status = \"planned\"\n[git]\nmain = \"\"\n\
             branches = [\"refs/heads/*\", \"\"]\n",
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(joined.contains("git.main must name a branch"), "{joined}");
        assert!(joined.contains("git.branches: empty pattern"), "{joined}");
    }

    #[test]
    fn flags_structured_structure_problems() {
        // A structured section with no structure, a structured section with an
        // invalid structure, and a `structure` on a non-structured section are
        // all rejected.
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "planned"

[[types.feature.sections]]
heading = "Empty"
kind = "structured"

[[types.feature.sections]]
heading = "Bad"
kind = "structured"
structure = "this is not a marker"

[[types.feature.sections]]
heading = "Notes"
kind = "prose"
structure = "- @x"
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(
            joined.contains("section 'Empty': a 'structured' section needs a 'structure'"),
            "{joined}"
        );
        assert!(
            joined.contains("section 'Bad': invalid structure"),
            "{joined}"
        );
        assert!(
            joined
                .contains("section 'Notes': 'structure' is only allowed on a 'structured' section"),
            "{joined}"
        );
    }

    #[test]
    fn flags_a_modulus_outside_the_bounds_and_accepts_a_real_one() {
        let base = "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\ndefault_status = \"planned\"\n";
        for m in [0u64, 1] {
            let cfg: ProjectConfig =
                toml::from_str(&format!("{base}[ids]\nmodulus = {m}\n")).unwrap();
            let joined = cfg.validate().join("\n");
            assert!(
                joined.contains(&format!("ids.modulus must be at least 2 (got {m})")),
                "{joined}"
            );
        }
        // Above the cap, an id would stride past the store's i64 and wrap
        // into a reusable number; the cap itself is accepted.
        for m in [crate::ids::MAX_MODULUS + 1, i64::MAX as u64] {
            let cfg: ProjectConfig =
                toml::from_str(&format!("{base}[ids]\nmodulus = {m}\n")).unwrap();
            let joined = cfg.validate().join("\n");
            assert!(
                joined.contains(&format!("ids.modulus must be at most 4294967296 (got {m})")),
                "{joined}"
            );
        }
        for m in [16, crate::ids::MAX_MODULUS] {
            let cfg: ProjectConfig =
                toml::from_str(&format!("{base}[ids]\nmodulus = {m}\n")).unwrap();
            assert_eq!(cfg.ids.modulus, Some(m));
            assert!(cfg.validate().is_empty());
        }
        // Absent: no modulus, so allocation stays the dense sequence.
        let cfg: ProjectConfig = toml::from_str(base).unwrap();
        assert_eq!(cfg.ids.modulus, None);
    }

    /// `[ids]` accepts only `modulus`: a misspelled key is a parse error, not a
    /// silently dense project. An empty table still means "no modulus".
    #[test]
    fn rejects_an_unknown_ids_key_at_parse_time() {
        let base = "[types.feature]\nprefix = \"FEAT\"\nstatuses = [\"planned\"]\ndefault_status = \"planned\"\n";
        let err = toml::from_str::<ProjectConfig>(&format!("{base}[ids]\nmodulos = 16\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `modulos`"), "{err}");
        assert!(err.contains("expected `modulus`"), "{err}");
        let cfg: ProjectConfig = toml::from_str(&format!("{base}[ids]\n")).unwrap();
        assert_eq!(cfg.ids.modulus, None);
    }

    #[test]
    fn flags_bad_default_status_and_duplicate_prefix_and_unknown_rule_type() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "nope"

[types.bug]
prefix = "FEAT"
statuses = ["todo"]
default_status = "todo"

[[rules]]
when = { type = "ghost" }
require_field = "x"
"#,
        )
        .unwrap();
        let problems = cfg.validate();
        let joined = problems.join("\n");
        assert!(
            joined.contains("default_status 'nope' not in statuses"),
            "{joined}"
        );
        assert!(joined.contains("already used by type"), "{joined}");
        assert!(
            joined.contains("when.type 'ghost' is not a defined type"),
            "{joined}"
        );
    }

    #[test]
    fn base_must_stay_inside_the_project() {
        // Fine: a child dir, a nested child, and the root itself.
        assert!(base_escape("inventory").is_none());
        assert!(base_escape("docs/inventory").is_none());
        assert!(base_escape(".").is_none());
        assert!(base_escape("./inventory").is_none());

        // Refused: climbing out, absolute, or nothing at all.
        assert!(base_escape("..").is_some());
        assert!(base_escape("../victim").is_some());
        assert!(base_escape("inventory/../../victim").is_some());
        assert!(base_escape("/etc").is_some());
        assert!(base_escape("").is_some());
    }

    #[test]
    fn validate_reports_an_escaping_base() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
base = "../victim"

[types.feature]
prefix = "FEAT"
statuses = ["todo"]
default_status = "todo"
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(joined.contains("must live inside the project"), "{joined}");
    }

    #[test]
    fn resolve_base_refuses_a_symlink_that_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        let outside = tmp.path().join("victim");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Lexically innocent — a single normal component — but it points out.
        std::os::unix::fs::symlink(&outside, root.join("inventory")).unwrap();

        let cfg: ProjectConfig = toml::from_str(
            r#"
base = "inventory"

[types.feature]
prefix = "FEAT"
statuses = ["todo"]
default_status = "todo"
"#,
        )
        .unwrap();
        assert!(base_escape(&cfg.base).is_none(), "lexically fine");
        let err = cfg.resolve_base(&root).unwrap_err().to_string();
        assert!(err.contains("outside the project"), "{err}");
    }

    #[test]
    fn resolve_base_allows_a_base_that_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg: ProjectConfig = toml::from_str(
            r#"
base = "inventory"

[types.feature]
prefix = "FEAT"
statuses = ["todo"]
default_status = "todo"
"#,
        )
        .unwrap();
        // `init` writes the config before the directory exists.
        let got = cfg.resolve_base(tmp.path()).unwrap();
        assert_eq!(got, tmp.path().join("inventory"));
    }
}
