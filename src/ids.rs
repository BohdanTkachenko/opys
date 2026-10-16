//! The id-allocation seam (ADR-0054).
//!
//! Every new document number comes from an [`IdSource`], so *where* numbers
//! come from is swappable without touching the commands. Two sources exist:
//!
//! - [`SequenceMax`] — the dense default: one past the corpus-wide max, correct
//!   on a single machine under the backend's inventory lock.
//! - [`Interleaved`] — one residue class of the same sequence: with
//!   `[ids] modulus = M` configured, each allocator owns a *slot* `k` in
//!   `0..M` and takes the smallest `n ≡ k (mod M)` above every number it has
//!   seen. Allocators with different slots cannot collide, on any branches,
//!   with no shared ledger. Two with the same slot are a misconfiguration
//!   that `renumber` repairs exactly like today's collisions.
//!
//! Commands never name a source: they call [`crate::Ctx::id_source`] (or
//! [`resolve`] directly where there is no `Ctx`, as the server does), which
//! reads the project's `[ids]` table and the environment. With no modulus the
//! result is `SequenceMax`, byte-identically — `OPYS_ID_SLOT` is not even read.
//!
//! Whatever the source, the global invariant stands: ids are drawn from one
//! monotonically increasing sequence and never reused — gaps are harmless
//! (monotonicity, not density, is the contract), so an abandoned reservation
//! or the `M - 1` gap between one slot's consecutive ids costs nothing.

use crate::error::{usage, Result};
use crate::project_config::ProjectConfig;
use crate::store::Store;

/// The environment variable naming an allocator's slot under `[ids] modulus`.
pub const SLOT_ENV: &str = "OPYS_ID_SLOT";

/// The largest `[ids] modulus` accepted (`2^32`). Numbers are stored as
/// signed 64-bit integers, and an id above `i64::MAX` wraps out of the
/// corpus-wide max — so a later allocation would *reuse* it, silently
/// overwriting a document. Bounding the stride keeps every reachable id
/// (`max + count * modulus`) far below that edge; nobody needs four billion
/// slots.
pub const MAX_MODULUS: u64 = 1 << 32;

/// A source of new document numbers.
///
/// A source is stateless with respect to the store: `reserve` recomputes from
/// the store's current max, so numbers handed out become unavailable only once
/// a document carrying them is `put_doc`'d. A caller that builds several
/// documents before storing any must reserve them as one [`IdSource::block`],
/// never as repeated `reserve(1)` calls.
pub trait IdSource {
    /// Reserve `count` numbers, returning the first. The numbers are
    /// `first, first + step, …` for `count` steps (see [`IdSource::step`]);
    /// an unused tail is simply a gap.
    fn reserve(&mut self, store: &mut Store, count: u64) -> Result<u64>;

    /// The distance between two consecutive numbers this source hands out
    /// (`1` for the dense sequence, the modulus for an interleaved one).
    fn step(&self) -> u64 {
        1
    }

    /// Reserve `count` numbers and return every one of them, in allocation
    /// order — the shape to consume when documents are built before any is
    /// stored (`import`, `renumber`).
    fn block(&mut self, store: &mut Store, count: u64) -> Result<Vec<u64>> {
        let first = self.reserve(store, count)?;
        let step = self.step();
        Ok((0..count).map(|i| first + i * step).collect())
    }
}

/// The default source: one past the highest number across live docs, every
/// relation-map entry (struck or not), and the retired ledger. Safe against
/// concurrent same-machine invocations because the backend holds the exclusive
/// inventory lock from load to flush; cross-branch allocation is
/// [`Interleaved`]'s job (ADR-0054), with `renumber` as the repair of last
/// resort.
pub struct SequenceMax;

impl IdSource for SequenceMax {
    fn reserve(&mut self, store: &mut Store, count: u64) -> Result<u64> {
        debug_assert!(count > 0, "reserve of zero numbers is a caller bug");
        let _ = count; // max-based: consecutive numbers after the max are free
        Ok(store.max_doc_num()? as u64 + 1)
    }
}

/// One residue class of the global sequence: the smallest `n ≡ slot (mod
/// modulus)` above the corpus-wide max, then every `modulus`-th number after
/// it.
///
/// The fields are private and [`Interleaved::new`] is the only way in, so the
/// invariants the arithmetic depends on — `2 <= modulus <= MAX_MODULUS` and
/// `slot < modulus` — hold for every instance that exists. A slot at or above
/// the modulus lands in *another* allocator's residue class, which is exactly
/// the collision the modulus is there to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interleaved {
    slot: u64,
    modulus: u64,
}

impl Interleaved {
    /// An allocator owning `slot` under `modulus`, or a usage error when that
    /// pair cannot allocate safely: a modulus outside `2..=`[`MAX_MODULUS`]
    /// (the same bounds `config validate` reports) or a slot not below it.
    ///
    /// [`resolve`] is the normal way to build one — it reads the project's
    /// `[ids]` table and the environment, and reports a bad slot in terms of
    /// `OPYS_ID_SLOT`. This constructor is the guard for every other caller
    /// (tests, embedders) that names a slot directly.
    pub fn new(slot: u64, modulus: u64) -> Result<Interleaved> {
        check_modulus(modulus)?;
        let last = modulus - 1;
        if slot >= modulus {
            return Err(usage(format!(
                "id slot {slot} is not below the [ids] modulus {modulus} (valid slots: 0 to {last})"
            )));
        }
        Ok(Interleaved { slot, modulus })
    }
}

impl Interleaved {
    /// The slot this allocator owns.
    pub fn slot(&self) -> u64 {
        self.slot
    }

    /// The modulus the slot is taken under.
    pub fn modulus(&self) -> u64 {
        self.modulus
    }
}

impl IdSource for Interleaved {
    fn reserve(&mut self, store: &mut Store, count: u64) -> Result<u64> {
        debug_assert!(count > 0, "reserve of zero numbers is a caller bug");
        let _ = count; // max-based: every congruent number after the max is free
        let max = store.max_doc_num()? as u64;
        Ok(first_congruent(max, self.slot, self.modulus))
    }

    fn step(&self) -> u64 {
        self.modulus
    }
}

/// The smallest `n > max` with `n ≡ slot (mod modulus)`. Pure, so the
/// arithmetic is testable without a store.
///
/// Crate-internal: outside this crate the arithmetic is only reachable through
/// [`Interleaved`], whose constructor guarantees the `modulus >= 2` this
/// division needs.
pub(crate) fn first_congruent(max: u64, slot: u64, modulus: u64) -> u64 {
    let base = max + 1;
    base + (slot + modulus - base % modulus) % modulus
}

/// The `[ids] modulus` bounds every allocator shares: at least 2 (one slot
/// cannot interleave) and at most [`MAX_MODULUS`]. `ProjectConfig::validate`
/// reports the same two as content problems, so a project sees them from
/// `config validate`/`verify` as well as from the command that would allocate.
fn check_modulus(modulus: u64) -> Result<()> {
    if modulus < 2 {
        return Err(usage(format!(
            "[ids] modulus must be at least 2 (got {modulus}) — see `opys config validate`"
        )));
    }
    if modulus > MAX_MODULUS {
        return Err(usage(format!(
            "[ids] modulus must be at most {MAX_MODULUS} (got {modulus}) — see `opys config validate`"
        )));
    }
    Ok(())
}

/// How this process would allocate in `pcfg`, as one human line for
/// `config validate` — the answer to "which slot am I in", which is otherwise
/// observable only by allocating a document. `Ok(None)` when no `[ids]
/// modulus` is configured (allocation is the dense sequence); the errors are
/// [`resolve`]'s, so a bad `OPYS_ID_SLOT` is reported here too.
pub fn describe(pcfg: &ProjectConfig) -> Result<Option<String>> {
    let Some(modulus) = pcfg.ids.modulus else {
        return Ok(None);
    };
    check_modulus(modulus)?;
    let env_slot = std::env::var_os(SLOT_ENV).map(|s| s.to_string_lossy().into_owned());
    let host = user_at_host();
    let slot = slot_for(modulus, env_slot.as_deref(), &host)?;
    let from = match env_slot {
        Some(_) => SLOT_ENV.to_string(),
        None => format!("the hash of {host}"),
    };
    Ok(Some(format!(
        "ids: slot {slot} of {modulus} (from {from}) — next id here is congruent to {slot} mod {modulus}"
    )))
}

/// Build the project's id source: [`SequenceMax`] when `opys.toml` sets no
/// `[ids] modulus`, otherwise an [`Interleaved`] source in the slot named by
/// `OPYS_ID_SLOT` or, failing that, hashed from `user@host` (see
/// [`slot_for`]). A modulus outside `2..=MAX_MODULUS` (also reported by
/// `config validate`) and a slot that is not an integer below the modulus —
/// an empty `OPYS_ID_SLOT` included — are usage errors.
pub fn resolve(pcfg: &ProjectConfig) -> Result<Box<dyn IdSource>> {
    let Some(modulus) = pcfg.ids.modulus else {
        return Ok(Box::new(SequenceMax));
    };
    // Before the slot: `slot_for`'s message names the modulus, and its hash
    // fallback divides by it.
    check_modulus(modulus)?;
    // An empty value is *not* unset: `OPYS_ID_SLOT="$SLOT"` with a `$SLOT`
    // that never got assigned must fail here, not silently share the hash
    // slot with every other agent the same orchestrator spawned.
    let env_slot = std::env::var_os(SLOT_ENV).map(|s| s.to_string_lossy().into_owned());
    let slot = slot_for(modulus, env_slot.as_deref(), &user_at_host())?;
    Ok(Box::new(Interleaved::new(slot, modulus)?))
}

/// The allocator's slot under `modulus`: `env_slot` (the value of
/// `OPYS_ID_SLOT`) when given, which must parse as an integer below the
/// modulus; otherwise [`hash_slot`] of `user_host`. Pure — the process
/// environment is read by [`resolve`].
pub fn slot_for(modulus: u64, env_slot: Option<&str>, user_host: &str) -> Result<u64> {
    let Some(raw) = env_slot else {
        return Ok(hash_slot(user_host, modulus));
    };
    let last = modulus - 1;
    match raw.trim().parse::<u64>() {
        Ok(slot) if slot < modulus => Ok(slot),
        Ok(slot) => Err(usage(format!(
            "{SLOT_ENV}={slot} is not a slot below the [ids] modulus {modulus} (valid slots: 0 to {last})"
        ))),
        Err(_) => Err(usage(format!(
            "{SLOT_ENV}={raw:?} is not an integer slot below the [ids] modulus {modulus} (valid slots: 0 to {last})"
        ))),
    }
}

/// The default slot for an allocator identified as `user@host`: FNV-1a 64 of
/// the bytes, modulo `modulus`. Stable across runs, platforms and opys
/// versions, so the same person on the same machine always lands in the same
/// slot and other tools can reproduce it.
pub fn hash_slot(user_host: &str, modulus: u64) -> u64 {
    fnv1a64(user_host.as_bytes()) % modulus
}

/// FNV-1a, 64-bit (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `user@host` for the running process: user from `USER` then `USERNAME`
/// (else `unknown`); host from `HOSTNAME`, then `COMPUTERNAME`, then
/// `/etc/hostname` (trimmed), then `localhost`. No command is spawned — the
/// slot must be derivable from the raw environment by any tool, so nothing
/// here calls `hostname(1)` or `gethostname`.
///
/// The fallbacks are best-effort by design: on macOS no shell exports
/// `HOSTNAME` and there is no `/etc/hostname`, so everyone on such a machine
/// hashes as `user@localhost`. That is still stable and still distinct per
/// user, but two accounts of the same name on two Macs share a slot — they
/// export `HOSTNAME` or set `OPYS_ID_SLOT` (which is what an orchestrator
/// does anyway).
fn user_at_host() -> String {
    let user = env_nonempty("USER")
        .or_else(|| env_nonempty("USERNAME"))
        .unwrap_or_else(|| "unknown".to_string());
    let host = env_nonempty("HOSTNAME")
        .or_else(|| env_nonempty("COMPUTERNAME"))
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "localhost".to_string());
    format!("{user}@{host}")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var_os(key)
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Format a document id: `PREFIX-NNNN`, zero-padded to `pad` (more digits when
/// the number outgrows it).
pub fn format_id(prefix: &str, num: u64, pad: usize) -> String {
    format!("{prefix}-{num:0pad$}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_id_pads_and_overflows() {
        assert_eq!(format_id("FEAT", 7, 4), "FEAT-0007");
        assert_eq!(format_id("FEAT", 123456, 4), "FEAT-123456");
    }

    #[test]
    fn first_congruent_is_the_smallest_number_above_max_in_the_slot() {
        // max 0: slot 0 never yields 0 (ids start at 1), slot 1 yields 1.
        assert_eq!(first_congruent(0, 0, 2), 2);
        assert_eq!(first_congruent(0, 1, 2), 1);
        // max 1, M = 2: slot 0 → 2, then 4, 6; slot 1 → 3, 5, 7.
        assert_eq!(first_congruent(1, 0, 2), 2);
        assert_eq!(first_congruent(3, 0, 2), 4);
        assert_eq!(first_congruent(1, 1, 2), 3);
        assert_eq!(first_congruent(5, 1, 2), 7);
        // A max already in the slot steps a full modulus.
        assert_eq!(first_congruent(16, 0, 16), 32);
        assert_eq!(first_congruent(5, 0, 16), 16);
        assert_eq!(first_congruent(5, 5, 16), 21);
        for max in 0..64 {
            for modulus in 2..7 {
                for slot in 0..modulus {
                    let n = first_congruent(max, slot, modulus);
                    assert!(n > max);
                    assert_eq!(n % modulus, slot);
                    assert!(n - max <= modulus, "{n} is not the smallest above {max}");
                }
            }
        }
    }

    /// The fixed hash: pinned vectors so the slot of a given `user@host` can
    /// never drift silently between versions.
    #[test]
    fn fnv1a64_matches_the_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(fnv1a64(b"dan@workbench-pc"), 0xc9a3_9bbb_c9d8_b725);
    }

    #[test]
    fn hash_slot_is_stable_and_below_the_modulus() {
        assert_eq!(hash_slot("dan@workbench-pc", 16), 5);
        assert_eq!(hash_slot("dan@workbench-pc", 2), 1);
        assert_eq!(hash_slot("alice@box", 16), 2);
        assert_eq!(hash_slot("alice@box", 4), 2);
        assert_eq!(hash_slot("alice@box", 16), hash_slot("alice@box", 16));
        for modulus in [2u64, 3, 7, 16, 1000] {
            for who in ["dan@workbench-pc", "alice@box", "unknown@localhost", ""] {
                assert!(hash_slot(who, modulus) < modulus);
            }
        }
    }

    #[test]
    fn slot_for_prefers_the_env_slot_and_validates_it() {
        assert_eq!(slot_for(16, None, "dan@workbench-pc").unwrap(), 5);
        assert_eq!(slot_for(4, Some("3"), "dan@workbench-pc").unwrap(), 3);
        assert_eq!(slot_for(4, Some(" 0 "), "dan@workbench-pc").unwrap(), 0);
        let too_big = slot_for(2, Some("2"), "x").unwrap_err().to_string();
        assert!(too_big.contains("OPYS_ID_SLOT=2"), "{too_big}");
        assert!(too_big.contains("modulus 2"), "{too_big}");
        assert!(too_big.contains("0 to 1"), "{too_big}");
        let not_int = slot_for(16, Some("abc"), "x").unwrap_err().to_string();
        assert!(not_int.contains("OPYS_ID_SLOT=\"abc\""), "{not_int}");
        assert!(not_int.contains("modulus 16"), "{not_int}");
        let negative = slot_for(16, Some("-1"), "x").unwrap_err().to_string();
        assert!(negative.contains("not an integer slot"), "{negative}");
        // Set but empty is not "unset": it is the orchestrator's unassigned
        // `$SLOT`, and must not fall back to the hash.
        let empty = slot_for(4, Some(""), "x").unwrap_err().to_string();
        assert!(empty.contains("OPYS_ID_SLOT=\"\""), "{empty}");
        assert!(empty.contains("modulus 4"), "{empty}");
        let blank = slot_for(4, Some("  "), "x").unwrap_err().to_string();
        assert!(blank.contains("not an integer slot"), "{blank}");
    }

    /// The pure bound `resolve` and `ProjectConfig::validate` share: a stride
    /// at the cap still keeps a block of ids inside the store's `i64`.
    #[test]
    fn max_modulus_keeps_ids_inside_the_store_number() {
        assert_eq!(MAX_MODULUS, 4_294_967_296);
        let last = first_congruent(u32::MAX as u64, MAX_MODULUS - 1, MAX_MODULUS)
            + 1_000_000 * MAX_MODULUS;
        assert!(i64::try_from(last).is_ok());
    }

    #[test]
    fn interleaved_steps_by_the_modulus_and_sequence_max_by_one() {
        assert_eq!(Interleaved::new(1, 4).unwrap().step(), 4);
        assert_eq!(SequenceMax.step(), 1);
    }

    /// The constructor is the only way to build an [`Interleaved`], so it is
    /// where the arithmetic's invariants are established: a modulus that
    /// cannot interleave (or is past the cap) and a slot outside `0..modulus`
    /// are refused rather than allocating in someone else's residue class.
    #[test]
    fn interleaved_new_rejects_an_unusable_slot_or_modulus() {
        // (The fields are private outside the module; this child module sees
        // them, which is what lets the accepted cases be checked at all.)
        assert_eq!(
            Interleaved::new(1, 2).unwrap(),
            Interleaved {
                slot: 1,
                modulus: 2
            }
        );
        assert_eq!(Interleaved::new(0, 2).unwrap().step(), 2);
        assert_eq!(
            Interleaved::new(MAX_MODULUS - 1, MAX_MODULUS)
                .unwrap()
                .step(),
            MAX_MODULUS
        );

        let low = Interleaved::new(0, 1).unwrap_err().to_string();
        assert!(low.contains("must be at least 2 (got 1)"), "{low}");
        let zero = Interleaved::new(0, 0).unwrap_err().to_string();
        assert!(zero.contains("must be at least 2 (got 0)"), "{zero}");
        let high = Interleaved::new(0, MAX_MODULUS + 1)
            .unwrap_err()
            .to_string();
        assert!(high.contains("must be at most 4294967296"), "{high}");
        let slot = Interleaved::new(4, 4).unwrap_err().to_string();
        assert!(slot.contains("id slot 4"), "{slot}");
        assert!(slot.contains("modulus 4"), "{slot}");
        assert!(slot.contains("valid slots: 0 to 3"), "{slot}");
        let far = Interleaved::new(99, 4).unwrap_err().to_string();
        assert!(far.contains("id slot 99"), "{far}");
    }
}
