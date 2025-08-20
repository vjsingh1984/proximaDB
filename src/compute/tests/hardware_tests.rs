// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 🔴 ENTIRE MODULE COMMENTED - hardware module doesn't exist, CpuAccelerator/RocmAccelerator not available
/*
#[cfg(test)]
mod tests {
    // 🔴 UNUSED IMPORT - hardware module commented out
    // use crate::compute::hardware::*;
    use crate::compute::ComputeBackend;
    
    #[tokio::test]
    async fn test_cpu_accelerator_initialization() {
        let mut accelerator = CpuAccelerator::new(Some(4), true);
        
        // CPU should always initialize successfully
        assert!(accelerator.initialize().await.is_ok());
        assert!(accelerator.is_available());
    }
    
    #[tokio::test]
    async fn test_cpu_accelerator_info() {
        let accelerator = CpuAccelerator::new(None, true);
        let info = accelerator.get_info();
        
        // Check basic info fields
        assert!(matches!(info.backend, ComputeBackend::CpuSIMD(_)));
        assert!(!info.device_name.is_empty());
        assert!(info.memory_total > 0);
        assert!(info.memory_free > 0);
        assert!(info.memory_free <= info.memory_total);
        assert!(info.multiprocessor_count > 0);
    }
    
    #[tokio::test]
    async fn test_batch_dot_product() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let queries = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
        ];
        
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        
        let results = accelerator.batch_dot_product(&queries, &vectors).await.unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 3);
        
        // Query 1 dot products
        assert_eq!(results[0][0], 1.0); // [1,2,3] · [1,0,0] = 1
        assert_eq!(results[0][1], 2.0); // [1,2,3] · [0,1,0] = 2
        assert_eq!(results[0][2], 3.0); // [1,2,3] · [0,0,1] = 3
        
        // Query 2 dot products
        assert_eq!(results[1][0], 4.0); // [4,5,6] · [1,0,0] = 4
        assert_eq!(results[1][1], 5.0); // [4,5,6] · [0,1,0] = 5
        assert_eq!(results[1][2], 6.0); // [4,5,6] · [0,0,1] = 6
    }
    
    #[tokio::test]
    async fn test_batch_cosine_similarity() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let queries = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
        ];
        
        let vectors = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.707, 0.707, 0.0], // 45 degree angle
        ];
        
        let results = accelerator.batch_cosine_similarity(&queries, &vectors).await.unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 3);
        
        // Note: The distance calculator returns distance, not similarity
        // For cosine: distance = 1 - similarity
        // So perfect match = 0, orthogonal = 1, 45 degrees ≈ 0.293
        
        // Query 1 distances
        assert!((results[0][0] - 0.0).abs() < 0.001); // Perfect match (distance = 0)
        assert!((results[0][1] - 1.0).abs() < 0.001); // Orthogonal (distance = 1)
        assert!((results[0][2] - 0.293).abs() < 0.01); // 45 degrees (distance ≈ 1 - 0.707)
        
        // Query 2 distances
        assert!((results[1][0] - 1.0).abs() < 0.001); // Orthogonal (distance = 1)
        assert!((results[1][1] - 0.0).abs() < 0.001); // Perfect match (distance = 0)
        assert!((results[1][2] - 0.293).abs() < 0.01); // 45 degrees (distance ≈ 1 - 0.707)
    }
    
    #[tokio::test]
    async fn test_batch_euclidean_distance() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let queries = vec![
            vec![0.0, 0.0],
            vec![3.0, 4.0],
        ];
        
        let vectors = vec![
            vec![0.0, 0.0],
            vec![3.0, 4.0],
            vec![3.0, 0.0],
            vec![0.0, 4.0],
        ];
        
        let results = accelerator.batch_euclidean_distance(&queries, &vectors).await.unwrap();
        
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 4);
        
        // Query 1 distances from origin
        assert_eq!(results[0][0], 0.0);   // Same point
        assert_eq!(results[0][1], 5.0);   // Distance to (3,4)
        assert_eq!(results[0][2], 3.0);   // Distance to (3,0)
        assert_eq!(results[0][3], 4.0);   // Distance to (0,4)
        
        // Query 2 distances from (3,4)
        assert_eq!(results[1][0], 5.0);   // Distance to origin
        assert_eq!(results[1][1], 0.0);   // Same point
        assert_eq!(results[1][2], 4.0);   // Distance to (3,0)
        assert_eq!(results[1][3], 3.0);   // Distance to (0,4)
    }
    
    #[tokio::test]
    async fn test_matrix_multiply() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        // 2x3 matrix
        let a = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
        ];
        
        // 3x2 matrix
        let b = vec![
            vec![7.0, 8.0],
            vec![9.0, 10.0],
            vec![11.0, 12.0],
        ];
        
        let result = accelerator.matrix_multiply(&a, &b).await.unwrap();
        
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 2);
        
        // Expected result:
        // [1*7 + 2*9 + 3*11, 1*8 + 2*10 + 3*12] = [58, 64]
        // [4*7 + 5*9 + 6*11, 4*8 + 5*10 + 6*12] = [139, 154]
        assert_eq!(result[0][0], 58.0);
        assert_eq!(result[0][1], 64.0);
        assert_eq!(result[1][0], 139.0);
        assert_eq!(result[1][1], 154.0);
    }
    
    #[tokio::test]
    async fn test_matrix_multiply_incompatible_dimensions() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let a = vec![vec![1.0, 2.0]]; // 1x2
        let b = vec![vec![3.0, 4.0]]; // 1x2 (incompatible)
        
        let result = accelerator.matrix_multiply(&a, &b).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains_hash("incompatible"));
    }
    
    #[tokio::test]
    async fn test_normalize_vectors() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let vectors = vec![
            vec![3.0, 4.0],           // Length 5
            vec![1.0, 0.0, 0.0],      // Already normalized
            vec![0.0, 0.0, 0.0],      // Zero vector
            vec![1.0, 1.0, 1.0, 1.0], // Length 2
        ];
        
        let normalized = accelerator.normalize_vectors(&vectors).await.unwrap();
        
        assert_eq!(normalized.len(), 4);
        
        // Check first vector: [3,4] -> [0.6, 0.8]
        assert!((normalized[0][0] - 0.6).abs() < 0.001);
        assert!((normalized[0][1] - 0.8).abs() < 0.001);
        
        // Check second vector: already normalized
        assert_eq!(normalized[1][0], 1.0);
        assert_eq!(normalized[1][1], 0.0);
        assert_eq!(normalized[1][2], 0.0);
        
        // Check zero vector: remains zero
        assert_eq!(normalized[2], vec![0.0, 0.0, 0.0]);
        
        // Check fourth vector: all components should be 0.5
        for &val in &normalized[3] {
            assert!((val - 0.5).abs() < 0.001);
        }
        
        // Verify all non-zero vectors have length 1
        for (i, vec) in normalized.iter().enumerate() {
            if i != 2 { // Skip zero vector
                let length: f32 = vec.iter().map(|&x| x * x).sum::<f32>().sqrt();
                assert!((length - 1.0).abs() < 0.001);
            }
        }
    }
    
    #[tokio::test]
    async fn test_rocm_accelerator_initialization() {
        let mut accelerator = RocmAccelerator::new(0);
        
        // Should fail if ROCm is not available (which it usually isn't in CI)
        let result = accelerator.initialize().await;
        
        // In most test environments, ROCm won't be available
        if result.is_err() {
            assert!(result.unwrap_err().contains_hash("ROCm"));
        }
        
        assert!(!accelerator.is_available());
    }
    
    #[tokio::test]
    async fn test_rocm_accelerator_info() {
        let accelerator = RocmAccelerator::new(1);
        let info = accelerator.get_info();
        
        assert!(matches!(info.backend, ComputeBackend::ROCm));
        assert_eq!(info.device_name, "ROCm Device 1");
        assert!(info.memory_total > 0);
        assert!(info.compute_capability.is_some());
    }
    
    #[tokio::test]
    async fn test_create_accelerator_factory() {
        // Test CPU backend
        let cpu_accel = create_accelerator(ComputeBackend::CpuSIMD(crate::compute::distance_computation::PlatformCapability::X86Avx2));
        assert!(cpu_accel.is_available());
        
        // Test ROCm backend
        let rocm_accel = create_accelerator(ComputeBackend::ROCm);
        let info = rocm_accel.get_info();
        assert!(matches!(info.backend, ComputeBackend::ROCm));
        
        // Test default (should be CPU)
        let default_accel = create_accelerator(ComputeBackend::CpuSIMD(crate::compute::distance_computation::PlatformCapability::X86Avx2));
        assert!(default_accel.is_available());
    }
    
    #[tokio::test]
    async fn test_empty_batch_operations() {
        let mut accelerator = CpuAccelerator::new(Some(2), true);
        accelerator.initialize().await.unwrap();
        
        let empty_queries: Vec<Vec<f32>> = vec![];
        let empty_vectors: Vec<Vec<f32>> = vec![];
        
        // All batch operations should handle empty inputs gracefully
        let dot_result = accelerator.batch_dot_product(&empty_queries, &empty_vectors).await.unwrap();
        assert!(dot_result.is_empty());
        
        let cosine_result = accelerator.batch_cosine_similarity(&empty_queries, &empty_vectors).await.unwrap();
        assert!(cosine_result.is_empty());
        
        let euclidean_result = accelerator.batch_euclidean_distance(&empty_queries, &empty_vectors).await.unwrap();
        assert!(euclidean_result.is_empty());
        
        let normalize_result = accelerator.normalize_vectors(&empty_vectors).await.unwrap();
        assert!(normalize_result.is_empty());
        
        let matrix_result = accelerator.matrix_multiply(&empty_queries, &empty_vectors).await.unwrap();
        assert!(matrix_result.is_empty());
    }
    
    // 🔴 UNUSED TEST - HardwareInfo struct doesn't exist (hardware module commented out)
    // #[test]
    // fn test_hardware_info_struct() {
    //     let info = HardwareInfo {
    //         backend: ComputeBackend::CpuSIMD(crate::compute::distance_computation::PlatformCapability::X86Avx2),
    //         device_name: "Test CPU".to_string(),
    //         memory_total: 16 * 1024 * 1024 * 1024,
    //         memory_free: 8 * 1024 * 1024 * 1024,
    //         compute_capability: Some("AVX2".to_string()),
            max_threads_per_block: None,
            multiprocessor_count: Some(8),
        };
        
        // Test Debug trait
        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains_hash("Test CPU"));
        
        // Test Clone trait
        let cloned = info.clone();
        assert_eq!(cloned.device_name, info.device_name);
        assert_eq!(cloned.memory_total, info.memory_total);
    }
}
*/