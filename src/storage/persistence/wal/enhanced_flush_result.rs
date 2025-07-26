// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Enhanced Flush Result that carries vector data for AXIS indexing

use crate::core::VectorRecord;
use crate::storage::traits::FlushResult;

/// Enhanced flush result that includes the actual vector data
/// This is used to pass vectors from flush to AXIS indexing
#[derive(Debug, Clone)]
pub struct EnhancedFlushResult {
    /// Base flush result with standard metrics
    pub base: FlushResult,
    
    /// The actual vector records that were flushed
    /// This is what AXIS needs for indexing
    pub vector_records: Vec<VectorRecord>,
    
    /// IDs of vectors that were deleted during flush (e.g., expired)
    pub deleted_vector_ids: Vec<String>,
}

impl EnhancedFlushResult {
    /// Create from base result and vectors
    pub fn new(base: FlushResult, vector_records: Vec<VectorRecord>) -> Self {
        Self {
            base,
            vector_records,
            deleted_vector_ids: Vec::new(),
        }
    }
    
    /// Create with deletions
    pub fn with_deletions(
        base: FlushResult, 
        vector_records: Vec<VectorRecord>,
        deleted_vector_ids: Vec<String>
    ) -> Self {
        Self {
            base,
            vector_records,
            deleted_vector_ids,
        }
    }
}