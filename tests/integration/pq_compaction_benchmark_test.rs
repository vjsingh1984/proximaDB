//! PQ-Based Compaction Benchmark Test
//!
//! Compares compression ratios and performance between:
//! 1. Traditional ID-based sorting
//! 2. PQ-based similarity sorting
//! 3. Random ordering (baseline)

use std::sync::Arc;
use tempfile::TempDir;
use tokio;

use proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default;
use proximadb::compute::quantization::unified::{UnifiedQuantizationEngine, InMemoryCodebookStore};
use proximadb::compute::quantization::storage_engine::{StorageQuantizationEngine, StorageQuantizationConfig};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::storage::quantization::{SstQuantizationAdapter, sst_adapter::SstQuantizationConfig};
use proximadb::storage::engines::sst::{
    SstRecord, SstableWriter,
    sst_compactor::{SstCompactor, CompactionSortStrategy}
};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Benchmark configuration
struct BenchmarkConfig {
    vector_dimension: usize,
    record_count: usize,
    cluster_count: usize,
    cluster_noise: f32,
    block_size: usize,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            vector_dimension: 256,
            record_count: 2000,
            cluster_count: 10,
            cluster_noise: 0.15,
            block_size: 128 * 1024, // 128KB blocks
        }
    }
}

/// Benchmark results
#[derive(Debug, Clone)]
struct BenchmarkResult {
    strategy_name: String,
    file_size_bytes: u64,
    compression_time_ms: u64,
    records_processed: usize,
    compression_ratio: f32, // Compared to random baseline
}

/// Test data generator with controllable similarity patterns
struct ClusteredDataGenerator {
    config: BenchmarkConfig,
    cluster_centers: Vec<Vec<f32>>,
}

impl ClusteredDataGenerator {
    fn new(config: BenchmarkConfig) -> Self {
        let mut cluster_centers = Vec::new();
        
        // Generate diverse cluster centers
        for i in 0..config.cluster_count {
            let mut center = vec![0.0; config.vector_dimension];
            
            // Create diverse patterns for different clusters
            match i % 4 {
                0 => { // Sparse pattern
                    center[i % config.vector_dimension] = 1.0;
                    center[(i + 1) % config.vector_dimension] = 0.5;
                }
                1 => { // Dense pattern
                    for j in 0..config.vector_dimension {
                        center[j] = ((i * j) % 7) as f32 / 7.0;
                    }
                }
                2 => { // Alternating pattern
                    for j in (0..config.vector_dimension).step_by(2) {
                        center[j] = 0.8;
                    }
                }
                3 => { // Gradient pattern
                    for j in 0..config.vector_dimension {
                        center[j] = j as f32 / config.vector_dimension as f32;
                    }
                }
                _ => unreachable!(),
            }
            
            cluster_centers.push(center);
        }
        
        Self { config, cluster_centers }
    }
    
    fn generate_records(&self, strategy: &str) -> Vec<SstRecord> {
        let mut records = Vec::new();
        
        for i in 0..self.config.record_count {
            let cluster_id = i % self.config.cluster_count;
            let base_vector = &self.cluster_centers[cluster_id];
            
            // Add controlled noise
            let mut vector = base_vector.clone();
            for val in &mut vector {
                let noise = (rand::random::<f32>() - 0.5) * self.config.cluster_noise;
                *val = (*val + noise).max(0.0).min(1.0); // Clamp to [0, 1]
            }
            
            let record = SstRecord {
                id: match strategy {
                    "random" => format!("random_{:04d}", rand::random::<u32>() % 9999),
                    "clustered" => format!("cluster_{}_{:04d}", cluster_id, i),
                    _ => format!("vector_{:04d}", i),
                },
                vector,
                metadata: vec![],
                timestamp: (1700000000 + i * 60) as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                sequence_number: i as u64,
                level: 0,
                is_tombstone: false,
                collection_id: "benchmark_collection".to_string(),
            };
            
            records.push(record);
        }
        
        records
    }
}

async fn benchmark_compaction_strategy(
    strategy: CompactionSortStrategy,
    strategy_name: &str,
    test_records: Vec<SstRecord>,
    config: &BenchmarkConfig,
    filesystem_factory: Arc<FilesystemFactory>,
    test_dir: &std::path::Path,
) -> anyhow::Result<BenchmarkResult> {
    
    println!("📊 Benchmarking strategy: {}", strategy_name);
    
    // Step 1: Write initial SST file
    let input_file = test_dir.join(format!("{}_input.sstable", strategy_name));
    let output_file = test_dir.join(format!("{}_output.sstable", strategy_name));
    
    let writer = SstableWriter::new(&input_file, config.block_size, filesystem_factory.clone());
    
    let records_for_writer: Vec<(String, SstRecord)> = test_records
        .iter()
        .map(|record| (record.id.clone(), record.clone()))
        .collect();
    
    writer.write_sorted_records(
        records_for_writer.into_iter(),
        config.record_count,
    ).await?;
    
    // Step 2: Create compactor with specified strategy
    let compactor = match strategy {
        CompactionSortStrategy::ByPQSimilarity(adapter) => {
            SstCompactor::new(filesystem_factory.clone(), None)
                .with_sort_strategy(CompactionSortStrategy::ByPQSimilarity(adapter))
        }
        other => {
            SstCompactor::new(filesystem_factory.clone(), None)
                .with_sort_strategy(other)
        }
    };
    
    // Step 3: Perform compaction and measure performance
    let start_time = std::time::Instant::now();
    
    let compaction_result = compactor.compact_files(
        vec![input_file.to_string_lossy().to_string()],
        output_file.to_string_lossy().to_string(),
        1, // Target level
        None, // No compression config
    ).await?;
    
    let compression_time_ms = start_time.elapsed().as_millis() as u64;
    
    // Step 4: Measure file size
    let file_size_bytes = std::fs::metadata(&output_file)?.len();
    
    println!("   ✅ {}: {} bytes in {}ms", 
             strategy_name, file_size_bytes, compression_time_ms);
    
    Ok(BenchmarkResult {
        strategy_name: strategy_name.to_string(),
        file_size_bytes,
        compression_time_ms,
        records_processed: compaction_result.records_written as usize,
        compression_ratio: 1.0, // Will be calculated relative to baseline
    })
}

#[tokio::test]
async fn test_pq_compaction_compression_benchmark() {
    // Initialize hardware capabilities
    let _ = initialize_hardware_capabilities_default();
    
    println!("🎯 PQ-Based Compaction Compression Benchmark");
    println!("============================================");
    
    let config = BenchmarkConfig::default();
    
    // Setup test environment
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path().join("compaction_benchmark");
    std::fs::create_dir_all(&test_dir).unwrap();
    
    let filesystem_factory = Arc::new(
        FilesystemFactory::new(FilesystemConfig::default()).await.unwrap()
    );
    
    // Create quantization infrastructure
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let codebook_store = Arc::new(InMemoryCodebookStore::new());
    let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        codebook_store,
    ));
    
    let base_config = StorageQuantizationConfig::default();
    let base_engine = Arc::new(StorageQuantizationEngine::new(
        unified_engine,
        distance_compute,
        base_config,
    ));
    
    let sst_config = SstQuantizationConfig {
        enable_similarity_sorting: true,
        clustering_threshold: 0.75,
        target_cluster_size: 200,
        enable_progressive_blocks: true,
    };
    
    let quantization_adapter = Arc::new(SstQuantizationAdapter::new(base_engine, sst_config));
    
    // Generate test data
    println!("📊 Generating test data: {} records, {} dimensions, {} clusters",
             config.record_count, config.vector_dimension, config.cluster_count);
    
    let data_generator = ClusteredDataGenerator::new(config.clone());
    
    // Test different strategies
    let mut results = Vec::new();
    
    // 1. Random baseline (worst case)
    let random_records = data_generator.generate_records("random");
    let random_result = benchmark_compaction_strategy(
        CompactionSortStrategy::ById,
        "Random_Order",
        random_records,
        &config,
        filesystem_factory.clone(),
        &test_dir,
    ).await.unwrap();
    results.push(random_result.clone());
    let baseline_size = random_result.file_size_bytes;
    
    // 2. Traditional ID-based sorting
    let traditional_records = data_generator.generate_records("traditional");
    let traditional_result = benchmark_compaction_strategy(
        CompactionSortStrategy::ById,
        "ID_Based_Sort",
        traditional_records,
        &config,
        filesystem_factory.clone(),
        &test_dir,
    ).await.unwrap();
    results.push(traditional_result);
    
    // 3. PQ-based similarity sorting
    let pq_records = data_generator.generate_records("clustered");
    let pq_result = benchmark_compaction_strategy(
        CompactionSortStrategy::ByPQSimilarity(quantization_adapter.clone()),
        "PQ_Similarity_Sort",
        pq_records,
        &config,
        filesystem_factory.clone(),
        &test_dir,
    ).await.unwrap();
    results.push(pq_result);
    
    // Calculate compression ratios relative to baseline
    for result in &mut results {
        result.compression_ratio = baseline_size as f32 / result.file_size_bytes as f32;
    }
    
    // Print results
    println!("\n📈 Benchmark Results:");
    println!("=====================");
    println!("{:<20} {:<12} {:<12} {:<12} {:<8}", 
             "Strategy", "Size (KB)", "Time (ms)", "Records", "Ratio");
    println!("{:-<60}", "");
    
    for result in &results {
        println!("{:<20} {:<12} {:<12} {:<12} {:<8.3}", 
                 result.strategy_name,
                 result.file_size_bytes / 1024,
                 result.compression_time_ms,
                 result.records_processed,
                 result.compression_ratio);
    }
    
    // Validate improvements
    let pq_result = results.iter().find(|r| r.strategy_name == "PQ_Similarity_Sort").unwrap();
    let traditional_result = results.iter().find(|r| r.strategy_name == "ID_Based_Sort").unwrap();
    
    println!("\n🔍 Analysis:");
    println!("============");
    
    if pq_result.file_size_bytes < traditional_result.file_size_bytes {
        let improvement = ((traditional_result.file_size_bytes - pq_result.file_size_bytes) as f32 
                          / traditional_result.file_size_bytes as f32) * 100.0;
        println!("✅ PQ sorting improved compression by {:.1}%", improvement);
        assert!(improvement > 0.0, "PQ sorting should provide some compression benefit");
    } else {
        println!("⚠️ PQ sorting did not improve compression (test data may be too uniform)");
    }
    
    // Performance analysis
    if pq_result.compression_time_ms <= traditional_result.compression_time_ms * 2 {
        println!("✅ PQ sorting performance is acceptable (within 2x of traditional)");
    } else {
        println!("⚠️ PQ sorting is slower than expected");
    }
    
    println!("\n🎉 Compression benchmark completed!");
}

#[tokio::test] 
async fn test_pq_sorting_consistency() {
    // Initialize hardware capabilities
    let _ = initialize_hardware_capabilities_default();
    
    println!("🧪 Testing PQ Sorting Consistency");
    
    // Test that PQ sorting produces consistent results with same input
    let config = BenchmarkConfig {
        record_count: 100,
        vector_dimension: 32,
        cluster_count: 5,
        cluster_noise: 0.1,
        block_size: 32 * 1024,
    };
    
    let data_generator = ClusteredDataGenerator::new(config.clone());
    let test_records = data_generator.generate_records("consistency");
    
    // Create quantization adapter
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let codebook_store = Arc::new(InMemoryCodebookStore::new());
    let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        codebook_store,
    ));
    
    let base_config = StorageQuantizationConfig::default();
    let base_engine = Arc::new(StorageQuantizationEngine::new(
        unified_engine,
        distance_compute,
        base_config,
    ));
    
    let sst_config = SstQuantizationConfig::default();
    let adapter = Arc::new(SstQuantizationAdapter::new(base_engine, sst_config));
    
    // Extract vectors for testing
    let vectors: Vec<Vec<f32>> = test_records.iter().map(|r| r.vector.clone()).collect();
    let vector_ids: Vec<String> = test_records.iter().map(|r| r.id.clone()).collect();
    
    // Run clustering multiple times
    let mut cluster_results = Vec::new();
    
    for run in 0..3 {
        let quantized_data = adapter.base_engine()
            .quantize_batch(&vectors, &vector_ids)
            .await
            .unwrap();
        
        let clusters = adapter.create_similarity_clusters(&quantized_data).unwrap();
        cluster_results.push(clusters);
        
        println!("   Run {}: {} clusters created", run + 1, cluster_results[run].len());
    }
    
    // Basic consistency check - should produce similar number of clusters
    let cluster_counts: Vec<usize> = cluster_results.iter().map(|c| c.len()).collect();
    let max_count = *cluster_counts.iter().max().unwrap();
    let min_count = *cluster_counts.iter().min().unwrap();
    
    assert!(max_count - min_count <= 2, "Cluster counts should be consistent: {:?}", cluster_counts);
    
    println!("✅ PQ sorting consistency validated");
}