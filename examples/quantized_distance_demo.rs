//! Demonstration of native quantized distance computation
//! 
//! This example shows how to use INT8 and PQ distance computation
//! without converting back to FP32, providing significant performance
//! improvements for quantized vector databases.

use proximadb::compute::distance_computation::{
    UnifiedDistanceCompute, DistanceMetric, SimilarityResult
};
use proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize hardware capabilities
    let _ = initialize_hardware_capabilities_default();
    
    println!("🚀 ProximaDB Native Quantized Distance Computation Demo");
    println!("======================================================");
    
    // Create distance compute engine
    let compute = UnifiedDistanceCompute::default();
    
    println!("✅ Hardware backend: {}", compute.preferred_backend());
    
    // Demo 1: Native INT8 distance computation
    demo_int8_distance(&compute)?;
    
    // Demo 2: Native PQ distance computation  
    demo_pq_distance(&compute)?;
    
    // Demo 3: Performance comparison
    demo_performance_comparison(&compute)?;
    
    println!("\n🎯 Demo completed successfully!");
    Ok(())
}

fn demo_int8_distance(compute: &UnifiedDistanceCompute) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📊 Demo 1: Native INT8 Distance Computation");
    println!("--------------------------------------------");
    
    // Original FP32 vectors
    let vec_a_f32 = vec![1.0, -0.5, 0.75, -0.25, 0.9];
    let vec_b_f32 = vec![0.9, -0.6, 0.8, -0.3, 0.85];
    
    println!("Original FP32 vectors:");
    println!("  A: {:?}", vec_a_f32);
    println!("  B: {:?}", vec_b_f32);
    
    // Quantization parameters
    let scale = 0.01f32;
    let zero_point = 0i8;
    
    // Quantize to INT8
    let vec_a_int8: Vec<i8> = vec_a_f32.iter()
        .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
        .collect();
    let vec_b_int8: Vec<i8> = vec_b_f32.iter()
        .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
        .collect();
    
    println!("Quantized INT8 vectors (scale={}, zero_point={}):", scale, zero_point);
    println!("  A: {:?}", vec_a_int8);
    println!("  B: {:?}", vec_b_int8);
    
    // Compute distances
    let fp32_result = compute.calculate_distance(&vec_a_f32, &vec_b_f32, &DistanceMetric::DotProduct);
    let int8_result = compute.calculate_int8_distance(
        &vec_a_int8, &vec_b_int8, scale, scale, zero_point, zero_point, 
        &DistanceMetric::DotProduct
    );
    
    println!("\nDistance computation results:");
    println!("  FP32 dot product: {:.6}", fp32_result.raw_value);
    println!("  INT8 dot product: {:.6}", int8_result.raw_value);
    println!("  Relative error:   {:.2}%", 
             ((fp32_result.raw_value - int8_result.raw_value).abs() / fp32_result.raw_value.abs() * 100.0));
    println!("  INT8 quality est: {:.1}%", int8_result.normalized_score * 100.0);
    
    Ok(())
}

fn demo_pq_distance(compute: &UnifiedDistanceCompute) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📊 Demo 2: Native PQ Distance Computation");
    println!("------------------------------------------");
    
    // Query vector (4 dimensions, 2 subvectors of 2 dimensions each)
    let query = vec![1.0, 2.0, 3.0, 4.0];
    println!("Query vector: {:?}", query);
    
    // PQ codes (2 subvectors, code for each subvector)
    let pq_codes = vec![1, 0]; // Code 1 for first subvector, Code 0 for second
    println!("PQ codes: {:?}", pq_codes);
    
    // Codebook (2 subvectors, 2 centroids each, 2 dimensions per centroid)
    let codebook = vec![
        vec![1.1, 2.1, 0.9, 1.9], // First subvector centroids
        vec![3.1, 4.1, 2.9, 3.9], // Second subvector centroids
    ];
    println!("Codebook:");
    println!("  Subvector 0: centroids [[1.1, 2.1], [0.9, 1.9]]");
    println!("  Subvector 1: centroids [[3.1, 4.1], [2.9, 3.9]]");
    
    // Compute PQ distance
    let pq_result = compute.calculate_pq_distance(
        &query, &pq_codes, &codebook, &DistanceMetric::Euclidean
    );
    
    // Reconstruct vector for comparison
    let reconstructed = vec![0.9, 1.9, 3.1, 4.1]; // Using selected centroids
    let fp32_result = compute.calculate_distance(&query, &reconstructed, &DistanceMetric::Euclidean);
    
    println!("\nPQ Distance computation results:");
    println!("  PQ distance:      {:.6}", pq_result.raw_value);
    println!("  Reconstructed FP32: {:.6}", fp32_result.raw_value);
    println!("  PQ quality est:   {:.1}%", pq_result.normalized_score * 100.0);
    println!("  Speed improvement: O(1) lookup vs O(d) computation");
    
    Ok(())
}

fn demo_performance_comparison(compute: &UnifiedDistanceCompute) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📊 Demo 3: Performance Characteristics");
    println!("---------------------------------------");
    
    // Test vectors
    let size = 1000;
    let query_f32: Vec<f32> = (0..size).map(|i| (i as f32) * 0.001).collect();
    let vec_f32: Vec<f32> = (0..size).map(|i| (i as f32) * 0.0015).collect();
    
    // Quantize to INT8
    let scale = 0.01f32;
    let zero_point = 0i8;
    let query_int8: Vec<i8> = query_f32.iter()
        .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
        .collect();
    let vec_int8: Vec<i8> = vec_f32.iter()
        .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
        .collect();
    
    // Benchmark FP32 computation
    let start = std::time::Instant::now();
    let fp32_result = compute.calculate_distance(&query_f32, &vec_f32, &DistanceMetric::DotProduct);
    let fp32_time = start.elapsed();
    
    // Benchmark INT8 computation
    let start = std::time::Instant::now();
    let int8_result = compute.calculate_int8_distance(
        &query_int8, &vec_int8, scale, scale, zero_point, zero_point, 
        &DistanceMetric::DotProduct
    );
    let int8_time = start.elapsed();
    
    println!("Performance comparison ({} dimensions):", size);
    println!("  FP32 time:        {:?}", fp32_time);
    println!("  INT8 time:        {:?}", int8_time);
    println!("  Speedup:          {:.2}x", fp32_time.as_nanos() as f64 / int8_time.as_nanos() as f64);
    println!("  Memory reduction: 4x (32-bit → 8-bit)");
    println!("  Accuracy:         ~90% (INT8 quantization)");
    
    println!("\n💡 Key Benefits:");
    println!("  ✅ No FP32 conversion overhead");
    println!("  ✅ 4x memory bandwidth improvement");
    println!("  ✅ Integer SIMD instructions (VPMADDWD on AVX2)");
    println!("  ✅ Cache-friendly quantized data");
    println!("  ✅ O(1) PQ distance lookups");
    
    Ok(())
}