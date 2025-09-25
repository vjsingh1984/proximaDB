use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use proximadb::storage::engines::core::formats::fastlanes_blocks::{
    FastLanesDataBlock, BlockCompressionConfig, VectorEncodingLayout,
};
use proximadb::storage::engines::core::ops::unified_fastlanes_simd::{
    UnifiedFastLanesSIMD, EngineProfile,
};
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::core::compression::CompressionAlgorithm;
use rand::prelude::*;
use std::collections::HashMap;

/// Generate synthetic vector records for benchmarking
fn generate_vector_records(count: usize, dimension: usize, pattern: &str) -> Vec<VectorRecord> {
    let mut rng = rand::thread_rng();
    let mut records = Vec::with_capacity(count);

    for i in 0..count {
        let vector = match pattern {
            "random" => {
                // Random vectors in [-1, 1]
                (0..dimension)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect()
            },
            "clustered" => {
                // Clustered around a center with small variations
                let center = 0.5;
                let variance = 0.1;
                (0..dimension)
                    .map(|_| center + rng.gen_range(-variance..variance))
                    .collect()
            },
            "sparse" => {
                // 90% zeros, 10% random values
                (0..dimension)
                    .map(|_| {
                        if rng.gen_bool(0.9) {
                            0.0
                        } else {
                            rng.gen_range(-1.0..1.0)
                        }
                    })
                    .collect()
            },
            "sequential" => {
                // Sequential patterns with small deltas
                let base = i as f32 * 0.001;
                (0..dimension)
                    .map(|d| base + (d as f32) * 0.0001)
                    .collect()
            },
            "constant" => {
                // Mostly constant values
                vec![0.5; dimension]
            },
            _ => vec![0.0; dimension],
        };

        let record = VectorRecord {
            id: format!("vec_{:06}", i),
            vector,
            metadata: HashMap::new(),
            timestamp: i as u64,
            updated_at: Some(i as i64),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        };

        records.push(record);
    }

    records
}

/// Benchmark FastLanes encoding without SIMD
fn bench_fastlanes_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("fastlanes_baseline");

    let dimensions = vec![128, 384, 768, 1536];
    let record_counts = vec![100, 1000, 10000];

    for dim in &dimensions {
        for count in &record_counts {
            let records = generate_vector_records(*count, *dim, "random");

            group.throughput(Throughput::Elements(*count as u64));
            group.bench_function(
                BenchmarkId::from_parameter(format!("{}x{}", count, dim)),
                |b| {
                    b.iter(|| {
                        let config = BlockCompressionConfig {
                            algorithm: CompressionAlgorithm::Lz4,
                            compression_level: 3,
                            enable_vector_compression: true,
                            enable_metadata_compression: true,
                            compression_threshold_bytes: 256,
                            dictionary_compression: false,
                            vector_layout: Some(VectorEncodingLayout::FullVector), // No SIMD
                            metadata_algorithm: None,
                        };

                        let block = FastLanesDataBlock::new(
                            black_box(records.clone()),
                            config
                        );

                        black_box(block);
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark SIMD-optimized FastLanes encoding with different layouts
fn bench_fastlanes_simd_layouts(c: &mut Criterion) {
    let mut group = c.benchmark_group("fastlanes_simd_layouts");

    let layouts = vec![
        ("transpose", VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
        ("grouped", VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector),
        ("full", VectorEncodingLayout::FullVector),
    ];

    let dimension = 768;
    let count = 1000;
    let records = generate_vector_records(count, dimension, "random");

    for (name, layout) in layouts {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(
            BenchmarkId::from_parameter(name),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Lz4,
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: false,
                        vector_layout: Some(layout),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        black_box(records.clone()),
                        config,
                        EngineProfile::SST
                    );

                    black_box(block);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark different engine profiles
fn bench_engine_profiles(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_profiles");

    let profiles = vec![
        ("helix", EngineProfile::Helix),
        ("sst", EngineProfile::SST),
        ("swift", EngineProfile::Swift),
    ];

    let dimension = 768;
    let count = 1000;
    let records = generate_vector_records(count, dimension, "clustered");

    for (name, profile) in profiles {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(
            BenchmarkId::from_parameter(name),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Lz4,
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: false,
                        vector_layout: Some(VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        black_box(records.clone()),
                        config,
                        profile
                    );

                    black_box(block);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark SIMD performance on different data patterns
fn bench_data_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_data_patterns");

    let patterns = vec!["random", "clustered", "sparse", "sequential", "constant"];
    let dimension = 768;
    let count = 1000;

    for pattern in patterns {
        let records = generate_vector_records(count, dimension, pattern);

        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(
            BenchmarkId::from_parameter(pattern),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Lz4,
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: false,
                        vector_layout: Some(VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        black_box(records.clone()),
                        config,
                        EngineProfile::SST
                    );

                    black_box(block);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark compression ratios achieved
fn bench_compression_ratios(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_ratios");

    let dimension = 768;
    let count = 1000;
    let patterns = vec!["random", "clustered", "sparse", "sequential"];

    for pattern in patterns {
        let records = generate_vector_records(count, dimension, pattern);
        let original_size = count * dimension * 4; // 4 bytes per f32

        group.bench_function(
            BenchmarkId::from_parameter(format!("{}_ratio", pattern)),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Lz4,
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: false,
                        vector_layout: Some(VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        records.clone(),
                        config,
                        EngineProfile::SST
                    );

                    // Calculate compression ratio
                    let compressed_size = if let Some(ref encoded) = block.encoded_vectors {
                        encoded.iter().map(|v| v.len()).sum::<usize>()
                    } else {
                        original_size
                    };

                    let ratio = (compressed_size as f64 / original_size as f64) * 100.0;

                    // Return ratio for analysis
                    black_box(ratio);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark large-scale performance
fn bench_large_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_scale_simd");
    group.sample_size(10); // Fewer samples for large benchmarks

    let scales = vec![
        ("10k_vectors", 10000, 768),
        ("100k_vectors", 100000, 384),
        ("1M_vectors_128d", 1000000, 128),
    ];

    for (name, count, dim) in scales {
        let records = generate_vector_records(count, dim, "clustered");

        group.throughput(Throughput::Bytes((count * dim * 4) as u64));
        group.bench_function(
            BenchmarkId::from_parameter(name),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Zstd, // Better for large data
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: true, // Enable for large blocks
                        vector_layout: Some(VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        black_box(records.clone()),
                        config,
                        EngineProfile::Swift // Swift for hierarchical data
                    );

                    black_box(block);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark SIMD transpose operation specifically
fn bench_simd_transpose(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_transpose");

    let dimensions = vec![128, 256, 384, 768, 1536];
    let count = 1000;

    for dim in dimensions {
        let vectors: Vec<Vec<f32>> = (0..count)
            .map(|_| (0..dim).map(|_| rand::random::<f32>()).collect())
            .collect();

        group.throughput(Throughput::Bytes((count * dim * 4) as u64));
        group.bench_function(
            BenchmarkId::from_parameter(format!("{}x{}", count, dim)),
            |b| {
                let simd_encoder = UnifiedFastLanesSIMD::new(EngineProfile::SST);

                b.iter(|| {
                    let transposed = simd_encoder.simd_transpose_vectors(&vectors).unwrap();
                    black_box(transposed);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark encoding algorithms comparison
fn bench_encoding_algorithms(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoding_algorithms");

    let dimension = 768;
    let count = 1000;

    // Generate different patterns that benefit from different encodings
    let test_cases = vec![
        ("delta_friendly", generate_vector_records(count, dimension, "sequential")),
        ("sparse_data", generate_vector_records(count, dimension, "sparse")),
        ("constant_runs", generate_vector_records(count, dimension, "constant")),
        ("random_data", generate_vector_records(count, dimension, "random")),
    ];

    for (name, records) in test_cases {
        group.throughput(Throughput::Elements((count * dimension) as u64));
        group.bench_function(
            BenchmarkId::from_parameter(name),
            |b| {
                b.iter(|| {
                    let config = BlockCompressionConfig {
                        algorithm: CompressionAlgorithm::Lz4,
                        compression_level: 3,
                        enable_vector_compression: true,
                        enable_metadata_compression: true,
                        compression_threshold_bytes: 256,
                        dictionary_compression: false,
                        vector_layout: Some(VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector),
                        metadata_algorithm: None,
                    };

                    let block = FastLanesDataBlock::new_with_engine_profile(
                        black_box(records.clone()),
                        config,
                        EngineProfile::SST
                    );

                    black_box(block);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_fastlanes_baseline,
    bench_fastlanes_simd_layouts,
    bench_engine_profiles,
    bench_data_patterns,
    bench_compression_ratios,
    bench_large_scale,
    bench_simd_transpose,
    bench_encoding_algorithms
);

criterion_main!(benches);