/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tests for AXIS index integration with compaction process

#[cfg(test)]
mod tests {
    use super::super::*;

    use crate::storage::traits::CompactionResult;
    use chrono::Utc;
    use std::collections::HashMap;

    // Note: These tests focus on the CompactionAxisUpdater behavior without a real AxisManager.
    // Testing with a real AxisManager would require a more complex setup and is covered
    // in integration tests.

    #[tokio::test]
    async fn test_compaction_axis_updater_with_no_manager() {
        // Test that CompactionAxisUpdater works without an AxisManager
        let updater = CompactionAxisUpdater::new(None);

        let compaction_result = CompactionResult {
            success: true,
            collections_affected: vec!["test_collection".to_string()],
            entries_processed: Some(3),
            entries_removed: Some(2),
            bytes_read: Some(1000),
            bytes_written: Some(500),
            input_files: Some(5),
            output_files: Some(1),
            duration_ms: Some(100),
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        let deleted_vector_ids = vec!["vec_1".to_string(), "vec_3".to_string()];
        let merged_vectors = vec![];

        // Should succeed without doing anything
        updater
            .update_indexes_after_compaction(
                "test_collection",
                &compaction_result,
                &deleted_vector_ids,
                &merged_vectors,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_compaction_stats_no_manager() {
        // Test stats retrieval without an AxisManager
        let updater = CompactionAxisUpdater::new(None);
        let stats = updater
            .get_compaction_stats("test_collection")
            .await
            .unwrap();

        assert_eq!(stats.total_indexes, 0);
        assert_eq!(stats.dynamic_indexes, 0);
        assert_eq!(stats.static_indexes, 0);
        assert_eq!(stats.total_vectors_indexed, 0);
        assert_eq!(stats.total_memory_usage_bytes, 0);
    }

    #[tokio::test]
    async fn test_compaction_result_structure() {
        // Test that CompactionResult structure is correctly handled
        let compaction_result = CompactionResult {
            success: true,
            collections_affected: vec!["test_collection".to_string()],
            entries_processed: Some(100),
            entries_removed: Some(25),
            bytes_read: Some(10000),
            bytes_written: Some(7500),
            input_files: Some(5),
            output_files: Some(1),
            duration_ms: Some(500),
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        assert!(compaction_result.success);
        assert_eq!(compaction_result.collections_affected.len(), 1);
        assert_eq!(compaction_result.entries_processed, Some(100));
        assert_eq!(compaction_result.entries_removed, Some(25));
    }
}
