//! Comprehensive benchmarks for ProximaDB optimization features
//! 
//! Validates performance improvements across:
//! - Vector serialization (bytemuck vs bincode)
//! - Compression algorithms (ZSTD, LZ4, raw)
//! - Memory pooling efficiency
//! - Fixed-length vs dynamic vectors
//! - SST vs VIPER engine performance

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proximadb::core::serialization::{
    VectorSerializationConfig, CompressionAlgorithm,
    fixed_length::{FixedVector, FixedLengthSerializer, Dim512, Dim1024},
    streaming::{StreamingCompressor, StreamingConfig},
};
use proximadb::core::memory::{VectorMemoryPool, PoolConfig};
use proximadb::storage::engines::sst::{SstRecord, DataBlock, DataBlockCompressionConfig};
use proximadb::storage::engines::viper::optimized_vector_writer::{
    OptimizedVectorWriter, OptimizedVectorWriterConfig
};
use proximadb::proto::proximadb::MetadataItem;
use proximadb::core::VectorRecord;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Generate test vectors with specified characteristics
fn generate_test_vectors(count: usize, dimension: usize, sparsity: f32) -> Vec<Vec<f32>> {
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    
    (0..count).map(|_| {
        let mut vector = vec![0.0; dimension];
        let non_zero_count = ((1.0 - sparsity) * dimension as f32) as usize;
        
        for _ in 0..non_zero_count {
            let idx = rng.gen_range(0..dimension);
            vector[idx] = rng.gen_range(-1.0..1.0);
        }
        
        vector
    }).collect()
}

/// Create test VectorRecord
fn create_vector_record(id: &str, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector,
        metadata: vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue("benchmark".to_string())),
            },
        ],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    }
}

/// Benchmark vector serialization: bytemuck vs bincode
fn bench_vector_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_serialization");
    
    for dimension in [128, 512, 1024] {
        for sparsity in [0.1, 0.5, 0.9] {
            let vectors = generate_test_vectors(100, dimension, sparsity);
            let test_name = format!("{}D_{}%_sparse", dimension, (sparsity * 100.0) as u32);
            
            group.throughput(Throughput::Elements(vectors.len() as u64));
            
            // Benchmark optimized serialization
            let mut config = VectorSerializationConfig::default();
            config.compression_algorithm = CompressionAlgorithm::Zstd;
            config.compression_level = 3;
            
            group.bench_with_input(
                BenchmarkId::new("bytemuck_zstd", &test_name),
                &vectors,
                |b, vectors| {
                    b.iter(|| {
                        for vector in vectors {
                            let _serialized = config.serialize_vector(vector).unwrap();
                        }
                    });
                },
            );
            
            // Benchmark legacy bincode
            group.bench_with_input(
                BenchmarkId::new("bincode", &test_name),
                &vectors,
                |b, vectors| {
                    b.iter(|| {
                        for vector in vectors {
                            let _serialized = bincode::serialize(vector).unwrap();
                        }
                    });
                },
            );
        }
    }
    
    group.finish();
}

/// Benchmark compression algorithms
fn bench_compression_algorithms(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_algorithms");
    
    let vectors = generate_test_vectors(50, 1024, 0.8); // Large sparse vectors
    
    let algorithms = vec![
        ("none", CompressionAlgorithm::None),
        ("zstd_1", CompressionAlgorithm::Zstd),
        ("lz4", CompressionAlgorithm::Lz4),
    ];
    
    for (name, algorithm) in algorithms {
        let mut config = VectorSerializationConfig::default();
        config.compression_algorithm = algorithm;
        config.compression_level = if name.contains("zstd") { 1 } else { 3 };
        
        group.throughput(Throughput::Bytes(
            (vectors.len() * 1024 * 4) as u64 // vectors * dimension * f32_size
        ));
        
        group.bench_with_input(
            BenchmarkId::new("compress", name),
            &config,
            |b, config| {
                b.iter(|| {
                    for vector in &vectors {
                        let _compressed = config.serialize_vector(vector).unwrap();
                    }
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark memory pooling effectiveness
fn bench_memory_pooling(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pooling");
    
    let vectors = generate_test_vectors(1000, 256, 0.3);
    let config = VectorSerializationConfig::default();
    
    // Without pooling
    group.bench_function("no_pooling", |b| {
        b.iter(|| {
            for vector in &vectors {
                let _serialized = config.serialize_vector(vector).unwrap();
            }
        });
    });
    
    // With pooling
    let pool = VectorMemoryPool::new();
    group.bench_function("with_pooling", |b| {
        b.iter(|| {
            let _serialized = pool.serialize_vector_batch_pooled(&vectors, &config).unwrap();
        });
    });
    
    group.finish();
}

/// Benchmark fixed-length vs dynamic vectors
fn bench_fixed_vs_dynamic(c: &mut Criterion) {
    let mut group = c.benchmark_group("fixed_vs_dynamic");
    
    // 512-dimensional vectors
    let dynamic_vectors = generate_test_vectors(100, 512, 0.5);
    let fixed_vectors: Vec<FixedVector<Dim512>> = dynamic_vectors.iter()
        .map(|v| FixedVector::new(v.clone()).unwrap())
        .collect();
    
    // Dynamic serialization
    let dynamic_config = VectorSerializationConfig::default();
    group.bench_function("dynamic_512d", |b| {
        b.iter(|| {
            for vector in &dynamic_vectors {
                let _serialized = dynamic_config.serialize_vector(vector).unwrap();
            }
        });
    });
    
    // Fixed-length serialization
    let fixed_serializer = FixedLengthSerializer::<Dim512>::default();
    group.bench_function("fixed_512d", |b| {
        b.iter(|| {
            for vector in &fixed_vectors {
                let _serialized = fixed_serializer.serialize(vector).unwrap();
            }
        });
    });
    
    group.finish();
}

/// Benchmark SST engine optimizations
fn bench_sst_optimizations(c: &mut Criterion) {
    let mut group = c.benchmark_group("sst_optimizations");
    
    let vectors = generate_test_vectors(100, 512, 0.6);
    let records: Vec<VectorRecord> = vectors.into_iter().enumerate()
        .map(|(i, v)| create_vector_record(&format!("sst_test_{}", i), v))
        .collect();
    
    // Legacy SstRecord serialization (bincode)
    let sst_records: Vec<proximadb::storage::engines::sst::SstRecord> = records.iter()
        .map(|r| proximadb::storage::engines::sst::SstRecord::from_vector_record(r.clone()))
        .collect();
    
    group.bench_function("legacy_bincode", |b| {
        b.iter(|| {
            for record in &sst_records {
                let _serialized = bincode::serialize(record).unwrap();
            }
        });
    });
    
    // Optimized SstRecord serialization
    group.bench_function("optimized_bytemuck", |b| {
        b.iter(|| {
            for record in &sst_records {
                let _serialized = record.serialize().unwrap();
            }
        });
    });
    
    // DataBlock compression
    let data_block = proximadb::storage::engines::sst::DataBlock::new(1, sst_records.clone());
    let compression_config = DataBlockCompressionConfig::default();
    
    group.bench_function("datablock_compressed", |b| {
        b.iter(|| {
            let _serialized = data_block.serialize_with_config(&compression_config).unwrap();
        });
    });
    
    group.finish();
}

/// Benchmark VIPER engine optimizations
fn bench_viper_optimizations(c: &mut Criterion) {
    let mut group = c.benchmark_group("viper_optimizations");
    
    let vectors = generate_test_vectors(200, 768, 0.4);
    let records: Vec<VectorRecord> = vectors.into_iter().enumerate()
        .map(|(i, v)| create_vector_record(&format!("viper_test_{}", i), v))
        .collect();
    
    // BinaryArray optimization
    let mut binary_config = OptimizedVectorWriterConfig::default();
    binary_config.use_binary_array = true;
    let binary_writer = OptimizedVectorWriter::new(binary_config);
    
    let schema = binary_writer.create_optimized_schema().unwrap();
    
    group.bench_function("binary_array", |b| {
        b.iter(|| {
            let _batch = binary_writer.records_to_optimized_batch(&records, &schema).unwrap();
        });
    });
    
    // ListArray fallback
    let mut list_config = OptimizedVectorWriterConfig::default();
    list_config.use_binary_array = false;
    let list_writer = OptimizedVectorWriter::new(list_config);
    
    group.bench_function("list_array_fallback", |b| {
        b.iter(|| {
            let _batch = list_writer.records_to_optimized_batch(&records, &schema).unwrap();
        });
    });
    
    group.finish();
}

/// Benchmark streaming compression
fn bench_streaming_compression(c: &mut Criterion) {
    let mut group = c.benchmark_group("streaming_compression");
    
    let rt = Runtime::new().unwrap();
    
    for worker_count in [1, 2, 4] {
        let config = StreamingConfig {
            worker_count,
            buffer_size: 50,
            ..Default::default()
        };
        
        let vectors = generate_test_vectors(500, 512, 0.7);
        let vector_config = VectorSerializationConfig::default();
        
        group.bench_with_input(
            BenchmarkId::new("streaming", format!("{}workers", worker_count)),
            &worker_count,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let compressor = StreamingCompressor::new(config.clone()).unwrap();
                    let _results = compressor.compress_stream(vectors.clone(), vector_config.clone()).await.unwrap();
                    compressor.shutdown().await.unwrap();
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark comprehensive end-to-end performance
fn bench_end_to_end_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end");
    group.measurement_time(Duration::from_secs(10));
    
    let rt = Runtime::new().unwrap();
    
    // Large-scale benchmark
    let vectors = generate_test_vectors(1000, 1024, 0.8);
    let records: Vec<VectorRecord> = vectors.into_iter().enumerate()
        .map(|(i, v)| create_vector_record(&format!("e2e_test_{}", i), v))
        .collect();
    
    group.throughput(Throughput::Elements(records.len() as u64));
    
    // SST pipeline
    group.bench_function("sst_pipeline", |b| {
        b.iter(|| {
            let sst_records: Vec<proximadb::storage::engines::sst::SstRecord> = records.iter()
                .map(|r| proximadb::storage::engines::sst::SstRecord::from_vector_record(r.clone()))
                .collect();
            
            let data_block = proximadb::storage::engines::sst::DataBlock::new(1, sst_records);
            let compression_config = DataBlockCompressionConfig::default();
            let _serialized = data_block.serialize_with_config(&compression_config).unwrap();
        });
    });
    
    // VIPER pipeline
    group.bench_function("viper_pipeline", |b| {
        b.iter(|| {
            let config = OptimizedVectorWriterConfig::default();
            let writer = OptimizedVectorWriter::new(config);
            let schema = writer.create_optimized_schema().unwrap();
            let _batch = writer.records_to_optimized_batch(&records, &schema).unwrap();
        });
    });
    
    // Streaming pipeline
    group.bench_function("streaming_pipeline", |b| {
        b.to_async(&rt).iter(|| async {
            let config = StreamingConfig::default();
            let compressor = StreamingCompressor::new(config).unwrap();
            let vector_config = VectorSerializationConfig::default();
            
            let vectors: Vec<Vec<f32>> = records.iter().map(|r| r.vector.clone()).collect();
            let _results = compressor.compress_stream(vectors, vector_config).await.unwrap();
            compressor.shutdown().await.unwrap();
        });
    });
    
    group.finish();
}

/// Benchmark memory usage patterns
fn bench_memory_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_patterns");
    
    // Test different pool configurations
    let vectors = generate_test_vectors(500, 256, 0.5);
    let config = VectorSerializationConfig::default();
    
    // Small pool
    let small_pool_config = PoolConfig {
        initial_size: 4,
        max_size: 16,
        ..Default::default()
    };
    let small_pool = VectorMemoryPool::with_config(small_pool_config);
    
    group.bench_function("small_pool", |b| {
        b.iter(|| {
            let _serialized = small_pool.serialize_vector_batch_pooled(&vectors, &config).unwrap();
        });
    });
    
    // Large pool
    let large_pool_config = PoolConfig {
        initial_size: 32,
        max_size: 128,
        ..Default::default()
    };
    let large_pool = VectorMemoryPool::with_config(large_pool_config);
    
    group.bench_function("large_pool", |b| {
        b.iter(|| {
            let _serialized = large_pool.serialize_vector_batch_pooled(&vectors, &config).unwrap();
        });
    });
    
    group.finish();
}

criterion_group!(
    optimization_benches,
    bench_vector_serialization,
    bench_compression_algorithms,
    bench_memory_pooling,
    bench_fixed_vs_dynamic,
    bench_sst_optimizations,
    bench_viper_optimizations,
    bench_streaming_compression,
    bench_end_to_end_performance,
    bench_memory_patterns,
);

criterion_main!(optimization_benches);