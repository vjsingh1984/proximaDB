// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Re-export shim.
//!
//! The strongly-typed metadata types (`MetadataValue`, `TypedMetadata`) now
//! live in the `proximadb-data-model` foundation crate (extracted from the root
//! crate to shrink single-compile RSS — part of the root-crate decomposition
//! track). This file preserves the `crate::core::metadata_types::*` path for
//! existing callers and is a pure forwarding shim. New code should depend on
//! `proximadb_data_model::metadata_types` directly.
pub use proximadb_data_model::metadata_types::*;
