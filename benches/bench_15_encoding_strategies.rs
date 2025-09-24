use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use proximadb::storage::engines::core::formats::fastlanes_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, FastLanesDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use rand::prelude::*;
use std::time::Instant;

/// Helper function to create and serialize a FastLanesDataBlock
fn create_and_serialize_block(
    vectors: &[VectorRecord],
    config: &BlockCompressionConfig
) -> anyhow::Result<Vec<u8>> {
    let block = FastLanesDataBlock::new(vectors.to_vec(), config.clone());
    block.serialize()
}

/// Generate test vectors for benchmarking
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|i| VectorRecord {
            id: i.to_string(),
            vector: (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect(),
            metadata: std::collections::HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 0,
            updated_at: None,
            version: None,
        })
        .collect()
}

/// WORM (Write Once, Read Many) optimized configuration
fn create_worm_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::TransposeVector,
        algorithm: CompressionAlgorithm::Zstd,
        compression_level: 6,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 4096,
        dictionary_compression: true,
    }
}

/// Real-time optimized configuration
fn create_realtime_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: true,
        compression_threshold_bytes: 16384,
        dictionary_compression: false,
    }
}

/// Balanced workload configuration
fn create_balanced_config() -> BlockCompressionConfig {
    BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::Auto,
        algorithm: CompressionAlgorithm::Snappy,
        compression_level: 3,
        enable_vector_compression: true,
        enable_metadata_compression: true,
        compression_threshold_bytes: 8192,
        dictionary_compression: false,
    }
}

/// Benchmark encoding strategies across different dimensions
fn bench_encoding_strategies(c: &mut Criterion) {
    let dimensions = vec![384, 768, 1536, 3072];
    let vector_count = 1000;

    let mut group = c.benchmark_group("encoding_strategies");
    group.sample_size(10); // Reduce sample size for heavy operations

    for dimension in dimensions {
        let vectors = generate_test_vectors(vector_count, dimension);

        // Columnar encoding benchmark
        group.bench_with_input(
            BenchmarkId::new("columnar", dimension),
            &vectors,
            |b, vectors| {
                let config = BlockCompressionConfig {
                    vector_layout: VectorEncodingLayout::TransposeVector,
                    algorithm: CompressionAlgorithm::Lz4,
                    compression_level: 1,
                    enable_vector_compression: true,
                    enable_metadata_compression: false,
                    compression_threshold_bytes: 0,
                    dictionary_compression: false,
                };

                b.iter(|| {
                    let result = create_and_serialize_block(
                        black_box(vectors),
                        black_box(&config)
                    );
                    black_box(result)
                });
            },
        );

        // Row-wise encoding benchmark
        group.bench_with_input(
            BenchmarkId::new("rowwise", dimension),
            &vectors,
            |b, vectors| {
                let config = BlockCompressionConfig {
                    vector_layout: VectorEncodingLayout::FullVector,
                    algorithm: CompressionAlgorithm::Lz4,
                    compression_level: 1,
                    enable_vector_compression: false,
                    enable_metadata_compression: false,
                    compression_threshold_bytes: 0,
                    dictionary_compression: false,
                };

                b.iter(|| {
                    let result = create_and_serialize_block(
                        black_box(vectors),
                        black_box(&config)
                    );
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark WORM workload simulation
fn bench_worm_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("worm_workload");

    let config = create_worm_config();
    let vectors = generate_test_vectors(1000, 768);

    // Simulate WORM: 1 write, multiple reads
    group.bench_function("columnar_worm", |b| {
        b.iter(|| {
            // Encode once
            let encoded = create_and_serialize_block(&vectors, &config).unwrap();

            // Simulate multiple reads
            for _ in 0..100 {
                let _decoded = FastLanesDataBlock::deserialize(black_box(&encoded));
                black_box(&_decoded);
            }
        });
    });

    group.finish();
}

/// Benchmark real-time workload simulation
fn bench_realtime_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("realtime_workload");

    let config = create_realtime_config();

    // Simulate real-time: frequent small batches
    group.bench_function("rowwise_realtime", |b| {
        b.iter(|| {
            // Small batches for low latency
            let vectors = generate_test_vectors(10, 384);
            let encoded = create_and_serialize_block(&vectors, &config).unwrap();
            let _decoded = FastLanesDataBlock::deserialize(black_box(&encoded));
            black_box(&_decoded);
        });
    });

    group.finish();
}

/// Benchmark balanced workload simulation
fn bench_balanced_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("balanced_workload");

    let config = create_balanced_config();

    // Test auto-selection logic
    let dimensions = vec![256, 512, 1024]; // Around the auto-selection threshold

    for dimension in dimensions {
        let vectors = generate_test_vectors(500, dimension);

        group.bench_with_input(
            BenchmarkId::new("auto_selection", dimension),
            &vectors,
            |b, vectors| {
                b.iter(|| {
                    let encoded = create_and_serialize_block(vectors, &config).unwrap();
                    let _decoded = FastLanesDataBlock::deserialize(black_box(&encoded));
                    black_box(&_decoded);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark compression ratio vs encoding time trade-offs
fn bench_compression_tradeoffs(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_tradeoffs");
    group.measurement_time(std::time::Duration::from_secs(10));

    let vectors = generate_test_vectors(1000, 768);
    let algorithms = vec![
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Snappy,
        CompressionAlgorithm::Gzip,
        CompressionAlgorithm::Zstd,
    ];

    for algorithm in algorithms {
        let config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::TransposeVector,
            algorithm: algorithm.clone(),
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 4096,
            dictionary_compression: false,
        };

        group.bench_with_input(
            BenchmarkId::new("algorithm", format!("{:?}", algorithm)),
            &config,
            |b, config| {
                b.iter(|| {
                    let result = create_and_serialize_block(&vectors, config);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Custom benchmark to measure actual compression ratios
fn bench_compression_ratios(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_ratios");

    let dimensions = vec![384, 768, 1536];
    let vector_count = 1000;

    for dimension in dimensions {
        let vectors = generate_test_vectors(vector_count, dimension);

        // Calculate uncompressed size
        let uncompressed_size = vectors.len() * dimension * 4; // f32 = 4 bytes

        let columnar_config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::TransposeVector,
            algorithm: CompressionAlgorithm::Zstd,
            compression_level: 6,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 0,
            dictionary_compression: true,
        };

        let rowwise_config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::FullVector,
            algorithm: CompressionAlgorithm::Lz4,
            compression_level: 1,
            enable_vector_compression: false,
            enable_metadata_compression: false,
            compression_threshold_bytes: 0,
            dictionary_compression: false,
        };

        // Benchmark with compression ratio measurement
        group.bench_function(&format!("columnar_{}d", dimension), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                for _ in 0..iters {
                    let compressed = create_and_serialize_block(&vectors, &columnar_config).unwrap();
                    let compression_ratio = uncompressed_size as f64 / compressed.len() as f64;

                    // Print compression ratio (will be captured by criterion)
                    if iters == 1 {
                        println!("{}D Columnar: {:.2}x compression", dimension, compression_ratio);
                    }

                    black_box(compressed);
                }
                start.elapsed()
            });
        });

        group.bench_function(&format!("rowwise_{}d", dimension), |b| {
            b.iter_custom(|iters| {
                let start = Instant::now();
                for _ in 0..iters {
                    let compressed = create_and_serialize_block(&vectors, &rowwise_config).unwrap();
                    let compression_ratio = uncompressed_size as f64 / compressed.len() as f64;

                    if iters == 1 {
                        println!("{}D Row-wise: {:.2}x compression", dimension, compression_ratio);
                    }

                    black_box(compressed);
                }
                start.elapsed()
            });
        });

        // Benchmark GroupedVector strategy for dimensions > 128
        if dimension > 128 {
            let grouped_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::GroupedVector,
                algorithm: CompressionAlgorithm::Lz4,
                compression_level: 1,
                enable_vector_compression: true,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };

            group.bench_function(&format!("grouped_{}d", dimension), |b| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let compressed = create_and_serialize_block(&vectors, &grouped_config).unwrap();
                        let compression_ratio = uncompressed_size as f64 / compressed.len() as f64;

                        if iters == 1 {
                            println!("{}D GroupedVector: {:.2}x compression", dimension, compression_ratio);
                        }

                        black_box(compressed);
                    }
                    start.elapsed()
                });
            });
        }
    }

    group.finish();
}

/// Benchmark decoding strategies across different dimensions
fn bench_decoding_strategies(c: &mut Criterion) {
    let dimensions = vec![384, 768, 1536, 3072];
    let vector_count = 1000;

    let mut group = c.benchmark_group("decoding_strategies");
    group.sample_size(10);

    for dimension in dimensions {
        let vectors = generate_test_vectors(vector_count, dimension);

        // Pre-encode data for decoding benchmarks
        let columnar_config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::TransposeVector,
            algorithm: CompressionAlgorithm::Lz4,
            compression_level: 1,
            enable_vector_compression: true,
            enable_metadata_compression: false,
            compression_threshold_bytes: 0,
            dictionary_compression: false,
        };

        let rowwise_config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::FullVector,
            algorithm: CompressionAlgorithm::Lz4,
            compression_level: 1,
            enable_vector_compression: false,
            enable_metadata_compression: false,
            compression_threshold_bytes: 0,
            dictionary_compression: false,
        };

        let columnar_encoded = create_and_serialize_block(&vectors, &columnar_config)
            .expect("Failed to encode columnar data for decoding benchmark");

        let rowwise_encoded = create_and_serialize_block(&vectors, &rowwise_config)
            .expect("Failed to encode row-wise data for decoding benchmark");

        // Columnar decoding benchmark
        group.bench_with_input(
            BenchmarkId::new("columnar_decode", dimension),
            &columnar_encoded,
            |b, encoded_data| {
                b.iter(|| {
                    let result = FastLanesDataBlock::deserialize(black_box(encoded_data));
                    black_box(result)
                });
            },
        );

        // Row-wise decoding benchmark
        group.bench_with_input(
            BenchmarkId::new("rowwise_decode", dimension),
            &rowwise_encoded,
            |b, encoded_data| {
                b.iter(|| {
                    let result = FastLanesDataBlock::deserialize(black_box(encoded_data));
                    black_box(result)
                });
            },
        );

        // GroupedVector decoding benchmark for high dimensions
        if dimension > 128 {
            let grouped_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::GroupedVector,
                algorithm: CompressionAlgorithm::Lz4,
                compression_level: 1,
                enable_vector_compression: true,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };

            let grouped_encoded = create_and_serialize_block(&vectors, &grouped_config)
                .expect("Failed to encode grouped data for decoding benchmark");

            group.bench_with_input(
                BenchmarkId::new("grouped_decode", dimension),
                &grouped_encoded,
                |b, encoded_data| {
                    b.iter(|| {
                        let result = FastLanesDataBlock::deserialize(black_box(encoded_data));
                        black_box(result)
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark encode-decode round-trip performance
fn bench_roundtrip_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip_performance");
    group.sample_size(10);

    let dimensions = vec![384, 768, 1536];
    let vector_count = 1000;

    for dimension in dimensions {
        let vectors = generate_test_vectors(vector_count, dimension);

        // Columnar round-trip
        group.bench_with_input(
            BenchmarkId::new("columnar_roundtrip", dimension),
            &vectors,
            |b, vectors| {
                let config = BlockCompressionConfig {
                    vector_layout: VectorEncodingLayout::TransposeVector,
                    algorithm: CompressionAlgorithm::Lz4,
                    compression_level: 1,
                    enable_vector_compression: true,
                    enable_metadata_compression: false,
                    compression_threshold_bytes: 0,
                    dictionary_compression: false,
                };

                b.iter(|| {
                    let encoded = create_and_serialize_block(vectors, &config)
                        .expect("Encoding failed");
                    let decoded = FastLanesDataBlock::deserialize(&encoded)
                        .expect("Decoding failed");
                    black_box(decoded)
                });
            },
        );

        // Row-wise round-trip
        group.bench_with_input(
            BenchmarkId::new("rowwise_roundtrip", dimension),
            &vectors,
            |b, vectors| {
                let config = BlockCompressionConfig {
                    vector_layout: VectorEncodingLayout::FullVector,
                    algorithm: CompressionAlgorithm::Lz4,
                    compression_level: 1,
                    enable_vector_compression: false,
                    enable_metadata_compression: false,
                    compression_threshold_bytes: 0,
                    dictionary_compression: false,
                };

                b.iter(|| {
                    let encoded = create_and_serialize_block(vectors, &config)
                        .expect("Encoding failed");
                    let decoded = FastLanesDataBlock::deserialize(&encoded)
                        .expect("Decoding failed");
                    black_box(decoded)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark decoding performance with different compression algorithms
fn bench_decode_compression_algorithms(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_compression_algorithms");
    group.measurement_time(std::time::Duration::from_secs(15));

    let vectors = generate_test_vectors(1000, 768);
    let algorithms = vec![
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Snappy,
        CompressionAlgorithm::Gzip,
        CompressionAlgorithm::Zstd,
    ];

    // Pre-encode with each algorithm
    let mut encoded_data = Vec::new();

    for algorithm in &algorithms {
        let config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::TransposeVector,
            algorithm: algorithm.clone(),
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 4096,
            dictionary_compression: false,
        };

        let encoded = create_and_serialize_block(&vectors, &config)
            .expect("Failed to encode for decoding benchmark");
        encoded_data.push((algorithm.clone(), encoded));
    }

    // Benchmark decoding for each algorithm
    for (algorithm, encoded) in encoded_data {
        group.bench_with_input(
            BenchmarkId::new("decode_algorithm", format!("{:?}", algorithm)),
            &encoded,
            |b, encoded_data| {
                b.iter(|| {
                    let result = FastLanesDataBlock::deserialize(black_box(encoded_data));
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark query pattern simulation: encode once, decode many times
fn bench_query_pattern_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_pattern_simulation");

    let vectors = generate_test_vectors(1000, 768);

    let columnar_config = create_worm_config();
    let rowwise_config = create_realtime_config();

    // Pre-encode data
    let columnar_encoded = create_and_serialize_block(&vectors, &columnar_config)
        .expect("Failed to encode columnar data");
    let rowwise_encoded = create_and_serialize_block(&vectors, &rowwise_config)
        .expect("Failed to encode row-wise data");

    // Simulate analytical workload: 1 encode, many decodes
    group.bench_function("analytical_workload_columnar", |b| {
        b.iter(|| {
            // Simulate multiple analytical queries (10 decodes per encode)
            for _ in 0..10 {
                let _decoded = FastLanesDataBlock::deserialize(black_box(&columnar_encoded));
                black_box(_decoded);
            }
        });
    });

    // Simulate OLTP workload: frequent small operations
    group.bench_function("oltp_workload_rowwise", |b| {
        b.iter(|| {
            // Simulate quick access patterns (single decode)
            let _decoded = FastLanesDataBlock::deserialize(black_box(&rowwise_encoded));
            black_box(_decoded);
        });
    });

    // Compare read-heavy vs write-heavy patterns
    group.bench_function("read_heavy_columnar", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                // 1 encode, 100 decodes (read-heavy)
                let _encoded = create_and_serialize_block(&vectors, &columnar_config);
                for _ in 0..100 {
                    let _decoded = FastLanesDataBlock::deserialize(&columnar_encoded);
                    black_box(_decoded);
                }
            }
            start.elapsed()
        });
    });

    group.bench_function("write_heavy_rowwise", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            for _ in 0..iters {
                // 10 encodes, 10 decodes (write-heavy)
                for _ in 0..10 {
                    let _encoded = create_and_serialize_block(&vectors, &rowwise_config);
                    let _decoded = FastLanesDataBlock::deserialize(&rowwise_encoded);
                    black_box((_encoded, _decoded));
                }
            }
            start.elapsed()
        });
    });

    group.finish();
}

/// Benchmark memory efficiency during decode operations
fn bench_decode_memory_efficiency(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_memory_efficiency");

    let vector_counts = vec![100, 1000, 5000];
    let dimension = 768;

    for count in vector_counts {
        let vectors = generate_test_vectors(count, dimension);

        let columnar_config = BlockCompressionConfig {
            vector_layout: VectorEncodingLayout::TransposeVector,
            algorithm: CompressionAlgorithm::Zstd,
            compression_level: 6,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 0,
            dictionary_compression: true,
        };

        let encoded = create_and_serialize_block(&vectors, &columnar_config)
            .expect("Failed to encode for memory benchmark");

        group.bench_with_input(
            BenchmarkId::new("memory_decode", format!("{}vectors", count)),
            &encoded,
            |b, encoded_data| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        // Measure decode with memory pressure
                        let decoded = FastLanesDataBlock::deserialize(encoded_data)
                            .expect("Decoding failed");

                        // Force memory usage
                        let _memory_pressure: Vec<_> = decoded.records.iter().take(10).collect();
                        black_box(_memory_pressure);
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_encoding_strategies,
    bench_decoding_strategies,
    bench_roundtrip_performance,
    bench_decode_compression_algorithms,
    bench_query_pattern_simulation,
    bench_decode_memory_efficiency,
    bench_worm_workload,
    bench_realtime_workload,
    bench_balanced_workload,
    bench_compression_tradeoffs,
    bench_compression_ratios
);
criterion_main!(benches);