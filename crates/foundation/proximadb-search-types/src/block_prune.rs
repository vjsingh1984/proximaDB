// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Block-pruning configuration leaf types.
//!
//! Hoisted from `proximadb::core::search` (root-crate decomposition,
//! Slice D / D2 link 2). The old path re-exports this enum so existing
//! `crate::core::search::BlockPruneMode` references resolve unchanged.

/// Block pruning mode.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum BlockPruneMode {
    /// Prune to sqrt(total_blocks)
    Sqrt,
    /// Prune by a configured ratio
    Ratio,
    /// Prune to a fixed number of blocks
    Fixed(usize),
}
