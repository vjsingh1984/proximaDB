/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

// 🔴 ENTIRE FILE MARKED FOR REMOVAL
// User feedback: "EnhancedEngineCompactionResult this seems very excessive for release 1"
// This entire file is redundant with storage/traits.rs CompactionResult
// All engines now use the standard CompactionResult type from traits.rs
// Vector tracking can be added to engine_metrics field if needed

/*
//! Extended compaction types for AXIS integration
//!
//! This module defines enhanced compaction result types that include
//! vector-level information needed for AXIS index updates.

use crate::core::VectorRecord;

/// Enhanced engine compaction result with vector tracking
#[derive(Debug, Clone)]
pub struct EnhancedEngineCompactionResult {
    /// Basic compaction metrics
    pub files_processed: u64,
    pub bytes_processed: u64,

    /// Vector IDs that were deleted during compaction
    pub deleted_vector_ids: Vec<String>,

    /// Vectors that were merged/updated during compaction
    pub merged_vectors: Vec<VectorRecord>,

    /// Whether full index rebuild is recommended
    pub recommend_full_rebuild: bool,
}

impl Default for EnhancedEngineCompactionResult {
    fn default() -> Self {
        Self {
            files_processed: 0,
            bytes_processed: 0,
            deleted_vector_ids: Vec::new(),
            merged_vectors: Vec::new(),
            recommend_full_rebuild: false,
        }
    }
}

// From implementations removed - engines now return EnhancedEngineCompactionResult directly
*/
