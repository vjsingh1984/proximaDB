// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! HyperLogLog distinct-cardinality estimator (ADR-037 Decision 1).
//!
//! Bounded state (`2^P` one-byte registers), merges register-wise by max — so a
//! collection's distinct estimate is the max-merge of its segments' sketches at
//! compaction, with no rescan. Feeds `FieldStatistics::distinct_estimate`
//! (`distinct_method: "hll"`) and `DocumentStatistics::unique_terms_estimate`.

use serde::{Deserialize, Serialize};

/// log2 of the register count. P=14 → 16384 registers (16 KiB), standard error
/// ≈ 1.04/√m ≈ 0.81%. A fixed precision keeps two sketches trivially mergeable.
const P: u32 = 14;
const M: usize = 1 << P; // register count

/// A mergeable HyperLogLog over 64-bit hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperLogLog {
    /// One rank per register; `0` means "register empty".
    registers: Vec<u8>,
}

impl Default for HyperLogLog {
    fn default() -> Self {
        Self::new()
    }
}

impl HyperLogLog {
    pub fn new() -> Self {
        Self {
            registers: vec![0u8; M],
        }
    }

    /// Insert a pre-computed 64-bit hash (use [`super::fnv1a_64`] on the value's
    /// bytes). Idempotent for repeated values — that is the whole point.
    pub fn insert_hash(&mut self, hash: u64) {
        // Top P bits select the register; the remaining bits give the rank
        // (position of the leftmost set bit, 1-based).
        let idx = (hash >> (64 - P)) as usize;
        let rest = hash << P; // shift index bits out; rank counts leading zeros of the rest
        let rank = if rest == 0 {
            (64 - P + 1) as u8
        } else {
            rest.leading_zeros() as u8 + 1
        };
        if let Some(reg) = self.registers.get_mut(idx)
            && rank > *reg
        {
            *reg = rank;
        }
    }

    /// Convenience: hash then insert the raw bytes of a value.
    pub fn insert_bytes(&mut self, bytes: &[u8]) {
        self.insert_hash(super::fnv1a_64(bytes));
    }

    /// Register-wise max merge — the associative, mergeable core (compaction
    /// folds segment sketches into the collection sketch). No-op on shape
    /// mismatch (both are fixed at `M` registers by construction).
    pub fn merge(&mut self, other: &HyperLogLog) {
        if self.registers.len() != other.registers.len() {
            return;
        }
        for (a, b) in self.registers.iter_mut().zip(other.registers.iter()) {
            if *b > *a {
                *a = *b;
            }
        }
    }

    /// Estimated distinct cardinality (HLL with linear-counting small-range
    /// correction). Always ≥ 0; approximate.
    pub fn estimate(&self) -> u64 {
        // alpha_m for the bias correction (m ≥ 128 → the standard constant).
        let m = M as f64;
        let alpha = 0.7213 / (1.0 + 1.079 / m);

        let mut sum = 0.0_f64;
        let mut zeros = 0u32;
        for &r in &self.registers {
            sum += 2f64.powi(-(r as i32));
            if r == 0 {
                zeros += 1;
            }
        }
        let raw = alpha * m * m / sum;

        // Small-range correction: linear counting when many registers are empty.
        if raw <= 2.5 * m && zeros > 0 {
            let lc = m * (m / zeros as f64).ln();
            return lc.round().max(0.0) as u64;
        }
        raw.round().max(0.0) as u64
    }

    /// True once any value has been inserted.
    pub fn is_empty(&self) -> bool {
        self.registers.iter().all(|&r| r == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hll_of(n: u64) -> HyperLogLog {
        let mut h = HyperLogLog::new();
        for i in 0..n {
            h.insert_bytes(format!("item-{i}").as_bytes());
        }
        h
    }

    #[test]
    fn empty_estimates_zero() {
        assert_eq!(HyperLogLog::new().estimate(), 0);
        assert!(HyperLogLog::new().is_empty());
    }

    #[test]
    fn small_cardinality_is_exact_ish() {
        // Linear counting is near-exact for tiny sets.
        let h = hll_of(10);
        let est = h.estimate();
        assert!((9..=11).contains(&est), "10 distinct -> {est}");
    }

    #[test]
    fn duplicates_do_not_inflate() {
        let mut h = HyperLogLog::new();
        for _ in 0..1000 {
            h.insert_bytes(b"same-value");
        }
        assert_eq!(h.estimate(), 1);
    }

    #[test]
    fn large_cardinality_within_two_percent() {
        let n = 100_000u64;
        let est = hll_of(n).estimate() as f64;
        let err = (est - n as f64).abs() / n as f64;
        assert!(err < 0.02, "rel err {err} for {n} distinct (est {est})");
    }

    #[test]
    fn merge_is_union() {
        // Disjoint halves merge to the full distinct count.
        let mut a = HyperLogLog::new();
        let mut b = HyperLogLog::new();
        for i in 0..50_000u64 {
            a.insert_bytes(format!("a-{i}").as_bytes());
            b.insert_bytes(format!("b-{i}").as_bytes());
        }
        a.merge(&b);
        let est = a.estimate() as f64;
        let err = (est - 100_000.0).abs() / 100_000.0;
        assert!(err < 0.02, "merged rel err {err} (est {est})");
    }

    #[test]
    fn merge_of_overlap_is_not_double_counted() {
        // Identical sketches merge to the same cardinality (idempotent union).
        let a = hll_of(20_000);
        let mut b = hll_of(20_000);
        b.merge(&a);
        let est = b.estimate() as f64;
        let err = (est - 20_000.0).abs() / 20_000.0;
        assert!(err < 0.02, "overlap rel err {err} (est {est})");
    }
}
