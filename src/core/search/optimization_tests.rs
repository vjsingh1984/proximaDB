#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::hardware_capabilities::HardwareCapabilities;
    use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
    use crate::storage::cache::specialized::{MetadataStore, QueryCache, VectorStore};
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
        let preprocessor = QueryPreprocessor::new(hardware.clone(), 100);

        // Test vector normalization with SIMD
        let query_vector = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let result = preprocessor
            .preprocess(&query_vector, DistanceMetric::Cosine)
            .await;

        assert!(result.is_ok());
        let cached = result.unwrap();

        // Verify normalization
        let norm: f32 = cached.normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001, "Vector should be normalized");

        // Verify quantization levels were created
        assert!(cached.quantized_binary.is_some());
        assert!(cached.quantized_int8.is_some());

        // Test cache hit
        let result2 = preprocessor
            .preprocess(&query_vector, DistanceMetric::Cosine)
            .await;
        assert!(result2.is_ok());
        let cached2 = result2.unwrap();
        assert_eq!(cached.vector_hash, cached2.vector_hash, "Should hit cache");
    }

    #[tokio::test]
    async fn test_parallel_wal_search() {
        init_test_environment();

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute =
            Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::new());

        let parallel_search = ParallelWALSearch::new(
            hardware.clone(),
            distance_compute.clone(),
            100, // batch_size
            2.0, // early_termination_multiplier
        );

        // Create test batches
        let mut batches = Vec::new();
        for i in 0..10 {
            let mut batch_vectors = Vec::new();
            for j in 0..50 {
                let vector: Vec<f32> = (0..128).map(|k| ((i * 50 + j + k) as f32).sin()).collect();
                batch_vectors.push(vector);
            }
            batches.push(batch_vectors);
        }

        // Test parallel search
        let query = vec![0.5; 128];
        let results = parallel_search
            .search_parallel(&batches, &query, 10, DistanceMetric::Cosine)
            .await;

        assert!(results.is_ok());
        let search_results = results.unwrap();
        assert_eq!(search_results.len(), 10, "Should return top-k results");

        // Verify results are sorted by distance
        for i in 1..search_results.len() {
            assert!(
                search_results[i - 1].distance <= search_results[i].distance,
                "Results should be sorted by distance"
            );
        }
    }

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

    #[tokio::test]
    async fn test_progressive_search_pipeline() {
        init_test_environment();

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let query_preprocessor = Arc::new(QueryPreprocessor::new(hardware.clone(), 100));

        let config = PipelineConfig {
            enable_binary_stage: true,
            enable_int8_stage: true,
            enable_pq_stage: true,
            binary_selectivity: 0.1,
            int8_selectivity: 0.05,
            pq_selectivity: 0.02,
            max_candidates: 1000,
        };

        let pipeline = UnifiedProgressiveSearchPipeline::new(query_preprocessor.clone(), config);

        // Create test data
        let vectors: Vec<Vec<f32>> = (0..1000)
            .map(|i| (0..128).map(|j| ((i + j) as f32).sin()).collect())
            .collect();

        let query = vec![0.5; 128];

        // Test progressive search
        let results = pipeline
            .progressive_search(&vectors, &query, 10, DistanceMetric::Cosine)
            .await;

        assert!(results.is_ok());
        let search_results = results.unwrap();
        assert_eq!(search_results.len(), 10, "Should return top-k results");

        // Get stage statistics
        let stats = pipeline.get_stage_statistics().await;
        assert!(
            stats.binary_candidates > 0,
            "Binary stage should process candidates"
        );
        if config.enable_int8_stage {
            assert!(
                stats.int8_candidates > 0,
                "INT8 stage should process candidates"
            );
        }
    }

    #[tokio::test]
    async fn test_smart_execution_strategy() {
        init_test_environment();

        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let strategy = SmartExecutionStrategy::new(hardware.clone());

        // Test strategy selection for small dataset
        let small_context = SearchExecutionContext {
            dataset_size: 1000,
            dimension: 128,
            has_index: false,
            has_quantization: true,
            query_complexity: QueryComplexity::Simple,
            system_load: SystemLoad::Low,
        };

        let small_strategy = strategy.select_strategy(&small_context).await;
        match small_strategy {
            ExecutionStrategy::DirectFP32 { .. } => {
                // Small dataset should use direct search
            }
            _ => panic!("Small dataset should use direct FP32 search"),
        }

        // Test strategy selection for large dataset with index
        let large_context = SearchExecutionContext {
            dataset_size: 1_000_000,
            dimension: 768,
            has_index: true,
            has_quantization: true,
            query_complexity: QueryComplexity::Complex,
            system_load: SystemLoad::Medium,
        };

        let large_strategy = strategy.select_strategy(&large_context).await;
        match large_strategy {
            ExecutionStrategy::IndexFirst { .. } => {
                // Large dataset with index should use index-first
            }
            ExecutionStrategy::Progressive { .. } => {
                // Or progressive search is also valid
            }
            _ => panic!("Large dataset should use index-first or progressive search"),
        }
    }

    #[tokio::test]
    async fn test_integrated_search_optimization() {
        init_test_environment();

        // Create cache infrastructure
        let query_cache = Arc::new(QueryCache::new(100));
        let vector_store = Arc::new(VectorStore::new(1000));
        let metadata_store = Arc::new(MetadataStore::new(100));
        let cache_orchestrator = Arc::new(CrossCacheOrchestrator::new(
            query_cache.clone(),
            vector_store.clone(),
            metadata_store.clone(),
        ));

        let config = OptimizationConfig {
            enable_query_preprocessing: true,
            enable_parallel_search: true,
            enable_filter_pushdown: true,
            enable_progressive_search: true,
            enable_smart_routing: true,
            enable_zero_copy: true,
            enable_result_caching: true,
            cache_ttl_seconds: 300,
        };

        let optimizer = IntegratedSearchOptimizer::new(
            query_cache.clone(),
            vector_store.clone(),
            metadata_store.clone(),
            cache_orchestrator.clone(),
            config,
        );

        // Test end-to-end optimized search
        let vectors: Vec<Vec<f32>> = (0..5000)
            .map(|i| (0..256).map(|j| ((i + j) as f32).sin()).collect())
            .collect();

        let query = vec![0.5; 256];
        let filter = Some(FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("active"),
        });

        // First search - should miss cache
        let result1 = optimizer
            .optimized_search(&vectors, &query, 20, DistanceMetric::Cosine, filter.clone())
            .await;

        assert!(result1.is_ok());
        let search_results1 = result1.unwrap();
        assert_eq!(search_results1.results.len(), 20);
        assert!(!search_results1.cache_hit, "First search should miss cache");

        // Second search - should hit cache
        let result2 = optimizer
            .optimized_search(&vectors, &query, 20, DistanceMetric::Cosine, filter.clone())
            .await;

        assert!(result2.is_ok());
        let search_results2 = result2.unwrap();
        assert_eq!(search_results2.results.len(), 20);
        assert!(search_results2.cache_hit, "Second search should hit cache");

        // Verify results are identical
        for i in 0..20 {
            assert_eq!(
                search_results1.results[i].id, search_results2.results[i].id,
                "Cached results should be identical"
            );
        }

        // Test performance metrics
        assert!(
            search_results2.search_time_ms < search_results1.search_time_ms,
            "Cached search should be faster"
        );
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

        // Test memory-mapped operations
        let mmap_result = MemoryMappedVectors::from_file("/tmp/test_vectors.bin");
        if mmap_result.is_ok() {
            let mmap_vectors = mmap_result.unwrap();
            let vector_view = mmap_vectors.vector(0);
            assert!(
                vector_view.is_some(),
                "Should get vector view without copying"
            );
        }
    }

    #[tokio::test]
    async fn test_performance_improvements() {
        init_test_environment();

        // This test validates that optimizations achieve expected performance gains
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();

        // Baseline: Unoptimized search
        let start_baseline = std::time::Instant::now();
        let vectors: Vec<Vec<f32>> = (0..10000)
            .map(|i| (0..512).map(|j| ((i + j) as f32).sin()).collect())
            .collect();

        let query = vec![0.5; 512];
        let mut baseline_results = Vec::new();

        for vector in &vectors {
            let distance = compute_cosine_distance_baseline(&query, vector);
            baseline_results.push(distance);
        }
        baseline_results.sort_by(|a, b| a.partial_cmp(b).unwrap());
        baseline_results.truncate(100);

        let baseline_time = start_baseline.elapsed();

        // Optimized: With all optimizations
        let query_preprocessor = Arc::new(QueryPreprocessor::new(hardware.clone(), 100));
        let pipeline = UnifiedProgressiveSearchPipeline::new(
            query_preprocessor.clone(),
            PipelineConfig::default(),
        );

        let start_optimized = std::time::Instant::now();
        let optimized_results = pipeline
            .progressive_search(&vectors, &query, 100, DistanceMetric::Cosine)
            .await
            .unwrap();
        let optimized_time = start_optimized.elapsed();

        // Verify performance improvement
        let speedup = baseline_time.as_secs_f64() / optimized_time.as_secs_f64();
        println!(
            "Performance speedup: {:.2}x (baseline: {:?}, optimized: {:?})",
            speedup, baseline_time, optimized_time
        );

        assert!(
            speedup >= 2.0,
            "Should achieve at least 2x speedup, got {:.2}x",
            speedup
        );

        // Verify result quality (allowing small differences due to quantization)
        let recall = calculate_recall(&baseline_results[..10], &optimized_results[..10]);
        assert!(
            recall >= 0.9,
            "Should maintain at least 90% recall, got {:.2}",
            recall
        );
    }

    // Helper functions
    fn compute_cosine_distance_baseline(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        1.0 - (dot / (norm_a * norm_b))
    }

    fn calculate_recall(baseline: &[f32], optimized: &[SearchResult]) -> f32 {
        let baseline_set: std::collections::HashSet<_> = baseline.iter().collect();
        let optimized_set: std::collections::HashSet<_> =
            optimized.iter().map(|r| &r.distance).collect();

        let intersection = baseline_set.intersection(&optimized_set).count();
        intersection as f32 / baseline.len() as f32
    }
}
