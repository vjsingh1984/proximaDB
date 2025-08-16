use anyhow::Result;
use proximadb::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig, SearchStage,
};
use proximadb::compute::quantization::unified::{
    UnifiedQuantizationLevel, QuantizationLevelType, ProductQuantization as PqConfig,
    UnifiedQuantizationEngine, InMemoryCodebookStore,
};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::compute::distance_computation::DistanceMetric;
use std::sync::Arc;
use std::time::Instant;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "=".repeat(80));
    println!("📊 I/O REDUCTION BENCHMARK");
    println!("{}", "=".repeat(80));
    
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Test parameters
    let vector_counts = vec![1000, 5000, 10000, 50000];
    let dimensions = vec![128, 384, 768, 1536]; // Various model dimensions
    let k = 10;
    
    println!("\n📊 BENCHMARK CONFIGURATION:");
    println!("  • Vector counts: {:?}", vector_counts);
    println!("  • Dimensions: {:?}", dimensions);
    println!("  • Top-K: {}", k);
    
    // Store all results
    let mut all_results = Vec::new();
    
    for num_vectors in &vector_counts {
        for dimension in &dimensions {
            println!("\n{}", "-".repeat(80));
            println!("🔬 Testing {} vectors × {} dimensions", num_vectors, dimension);
            
            // Generate test vectors
            let vectors = generate_test_vectors(*num_vectors, *dimension);
            let query = vectors[0].clone();
            
            // Test different quantization configurations
            let configs = vec![
                ("No Quantization", None, None, None),
                ("Binary Only", None, None, Some(UnifiedQuantizationLevel::binary())),
                ("INT8 + Binary", None, Some(UnifiedQuantizationLevel::int8()), Some(UnifiedQuantizationLevel::binary())),
                ("PQ8 + INT8 + Binary", 
                    Some(UnifiedQuantizationLevel {
                        level_type: Some(QuantizationLevelType::Pq(PqConfig {
                            num_subvectors: 8,
                            bits_per_code: 8,
                            codebook_id: None,
                            adaptive_subvectors: false,
                        })),
                    }),
                    Some(UnifiedQuantizationLevel::int8()),
                    Some(UnifiedQuantizationLevel::binary())
                ),
                ("PQ4 + Binary",
                    Some(UnifiedQuantizationLevel {
                        level_type: Some(QuantizationLevelType::Pq(PqConfig {
                            num_subvectors: 16,
                            bits_per_code: 4,
                            codebook_id: None,
                            adaptive_subvectors: false,
                        })),
                    }),
                    None,
                    Some(UnifiedQuantizationLevel::binary())
                ),
            ];
            
            for (config_name, primary, fast, filter) in configs {
                let result = benchmark_configuration(
                    config_name,
                    *num_vectors,
                    *dimension,
                    &vectors,
                    &query,
                    k,
                    primary,
                    fast,
                    filter,
                ).await?;
                
                all_results.push(result);
            }
        }
    }
    
    // Print comprehensive results
    print_benchmark_results(&all_results);
    
    Ok(())
}

async fn benchmark_configuration(
    config_name: &str,
    num_vectors: usize,
    dimension: usize,
    vectors: &[Vec<f32>],
    query: &[f32],
    k: usize,
    primary_level: Option<UnifiedQuantizationLevel>,
    fast_level: Option<UnifiedQuantizationLevel>,
    filter_level: Option<UnifiedQuantizationLevel>,
) -> Result<BenchmarkResult> {
    // Setup quantization
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let codebook_store = Arc::new(InMemoryCodebookStore::new());
    let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        codebook_store,
    ));
    
    let config = StorageQuantizationConfig {
        primary_level: primary_level.clone(),
        fast_level: fast_level.clone(),
        filter_level: filter_level.clone(),
        candidate_multiplier: 5,
        training_sample_size: 1000.min(num_vectors),
        enable_progressive: true,
        enable_hardware_acceleration: true,
        filter_threshold: 0.5,
        quality_threshold: 0.95,
        memory_budget_mb: 512,
    };
    
    let mut engine = StorageQuantizationEngine::new(
        unified_engine,
        distance_compute.clone(),
        config,
    );
    
    // Train if needed
    let train_start = Instant::now();
    if primary_level.is_some() || fast_level.is_some() {
        engine.train(vectors).await?;
    }
    let train_time = train_start.elapsed();
    
    // Quantize
    let quant_start = Instant::now();
    let quantized = engine.quantize_batch(vectors, None).await?;
    let quant_time = quant_start.elapsed();
    
    // Measure bytes read in full scan vs progressive
    let full_scan_bytes = num_vectors * dimension * 4; // All FP32 vectors
    
    // Perform progressive search
    let search_start = Instant::now();
    let stages = engine.progressive_search(
        query,
        &quantized,
        k,
        &DistanceMetric::Cosine,
    ).await?;
    let search_time = search_start.elapsed();
    
    // Calculate progressive bytes read
    let mut progressive_bytes = 0usize;
    let mut candidates_at_each_stage = Vec::new();
    
    for stage in &stages {
        candidates_at_each_stage.push(stage.metrics.output_count);
        
        match stage.stage {
            SearchStage::BinaryFilter => {
                // Binary: 1 bit per dimension
                progressive_bytes += stage.metrics.input_count * dimension / 8;
            }
            SearchStage::FastApproximation => {
                // INT8: 1 byte per dimension
                progressive_bytes += stage.metrics.input_count * dimension;
            }
            SearchStage::PQRanking => {
                // PQ: depends on configuration
                if let Some(ref primary) = primary_level {
                    if let Some(QuantizationLevelType::Pq(ref pq)) = primary.level_type {
                        progressive_bytes += stage.metrics.input_count * pq.num_subvectors as usize;
                    }
                }
            }
            SearchStage::FullPrecision => {
                // Full precision: 4 bytes per dimension
                progressive_bytes += stage.metrics.input_count * dimension * 4;
            }
        }
    }
    
    let io_reduction = 100.0 * (1.0 - (progressive_bytes as f64 / full_scan_bytes as f64));
    let speedup = full_scan_bytes as f64 / progressive_bytes.max(1) as f64;
    
    println!("  • {}: {:.1}% I/O reduction, {:.2}x speedup", 
        config_name, io_reduction, speedup);
    
    Ok(BenchmarkResult {
        config_name: config_name.to_string(),
        num_vectors,
        dimension,
        train_time,
        quant_time,
        search_time,
        full_scan_bytes,
        progressive_bytes,
        io_reduction,
        speedup,
        num_stages: stages.len(),
        final_candidates: stages.last().map(|s| s.metrics.output_count).unwrap_or(num_vectors),
    })
}

fn generate_test_vectors(count: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);
    
    for i in 0..count {
        let mut vector = vec![0.0f32; dim];
        
        // Create different distribution patterns
        let pattern = i % 5;
        match pattern {
            0 => {
                // Dense random
                for j in 0..dim {
                    vector[j] = rng.gen_range(-1.0..1.0);
                }
            }
            1 => {
                // Sparse (90% zeros)
                for j in 0..dim/10 {
                    let idx = rng.gen_range(0..dim);
                    vector[idx] = rng.gen_range(-1.0..1.0);
                }
            }
            2 => {
                // Clustered
                let cluster_center = (i / 100) as f32 * 0.1;
                for j in 0..dim {
                    vector[j] = cluster_center + rng.gen_range(-0.1..0.1);
                }
            }
            3 => {
                // Binary-like
                for j in 0..dim {
                    vector[j] = if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
                }
            }
            _ => {
                // Mixed
                for j in 0..dim {
                    if j % 2 == 0 {
                        vector[j] = rng.gen_range(-1.0..1.0);
                    }
                }
            }
        }
        
        // Normalize
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut vector {
                *val /= norm;
            }
        }
        
        vectors.push(vector);
    }
    
    vectors
}

fn print_benchmark_results(results: &[BenchmarkResult]) {
    println!("\n{}", "=".repeat(80));
    println!("📊 I/O REDUCTION BENCHMARK RESULTS");
    println!("{}", "=".repeat(80));
    
    // Group by configuration
    let mut by_config: std::collections::HashMap<String, Vec<&BenchmarkResult>> = 
        std::collections::HashMap::new();
    
    for result in results {
        by_config.entry(result.config_name.clone())
            .or_insert_with(Vec::new)
            .push(result);
    }
    
    // Print average metrics per configuration
    println!("\n📈 AVERAGE I/O REDUCTION BY CONFIGURATION:");
    println!("{:<25} {:>15} {:>15} {:>15}", 
        "Configuration", "Avg I/O Reduction", "Avg Speedup", "Avg Search Time");
    println!("{}", "-".repeat(70));
    
    for (config, results) in by_config.iter() {
        let avg_io = results.iter().map(|r| r.io_reduction).sum::<f64>() / results.len() as f64;
        let avg_speedup = results.iter().map(|r| r.speedup).sum::<f64>() / results.len() as f64;
        let avg_search = results.iter()
            .map(|r| r.search_time.as_micros() as f64)
            .sum::<f64>() / results.len() as f64 / 1000.0;
        
        println!("{:<25} {:>14.1}% {:>14.2}x {:>14.2}ms",
            config, avg_io, avg_speedup, avg_search);
    }
    
    // Print by scale
    println!("\n📊 I/O REDUCTION BY SCALE (PQ8 + INT8 + Binary):");
    println!("{:<15} {:>15} {:>15} {:>15} {:>15}", 
        "Vectors", "128-dim", "384-dim", "768-dim", "1536-dim");
    println!("{}", "-".repeat(75));
    
    if let Some(pq8_results) = by_config.get("PQ8 + INT8 + Binary") {
        for count in &[1000, 5000, 10000, 50000] {
            print!("{:<15}", format!("{}K", count / 1000));
            for dim in &[128, 384, 768, 1536] {
                if let Some(result) = pq8_results.iter()
                    .find(|r| r.num_vectors == *count && r.dimension == *dim) {
                    print!(" {:>14.1}%", result.io_reduction);
                } else {
                    print!(" {:>15}", "-");
                }
            }
            println!();
        }
    }
    
    // Find best configurations
    let best_io = results.iter()
        .max_by(|a, b| a.io_reduction.partial_cmp(&b.io_reduction).unwrap())
        .unwrap();
    
    let best_speedup = results.iter()
        .max_by(|a, b| a.speedup.partial_cmp(&b.speedup).unwrap())
        .unwrap();
    
    println!("\n🏆 BEST RESULTS:");
    println!("  • Best I/O reduction: {:.1}% ({} on {}×{} vectors)",
        best_io.io_reduction, best_io.config_name, best_io.num_vectors, best_io.dimension);
    println!("  • Best speedup: {:.2}x ({} on {}×{} vectors)",
        best_speedup.speedup, best_speedup.config_name, 
        best_speedup.num_vectors, best_speedup.dimension);
    
    // Calculate data read savings
    let total_full_scan: usize = results.iter().map(|r| r.full_scan_bytes).sum();
    let total_progressive: usize = results.iter().map(|r| r.progressive_bytes).sum();
    let total_saved = total_full_scan - total_progressive;
    
    println!("\n💾 TOTAL DATA SAVINGS:");
    println!("  • Full scan would read: {:.2} GB", total_full_scan as f64 / (1024.0 * 1024.0 * 1024.0));
    println!("  • Progressive search reads: {:.2} GB", total_progressive as f64 / (1024.0 * 1024.0 * 1024.0));
    println!("  • Data saved: {:.2} GB ({:.1}% reduction)", 
        total_saved as f64 / (1024.0 * 1024.0 * 1024.0),
        100.0 * total_saved as f64 / total_full_scan as f64);
    
    println!("\n✅ I/O REDUCTION BENCHMARK COMPLETE");
    println!("{}", "=".repeat(80));
}

#[derive(Debug, Clone)]
struct BenchmarkResult {
    config_name: String,
    num_vectors: usize,
    dimension: usize,
    train_time: std::time::Duration,
    quant_time: std::time::Duration,
    search_time: std::time::Duration,
    full_scan_bytes: usize,
    progressive_bytes: usize,
    io_reduction: f64,
    speedup: f64,
    num_stages: usize,
    final_candidates: usize,
}