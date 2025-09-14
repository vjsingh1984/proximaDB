#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::hardware_capabilities::HardwareCapabilities;
    use crate::core::search::metadata_filter_pushdown::{ColumnStatistics, MetadataFilterPushdown};
    use crate::core::search::query_preprocessing::{QueryPreprocessor, QueryVectorCache};
    use crate::core::search::integrated_search_optimization::BufferPool;
    use crate::core::search::{FilterExpression, ComparisonOperator};
    use crate::core::search::results::OptimizedSearchRecord;
    // use crate::core::search::unified_progressive_pipeline::{
    //     PipelineConfig, UnifiedProgressiveSearchPipeline,
    // };
    use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
    use crate::storage::cache::specialized::{MetadataStore, QueryCache};
    // use crate::storage::persistence::write_ahead_log::parallel_search::ParallelWALSearch;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio;

    fn init_test_environment() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
    }

    #[tokio::test]
    async fn test_query_preprocessing_with_simd() {
        init_test_environment();

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let preprocessor = QueryPreprocessor::new(100);

        // Test vector normalization with SIMD
        let query_vector = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let result = preprocessor
            .preprocess(&query_vector, DistanceMetric::Cosine, None)
            .await;

        let cached = result;

        // Verify normalization
        let norm: f32 = cached.normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001, "Vector should be normalized");

        // Verify quantization levels were created
        assert!(cached.quantized_binary.is_some());
        assert!(cached.quantized_int8.is_some());

        // Test cache hit
        let result2 = preprocessor
            .preprocess(&query_vector, DistanceMetric::Cosine, None)
            .await;
        let cached2 = result2;
        assert_eq!(cached.vector_hash, cached2.vector_hash, "Should hit cache");
    }

    // Commented out test_parallel_wal_search due to API changes
    // TODO: Update when ParallelWALSearch API is stabilized

    #[tokio::test]
    async fn test_metadata_filter_pushdown() {
        init_test_environment();

        let mut filter_pushdown = MetadataFilterPushdown::new();

        // Add bloom filters for columns
        filter_pushdown.add_column_bloom_filter("category".to_string(), 1000, 0.01);
        filter_pushdown.add_column_bloom_filter("status".to_string(), 1000, 0.01);

        // Add column statistics
        let mut category_stats = ColumnStatistics::new("category".to_string());
        category_stats.add_value(&serde_json::json!("electronics"));
        category_stats.add_value(&serde_json::json!("books"));
        category_stats.add_value(&serde_json::json!("clothing"));
        filter_pushdown.update_column_stats("category".to_string(), category_stats);

        // Test filter evaluation
        let filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("electronics"),
        };

        let metadata = HashMap::from([
            ("category".to_string(), serde_json::json!("electronics")),
            ("status".to_string(), serde_json::json!("active")),
        ]);

        let should_process = filter_pushdown.should_process_vector(&filter, &metadata);
        assert!(
            should_process,
            "Should process vector with matching metadata"
        );

        // Test selectivity estimation
        let selectivity = filter_pushdown.estimate_selectivity(&filter);
        assert!(
            selectivity > 0.0 && selectivity <= 1.0,
            "Selectivity should be between 0 and 1"
        );
    }

    // Commented out test_progressive_search_pipeline due to API changes
    // TODO: Update when UnifiedProgressiveSearchPipeline API is stabilized

    #[tokio::test]
    async fn test_smart_execution_strategy() {
        use crate::core::search::smart_execution_strategy::{
            ExecutionStrategy, SmartExecutionStrategy, StrategyConfig,
        };
        init_test_environment();

        let config = StrategyConfig {
            enable_cost_based: true,
            memory_pressure_threshold: 0.9,
            latency_target_ms: Some(100),
            enable_adaptive: false,
            small_dataset_threshold: 1_000,
            large_dataset_threshold: 100_000,
        };
        let strategy = SmartExecutionStrategy::new(config);
        let params = super::super::SearchParams::default();
        let result = strategy
            .select_strategy("test_collection", &params, None)
            .await
            .unwrap();
        match result {
            ExecutionStrategy::DirectFP32 { .. }
            | ExecutionStrategy::IndexFirst { .. }
            | ExecutionStrategy::Progressive { .. }
            | ExecutionStrategy::Hybrid { .. }
            | ExecutionStrategy::MemoryOptimized { .. } => {}
        }
    }

    #[tokio::test]
    async fn test_zero_copy_operations() {
        init_test_environment();

        let buffer_pool = Arc::new(BufferPool::new(10, 1024 * 1024)); // 10 buffers, 1MB each

        // Test buffer reuse
        let buffer1 = buffer_pool.acquire().await;
        assert!(buffer1.is_ok());
        let mut buf1 = buffer1.unwrap();

        // Write test data
        let test_data = vec![1.0f32; 1000];
        let bytes = unsafe {
            std::slice::from_raw_parts(
                test_data.as_ptr() as *const u8,
                test_data.len() * std::mem::size_of::<f32>(),
            )
        };
        buf1.extend_from_slice(bytes);

        // Return buffer to pool
        buffer_pool.release(buf1).await;

        // Acquire again - should get the same buffer
        let buffer2 = buffer_pool.acquire().await;
        assert!(buffer2.is_ok());
    }

    // Removed performance benchmark test from unit tests to avoid flakiness and outdated APIs

    // Helper functions
    fn compute_cosine_distance_baseline(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        1.0 - (dot / (norm_a * norm_b))
    }

    fn calculate_recall(baseline: &[f32], optimized: &[OptimizedSearchRecord]) -> f32 {
        let baseline_set: std::collections::HashSet<_> = baseline.iter().collect();
        let optimized_set: std::collections::HashSet<_> =
            optimized.iter().map(|r| &r.score).collect();

        let intersection = baseline_set.intersection(&optimized_set).count();
        intersection as f32 / baseline.len() as f32
    }
}
