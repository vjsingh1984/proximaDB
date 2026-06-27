// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # Streaming sketches for the statistics substrate (ADR-037, TD-174)
//!
//! Lean, in-tree, **mergeable** sketches maintained on the flush/compaction
//! write boundary — the sibling of the ADR-030 KSU resident-bytes meter. Each
//! sketch holds bounded state and merges associatively, so compaction folds
//! segment-level sketches into collection-level statistics with no rescan
//! (Decision 1: "measured once at the write boundary, read many"; the engine
//! never scans the corpus to answer "what is in here?").
//!
//! These are deliberately dependency-free (no external sketch crate): the
//! algorithms are compact and well-understood, and keeping them in-tree avoids
//! Cargo.lock / supply-chain churn on a 66-crate workspace. All hashing uses a
//! fixed FNV-1a so results are deterministic and unit-testable, and stable for
//! the lifetime of a process (sketches are resident, not persisted across binary
//! versions in v1).
//!
//! Every estimate produced here is approximate by construction; the envelope
//! labels it (`approximate: true`, `distinct_method: "hll"`) so the consumer
//! (AnvaiOps ADR-0021) never mistakes a sketch for an exact count.

mod frequent;
mod hyperloglog;
mod quantiles;
mod reservoir;

pub use frequent::{FrequentItem, FrequentItems};
pub use hyperloglog::HyperLogLog;
pub use quantiles::TDigest;
pub use reservoir::Reservoir;

/// 64-bit FNV-1a — a deterministic, dependency-free hash. Stable across runs
/// (unlike the std `DefaultHasher`, which is randomized), so sketch estimates
/// are reproducible and unit-testable.
#[inline]
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    // FNV-1a mixes the low bits well but the high bits poorly — and HyperLogLog
    // reads the *high* bits for the register index. Run the murmur3 64-bit
    // finalizer so every bit avalanches; without it short keys collide into a
    // handful of registers and the distinct estimate collapses (~10× low).
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    hash ^= hash >> 33;
    hash
}

/// Mix a seed into a hash so the same value yields independent streams (used by
/// the frequent-items / reservoir salting). Deterministic.
#[inline]
pub fn fnv1a_64_seeded(seed: u64, bytes: &[u8]) -> u64 {
    fnv1a_64(bytes) ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}
