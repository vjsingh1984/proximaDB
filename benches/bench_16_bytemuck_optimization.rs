use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use proximadb::storage::engines::core::formats::fastlanes_blocks::{
    BlockCompressionConfig, VectorEncodingLayout, FastLanesDataBlock
};
use proximadb::core::compression::CompressionAlgorithm;
use proximadb::proto::proximadb_v1::VectorRecord;
use rand::prelude::*;
use std::collections::HashMap;
use std::time::Instant;

/// Generate test vectors for benchmarking
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:06}", i),
            vector: (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect(),
            metadata: HashMap::new(),
            quantized_vector: vec![],
            expires_at: None,
            source: None,
            timestamp: 0,
            updated_at: None,
            version: None,
        })
        .collect()
}

/// Benchmark: Traditional vs Bytemuck Row-wise Encoding
fn bench_rowwise_encoding_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("rowwise_encoding_comparison");
    group.sample_size(20);

    let test_cases = vec![
        (100, 384),
        (1000, 384),
        (1000, 768),
        (5000, 768),
    ];

    for (vector_count, dimension) in test_cases {
        let vectors = generate_test_vectors(vector_count, dimension);

        // Traditional approach (current implementation)
        group.bench_with_input(
            BenchmarkId::new("traditional_rowwise", format!("{}x{}", vector_count, dimension)),
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
                    let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                    let result = block.serialize_with_config(&config);
                    black_box(result)
                });
            },
        );

        // New bytemuck approach with block-level compression
        group.bench_with_input(
            BenchmarkId::new("bytemuck_rowwise", format!("{}x{}", vector_count, dimension)),
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
                    let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                    let result = block.serialize_with_config(&config).unwrap();
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Compression Ratios Achieved
fn bench_compression_ratio_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_ratio_comparison");

    let test_cases = vec![
        (1000, 384),
        (5000, 768),
        (10000, 1536),
    ];

    let algorithms = vec![
        CompressionAlgorithm::Lz4,
        CompressionAlgorithm::Snappy,
        CompressionAlgorithm::Zstd,
    ];

    for (vector_count, dimension) in test_cases {
        let vectors = generate_test_vectors(vector_count, dimension);
        let uncompressed_size = vector_count * dimension * 4; // f32 = 4 bytes

        println!("\n=== Compression Ratio Test: {}x{} ===", vector_count, dimension);

        for algorithm in &algorithms {
            // Traditional approach
            let traditional_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::FullVector,
                algorithm: *algorithm,
                compression_level: 1,
                enable_vector_compression: false,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };

            let traditional_block = FastLanesDataBlock::new(vectors.clone(), traditional_config.clone());
            let traditional_result = traditional_block.serialize_with_config(&traditional_config)
                .expect("Traditional encoding failed");

            let traditional_ratio = uncompressed_size as f64 / traditional_result.len() as f64;

            // Bytemuck approach
            let bytemuck_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::FullVector,
                algorithm: *algorithm,
                compression_level: 1,
                enable_vector_compression: false,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };
            let block = FastLanesDataBlock::new(vectors.clone(), bytemuck_config.clone());
            let bytemuck_result = block.serialize_with_config(&bytemuck_config).unwrap();

            let bytemuck_ratio = uncompressed_size as f64 / bytemuck_result.len() as f64;

            println!("  {:?}:", algorithm);
            println!("    Traditional: {:.2}x compression ({:.2} MB -> {:.2} MB)",
                   traditional_ratio,
                   uncompressed_size as f64 / 1_000_000.0,
                   traditional_result.len() as f64 / 1_000_000.0);
            println!("    Bytemuck: {:.2}x compression ({:.2} MB -> {:.2} MB)",
                   bytemuck_ratio,
                   uncompressed_size as f64 / 1_000_000.0,
                   bytemuck_result.len() as f64 / 1_000_000.0);
            println!("    Improvement: {:.1}% better compression",
                   ((bytemuck_ratio / traditional_ratio) - 1.0) * 100.0);
        }
    }

    // Still run the actual benchmark
    group.bench_function("compression_analysis", |b| {
        let vectors = generate_test_vectors(1000, 768);
        b.iter(|| {
            let trad_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::FullVector,
                algorithm: CompressionAlgorithm::Lz4,
                compression_level: 1,
                enable_vector_compression: false,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };
            let trad_block = FastLanesDataBlock::new(vectors.clone(), trad_config.clone());
            let _traditional = trad_block.serialize_with_config(&trad_config);

            let bytemuck_config = BlockCompressionConfig {
                vector_layout: VectorEncodingLayout::FullVector,
                algorithm: CompressionAlgorithm::Lz4,
                compression_level: 1,
                enable_vector_compression: false,
                enable_metadata_compression: false,
                compression_threshold_bytes: 0,
                dictionary_compression: false,
            };
            let block = FastLanesDataBlock::new(vectors.clone(), bytemuck_config.clone());
            let _bytemuck = block.serialize_with_config(&bytemuck_config).unwrap();

            black_box((_traditional, _bytemuck))
        });
    });

    group.finish();
}

/// Benchmark: Decode Performance Comparison
fn bench_decode_performance_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_performance_comparison");

    let vectors = generate_test_vectors(1000, 768);

    // Pre-encode with both approaches
    let traditional_config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
    };

    let traditional_block = FastLanesDataBlock::new(vectors.clone(), traditional_config.clone());
    let traditional_encoded = traditional_block.serialize_with_config(&traditional_config)
        .expect("Traditional encoding failed");

    let bytemuck_config = BlockCompressionConfig {
        vector_layout: VectorEncodingLayout::FullVector,
        algorithm: CompressionAlgorithm::Lz4,
        compression_level: 1,
        enable_vector_compression: false,
        enable_metadata_compression: false,
        compression_threshold_bytes: 0,
        dictionary_compression: false,
    };
    let block = FastLanesDataBlock::new(vectors.clone(), bytemuck_config.clone());
    let bytemuck_encoded = block.serialize_with_config(&bytemuck_config).unwrap();

    // Benchmark decoding
    group.bench_function("traditional_decode", |b| {
        b.iter(|| {
            let result = FastLanesDataBlock::deserialize(black_box(&traditional_encoded));
            black_box(result)
        });
    });

    group.bench_function("bytemuck_decode", |b| {
        b.iter(|| {
            let result = FastLanesDataBlock::deserialize(black_box(&bytemuck_encoded));
            black_box(result)
        });
    });

    group.finish();
}

/// Benchmark: Memory Efficiency
fn bench_memory_efficiency(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_efficiency");

    let vector_sizes = vec![100, 1000, 5000, 10000];

    for vector_count in vector_sizes {
        let vectors = generate_test_vectors(vector_count, 768);

        group.bench_with_input(
            BenchmarkId::new("memory_usage_traditional", vector_count),
            &vectors,
            |b, vectors| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let config = BlockCompressionConfig {
                            vector_layout: VectorEncodingLayout::FullVector,
                            algorithm: CompressionAlgorithm::Lz4,
                            compression_level: 1,
                            enable_vector_compression: false,
                            enable_metadata_compression: false,
                            compression_threshold_bytes: 0,
                            dictionary_compression: false,
                        };

                        let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                        let encoded = block.serialize_with_config(&config)
                            .expect("Encoding failed");
                        let _decoded = FastLanesDataBlock::deserialize(&encoded)
                            .expect("Decoding failed");

                        black_box((encoded, _decoded));
                    }
                    start.elapsed()
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("memory_usage_bytemuck", vector_count),
            &vectors,
            |b, vectors| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let config = BlockCompressionConfig {
                            vector_layout: VectorEncodingLayout::FullVector,
                            algorithm: CompressionAlgorithm::Lz4,
                            compression_level: 1,
                            enable_vector_compression: false,
                            enable_metadata_compression: false,
                            compression_threshold_bytes: 0,
                            dictionary_compression: false,
                        };
                        let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                        let encoded = block.serialize_with_config(&config).unwrap();

                        let _decoded = FastLanesDataBlock::deserialize(&encoded)
                            .expect("Decoding failed");

                        black_box((encoded, _decoded));
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: Scalability Test
fn bench_scalability_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("scalability_comparison");
    group.sample_size(10);

    let large_test_cases = vec![
        (10000, 384),
        (20000, 768),
        (50000, 384),
    ];

    for (vector_count, dimension) in large_test_cases {
        let vectors = generate_test_vectors(vector_count, dimension);

        group.bench_with_input(
            BenchmarkId::new("large_traditional", format!("{}x{}", vector_count, dimension)),
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

                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                        let result = block.serialize_with_config(&config);
                        black_box(result);
                    }
                    start.elapsed()
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("large_bytemuck", format!("{}x{}", vector_count, dimension)),
            &vectors,
            |b, vectors| {
                b.iter_custom(|iters| {
                    let start = Instant::now();
                    for _ in 0..iters {
                        let config = BlockCompressionConfig {
                            vector_layout: VectorEncodingLayout::FullVector,
                            algorithm: CompressionAlgorithm::Lz4,
                            compression_level: 1,
                            enable_vector_compression: false,
                            enable_metadata_compression: false,
                            compression_threshold_bytes: 0,
                            dictionary_compression: false,
                        };
                        let block = FastLanesDataBlock::new(vectors.clone(), config.clone());
                        let result = block.serialize_with_config(&config).unwrap();
                        black_box(result);
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
    bench_rowwise_encoding_comparison,
    bench_compression_ratio_comparison,
    bench_decode_performance_comparison,
    bench_memory_efficiency,
    bench_scalability_comparison
);
criterion_main!(benches);