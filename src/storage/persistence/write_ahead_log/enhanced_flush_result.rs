// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Enhanced Flush Result that carries canonical record data for projection indexing

use crate::storage::traits::FlushResult;
use proximadb_records::ProximaRecord;

/// Enhanced flush result that includes the actual record data.
/// This is used to pass flushed records to projection/index builders.
#[derive(Debug, Clone)]
pub struct EnhancedFlushResult {
    /// Base flush result with standard metrics
    pub base: FlushResult,

    /// The actual records that were flushed.
    pub vector_records: Vec<ProximaRecord>,

    /// IDs of vectors that were deleted during flush (e.g., expired)
    pub deleted_vector_ids: Vec<String>,
}

impl EnhancedFlushResult {
    /// Create from base result and vectors
    pub fn new(base: FlushResult, vector_records: Vec<ProximaRecord>) -> Self {
        Self {
            base,
            vector_records,
            deleted_vector_ids: Vec::new(),
        }
    }

    /// Create with deletions
    pub fn with_deletions(
        base: FlushResult,
        vector_records: Vec<ProximaRecord>,
        deleted_vector_ids: Vec<String>,
    ) -> Self {
        Self {
            base,
            vector_records,
            deleted_vector_ids,
        }
    }
}
