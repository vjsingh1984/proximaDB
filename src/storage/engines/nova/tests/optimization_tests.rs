// Comprehensive tests for NOVA engine optimizations
// Tests hierarchical statistics, streaming processing, and progressive search

use anyhow::Result;
use std::sync::Arc;
use tokio;

use crate::storage::engines::nova::{
    hierarchical_stats::*,
    streaming_processor::*,
    progressive_search::*,
    zone_maps::*,
    streaming_search::*,
};
use crate::compute::distance_computation::DistanceMetric;

/// Test suite for hierarchical statistics
#[cfg(test)]
mod hierarchical_stats_tests {
    use super::*;

    #[test]
    fn test_zone_map_creation_and_intersection() {
        let vectors = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];

        let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
        
        // Test basic properties
        assert_eq!(zone_map.dimension, 3);
        assert_eq!(zone_map.min_values, vec![1.0, 2.0, 3.0]);
        assert_eq!(zone_map.max_values, vec![7.0, 8.0, 9.0]);
        assert_eq!(zone_map.centroid, vec![4.0, 5.0, 6.0]);
        
        // Test intersections
        assert!(zone_map.intersects_query(&[5.0, 5.0, 5.0], DistanceMetric::Euclidean, 10.0));
        assert!(!zone_map.intersects_query(&[20.0, 20.0, 20.0], DistanceMetric::Euclidean, 1.0));
    }

    #[test]
    fn test_superblock_creation() {
        let enhanced_stats = vec![
            create_test_enhanced_stats(0),
            create_test_enhanced_stats(1),
            create_test_enhanced_stats(2),
        ];

        let superblock = SuperBlock::new(0, 0..10, &enhanced_stats).unwrap();
        
        assert_eq!(superblock.id, 0);
        assert_eq!(superblock.row_groups, 0..10);
        assert_eq!(superblock.zone_map.dimension, 3);
        assert!(superblock.vector_count > 0);
    }

    #[test]
    fn test_superblock_candidate_detection() {
        let enhanced_stats = vec![create_test_enhanced_stats(0)];
        let superblock = SuperBlock::new(0, 0..1, &enhanced_stats).unwrap();
        
        // Query within the zone should return true
        let query = vec![2.0, 3.0, 4.0];
        assert!(superblock.can_contain_candidates(&query, DistanceMetric::Euclidean, 10.0));
        
        // Query far outside should return false
        let far_query = vec![100.0, 100.0, 100.0];
        assert!(!superblock.can_contain_candidates(&far_query, DistanceMetric::Euclidean, 1.0));
    }

    fn create_test_enhanced_stats(id: u32) -> EnhancedRowGroupStats {
        let zone_map = ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();
        
        EnhancedRowGroupStats {
            row_group_id: id,
            parquet_metadata: None,
            vector_zone_map: zone_map,
            quantized_selectivity: QuantizedSelectivity {
                binary_effectiveness: 0.8,
                int8_accuracy: 0.9,
                pq_quality: 0.85,
                progressive_efficiency: 0.75,
            },
            compression_ratio: 4.0,
            search_cost_estimate: SearchCostEstimate {
                io_cost: 10.0,
                cpu_cost: 20.0,
                memory_cost: 15.0,
                estimated_latency_ms: 50.0,
                // confidence removed -  0.8,
            },
            access_stats: AccessStats {
                access_count: 0,
                last_access: chrono::Utc::now(),
                avg_selectivity: 0.5,
                cache_hit_rate: 0.0,
                access_frequency: 0.0,
            },
        }
    }
}

/// Test suite for streaming processor
#[cfg(test)]
mod streaming_processor_tests {
    use super::*;

    #[tokio::test]
    async fn test_streaming_config_creation() {
        let config = StreamingConfig::default();
        let processor = StreamingRowGroupProcessor::new(config);
        
        // Verify default configuration
        assert_eq!(processor.config.max_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(processor.config.prefetch_queue_size, 4);
        assert_eq!(processor.config.max_concurrent_processors, 8);
    }

    #[test]
    fn test_memory_tracker() {
        let mut tracker = MemoryTracker::new(1000);
        
        // Test memory reservation
        assert!(tracker.reserve_memory("test1", 400).is_ok());
        assert_eq!(tracker.current_usage, 400);
        
        // Test memory limit enforcement
        assert!(tracker.reserve_memory("test2", 700).is_err());
        
        // Test memory release
        tracker.release_memory("test1");
        assert_eq!(tracker.current_usage, 0);
        
        // Test pressure detection
        assert!(tracker.reserve_memory("test3", 900).is_ok());
        assert!(tracker.is_under_pressure(0.8)); // 90% > 80%
    }

    #[test]
    fn test_processing_stages() {
        let stages = vec![
            ProcessingStage::BloomFilter,
            ProcessingStage::ZoneMapPruning,
            ProcessingStage::BinaryFilter,
            ProcessingStage::Int8Filter,
            ProcessingStage::PQFilter,
            ProcessingStage::FullPrecision,
        ];
        
        assert_eq!(stages.len(), 6);
        assert_eq!(stages[0], ProcessingStage::BloomFilter);
        assert_eq!(stages[5], ProcessingStage::FullPrecision);
    }
}

/// Test suite for progressive search
#[cfg(test)]
mod progressive_search_tests {
    use super::*;

    #[test]
    fn test_progressive_search_config() {
        let config = ProgressiveSearchConfig::default();
        
        assert!(config.enable_superblock_pruning);
        assert!(config.cost_based_ordering);
        assert!(config.adaptive_thresholds);
        assert_eq!(config.quality_target, 0.8);
    }

    #[test]
    fn test_stage_config() {
        let config = ProgressiveSearchConfig::default();
        
        // Verify binary stage config
        assert_eq!(config.binary_config.max_candidates, 10000);
        assert_eq!(config.binary_config.distance_threshold, Some(100.0));
        
        // Verify INT8 stage config
        assert_eq!(config.int8_config.max_candidates, 1000);
        assert_eq!(config.int8_config.distance_threshold, Some(50.0));
        
        // Verify PQ stage config
        assert_eq!(config.pq_config.max_candidates, 200);
        assert_eq!(config.pq_config.distance_threshold, Some(20.0));
        
        // Verify full precision stage config
        assert_eq!(config.full_precision_config.max_candidates, 50);
        assert_eq!(config.full_precision_config.distance_threshold, None);
    }

    #[test]
    fn test_progressive_candidate_ordering() {
        use std::collections::BinaryHeap;
        
        let mut heap = BinaryHeap::new();
        
        heap.push(ProgressiveCandidate {
            row_group_id: 0,
            row_offset: 0,
            similarity: 10.0,
            // confidence removed -  0.8,
            stage: ProcessingStage::BinaryFilter,
            vector_id: None,
            record: None,
        });
        
        heap.push(ProgressiveCandidate {
            row_group_id: 0,
            row_offset: 1,
            similarity: 5.0,
            // confidence removed -  0.8,
            stage: ProcessingStage::BinaryFilter,
            vector_id: None,
            record: None,
        });
        
        // Min-heap: smallest distance first
        assert_eq!(heap.pop().unwrap().distance, 5.0);
        assert_eq!(heap.pop().unwrap().distance, 10.0);
    }

    #[test]
    fn test_binary_sketch_operations() {
        let vector = vec![0.5, -0.3, 0.8, -0.1, 0.0];
        let sketch = BinarySketch::from_vector(&vector, 0.0);
        
        assert_eq!(sketch.dimension, 5);
        
        // Test hamming distance
        let other_vector = vec![0.7, -0.1, 0.9, -0.2, 0.1];
        let other_sketch = BinarySketch::from_vector(&other_vector, 0.0);
        
        let distance = sketch.hamming_distance(&other_sketch);
        assert!(distance >= 0);
    }

    #[test]
    fn test_int8_vector_operations() {
        let vector = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let int8_vec = Int8Vector::from_vector(&vector);
        
        assert_eq!(int8_vec.values.len(), 5);
        assert!(int8_vec.scale > 0.0);
        
        // Test distance computation
        let other_vector = vec![1.5, 2.5, 3.5, 4.5, 5.5];
        let other_int8_vec = Int8Vector::from_vector(&other_vector);
        
        let distance = int8_vec.l2_distance_squared(&other_int8_vec);
        assert!(distance >= 0.0);
    }
}

/// Test suite for zone maps
#[cfg(test)]
mod zone_maps_tests {
    use super::*;

    #[test]
    fn test_zone_map_config() {
        let config = ZoneMapConfig::default();
        
        assert!(config.enable_hierarchical);
        assert_eq!(config.hierarchical_levels, 3);
        assert_eq!(config.sketch_width, 1024);
        assert_eq!(config.sketch_depth, 4);
        assert_eq!(config.hll_precision, 12);
        assert_eq!(config.bloom_false_positive_rate, 0.01);
    }

    #[test]
    fn test_query_characteristics() {
        let query = vec![1.0, 0.0, 2.0, 0.0, 3.0];
        let characteristics = QueryCharacteristics::from_query(&query, DistanceMetric::Euclidean, 10);
        
        assert_eq!(characteristics.top_k, 10);
        assert_eq!(characteristics.sparsity, 0.4); // 2/5 zeros
        assert!(characteristics.norm > 0.0);
        assert_eq!(characteristics.dominant_dimensions.len(), 1); // top 10% of 5 = 1
    }

    #[test]
    fn test_selectivity_model() {
        let model = SelectivityModel {
            parameters: vec![0.1, -0.2, 0.5], // norm, sparsity, intercept
            model_type: ModelType::Linear,
            accuracy: 0.8,
            training_samples: 100,
        };
        
        let characteristics = QueryCharacteristics {
            norm: 2.0,
            sparsity: 0.3,
            dominant_dimensions: vec![0, 1, 2],
            distance_metric: DistanceMetric::Euclidean,
            top_k: 10,
        };
        
        let selectivity = model.predict(&characteristics);
        // Expected: 0.1 * 2.0 + (-0.2) * 0.3 + 0.5 = 0.64
        assert!((selectivity - 0.64).abs() < 0.01);
    }

    #[test]
    fn test_advanced_intersection_result() {
        let mut result = AdvancedIntersectionResult::default();
        
        result.intersects = true;
        result.confidence = 0.95;
        result.estimated_selectivity = Some(0.3);
        result.estimated_cost_savings = 100.0;
        
        assert!(result.intersects);
        assert_eq!(result.confidence, 0.95);
        assert_eq!(result.estimated_selectivity, Some(0.3));
    }
}

/// Test suite for streaming search
#[cfg(test)]
mod streaming_search_tests {
    use super::*;

    #[test]
    fn test_streaming_search_config() {
        let config = StreamingSearchConfig::default();
        
        assert!(config.enable_cost_based_ordering);
        assert!(config.enable_adaptive_thresholds);
        assert!(config.enable_query_caching);
        assert_eq!(config.target_latency_ms, Some(1000));
        assert_eq!(config.target_throughput_qps, Some(100.0));
        assert_eq!(config.max_memory_usage_bytes, 512 * 1024 * 1024);
        assert_eq!(config.min_recall_threshold, 0.95);
        assert_eq!(config.precision_target, 0.9);
    }

    #[test]
    fn test_execution_plan() {
        let plan = ExecutionPlan::new();
        
        assert_eq!(plan.parallelism_level, 4);
        assert_eq!(plan.memory_budget_per_stage, 64 * 1024 * 1024);
        assert!(plan.selected_superblocks.is_empty());
        assert!(plan.row_group_order.is_empty());
    }

    #[test]
    fn test_performance_tracker() {
        let mut tracker = PerformanceTracker::new();
        
        let characteristics = QueryCharacteristics {
            dimension: 768,
            top_k: 10,
            distance_metric: DistanceMetric::Euclidean,
            query_norm: 1.0,
            query_sparsity: 0.1,
            estimated_selectivity: 0.5,
        };
        
        let performance = ActualPerformance {
            latency_ms: 500,
            memory_peak: 64 * 1024 * 1024,
            candidates_processed: 1000,
            pruning_effectiveness: 0.8,
            recall: Some(0.95),
            precision: Some(0.9),
        };
        
        tracker.record_query_execution("test_query", &characteristics, performance);
        
        assert_eq!(tracker.query_history.len(), 1);
        assert!(tracker.workload_stats.avg_query_selectivity > 0.0);
    }

    #[test]
    fn test_selectivity_estimation() {
        let tracker = PerformanceTracker::new();
        
        // Test sparse query
        let sparse_characteristics = QueryCharacteristics {
            dimension: 768,
            top_k: 10,
            distance_metric: DistanceMetric::Euclidean,
            query_norm: 1.0,
            query_sparsity: 0.9, // Very sparse
            estimated_selectivity: 0.5, // Will be updated
        };
        
        let selectivity = tracker.estimate_selectivity(&sparse_characteristics);
        assert_eq!(selectivity, 0.1); // Should be highly selective
        
        // Test dense query
        let dense_characteristics = QueryCharacteristics {
            dimension: 768,
            top_k: 10,
            distance_metric: DistanceMetric::Euclidean,
            query_norm: 1.0,
            query_sparsity: 0.1, // Dense
            estimated_selectivity: 0.5, // Will be updated
        };
        
        let selectivity = tracker.estimate_selectivity(&dense_characteristics);
        assert_eq!(selectivity, 0.7); // Should be less selective
    }
}

/// Integration tests combining multiple optimization techniques
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_streaming_search_engine_creation() {
        let config = StreamingSearchConfig::default();
        let engine = StreamingSearchEngine::new(config, DistanceMetric::Euclidean);
        
        // Verify engine was created successfully
        // (Most fields are private, so we can't inspect them directly)
        assert!(true); // If we get here, creation succeeded
    }

    #[test]
    fn test_end_to_end_optimization_pipeline() {
        // Test the complete optimization pipeline components
        
        // 1. Create test data
        let vectors = create_test_vectors(1000, 768);
        let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
        
        // 2. Create enhanced statistics
        let enhanced_stats = create_test_enhanced_stats_vec(10);
        
        // 3. Create SuperBlocks
        let superblock = SuperBlock::new(0, 0..10, &enhanced_stats).unwrap();
        
        // 4. Verify optimization components work together
        assert_eq!(zone_map.dimension, 768);
        assert_eq!(superblock.zone_map.dimension, 3); // From test data
        assert_eq!(enhanced_stats.len(), 10);
        
        // 5. Test query characteristics analysis
        let query = create_test_query(768);
        let characteristics = QueryCharacteristics::from_query(&query, DistanceMetric::Euclidean, 10);
        
        assert_eq!(characteristics.dimension, 768);
        assert_eq!(characteristics.top_k, 10);
        assert!(characteristics.norm > 0.0);
    }

    #[test]
    fn test_memory_efficiency_optimization() {
        // Test memory efficiency across different configurations
        
        let small_config = StreamingConfig {
            max_memory_bytes: 64 * 1024 * 1024, // 64MB
            prefetch_queue_size: 2,
            max_concurrent_processors: 2,
            ..StreamingConfig::default()
        };
        
        let large_config = StreamingConfig {
            max_memory_bytes: 1024 * 1024 * 1024, // 1GB
            prefetch_queue_size: 8,
            max_concurrent_processors: 16,
            ..StreamingConfig::default()
        };
        
        // Verify configurations are different
        assert!(small_config.max_memory_bytes < large_config.max_memory_bytes);
        assert!(small_config.prefetch_queue_size < large_config.prefetch_queue_size);
        assert!(small_config.max_concurrent_processors < large_config.max_concurrent_processors);
    }

    #[test]
    fn test_progressive_search_effectiveness() {
        // Test that progressive search reduces candidates at each stage
        
        let config = ProgressiveSearchConfig::default();
        
        // Verify stage progression reduces candidates
        assert!(config.binary_config.max_candidates > config.int8_config.max_candidates);
        assert!(config.int8_config.max_candidates > config.pq_config.max_candidates);
        assert!(config.pq_config.max_candidates > config.full_precision_config.max_candidates);
        
        // Verify distance thresholds are increasingly strict
        let binary_threshold = config.binary_config.distance_threshold.unwrap_or(f32::INFINITY);
        let int8_threshold = config.int8_config.distance_threshold.unwrap_or(f32::INFINITY);
        let pq_threshold = config.pq_config.distance_threshold.unwrap_or(f32::INFINITY);
        
        assert!(binary_threshold > int8_threshold);
        assert!(int8_threshold > pq_threshold);
    }

    // Helper functions for test data creation
    
    fn create_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|i| {
                (0..dimension)
                    .map(|j| (i as f32 + j as f32) / (dimension as f32))
                    .collect()
            })
            .collect()
    }
    
    fn create_test_query(dimension: usize) -> Vec<f32> {
        (0..dimension).map(|i| i as f32 / dimension as f32).collect()
    }
    
    fn create_test_enhanced_stats_vec(count: usize) -> Vec<EnhancedRowGroupStats> {
        (0..count)
            .map(|i| {
                let zone_map = ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).unwrap();
                
                EnhancedRowGroupStats {
                    row_group_id: i as u32,
                    parquet_metadata: None,
                    vector_zone_map: zone_map,
                    quantized_selectivity: QuantizedSelectivity {
                        binary_effectiveness: 0.8,
                        int8_accuracy: 0.9,
                        pq_quality: 0.85,
                        progressive_efficiency: 0.75,
                    },
                    compression_ratio: 4.0,
                    search_cost_estimate: SearchCostEstimate {
                        io_cost: 10.0 + i as f32,
                        cpu_cost: 20.0 + i as f32,
                        memory_cost: 15.0 + i as f32,
                        estimated_latency_ms: 50.0 + i as f32,
                        // confidence removed -  0.8,
                    },
                    access_stats: AccessStats {
                        access_count: i as u64,
                        last_access: chrono::Utc::now(),
                        avg_selectivity: 0.5,
                        cache_hit_rate: 0.0,
                        access_frequency: 0.0,
                    },
                }
            })
            .collect()
    }
}

/// Benchmark tests for performance validation
#[cfg(test)]
mod benchmark_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_zone_map_performance() {
        let vectors = create_large_test_dataset(10000, 768);
        
        let start = Instant::now();
        let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
        let creation_time = start.elapsed();
        
        // Verify reasonable creation time (should be under 1 second)
        assert!(creation_time.as_millis() < 1000);
        
        // Test intersection performance
        let query = vec![0.5; 768];
        let start = Instant::now();
        
        for _ in 0..1000 {
            let _intersects = zone_map.intersects_query(
                &query,
                DistanceMetric::Euclidean,
                10.0,
            );
        }
        
        let intersection_time = start.elapsed();
        
        // Verify reasonable intersection time (should be under 100ms for 1000 queries)
        assert!(intersection_time.as_millis() < 100);
    }

    #[test]
    fn test_binary_sketch_performance() {
        let vectors = create_large_test_dataset(1000, 768);
        
        let start = Instant::now();
        let sketches: Vec<BinarySketch> = vectors.iter()
            .map(|v| BinarySketch::from_vector(v, 0.0))
            .collect();
        let creation_time = start.elapsed();
        
        // Verify reasonable creation time
        assert!(creation_time.as_millis() < 500);
        
        // Test distance computation performance
        let query_sketch = BinarySketch::from_vector(&vectors[0], 0.0);
        let start = Instant::now();
        
        let distances: Vec<u32> = sketches.iter()
            .map(|sketch| query_sketch.hamming_distance(sketch))
            .collect();
        let distance_time = start.elapsed();
        
        // Verify reasonable distance computation time
        assert!(distance_time.as_millis() < 100);
        assert_eq!(distances.len(), 1000);
    }

    #[test]
    fn test_memory_usage_patterns() {
        // Test memory usage for different data sizes
        
        let small_vectors = create_large_test_dataset(100, 128);
        let medium_vectors = create_large_test_dataset(1000, 256);
        let large_vectors = create_large_test_dataset(5000, 512);
        
        // Create zone maps and verify they complete successfully
        let small_zone = ZoneMap::from_vectors(&small_vectors).unwrap();
        let medium_zone = ZoneMap::from_vectors(&medium_vectors).unwrap();
        let large_zone = ZoneMap::from_vectors(&large_vectors).unwrap();
        
        // Verify dimensions are correct
        assert_eq!(small_zone.dimension, 128);
        assert_eq!(medium_zone.dimension, 256);
        assert_eq!(large_zone.dimension, 512);
        
        // Verify zone maps have reasonable bounds
        assert!(small_zone.min_values.len() == 128);
        assert!(medium_zone.min_values.len() == 256);
        assert!(large_zone.min_values.len() == 512);
    }

    fn create_large_test_dataset(count: usize, dimension: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|i| {
                (0..dimension)
                    .map(|j| {
                        // Create more realistic test data with some randomness
                        let base = (i as f32 + j as f32) / (dimension as f32);
                        let noise = ((i * 7 + j * 11) % 100) as f32 / 1000.0; // Simple pseudo-random
                        base + noise
                    })
                    .collect()
            })
            .collect()
    }
}