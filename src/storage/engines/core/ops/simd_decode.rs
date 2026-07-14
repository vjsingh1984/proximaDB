// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim — the `simd_decode` ops subtree has been hoisted to the
//! `proximadb-engine-core` crate (first occupant; engines extraction). All
//! `crate::storage::engines::core::ops::simd_decode::*` paths resolve unchanged
//! through this glob.
//!
//! See `docs/12-design/ROOT_CRATE_DECOMPOSITION_ENGINES_EXTRACTION_2026_07_12.adoc`.

pub use proximadb_engine_core::simd_decode::*;
