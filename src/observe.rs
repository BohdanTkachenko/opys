//! Time-aware observations: **stuck** and **overflowed** documents, the clock
//! they are measured against, and the fixes `verify --fix` performs.
//!
//! Two things an agent loop has to notice are not content problems (ADR-0052).
//! A document can sit in a status past any reasonable time for the project,
//! which means whoever took it is gone; and a document can bounce between two
//! statuses forever, because the coder and the reviewer disagree and neither is
//! a person. Neither is a snapshot invariant: a corpus is valid when written and
//! *becomes* stuck with no write at all, so neither may be a `verify` problem —
//! they are reported apart, under their own heading, and carry their own exit
//! code (`3`).
//!
//! Both read [`crate::history`], the lifecycle map: its last key is when the
//! document entered its current status, and the number of entries recording a
//! status is how often it has been there. Both thresholds are per-type,
//! per-status config ([`DocType::ttl`], [`DocType::max_visits`]) — a fast
//! repository wants minutes where a "ship the game" feature may legitimately sit
//! for two years, so a constant would be wrong for every project but one.
//!
//! This module is the **one** place the wall clock is read ([`now`], which
//! honours `OPYS_NOW`) and the one place the two predicates live, so `verify`,
//! [`opys next`](crate::next) and the node's flow view can never disagree about
//! who is stuck.
//!
//! One reading is worth spelling out, because the word "exceeds" is ambiguous
//! and the whole feature turns on it: `max_visits = { limit = N }` allows N
//! entries into the status, and a document that has made its N-th **is at its
//! limit** — [`overflow`] reports it and `set-status` refuses another entry. The
//! two trip at the same count on purpose: a refusal a user cannot act on, with
//! no observation naming where the document should go instead, would be a dead
//! end rather than the escape hatch ADR-0052 asks for.
//!
//! That reading only holds because *nothing* writes an entry past the limit,
//! [`apply_fixes`] included: a fix is an ordinary `set-status` under the same
//! guard, so the highest count opys can produce is `limit` itself.

use std::fmt;

use serde::Serialize;
use time::{Duration, OffsetDateTime};

use crate::doc::Doc;
use crate::project::Project;
use crate::project_config::DocType;
use crate::store::Store;

/// The raw `OPYS_NOW` override, when it is set and non-empty.
///
/// **The** read of that variable. [`crate::commands::now_rfc3339`] and
/// [`crate::commands::now_utc_micros`] go through here too, so a test pins one
/// clock and every timestamp — `created`, `updated`, the history key, and the
/// instant these observations measure against — follows it. They keep their own
/// formatting: `now_rfc3339` returns the override *verbatim* (a value carrying
/// an offset stays in that offset in `created`), which re-rendering a parsed
/// instant would quietly change.
pub fn override_str() -> Option<String> {
    std::env::var("OPYS_NOW").ok().filter(|s| !s.is_empty())
}

/// Now, as an instant: `OPYS_NOW` when it parses as RFC3339, else the wall clock
/// (local, falling back to UTC). An unparseable override falls back too — the
/// same rule [`crate::commands::now_utc_micros`] has always applied.
pub fn now() -> OffsetDateTime {
    override_str()
        .and_then(|s| crate::history::parse(&s))
        .unwrap_or_else(|| {
            OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
        })
}

/// Parse a config duration: one or more `<integer><unit>` groups, units `s`,
/// `m`, `h`, `d`, `w`, `y` (`w` = 7 days, `y` = 365 days) — `"10m"`, `"2d"`,
/// `"1h30m"`, `"2y"`. `None` for anything else: an empty string, a bare number,
/// a bare unit, whitespace, a sign, an unknown unit, or a value so large it
/// overflows the seconds count.
///
/// Deliberately in-tree and deliberately strict. It is small enough not to be
/// worth a dependency, and a config duration is read by humans as often as by
/// this parser, so "1 hour" quietly meaning one *minute* (a lenient parser's
/// classic) is the one outcome worth refusing outright.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let mut total: i64 = 0;
    let mut pending: Option<i64> = None;
    let mut groups = 0usize;
    for ch in s.chars() {
        if let Some(d) = ch.to_digit(10) {
            pending = Some(
                pending
                    .unwrap_or(0)
                    .checked_mul(10)?
                    .checked_add(i64::from(d))?,
            );
            continue;
        }
        let n = pending.take()?;
        let unit: i64 = match ch {
            's' => 1,
            'm' => 60,
            'h' => 60 * 60,
            'd' => 24 * 60 * 60,
            'w' => 7 * 24 * 60 * 60,
            'y' => 365 * 24 * 60 * 60,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(unit)?)?;
        groups += 1;
    }
    // A trailing number without a unit ("90", "1h30") is not a duration.
    if pending.is_some() || groups == 0 {
        return None;
    }
    Some(Duration::seconds(total))
}

/// What was observed about a document.
///
/// Serialized internally tagged (`"kind": "stuck"` beside the variant's own
/// fields), because [`Observation`] flattens it: `opys next --json` and the
/// node's endpoint hand a dispatcher one flat object per observation rather
/// than a wrapper it has to unwrap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Kind {
    /// Shape or claim-age observation on a declared work copy; never auto-fixed.
    WorkBranch { message: String },
    /// The document has held `status` since `since` (a history key), longer than
    /// the type's `ttl` of `after` (the configured spelling, echoed verbatim so
    /// the message never renders a duration back differently than it was
    /// written).
    Stuck {
        status: String,
        since: String,
        after: String,
    },
    /// The document has entered `status` `visits` times, reaching the type's
    /// `limit`.
    Overflow {
        status: String,
        visits: usize,
        limit: usize,
    },
}

/// One observation about one document: what was seen, and where `verify --fix`
/// would move it (`None` for a `ttl` written as a bare duration — that one is
/// reported and nothing more).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Observation {
    pub id: String,
    #[serde(flatten)]
    pub kind: Kind,
    pub target: Option<String>,
}

impl fmt::Display for Observation {
    /// The line `verify`, `opys next` and the node all print. Byte-stable: the
    /// history key and the configured duration are echoed as written.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fix = match &self.target {
            Some(t) => format!("`--fix` moves it to {t}"),
            None => "no ttl target, nothing to fix".to_string(),
        };
        match &self.kind {
            Kind::WorkBranch { message } => write!(f, "{}: {message}", self.id),
            Kind::Stuck {
                status,
                since,
                after,
            } => write!(
                f,
                "{}: stuck in {status} since {since} (ttl {after}) — {fix}",
                self.id
            ),
            Kind::Overflow {
                status,
                visits,
                limit,
            } => write!(
                f,
                "{}: {visits} visit{} to {status} reaches its limit of {limit} — {fix}",
                self.id,
                if *visits == 1 { "" } else { "s" }
            ),
        }
    }
}

/// The document has sat in its status longer than the type's `ttl` for it.
///
/// Measured from the **last** history key, which is when the document entered
/// the status it holds. Strictly greater, so a clock exactly `after` past that
/// key is not yet stuck — the boundary is then a property of the config rather
/// than of how the two instants happen to round.
///
/// `None` when the type sets no `ttl` for the status, the document has no
/// readable history, or the configured duration does not parse (all three are
/// reported elsewhere: the first is simply not configured, the second is a
/// `verify` problem, the third a `config validate` one).
pub fn stuck(doc: &Doc, t: &DocType, now: OffsetDateTime) -> Option<Observation> {
    let id = doc.id()?;
    let status = doc.status()?;
    let ttl = t.ttl.get(status)?;
    let after = parse_duration(&ttl.after)?;
    let entries = crate::history::entries(&doc.frontmatter);
    let (since, _) = entries.last()?;
    let entered = crate::history::parse(since)?;
    if now - entered <= after {
        return None;
    }
    Some(Observation {
        id: id.to_string(),
        kind: Kind::Stuck {
            status: status.to_string(),
            since: since.clone(),
            after: ttl.after.clone(),
        },
        target: ttl.then.clone(),
    })
}

/// The document has entered its current status as often as the type's
/// `max_visits` allows.
///
/// Only the status the document holds *now* is considered: that is the one a fix
/// moves it out of, and the one whose `then` edge `config validate` checked.
/// Fires at `visits >= limit` — see the module docs for why the guard and the
/// observation trip together.
pub fn overflow(doc: &Doc, t: &DocType) -> Option<Observation> {
    let id = doc.id()?;
    let status = doc.status()?;
    let mv = t.max_visits.get(status)?;
    if mv.limit == 0 {
        // A meaningless limit; `config validate` reports it. Observing every
        // document of the type would drown that message in noise.
        return None;
    }
    let visits = visits(doc, status);
    if visits < mv.limit {
        return None;
    }
    Some(Observation {
        id: id.to_string(),
        kind: Kind::Overflow {
            status: status.to_string(),
            visits,
            limit: mv.limit,
        },
        target: Some(mv.then.clone()),
    })
}

/// How many times the document's history records it entering `status`.
pub fn visits(doc: &Doc, status: &str) -> usize {
    crate::history::entries(&doc.frontmatter)
        .iter()
        .filter(|(_, s)| s == status)
        .count()
}

/// Every observation over `docs`, ordered by id number then id.
///
/// The corpus-wide pass, and `verify`'s only reader of it. `opys next` does
/// **not** come through here: [`crate::next`] composes the same two predicates
/// per item ([`overflow`] then [`stuck`], in `next::item`), because an item
/// carries only its own observations and this function's whole-corpus sort would
/// be the wrong one for a ranked list. The two agree by convention, not by
/// construction — a new [`Kind`], or a different per-document order, has to be
/// made in both places. Documents whose id prefix belongs to no type are
/// skipped — `verify::collect_problems` already reports them, and a type is what
/// carries the thresholds.
///
/// The order is by id rather than by load order because load order is *path*
/// order, and a `--fix` that relocates a document changes its path: sorting by
/// id keeps the block byte-stable across the fix it just performed. Per
/// document, an overflow is listed before a stuck — the order `apply_fixes`
/// resolves them in.
pub fn observations(
    prj: &Project,
    docs: &[Doc],
    now: OffsetDateTime,
) -> crate::error::Result<Vec<Observation>> {
    let view = crate::work::effective(prj, docs, now);
    view.ensure_main()?;
    let mut out = view.observations;
    for doc in docs {
        let Some(id) = doc.id() else { continue };
        // A main-side timeout/visit budget cannot move a document while its
        // declared branch holds it. Work-copy observations have no fix target.
        if view.refs.contains_key(id) {
            continue;
        }

        let Some(tname) = prj.pcfg.type_name_for_id(id) else {
            continue;
        };
        let t = &prj.pcfg.types[tname];
        out.extend(overflow(doc, t));
        out.extend(stuck(doc, t, now));
    }
    out.sort_by(|a, b| {
        crate::refs::id_number(&a.id)
            .cmp(&crate::refs::id_number(&b.id))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// What [`apply_fixes`] did: the transitions it performed, and the ones the
/// engine refused.
#[derive(Debug, Default)]
pub struct FixReport {
    /// One line per applied fix, for the caller to print.
    pub applied: Vec<String>,
    /// One line per refusal, phrased as a `verify` problem.
    pub refused: Vec<String>,
}

/// Perform the fixable observations against a loaded store, as ordinary status
/// transitions: history appended, layout relocation recorded, the type's rules
/// evaluated. The caller flushes and syncs.
///
/// Two rules, both load-bearing:
///
/// - **at most one fix per document per run.** A document can be overflowed
///   *and* stuck on the same status; the second fix would be computed from a
///   status the first one just left, and `set-status` would refuse it as an
///   undeclared edge. The overflow is preferred — an exhausted visit budget is
///   the stronger signal, and it is the one a person is meant to take over.
/// - **a refusal is data, not a failure.** A `then` may be a status whose
///   `[[rules]]` demand a field no config check can foresee. Refusing to fix one
///   document must not abort the pass or turn `verify` into exit 2: the refusal
///   is collected, phrased as a problem, and the re-verify decides the code.
pub fn apply_fixes(prj: &Project, store: &mut Store, obs: &[Observation]) -> FixReport {
    use crate::commands::set_status;

    let mut report = FixReport::default();
    let mut touched: Vec<&str> = Vec::new();
    for o in obs {
        let Some(target) = &o.target else { continue };
        if touched.contains(&o.id.as_str()) {
            continue;
        }
        touched.push(&o.id);
        // Every fix is an ordinary move, subject to the visit budget like any
        // other — the overflow fix included. Bypassing it there would suppress
        // the budget of the *target*, never the spent one being escaped (the
        // guard counts entries into the status being entered, and
        // `validate_observation_target` already rejects `then == key`), so the
        // only thing a bypass could ever do is write the one state the guard
        // exists to prevent: a count past `limit` (see the module docs), by
        // performing a move `set-status` had just refused. A `then` whose own
        // budget is spent lands in the refusals instead, naming its own `then`
        // — the next step up the escalation, for a person to take.
        let (from, what) = match &o.kind {
            Kind::WorkBranch { .. } => continue,
            Kind::Stuck { status, .. } => (status, "stuck"),
            Kind::Overflow { status, .. } => (status, "max_visits"),
        };
        match set_status::core(prj, store, &o.id, target, None) {
            Ok(()) => report
                .applied
                .push(format!("fixed {}: {from} -> {target} ({what})", o.id)),
            Err(e) => report
                .refused
                .push(format!("{}: could not fix ({what}): {e}", o.id)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A single-type project (`t`, prefix `T`) over the extra config in `toml`.
    fn project(toml: &str) -> Project {
        let pcfg: crate::project_config::ProjectConfig =
            toml::from_str(&format!("[types.t]\nprefix = \"T\"\n{toml}")).expect("parses");
        Project {
            root: PathBuf::from("/p"),
            base: PathBuf::from("/p/opys"),
            pcfg,
        }
    }

    fn doc_type(toml: &str) -> DocType {
        project(toml)
            .pcfg
            .types
            .into_values()
            .next()
            .expect("one type")
    }

    /// A document at `path` with `id` and `status`, whose history holds one entry
    /// per element of `entries`, keyed one second apart from 2026-01-02T03:04:05Z.
    fn doc_at(path: &str, id: &str, status: &str, entries: &[&str]) -> Doc {
        let mut fm = String::from("history:\n");
        for (i, s) in entries.iter().enumerate() {
            fm.push_str(&format!(
                "  \"2026-01-02T03:04:{:02}.000000Z\": {s}\n",
                5 + i
            ));
        }
        Doc::parse(
            PathBuf::from(path),
            &format!("---\nid: {id}\nstatus: {status}\n{fm}---\n\n# Work\n"),
        )
        .expect("fixture parses")
    }

    fn doc(status: &str, entries: &[&str]) -> Doc {
        doc_at("T-0001.md", "T-0001", status, entries)
    }

    fn at(s: &str) -> OffsetDateTime {
        crate::history::parse(s).expect("fixture instant")
    }

    #[test]
    fn durations_accept_one_or_more_integer_unit_groups() {
        assert_eq!(parse_duration("10m"), Some(Duration::minutes(10)));
        assert_eq!(parse_duration("90s"), Some(Duration::seconds(90)));
        assert_eq!(parse_duration("2d"), Some(Duration::days(2)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("2w"), Some(Duration::days(14)));
        assert_eq!(parse_duration("2y"), Some(Duration::days(730)));
        assert_eq!(parse_duration("0s"), Some(Duration::seconds(0)));
    }

    /// Everything else is a config problem, including the lenient parser's
    /// classics: a bare number, a bare unit, whitespace, a sign, and a value
    /// that overflows the seconds count.
    #[test]
    fn durations_reject_everything_else() {
        for s in [
            "",
            "10",
            "m",
            "10x",
            "1h30",
            "-1d",
            "1h 30m",
            "10M",
            "1.5h",
            "999999999999999999999y",
        ] {
            assert_eq!(parse_duration(s), None, "{s:?} must not parse");
        }
    }

    #[test]
    fn stuck_measures_from_the_last_history_key_and_is_strict_at_the_boundary() {
        let t = doc_type(
            "statuses = [\"doing\"]\ndefault_status = \"doing\"\n\
             [types.t.ttl]\ndoing = { after = \"10m\", then = \"todo\" }\n",
        );
        let d = doc("doing", &["doing"]);
        // Exactly `after` past the entry is not yet stuck; one second more is.
        assert_eq!(stuck(&d, &t, at("2026-01-02T03:14:05Z")), None);
        let o = stuck(&d, &t, at("2026-01-02T03:14:06Z")).expect("stuck");
        assert_eq!(o.target.as_deref(), Some("todo"));
        assert_eq!(
            o.to_string(),
            "T-0001: stuck in doing since 2026-01-02T03:04:05.000000Z (ttl 10m) — \
             `--fix` moves it to todo"
        );
    }

    /// A bare duration is observed and nothing more, and a status with no `ttl`
    /// is not observed at all.
    #[test]
    fn a_bare_ttl_duration_has_nothing_to_fix() {
        let t = doc_type(
            "statuses = [\"doing\", \"idle\"]\ndefault_status = \"doing\"\n\
             [types.t.ttl]\ndoing = \"10m\"\n",
        );
        let o = stuck(&doc("doing", &["doing"]), &t, at("2026-06-01T00:00:00Z")).expect("stuck");
        assert_eq!(o.target, None);
        assert!(o.to_string().ends_with("— no ttl target, nothing to fix"));
        assert_eq!(
            stuck(&doc("idle", &["idle"]), &t, at("2026-06-01T00:00:00Z")),
            None
        );
    }

    /// An unreadable clock or an unparseable duration observes nothing: both are
    /// already reported, by `verify` and `config validate` respectively.
    #[test]
    fn stuck_stays_silent_without_history_or_a_parseable_duration() {
        let t = doc_type(
            "statuses = [\"doing\"]\ndefault_status = \"doing\"\n\
             [types.t.ttl]\ndoing = \"ten minutes\"\n",
        );
        assert_eq!(
            stuck(&doc("doing", &["doing"]), &t, at("2026-06-01T00:00:00Z")),
            None
        );
        let t = doc_type(
            "statuses = [\"doing\"]\ndefault_status = \"doing\"\n\
             [types.t.ttl]\ndoing = \"10m\"\n",
        );
        assert_eq!(
            stuck(&doc("doing", &[]), &t, at("2026-06-01T00:00:00Z")),
            None
        );
    }

    /// `limit = N` allows N entries: the N-th reaches the limit, N-1 does not.
    #[test]
    fn overflow_fires_when_the_visit_count_reaches_the_limit() {
        let t = doc_type(
            "statuses = [\"doing\", \"review\", \"user-review\"]\ndefault_status = \"doing\"\n\
             [types.t.max_visits]\nreview = { limit = 3, then = \"user-review\" }\n",
        );
        let under = doc("review", &["review", "doing", "review"]);
        assert_eq!(overflow(&under, &t), None);
        let at_limit = doc("review", &["review", "doing", "review", "doing", "review"]);
        let o = overflow(&at_limit, &t).expect("overflowed");
        assert_eq!(o.target.as_deref(), Some("user-review"));
        assert_eq!(
            o.to_string(),
            "T-0001: 3 visits to review reaches its limit of 3 — `--fix` moves it to user-review"
        );
        // Only the status the document holds now is counted.
        assert_eq!(overflow(&doc("doing", &["review", "doing"]), &t), None);
    }

    /// The one entry point, and the reason it sorts: documents reach it in the
    /// backend's *path* order, which a `--fix` relocation changes, so the block
    /// is ordered by id number instead. Here `opys/T-0002.md` and `opys/X-0003.md`
    /// both load before `opys/_stuck/T-0001.md` ('T' and 'X' sort before '_'),
    /// and T-0001 is still named first. Per document the overflow precedes the
    /// stuck — the order `apply_fixes` resolves them in — and a document whose
    /// id prefix belongs to no type is skipped, because a type is what carries
    /// the thresholds (`verify` already reports the stray prefix as a problem).
    #[test]
    fn observations_are_ordered_by_id_not_load_order() {
        let prj = project(
            "statuses = [\"doing\", \"review\", \"user-review\", \"todo\"]\n\
             default_status = \"doing\"\n\
             [types.t.ttl]\n\
             doing = { after = \"10m\", then = \"todo\" }\n\
             review = { after = \"10m\", then = \"user-review\" }\n\
             [types.t.max_visits]\n\
             review = { limit = 2, then = \"user-review\" }\n",
        );
        let docs = vec![
            doc_at("opys/T-0002.md", "T-0002", "doing", &["doing"]),
            doc_at("opys/X-0003.md", "X-0003", "doing", &["doing"]),
            doc_at(
                "opys/_stuck/T-0001.md",
                "T-0001",
                "review",
                &["review", "doing", "review"],
            ),
        ];
        // The fixture really is in path order, which is what the backend hands over.
        let mut by_path: Vec<&std::path::Path> = docs.iter().map(|d| d.path.as_path()).collect();
        by_path.sort();
        assert_eq!(
            by_path,
            docs.iter().map(|d| d.path.as_path()).collect::<Vec<_>>()
        );

        let out = observations(&prj, &docs, at("2026-06-01T00:00:00Z")).unwrap();
        assert_eq!(
            out.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            ["T-0001", "T-0001", "T-0002"]
        );
        assert!(
            matches!(out[0].kind, Kind::Overflow { .. })
                && matches!(out[1].kind, Kind::Stuck { .. }),
            "{out:?}"
        );
    }
}
