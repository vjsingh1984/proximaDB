use anyhow::Result;
use proximadb::core::VectorRecord;
use proximadb::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig, SearchStage,
};
use proximadb::compute::quantization::unified::{
    UnifiedQuantizationLevel, QuantizationLevelType, ProductQuantization as PqConfig,
    UnifiedQuantizationEngine, InMemoryCodebookStore,
};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::storage::engines::sst::{SstEngine, SstConfig};
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "=".repeat(80));
    println!("🔍 PROGRESSIVE SEARCH WITH 256KB BLOCKS");
    println!("{}", "=".repeat(80));
    
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Test parameters
    let num_vectors = 5000;
    let dimension = 768; // BERT-like dimensions
    let query_k = 10;
    let block_sizes = vec![256, 512, 1024, 2048]; // KB
    
    println!("\n📊 TEST CONFIGURATION:");
    println!("  • Vectors: {}", num_vectors);
    println!("  • Dimension: {}", dimension);
    println!("  • Top-K: {}", query_k);
    println!("  • Block sizes: {:?} KB", block_sizes);
    
    // Generate test vectors
    let vectors = generate_test_vectors(num_vectors, dimension);
    let query = vectors[0].vector.clone();
    
    // Setup quantization
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let codebook_store = Arc::new(InMemoryCodebookStore::new());
    let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        codebook_store,
    ));
    
    let quant_config = StorageQuantizationConfig {
        primary_level: Some(UnifiedQuantizationLevel {
            level_type: Some(QuantizationLevelType::Pq(PqConfig {
                num_subvectors: 8,
                bits_per_code: 8,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }),
        fast_level: Some(UnifiedQuantizationLevel::int8()),
        filter_level: Some(UnifiedQuantizationLevel::binary()),
        candidate_multiplier: 5,
        training_sample_size: 1000,
        enable_progressive: true,
        enable_hardware_acceleration: true,
        filter_threshold: 0.5,
        quality_threshold: 0.95,
        memory_budget_mb: 512,
    };
    
    let mut quantization_engine = StorageQuantizationEngine::new(
        unified_engine,
        distance_compute.clone(),
        quant_config,
    );
    
    // Train quantization
    println!("\n🎯 Training quantization models...");
    let vector_data: Vec<Vec<f32>> = vectors.iter()
        .map(|v| v.vector.clone())
        .collect();
    quantization_engine.train(&vector_data).await?;
    
    // Quantize vectors
    let quantized = quantization_engine.quantize_batch(&vector_data, None).await?;
    
    // Test each block size
    let mut results = Vec::new();
    
    for block_size_kb in &block_sizes {
        println!("\n{}", "-".repeat(80));
        println!("📦 Testing with {}KB blocks", block_size_kb);
        
        // Create temporary directory
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap();
        
        // Setup SST engine with specific block size
        let mut sst_config = SstConfig::default();
        sst_config.block_size_kb = *block_size_kb;
        sst_config.storage_config.as_ref().and_then(|s| s.compression.as_ref()) = "zstd".to_string();
        sst_config.compression_level = 3;
        
        let filesystem = FilesystemFactory::create_local(base_path);
        let mut sst_engine = SstEngine::new(
            "test_collection".to_string(),
            Arc::new(filesystem),
            sst_config.clone(),
            None,
            None,
        );
        
        // Insert and flush vectors
        let flush_start = Instant::now();
        sst_engine.insert_batch(vectors.clone()).await?;
        let flush_params = FlushParameters {
            force: false,
            collection_id: Some("test_collection".to_string()),
            collection_config: None,
        };
        let flush_result = sst_engine.flush(flush_params).await?;
        let flush_time = flush_start.elapsed();
        
        println!("  • Flush time: {:.2}ms", flush_time.as_millis());
        println!("  • Entries flushed: {}", flush_result.entries_flushed);
        println!("  • Files created: {}", flush_result.files_created);
        
        // Perform progressive search
        let search_start = Instant::now();
        let stages = quantization_engine.progressive_search(
            &query,
            &quantized,
            query_k,
            &DistanceMetric::Cosine,
        ).await?;
        let search_time = search_start.elapsed();
        
        // Calculate metrics
        let mut total_candidates = num_vectors;
        let mut cumulative_reduction = 0.0;
        
        println!("\n  📈 Progressive Search Stages:");
        for (i, stage) in stages.iter().enumerate() {
            let reduction = stage.metrics.reduction_percent;
            cumulative_reduction += reduction;
            
            println!("    Stage {}: {:?}", i + 1, stage.stage);
            println!("      • Input: {} candidates", stage.metrics.input_count);
            println!("      • Output: {} candidates", stage.metrics.output_count);
            println!("      • Reduction: {:.1}%", reduction);
            println!("      • Time: {}μs", stage.metrics.time_us);
            
            total_candidates = stage.metrics.output_count;
        }
        
        let io_reduction = 100.0 * (1.0 - (total_candidates as f64 / num_vectors as f64));
        
        println!("\n  📊 Summary for {}KB blocks:", block_size_kb);
        println!("    • Total search time: {:.2}ms", search_time.as_micros() as f64 / 1000.0);
        println!("    • I/O reduction: {:.1}%", io_reduction);
        println!("    • Final candidates: {} / {}", total_candidates, num_vectors);
        println!("    • Average reduction per stage: {:.1}%", 
            cumulative_reduction / stages.len() as f32);
        
        results.push(BlockSizeResult {
            block_size_kb: *block_size_kb,
            flush_time,
            search_time,
            io_reduction,
            num_stages: stages.len(),
            final_candidates: total_candidates,
        });
    }
    
    // Print comparison
    print_comparison(&results);
    
    Ok(())
}

fn generate_test_vectors(count: usize, dim: usize) -> Vec<VectorRecord> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);
    
    for i in 0..count {
        let mut vector = vec![0.0f32; dim];
        
        // Create clusters of similar vectors
        let cluster_id = i / 100;
        let base_value = cluster_id as f32 * 0.1;
        
        for j in 0..dim {
            vector[j] = base_value + rng.gen_range(-0.5..0.5);
        }
        
        // Normalize
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut vector {
                *val /= norm;
            }
        }
        
        vectors.push(VectorRecord {
            id: format!("vec_{}", i),
            vector,
            metadata: None,
            timestamp: None,
            collection_id: "test_collection".to_string(),
        });
    }
    
    vectors
}

fn print_comparison(results: &[BlockSizeResult]) {
    println!("\n{}", "=".repeat(80));
    println!("📊 BLOCK SIZE COMPARISON");
    println!("{}", "=".repeat(80));
    
    println!("\n📈 PERFORMANCE METRICS:");
    println!("{:<15} {:>15} {:>15} {:>15} {:>15}", 
        "Block Size", "Flush Time", "Search Time", "I/O Reduction", "Final Candidates");
    println!("{}", "-".repeat(75));
    
    for result in results {
        println!("{:<15} {:>14.2}ms {:>14.2}ms {:>14.1}% {:>15}",
            format!("{}KB", result.block_size_kb),
            result.flush_time.as_millis(),
            result.search_time.as_micros() as f64 / 1000.0,
            result.io_reduction,
            result.final_candidates,
        );
    }
    
    // Find best configuration
    let best_io = results.iter()
        .max_by(|a, b| a.io_reduction.partial_cmp(&b.io_reduction).unwrap())
        .unwrap();
    let fastest = results.iter()
        .min_by(|a, b| a.search_time.cmp(&b.search_time))
        .unwrap();
    
    println!("\n🏆 BEST CONFIGURATIONS:");
    println!("  • Best I/O reduction: {}KB blocks ({:.1}% reduction)",
        best_io.block_size_kb, best_io.io_reduction);
    println!("  • Fastest search: {}KB blocks ({:.2}ms)",
        fastest.block_size_kb, fastest.search_time.as_micros() as f64 / 1000.0);
    
    // Calculate 256KB vs 2048KB comparison
    if let (Some(kb_256), Some(kb_2048)) = 
        (results.iter().find(|r| r.block_size_kb == 256),
         results.iter().find(|r| r.block_size_kb == 2048)) {
        
        let io_improvement = kb_256.io_reduction - kb_2048.io_reduction;
        let search_diff = (kb_256.search_time.as_micros() as f64 - 
                          kb_2048.search_time.as_micros() as f64) / 1000.0;
        
        println!("\n📊 256KB vs 2048KB COMPARISON:");
        println!("  • I/O reduction difference: {:.1}%", io_improvement);
        println!("  • Search time difference: {:.2}ms", search_diff);
        
        if io_improvement > 0.0 {
            println!("  ✅ 256KB blocks provide {:.1}% better I/O reduction", io_improvement);
        }
    }
    
    println!("\n✅ PROGRESSIVE SEARCH TEST COMPLETE");
    println!("{}", "=".repeat(80));
}

#[derive(Debug, Clone)]
struct BlockSizeResult {
    block_size_kb: u32,
    flush_time: std::time::Duration,
    search_time: std::time::Duration,
    io_reduction: f64,
    num_stages: usize,
    final_candidates: usize,
}