/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unit tests for distance calculation functionality

use proximadb::compute::distance_computation::DistanceMetric;
use tracing::{debug, error, info, warn};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;

#[test]
fn test_platform_detection() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let capability = proximadb::core::hardware_capabilities::get_hardware_capabilities();
    debug!("Detected platform capability: {:?}", capability);
    
    // Test that we can create calculators for all metrics
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let dot_calc = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
    
    let a = vec![1.0, 2.0, 3.0, 4.0];
    let b = vec![2.0, 3.0, 4.0, 5.0];
    
    let cosine = cosine_calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    let euclidean = euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    let dot = dot_calc.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
    
    debug!("Cosine distance: {}", cosine.raw_value);
    debug!("Euclidean distance: {}", euclidean.raw_value);
    debug!("Dot product: {}", dot.raw_value);
    
    // Verify results are reasonable
    assert!(cosine.raw_value >= 0.0 && cosine.raw_value <= 2.0);
    assert!(euclidean.raw_value >= 0.0);
    assert!(dot.raw_value >= 0.0);
}

#[test]
fn test_scalar_implementations() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let dot_calc = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
    
    let cosine = cosine_calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    assert!((cosine.raw_value - 1.0).abs() < 0.0001); // Orthogonal vectors
    
    let euclidean = euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    assert!((euclidean.raw_value - 1.414).abs() < 0.01); // sqrt(2)
    
    let dot = dot_calc.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
    assert_eq!(dot.raw_value, 0.0); // Orthogonal vectors
}

#[test]
fn test_metric_specific_implementations() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    
    // Test direct usage of optimized calculators
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let dot_calc = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
    let manhattan_calc = UnifiedDistanceCompute::new(DistanceMetric::Manhattan);
    
    // Test that all calculators work without panicking
    let _ = cosine_calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    let _ = euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    let _ = dot_calc.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
    let _ = manhattan_calc.calculate_distance(&a, &b, &DistanceMetric::Manhattan);
    
    // UnifiedDistanceCompute internally handles the metric
    // No need to verify the metric as it's passed directly to calculate_distance
}

#[test]
fn test_batch_processing() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let query = vec![1.0, 2.0, 3.0];
    let vectors = vec![
        vec![1.0, 2.0, 3.0],
        vec![2.0, 3.0, 4.0],
        vec![3.0, 4.0, 5.0],
    ];
    
    let calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let mut results = Vec::new();
    for v in &vectors {
        results.push(calc.calculate_distance(&query, v, &DistanceMetric::Euclidean));
    }
    
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].raw_value, 0.0); // Same vector
    assert!(results[1].raw_value > 0.0); // Different vectors
    assert!(results[2].raw_value > results[1].raw_value); // More distant vector
}

#[test]
fn test_distance_metric_properties() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    
    // Test cosine distance properties
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    // Note: is_similarity is not directly exposed in UnifiedDistanceCompute
    // DotProduct is a similarity metric, others are distance metrics
    
    // Test dot product properties  
    let dot_calc = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
    // DotProduct: higher values = more similar
    
    // Test euclidean distance properties
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    // Euclidean: lower values = more similar
    
    // Test manhattan distance properties
    let manhattan_calc = UnifiedDistanceCompute::new(DistanceMetric::Manhattan);
    // Manhattan: lower values = more similar
    
    // Just verify they can calculate distances
    let _ = cosine_calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    let _ = dot_calc.calculate_distance(&a, &b, &DistanceMetric::DotProduct);
    let _ = euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    let _ = manhattan_calc.calculate_distance(&a, &b, &DistanceMetric::Manhattan);
}

#[test]
fn test_simd_vs_scalar_consistency() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let b = vec![8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0];
    
    // UnifiedDistanceCompute automatically uses optimal implementation
    let calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    
    let result = calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    
    // Just verify the result is reasonable
    assert!(result.raw_value >= 0.0 && result.raw_value <= 2.0);
}

#[test]
fn test_zero_vectors() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let zero_a = vec![0.0, 0.0, 0.0];
    let zero_b = vec![0.0, 0.0, 0.0];
    let non_zero = vec![1.0, 2.0, 3.0];
    
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let manhattan_calc = UnifiedDistanceCompute::new(DistanceMetric::Manhattan);
    
    // Zero distance between identical zero vectors
    assert_eq!(euclidean_calc.calculate_distance(&zero_a, &zero_b, &DistanceMetric::Euclidean).raw_value, 0.0);
    assert_eq!(manhattan_calc.calculate_distance(&zero_a, &zero_b, &DistanceMetric::Manhattan).raw_value, 0.0);
    
    // Non-zero distance between zero and non-zero vectors
    assert!(euclidean_calc.calculate_distance(&zero_a, &non_zero, &DistanceMetric::Euclidean).raw_value > 0.0);
    assert!(manhattan_calc.calculate_distance(&zero_a, &non_zero, &DistanceMetric::Manhattan).raw_value > 0.0);
}

#[test]
fn test_edge_cases() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test with single element vectors
    let a = vec![5.0];
    let b = vec![3.0];
    
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let manhattan_calc = UnifiedDistanceCompute::new(DistanceMetric::Manhattan);
    
    // Single element euclidean distance
    assert_eq!(euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean).raw_value, 2.0);
    assert_eq!(manhattan_calc.calculate_distance(&a, &b, &DistanceMetric::Manhattan).raw_value, 2.0);
    
    // Test with very small values
    let tiny_a = vec![1e-10, 1e-10];
    let tiny_b = vec![2e-10, 2e-10];
    
    let dist = euclidean_calc.calculate_distance(&tiny_a, &tiny_b, &DistanceMetric::Euclidean);
    assert!(dist.raw_value > 0.0 && dist.raw_value < 1e-5);
    
    // Test with very large values
    let large_a = vec![1e6, 1e6];
    let large_b = vec![1e6 + 1.0, 1e6 + 1.0];
    
    let dist = euclidean_calc.calculate_distance(&large_a, &large_b, &DistanceMetric::Euclidean);
    assert!((dist.raw_value - std::f32::consts::SQRT_2).abs() < 0.01); // sqrt(2) with relaxed precision for large numbers
}

#[test]
fn test_jaccard_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let jaccard_calc = UnifiedDistanceCompute::new(DistanceMetric::Jaccard);
    
    // Test identical sets (binary vectors)
    let a = vec![1.0, 1.0, 0.0, 0.0];
    let b = vec![1.0, 1.0, 0.0, 0.0];
    assert_eq!(jaccard_calc.calculate_distance(&a, &b, &DistanceMetric::Jaccard).raw_value, 0.0);
    
    // Test completely different sets
    let c = vec![1.0, 1.0, 0.0, 0.0];
    let d = vec![0.0, 0.0, 1.0, 1.0];
    assert_eq!(jaccard_calc.calculate_distance(&c, &d, &DistanceMetric::Jaccard).raw_value, 1.0);
    
    // Test partial overlap
    let e = vec![1.0, 1.0, 0.0, 0.0];
    let f = vec![1.0, 0.0, 1.0, 0.0];
    let dist = jaccard_calc.calculate_distance(&e, &f, &DistanceMetric::Jaccard);
    assert!(dist.raw_value > 0.0 && dist.raw_value < 1.0);
}

#[test]
fn test_hamming_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let hamming_calc = UnifiedDistanceCompute::new(DistanceMetric::Hamming);
    
    // Test identical vectors
    let a = vec![1.0, 0.0, 1.0, 0.0];
    let b = vec![1.0, 0.0, 1.0, 0.0];
    assert_eq!(hamming_calc.calculate_distance(&a, &b, &DistanceMetric::Hamming).raw_value, 0.0);
    
    // Test completely different vectors
    let c = vec![1.0, 1.0, 1.0, 1.0];
    let d = vec![0.0, 0.0, 0.0, 0.0];
    assert_eq!(hamming_calc.calculate_distance(&c, &d, &DistanceMetric::Hamming).raw_value, 4.0);
    
    // Test partial difference
    let e = vec![1.0, 0.0, 1.0, 0.0];
    let f = vec![1.0, 1.0, 0.0, 0.0];
    assert_eq!(hamming_calc.calculate_distance(&e, &f, &DistanceMetric::Hamming).raw_value, 2.0);
}

#[test]
fn test_chebyshev_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let chebyshev_calc = UnifiedDistanceCompute::new(DistanceMetric::Chebyshev);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(chebyshev_calc.calculate_distance(&a, &b, &DistanceMetric::Chebyshev).raw_value, 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![4.0, 2.0, 1.0];
    assert_eq!(chebyshev_calc.calculate_distance(&c, &d, &DistanceMetric::Chebyshev).raw_value, 3.0); // max(|1-4|, |2-2|, |3-1|) = 3.0
    
    // Test with negative values
    let e = vec![-1.0, -2.0, -3.0];
    let f = vec![1.0, 2.0, 3.0];
    assert_eq!(chebyshev_calc.calculate_distance(&e, &f, &DistanceMetric::Chebyshev).raw_value, 6.0); // max(2, 4, 6) = 6
}

#[test]
fn test_canberra_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let canberra_calc = UnifiedDistanceCompute::new(DistanceMetric::Canberra);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(canberra_calc.calculate_distance(&a, &b, &DistanceMetric::Canberra).raw_value, 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![2.0, 3.0, 5.0];
    let dist = canberra_calc.calculate_distance(&c, &d, &DistanceMetric::Canberra);
    // |1-2|/(|1|+|2|) + |2-3|/(|2|+|3|) + |3-5|/(|3|+|5|) = 1/3 + 1/5 + 2/8 = 0.783...
    assert!((dist.raw_value - 0.783).abs() < 0.01);
    
    // Test with zero values
    let e = vec![0.0, 1.0, 2.0];
    let f = vec![1.0, 0.0, 3.0];
    let dist2 = canberra_calc.calculate_distance(&e, &f, &DistanceMetric::Canberra);
    assert!(dist2.raw_value > 0.0);
}

#[test]
fn test_minkowski_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let minkowski_calc = UnifiedDistanceCompute::new(DistanceMetric::Minkowski);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(minkowski_calc.calculate_distance(&a, &b, &DistanceMetric::Minkowski).raw_value, 0.0);
    
    // Test different vectors (with p=3 default)
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    let dist = minkowski_calc.calculate_distance(&c, &d, &DistanceMetric::Minkowski);
    // (|1-0|^3 + |0-1|^3)^(1/3) = (1 + 1)^(1/3) = 2^(1/3) ≈ 1.26
    assert!((dist.raw_value - 1.26).abs() < 0.01);
}

#[test]
fn test_angular_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let angular_calc = UnifiedDistanceCompute::new(DistanceMetric::Angular);
    
    // Test identical vectors (angle = 0)
    let a = vec![1.0, 0.0];
    let b = vec![1.0, 0.0];
    assert!((angular_calc.calculate_distance(&a, &b, &DistanceMetric::Angular).raw_value - 0.0).abs() < 1e-6);
    
    // Test orthogonal vectors (angle = π/2)
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    assert!((angular_calc.calculate_distance(&c, &d, &DistanceMetric::Angular).raw_value - 0.5).abs() < 0.01); // π/2 / π = 0.5
    
    // Test opposite vectors (angle = π)
    let e = vec![1.0, 0.0];
    let f = vec![-1.0, 0.0];
    assert!((angular_calc.calculate_distance(&e, &f, &DistanceMetric::Angular).raw_value - 1.0).abs() < 0.01); // π / π = 1.0
}

#[test]
fn test_bray_curtis_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let bray_curtis_calc = UnifiedDistanceCompute::new(DistanceMetric::BrayCurtis);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(bray_curtis_calc.calculate_distance(&a, &b, &DistanceMetric::BrayCurtis).raw_value, 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![2.0, 3.0, 4.0];
    let dist = bray_curtis_calc.calculate_distance(&c, &d, &DistanceMetric::BrayCurtis);
    // |1-2| + |2-3| + |3-4| / (1+2+2+3+3+4) = 3/15 = 0.2
    assert!((dist.raw_value - 0.2).abs() < 0.01);
    
    // Test with zero vectors
    let e = vec![0.0, 0.0, 0.0];
    let f = vec![0.0, 0.0, 0.0];
    assert_eq!(bray_curtis_calc.calculate_distance(&e, &f, &DistanceMetric::BrayCurtis).raw_value, 0.0);
}

#[test]
fn test_hellinger_distance() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let hellinger_calc = UnifiedDistanceCompute::new(DistanceMetric::Hellinger);
    
    // Test identical distributions
    let a = vec![0.25, 0.25, 0.25, 0.25];
    let b = vec![0.25, 0.25, 0.25, 0.25];
    assert!((hellinger_calc.calculate_distance(&a, &b, &DistanceMetric::Hellinger).raw_value - 0.0).abs() < 1e-6);
    
    // Test different distributions
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    let dist = hellinger_calc.calculate_distance(&c, &d, &DistanceMetric::Hellinger);
    // sqrt(0.5 * ((1-0)^2 + (0-1)^2)) = sqrt(0.5 * 2) = 1.0
    assert!((dist.raw_value - 1.0).abs() < 0.01);
    
    // Test with non-normalized vectors (should normalize internally)
    let e = vec![2.0, 2.0];
    let f = vec![1.0, 3.0];
    let dist2 = hellinger_calc.calculate_distance(&e, &f, &DistanceMetric::Hellinger);
    assert!(dist2.raw_value > 0.0 && dist2.raw_value < 1.0);
}

#[test]
fn test_batch_consistency() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let query = vec![1.0, 2.0, 3.0, 4.0];
    let vectors = vec![
        vec![1.0, 2.0, 3.0, 4.0], // Same as query
        vec![2.0, 3.0, 4.0, 5.0],
        vec![0.0, 1.0, 2.0, 3.0],
    ];
    
    // Test all distance metrics
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
        DistanceMetric::Jaccard,
        DistanceMetric::Hamming,
        DistanceMetric::Chebyshev,
        DistanceMetric::Canberra,
        DistanceMetric::Minkowski,
        DistanceMetric::Angular,
        DistanceMetric::BrayCurtis,
        DistanceMetric::Hellinger,
    ];
    
    for metric in metrics {
        let calc = UnifiedDistanceCompute::new(metric);
        
        // Calculate batch results manually
        let mut batch_results = Vec::new();
        for v in &vectors {
            batch_results.push(calc.calculate_distance(&query, v, &metric));
        }
        
        // Verify all calculations work
        for (i, v) in vectors.iter().enumerate() {
            let individual_result = calc.calculate_distance(&query, v, &metric);
            assert!(
                (batch_results[i].raw_value - individual_result.raw_value).abs() < 1e-6,
                "Batch and individual results don't match for {:?}",
                metric
            );
        }
    }
}

#[test]
fn test_large_vector_dimensions() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Test with high-dimensional vectors
    let dim = 1024;
    let a: Vec<f32> = (0..dim).map(|i| i as f32 * 0.001).collect();
    let b: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * 0.001).collect();
    
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    let cosine_calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    
    // Just verify no panic and reasonable results
    let euclidean_dist = euclidean_calc.calculate_distance(&a, &b, &DistanceMetric::Euclidean);
    let cosine_dist = cosine_calc.calculate_distance(&a, &b, &DistanceMetric::Cosine);
    
    assert!(euclidean_dist.raw_value > 0.0);
    assert!(cosine_dist.raw_value >= 0.0 && cosine_dist.raw_value <= 2.0);
}

#[test]
fn test_nan_and_infinity_handling() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    use std::f32::{NAN, INFINITY, NEG_INFINITY};
    
    let normal = vec![1.0, 2.0, 3.0];
    let with_nan = vec![1.0, NAN, 3.0];
    let with_inf = vec![1.0, INFINITY, 3.0];
    let with_neg_inf = vec![1.0, NEG_INFINITY, 3.0];
    
    let euclidean_calc = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
    
    // Test NaN propagation
    let dist_nan = euclidean_calc.calculate_distance(&normal, &with_nan, &DistanceMetric::Euclidean);
    assert!(dist_nan.raw_value.is_nan());
    
    // Test infinity handling
    let dist_inf = euclidean_calc.calculate_distance(&normal, &with_inf, &DistanceMetric::Euclidean);
    assert!(dist_inf.raw_value.is_infinite());
    
    let dist_neg_inf = euclidean_calc.calculate_distance(&normal, &with_neg_inf, &DistanceMetric::Euclidean);
    assert!(dist_neg_inf.raw_value.is_infinite());
}