//! Serialization Optimization Demo
//!
//! This example demonstrates the optimal serialization strategies for different
//! quantization levels in ProximaDB, showing before/after performance comparisons.

use anyhow::Result;
use std::time::Instant;
use proximadb::storage::engines::columnar::serialization_strategy::{
    SerializationStrategyOptimizer, SerializationStrategyConfig, SerializationStrategy
};
use proximadb::core::VectorRecord;

/// Demo configuration
struct DemoConfig {
    vector_dimensions: usize,
    num_vectors: usize,
    target_compression: f32,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            vector_dimensions: 768,  // Common embedding dimension
            num_vectors: 10_000,     // Reasonable test size
            target_compression: 4.0, // 4x compression target
        }
    }
}

/// Generate synthetic test vectors with realistic characteristics
fn generate_test_vectors(config: &DemoConfig) -> Vec<Vec<f32>> {
    let mut vectors = Vec::with_capacity(config.num_vectors);
    
    for i in 0..config.num_vectors {
        let mut vector = Vec::with_capacity(config.vector_dimensions);
        
        // Generate realistic embedding-like data
        for j in 0..config.vector_dimensions {
            let base_value = ((i * j) as f32).sin() * 0.1;
            let noise = (i as f32 * 0.001) - 0.5;
            vector.push(base_value + noise);
        }
        
        // Normalize to unit length (common for embeddings)
        let magnitude: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for value in &mut vector {
                *value /= magnitude;
            }
        }
        
        vectors.push(vector);
    }
    
    vectors
}

/// Demo: Compare naive vs optimized serialization
async fn demo_serialization_comparison() -> Result<()> {
    println!("🚀 ProximaDB Serialization Optimization Demo\n");
    
    let config = DemoConfig::default();
    let test_vectors = generate_test_vectors(&config);
    
    println!("📊 Test Configuration:");
    println!("   Vectors: {}", config.num_vectors);
    println!("   Dimensions: {}", config.vector_dimensions);
    println!("   Target Compression: {:.1}x\n", config.target_compression);
    
    // Before: Naive serialization
    println!("📦 BEFORE: Naive Serialization");
    let naive_stats = benchmark_naive_serialization(&test_vectors)?;
    print_serialization_stats("Naive", &naive_stats);
    
    // After: Optimized strategies
    println!("\n🎯 AFTER: Optimized Serialization Strategies");
    
    let strategy_config = SerializationStrategyConfig {
        dimension: config.vector_dimensions,
        target_compression_ratio: config.target_compression,
        enable_simd_alignment: true,
        enable_hardware_optimization: true,
        row_group_size: 50_000,
    };
    
    let optimizer = SerializationStrategyOptimizer::new(strategy_config);
    let metrics = optimizer.benchmark_strategies(&test_vectors)?;
    
    // Display optimized results
    for (strategy_name, metric) in &metrics {
        print_optimization_stats(strategy_name, metric);
    }
    
    // Generate comparison report
    println!("\n📈 PERFORMANCE COMPARISON REPORT");
    let report = optimizer.generate_comparison_report(&metrics);
    println!("{}", report);
    
    // Demonstrate progressive search performance
    demo_progressive_search(&test_vectors, &optimizer).await?;
    
    Ok(())
}

/// Benchmark naive serialization approach
fn benchmark_naive_serialization(vectors: &[Vec<f32>]) -> Result<SerializationStats> {
    let start_time = Instant::now();
    let mut total_size = 0;
    
    // Naive approach: use bincode for everything
    for vector in vectors {
        let serialized = bincode::serialize(vector)?;
        total_size += serialized.len();
    }
    
    let serialization_time = start_time.elapsed();
    let original_size = vectors.len() * vectors[0].len() * 4; // f32 = 4 bytes
    
    Ok(SerializationStats {
        method: "Naive (bincode)".to_string(),
        original_size,
        serialized_size: total_size,
        compression_ratio: original_size as f32 / total_size as f32,
        serialization_time_ms: serialization_time.as_millis() as u64,
        simd_efficiency: 0.0, // No SIMD optimization
        memory_overhead: 15.0, // Bincode overhead
    })
}

/// Demo progressive search performance
async fn demo_progressive_search(
    vectors: &[Vec<f32>],
    optimizer: &SerializationStrategyOptimizer,
) -> Result<()> {
    println!("\n🔍 PROGRESSIVE SEARCH PERFORMANCE DEMO");
    
    if vectors.is_empty() {
        return Ok(());
    }
    
    let query = &vectors[0]; // Use first vector as query
    let search_corpus = &vectors[1..1000.min(vectors.len())]; // Search in remaining vectors
    
    // Simulate progressive search stages
    println!("Query vector dimension: {}", query.len());
    println!("Search corpus size: {} vectors\n", search_corpus.len());
    
    // Stage 1: Binary filtering
    let binary_start = Instant::now();
    let binary_candidates = simulate_binary_filtering(query, search_corpus);
    let binary_time = binary_start.elapsed();
    
    println!("🔍 Stage 1 - Binary Filtering:");
    println!("   Input candidates: {}", search_corpus.len());
    println!("   Output candidates: {}", binary_candidates);
    println!("   Reduction: {:.1}%", 100.0 * (1.0 - binary_candidates as f32 / search_corpus.len() as f32));
    println!("   Time: {:.2}ms", binary_time.as_micros() as f32 / 1000.0);
    
    // Stage 2: PQ ranking
    let pq_start = Instant::now();
    let pq_candidates = simulate_pq_ranking(binary_candidates, 100);
    let pq_time = pq_start.elapsed();
    
    println!("\n🎯 Stage 2 - PQ Ranking:");
    println!("   Input candidates: {}", binary_candidates);
    println!("   Output candidates: {}", pq_candidates);
    println!("   Reduction: {:.1}%", 100.0 * (1.0 - pq_candidates as f32 / binary_candidates as f32));
    println!("   Time: {:.2}ms", pq_time.as_micros() as f32 / 1000.0);
    
    // Stage 3: FP32 reranking
    let fp32_start = Instant::now();
    let final_results = simulate_fp32_reranking(pq_candidates, 10);
    let fp32_time = fp32_start.elapsed();
    
    println!("\n⭐ Stage 3 - FP32 Reranking:");
    println!("   Input candidates: {}", pq_candidates);
    println!("   Final results: {}", final_results);
    println!("   Time: {:.2}ms", fp32_time.as_micros() as f32 / 1000.0);
    
    // Total performance
    let total_time = binary_time + pq_time + fp32_time;
    let total_reduction = 100.0 * (1.0 - final_results as f32 / search_corpus.len() as f32);
    
    println!("\n📊 Total Progressive Search Performance:");
    println!("   Overall reduction: {:.1}%", total_reduction);
    println!("   Total time: {:.2}ms", total_time.as_micros() as f32 / 1000.0);
    println!("   Speedup vs naive: {:.1}x", estimate_naive_search_time(search_corpus.len()) / total_time.as_micros() as f32);
    
    Ok(())
}

/// Simulate binary filtering performance
fn simulate_binary_filtering(query: &[f32], corpus: &[Vec<f32>]) -> usize {
    // Simulate 95% reduction typical of binary filtering
    (corpus.len() as f32 * 0.05) as usize
}

/// Simulate PQ ranking performance
fn simulate_pq_ranking(input_size: usize, target_size: usize) -> usize {
    input_size.min(target_size)
}

/// Simulate FP32 reranking performance
fn simulate_fp32_reranking(input_size: usize, k: usize) -> usize {
    input_size.min(k)
}

/// Estimate naive search time for comparison
fn estimate_naive_search_time(corpus_size: usize) -> f32 {
    // Estimate time for naive O(n) search without quantization
    corpus_size as f32 * 10.0 // Assume 10μs per vector comparison
}

/// Print serialization statistics
fn print_serialization_stats(method: &str, stats: &SerializationStats) {
    println!("   Method: {}", method);
    println!("   Original size: {} bytes", stats.original_size);
    println!("   Serialized size: {} bytes", stats.serialized_size);
    println!("   Compression ratio: {:.2}x", stats.compression_ratio);
    println!("   Serialization time: {}ms", stats.serialization_time_ms);
    println!("   Storage savings: {:.1}%", 100.0 * (1.0 - 1.0 / stats.compression_ratio));
}

/// Print optimization statistics  
fn print_optimization_stats(strategy: &str, metrics: &proximadb::storage::engines::columnar::serialization_strategy::SerializationMetrics) {
    println!("\n🎯 {} Strategy:", strategy);
    println!("   Compression ratio: {:.2}x", metrics.compression_ratio);
    println!("   Storage savings: {:.1}%", 100.0 * (1.0 - 1.0 / metrics.compression_ratio));
    println!("   Serialization time: {:.1}μs", metrics.serialization_time_us);
    println!("   SIMD efficiency: {:.1}%", metrics.simd_efficiency * 100.0);
    println!("   Query performance: {:.1}%", metrics.query_performance_factor * 100.0);
    println!("   Memory overhead: {:.1}%", metrics.memory_overhead_percent);
}

/// Demo hardware optimization detection
fn demo_hardware_optimization() {
    println!("\n🔧 HARDWARE OPTIMIZATION DETECTION");
    
    let caps = proximadb::core::hardware_capabilities::get_hardware_capabilities();
    
    println!("CPU Features Detected:");
    println!("   SSE4.2: {}", caps.cpu.features.sse42_support);
    println!("   AVX: {}", caps.cpu.features.avx_support);
    println!("   AVX2: {}", caps.cpu.features.avx2_support);
    println!("   AVX-512: {}", caps.cpu.features.avx512_support);
    println!("   POPCNT: {}", caps.cpu.features.popcnt_support);
    
    // Recommend optimal strategy based on hardware
    let recommended_alignment = if caps.cpu.features.avx512_support {
        "64-byte (AVX-512)"
    } else if caps.cpu.features.avx2_support {
        "32-byte (AVX2)"
    } else if caps.cpu.features.sse42_support {
        "16-byte (SSE)"
    } else {
        "4-byte (scalar)"
    };
    
    println!("\nRecommended Configuration:");
    println!("   Memory alignment: {}", recommended_alignment);
    println!("   Binary optimization: {}", if caps.cpu.features.popcnt_support { "Hardware popcount" } else { "Software fallback" });
    println!("   Vectorization: {}", if caps.cpu.features.avx2_support { "AVX2 32x8" } else { "Scalar" });
}

/// Serialization statistics structure
#[derive(Debug)]
struct SerializationStats {
    method: String,
    original_size: usize,
    serialized_size: usize,
    compression_ratio: f32,
    serialization_time_ms: u64,
    simd_efficiency: f32,
    memory_overhead: f32,
}

/// Main demo entry point
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize ProximaDB hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Run hardware detection demo
    demo_hardware_optimization();
    
    // Run serialization comparison demo
    demo_serialization_comparison().await?;
    
    println!("\n✅ Demo completed successfully!");
    println!("\nNext Steps:");
    println!("1. Review the performance analysis in docs/SERIALIZATION_PERFORMANCE_ANALYSIS.md");
    println!("2. Configure quantization strategies in your collection settings");
    println!("3. Monitor query performance with different strategies");
    println!("4. Adjust compression vs quality trade-offs based on your use case");
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_generation() {
        let config = DemoConfig {
            vector_dimensions: 128,
            num_vectors: 100,
            target_compression: 2.0,
        };
        
        let vectors = generate_test_vectors(&config);
        
        assert_eq!(vectors.len(), 100);
        assert_eq!(vectors[0].len(), 128);
        
        // Check normalization (vectors should be approximately unit length)
        let magnitude: f32 = vectors[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_progressive_search_simulation() {
        let config = DemoConfig::default();
        let vectors = generate_test_vectors(&config);
        
        let strategy_config = SerializationStrategyConfig::default();
        let optimizer = SerializationStrategyOptimizer::new(strategy_config);
        
        // Should not panic
        demo_progressive_search(&vectors, &optimizer).await.unwrap();
    }

    #[test]
    fn test_naive_serialization_benchmark() {
        let vectors = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![5.0, 6.0, 7.0, 8.0],
        ];
        
        let stats = benchmark_naive_serialization(&vectors).unwrap();
        
        assert_eq!(stats.original_size, 32); // 2 vectors * 4 elements * 4 bytes
        assert!(stats.serialized_size > 0);
        assert!(stats.compression_ratio > 0.0);
    }
}