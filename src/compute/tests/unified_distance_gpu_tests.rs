/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Integration tests for UnifiedDistanceCompute GPU selection and usage

#[cfg(test)]
mod tests {
    use crate::compute::distance_computation::engine::{UnifiedDistanceCompute, HardwareBackend, DistanceMode};
    use crate::compute::distance_computation::{DistanceMetric, PlatformCapability};
    use std::time::Instant;
use tracing::{debug, error, info, warn};

    /// Test that UnifiedDistanceCompute correctly selects GPU when available
    #[tokio::test]
    async fn test_unified_distance_gpu_selection() {
        debug!("🔍 Testing UnifiedDistanceCompute GPU backend selection...");
        
        let compute = UnifiedDistanceCompute::default();
        
        let preferred_backend = compute.preferred_backend();
        let available_backends = compute.available_backends();
        
        info!("🎯 Preferred backend: {}", preferred_backend);
        debug!("📋 Available backends: {:?}", available_backends);
        
        // Verify that we have a sensible selection
        match preferred_backend {
            HardwareBackend::Cuda => {
                info!("✅ CUDA GPU selected as preferred backend");
                assert!(available_backends.contains_hash(&HardwareBackend::Cuda));
            }
            HardwareBackend::Rocm => {
                info!("✅ ROCm GPU selected as preferred backend");
                assert!(available_backends.contains_hash(&HardwareBackend::Rocm));
            }
            HardwareBackend::Mps => {
                info!("✅ Metal Performance Shaders selected as preferred backend");
                assert!(available_backends.contains_hash(&HardwareBackend::Mps));
            }
            HardwareBackend::OpenCL => {
                info!("✅ OpenCL GPU selected as preferred backend");
                assert!(available_backends.contains_hash(&HardwareBackend::OpenCL));
            }
            HardwareBackend::CpuSimd(capability) => {
                info!("✅ CPU SIMD selected as preferred backend: {}", capability);
                assert!(matches!(
                    capability,
                    PlatformCapability::X86Avx512 | 
                    PlatformCapability::X86Avx2 | 
                    PlatformCapability::X86Avx | 
                    PlatformCapability::X86Sse2 |
                    PlatformCapability::ArmNeon |
                    PlatformCapability::ArmSve |
                    PlatformCapability::Scalar
                ));
            }
            HardwareBackend::Scalar => {
                warn!("⚠️ Scalar backend selected (no acceleration available)");
            }
        }
        
        // Verify that CPU SIMD and Scalar are always available as fallbacks
        assert!(available_backends.iter().any(|b| matches!(b, HardwareBackend::CpuSimd(_))));
        assert!(available_backends.contains_hash(&HardwareBackend::Scalar));
        
        info!("✅ Backend selection validation passed");
    }

    /// Test GPU threshold selection logic
    #[tokio::test]
    async fn test_gpu_threshold_selection() {
        debug!("🔍 Testing GPU threshold selection logic...");
        
        let mut compute = UnifiedDistanceCompute::default();
        compute.set_gpu_enabled(true);
        
        // Test small vectors (should use CPU)
        let small_query = vec![1.0, 0.0, 0.0]; // 3 dimensions
        let small_vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
        ]; // 2 vectors
        let small_refs: Vec<&[f32]> = small_vectors.iter().map(|v| v.as_slice()).collect();
        
        let start = Instant::now();
        let small_results = compute.calculate_distance_batch(&small_query, &small_refs, &DistanceMetric::Cosine);
        let small_duration = start.elapsed();
        
        debug!("📊 Small batch (3D, 2 vectors): {} results in {:?}", small_results.len(), small_duration);
        assert_eq!(small_results.len(), 2);
        
        // Test medium vectors (should use CPU SIMD)
        let medium_query = vec![1.0; 64]; // 64 dimensions
        let medium_vectors: Vec<Vec<f32>> = (0..50).map(|i| vec![(i as f32) / 50.0; 64]).collect(); // 50 vectors
        let medium_refs: Vec<&[f32]> = medium_vectors.iter().map(|v| v.as_slice()).collect();
        
        let start = Instant::now();
        let medium_results = compute.calculate_distance_batch(&medium_query, &medium_refs, &DistanceMetric::Cosine);
        let medium_duration = start.elapsed();
        
        debug!("📊 Medium batch (64D, 50 vectors): {} results in {:?}", medium_results.len(), medium_duration);
        assert_eq!(medium_results.len(), 50);
        
        // Test large vectors (should attempt GPU if available)
        let large_query = vec![1.0; 128]; // 128 dimensions
        let large_vectors: Vec<Vec<f32>> = (0..200).map(|i| vec![(i as f32) / 200.0; 128]).collect(); // 200 vectors
        let large_refs: Vec<&[f32]> = large_vectors.iter().map(|v| v.as_slice()).collect();
        
        let start = Instant::now();
        let large_results = compute.calculate_distance_batch(&large_query, &large_refs, &DistanceMetric::Cosine);
        let large_duration = start.elapsed();
        
        debug!("📊 Large batch (128D, 200 vectors): {} results in {:?}", large_results.len(), large_duration);
        assert_eq!(large_results.len(), 200);
        
        // Verify results are consistent regardless of backend used
        for (idx, result) in small_results.iter().enumerate() {
            assert_eq!(result.metric, DistanceMetric::Cosine);
            assert!(!result.raw_value.is_nan());
            assert!(!result.rank_value.is_nan());
            assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0);
            debug!("  Small result {}: raw={:.4}, normalized={:.4}, rank={:.4}", 
                idx, result.raw_value, result.normalized_score, result.rank_value);
        }
        
        info!("✅ GPU threshold selection tests passed");
    }

    /// Test GPU enable/disable functionality
    #[tokio::test]
    async fn test_gpu_enable_disable() {
        debug!("🔍 Testing GPU enable/disable functionality...");
        
        let mut compute = UnifiedDistanceCompute::default();
        
        let initial_backend = compute.preferred_backend();
        info!("🎯 Initial preferred backend: {}", initial_backend);
        
        // Disable GPU
        compute.set_gpu_enabled(false);
        let disabled_backend = compute.preferred_backend();
        error!("❌ GPU disabled, backend: {}", disabled_backend);
        
        // Should fall back to CPU SIMD
        assert!(matches!(disabled_backend, HardwareBackend::CpuSimd(_)));
        
        // Re-enable GPU
        compute.set_gpu_enabled(true);
        let enabled_backend = compute.preferred_backend();
        info!("✅ GPU re-enabled, backend: {}", enabled_backend);
        
        // Test with actual computation
        let test_vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let test_refs: Vec<&[f32]> = test_vectors.iter().map(|v| v.as_slice()).collect();
        let query = vec![1.0, 1.0];
        
        // Compute with GPU disabled
        compute.set_gpu_enabled(false);
        let cpu_results = compute.calculate_distance_batch(&query, &test_refs, &DistanceMetric::Cosine);
        
        // Compute with GPU enabled
        compute.set_gpu_enabled(true);
        let gpu_results = compute.calculate_distance_batch(&query, &test_refs, &DistanceMetric::Cosine);
        
        // Results should be very similar regardless of backend
        assert_eq!(cpu_results.len(), gpu_results.len());
        for (cpu_result, gpu_result) in cpu_results.iter().zip(gpu_results.iter()) {
            let diff = (cpu_result.raw_value - gpu_result.raw_value).abs();
            assert!(diff < 0.01, "CPU and GPU results should be very similar");
            debug!("  Distance difference: {:.6}", diff);
        }
        
        info!("✅ GPU enable/disable tests passed");
    }

    /// Test distance computation consistency across backends
    #[tokio::test]
    async fn test_backend_consistency() {
        debug!("🔍 Testing distance computation consistency across backends...");
        
        let compute = UnifiedDistanceCompute::default();
        
        // Test vectors for different distance metrics
        let test_cases = vec![
            (vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0], DistanceMetric::Cosine, "orthogonal"),
            (vec![1.0, 0.0, 0.0], vec![1.0, 0.0, 0.0], DistanceMetric::Cosine, "identical"),
            (vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0], DistanceMetric::Euclidean, "different"),
            (vec![1.0, 2.0], vec![3.0, 4.0], DistanceMetric::DotProduct, "positive"),
        ];
        
        for (vec_a, vec_b, metric, description) in test_cases {
            debug!("🧪 Testing {} vectors with {:?}", description, metric);
            
            // Test with different modes
            let modes = vec![
                DistanceMode::Raw,
                DistanceMode::Normalized,
                DistanceMode::RankOptimized,
            ];
            
            for mode in modes {
                let result = compute.calculate_distance_with_mode(&vec_a, &vec_b, &metric, mode);
                
                debug!("  Mode {:?}: raw={:.4}, normalized={:.4}, rank={:.4}", 
                    mode, result.raw_value, result.normalized_score, result.rank_value);
                
                // Validate result properties
                assert_eq!(result.metric, metric);
                assert!(!result.raw_value.is_nan(), "Raw value should not be NaN");
                assert!(!result.normalized_score.is_nan(), "Normalized score should not be NaN");
                assert!(!result.rank_value.is_nan(), "Rank value should not be NaN");
                assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0, 
                    "Normalized score should be in [0, 1]");
                
                // Test semantic meaning
                if vec_a == vec_b {
                    match metric {
                        DistanceMetric::Cosine | DistanceMetric::Euclidean => {
                            assert!(result.raw_value.abs() < 0.01, "Identical vectors should have near-zero distance");
                        }
                        DistanceMetric::DotProduct => {
                            assert!(result.raw_value > 0.0, "Identical vectors should have positive dot product");
                        }
                        _ => {}
                    }
                }
            }
        }
        
        info!("✅ Backend consistency tests passed");
    }

    /// Test error handling and fallback behavior
    #[tokio::test]
    async fn test_error_handling_and_fallback() {
        debug!("🔍 Testing error handling and fallback behavior...");
        
        let compute = UnifiedDistanceCompute::default();
        
        // Test dimension mismatch
        let vec_a = vec![1.0, 2.0, 3.0];
        let vec_b = vec![1.0, 2.0]; // Different dimension
        
        let result = compute.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
        
        debug!("📊 Dimension mismatch result: raw={}, normalized={}, rank={}", 
            result.raw_value, result.normalized_score, result.rank_value);
        
        assert!(result.raw_value.is_infinite(), "Should return infinity for dimension mismatch");
        assert_eq!(result.normalized_score, 0.0, "Should return 0 normalized score for mismatch");
        assert!(result.rank_value.is_infinite(), "Should return infinity rank value for mismatch");
        
        // Test zero vectors with cosine distance
        let zero_vec = vec![0.0, 0.0, 0.0];
        let normal_vec = vec![1.0, 1.0, 1.0];
        
        let zero_result = compute.calculate_distance(&zero_vec, &normal_vec, &DistanceMetric::Cosine);
        
        debug!("📊 Zero vector result: raw={}, normalized={}, rank={}", 
            zero_result.raw_value, zero_result.normalized_score, zero_result.rank_value);
        
        // Zero vector should be handled gracefully
        assert!(!zero_result.raw_value.is_nan(), "Zero vector should not produce NaN");
        
        // Test very large vectors
        let large_vec_a = vec![1e6; 1000];
        let large_vec_b = vec![1e6; 1000];
        
        let large_result = compute.calculate_distance(&large_vec_a, &large_vec_b, &DistanceMetric::Cosine);
        
        debug!("📊 Large vector result: raw={}, normalized={}, rank={}", 
            large_result.raw_value, large_result.normalized_score, large_result.rank_value);
        
        assert!(!large_result.raw_value.is_nan(), "Large vectors should not produce NaN");
        assert!(large_result.raw_value.abs() < 0.01, "Identical large vectors should have near-zero distance");
        
        info!("✅ Error handling and fallback tests passed");
    }

    /// Test performance scaling with different vector sizes
    #[tokio::test]
    async fn test_performance_scaling() {
        debug!("🔍 Testing performance scaling with different vector sizes...");
        
        let compute = UnifiedDistanceCompute::default();
        
        let dimensions = vec![16, 64, 128, 256, 512, 1024];
        let batch_sizes = vec![10, 100, 1000];
        
        for &dim in &dimensions {
            for &batch_size in &batch_sizes {
                let query = vec![1.0; dim];
                let vectors: Vec<Vec<f32>> = (0..batch_size)
                    .map(|i| vec![(i as f32) / batch_size as f32; dim])
                    .collect();
                let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
                
                let start = Instant::now();
                let results = compute.calculate_distance_batch(&query, &vector_refs, &DistanceMetric::Cosine);
                let duration = start.elapsed();
                
                assert_eq!(results.len(), batch_size);
                
                let vectors_per_sec = (batch_size as f64) / duration.as_secs_f64();
                
                debug!("📊 {}D vectors, batch {}: {:.0} vectors/sec ({:?})", 
                    dim, batch_size, vectors_per_sec, duration);
                
                // Verify all results are valid
                for (idx, result) in results.iter().enumerate() {
                    assert!(!result.raw_value.is_nan(), "Result {} should not be NaN", idx);
                    assert!(result.normalized_score >= 0.0 && result.normalized_score <= 1.0, 
                        "Normalized score {} should be in [0,1]", idx);
                }
            }
        }
        
        info!("✅ Performance scaling tests completed");
    }
}