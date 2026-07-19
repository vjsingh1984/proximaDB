// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim.
//!
//! The zero-overhead vector storage types (`ZeroOverheadVector`,
//! `ZeroOverheadCollection`, `CollectionConfig`, `QuantizationMethod`, …) now
//! live in the `proximadb-index-storage` foundation crate (extracted from the
//! root crate to shrink single-compile RSS — part of the root-crate
//! decomposition track). This file preserves the
//! `crate::index::axis::zero_overhead_vector::*` path for existing callers and
//! is a pure forwarding shim. New code should depend on
//! `proximadb_index_storage::zero_overhead_vector` directly.
pub use proximadb_index_storage::zero_overhead_vector::*;
