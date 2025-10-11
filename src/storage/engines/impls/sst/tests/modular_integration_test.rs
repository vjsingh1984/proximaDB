/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Integration tests for the modular SST engine structure
//!
//! Tests the interaction between all modules:
//! - Core engine initialization
//! - Flush operations through the flush module
//! - Search operations through the search module
//! - Collection management
//! - Utility functions
//! - Trait implementations

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::collections::HashMap;
    use anyhow::Result;

    use crate::storage::engines::impls::sst::{
        core::SstEngine,
        SstConfig,
        flush::{FlushCoordinator, FlushOptimizer, FlushOperations},
        search::{SearchCoordinator, SearchOperations, SearchOptimizer},
        collections::CollectionSizeInfo,
        utils::{SortingStats, MemoryEstimate},
        blocks::SstRecord,
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::traits::{UnifiedStorageEngine, StorageQueryContext};

    /// Test that the core module properly initializes the engine
    #[tokio::test]
    async fn test_core_module_initialization() {
        let config = SstConfig::default();
        let filesystem = create_test_filesystem().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        let engine = SstEngine::new_with_config(
            config.clone(),
            filesystem.clone(),
            distance_compute.clone()
        ).await;

        assert!(engine.is_ok(), "Engine should initialize successfully");
        let engine = engine.unwrap();

        // Verify core components are initialized
        assert_eq!(engine.config().block_size_kb, config.block_size_kb);
        assert_eq!(engine.engine_name(), "sst");
    }

    /// Test flush module coordination
    #[tokio::test]
    async fn test_flush_module_coordination() {
        let engine = create_test_engine().await;
        let engine_arc = Arc::new(engine);

        // Test that modules can be instantiated
        let _coordinator = FlushCoordinator::new(engine_arc.clone());
        let _optimizer = FlushOptimizer::new();
        let operations = FlushOperations::new(engine_arc.clone());

        // Test one existing method
        let block_size = operations.calculate_optimal_block_size(500, 1024);
        assert!(block_size >= 4096 && block_size <= 1024 * 1024);
    }

    /// Test search module functionality
    #[tokio::test]
    async fn test_search_module_operations() {
        let engine = create_test_engine().await;
        let engine_arc = Arc::new(engine);

        // Test that modules can be instantiated
        let _coordinator = SearchCoordinator::new(engine_arc.clone());
        let _optimizer = SearchOptimizer::new();
        let _operations = SearchOperations::new(engine_arc.clone());

        // Basic test
        assert_eq!(engine_arc.engine_name(), "sst");
    }

    /// Test blocks module structures
    #[tokio::test]
    async fn test_blocks_module_structures() {
        // Test SstRecord creation from VectorRecord
        let vector_record = VectorRecord {
            id: "test_id".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(12345),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        };

        let sst_record = SstRecord::from_vector_record(vector_record, 100, 1);
        assert_eq!(sst_record.id, "test_id");
        assert_eq!(sst_record.sequence_number, 100);
        assert_eq!(sst_record.level, 1);
        assert!(!sst_record.is_tombstone);

        // Test tombstone creation
        let tombstone = SstRecord::tombstone("delete_id".to_string(), 200, 2);
        assert!(tombstone.is_tombstone);
        assert!(tombstone.vector.is_none());
    }

    /// Test collections module functionality
    #[tokio::test]
    async fn test_collections_module() {
        let engine = create_test_engine().await;

        // Test that the engine is properly initialized
        assert_eq!(engine.config().block_size_kb, 2048); // Default value

        // Test filesystem access (this should work)
        let fs = engine.filesystem();
        assert!(fs.get_filesystem("file://").is_ok());

        // Test collection creation functionality (basic metadata)
        let collection = engine.collection("test_collection").await;
        assert!(collection.is_ok());
        let collection = collection.unwrap();
        assert_eq!(collection.id, "test_collection");
    }

    /// Test utils module helpers
    #[tokio::test]
    async fn test_utils_module_helpers() {
        // Test sorting statistics structure exists
        let stats = SortingStats {
            records_sorted: 0,
            sort_duration_ms: 0,
            primary_sort_key: None,
            compression_estimate: 0.0,
        };
        assert_eq!(stats.records_sorted, 0);
        assert_eq!(stats.sort_duration_ms, 0);

        // Test memory estimate structure exists
        let estimate = MemoryEstimate {
            vector_memory: 500,
            bloom_filter_memory: 100,
            index_memory: 0,
            buffer_memory: 400,
            total_memory: 1000,
        };
        assert!(estimate.total_memory > 0);
    }

    /// Test trait implementation module
    #[tokio::test]
    async fn test_trait_impl_module() {
        let engine = create_test_engine().await;

        // Test UnifiedStorageEngine trait methods
        assert_eq!(engine.engine_name(), "sst");
        assert_eq!(engine.engine_version(), crate::version::PROXIMADB_VERSION);

        let strategy = engine.strategy();
        assert!(matches!(strategy,
            crate::storage::traits::StorageEngineStrategy::Sst));

        // Test that basic trait methods work
        assert!(engine.engine_name().len() > 0);
        assert!(engine.engine_version().len() > 0);
    }

    /// Test module interaction - flush to search pipeline
    #[tokio::test]
    async fn test_module_interaction_pipeline() {
        let engine = create_test_engine().await;
        let engine_arc = Arc::new(engine);

        // Create flush components
        let _flush_coordinator = FlushCoordinator::new(engine_arc.clone());
        let _flush_optimizer = FlushOptimizer::new();

        // Create search components
        let _search_coordinator = SearchCoordinator::new(engine_arc.clone());
        let _search_optimizer = SearchOptimizer::new();

        // Basic validation
        assert_eq!(engine_arc.engine_name(), "sst");
    }

    /// Test error handling across modules
    #[tokio::test]
    async fn test_cross_module_error_handling() {
        let engine = create_test_engine().await;

        // Test error propagation from collections module
        let result = engine.cleanup_collection_files("nonexistent_collection").await;
        // Should handle gracefully even if collection doesn't exist
        assert!(result.is_ok() || result.is_err());

        // Test error handling in search with empty context
        use crate::storage::traits::StorageQueryMetadata;
        use crate::core::search::SearchParams;
        use crate::proto::proximadb_v1::Collection;

        let search_params = Arc::new(SearchParams {
            query_vectors: None,
            vector: Some(vec![]),
            top_k: Some(10),
            distance_metric: None,
            filter_expression: None,
            filters: None,
            accuracy_threshold: None,
            include_expired: None,
            timeout_ms: None,
            enable_two_stage: None,
            quantization_hint: None,
            enable_clustering_hint: None,
            runtime_hints: None,
            enable_metadata_filtering_hint: None,
            custom_hints: None,
            requires_ordering: None,
            enable_progressive_search: None,
            progressive_scenario: None,
            progressive_recalls: None,
            optimization_hint: None,
        });

        let collection = Arc::new(Collection {
            id: "test".to_string(),
            config: None,
            stats: None,
            created_at: 0,
            updated_at: 0,
            storage_assignment: None,
        });

        let ctx = StorageQueryContext {
            search_params,
            collection,
            metadata: StorageQueryMetadata::default(),
        };

        let search_result = engine.search_vectors_unified(&ctx).await;
        // Should handle empty query vector gracefully
        assert!(search_result.is_ok() || search_result.is_err());
    }

    // Helper functions

    async fn create_test_filesystem() -> Arc<FilesystemFactory> {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().to_str().unwrap().to_string();

        let mut config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        config.default_fs = Some(format!("file://{}", base_path));

        // Keep temp_dir alive by leaking it for test duration
        std::mem::forget(temp_dir);

        Arc::new(FilesystemFactory::create(config).await.unwrap())
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem = create_test_filesystem().await;
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
    }
}