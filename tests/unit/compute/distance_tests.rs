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

use proximadb::compute::distance::{
    create_distance_calculator, detect_platform_capability,
    DistanceMetric, CosineScalar, EuclideanScalar, DotProductScalar, DistanceCompute
};

#[test]
fn test_platform_detection() {
    let capability = detect_platform_capability();
    println!("Detected platform capability: {:?}", capability);
    
    // Test that we can create calculators for all metrics
    let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    let dot_calc = create_distance_calculator(DistanceMetric::DotProduct);
    
    let a = vec![1.0, 2.0, 3.0, 4.0];
    let b = vec![2.0, 3.0, 4.0, 5.0];
    
    let cosine = cosine_calc.distance(&a, &b);
    let euclidean = euclidean_calc.distance(&a, &b);
    let dot = dot_calc.distance(&a, &b);
    
    println!("Cosine distance: {}", cosine);
    println!("Euclidean distance: {}", euclidean);
    println!("Dot product: {}", dot);
    
    // Verify results are reasonable
    assert!(cosine >= 0.0 && cosine <= 2.0);
    assert!(euclidean >= 0.0);
    assert!(dot >= 0.0);
}

#[test]
fn test_scalar_implementations() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    
    let cosine = CosineScalar.distance(&a, &b);
    assert!((cosine - 1.0).abs() < 0.0001); // Orthogonal vectors
    
    let euclidean = EuclideanScalar.distance(&a, &b);
    assert!((euclidean - 1.414).abs() < 0.01); // sqrt(2)
    
    let dot = DotProductScalar.distance(&a, &b);
    assert_eq!(dot, 0.0); // Orthogonal vectors
}

#[test]
fn test_metric_specific_implementations() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    
    // Test direct usage of optimized calculators
    let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    let dot_calc = create_distance_calculator(DistanceMetric::DotProduct);
    let manhattan_calc = create_distance_calculator(DistanceMetric::Manhattan);
    
    // Test that all calculators work without panicking
    let _ = cosine_calc.distance(&a, &b);
    let _ = euclidean_calc.distance(&a, &b);
    let _ = dot_calc.distance(&a, &b);
    let _ = manhattan_calc.distance(&a, &b);
    
    // Verify calculator returns correct metric
    assert_eq!(cosine_calc.metric(), DistanceMetric::Cosine);
    assert_eq!(euclidean_calc.metric(), DistanceMetric::Euclidean);
    assert_eq!(dot_calc.metric(), DistanceMetric::DotProduct);
    assert_eq!(manhattan_calc.metric(), DistanceMetric::Manhattan);
}

#[test]
fn test_batch_processing() {
    let query = vec![1.0, 2.0, 3.0];
    let vectors = vec![
        vec![1.0, 2.0, 3.0],
        vec![2.0, 3.0, 4.0],
        vec![3.0, 4.0, 5.0],
    ];
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
    
    let calc = create_distance_calculator(DistanceMetric::Euclidean);
    let results = calc.distance_batch(&query, &vector_refs);
    
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], 0.0); // Same vector
    assert!(results[1] > 0.0); // Different vectors
    assert!(results[2] > results[1]); // More distant vector
}

#[test]
fn test_distance_metric_properties() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    
    // Test cosine distance properties
    let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
    assert!(!cosine_calc.is_similarity());
    
    // Test dot product properties  
    let dot_calc = create_distance_calculator(DistanceMetric::DotProduct);
    assert!(dot_calc.is_similarity());
    
    // Test euclidean distance properties
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    assert!(!euclidean_calc.is_similarity());
    
    // Test manhattan distance properties
    let manhattan_calc = create_distance_calculator(DistanceMetric::Manhattan);
    assert!(!manhattan_calc.is_similarity());
}

#[test]
fn test_simd_vs_scalar_consistency() {
    let a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let b = vec![8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0];
    
    // Test that SIMD and scalar implementations give consistent results
    let simd_calc = create_distance_calculator(DistanceMetric::Cosine);
    let scalar_cosine = CosineScalar;
    
    let simd_result = simd_calc.distance(&a, &b);
    let scalar_result = scalar_cosine.distance(&a, &b);
    
    // Allow small floating point differences
    assert!((simd_result - scalar_result).abs() < 1e-6);
}

#[test]
fn test_zero_vectors() {
    let zero_a = vec![0.0, 0.0, 0.0];
    let zero_b = vec![0.0, 0.0, 0.0];
    let non_zero = vec![1.0, 2.0, 3.0];
    
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    let manhattan_calc = create_distance_calculator(DistanceMetric::Manhattan);
    
    // Zero distance between identical zero vectors
    assert_eq!(euclidean_calc.distance(&zero_a, &zero_b), 0.0);
    assert_eq!(manhattan_calc.distance(&zero_a, &zero_b), 0.0);
    
    // Non-zero distance between zero and non-zero vectors
    assert!(euclidean_calc.distance(&zero_a, &non_zero) > 0.0);
    assert!(manhattan_calc.distance(&zero_a, &non_zero) > 0.0);
}

#[test]
fn test_edge_cases() {
    // Test with single element vectors
    let a = vec![5.0];
    let b = vec![3.0];
    
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
    let manhattan_calc = create_distance_calculator(DistanceMetric::Manhattan);
    
    // Single element euclidean distance
    assert_eq!(euclidean_calc.distance(&a, &b), 2.0);
    assert_eq!(manhattan_calc.distance(&a, &b), 2.0);
    
    // Test with very small values
    let tiny_a = vec![1e-10, 1e-10];
    let tiny_b = vec![2e-10, 2e-10];
    
    let dist = euclidean_calc.distance(&tiny_a, &tiny_b);
    assert!(dist > 0.0 && dist < 1e-5);
    
    // Test with very large values
    let large_a = vec![1e6, 1e6];
    let large_b = vec![1e6 + 1.0, 1e6 + 1.0];
    
    let dist = euclidean_calc.distance(&large_a, &large_b);
    assert!((dist - std::f32::consts::SQRT_2).abs() < 0.01); // sqrt(2) with relaxed precision for large numbers
}

#[test]
fn test_jaccard_distance() {
    let jaccard_calc = create_distance_calculator(DistanceMetric::Jaccard);
    
    // Test identical sets (binary vectors)
    let a = vec![1.0, 1.0, 0.0, 0.0];
    let b = vec![1.0, 1.0, 0.0, 0.0];
    assert_eq!(jaccard_calc.distance(&a, &b), 0.0);
    
    // Test completely different sets
    let c = vec![1.0, 1.0, 0.0, 0.0];
    let d = vec![0.0, 0.0, 1.0, 1.0];
    assert_eq!(jaccard_calc.distance(&c, &d), 1.0);
    
    // Test partial overlap
    let e = vec![1.0, 1.0, 0.0, 0.0];
    let f = vec![1.0, 0.0, 1.0, 0.0];
    let dist = jaccard_calc.distance(&e, &f);
    assert!(dist > 0.0 && dist < 1.0);
}

#[test]
fn test_hamming_distance() {
    let hamming_calc = create_distance_calculator(DistanceMetric::Hamming);
    
    // Test identical vectors
    let a = vec![1.0, 0.0, 1.0, 0.0];
    let b = vec![1.0, 0.0, 1.0, 0.0];
    assert_eq!(hamming_calc.distance(&a, &b), 0.0);
    
    // Test completely different vectors
    let c = vec![1.0, 1.0, 1.0, 1.0];
    let d = vec![0.0, 0.0, 0.0, 0.0];
    assert_eq!(hamming_calc.distance(&c, &d), 4.0);
    
    // Test partial difference
    let e = vec![1.0, 0.0, 1.0, 0.0];
    let f = vec![1.0, 1.0, 0.0, 0.0];
    assert_eq!(hamming_calc.distance(&e, &f), 2.0);
}

#[test]
fn test_chebyshev_distance() {
    let chebyshev_calc = create_distance_calculator(DistanceMetric::Chebyshev);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(chebyshev_calc.distance(&a, &b), 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![4.0, 2.0, 1.0];
    assert_eq!(chebyshev_calc.distance(&c, &d), 3.0); // max(|1-4|, |2-2|, |3-1|) = 3.0
    
    // Test with negative values
    let e = vec![-1.0, -2.0, -3.0];
    let f = vec![1.0, 2.0, 3.0];
    assert_eq!(chebyshev_calc.distance(&e, &f), 6.0); // max(2, 4, 6) = 6
}

#[test]
fn test_canberra_distance() {
    let canberra_calc = create_distance_calculator(DistanceMetric::Canberra);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(canberra_calc.distance(&a, &b), 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![2.0, 3.0, 5.0];
    let dist = canberra_calc.distance(&c, &d);
    // |1-2|/(|1|+|2|) + |2-3|/(|2|+|3|) + |3-5|/(|3|+|5|) = 1/3 + 1/5 + 2/8 = 0.783...
    assert!((dist - 0.783).abs() < 0.01);
    
    // Test with zero values
    let e = vec![0.0, 1.0, 2.0];
    let f = vec![1.0, 0.0, 3.0];
    let dist2 = canberra_calc.distance(&e, &f);
    assert!(dist2 > 0.0);
}

#[test]
fn test_minkowski_distance() {
    let minkowski_calc = create_distance_calculator(DistanceMetric::Minkowski);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(minkowski_calc.distance(&a, &b), 0.0);
    
    // Test different vectors (with p=3 default)
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    let dist = minkowski_calc.distance(&c, &d);
    // (|1-0|^3 + |0-1|^3)^(1/3) = (1 + 1)^(1/3) = 2^(1/3) ≈ 1.26
    assert!((dist - 1.26).abs() < 0.01);
}

#[test]
fn test_angular_distance() {
    let angular_calc = create_distance_calculator(DistanceMetric::Angular);
    
    // Test identical vectors (angle = 0)
    let a = vec![1.0, 0.0];
    let b = vec![1.0, 0.0];
    assert!((angular_calc.distance(&a, &b) - 0.0).abs() < 1e-6);
    
    // Test orthogonal vectors (angle = π/2)
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    assert!((angular_calc.distance(&c, &d) - 0.5).abs() < 0.01); // π/2 / π = 0.5
    
    // Test opposite vectors (angle = π)
    let e = vec![1.0, 0.0];
    let f = vec![-1.0, 0.0];
    assert!((angular_calc.distance(&e, &f) - 1.0).abs() < 0.01); // π / π = 1.0
}

#[test]
fn test_bray_curtis_distance() {
    let bray_curtis_calc = create_distance_calculator(DistanceMetric::BrayCurtis);
    
    // Test identical vectors
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![1.0, 2.0, 3.0];
    assert_eq!(bray_curtis_calc.distance(&a, &b), 0.0);
    
    // Test different vectors
    let c = vec![1.0, 2.0, 3.0];
    let d = vec![2.0, 3.0, 4.0];
    let dist = bray_curtis_calc.distance(&c, &d);
    // |1-2| + |2-3| + |3-4| / (1+2+2+3+3+4) = 3/15 = 0.2
    assert!((dist - 0.2).abs() < 0.01);
    
    // Test with zero vectors
    let e = vec![0.0, 0.0, 0.0];
    let f = vec![0.0, 0.0, 0.0];
    assert_eq!(bray_curtis_calc.distance(&e, &f), 0.0);
}

#[test]
fn test_hellinger_distance() {
    let hellinger_calc = create_distance_calculator(DistanceMetric::Hellinger);
    
    // Test identical distributions
    let a = vec![0.25, 0.25, 0.25, 0.25];
    let b = vec![0.25, 0.25, 0.25, 0.25];
    assert!((hellinger_calc.distance(&a, &b) - 0.0).abs() < 1e-6);
    
    // Test different distributions
    let c = vec![1.0, 0.0];
    let d = vec![0.0, 1.0];
    let dist = hellinger_calc.distance(&c, &d);
    // sqrt(0.5 * ((1-0)^2 + (0-1)^2)) = sqrt(0.5 * 2) = 1.0
    assert!((dist - 1.0).abs() < 0.01);
    
    // Test with non-normalized vectors (should normalize internally)
    let e = vec![2.0, 2.0];
    let f = vec![1.0, 3.0];
    let dist2 = hellinger_calc.distance(&e, &f);
    assert!(dist2 > 0.0 && dist2 < 1.0);
}

#[test]
fn test_batch_consistency() {
    let query = vec![1.0, 2.0, 3.0, 4.0];
    let vectors = vec![
        vec![1.0, 2.0, 3.0, 4.0], // Same as query
        vec![2.0, 3.0, 4.0, 5.0],
        vec![0.0, 1.0, 2.0, 3.0],
    ];
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
    
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
        let calc = create_distance_calculator(metric);
        let batch_results = calc.distance_batch(&query, &vector_refs);
        
        // Verify batch results match individual calculations
        for (i, vec_ref) in vector_refs.iter().enumerate() {
            let individual_result = calc.distance(&query, vec_ref);
            assert!(
                (batch_results[i] - individual_result).abs() < 1e-6,
                "Batch and individual results don't match for {:?}",
                metric
            );
        }
    }
}

#[test]
fn test_large_vector_dimensions() {
    // Test with high-dimensional vectors
    let dim = 1024;
    let a: Vec<f32> = (0..dim).map(|i| i as f32 * 0.001).collect();
    let b: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * 0.001).collect();
    
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
    
    // Just verify no panic and reasonable results
    let euclidean_dist = euclidean_calc.distance(&a, &b);
    let cosine_dist = cosine_calc.distance(&a, &b);
    
    assert!(euclidean_dist > 0.0);
    assert!(cosine_dist >= 0.0 && cosine_dist <= 2.0);
}

#[test]
fn test_nan_and_infinity_handling() {
    use std::f32::{NAN, INFINITY, NEG_INFINITY};
    
    let normal = vec![1.0, 2.0, 3.0];
    let with_nan = vec![1.0, NAN, 3.0];
    let with_inf = vec![1.0, INFINITY, 3.0];
    let with_neg_inf = vec![1.0, NEG_INFINITY, 3.0];
    
    let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
    
    // Test NaN propagation
    let dist_nan = euclidean_calc.distance(&normal, &with_nan);
    assert!(dist_nan.is_nan());
    
    // Test infinity handling
    let dist_inf = euclidean_calc.distance(&normal, &with_inf);
    assert!(dist_inf.is_infinite());
    
    let dist_neg_inf = euclidean_calc.distance(&normal, &with_neg_inf);
    assert!(dist_neg_inf.is_infinite());
}