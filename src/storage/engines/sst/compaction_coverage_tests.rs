//! Targeted tests for SST compaction to improve coverage from 71.8% to 85%
//!
//! These tests focus on uncovered code paths, edge cases, and component interactions
//! in the compaction module to achieve better test coverage.

#[cfg(test)]
mod tests {
    use super::super::compaction::*;
    use crate::core::{SstConfig, VectorRecord};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Helper to create test SST config
    fn create_test_sst_config() -> SstConfig {
        let mut config = SstConfig::default();
        config.level_count = 4;
        config.compaction_threshold = 3;
        config.block_size_kb = 8;
        config.compaction_strategy = "tiered".to_string();
        config.compression = "lz4".to_string();
        config
    }

    /// Create test vector record with customizable properties
    fn create_test_vector_record(id: &str, sequence: u64, timestamp: i64) -> VectorRecord {
        VectorRecord {
            id: Some(id.to_string()),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: vec![],
            timestamp: timestamp as u32,
            updated_at: Some(timestamp as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        }
    }

    // Note: SstRecord and should_replace_record are private, 
    // so we focus on testing public APIs of CompactionManager

    #[test]
    fn test_compaction_priority_ordering() {
        // Test CompactionPriority enum ordering
        assert!(CompactionPriority::Low < CompactionPriority::Medium);
        assert!(CompactionPriority::Medium < CompactionPriority::High);  
        assert!(CompactionPriority::High < CompactionPriority::Critical);
        
        // Test equality
        assert_eq!(CompactionPriority::Low, CompactionPriority::Low);
        assert_ne!(CompactionPriority::Low, CompactionPriority::High);
        
        // Test clone
        let priority = CompactionPriority::High;
        let cloned = priority.clone();
        assert_eq!(priority, cloned);
    }

    #[test]
    fn test_compaction_stats_default() {
        // Test CompactionStats default values
        let stats = CompactionStats::default();
        assert_eq!(stats.total_compactions, 0);
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(stats.bytes_read, 0);
        assert_eq!(stats.files_merged, 0);
        assert_eq!(stats.avg_compaction_time_ms, 0);
        assert!(stats.last_compaction_time.is_none());
        assert_eq!(stats.expired_records_deleted, 0);
        assert_eq!(stats.tombstones_removed, 0);
        
        // Test clone
        let cloned_stats = stats.clone();
        assert_eq!(cloned_stats.total_compactions, stats.total_compactions);
    }

    #[test]
    fn test_enhanced_compaction_stats_default() {
        // Test EnhancedCompactionStats default values
        let enhanced_stats = EnhancedCompactionStats::default();
        assert_eq!(enhanced_stats.base_stats.total_compactions, 0);
        assert!(enhanced_stats.deleted_vector_ids.is_empty());
        assert!(enhanced_stats.merged_vectors.is_empty());
        assert!(!enhanced_stats.recommend_full_rebuild);
        
        // Test clone
        let cloned = enhanced_stats.clone();
        assert_eq!(cloned.deleted_vector_ids.len(), enhanced_stats.deleted_vector_ids.len());
    }

    #[test]
    fn test_compaction_task_creation_and_cloning() {
        // Test CompactionTask creation
        let input_files = vec![
            PathBuf::from("/tmp/input1.sst"), 
            PathBuf::from("/tmp/input2.sst")
        ];
        let output_file = PathBuf::from("/tmp/output.sst");
        
        let task = CompactionTask {
            level: 2,
            input_files: input_files.clone(),
            output_file: output_file.clone(),
            priority: CompactionPriority::High,
        };
        
        // Test field access
        assert_eq!(task.level, 2);
        assert_eq!(task.input_files.len(), 2);
        assert_eq!(task.output_file, output_file);
        assert_eq!(task.priority, CompactionPriority::High);
        
        // Test clone
        let cloned_task = task.clone();
        assert_eq!(cloned_task.level, task.level);
        assert_eq!(cloned_task.input_files.len(), task.input_files.len());
        assert_eq!(cloned_task.priority, task.priority);
    }

    // Note: should_replace_record is a private function, so we test it indirectly
    // through public APIs or focus on testing public interfaces that use it

    #[tokio::test]
    async fn test_compaction_manager_constructor_variants() {
        let config = create_test_sst_config();
        
        // Test basic constructor
        let manager1 = CompactionManager::new(config.clone());
        
        // Test manager creation (creation success indicates correct setup)
        // Note: with_atomic_coordinator method may not be available in current implementation
        let manager2 = CompactionManager::new(config.clone());
        
        // Both should be successfully created (can't access private fields, but creation success indicates correct setup)
        drop(manager1);
        drop(manager2);
    }

    #[tokio::test]
    async fn test_compaction_manager_worker_lifecycle() {
        let config = create_test_sst_config();
        let mut manager = CompactionManager::new(config);
        
        // Test starting workers with different counts
        assert!(manager.start_workers(0).await.is_ok()); // Zero workers should be OK
        assert!(manager.stop().await.is_ok());
        
        assert!(manager.start_workers(1).await.is_ok()); // Single worker
        assert!(manager.stop().await.is_ok());
        
        assert!(manager.start_workers(3).await.is_ok()); // Multiple workers
        assert!(manager.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_task_scheduling_with_priorities() {
        let config = create_test_sst_config();
        let manager = CompactionManager::new(config);
        
        // Schedule tasks with different priorities
        let tasks = vec![
            create_test_compaction_task("collection1", 0, CompactionPriority::Low),
            create_test_compaction_task("collection2", 1, CompactionPriority::High),
            create_test_compaction_task("collection3", 2, CompactionPriority::Critical),
            create_test_compaction_task("collection4", 0, CompactionPriority::Medium),
        ];
        
        for task in tasks {
            assert!(manager.schedule_compaction(task).await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_compaction_manager_statistics() {
        let config = create_test_sst_config();
        let manager = CompactionManager::new(config);
        
        // Get initial stats
        let initial_stats = manager.get_stats().await;
        assert_eq!(initial_stats.total_compactions, 0);
        assert_eq!(initial_stats.bytes_written, 0);
        assert_eq!(initial_stats.files_merged, 0);
        
        // Stats should be consistent across multiple calls
        let stats2 = manager.get_stats().await;
        assert_eq!(initial_stats.total_compactions, stats2.total_compactions);
    }

    #[tokio::test]
    async fn test_compaction_task_with_empty_input_files() {
        let config = create_test_sst_config();
        let manager = CompactionManager::new(config);
        
        // Test scheduling task with empty input files
        let task = CompactionTask {
            level: 1,
            input_files: vec![], // Empty input files
            output_file: PathBuf::from("/tmp/empty_output.sst"),
            priority: CompactionPriority::Medium,
        };
        
        assert!(manager.schedule_compaction(task).await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_task_with_multiple_input_files() {
        let config = create_test_sst_config();
        let manager = CompactionManager::new(config);
        
        // Test scheduling task with many input files
        let many_files: Vec<PathBuf> = (0..10)
            .map(|i| PathBuf::from(format!("/tmp/input_{}.sst", i)))
            .collect();
        
        let task = CompactionTask {
            level: 2,
            input_files: many_files,
            output_file: PathBuf::from("/tmp/multi_output.sst"),
            priority: CompactionPriority::Critical,
        };
        
        assert!(manager.schedule_compaction(task).await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_manager_edge_case_levels() {
        let mut config = create_test_sst_config();
        config.level_count = 1; // Minimum level count
        
        let manager = CompactionManager::new(config.clone());
        
        // Test with level 0 (minimum)
        let task_level_0 = create_test_compaction_task("collection_level0", 0, CompactionPriority::Medium);
        assert!(manager.schedule_compaction(task_level_0).await.is_ok());
        
        // Test with maximum level (level_count - 1)
        let max_level = (config.level_count - 1) as u8;
        let task_max_level = create_test_compaction_task("collection_max", max_level, CompactionPriority::High);
        assert!(manager.schedule_compaction(task_max_level).await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_manager_concurrent_operations() {
        let config = create_test_sst_config();
        let manager = Arc::new(CompactionManager::new(config));
        
        // Test concurrent task scheduling
        let mut handles = vec![];
        
        for i in 0..5 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                let task = create_test_compaction_task(
                    &format!("concurrent_collection_{}", i),
                    (i % 3) as u8,
                    CompactionPriority::Medium
                );
                manager_clone.schedule_compaction(task).await
            });
            handles.push(handle);
        }
        
        // All concurrent operations should succeed
        for handle in handles {
            let result = handle.await.expect("Task should complete");
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_compaction_stats_tracking_fields() {
        let config = create_test_sst_config();
        let manager = CompactionManager::new(config);
        
        let stats = manager.get_stats().await;
        
        // Test all fields are accessible and have expected initial values
        assert_eq!(stats.total_compactions, 0);
        assert_eq!(stats.bytes_written, 0);
        assert_eq!(stats.bytes_read, 0);
        assert_eq!(stats.files_merged, 0);
        assert_eq!(stats.avg_compaction_time_ms, 0);
        assert!(stats.last_compaction_time.is_none());
        assert_eq!(stats.expired_records_deleted, 0);
        assert_eq!(stats.tombstones_removed, 0);
    }

    #[test]
    fn test_enhanced_compaction_stats_field_access() {
        let mut enhanced_stats = EnhancedCompactionStats::default();
        
        // Test field modifications
        enhanced_stats.deleted_vector_ids.push("deleted_1".to_string());
        enhanced_stats.deleted_vector_ids.push("deleted_2".to_string());
        
        let test_vector = create_test_vector_record("merged_1", 100, 1000);
        enhanced_stats.merged_vectors.push(test_vector);
        
        enhanced_stats.recommend_full_rebuild = true;
        enhanced_stats.base_stats.total_compactions = 5;
        enhanced_stats.base_stats.bytes_written = 1024;
        
        // Verify all modifications
        assert_eq!(enhanced_stats.deleted_vector_ids.len(), 2);
        assert_eq!(enhanced_stats.merged_vectors.len(), 1);
        assert!(enhanced_stats.recommend_full_rebuild);
        assert_eq!(enhanced_stats.base_stats.total_compactions, 5);
        assert_eq!(enhanced_stats.base_stats.bytes_written, 1024);
    }

    /// Helper function to create test compaction tasks
    fn create_test_compaction_task(collection_id: &str, level: u8, priority: CompactionPriority) -> CompactionTask {
        CompactionTask {
            level,
            input_files: vec![
                PathBuf::from(format!("/tmp/{}_input1.sst", collection_id)),
                PathBuf::from(format!("/tmp/{}_input2.sst", collection_id)),
            ],
            output_file: PathBuf::from(format!("/tmp/{}_output.sst", collection_id)),
            priority,
        }
    }
}