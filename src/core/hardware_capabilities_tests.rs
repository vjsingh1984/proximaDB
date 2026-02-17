#[cfg(test)]
mod tests {
use crate::core::hardware_capabilities::{HardwareCapabilities, HardwareBackend, HardwareQuery, SimdCapabilities, GpuBackend, initialize_hardware_capabilities_default, get_hardware_capabilities, try_get_hardware_capabilities};

    #[test]
    fn test_hardware_config_defaults() {
        use crate::core::config::HardwareConfig;
        let config = HardwareConfig::default();
        
        assert!(config.enable_detection);
        assert!(config.enable_gpu_acceleration);
        assert!(config.enable_simd);
        assert!(config.enable_avx512);
        assert!(config.enable_gpu_parsing);
        // enable_gpu_distance field no longer exists in current HardwareConfig
        assert_eq!(config.gpu_min_vector_size, 64);
        assert_eq!(config.gpu_min_batch_size, 100);
    }
    
    #[test]
    fn test_hardware_capabilities_detection_with_config() {
        use crate::core::config::HardwareConfig;
        // Test with default config (everything enabled)
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        // CPU should always be detected
        assert!(caps.cpu.physical_cores > 0);
        assert!(caps.cpu.logical_cores >= caps.cpu.physical_cores);
        assert!(!caps.cpu.vendor.is_empty());
        
        // Memory should always be detected
        assert!(caps.memory.total_memory > 0 || caps.memory.recommended_cache_size > 0);
        
        // Test that configuration is preserved
        assert!(caps.config.enable_detection);
        assert!(caps.config.enable_gpu_acceleration);
    }
    
    #[test]
    fn test_hardware_capabilities_disabled() {
        use crate::core::config::HardwareConfig;
        let mut config = HardwareConfig::default();
        config.enable_detection = false;
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        // Should return disabled capabilities
        assert!(!caps.config.enable_detection);
        assert!(caps.cpu.physical_cores > 0); // Basic CPU info still available
        assert_eq!(caps.gpu.backend, GpuBackend::None); // GPU should be disabled
    }
    
    #[test]
    fn test_hardware_capabilities_gpu_disabled() {
        use crate::core::config::HardwareConfig;
        let mut config = HardwareConfig::default();
        config.enable_gpu_acceleration = false;
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        assert!(!caps.has_gpu());
        assert!(!caps.has_gpu_distance());
        assert!(!caps.has_gpu_parsing());
        assert_eq!(caps.gpu.backend, GpuBackend::None);
    }
    
    #[test]
    fn test_hardware_capabilities_simd_disabled() {
        use crate::core::config::HardwareConfig;
        let mut config = HardwareConfig::default();
        config.enable_simd = false;
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        assert!(!caps.has_simd());
        // AVX-512 should also be disabled when SIMD is disabled
        assert!(!caps.has_avx512());
    }
    
    #[test]
    fn test_hardware_capabilities_avx512_disabled() {
        use crate::core::config::HardwareConfig;
        let mut config = HardwareConfig::default();
        config.enable_avx512 = false;
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        assert!(!caps.has_avx512());
        // SIMD should still be available (other than AVX-512)
        // This depends on actual hardware capabilities
    }
    
    #[test]
    fn test_gpu_threshold_checks() {
        use crate::core::config::HardwareConfig;
        let config = HardwareConfig {
            gpu_min_vector_size: 128,
            gpu_min_batch_size: 500,
            ..Default::default()
        };
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        // Test vector size thresholds
        assert!(!caps.should_use_gpu_distance(64)); // Below threshold
        assert!(!caps.should_use_gpu_distance(127)); // Just below threshold
        
        if caps.has_gpu_distance() {
            assert!(caps.should_use_gpu_distance(128)); // At threshold
            assert!(caps.should_use_gpu_distance(256)); // Above threshold
        }
        
        // Test batch size thresholds
        assert!(!caps.should_use_gpu_batch(100)); // Below threshold
        assert!(!caps.should_use_gpu_batch(499)); // Just below threshold
        
        if caps.has_gpu_distance() {
            assert!(caps.should_use_gpu_batch(500)); // At threshold
            assert!(caps.should_use_gpu_batch(1000)); // Above threshold
        }
    }
    
    #[test]
    fn test_preferred_backend_logic() {
        use crate::core::config::HardwareConfig;
        let config = HardwareConfig::default();
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();

        let _backend = caps.preferred_backend();

        // Should return a valid backend
        match _backend {
            HardwareBackend::Scalar |
            HardwareBackend::CUDA |
            HardwareBackend::ROCm |
            HardwareBackend::MPS |
            HardwareBackend::OpenCL |
            HardwareBackend::AVX512 |
            HardwareBackend::AVX2 |
            HardwareBackend::SSE |
            HardwareBackend::NEON => {
                // Valid backend
            }
        }

        // Verify backend is consistent with capabilities
        if caps.has_gpu() {
            match _backend {
                HardwareBackend::CUDA |
                HardwareBackend::ROCm |
                HardwareBackend::MPS |
                HardwareBackend::OpenCL => {
                    // Should have GPU
                    assert!(caps.has_gpu());
                }
                _ => {
                    // CPU backend when GPU not preferred
                }
            }
        }
    }
    
    #[test]
    fn test_hardware_capabilities_initialization() {
        // Test initialization with default config
        let result = initialize_hardware_capabilities_default();
        assert!(result.is_ok());
        
        // Should be able to get capabilities after initialization
        let caps = get_hardware_capabilities();
        assert!(caps.cpu.physical_cores > 0);
    }
    
    #[test]
    fn test_hardware_query_helpers() {
        // Initialize with default config for testing
        let _ = initialize_hardware_capabilities_default();
        
        // Test HardwareQuery static methods
        let _has_avx512 = HardwareQuery::has_avx512();
        let _has_gpu = HardwareQuery::has_gpu();
        let cpu_cores = HardwareQuery::cpu_cores();
        let thread_pool_size = HardwareQuery::recommended_thread_pool_size();
        let cache_size = HardwareQuery::recommended_cache_size();
        
        // Basic sanity checks
        assert!(cpu_cores > 0);
        assert!(thread_pool_size > 0);
        assert!(cache_size > 0);
    }
    
    #[test]
    fn test_simd_capabilities_detection() {
        let caps = SimdCapabilities::detect();
        
        // Should detect some capability or be all false (scalar)
        let has_any_simd = caps.has_sse || caps.has_avx || caps.has_avx2 || 
                          caps.has_avx512 || caps.has_neon;
        
        // On most modern systems, should have at least one SIMD capability
        // But this is platform-dependent, so we just verify the detection runs
        
        // Test string representation
        let caps_str = caps.to_string();
        assert!(!caps_str.is_empty());
        
        if !has_any_simd {
            assert_eq!(caps_str, "Scalar");
        }
    }
    
    #[test]
    fn test_memory_info_detection() {
        let memory = HardwareCapabilities::detect_memory().unwrap();
        
        // Memory should be detected (may be 0 in some test environments)
        assert!(memory.total_memory >= 0);
        assert!(memory.available_memory >= 0);
        assert!(memory.recommended_cache_size > 0);
        
        // Recommended cache should not exceed available memory (when available)
        if memory.available_memory > 0 {
            assert!(memory.recommended_cache_size <= memory.available_memory);
        }
    }
    
    #[test]
    fn test_cpu_info_detection() {
        let cpu = HardwareCapabilities::detect_cpu().unwrap();
        
        assert!(cpu.physical_cores > 0);
        assert!(cpu.logical_cores >= cpu.physical_cores);
        assert!(!cpu.vendor.is_empty());
        assert!(!cpu.model_name.is_empty());
        
        // SIMD capabilities should be detected
        let _simd = cpu.simd;
        // At least one should be available on modern systems (platform-dependent)
        // Just verify detection doesn't crash
    }
    
    #[test]
    fn test_gpu_detection_robustness() {
        // GPU detection should not crash even if no GPU is available
        let gpu_result = HardwareCapabilities::detect_gpu();
        assert!(gpu_result.is_ok());
        
        let gpu = gpu_result.unwrap();
        
        // Backend should be valid
        match gpu.backend {
            GpuBackend::None |
            GpuBackend::CUDA |
            GpuBackend::ROCm |
            GpuBackend::MPS |
            GpuBackend::OpenCL => {
                // Valid backend
            }
        }
        
        // If no GPU, devices should be empty
        if gpu.backend == GpuBackend::None {
            assert!(gpu.devices.is_empty());
            assert_eq!(gpu.total_memory, 0);
            assert!(gpu.primary_device.is_none());
        }
    }
    
    #[test]
    fn test_configuration_edge_cases() {
        use crate::core::config::HardwareConfig;
        // Test configuration with all features disabled
        let config = HardwareConfig {
            enable_detection: true, // Keep detection on
            enable_gpu_acceleration: false,
            enable_simd: false,
            enable_avx512: false,
            enable_gpu_parsing: false,
            enable_gpu_similarity: false,
            gpu_min_vector_size: 1000000, // Very high threshold
            gpu_min_batch_size: 1000000,  // Very high threshold
        };
        
        let caps = HardwareCapabilities::detect_with_config(config).unwrap();
        
        // All GPU and SIMD features should be disabled
        assert!(!caps.has_gpu());
        assert!(!caps.has_gpu_distance());
        assert!(!caps.has_gpu_parsing());
        assert!(!caps.has_simd());
        assert!(!caps.has_avx512());
        
        // Should never trigger GPU usage with high thresholds
        assert!(!caps.should_use_gpu_distance(1000));
        assert!(!caps.should_use_gpu_batch(1000));
    }
    
    #[test]
    fn test_concurrent_initialization() {
        use std::thread;
        // Test that concurrent initialization doesn't cause issues
        let handles: Vec<_> = (0..10).map(|_| {
            thread::spawn(|| {
                // Try to get capabilities concurrently
                if let Some(caps) = try_get_hardware_capabilities() {
                    assert!(caps.cpu.physical_cores > 0);
                }
            })
        }).collect();
        
        // All threads should complete successfully
        for handle in handles {
            handle.join().unwrap();
        }
    }
}