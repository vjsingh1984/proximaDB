// Copyright 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Re-export shim — `WriteStrategyFactory` hoisted to `proximadb-storage-common`
//! (TD-DECOMP-82). All `crate::storage::persistence::filesystem::write_strategy::*`
//! paths resolve unchanged.

pub use proximadb_storage_common::write_strategy::{MetadataWriteStrategy, WriteStrategyFactory};
