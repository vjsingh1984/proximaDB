// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim — the RAPTOR writer has been hoisted to the
//! `proximadb-raptor-engine` crate (the biggest raptor leaf, ~5,800 LOC, moved
//! out of the root). `crate::storage::engines::raptor::writer::RaptorWriter`
//! resolves unchanged through this re-export.
//!
//! See `docs/12-design/ROOT_CRATE_DECOMPOSITION_ENGINES_EXTRACTION_2026_07_12.adoc`.

pub use proximadb_raptor_engine::RaptorWriter;
