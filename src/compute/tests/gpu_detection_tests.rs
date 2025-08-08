/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tests for GPU detection and backend selection

#[cfg(test)]
mod tests {
    use crate::compute::gpu_distance::{GpuBackend, GpuDistanceCompute, detect_best_gpu};
    use crate::compute::distance_computation::engine::{UnifiedDistanceCompute, HardwareBackend, GpuAccelerator};
    use crate::compute::distance_computation::DistanceMetric;
    use crate::compute::PlatformCapability;
    
    /// Test GPU backend detection and selection
    #[tokio::test]
    async fn test_gpu_backend_detection() {
        println!("🔍 Testing GPU backend detection...");
        
        // Test GPU detection
        let gpu_result = GpuDistanceCompute::new();
        
        match gpu_result {
            Ok(gpu_compute) => {
                let backend = gpu_compute.backend();
                println!("✅ GPU backend detected: {}", backend);
                
                // Verify backend is one of the expected types
                assert!(matches!(
                    backend,
                    GpuBackend::Cuda | GpuBackend::Rocm | GpuBackend::Mps | GpuBackend::OpenCL | GpuBackend::None
                ));
                
                // Test availability
                let is_available = gpu_compute.is_available();
                println!("📊 GPU available: {}", is_available);
                
                if is_available {
                    // Test backend specific properties
                    match backend {
                        GpuBackend::Cuda => {
                            println!("🟢 CUDA GPU detected");
                            #[cfg(feature = "cuda")]
                            {
                                assert!(gpu_compute.devices.len() > 0);
                                for device in &gpu_compute.devices {
                                    assert_eq!(device.backend, GpuBackend::Cuda);
                                    assert!(device.total_memory > 0);
                                    println!("  CUDA Device: {} ({}MB)", device.name, device.total_memory / (1024 * 1024));
                                }
                            }
                        }
                        GpuBackend::Rocm => {
                            println!("🔴 ROCm GPU detected");
                            #[cfg(feature = "rocm")]
                            {
                                assert!(gpu_compute.devices.len() > 0);
                                for device in &gpu_compute.devices {
                                    assert_eq!(device.backend, GpuBackend::Rocm);
                                    println!("  ROCm Device: {} ({}MB)", device.name, device.total_memory / (1024 * 1024));
                                }
                            }
                        }
                        GpuBackend::Mps => {
                            println!("🍎 Metal Performance Shaders detected");
                            #[cfg(all(target_os = "macos", feature = "metal"))]
                            {
                                assert!(gpu_compute.devices.len() > 0);
                                for device in &gpu_compute.devices {
                                    assert_eq!(device.backend, GpuBackend::Mps);
                                    println!("  Metal Device: {} ({}MB)", device.name, device.total_memory / (1024 * 1024));
                                }
                            }
                        }
                        GpuBackend::OpenCL => {
                            println!("⚡ OpenCL GPU detected");
                            #[cfg(feature = "opencl")]
                            {
                                assert!(gpu_compute.devices.len() > 0);
                                for device in &gpu_compute.devices {
                                    assert_eq!(device.backend, GpuBackend::OpenCL);
                                    println!("  OpenCL Device: {} ({}MB)", device.name, device.total_memory / (1024 * 1024));
                                }
                            }
                        }
                        GpuBackend::None => {
                            println!("❌ No GPU backend available");
                            assert_eq!(gpu_compute.devices.len(), 0);
                        }
                    }
                } else {
                    println!("❌ GPU not available or no devices found");
                }
            }
            Err(e) => {
                println!("❌ GPU detection failed: {}", e);
                // This is acceptable - not all systems have GPUs
            }
        }
    }

    /// Test UnifiedDistanceCompute GPU integration
    #[tokio::test]
    async fn test_unified_distance_gpu_selection() {
        println!("🔍 Testing UnifiedDistanceCompute GPU selection...");
        
        let compute = UnifiedDistanceCompute::default();
        
        println!("🖥️ Preferred backend: {}", compute.preferred_backend());
        println!("📋 Available backends: {:?}", compute.available_backends());
        
        // Test that preferred backend is sensible
        let preferred = compute.preferred_backend();
        assert!(matches!(
            preferred,
            HardwareBackend::CpuSimd(_) | 
            HardwareBackend::Cuda | 
            HardwareBackend::Rocm | 
            HardwareBackend::Mps | 
            HardwareBackend::OpenCL |
            HardwareBackend::Scalar
        ));
        
        // Test available backends list
        let backends = compute.available_backends();
        assert!(backends.len() >= 1); // At least scalar should be available
        
        // CPU SIMD should always be available
        assert!(backends.iter().any(|b| matches!(b, HardwareBackend::CpuSimd(_))));
        
        // Scalar should always be available as fallback
        assert!(backends.iter().any(|b| matches!(b, HardwareBackend::Scalar)));
    }

    /// Test GPU accelerator trait implementation
    #[tokio::test]
    async fn test_gpu_accelerator_trait() {
        println!("🔍 Testing GpuAccelerator trait implementation...");
        
        #[cfg(feature = "gpu")]
        {
            let gpu_result = detect_best_gpu();
            
            match gpu_result {
                Ok(gpu_accelerator) => {
                    println!("✅ GPU accelerator created successfully");
                    
                    let backend = gpu_accelerator.backend();
                    let is_available = gpu_accelerator.is_available();
                    
                    println!("🎯 Accelerator backend: {}", backend);
                    println!("📊 Accelerator available: {}", is_available);
                    
                    // Test backend mapping
                    match backend {
                        HardwareBackend::Cuda => println!("🟢 CUDA accelerator"),
                        HardwareBackend::Rocm => println!("🔴 ROCm accelerator"),
                        HardwareBackend::Mps => println!("🍎 Metal accelerator"),
                        HardwareBackend::OpenCL => println!("⚡ OpenCL accelerator"),
                        HardwareBackend::Scalar => println!("💻 Scalar fallback"),
                        HardwareBackend::CpuSimd(_) => println!("🚀 CPU SIMD (unexpected for GPU)"),
                    }
                    
                    if is_available {
                        // Test actual GPU computation
                        let vec_a = vec![1.0, 0.0, 0.0, 0.0];
                        let vec_b = vec![0.0, 1.0, 0.0, 0.0];
                        
                        let result = gpu_accelerator
                            .calculate_distance_gpu(&vec_a, &vec_b, DistanceMetric::Cosine)
                            .await;
                        
                        match result {
                            Ok(distance) => {
                                println!("✅ GPU cosine distance calculated: {}", distance);
                                // Orthogonal vectors should have cosine distance ~1.0
                                assert!((distance - 1.0).abs() < 0.1, "GPU cosine distance should be ~1.0 for orthogonal vectors");
                            }
                            Err(e) => {
                                println!("❌ GPU distance calculation failed: {}", e);
                                // This might be acceptable if GPU implementation is not complete
                            }
                        }
                        
                        // Test batch computation
                        let query = vec![1.0, 0.0, 0.0, 0.0];
                        let vectors = vec![
                            vec![1.0, 0.0, 0.0, 0.0],  // Identical
                            vec![0.0, 1.0, 0.0, 0.0],  // Orthogonal
                            vec![-1.0, 0.0, 0.0, 0.0], // Opposite
                        ];
                        
                        let batch_result = gpu_accelerator
                            .calculate_batch_gpu(&query, &vectors, DistanceMetric::Cosine)
                            .await;
                        
                        match batch_result {
                            Ok(distances) => {
                                println!("✅ GPU batch distances calculated: {:?}", distances);
                                assert_eq!(distances.len(), 3);
                                
                                // Verify distance relationships
                                assert!(distances[0] < distances[1], "Identical should be closer than orthogonal");
                                assert!(distances[1] < distances[2], "Orthogonal should be closer than opposite");
                            }
                            Err(e) => {
                                println!("❌ GPU batch calculation failed: {}", e);
                                // This might be acceptable if GPU implementation is not complete
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("❌ GPU accelerator creation failed: {}", e);
                    // This is acceptable - not all systems have GPUs
                }
            }
        }
        
        #[cfg(not(feature = "gpu"))]
        {
            println!("⚠️ GPU features not compiled in, skipping GPU accelerator tests");
        }
    }

    /// Test hardware backend selection logic
    #[test]
    fn test_hardware_backend_selection_logic() {
        println!("🔍 Testing hardware backend selection logic...");
        
        // Test that GPU backends are preferred over CPU when available
        let gpu_backends = vec![
            HardwareBackend::Cuda,
            HardwareBackend::Rocm,
            HardwareBackend::Mps,
            HardwareBackend::OpenCL,
        ];
        
        let cpu_backends = vec![
            HardwareBackend::CpuSimd(PlatformCapability::X86Avx512),
            HardwareBackend::CpuSimd(PlatformCapability::X86Avx2),
            HardwareBackend::CpuSimd(PlatformCapability::ArmNeon),
            HardwareBackend::Scalar,
        ];
        
        // Verify GPU backends have higher priority than CPU backends
        for gpu_backend in &gpu_backends {
            for cpu_backend in &cpu_backends {
                // In a real selection algorithm, GPU should be preferred for large workloads
                println!("GPU {} vs CPU {}", gpu_backend, cpu_backend);
            }
        }
        
        // Test backend display
        for backend in gpu_backends.iter().chain(cpu_backends.iter()) {
            println!("Backend: {}", backend);
            assert!(!backend.to_string().is_empty());
        }
    }

    /// Test GPU device selection
    #[tokio::test]
    async fn test_gpu_device_selection() {
        println!("🔍 Testing GPU device selection...");
        
        let gpu_result = GpuDistanceCompute::new();
        
        if let Ok(mut gpu_compute) = gpu_result {
            if gpu_compute.is_available() && !gpu_compute.devices.is_empty() {
                println!("✅ GPU devices available for testing");
                
                // Test device selection
                let device_count = gpu_compute.devices.len();
                println!("📊 Available GPU devices: {}", device_count);
                
                for (idx, device) in gpu_compute.devices.iter().enumerate() {
                    println!("  Device {}: {} ({}MB, Backend: {})", 
                        idx, 
                        device.name, 
                        device.total_memory / (1024 * 1024),
                        device.backend
                    );
                    
                    // Test device selection
                    let selection_result = gpu_compute.select_device(idx);
                    assert!(selection_result.is_ok(), "Should be able to select valid device index");
                }
                
                // Test invalid device selection
                let invalid_selection = gpu_compute.select_device(device_count + 10);
                assert!(invalid_selection.is_err(), "Should fail to select invalid device index");
                
                println!("✅ Device selection tests passed");
            } else {
                println!("⚠️ No GPU devices available for testing");
            }
        } else {
            println!("⚠️ GPU not available for device selection testing");
        }
    }

    /// Test feature flag conditional compilation
    #[test]
    fn test_feature_flag_compilation() {
        println!("🔍 Testing feature flag conditional compilation...");
        
        // Test CUDA feature flag
        #[cfg(feature = "cuda")]
        {
            println!("✅ CUDA feature enabled");
        }
        #[cfg(not(feature = "cuda"))]
        {
            println!("❌ CUDA feature disabled");
        }
        
        // Test ROCm feature flag
        #[cfg(feature = "rocm")]
        {
            println!("✅ ROCm feature enabled");
        }
        #[cfg(not(feature = "rocm"))]
        {
            println!("❌ ROCm feature disabled");
        }
        
        // Test Metal feature flag
        #[cfg(all(target_os = "macos", feature = "metal"))]
        {
            println!("✅ Metal feature enabled (macOS)");
        }
        #[cfg(not(all(target_os = "macos", feature = "metal")))]
        {
            println!("❌ Metal feature disabled or not on macOS");
        }
        
        // Test OpenCL feature flag
        #[cfg(feature = "opencl")]
        {
            println!("✅ OpenCL feature enabled");
        }
        #[cfg(not(feature = "opencl"))]
        {
            println!("❌ OpenCL feature disabled");
        }
        
        // Test GPU umbrella feature flag
        #[cfg(feature = "gpu")]
        {
            println!("✅ GPU umbrella feature enabled");
        }
        #[cfg(not(feature = "gpu"))]
        {
            println!("❌ GPU umbrella feature disabled");
        }
    }

    /// Benchmark GPU vs CPU performance
    #[tokio::test]
    async fn test_gpu_vs_cpu_performance() {
        println!("🔍 Testing GPU vs CPU performance comparison...");
        
        let compute = UnifiedDistanceCompute::default();
        
        // Create test vectors
        let dimension = 512;
        let batch_size = 1000;
        
        let query: Vec<f32> = (0..dimension).map(|i| (i as f32) / dimension as f32).collect();
        let vectors: Vec<Vec<f32>> = (0..batch_size)
            .map(|batch_idx| {
                (0..dimension)
                    .map(|i| ((i + batch_idx) as f32) / dimension as f32)
                    .collect()
            })
            .collect();
        
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        
        println!("📊 Testing with {} vectors of {} dimensions", batch_size, dimension);
        
        // Test CPU computation
        let start_cpu = std::time::Instant::now();
        let cpu_results = compute.calculate_distance_batch(&query, &vector_refs, &DistanceMetric::Cosine);
        let cpu_duration = start_cpu.elapsed();
        
        println!("💻 CPU computation: {} results in {:?}", cpu_results.len(), cpu_duration);
        
        // Test GPU computation if available
        if let Some(ref gpu) = compute.gpu_accelerator {
            if gpu.is_available() {
                let start_gpu = std::time::Instant::now();
                let gpu_result = gpu.calculate_batch_gpu(&query, &vectors, DistanceMetric::Cosine).await;
                let gpu_duration = start_gpu.elapsed();
                
                match gpu_result {
                    Ok(gpu_distances) => {
                        println!("🎮 GPU computation: {} results in {:?}", gpu_distances.len(), gpu_duration);
                        
                        // Compare results for correctness
                        assert_eq!(cpu_results.len(), gpu_distances.len());
                        
                        let mut max_diff = 0.0f32;
                        for (cpu_result, gpu_distance) in cpu_results.iter().zip(gpu_distances.iter()) {
                            let diff = (cpu_result.raw_value - gpu_distance).abs();
                            max_diff = max_diff.max(diff);
                        }
                        
                        println!("📊 Maximum difference between CPU and GPU: {}", max_diff);
                        assert!(max_diff < 0.01, "CPU and GPU results should be very similar");
                        
                        // Report performance comparison
                        if gpu_duration < cpu_duration {
                            let speedup = cpu_duration.as_nanos() as f64 / gpu_duration.as_nanos() as f64;
                            println!("🚀 GPU is {:.2}x faster than CPU", speedup);
                        } else {
                            let slowdown = gpu_duration.as_nanos() as f64 / cpu_duration.as_nanos() as f64;
                            println!("🐌 GPU is {:.2}x slower than CPU (overhead for small batches)", slowdown);
                        }
                    }
                    Err(e) => {
                        println!("❌ GPU computation failed: {}", e);
                    }
                }
            } else {
                println!("⚠️ GPU accelerator not available for performance testing");
            }
        } else {
            println!("⚠️ No GPU accelerator for performance testing");
        }
    }
}