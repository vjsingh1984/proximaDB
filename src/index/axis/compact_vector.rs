// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim.
//!
//! The compact vector representation types (`CompactVector`,
//! `CompactVectorCollection`, …) now live in the `proximadb-index-storage`
//! foundation crate (extracted from the root crate to shrink single-compile RSS
//! — part of the root-crate decomposition track). This file preserves the
//! `crate::index::axis::compact_vector::*` path for existing callers and is a
//! pure forwarding shim. New code should depend on
//! `proximadb_index_storage::compact_vector` directly.
pub use proximadb_index_storage::compact_vector::*;
