// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! MVCC version resolution for vector freshness, extracted from the root
//! `core/search` module (TD-DECOMP-45).
//!
//! [`mvcc_resolution`] resolves the effective version of a
//! [`proximadb_records::ProximaRecord`] under multi-version concurrency —
//! [`mvcc_resolution::MvccResolver`] merges deltas, handles append-only OIDs,
//! and computes the effective version for read-after-write correctness. Depends
//! only on `proximadb-records` + `tracing`, keeping it a clean horizontal-tier leaf.

pub mod mvcc_resolution;
