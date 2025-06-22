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