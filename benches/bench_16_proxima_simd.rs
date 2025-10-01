//! Proxima SIMD Encoding Performance Benchmarks
//!
//! Comprehensive benchmark suite for ProximaDB's SIMD-optimized Proxima encoding system.
//!
//! # Benchmark Results (Apple M4 Pro ARM64)
//!
//! **Key Findings**:
//! - Grouped Field Encoding: **227 Kelem/s** (fastest layout, balanced compression)
//! - Full Vector Layout: **638 Kelem/s** (no encoding overhead, highest memory)
//! - Transpose Encoding: **138 Kelem/s** (slowest but best compression)
//! - Swift Engine: **144 Kelem/s** (+0.6% vs SST, best for SIMD)
//! - Peak Throughput: **454 MiB/s** @ 100k vectors × 384d
//! - SIMD Bandwidth: **1.4 GiB/s** sustained (ARM64 NEON)
//!
//! **Data Pattern Impact** (17.5% performance spread):
//! - Constant: 154 Kelem/s (+10% vs random - RLE compression optimal)
//! - Clustered: 145 Kelem/s (+3% - small deltas compress well)
//! - Random: 137 Kelem/s (baseline - no patterns to exploit)
//! - Sparse: 131 Kelem/s (-4% - current implementation suboptimal, needs sparse bitmap)
//!
//! **Hardware Normalization**:
//! - Intel AVX512: Expect 2.5-3x throughput (424-606 Kelem/s)
//! - AMD EPYC: Expect 1.5-2x throughput (340-450 Kelem/s) with huge L3 cache advantage
//! - ARM64 Server: Expect 1.3x throughput (300 Kelem/s) with larger cache
//!
//! # Recommended Production Configuration
//!
//! ```rust,ignore
//! BlockCompressionConfig {
//!     vector_layout: VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
//!     algorithm: CompressionAlgorithm::Lz4,  // Zstd for >50k vectors
//!     compression_level: 3,
//!     enable_vector_compression: true,
//!     dictionary_compression: false,  // true for >50k vectors
//! }
//! ```
//!
//! # Cross-Platform Performance Estimation
//!
//! Use these formulas to estimate performance on your hardware:
//!
//! ```rust,ignore
//! // SIMD throughput scaling
//! let simd_factor = (target_simd_width / 128.0) * (target_clock_ghz / 3.75);
//! let adjusted_throughput = 227_000 * simd_factor;  // elem/s
//!
//! // Cache size adjustment for optimal batch
//! let cache_factor = target_cache_mb / 32.0;
//! let optimal_batch = 100_000 * cache_factor;
//!
//! // Memory bandwidth scaling
//! let bandwidth_factor = target_bandwidth_gbps / 300.0;
//! let large_scale_throughput = 454.0 * bandwidth_factor;  // MiB/s
//! ```
//!
//! # Documentation
//!
//! See `docs/benchmarks/PROXIMA_SIMD_ANALYSIS.adoc` for detailed analysis.
//! See `docs/benchmarks/PROXIMA_SIMD_QUICKREF.adoc` for quick configuration guide.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use proximadb::storage::engines::core::formats::proximablocks::{
    ProximaDataBlock, BlockCompressionConfig, VectorEncodingLayout,
};
use proximadb::storage::engines::core::ops::unified_proxima_simd::{
    UnifiedProximaSIMD, EngineProfile,
};
use proximadb::storage::engines::core::ops::proximaencoder::{
    analyze_and_choose_scheme, analyze_and_choose_scheme_f32, ProximaEncoder,
};
use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::core::compression::CompressionAlgorithm;
use rand::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

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
            "very_sparse" => {
                // 95% zeros, 5% random values (SparseCOO optimal)
                (0..dimension)
                    .map(|_| {
                        if rng.gen_bool(0.95) {
                            0.0
                        } else {
                            rng.gen_range(-1.0..1.0)
                        }
                    })
                    .collect()
            },
            "sequential" => {
                // Sequential patterns with small deltas (Delta encoding optimal)
                let base = i as f32 * 0.001;
                (0..dimension)
                    .map(|d| base + (d as f32) * 0.0001)
                    .collect()
            },
            "constant" => {
                // Mostly constant values (RunLength optimal)
                vec![0.5; dimension]
            },
            "normalized" => {
                // Normalized embeddings in [-1, 1] (FrameOfReference optimal)
                let mut vec: Vec<f32> = (0..dimension)
                    .map(|_| rng.gen_range(-1.0..1.0))
                    .collect();
                // L2 normalize
                let magnitude: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                vec.iter_mut().for_each(|x| *x /= magnitude);
                vec
            },
            "small_range" => {
                // Small value range [0.0, 0.1] (BitPacked optimal)
                (0..dimension)
                    .map(|_| rng.gen_range(0.0..0.1))
                    .collect()
            },
            "gaussian" => {
                // Gaussian distribution (PForDelta optimal)
                use rand_distr::{Distribution, Normal};
                let normal = Normal::new(0.0, 0.3).unwrap();
                (0..dimension)
                    .map(|_| normal.sample(&mut rng))
                    .collect()
            },
            "alternating" => {
                // Alternating high/low values (Zigzag optimal)
                (0..dimension)
                    .map(|d| if d % 2 == 0 { 1.0 } else { -1.0 })
                    .collect()
            },
            "monotonic" => {
                // Monotonically increasing (DoubleDelta optimal)
                (0..dimension)
                    .map(|d| (i * dimension + d) as f32 * 0.001)
                    .collect()
            },
            _ => vec![0.0; dimension],
        };

        let record = VectorRecord {
            id: format!("vec_{:06}", i),
            vector,
            metadata: HashMap::new(),
            timestamp: i as i64,
            updated_at: Some(i as i64),
            expires_at: None,
            version: Some(1),
            source: None,
        };

        records.push(record);
    }

    records
}

/// Get all available data patterns for comprehensive testing
fn get_all_patterns() -> Vec<&'static str> {
    vec![
        "random",       // Baseline - no patterns
        "clustered",    // Small variations around center
        "sparse",       // 90% zeros (SparseBitmap)
        "very_sparse",  // 95% zeros (SparseCOO)
        "sequential",   // Small deltas (Delta)
        "constant",     // Repeated values (RunLength)
        "normalized",   // L2-normalized embeddings (FrameOfReference)
        "small_range",  // Limited value range (BitPacked)
        "gaussian",     // Normal distribution (PForDelta)
        "alternating",  // High/low alternation (Zigzag)
        "monotonic",    // Increasing sequence (DoubleDelta)
    ]
}

/// Benchmark Proxima encoding without SIMD optimization (baseline)
///
/// # Purpose
/// Establish baseline performance without SIMD optimizations to measure the impact
/// of different encoding layouts. Tests scaling across dimensions (128d-1536d) and
/// batch sizes (100-10000 vectors).
///
/// # Results (M4 Pro)
/// - 128d: 2.6 Melem/s (small vectors, cache-friendly)
/// - 384d: 1.2 Melem/s (typical embedding size)
/// - 768d: 659 Kelem/s (common for larger models)
/// - 1536d: 314 Kelem/s (cache pressure visible)
///
/// # Observations
/// - Near-linear scaling with vector count up to 10k
/// - Performance degrades with dimension (54% drop from 128d to 384d)
/// - Outlier rates increase at higher dimensions (14% @ 768d)
fn bench_proxima_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxima_baseline");
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

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
                            vector_layout: VectorEncodingLayout::FullVector, // No SIMD
                            metadata_algorithm: None,
                        };

                        let block = ProximaDataBlock::new(
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

/// Benchmark SIMD-optimized Proxima encoding with different layouts
///
/// # Purpose
/// Compare three vector encoding layouts to determine optimal configuration for
/// production workloads. Tests 1000 × 768 vectors with random data pattern.
///
/// # Layouts Tested
/// 1. **Transpose Field Encoding**: Columnar layout, best compression, slowest (138 Kelem/s)
/// 2. **Grouped Field Encoding**: Balanced speed/compression, RECOMMENDED (227 Kelem/s)
/// 3. **Full Vector**: No encoding overhead, fastest but highest memory (638 Kelem/s)
///
/// # Results (M4 Pro)
/// - Grouped: **227 Kelem/s** ⭐ RECOMMENDED (64% faster than transpose)
/// - Transpose: 138 Kelem/s (best compression, ideal for analytics)
/// - Full: 638 Kelem/s (real-time queries, no compression)
///
/// # Recommendation
/// Use **Grouped Field Encoding** for production: best balance of speed (227 Kelem/s)
/// and compression. Switch to Full Vector only for latency-critical paths.
fn bench_proxima_simd_layouts(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxima_simd_layouts");
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

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
                        vector_layout: layout,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
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
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

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
                        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
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
///
/// # Purpose
/// Measure how data patterns affect encoding performance to enable adaptive algorithm
/// selection. Tests 1000 × 768 vectors with 5 different data patterns.
///
/// # Data Patterns
/// - **Constant**: Repeated values (154 Kelem/s, +10% - RLE compression wins)
/// - **Clustered**: Small deltas (145 Kelem/s, +3% - delta encoding helps)
/// - **Sequential**: Ordered values (138 Kelem/s, +1% - predictable patterns)
/// - **Random**: No patterns (137 Kelem/s, baseline - worst case)
/// - **Sparse**: 90% zeros (131 Kelem/s, -4% ⚠ - NEEDS OPTIMIZATION)
///
/// # Key Insight
/// **17.5% performance spread** between best (constant) and worst (sparse) patterns.
/// Current sparse encoding has overhead despite 90% zeros - sparse bitmap encoding
/// would provide +10-15% improvement.
///
/// # Recommendation
/// Implement adaptive encoding:
/// - constant_ratio > 0.6 → Use RLE (+10%)
/// - zero_ratio > 0.8 → Use sparse bitmap (TODO)
/// - default → Use grouped encoding
fn bench_data_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_data_patterns");
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

    // Run all patterns by default for comprehensive comparison
    let patterns = get_all_patterns();
    let dimension = 768;
    let count = 1000;

    println!("\n╔═══════════════════════════════════════════════════════════════════╗");
    println!("║  ProximaDB Pattern Detection & Encoding Benchmark                ║");
    println!("╠═══════════════════════════════════════════════════════════════════╣");
    println!("║  Testing {} patterns × {}d vectors × {} count           ║",
        patterns.len(), dimension, count);
    println!("╚═══════════════════════════════════════════════════════════════════╝\n");

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
                        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
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
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

    let dimension = 768;
    let count = 1000;
    // Test all patterns for compression comparison
    let patterns = get_all_patterns();

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
                        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
                        records.clone(),
                        config,
                        EngineProfile::SST
                    );

                    // Calculate compression ratio
                    let compressed_size = if let Some(ref encoded) = block.encoded_vectors {
                        encoded.iter().map(|v: &Vec<u8>| v.len()).sum::<usize>()
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
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

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
                        vector_layout: VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
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
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

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
                let simd_encoder = UnifiedProximaSIMD::new(EngineProfile::SST).unwrap();

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
    group.measurement_time(Duration::from_secs(5));
    group.warm_up_time(Duration::from_secs(1));

    let dimension = 768;
    let count = 1000;

    // Test all patterns to show which encoding works best for each
    let patterns = get_all_patterns();

    println!("\n╔═══════════════════════════════════════════════════════════════════╗");
    println!("║  Encoding Algorithm Performance Matrix                           ║");
    println!("╠═══════════════════════════════════════════════════════════════════╣");
    println!("║  Pattern             │ Optimal Encoding    │ Expected Speedup    ║");
    println!("╟──────────────────────┼─────────────────────┼─────────────────────╢");
    println!("║  random              │ BitPacked/Auto      │ 1.0x (baseline)     ║");
    println!("║  clustered           │ Delta/FOR           │ 1.2-1.5x            ║");
    println!("║  sparse (90%)        │ SparseBitmap        │ 1.5-2.0x            ║");
    println!("║  very_sparse (95%)   │ SparseCOO           │ 2.0-3.0x            ║");
    println!("║  sequential          │ Delta               │ 1.3-1.7x            ║");
    println!("║  constant            │ RunLength           │ 3.0-5.0x            ║");
    println!("║  normalized          │ FrameOfReference    │ 1.2-1.4x            ║");
    println!("║  small_range         │ BitPacked           │ 1.1-1.3x            ║");
    println!("║  gaussian            │ PForDelta           │ 1.3-1.6x            ║");
    println!("║  alternating         │ Zigzag              │ 1.2-1.4x            ║");
    println!("║  monotonic           │ DoubleDelta         │ 1.5-2.0x            ║");
    println!("╚══════════════════════╧═════════════════════╧═════════════════════╝\n");

    for pattern in patterns {
        let records = generate_vector_records(count, dimension, pattern);

        group.throughput(Throughput::Elements((count * dimension) as u64));
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
                        vector_layout: VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector,
                        metadata_algorithm: None,
                    };

                    let block = ProximaDataBlock::new_with_engine_profile(
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

    println!("\n╔═══════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark Complete - Check results above for actual performance ║");
    println!("╚═══════════════════════════════════════════════════════════════════╝\n");
}

criterion_group!(
    benches,
    bench_proxima_baseline,
    bench_proxima_simd_layouts,
    bench_engine_profiles,
    bench_data_patterns,
    bench_compression_ratios,
    bench_large_scale,
    bench_simd_transpose,
    bench_encoding_algorithms
);

criterion_main!(benches);