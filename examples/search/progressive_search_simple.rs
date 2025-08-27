use anyhow::Result;
use proximadb::compute::quantization::storage_engine::{
    StorageQuantizationEngine, StorageQuantizationConfig,
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
    println!("🔍 PROGRESSIVE SEARCH PERFORMANCE TEST");
    println!("{}", "=".repeat(80));
    
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Test parameters
    let test_sizes = vec![
        (1000, 128, "Small (1K×128)"),
        (5000, 384, "Medium (5K×384)"),
        (10000, 768, "Large (10K×768)"),
    ];
    
    println!("\n📊 TEST CONFIGURATION:");
    for (count, dim, desc) in &test_sizes {
        println!("  • {}: {} vectors × {} dimensions", desc, count, dim);
    }
    
    for (num_vectors, dimension, description) in test_sizes {
        println!("\n{}", "-".repeat(80));
        println!("🔬 Testing: {}", description);
        println!("{}", "-".repeat(80));
        
        // Generate test vectors
        let vectors = generate_test_vectors(num_vectors, dimension);
        let query = vectors[0].clone();
        
        // Setup quantization with PQ + INT8 + Binary
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        let config = StorageQuantizationConfig {
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
        
        // Train quantization
        println!("\n⚙️  Training quantization models...");
        let train_start = Instant::now();
        engine.train(&vectors).await?;
        let train_time = train_start.elapsed();
        println!("  ✓ Training completed in {:.2}ms", train_time.as_millis());
        
        // Quantize vectors
        let quant_start = Instant::now();
        let quantized = engine.quantize_batch(&vectors, None).await?;
        let quant_time = quant_start.elapsed();
        println!("  ✓ Quantization completed in {:.2}ms", quant_time.as_millis());
        
        // Calculate data sizes
        let full_scan_bytes = num_vectors * dimension * 4; // FP32
        let mut quantized_bytes = 0usize;
        for q in &quantized {
            if let Some(ref p) = q.primary { quantized_bytes += p.data.len(); }
            if let Some(ref f) = q.fast { quantized_bytes += f.data.len(); }
            if let Some(ref b) = q.filter { quantized_bytes += b.data.len(); }
        }
        
        // Perform progressive search
        println!("\n🔍 Performing progressive search...");
        let search_start = Instant::now();
        let stages = engine.progressive_search(
            &query,
            &quantized,
            10,
            &DistanceMetric::Cosine,
        ).await?;
        let search_time = search_start.elapsed();
        
        // Print stage details
        println!("\n📈 Progressive Search Stages:");
        let mut total_reduction = 0.0;
        for (i, stage) in stages.iter().enumerate() {
            println!("  Stage {}: {:?}", i + 1, stage.stage);
            println!("    • Input: {} candidates", stage.metrics.input_count);
            println!("    • Output: {} candidates", stage.metrics.output_count);
            println!("    • Reduction: {:.1}%", stage.metrics.reduction_percent);
            println!("    • Time: {}μs", stage.metrics.time_us);
            total_reduction += stage.metrics.reduction_percent;
        }
        
        let final_candidates = stages.last()
            .map(|s| s.metrics.output_count)
            .unwrap_or(num_vectors);
        let io_reduction = 100.0 * (1.0 - (final_candidates as f64 / num_vectors as f64));
        
        // Print summary
        println!("\n📊 Performance Summary:");
        println!("  • Search time: {:.2}ms", search_time.as_micros() as f64 / 1000.0);
        println!("  • I/O reduction: {:.1}%", io_reduction);
        println!("  • Final candidates: {} / {} ({:.2}%)", 
            final_candidates, num_vectors, 
            100.0 * final_candidates as f64 / num_vectors as f64);
        println!("  • Data compression: {:.2}x ({} MB → {} MB)",
            full_scan_bytes as f64 / quantized_bytes as f64,
            full_scan_bytes as f64 / (1024.0 * 1024.0),
            quantized_bytes as f64 / (1024.0 * 1024.0));
    }
    
    println!("\n✅ PROGRESSIVE SEARCH TEST COMPLETE");
    println!("{}", "=".repeat(80));
    
    Ok(())
}

fn generate_test_vectors(count: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);
    
    for i in 0..count {
        let mut vector = vec![0.0f32; dim];
        
        // Create clusters for realistic data
        let cluster_id = i / 100;
        let base_value = (cluster_id as f32) * 0.1;
        
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
        
        vectors.push(vector);
    }
    
    vectors
}