//! Performance benchmarks for HELIX engine
//!
//! This module contains benchmarks to measure:
//! - PCA training and projection performance
//! - Hilbert key computation speed
//! - Clustering effectiveness
//! - Query pruning ratios
//! - Compaction throughput

#[cfg(test)]
mod benchmarks {
    use super::super::*;
    use crate::compute::distance_computation::DistanceMetric;
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::traits::{FlushParameters, StorageQueryContext, StorageQueryMetadata};
    use crate::core::search::SearchParams;
    use crate::proto::proximadb_v1::{Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric, StorageEngine};
    use criterion::{black_box, Criterion};
    use rand::{Rng, SeedableRng};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    /// Generate random vector records for benchmarking
    fn generate_random_vectors(count: usize, dims: usize, seed: u64) -> Vec<VectorRecord> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        
        (0..count)
            .map(|i| {
                let vector: Vec<f32> = (0..dims)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect();
                
                VectorRecord {
                    id: format!("vec_{}", i),
                    vector,
                    metadata: Some(HashMap::from([
                        ("type".to_string(), "benchmark".to_string()),
                        ("cluster".to_string(), (i % 10).to_string()),
                    ])),
                    timestamp: i as i64,
                    expires_at: None,
                }
            })
            .collect()
    }

    /// Generate clustered vectors (for testing clustering effectiveness)
    fn generate_clustered_vectors(
        num_clusters: usize,
        vectors_per_cluster: usize,
        dims: usize,
    ) -> Vec<VectorRecord> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut all_vectors = Vec::new();
        
        for cluster_id in 0..num_clusters {
            // Generate cluster center
            let center: Vec<f32> = (0..dims)
                .map(|_| rng.gen_range(-10.0..10.0))
                .collect();
            
            // Generate vectors around center
            for i in 0..vectors_per_cluster {
                let mut vector = center.clone();
                for v in &mut vector {
                    *v += rng.gen_range(-0.5..0.5); // Small noise around center
                }
                
                all_vectors.push(VectorRecord {
                    id: format!("cluster_{}_vec_{}", cluster_id, i),
                    vector,
                    metadata: Some(HashMap::from([
                        ("cluster_id".to_string(), cluster_id.to_string()),
                    ])),
                    timestamp: (cluster_id * vectors_per_cluster + i) as i64,
                    expires_at: None,
                });
            }
        }
        
        all_vectors
    }

    #[bench]
    fn bench_pca_training(b: &mut Bencher) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let vectors = generate_random_vectors(1000, 768, 42);
        
        b.iter(|| {
            let model = clustering::PCAModel::train(&vectors, 16).unwrap();
            black_box(model);
        });
    }

    #[bench]
    fn bench_pca_projection(b: &mut Bencher) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let vectors = generate_random_vectors(100, 768, 42);
        let model = clustering::PCAModel::train(&vectors, 16).unwrap();
        let test_vector = vec![0.5; 768];
        
        b.iter(|| {
            let projected = model.project(&test_vector).unwrap();
            black_box(projected);
        });
    }

    #[bench]
    fn bench_hilbert_key_computation(b: &mut Bencher) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let vectors = vec![
            vec![0.1, 0.2, 0.3],
            vec![0.4, 0.5, 0.6],
            vec![0.7, 0.8, 0.9],
        ];
        
        b.iter(|| {
            for v in &vectors {
                let key = clustering::compute_hilbert_key(v);
                black_box(key);
            }
        });
    }

    #[tokio::test]
    async fn bench_flush_performance() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();
        
        let engine = HelixEngine::new(
            "bench_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Test different batch sizes
        for size in [100, 500, 1000, 5000] {
            let vectors = generate_random_vectors(size, 384, 42);
            
            let start = Instant::now();
            let params = FlushParameters {
                collection_id: Some("bench_collection".to_string()),
                records: vectors,
                collection_config: None,
                level: None,
            };
            
            let result = engine.do_flush(&params).await.unwrap();
            let elapsed = start.elapsed();
            
            println!(
                "Flush {} vectors: {:.2}ms, {:.2} MB/s",
                size,
                elapsed.as_millis(),
                (result.bytes_written as f64 / 1_048_576.0) / elapsed.as_secs_f64()
            );
        }
    }

    #[tokio::test]
    async fn bench_search_with_pruning() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let mut config = HelixConfig::default();
        config.proxima_block_size = 50;
        
        let engine = HelixEngine::new(
            "bench_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Create clustered data
        let vectors = generate_clustered_vectors(10, 100, 128);
        
        // Flush in multiple batches to create multiple files
        for chunk in vectors.chunks(200) {
            let params = FlushParameters {
                collection_id: Some("bench_collection".to_string()),
                records: chunk.to_vec(),
                collection_config: None,
                level: None,
            };
            engine.do_flush(&params).await.unwrap();
        }
        
        // Search and measure pruning
        let query_vector = vectors[50].vector.clone(); // Vector from cluster 0
        
        let start = Instant::now();
        
        let collection_config = CollectionConfig {
            name: "bench_collection".to_string(),
            dimension: 128,
            distance_metric: ProtoDistanceMetric::Euclidean as i32,
            storage_engine: StorageEngine::Helix as i32,
            ..Default::default()
        };
        
        let collection = Arc::new(Collection {
            id: "bench_collection".to_string(),
            config: Some(collection_config),
            ..Default::default()
        });
        
        let mut search_params = SearchParams::single_vector(query_vector);
        search_params.top_k = Some(10);
        search_params.distance_metric = Some(DistanceMetric::Euclidean);
        
        let metadata = StorageQueryMetadata::default();
        
        let ctx = StorageQueryContext {
            search_params: Arc::new(search_params),
            collection,
            metadata,
        };
        
        let results = engine.search_vectors_unified(&ctx).await.unwrap();
        let elapsed = start.elapsed();
        
        println!(
            "Search latency: {:.2}ms, found {} results",
            elapsed.as_millis(),
            results.len()
        );
        
        // Verify results are from the same cluster
        let same_cluster = results.iter()
            .filter(|r| r.id.starts_with("cluster_0_"))
            .count();
        
        println!(
            "Clustering accuracy: {:.1}% from same cluster",
            (same_cluster as f64 / results.len() as f64) * 100.0
        );
    }

    #[tokio::test]
    async fn bench_compaction_throughput() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let temp_dir = TempDir::new().unwrap();
        let mut config = HelixConfig::default();
        config.level0_file_num_compaction_trigger = 3;
        
        let engine = HelixEngine::new(
            "bench_collection".to_string(),
            config,
            temp_dir.path().to_path_buf(),
            None,
        ).await.unwrap();
        
        // Create multiple L0 files
        let vectors_per_file = 500;
        for i in 0..4 {
            let vectors = generate_random_vectors(vectors_per_file, 256, i);
            let params = FlushParameters {
                collection_id: Some("bench_collection".to_string()),
                records: vectors,
                collection_config: None,
                level: None,
            };
            engine.do_flush(&params).await.unwrap();
        }
        
        // Trigger compaction
        let start = Instant::now();
        let compact_params = CompactionParameters {
            collection_id: Some("bench_collection".to_string()),
            level: Some(0),
            collection_config: None,
        };
        
        let result = engine.do_compact(&compact_params).await.unwrap();
        let elapsed = start.elapsed();
        
        println!(
            "Compaction: {} files in {:.2}ms, {:.2} MB/s",
            result.files_compacted,
            elapsed.as_millis(),
            (result.bytes_written as f64 / 1_048_576.0) / elapsed.as_secs_f64()
        );
    }

    #[test]
    fn bench_hilbert_ordering_quality() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Generate clustered data
        let vectors = generate_clustered_vectors(5, 20, 64);
        
        // Train PCA model
        let model = clustering::PCAModel::train(&vectors, 8).unwrap();
        
        // Compute Hilbert keys
        let mut keys_and_ids: Vec<(u64, String)> = vectors
            .iter()
            .map(|v| {
                let key = model.project_and_compute_hilbert(&v.vector).unwrap();
                (key, v.id.clone())
            })
            .collect();
        
        // Sort by Hilbert key
        keys_and_ids.sort_by_key(|&(key, _)| key);
        
        // Measure clustering quality: consecutive vectors should be from same cluster
        let mut same_cluster_consecutive = 0;
        let mut total_consecutive = 0;
        
        for window in keys_and_ids.windows(2) {
            let id1 = &window[0].1;
            let id2 = &window[1].1;
            
            // Extract cluster ID from vector ID
            let cluster1 = id1.split('_').nth(1).unwrap();
            let cluster2 = id2.split('_').nth(1).unwrap();
            
            if cluster1 == cluster2 {
                same_cluster_consecutive += 1;
            }
            total_consecutive += 1;
        }
        
        let quality = same_cluster_consecutive as f64 / total_consecutive as f64;
        println!(
            "Hilbert ordering quality: {:.1}% consecutive vectors from same cluster",
            quality * 100.0
        );
        
        assert!(quality > 0.7, "Hilbert ordering should preserve locality");
    }

    #[tokio::test]
    async fn bench_liquid_clustering_effectiveness() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut tracker = clustering::QueryPatternTracker::default();
        
        // Simulate query patterns
        let hot_vectors = ["vec_1", "vec_2", "vec_3"];
        let cold_vectors = ["vec_100", "vec_101", "vec_102"];
        
        // Hot vectors accessed frequently
        for _ in 0..100 {
            for id in &hot_vectors {
                tracker.record_access(id, rand::random());
            }
        }
        
        // Cold vectors accessed rarely
        for id in &cold_vectors {
            tracker.record_access(id, rand::random());
        }
        
        // Get clustering hints
        let all_ids: Vec<String> = hot_vectors.iter()
            .chain(cold_vectors.iter())
            .map(|s| s.to_string())
            .collect();
        
        let config = clustering::LiquidClusteringConfig::default();
        let hints = tracker.get_clustering_hints(&all_ids, &config);
        
        // Verify hot vectors have higher scores
        let hot_avg_score: f32 = hot_vectors.iter()
            .map(|id| hints.get(*id).copied().unwrap_or(0.0))
            .sum::<f32>() / hot_vectors.len() as f32;
        
        let cold_avg_score: f32 = cold_vectors.iter()
            .map(|id| hints.get(*id).copied().unwrap_or(0.0))
            .sum::<f32>() / cold_vectors.len() as f32;
        
        println!(
            "Liquid clustering: hot vectors score {:.3}, cold vectors score {:.3}",
            hot_avg_score, cold_avg_score
        );
        
        assert!(hot_avg_score > cold_avg_score * 10.0, "Hot vectors should have much higher scores");
    }

    #[test]
    fn bench_memory_usage() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Measure memory overhead of metadata structures
        let num_blocks = 1000;
        let num_vectors_per_block = 100;
        
        // Create metadata
        let metadata: Vec<HelixBlockMetadata> = (0..num_blocks)
            .map(|i| HelixBlockMetadata {
                proxima_metadata: ProximaBlockMetadata {
                    block_id: i as u32,
                    block_size: num_vectors_per_block,
                    uncompressed_size: num_vectors_per_block * 1536, // 384 dims * 4 bytes
                    compressed_size: num_vectors_per_block * 768,
                    checksum: 0,
                    compression_algorithm: CompressionAlgorithm::Zstd,
                    encoding_marker: 0x10,
                    min_timestamp: 0,
                    max_timestamp: 1000,
                    hilbert_min: Some(i as i64 * 1000),
                    hilbert_max: Some((i + 1) as i64 * 1000),
                    metadata_stats: BlockMetadataStats {
                        unique_keys: num_vectors_per_block,
                        null_values: 0,
                        avg_value_size: 1536.0,
                        compression_ratio: 0.5,
                    },
                },
                hilbert_range: Some((i as u64 * 1000, (i + 1) as u64 * 1000)),
                pca_stats: None,
                clustering_hints: None,
            })
            .collect();
        
        let metadata_size = std::mem::size_of_val(&metadata[..]);
        let per_block_overhead = metadata_size / num_blocks;
        let per_vector_overhead = per_block_overhead / num_vectors_per_block;
        
        println!(
            "Memory overhead: {} bytes per block, {} bytes per vector",
            per_block_overhead, per_vector_overhead
        );
        
        // Should be minimal overhead
        assert!(per_vector_overhead < 10, "Metadata overhead should be minimal");
    }
}

/// Criterion benchmarks for more detailed performance analysis
#[cfg(all(test, feature = "bench"))]
mod criterion_benchmarks {
    use super::benchmarks::*;
    use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};

    fn pca_benchmark(c: &mut Criterion) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut group = c.benchmark_group("pca");
        
        for dims in [128, 256, 512, 768, 1024] {
            let vectors = generate_random_vectors(1000, dims, 42);
            
            group.throughput(Throughput::Elements(1000));
            group.bench_with_input(
                BenchmarkId::new("train", dims),
                &vectors,
                |b, vectors| {
                    b.iter(|| {
                        clustering::PCAModel::train(vectors, 16).unwrap()
                    });
                },
            );
        }
        
        group.finish();
    }

    fn hilbert_benchmark(c: &mut Criterion) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut group = c.benchmark_group("hilbert");
        
        for dims in [2, 3, 8, 16, 32] {
            let vector = vec![0.5; dims];
            
            group.bench_with_input(
                BenchmarkId::new("compute_key", dims),
                &vector,
                |b, vector| {
                    b.iter(|| {
                        clustering::compute_hilbert_key(vector)
                    });
                },
            );
        }
        
        group.finish();
    }

    fn search_benchmark(c: &mut Criterion) {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        let mut group = c.benchmark_group("search");
        
        // Setup would be done here for search benchmarks
        // This is a placeholder for the structure
        
        group.finish();
    }

    criterion_group!(benches, pca_benchmark, hilbert_benchmark, search_benchmark);
    criterion_main!(benches);
}