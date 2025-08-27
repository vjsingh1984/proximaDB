use anyhow::Result;
use proximadb::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig,
};
use proximadb::compute::quantization::unified::{
    UnifiedQuantizationLevel, QuantizationLevelType,
    ProductQuantization as PqConfig, UnifiedQuantizationEngine,
    InMemoryCodebookStore,
};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use std::sync::Arc;
use std::time::Instant;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "=".repeat(80));
    println!("🎯 QUANTIZATION SUMMARY STATISTICS");
    println!("{}", "=".repeat(80));
    
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Test parameters
    let num_vectors = 100;
    let dimension = 128;
    let sparsity_levels = vec![0, 30, 50, 70, 90];
    
    println!("\n📊 TEST CONFIGURATION:");
    println!("  • Vectors: {}", num_vectors);
    println!("  • Dimension: {}", dimension);
    println!("  • Sparsity levels: {:?}%", sparsity_levels);
    
    // Test different quantization types
    let configs = vec![
        ("No Quantization", None, None),
        ("Binary Quantization", Some(UnifiedQuantizationLevel::binary()), None),
        ("INT8 Quantization", None, Some(UnifiedQuantizationLevel::int8())),
        ("PQ8 Quantization", Some(UnifiedQuantizationLevel {
            level_type: Some(QuantizationLevelType::Pq(PqConfig {
                num_subvectors: 4,
                bits_per_code: 8,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }), None),
    ];
    
    // Store results
    let mut all_results = Vec::new();
    
    for (config_name, primary_level, fast_level) in configs {
        println!("\n{}", "-".repeat(80));
        println!("🔧 Testing: {}", config_name);
        
        for sparsity in &sparsity_levels {
            let vectors = generate_sparse_vectors(num_vectors, dimension, *sparsity);
            let result = run_quantization_test(
                config_name,
                *sparsity,
                &vectors,
                primary_level.clone(),
                fast_level.clone(),
            ).await?;
            all_results.push(result);
        }
    }
    
    // Print summary
    print_summary(&all_results);
    
    Ok(())
}

async fn run_quantization_test(
    config_name: &str,
    sparsity: usize,
    vectors: &[Vec<f32>],
    primary_level: Option<UnifiedQuantizationLevel>,
    fast_level: Option<UnifiedQuantizationLevel>,
) -> Result<TestResult> {
    let start = Instant::now();
    
    // Calculate baseline size
    let baseline_size = vectors.len() * vectors[0].len() * 4; // FP32
    
    // Setup quantization
    let (quantized_size, quant_time) = if primary_level.is_some() || fast_level.is_some() {
        let quant_start = Instant::now();
        
        // Create engines
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        let config = StorageQuantizationConfig {
            primary_level: primary_level.clone(),
            fast_level: fast_level.clone(),
            filter_level: Some(UnifiedQuantizationLevel::binary()),
            candidate_multiplier: 5,
            training_sample_size: 100,
            enable_progressive: true,
            enable_hardware_acceleration: true,
            filter_threshold: 0.5,
            quality_threshold: 0.95,
            memory_budget_mb: 512,
        };
        
        let mut engine = StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            config,
        );
        
        // Train and quantize
        engine.train(vectors).await?;
        let quantized = engine.quantize_batch(vectors, None).await?;
        
        // Calculate size
        let mut total_size = 0usize;
        for q in &quantized {
            if let Some(ref p) = q.primary { total_size += p.data.len(); }
            if let Some(ref f) = q.fast { total_size += f.data.len(); }
            if let Some(ref b) = q.filter { total_size += b.data.len(); }
        }
        
        (total_size, quant_start.elapsed())
    } else {
        (baseline_size, std::time::Duration::ZERO)
    };
    
    let compression_ratio = baseline_size as f64 / quantized_size.max(1) as f64;
    let space_savings = if quantized_size < baseline_size {
        ((baseline_size - quantized_size) as f64 / baseline_size as f64) * 100.0
    } else {
        0.0
    };
    
    println!("  • Sparsity {}%: ratio={:.2}x, savings={:.1}%, time={:.0}ms",
        sparsity, compression_ratio, space_savings, quant_time.as_millis());
    
    Ok(TestResult {
        config_name: config_name.to_string(),
        sparsity,
        compression_ratio,
        space_savings,
        quantization_time: quant_time,
    })
}

fn generate_sparse_vectors(count: usize, dim: usize, sparsity_percent: usize) -> Vec<Vec<f32>> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);
    
    for _ in 0..count {
        let mut vector = vec![0.0f32; dim];
        let non_zero_count = dim * (100 - sparsity_percent) / 100;
        
        for _ in 0..non_zero_count {
            let idx = rng.gen_range(0..dim);
            vector[idx] = rng.gen_range(-1.0..1.0);
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

fn print_summary(results: &[TestResult]) {
    println!("\n{}", "=".repeat(80));
    println!("📊 COMPRESSION SUMMARY");
    println!("{}", "=".repeat(80));
    
    // Group by config
    let mut by_config: std::collections::HashMap<String, Vec<&TestResult>> = std::collections::HashMap::new();
    for result in results {
        by_config.entry(result.config_name.clone())
            .or_insert_with(Vec::new)
            .push(result);
    }
    
    println!("\n📈 COMPRESSION RATIOS (Higher is Better):");
    println!("{:<25} {:>10} {:>10} {:>10} {:>10} {:>10}", 
        "Configuration", "0%", "30%", "50%", "70%", "90%");
    println!("{}", "-".repeat(85));
    
    for (config, results) in by_config.iter() {
        print!("{:<25}", config);
        for sparsity in &[0, 30, 50, 70, 90] {
            if let Some(r) = results.iter().find(|r| r.sparsity == *sparsity) {
                print!(" {:>9.2}x", r.compression_ratio);
            } else {
                print!(" {:>10}", "-");
            }
        }
        println!();
    }
    
    println!("\n💾 SPACE SAVINGS (%):");
    println!("{:<25} {:>10} {:>10} {:>10} {:>10} {:>10}", 
        "Configuration", "0%", "30%", "50%", "70%", "90%");
    println!("{}", "-".repeat(85));
    
    for (config, results) in by_config.iter() {
        print!("{:<25}", config);
        for sparsity in &[0, 30, 50, 70, 90] {
            if let Some(r) = results.iter().find(|r| r.sparsity == *sparsity) {
                print!(" {:>9.1}%", r.space_savings);
            } else {
                print!(" {:>10}", "-");
            }
        }
        println!();
    }
    
    // Calculate averages
    println!("\n📊 AVERAGE METRICS:");
    println!("{}", "-".repeat(85));
    
    for (config, results) in by_config.iter() {
        let avg_ratio = results.iter().map(|r| r.compression_ratio).sum::<f64>() / results.len() as f64;
        let avg_savings = results.iter().map(|r| r.space_savings).sum::<f64>() / results.len() as f64;
        
        println!("{:<25} Compression: {:.2}x | Savings: {:.1}%",
            config, avg_ratio, avg_savings);
    }
    
    println!("\n✅ QUANTIZATION SUMMARY COMPLETE");
    println!("{}", "=".repeat(80));
}

#[derive(Debug, Clone)]
struct TestResult {
    config_name: String,
    sparsity: usize,
    compression_ratio: f64,
    space_savings: f64,
    quantization_time: std::time::Duration,
}