//! The id-allocation seam (ADR-0055).
//!
//! Every new document number comes from an [`IdSource`], so *where* numbers
//! come from is swappable without touching the commands: [`SequenceMax`] (the
//! default — one past the corpus-wide max, correct on a single machine under
//! the backend's inventory lock) today; a lease-block source drawing ranges
//! from a shared ledger (the `opys/ids` ref) at team scale later. Commands
//! construct the default directly for now; the construction moves behind
//! [`crate::Ctx`] when a second source exists.
//!
//! Whatever the source, the global invariant stands: ids are drawn from one
//! monotonically increasing sequence and never reused — gaps are harmless
//! (monotonicity, not density, is the contract), so an abandoned reservation
//! costs nothing.

use crate::error::Result;
use crate::store::Store;

/// A source of new document numbers.
pub trait IdSource {
    /// Reserve `count` consecutive numbers, returning the first. The caller
    /// uses `first..first + count`; an unused tail is simply a gap.
    fn reserve(&mut self, store: &mut Store, count: u64) -> Result<u64>;
}

/// The default source: one past the highest number across live docs, every
/// relation-map entry (struck or not), and the retired ledger. Safe against
/// concurrent same-machine invocations because the backend holds the exclusive
/// inventory lock from load to flush; cross-machine allocation is the
/// lease-block source's job (ADR-0055), with `renumber` as the repair of last
/// resort.
pub struct SequenceMax;

impl IdSource for SequenceMax {
    fn reserve(&mut self, store: &mut Store, count: u64) -> Result<u64> {
        debug_assert!(count > 0, "reserve of zero numbers is a caller bug");
        let _ = count; // max-based: consecutive numbers after the max are free
        Ok(store.max_doc_num()? as u64 + 1)
    }
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
}
