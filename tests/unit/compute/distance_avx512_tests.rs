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

//! Unit tests for AVX-512 (enhanced AVX2) distance implementations

#[cfg(test)]
mod tests {
    use proximadb::compute::distance::{
        create_distance_calculator, detect_platform_capability, 
        DistanceCompute, DistanceMetric, PlatformCapability,
    };
    
    #[test]
    fn test_avx_distance_calculations() {
        let capability = detect_platform_capability();
        println!("Platform capability: {:?}", capability);
        
        // Test with various distance metrics
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
        ];
        
        for metric in metrics {
            let calc = create_distance_calculator(metric);
            
            // Test with aligned size (multiple of 32 for AVX512-like processing)
            let a = vec![1.0; 64];
            let b = vec![1.0; 64];
            
            let distance = calc.distance(&a, &b);
            match metric {
                DistanceMetric::Cosine => assert_eq!(distance, 0.0), // Identical vectors
                DistanceMetric::Euclidean => assert_eq!(distance, 0.0), // Identical vectors
                DistanceMetric::DotProduct => assert!(distance > 0.0), // Positive dot product for identical positive vectors
                DistanceMetric::Manhattan => assert_eq!(distance, 0.0), // Identical vectors
                _ => {}
            }
        }
    }

    #[test]
    fn test_avx_unaligned_sizes() {
        let capability = detect_platform_capability();
        
        // Test with various unaligned sizes
        let sizes = vec![33, 65, 127, 255, 513, 1025];
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
        ];
        
        for size in sizes {
            for metric in &metrics {
                let calc = create_distance_calculator(*metric);
                
                let a = vec![1.0; size];
                let b = vec![0.5; size];
                
                let distance = calc.distance(&a, &b);
                
                // Verify reasonable results
                match metric {
                    DistanceMetric::Cosine => {
                        assert!(distance >= -1e-6 && distance <= 2.0);
                        // For parallel constant vectors, cosine similarity = 1.0, distance = 0.0
                        assert!(distance.abs() < 1e-6);
                    }
                    DistanceMetric::Euclidean => {
                        assert!(distance > 0.0);
                        // Expected: sqrt(size * 0.5^2) = sqrt(size * 0.25) = 0.5 * sqrt(size)
                        let expected = 0.5 * (size as f32).sqrt();
                        assert!((distance - expected).abs() < 1e-3);
                    }
                    DistanceMetric::DotProduct => {
                        assert!(distance > 0.0); // Positive dot product
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn test_large_vector_performance() {
        let capability = detect_platform_capability();
        println!("Testing with platform capability: {:?}", capability);
        
        // Test with increasingly large vectors
        let sizes = vec![256, 512, 1024, 2048, 4096, 8192];
        
        for size in sizes {
            let a: Vec<f32> = (0..size).map(|i| (i as f32) * 0.001).collect();
            let b: Vec<f32> = (0..size).map(|i| ((i + 1) as f32) * 0.001).collect();
            
            let calc = create_distance_calculator(DistanceMetric::Cosine);
            
            let start = std::time::Instant::now();
            let distance = calc.distance(&a, &b);
            let duration = start.elapsed();
            
            println!("Cosine distance for size {}: {:?} (result: {})", size, duration, distance);
            
            // Verify computation completed quickly
            assert!(duration.as_secs() < 1);
            assert!(distance >= -1e-6 && distance <= 2.0); // Allow small negative values due to floating point precision
        }
    }

    #[test]
    fn test_batch_processing_performance() {
        let query = vec![1.0; 128];
        let num_vectors = 1000;
        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| vec![(i as f32) * 0.001; 128])
            .collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
        ];
        
        for metric in metrics {
            let calc = create_distance_calculator(metric);
            
            let start = std::time::Instant::now();
            let results = calc.distance_batch(&query, &vector_refs);
            let duration = start.elapsed();
            
            println!("Batch processing {} vectors with {:?}: {:?}", num_vectors, metric, duration);
            
            assert_eq!(results.len(), num_vectors);
            
            // Verify batch results match individual calculations for spot checks
            for i in (0..num_vectors).step_by(100) {
                let individual = calc.distance(&query, &vectors[i]);
                // Handle NaN case for cosine distance with zero vector
                if results[i].is_nan() && individual.is_nan() {
                    continue;
                }
                assert!(
                    (results[i] - individual).abs() < 1e-5,
                    "Batch result mismatch at index {} - batch: {}, individual: {}", i, results[i], individual
                );
            }
        }
    }

    #[test]
    fn test_orthogonal_vectors() {
        // Test orthogonal vectors with different metrics
        let mut a = vec![0.0; 64];
        let mut b = vec![0.0; 64];
        a[0] = 1.0;
        b[1] = 1.0;
        
        let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
        let dot_calc = create_distance_calculator(DistanceMetric::DotProduct);
        let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
        
        // Cosine distance of orthogonal vectors should be 1.0
        assert_eq!(cosine_calc.distance(&a, &b), 1.0);
        
        // Dot product of orthogonal vectors should be 0
        assert_eq!(dot_calc.distance(&a, &b), 0.0);
        
        // Euclidean distance should be sqrt(2)
        assert!((euclidean_calc.distance(&a, &b) - std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn test_parallel_vectors() {
        let calc_cosine = create_distance_calculator(DistanceMetric::Cosine);
        let calc_dot = create_distance_calculator(DistanceMetric::DotProduct);
        
        // Test with parallel vectors (one is scalar multiple of other)
        let a = vec![1.0; 64]; // 64 elements all 1.0
        let b = vec![2.0; 64]; // 64 elements all 2.0
        
        // Cosine distance should be 0 for parallel vectors
        assert!(calc_cosine.distance(&a, &b).abs() < 1e-6);
        
        // Dot product should be positive (high similarity)
        assert!(calc_dot.distance(&a, &b) > 0.0);
    }

    #[test]
    fn test_zero_vectors() {
        let zeros = vec![0.0; 64];
        let ones = vec![1.0; 64];
        
        let cosine_calc = create_distance_calculator(DistanceMetric::Cosine);
        let euclidean_calc = create_distance_calculator(DistanceMetric::Euclidean);
        let dot_calc = create_distance_calculator(DistanceMetric::DotProduct);
        
        // Cosine of zero vector is undefined (NaN due to 0/0)
        let cosine_dist = cosine_calc.distance(&zeros, &ones);
        assert!(cosine_dist.is_nan());
        
        // Euclidean distance
        let euclidean_dist = euclidean_calc.distance(&zeros, &ones);
        assert!((euclidean_dist - 8.0).abs() < 1e-6); // sqrt(64)
        
        // Dot product
        let dot_dist = dot_calc.distance(&zeros, &ones);
        assert_eq!(dot_dist, 0.0); // All zeros dot anything = 0
    }

    #[test]
    fn test_extreme_values() {
        let calc = create_distance_calculator(DistanceMetric::Euclidean);
        
        // Test with very small values
        let tiny = vec![1e-10; 64];
        let dist = calc.distance(&tiny, &tiny);
        assert_eq!(dist, 0.0); // Same vector
        
        // Test with very large values
        let large = vec![1e10; 64];
        let dist = calc.distance(&large, &large);
        assert_eq!(dist, 0.0); // Same vector
        
        // Test mixed magnitudes
        let mixed_small = vec![1e-10; 64];
        let mixed_large = vec![1e10; 64];
        let dist = calc.distance(&mixed_small, &mixed_large);
        assert!(dist.is_finite());
        assert!(dist > 0.0);
    }

    #[test]
    fn test_accuracy_consistency() {
        use proximadb::compute::distance::{CosineScalar, EuclideanScalar, DotProductScalar};
        
        // Compare optimized implementations with scalar versions
        let test_cases = vec![
            vec![1.0; 100],
            vec![0.5; 100],
            (0..100).map(|i| i as f32 * 0.01).collect(),
            (0..100).map(|i| (i % 10) as f32).collect(),
            (0..100).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect(),
        ];
        
        for (i, a) in test_cases.iter().enumerate() {
            for (j, b) in test_cases.iter().enumerate() {
                if i != j {
                    // Compare Cosine
                    let optimized_cosine = create_distance_calculator(DistanceMetric::Cosine).distance(a, b);
                    let scalar_cosine = CosineScalar.distance(a, b);
                    assert!(
                        (optimized_cosine - scalar_cosine).abs() < 1e-5,
                        "Cosine mismatch: optimized={}, scalar={}",
                        optimized_cosine,
                        scalar_cosine
                    );
                    
                    // Compare Euclidean
                    let optimized_euclidean = create_distance_calculator(DistanceMetric::Euclidean).distance(a, b);
                    let scalar_euclidean = EuclideanScalar.distance(a, b);
                    assert!(
                        (optimized_euclidean - scalar_euclidean).abs() < 1e-5,
                        "Euclidean mismatch: optimized={}, scalar={}",
                        optimized_euclidean,
                        scalar_euclidean
                    );
                    
                    // Compare Dot Product
                    let optimized_dot = create_distance_calculator(DistanceMetric::DotProduct).distance(a, b);
                    let scalar_dot = DotProductScalar.distance(a, b);
                    assert!(
                        (optimized_dot - scalar_dot).abs() < 1e-5,
                        "Dot product mismatch: optimized={}, scalar={}",
                        optimized_dot,
                        scalar_dot
                    );
                }
            }
        }
    }

    #[test]
    fn test_platform_specific_optimizations() {
        let capability = detect_platform_capability();
        
        // Log which optimizations are being used
        match capability {
            #[cfg(target_arch = "x86_64")]
            PlatformCapability::X86Avx512 => println!("Using AVX-512 optimizations"),
            #[cfg(target_arch = "x86_64")]
            PlatformCapability::X86Avx2 => println!("Using AVX2 optimizations"),
            #[cfg(target_arch = "x86_64")]
            PlatformCapability::X86Avx => println!("Using AVX optimizations"),
            #[cfg(target_arch = "x86_64")]
            PlatformCapability::X86Sse2 => println!("Using SSE2 optimizations"),
            #[cfg(target_arch = "aarch64")]
            PlatformCapability::ArmNeon => println!("Using ARM NEON optimizations"),
            PlatformCapability::Scalar => println!("Using scalar implementation"),
            _ => println!("Using unknown platform optimization"),
        }
        
        // Create a calculator and verify it works
        let calc = create_distance_calculator(DistanceMetric::Cosine);
        let a = vec![1.0; 128];
        let b = vec![2.0; 128];
        
        let distance = calc.distance(&a, &b);
        assert!(distance >= -1e-6 && distance <= 2.0); // Allow small negative values due to floating point precision
    }
}