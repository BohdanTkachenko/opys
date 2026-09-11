//! The `history` lifecycle map: a reserved, auto-maintained frontmatter key
//! holding UTC RFC3339 timestamps (six fractional digits, `Z`) → status, one
//! entry per document write that changed the status.
//!
//! Keys are fixed width, so lexical order is chronological, and a key that is
//! not strictly greater than the document's greatest existing key is bumped by
//! one microsecond — two transitions inside one second can never collide.
//! `new`/`import` seed the first entry from `created`, so `created` and the
//! first key always denote the same instant; `set-status` (and, until
//! TASK-0112 removes the auto-set, `block`/`unblock`) append. A status change on
//! a document that predates the map seeds it first, at the status being left
//! (see [`record`]), so no write can leave `created` and the first key
//! disagreeing.
//!
//! Nothing hand-edits the map. `verify` reports a botched one (see
//! `commands::verify::check_history`) but never repairs it: unlike the relation
//! maps, history cannot be rebuilt from ground truth.
//!
//! **One shape rule, [`is_map`], decides everywhere** what counts as a lifecycle
//! map: a *non-empty* mapping whose every key is a canonical UTC key and whose
//! every value is a string. [`entries`] reads nothing out of anything else,
//! `store::decompose` files anything else in `fm_fields` verbatim, and `verify`
//! reports it through [`botched`], naming the document and what is wrong. The
//! three used to disagree — `entries` silently dropped the wrong-typed pairs of
//! a map the store kept opaque — so `verify` blessed a document whose map the
//! next `set-status` then destroyed. A value opys does not understand is data:
//! it is reported, never rewritten, and every write path that would append to it
//! refuses instead (see [`botched`] and `Store::append_history`).
//!
//! One invariant lives outside that rule because it is not a property of the
//! parsed value at all: keys are **quoted** in the file. Both spellings parse to
//! the same string under YAML 1.2, so [`is_map`] cannot see the difference —
//! only the source can, which is what [`unquoted_keys`] reads (and
//! `verify::spelling_problems` reports). It matters because a YAML 1.1 reader
//! resolves a bare RFC3339 scalar as a *timestamp*, and a corpus is read by more
//! than this tool.
//!
//! The rule covers *every* shape, not just mappings. Only a value that carries
//! no entries at all — absent, `null`, `{}`, `[]`, a bare scalar — may be
//! replaced by a seed; a non-empty sequence or string (what a hand-written
//! migration script plausibly produces) is data, so [`botched`] names its shape
//! and [`seed`] leaves it alone.
//!
//! Not to be confused with [`crate::commands::history`], the `opys history`
//! command that reads a document's timeline out of git (feature `history`).

use serde_norway::{Mapping, Value};
use time::{
    format_description::well_known::Rfc3339, format_description::FormatItem,
    macros::format_description, Duration, OffsetDateTime,
};

use crate::frontmatter::Frontmatter;

/// The reserved frontmatter key holding the map.
pub const FIELD: &str = "history";

/// The canonical key format: RFC3339, UTC, exactly six fractional digits, `Z`.
/// (`Rfc3339`'s own formatter emits the *minimum* number of subsecond digits,
/// which would break fixed-width lexical ordering.)
const UTC_MICROS: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");

/// The log-entry stamp format: RFC3339, UTC, whole seconds, `Z`. A `log`
/// section is read by people as much as by `verify`, and a microsecond nobody
/// typed is noise in a `## Progress` bullet. The fixed-width lexical ordering a
/// history key needs is not at stake: log entries are appended in order and
/// compared as instants, never sorted as strings.
const UTC_SECONDS: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");

/// Render an instant as a history key (converted to UTC).
pub fn format_key(dt: OffsetDateTime) -> String {
    dt.to_offset(time::UtcOffset::UTC)
        .format(UTC_MICROS)
        .expect("UTC micros formatting")
}

/// Render an instant as a [log-entry](crate::body::log_entries) stamp: UTC,
/// whole seconds, `Z`. Spelled out rather than left to `Rfc3339`, whose
/// formatter emits whatever subsecond digits the value happens to carry.
pub fn format_seconds(dt: OffsetDateTime) -> String {
    dt.to_offset(time::UtcOffset::UTC)
        .format(UTC_SECONDS)
        .expect("UTC seconds formatting")
}

/// Parse any RFC3339 datetime (a history key included).
pub fn parse(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// Convert any RFC3339 datetime — `created`, or an `OPYS_NOW` carrying an
/// offset — into the canonical UTC key form.
pub fn key_from(s: &str) -> Option<String> {
    parse(s).map(format_key)
}

/// Whether `s` is a well-formed history key: RFC3339, and UTC spelled `Z`.
/// Deliberately looser than [`is_canonical_key`] — it accepts any fractional
/// width, so it is the right test for *reading* a key (backfilling `created`
/// from one, say), never for blessing one written into the map.
pub fn is_utc_key(s: &str) -> bool {
    s.ends_with('Z') && parse(s).is_some()
}

/// Whether `s` is spelled exactly the way [`format_key`] would spell it: RFC3339
/// UTC, `Z`, exactly six fractional digits. This is the form `verify` demands,
/// and the reason it does is [`crate::frontmatter`]'s block: fixed-width keys
/// are the only ones whose lexical order and chronological order agree, and a
/// stray `2026-01-01T00:00:00Z` beside a `2026-01-01T00:00:00.5Z` would sort
/// *after* it (`.` < `Z`), reordering the map on the next write.
pub fn is_canonical_key(s: &str) -> bool {
    parse(s).map(format_key).as_deref() == Some(s)
}

/// **The** shape rule for a lifecycle map, shared by [`entries`],
/// `store::decompose` and `verify`: a non-empty mapping whose every key is a
/// [canonical](is_canonical_key) UTC timestamp and whose every value is a
/// string.
///
/// It is deliberately all-or-nothing. Reading the well-formed subset of a map
/// (what this used to do) blesses a document the store cannot decompose, so the
/// value sits in `fm_fields` while the authoritative `history` table stays
/// empty — and the next append then shadows, and destroys, the whole map.
pub fn is_map(v: &Value) -> bool {
    matches!(v, Value::Mapping(m) if is_map_inner(m))
}

fn is_map_inner(m: &Mapping) -> bool {
    !m.is_empty()
        && m.iter()
            .all(|(k, val)| k.as_str().is_some_and(is_canonical_key) && val.as_str().is_some())
}

/// The map's entries in file order, or empty when the value is absent or fails
/// [`is_map`] (in which case [`botched`] says why).
pub fn entries(fm: &Frontmatter) -> Vec<(String, String)> {
    match fm.get(FIELD) {
        Some(Value::Mapping(m)) if is_map_inner(m) => m
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().expect("is_map checked").to_string(),
                    v.as_str().expect("is_map checked").to_string(),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Why a document's `history` value is present but unusable as a lifecycle map,
/// or `None` when it is absent, [seedable](is_seedable) (carries no entries at
/// all, so a first entry may simply replace it), or well formed.
///
/// This is exactly the complement of [`is_seedable`] and [`is_map`]: every
/// present value is seedable (carries no entries), readable, or botched — never
/// none of the three. A value that is *not* a mapping is reported too, by
/// shape: a hand-written migration plausibly produces a YAML list (ADR-0099
/// weighed one as the alternative spelling), and treating it as "carries no
/// entries" would let [`seed`] replace the operator's transitions with a single
/// wall-clock entry.
///
/// The string is the body of the message `verify` prints, and the reason every
/// append path refuses: a map opys cannot read is data it must not overwrite.
pub fn botched(fm: &Frontmatter) -> Option<String> {
    let v = fm.get(FIELD)?;
    if carries_no_entries(v) {
        return None;
    }
    let Value::Mapping(m) = v else {
        return Some(format!(
            "'history' is {}, not a timestamp→status mapping \
             (one quoted UTC timestamp per line, \
             e.g. \"2026-06-16T14:30:00.000000Z\": implemented)",
            shape_of(v)
        ));
    };
    for (k, v) in m {
        let Some(key) = k.as_str() else {
            return Some(
                "a history key is not a string (every key is a quoted UTC timestamp)".to_string(),
            );
        };
        if !is_canonical_key(key) {
            return Some(format!(
                "history key '{key}' is not a canonical UTC timestamp \
                 (RFC3339 with 'Z' and exactly six fractional digits, \
                 e.g. 2026-06-16T14:30:00.000000Z)"
            ));
        }
        if v.as_str().is_none() {
            return Some(format!(
                "history entry '{key}' does not record a status as a string"
            ));
        }
    }
    None
}

/// The lifecycle keys a document's **stored text** writes as bare (unquoted)
/// scalars — the one history invariant the parsed map cannot see.
///
/// Quoting is spelling, and parsing throws spelling away: to a YAML 1.2 reader
/// (`serde_norway`, and so opys itself) `2026-06-16T14:30:00.000000Z: planned`
/// is the very same string→string pair as the quoted form, so it satisfies
/// [`is_map`] and every check in `commands::verify::check_history`. A YAML
/// **1.1** reader resolves that bare scalar as a *timestamp* instead, and the
/// map stops being the string→string mapping format.md defines — a corpus is
/// read by more than this tool, so the spec requires the quoted spelling and
/// `verify` enforces it against the source (see
/// `commands::verify::spelling_problems`).
///
/// Ground truth is the parsed map: every key it holds must appear quoted inside
/// the document's own `history:` block. Only that block is consulted, so a
/// `created` that happens to spell the same instant cannot mask a bare key; and
/// a map opys cannot read ([`botched`], which `verify` reports first) yields
/// nothing here rather than a second opinion about the same document.
pub fn unquoted_keys(fm: &Frontmatter, stored: &str) -> Vec<String> {
    let keys: Vec<String> = entries(fm).into_iter().map(|(k, _)| k).collect();
    if keys.is_empty() {
        return Vec::new();
    }
    let Some(block) = block_source(stored) else {
        return Vec::new();
    };
    keys.into_iter()
        .filter(|k| !block.contains(&format!("\"{k}\"")) && !block.contains(&format!("'{k}'")))
        .collect()
}

/// The `history` entry of a document's frontmatter source: the `history:` line
/// plus the indented lines under it. `None` when the file carries no
/// frontmatter or no top-level `history:` line. A flow mapping written on the
/// key line (`history: {…}`) is covered too — it is that line's own tail.
fn block_source(stored: &str) -> Option<String> {
    let yaml = crate::frontmatter::source(stored)?;
    let bare = format!("{FIELD}:");
    let with_tail = format!("{FIELD}: ");
    let mut block: Option<String> = None;
    for line in yaml.lines() {
        match &mut block {
            None => {
                if line == bare || line.starts_with(&with_tail) {
                    block = Some(line.to_string());
                }
            }
            // The block ends where the next top-level key begins.
            Some(_) if !line.starts_with([' ', '\t']) => break,
            Some(acc) => {
                acc.push('\n');
                acc.push_str(line);
            }
        }
    }
    block
}

/// The greatest key among `keys`, comparing parsed instants and ignoring
/// unparseable ones (which `verify` reports separately).
pub fn greatest<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    keys.into_iter()
        .filter_map(|k| parse(k).map(|dt| (dt, k)))
        .max_by_key(|(dt, _)| *dt)
        .map(|(_, k)| k.to_string())
}

/// The key an append actually uses: `want`, unless it is not strictly greater
/// than `last` — then `last` plus one microsecond, so two writes at the same
/// instant produce two distinct, ordered keys. An unparseable `last` (or none)
/// leaves `want` alone.
///
/// When `last` is already the greatest instant `time` can represent (only
/// reachable through a hand-written key, which `verify` has no reason to
/// reject), there is no greater key to bump to: the addition would panic, so we
/// saturate and return `last` itself. Both append paths treat an equal key as a
/// replacement — the map keeps one entry per instant — so the document stays
/// monotonic and its last entry still records the new status.
pub fn next_key(last: Option<&str>, want: &str) -> String {
    let (Some(last), Some(want_dt)) = (last.and_then(parse), parse(want)) else {
        return want.to_string();
    };
    if want_dt > last {
        want.to_string()
    } else {
        format_key(last.checked_add(Duration::microseconds(1)).unwrap_or(last))
    }
}

/// Append one entry at `status`, timestamped `want` (bumped past the greatest
/// existing key when needed). Returns the key written.
pub fn append(fm: &mut Frontmatter, want: &str, status: &str) -> String {
    let existing = entries(fm);
    let last = greatest(existing.iter().map(|(k, _)| k.as_str()));
    let key = next_key(last.as_deref(), want);
    let mut map = match fm.get(FIELD) {
        Some(Value::Mapping(m)) => m.clone(),
        _ => Mapping::new(),
    };
    map.insert(
        Value::String(key.clone()),
        Value::String(status.to_string()),
    );
    fm.insert(FIELD, Value::Mapping(map));
    key
}

/// Whether a value holds no lifecycle entries at all, so writing a first entry
/// over it destroys nothing: `null`, a bare scalar (`5`, `true`, `""`), an empty
/// mapping, an empty sequence. Everything else carries data — a non-empty
/// mapping, a *non-empty sequence* (`- "…": open`), a non-empty scalar string
/// (`"2026-01-01T00:00:00.000000Z: open"`), a tagged value — and is reported by
/// [`botched`] rather than replaced.
///
/// The sequence and string cases are the reason this is a shape *complement*
/// rather than a mapping test: `botched` used to return `None` for every
/// non-mapping and `is_seedable` `true`, so a hand-migrated list was called
/// `missing history` by `verify` and then silently overwritten by the next
/// `set-status`.
fn carries_no_entries(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => s.is_empty(),
        Value::Sequence(s) => s.is_empty(),
        Value::Mapping(m) => m.is_empty(),
        Value::Tagged(_) => false,
    }
}

/// How to name a non-mapping value in a [`botched`] message.
fn shape_of(v: &Value) -> &'static str {
    match v {
        Value::Sequence(_) => "a sequence",
        Value::String(_) => "a string",
        Value::Tagged(_) => "a tagged value",
        // `carries_no_entries` covers the rest, so `botched` never gets here.
        _ => "not a mapping",
    }
}

/// Whether a first entry can be written without destroying anything: the key is
/// absent, or holds a value that [carries no entries at all](carries_no_entries)
/// (`{}`, `5`, `[]`, which `store::decompose` keeps verbatim in `fm_fields` and
/// `verify` reports as `missing history`). Any value carrying entries — a
/// non-empty mapping, sequence or string — is never overwritten, even when its
/// entries are wrong-typed: that is data, and the repair is a hand edit.
///
/// This promise is only worth anything because the *append* paths keep it too:
/// a value carrying entries that fails [`is_map`] makes [`botched`] speak up,
/// and every writer refuses rather than appending beside a value it cannot read
/// (`Store::append_history` refuses outright).
fn is_seedable(fm: &Frontmatter) -> bool {
    fm.get(FIELD).is_none_or(carries_no_entries)
}

/// Seed a document's first entry, keyed by its `created` stamp so the two denote
/// the same instant by construction (falling back to `now` when `created` is
/// absent or unparseable). A document that already carries entries is left alone
/// — `import` may seed one itself.
pub fn seed(fm: &mut Frontmatter, status: &str, now: &str) {
    if !is_seedable(fm) {
        return;
    }
    let key = fm
        .get_str("created")
        .and_then(key_from)
        .unwrap_or_else(|| now.to_string());
    let mut map = Mapping::new();
    map.insert(Value::String(key), Value::String(status.to_string()));
    fm.insert(FIELD, Value::Mapping(map));
}

/// Record a status change: append `status` at `want`, after seeding a first
/// entry when the document predates the map.
///
/// The seed is what keeps a *migration* possible. Appending straight onto a
/// history-less document would make the wall clock its *first* key, which
/// contradicts `created` (verify check 4) and no longer says `missing history`
/// — the corpus would be red, the migration procedure could neither find nor
/// fix it, and the only repair would be hand-editing a map the spec forbids
/// hand-editing. Seeding from `created` at the status the document was *leaving*
/// lands this transition as the second entry instead, so `created` still matches
/// the first and the tool keeps its promise that every write leaves the corpus
/// green.
pub fn record(fm: &mut Frontmatter, want: &str, prev: &str, status: &str) -> String {
    seed(fm, prev, want);
    append(fm, want, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_converts_an_offset_to_utc_with_six_digits() {
        assert_eq!(
            key_from("2026-06-16T10:30:00-04:00").unwrap(),
            "2026-06-16T14:30:00.000000Z"
        );
        assert_eq!(
            key_from("2026-01-02T03:04:05Z").unwrap(),
            "2026-01-02T03:04:05.000000Z"
        );
        assert_eq!(
            key_from("2026-01-02T03:04:05.5Z").unwrap(),
            "2026-01-02T03:04:05.500000Z"
        );
        assert!(key_from("not-a-date").is_none());
    }

    #[test]
    fn next_key_bumps_by_one_microsecond_when_not_strictly_greater() {
        let a = "2026-01-02T03:04:05.000000Z";
        // Same instant → bumped.
        assert_eq!(next_key(Some(a), a), "2026-01-02T03:04:05.000001Z");
        // Earlier → bumped past the last key, never backwards.
        assert_eq!(
            next_key(Some(a), "2026-01-01T00:00:00.000000Z"),
            "2026-01-02T03:04:05.000001Z"
        );
        // Strictly later → used as-is.
        let later = "2026-01-02T03:04:05.000002Z";
        assert_eq!(next_key(Some(a), later), later);
        // No history yet, or an unparseable last key → used as-is.
        assert_eq!(next_key(None, a), a);
        assert_eq!(next_key(Some("garbage"), a), a);
        // The bump carries across a second boundary.
        assert_eq!(
            next_key(Some("2026-01-02T03:04:05.999999Z"), a),
            "2026-01-02T03:04:06.000000Z"
        );
    }

    /// A hand-written key at the greatest representable instant has no successor:
    /// the bump saturates instead of panicking (`OffsetDateTime`'s `Add` does).
    #[test]
    fn next_key_saturates_at_the_maximum_instant() {
        let max = "9999-12-31T23:59:59.999999Z";
        assert_eq!(next_key(Some(max), "2026-01-02T03:04:05.000000Z"), max);
        // And the saturated key replaces the entry rather than duplicating it.
        let mut fm = Frontmatter::new();
        fm.insert(FIELD, {
            let mut m = Mapping::new();
            m.insert(Value::String(max.into()), Value::String("planned".into()));
            Value::Mapping(m)
        });
        assert_eq!(
            append(&mut fm, "2026-01-02T03:04:05.000000Z", "partial"),
            max
        );
        assert_eq!(entries(&fm), vec![(max.to_string(), "partial".to_string())]);
    }

    /// `is_canonical_key` is the spelling `verify` demands; `is_utc_key` is the
    /// looser read-side test.
    #[test]
    fn is_canonical_key_demands_six_fractional_digits() {
        assert!(is_canonical_key("2026-01-02T03:04:05.000000Z"));
        // Well-formed UTC, but not fixed width — lexical order would lie.
        assert!(!is_canonical_key("2026-01-02T03:04:05Z"));
        assert!(!is_canonical_key("2026-01-02T03:04:05.5Z"));
        assert!(!is_canonical_key("2026-01-02T03:04:05.000000000Z"));
        // Not UTC at all.
        assert!(!is_canonical_key("2026-01-02T03:04:05.000000+00:00"));
        assert!(!is_canonical_key("nonsense"));
        assert!(is_utc_key("2026-01-02T03:04:05Z"));
    }

    /// `record` seeds a document that predates the map from `created` at the
    /// status it is leaving, so the appended entry lands second and `created`
    /// still matches the first key.
    #[test]
    fn record_seeds_a_history_less_document_before_appending() {
        let mut fm = Frontmatter::new();
        fm.set_str("created", "2026-01-02T03:04:05-04:00");
        let key = record(&mut fm, "2026-06-16T14:30:00.000000Z", "planned", "partial");
        assert_eq!(key, "2026-06-16T14:30:00.000000Z");
        assert_eq!(
            entries(&fm),
            vec![
                ("2026-01-02T07:04:05.000000Z".into(), "planned".into()),
                ("2026-06-16T14:30:00.000000Z".into(), "partial".into()),
            ]
        );
        // A map that already holds entries is appended to, not reseeded.
        let key = record(&mut fm, "2026-06-16T14:30:00.000000Z", "partial", "planned");
        assert_eq!(key, "2026-06-16T14:30:00.000001Z");
        assert_eq!(entries(&fm).len(), 3);
    }

    /// An empty or wrong-typed value carries nothing, so it is replaced; a
    /// non-empty mapping is data and is left for a human.
    #[test]
    fn seed_replaces_an_empty_value_but_never_a_non_empty_mapping() {
        for empty in [Value::Mapping(Mapping::new()), Value::Number(5.into())] {
            let mut fm = Frontmatter::new();
            fm.insert(FIELD, empty);
            seed(&mut fm, "todo", "2026-01-02T03:04:05.000000Z");
            assert_eq!(
                entries(&fm),
                vec![("2026-01-02T03:04:05.000000Z".into(), "todo".into())]
            );
        }
        let mut fm = Frontmatter::new();
        let mut m = Mapping::new();
        m.insert(
            Value::String("2026-01-02T03:04:05.000000Z".into()),
            7.into(),
        );
        fm.insert(FIELD, Value::Mapping(m.clone()));
        seed(&mut fm, "todo", "2026-01-02T03:04:05.000000Z");
        assert_eq!(fm.get(FIELD), Some(&Value::Mapping(m)));
    }

    #[test]
    fn append_bumps_against_the_greatest_key_and_seed_follows_created() {
        let mut fm = Frontmatter::new();
        fm.set_str("created", "2026-01-02T03:04:05Z");
        seed(&mut fm, "todo", "2099-01-01T00:00:00.000000Z");
        assert_eq!(
            entries(&fm),
            vec![("2026-01-02T03:04:05.000000Z".into(), "todo".into())]
        );
        // Seeding twice is a no-op.
        seed(&mut fm, "done", "2099-01-01T00:00:00.000000Z");
        assert_eq!(entries(&fm).len(), 1);

        let key = append(&mut fm, "2026-01-02T03:04:05.000000Z", "in-progress");
        assert_eq!(key, "2026-01-02T03:04:05.000001Z");
        assert_eq!(entries(&fm).len(), 2);
        assert!(is_utc_key(&key));
    }

    #[test]
    fn entries_ignores_a_wrong_shaped_value() {
        for empty in [
            Value::Number(5.into()),
            Value::Mapping(Mapping::new()),
            Value::Sequence(Vec::new()),
            Value::Null,
            Value::String(String::new()),
        ] {
            let mut fm = Frontmatter::new();
            fm.insert(FIELD, empty.clone());
            assert!(entries(&fm).is_empty());
            assert_eq!(
                botched(&fm),
                None,
                "a value carrying no entries is seedable: {empty:?}"
            );
        }
    }

    /// A value that is not a mapping but *does* carry entries — the YAML list a
    /// hand-written migration produces, or one long scalar — is data. It used to
    /// fall through `botched`'s mapping-only early return into `is_seedable`'s
    /// catch-all `true`, so `verify` called it `missing history` and the next
    /// `set-status` replaced it with a single seed.
    #[test]
    fn a_non_mapping_that_carries_entries_is_botched_and_never_seeded() {
        let seq = Value::Sequence(vec![
            Value::String("2026-01-01T00:00:00.000000Z: open".into()),
            Value::String("2026-02-01T00:00:00.000000Z: reopened".into()),
        ]);
        let scalar = Value::String("2026-01-01T00:00:00.000000Z: open".into());
        for (value, want) in [
            (
                seq,
                "'history' is a sequence, not a timestamp→status mapping",
            ),
            (
                scalar,
                "'history' is a string, not a timestamp→status mapping",
            ),
        ] {
            let mut fm = Frontmatter::new();
            fm.insert(FIELD, value.clone());
            assert!(entries(&fm).is_empty());
            assert!(!is_map(&value));
            let why = botched(&fm).expect("a value carrying entries is botched");
            assert!(why.starts_with(want), "{why}");
            // `seed` (and therefore `record`) leaves the operator's data alone.
            seed(&mut fm, "todo", "2026-06-06T06:06:06.000000Z");
            assert_eq!(fm.get(FIELD), Some(&value));
        }
    }

    /// The shape rule is all-or-nothing: one bad pair makes the *whole* map
    /// unreadable, so the store, `verify` and `entries` agree about it. Reading
    /// the good subset is what let `verify` bless a map the store kept opaque
    /// and the next `set-status` destroyed.
    #[test]
    fn one_bad_pair_makes_the_whole_map_unreadable_and_botched() {
        let good = "2026-01-01T00:00:00.000000Z";
        let bad_value = {
            let mut m = Mapping::new();
            m.insert(Value::String(good.into()), Value::String("planned".into()));
            m.insert(
                Value::String("2026-02-01T00:00:00.000000Z".into()),
                Value::Null,
            );
            m
        };
        let bad_key = {
            let mut m = Mapping::new();
            m.insert(Value::String(good.into()), Value::String("planned".into()));
            m.insert(
                Value::String("2026-02-01T00:00:00Z".into()),
                Value::String("partial".into()),
            );
            m
        };
        let non_string_key = {
            let mut m = Mapping::new();
            m.insert(Value::Number(5.into()), Value::String("planned".into()));
            m
        };

        for (m, want) in [
            (
                bad_value,
                "history entry '2026-02-01T00:00:00.000000Z' does not record a status as a string",
            ),
            (
                bad_key,
                "history key '2026-02-01T00:00:00Z' is not a canonical",
            ),
            (non_string_key, "a history key is not a string"),
        ] {
            let mut fm = Frontmatter::new();
            fm.insert(FIELD, Value::Mapping(m.clone()));
            assert!(!is_map(&Value::Mapping(m.clone())));
            assert!(
                entries(&fm).is_empty(),
                "entries must not read the well-formed subset"
            );
            let why = botched(&fm).expect("a non-empty unreadable map is botched");
            assert!(why.starts_with(want), "{why}");
            // And it is never seedable: `seed` leaves the operator's data alone.
            seed(&mut fm, "todo", "2026-06-06T06:06:06.000000Z");
            assert_eq!(fm.get(FIELD), Some(&Value::Mapping(m)));
        }
    }

    /// Quoting is spelling, so it survives neither parsing nor [`is_map`]: the
    /// bare and quoted forms are the same `Value`. [`unquoted_keys`] reads the
    /// source instead, and only the document's own `history:` block — a
    /// `created` spelling the same instant must not mask a bare key.
    #[test]
    fn unquoted_keys_reads_the_history_block_of_the_stored_text() {
        let key = "2026-06-16T14:30:00.000000Z";
        let doc = |block: &str| {
            format!(
                "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\n\
                 created: \"{key}\"\n{block}---\n\n# F\n"
            )
        };
        let fm = |text: &str| crate::frontmatter::parse(text, "x.md").unwrap().0;

        // The canonical spelling, and the other spelling YAML gives a string.
        for quoted in [
            format!("history:\n  \"{key}\": planned\n"),
            format!("history:\n  '{key}': planned\n"),
            format!("history: {{\"{key}\": planned}}\n"),
        ] {
            let text = doc(&quoted);
            assert_eq!(unquoted_keys(&fm(&text), &text), Vec::<String>::new());
        }

        // Bare: parses identically (so every other check passes), reported here.
        let text = doc(&format!("history:\n  {key}: planned\n"));
        assert!(is_map(fm(&text).get(FIELD).unwrap()));
        assert_eq!(botched(&fm(&text)), None);
        assert_eq!(unquoted_keys(&fm(&text), &text), vec![key.to_string()]);

        // One bare key among quoted ones is named on its own.
        let other = "2026-07-01T09:00:00.000000Z";
        let text = doc(&format!(
            "history:\n  \"{key}\": planned\n  {other}: partial\n"
        ));
        assert_eq!(unquoted_keys(&fm(&text), &text), vec![other.to_string()]);

        // A map opys cannot read has no keys to speak of — `botched` reports it.
        let text = doc(&format!("history:\n  {key}: [nope]\n"));
        assert!(botched(&fm(&text)).is_some());
        assert_eq!(unquoted_keys(&fm(&text), &text), Vec::<String>::new());
    }

    /// The block ends at the next top-level key: a quoted timestamp elsewhere in
    /// the frontmatter cannot vouch for a bare history key, whichever side of
    /// the block it sits on.
    #[test]
    fn unquoted_keys_looks_at_no_other_frontmatter_key() {
        let key = "2026-06-16T14:30:00.000000Z";
        let text = format!(
            "---\nid: FEAT-0001\nstatus: planned\ntags: [a]\ncreated: \"{key}\"\n\
             history:\n  {key}: planned\nspec: \"{key}\"\n---\n\n# F\n"
        );
        let fm = crate::frontmatter::parse(&text, "x.md").unwrap().0;
        assert_eq!(unquoted_keys(&fm, &text), vec![key.to_string()]);
    }

    /// A canonical, all-string map is readable and not botched.
    #[test]
    fn a_canonical_map_is_readable() {
        let mut fm = Frontmatter::new();
        let mut m = Mapping::new();
        m.insert(
            Value::String("2026-01-01T00:00:00.000000Z".into()),
            Value::String("planned".into()),
        );
        fm.insert(FIELD, Value::Mapping(m.clone()));
        assert!(is_map(&Value::Mapping(m)));
        assert_eq!(botched(&fm), None);
        assert_eq!(
            entries(&fm),
            vec![("2026-01-01T00:00:00.000000Z".into(), "planned".into())]
        );
    }
}
