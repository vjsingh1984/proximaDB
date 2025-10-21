/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 */

//! Typed result structures for batch operations
//!
//! This module provides clean, type-safe result structures for vector operations,
//! eliminating the need for JSON serialization in the service layer.

use serde::{Deserialize, Serialize};

/// Metrics for a batch operation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperationMetrics {
    /// Total number of vectors processed
    pub total_processed: i64,

    /// Number of vectors successfully processed
    pub successful_count: i64,

    /// Number of vectors that failed
    pub failed_count: i64,

    /// Number of vectors updated (vs inserted)
    pub updated_count: i64,

    /// Total processing time in microseconds
    pub processing_time_us: i64,

    /// Time spent writing to WAL in microseconds
    pub wal_write_time_us: i64,

    /// Time spent updating indexes in microseconds
    pub index_update_time_us: i64,
}

/// Result of a batch vector operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchOperationResult {
    /// Whether the operation succeeded
    pub success: bool,

    /// IDs of vectors that were processed
    pub vector_ids: Vec<String>,

    /// Operation metrics
    pub metrics: OperationMetrics,

    /// Error messages for failed vectors
    pub errors: Vec<String>,

    /// Optional error code
    pub error_code: Option<String>,
}

impl BatchOperationResult {
    /// Create a successful batch result
    pub fn success(vector_ids: Vec<String>, metrics: OperationMetrics) -> Self {
        Self {
            success: true,
            vector_ids,
            metrics,
            errors: vec![],
            error_code: None,
        }
    }

    /// Create a failed batch result
    pub fn failure(error_message: String, error_code: String) -> Self {
        Self {
            success: false,
            vector_ids: vec![],
            metrics: OperationMetrics::default(),
            errors: vec![error_message],
            error_code: Some(error_code),
        }
    }

    /// Create a partial success result
    pub fn partial(
        vector_ids: Vec<String>,
        metrics: OperationMetrics,
        errors: Vec<String>,
    ) -> Self {
        Self {
            success: !vector_ids.is_empty(),
            vector_ids,
            metrics,
            errors,
            error_code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_result_success() {
        let metrics = OperationMetrics {
            total_processed: 10,
            successful_count: 10,
            failed_count: 0,
            ..Default::default()
        };

        let result =
            BatchOperationResult::success(vec!["id1".to_string(), "id2".to_string()], metrics);

        assert!(result.success);
        assert_eq!(result.vector_ids.len(), 2);
        assert_eq!(result.metrics.successful_count, 10);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_failure() {
        let result = BatchOperationResult::failure(
            "Collection not found".to_string(),
            "NOT_FOUND".to_string(),
        );

        assert!(!result.success);
        assert!(result.vector_ids.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.error_code, Some("NOT_FOUND".to_string()));
    }
}
