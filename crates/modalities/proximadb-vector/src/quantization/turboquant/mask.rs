//! Packed-bitmask allowlist primitive.
//!
//! In-kernel allowlist filtering pushes the candidate set directly into the
//! scoring loop: blocks of vectors whose mask bits are all zero are
//! short-circuited before any LUT lookup or scoring work. Per LLD §"In-
//! Kernel Allowlist", this avoids the recall collapse that
//! oversample-then-post-filter suffers at small selectivity.
//!
//! ## Wire shape
//!
//! The bitmap is a packed `&[u64]` where bit `i` (little-endian within the
//! word `bitmap[i >> 6]`) is set when slot `i` is allowed. The shared
//! `CandidateMaskSet` impl (P5) constructs and owns the buffer; the
//! kernel (P4) reads it via the helpers below. Both layers must agree on
//! the same bit ordering — that agreement lives here and nowhere else.

use std::sync::atomic::{AtomicU64, Ordering};

/// Process-global cumulative count of 32-vector blocks short-circuited by
/// the mask early-exit path. Tests sample it before/after a search to
/// verify the skip path fires; production callers can expose it via the
/// Prometheus metric registered in P8.
pub static BLOCKS_SKIPPED_BY_MASK: AtomicU64 = AtomicU64::new(0);

/// Read the cumulative block-skip counter.
pub fn blocks_skipped_by_mask() -> u64 {
    BLOCKS_SKIPPED_BY_MASK.load(Ordering::Relaxed)
}

/// Reset the block-skip counter. Tests call this before issuing a
/// selective search to take a clean delta.
pub fn reset_blocks_skipped_by_mask() {
    BLOCKS_SKIPPED_BY_MASK.store(0, Ordering::Relaxed);
}

/// True iff slot `slot` is allowed by `mask`. The caller guarantees
/// `mask.len() * 64 >= n_vectors`; bits at index `>= n_vectors` are
/// ignored.
#[inline(always)]
pub fn mask_allows(mask: &[u64], slot: usize) -> bool {
    (mask[slot >> 6] >> (slot & 63)) & 1 != 0
}

/// Block-level early-exit predicate: true iff at least one slot in the
/// 32-vector block starting at `base_vec` is allowed by `mask`. Returns
/// true unconditionally when no mask is supplied, so the kernel only
/// short-circuits when filtering is requested.
///
/// `base_vec` is always a multiple of 32 (the block size in the SIMD
/// layout). The slot bitmap is packed 64 slots per `u64`, so the relevant
/// 32-bit window is either the low or high half of a single word.
#[inline(always)]
pub fn block_has_allowed(mask: Option<&[u64]>, base_vec: usize) -> bool {
    match mask {
        None => true,
        Some(m) => {
            let word = m[base_vec >> 6];
            let bit_offset = base_vec & 63;
            let allowed = ((word >> bit_offset) & 0xFFFF_FFFF) != 0;
            if !allowed {
                BLOCKS_SKIPPED_BY_MASK.fetch_add(1, Ordering::Relaxed);
            }
            allowed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests in this file mutate the global skip counter. Run them
    /// sequentially via `--test-threads=1` (the workspace already
    /// configures this for port-binding tests; same constraint applies
    /// here). Each test resets at start.
    fn reset() {
        reset_blocks_skipped_by_mask();
    }

    #[test]
    fn mask_allows_low_bits() {
        let mask = vec![0b1010u64];
        assert!(!mask_allows(&mask, 0));
        assert!(mask_allows(&mask, 1));
        assert!(!mask_allows(&mask, 2));
        assert!(mask_allows(&mask, 3));
    }

    #[test]
    fn mask_allows_crosses_word_boundary() {
        let mut mask = vec![0u64; 2];
        // Set bit 65 → word index 1, bit offset 1.
        mask[1] |= 1u64 << 1;
        assert!(!mask_allows(&mask, 64));
        assert!(mask_allows(&mask, 65));
        assert!(!mask_allows(&mask, 66));
    }

    #[test]
    fn block_has_allowed_returns_true_for_no_mask() {
        reset();
        let before = blocks_skipped_by_mask();
        assert!(block_has_allowed(None, 0));
        assert!(block_has_allowed(None, 32));
        assert!(block_has_allowed(None, 64));
        // Counter must NOT advance when there's no mask.
        assert_eq!(blocks_skipped_by_mask(), before);
    }

    #[test]
    fn block_has_allowed_skips_empty_low_block() {
        reset();
        let mask = vec![0u64];
        let before = blocks_skipped_by_mask();
        assert!(!block_has_allowed(Some(&mask), 0));
        assert_eq!(blocks_skipped_by_mask(), before + 1);
    }

    #[test]
    fn block_has_allowed_skips_empty_high_block() {
        reset();
        // Only the low 32 bits are set; the block starting at base_vec=32
        // (high half of the word) should be skipped.
        let mask = vec![0xFFFF_FFFFu64];
        let before = blocks_skipped_by_mask();
        assert!(block_has_allowed(Some(&mask), 0));
        assert!(!block_has_allowed(Some(&mask), 32));
        assert_eq!(blocks_skipped_by_mask(), before + 1);
    }

    #[test]
    fn block_has_allowed_keeps_block_with_any_set_bit() {
        reset();
        // Set bit 17 only — should still keep block at base_vec=0.
        let mask = vec![1u64 << 17];
        let before = blocks_skipped_by_mask();
        assert!(block_has_allowed(Some(&mask), 0));
        assert_eq!(blocks_skipped_by_mask(), before, "no skip recorded");
        // And block at base_vec=32 should skip.
        assert!(!block_has_allowed(Some(&mask), 32));
        assert_eq!(blocks_skipped_by_mask(), before + 1);
    }

    #[test]
    fn counter_resets() {
        reset();
        let mask = vec![0u64];
        block_has_allowed(Some(&mask), 0);
        block_has_allowed(Some(&mask), 32);
        assert!(blocks_skipped_by_mask() >= 2);
        reset_blocks_skipped_by_mask();
        assert_eq!(blocks_skipped_by_mask(), 0);
    }
}
