/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! GPU-Accelerated Distance Computation for ProximaDB
//!
//! This module provides GPU acceleration for distance calculations using:
//! - CUDA for NVIDIA GPUs
//! - ROCm for AMD GPUs
//! - Metal Performance Shaders (MPS) for Apple Silicon
//! - OpenCL for cross-platform GPU support
//!
//! The module automatically detects available GPU backends and selects
//! the most appropriate one for optimal performance.

use anyhow::{anyhow, Result};
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::similarity::{DistanceCompute, DistanceMetric};
use crate::compute::distance_computation::engine::{GpuAccelerator, HardwareBackend};
use crate::core::hardware_capabilities::GpuBackend;

// Using central GpuBackend enum from hardware_capabilities module

// Using central GpuDevice struct from hardware_capabilities module
pub use crate::core::hardware_capabilities::GpuDevice;

/// GPU distance computation manager
pub struct GpuDistanceCompute {
    /// Selected GPU backend
    backend: GpuBackend,
    /// Available GPU devices
    devices: Vec<GpuDevice>,
    /// Selected device index
    selected_device: Option<usize>,
    /// GPU memory pool size
    memory_pool_size: usize,
}

/// Detect and return the best available GPU accelerator
pub fn detect_best_gpu() -> Result<impl GpuAccelerator> {
    GpuDistanceCompute::new()
}

// Implement GpuAccelerator trait for GpuDistanceCompute
#[async_trait::async_trait]
impl GpuAccelerator for GpuDistanceCompute {
    fn backend(&self) -> HardwareBackend {
        match self.backend {
            GpuBackend::Cuda => HardwareBackend::Cuda,
            GpuBackend::Rocm => HardwareBackend::Rocm,
            GpuBackend::Mps => HardwareBackend::Mps,
            GpuBackend::OpenCL => HardwareBackend::OpenCL,
            GpuBackend::None => HardwareBackend::Scalar,
        }
    }
    
    fn is_available(&self) -> bool {
        self.backend != GpuBackend::None && !self.devices.is_empty()
    }
    
    async fn calculate_distance_gpu(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32> {
        self.calculate_distance_gpu_internal(vec_a, vec_b, metric).await
    }
    
    async fn calculate_batch_gpu(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        self.calculate_batch_gpu_internal(query, vectors, metric).await
    }
}

impl GpuDistanceCompute {
    /// Create a new GPU distance compute manager
    pub fn new() -> Result<Self> {
        let (backend, devices) = Self::detect_gpu_backend()?;
        
        info!("🚀 GPU backend detected: {}", backend);
        for (idx, device) in devices.iter().enumerate() {
            info!(
                "  Device {}: {} ({}MB memory)",
                idx,
                device.name,
                device.total_memory / (1024 * 1024)
            );
        }

        let selected_device = if !devices.is_empty() {
            Some(0) // Select first device by default
        } else {
            None
        };

        Ok(Self {
            backend,
            devices,
            selected_device,
            memory_pool_size: 1024 * 1024 * 1024, // 1GB default
        })
    }

    /// Detect available GPU backend and devices
    fn detect_gpu_backend() -> Result<(GpuBackend, Vec<GpuDevice>)> {
        // Try CUDA first (most common)
        #[cfg(feature = "cuda")]
        if let Ok(devices) = Self::detect_cuda_devices() {
            if !devices.is_empty() {
                return Ok((GpuBackend::Cuda, devices));
            }
        }

        // Try ROCm for AMD GPUs
        #[cfg(feature = "rocm")]
        if let Ok(devices) = Self::detect_rocm_devices() {
            if !devices.is_empty() {
                return Ok((GpuBackend::Rocm, devices));
            }
        }

        // Try Metal Performance Shaders on macOS
        #[cfg(all(target_os = "macos", feature = "metal"))]
        if let Ok(devices) = Self::detect_mps_devices() {
            if !devices.is_empty() {
                return Ok((GpuBackend::Mps, devices));
            }
        }

        // Fall back to OpenCL
        #[cfg(feature = "opencl")]
        if let Ok(devices) = Self::detect_opencl_devices() {
            if !devices.is_empty() {
                return Ok((GpuBackend::OpenCL, devices));
            }
        }

        Ok((GpuBackend::None, vec![]))
    }

    /// Select a specific GPU device
    pub fn select_device(&mut self, device_idx: usize) -> Result<()> {
        if device_idx >= self.devices.len() {
            return Err(anyhow!("Invalid device index: {}", device_idx));
        }
        self.selected_device = Some(device_idx);
        info!("Selected GPU device: {}", self.devices[device_idx].name);
        Ok(())
    }

    /// Check if GPU acceleration is available
    pub fn is_available(&self) -> bool {
        self.backend != GpuBackend::None && !self.devices.is_empty()
    }

    /// Get the current backend
    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    /// Calculate distance on GPU
    async fn calculate_distance_gpu_internal(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32> {
        if !self.is_available() {
            return Err(anyhow!("No GPU available"));
        }

        match self.backend {
            GpuBackend::Cuda => self.calculate_distance_cuda(vec_a, vec_b, metric).await,
            GpuBackend::Rocm => self.calculate_distance_rocm(vec_a, vec_b, metric).await,
            GpuBackend::Mps => self.calculate_distance_mps(vec_a, vec_b, metric).await,
            GpuBackend::OpenCL => self.calculate_distance_opencl(vec_a, vec_b, metric).await,
            GpuBackend::None => Err(anyhow!("No GPU backend available")),
        }
    }

    /// Calculate batch distances on GPU
    async fn calculate_batch_gpu_internal(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        if !self.is_available() {
            return Err(anyhow!("No GPU available"));
        }

        match self.backend {
            GpuBackend::Cuda => self.calculate_batch_cuda(query, vectors, metric).await,
            GpuBackend::Rocm => self.calculate_batch_rocm(query, vectors, metric).await,
            GpuBackend::Mps => self.calculate_batch_mps(query, vectors, metric).await,
            GpuBackend::OpenCL => self.calculate_batch_opencl(query, vectors, metric).await,
            GpuBackend::None => Err(anyhow!("No GPU backend available")),
        }
    }
}

// CUDA implementation
#[cfg(feature = "cuda")]
impl GpuDistanceCompute {
    fn detect_cuda_devices() -> Result<Vec<GpuDevice>> {
        use cudarc::driver::{CudaDevice, CudaDeviceBuilder};
        
        let device_count = CudaDevice::count()?;
        let mut devices = Vec::new();

        for i in 0..device_count {
            let cuda_device = CudaDeviceBuilder::new(i as i32).build()?;
            let props = cuda_device.get_properties()?;
            
            devices.push(GpuDevice {
                id: i,
                name: props.name.clone(),
                total_memory: props.total_global_mem,
                available_memory: cuda_device.free_memory()?,
                compute_capability: Some((props.major, props.minor)),
                backend: GpuBackend::Cuda,
            });
        }

        Ok(devices)
    }

    async fn calculate_distance_cuda(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32> {
        use cudarc::driver::{CudaDevice, CudaSlice, LaunchConfig};
        
        let device = CudaDevice::new(self.selected_device.unwrap_or(0) as i32)?;
        
        // Allocate GPU memory
        let gpu_a = device.htod_copy(vec_a)?;
        let gpu_b = device.htod_copy(vec_b)?;
        let mut gpu_result = device.alloc_zeros::<f32>(1)?;
        
        // Select kernel based on metric
        let kernel_name = match metric {
            DistanceMetric::Cosine => "cosine_distance_kernel",
            DistanceMetric::Euclidean => "euclidean_distance_kernel",
            DistanceMetric::DotProduct => "dot_product_kernel",
            _ => return Err(anyhow!("Unsupported metric for CUDA: {:?}", metric)),
        };
        
        // Load and launch kernel
        let module = device.load_ptx(include_str!("kernels/distance.ptx"))?;
        let kernel = module.get_function(kernel_name)?;
        
        let block_size = 256;
        let grid_size = (vec_a.len() + block_size - 1) / block_size;
        
        kernel.launch(
            LaunchConfig {
                grid_dim: (grid_size as u32, 1, 1),
                block_dim: (block_size as u32, 1, 1),
                shared_mem_bytes: 0,
            },
            (&gpu_a, &gpu_b, vec_a.len() as i32, &mut gpu_result),
        )?;
        
        // Copy result back
        let result = device.dtoh_copy(&gpu_result)?;
        Ok(result[0])
    }

    async fn calculate_batch_cuda(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        use cudarc::driver::{CudaDevice, LaunchConfig};
        
        let device = CudaDevice::new(self.selected_device.unwrap_or(0) as i32)?;
        let num_vectors = vectors.len();
        let dimension = query.len();
        
        // Flatten vectors for GPU transfer
        let flattened: Vec<f32> = vectors.iter().flatten().cloned().collect();
        
        // Allocate GPU memory
        let gpu_query = device.htod_copy(query)?;
        let gpu_vectors = device.htod_copy(&flattened)?;
        let mut gpu_results = device.alloc_zeros::<f32>(num_vectors)?;
        
        // Select batch kernel
        let kernel_name = match metric {
            DistanceMetric::Cosine => "cosine_distance_batch_kernel",
            DistanceMetric::Euclidean => "euclidean_distance_batch_kernel",
            DistanceMetric::DotProduct => "dot_product_batch_kernel",
            _ => return Err(anyhow!("Unsupported metric for CUDA: {:?}", metric)),
        };
        
        let module = device.load_ptx(include_str!("kernels/distance.ptx"))?;
        let kernel = module.get_function(kernel_name)?;
        
        let block_size = 256;
        let grid_size = (num_vectors + block_size - 1) / block_size;
        
        kernel.launch(
            LaunchConfig {
                grid_dim: (grid_size as u32, 1, 1),
                block_dim: (block_size as u32, 1, 1),
                shared_mem_bytes: dimension * std::mem::size_of::<f32>(),
            },
            (
                &gpu_query,
                &gpu_vectors,
                dimension as i32,
                num_vectors as i32,
                &mut gpu_results,
            ),
        )?;
        
        // Copy results back
        device.dtoh_copy(&gpu_results)
    }
}

// ROCm implementation
#[cfg(feature = "rocm")]
impl GpuDistanceCompute {
    fn detect_rocm_devices() -> Result<Vec<GpuDevice>> {
        // ROCm device detection using HIP API
        // This is a placeholder - actual implementation would use hip-rs or similar
        warn!("ROCm support not fully implemented yet");
        Ok(vec![])
    }

    async fn calculate_distance_rocm(
        &self,
        _vec_a: &[f32],
        _vec_b: &[f32],
        _metric: DistanceMetric,
    ) -> Result<f32> {
        Err(anyhow!("ROCm distance calculation not implemented yet"))
    }

    async fn calculate_batch_rocm(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("ROCm batch calculation not implemented yet"))
    }
}

// Metal Performance Shaders implementation
#[cfg(all(target_os = "macos", feature = "metal"))]
impl GpuDistanceCompute {
    fn detect_mps_devices() -> Result<Vec<GpuDevice>> {
        use metal::{Device, DeviceLocation};
        
        let mut devices = Vec::new();
        
        // Get all Metal devices
        let metal_devices = Device::all();
        
        for (idx, device) in metal_devices.iter().enumerate() {
            // Check if device supports MPS
            if device.supports_family(metal::MTLGPUFamily::Mac1) {
                devices.push(GpuDevice {
                    id: idx as u32,
                    name: device.name().to_string(),
                    total_memory: device.recommended_max_working_set_size(),
                    available_memory: device.recommended_max_working_set_size(), // Approximate
                    compute_capability: None,
                    backend: GpuBackend::Mps,
                });
            }
        }
        
        Ok(devices)
    }

    async fn calculate_distance_mps(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32> {
        use metal::{Device, Library, Function, ComputePipelineState};
        use metal::{Buffer, MTLResourceOptions};
        
        let device = Device::system_default()
            .ok_or_else(|| anyhow!("No Metal device found"))?;
        
        // Load Metal shader library
        let library_data = include_bytes!("shaders/distance.metallib");
        let library = device.new_library_with_data(library_data)?;
        
        // Select function based on metric
        let function_name = match metric {
            DistanceMetric::Cosine => "cosine_distance",
            DistanceMetric::Euclidean => "euclidean_distance",
            DistanceMetric::DotProduct => "dot_product",
            _ => return Err(anyhow!("Unsupported metric for MPS: {:?}", metric)),
        };
        
        let function = library.get_function(function_name, None)?;
        let pipeline = device.new_compute_pipeline_state_with_function(&function)?;
        
        // Create buffers
        let vec_a_buffer = device.new_buffer_with_data(
            vec_a.as_ptr() as *const _,
            (vec_a.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );
        
        let vec_b_buffer = device.new_buffer_with_data(
            vec_b.as_ptr() as *const _,
            (vec_b.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );
        
        let result_buffer = device.new_buffer(
            std::mem::size_of::<f32>() as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );
        
        // Create command buffer and encoder
        let command_queue = device.new_command_queue();
        let command_buffer = command_queue.new_command_buffer();
        let compute_encoder = command_buffer.new_compute_command_encoder();
        
        compute_encoder.set_compute_pipeline_state(&pipeline);
        compute_encoder.set_buffer(0, Some(&vec_a_buffer), 0);
        compute_encoder.set_buffer(1, Some(&vec_b_buffer), 0);
        compute_encoder.set_buffer(2, Some(&result_buffer), 0);
        
        let length = vec_a.len() as u32;
        compute_encoder.set_bytes(3, std::mem::size_of::<u32>() as u64, &length as *const _ as *const _);
        
        // Dispatch threads
        let thread_group_size = pipeline.thread_execution_width();
        let thread_groups = (vec_a.len() + thread_group_size - 1) / thread_group_size;
        
        compute_encoder.dispatch_thread_groups(
            metal::MTLSize { width: thread_groups as u64, height: 1, depth: 1 },
            metal::MTLSize { width: thread_group_size as u64, height: 1, depth: 1 },
        );
        
        compute_encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();
        
        // Read result
        let result_ptr = result_buffer.contents() as *const f32;
        let result = unsafe { *result_ptr };
        
        Ok(result)
    }

    async fn calculate_batch_mps(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        // Similar implementation but with batch processing kernel
        Err(anyhow!("MPS batch calculation not fully implemented yet"))
    }
}

// OpenCL implementation
#[cfg(feature = "opencl")]
impl GpuDistanceCompute {
    fn detect_opencl_devices() -> Result<Vec<GpuDevice>> {
        use opencl3::platform::Platform;
        use opencl3::device::{Device, CL_DEVICE_TYPE_GPU};
        
        let mut devices = Vec::new();
        
        // Get all platforms
        let platforms = Platform::list();
        
        for platform in platforms {
            // Get GPU devices for this platform
            let platform_devices = platform
                .get_devices(CL_DEVICE_TYPE_GPU)
                .unwrap_or_default();
            
            for (idx, device) in platform_devices.iter().enumerate() {
                let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                let total_memory = device
                    .global_mem_size()
                    .unwrap_or(0);
                
                devices.push(GpuDevice {
                    id: idx as u32,
                    name,
                    total_memory,
                    available_memory: total_memory, // OpenCL doesn't provide this directly
                    compute_capability: None,
                    backend: GpuBackend::OpenCL,
                });
            }
        }
        
        Ok(devices)
    }

    async fn calculate_distance_opencl(
        &self,
        vec_a: &[f32],
        vec_b: &[f32],
        metric: DistanceMetric,
    ) -> Result<f32> {
        use opencl3::context::Context;
        use opencl3::command_queue::{CommandQueue, CL_QUEUE_PROFILING_ENABLE};
        use opencl3::program::Program;
        use opencl3::kernel::Kernel;
        use opencl3::memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_WRITE_ONLY};
        
        // Get device
        let device_idx = self.selected_device.unwrap_or(0);
        let device = &self.devices[device_idx];
        
        // Create OpenCL context
        let context = Context::from_device(&device)?;
        let queue = CommandQueue::create(&context, &device, CL_QUEUE_PROFILING_ENABLE)?;
        
        // Load kernel source
        let kernel_source = include_str!("kernels/distance.cl");
        let program = Program::create_and_build_from_source(&context, kernel_source, "")?;
        
        // Select kernel based on metric
        let kernel_name = match metric {
            DistanceMetric::Cosine => "cosine_distance",
            DistanceMetric::Euclidean => "euclidean_distance",
            DistanceMetric::DotProduct => "dot_product",
            _ => return Err(anyhow!("Unsupported metric for OpenCL: {:?}", metric)),
        };
        
        let kernel = Kernel::create(&program, kernel_name)?;
        
        // Create buffers
        let vec_a_buffer = Buffer::<f32>::create(
            &context,
            CL_MEM_READ_ONLY,
            vec_a.len(),
            None,
        )?;
        
        let vec_b_buffer = Buffer::<f32>::create(
            &context,
            CL_MEM_READ_ONLY,
            vec_b.len(),
            None,
        )?;
        
        let result_buffer = Buffer::<f32>::create(
            &context,
            CL_MEM_WRITE_ONLY,
            1,
            None,
        )?;
        
        // Write data to buffers
        queue.enqueue_write_buffer(&vec_a_buffer, true, 0, vec_a, &[])?;
        queue.enqueue_write_buffer(&vec_b_buffer, true, 0, vec_b, &[])?;
        
        // Set kernel arguments
        kernel.set_arg(0, &vec_a_buffer)?;
        kernel.set_arg(1, &vec_b_buffer)?;
        kernel.set_arg(2, &result_buffer)?;
        kernel.set_arg(3, &(vec_a.len() as i32))?;
        
        // Execute kernel
        let global_work_size = vec_a.len();
        let local_work_size = 64; // Typical work group size
        
        queue.enqueue_nd_range_kernel(
            &kernel,
            1,
            None,
            &[global_work_size],
            &[local_work_size],
            &[],
        )?;
        
        // Read result
        let mut result = vec![0.0f32];
        queue.enqueue_read_buffer(&result_buffer, true, 0, &mut result, &[])?;
        
        Ok(result[0])
    }

    async fn calculate_batch_opencl(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("OpenCL batch calculation not fully implemented yet"))
    }
}

// Fallback implementations for when features are not enabled
#[cfg(not(feature = "cuda"))]
impl GpuDistanceCompute {
    fn detect_cuda_devices() -> Result<Vec<GpuDevice>> {
        Ok(vec![])
    }
    
    async fn calculate_distance_cuda(
        &self,
        _vec_a: &[f32],
        _vec_b: &[f32],
        _metric: DistanceMetric,
    ) -> Result<f32> {
        Err(anyhow!("CUDA support not compiled in"))
    }
    
    async fn calculate_batch_cuda(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("CUDA support not compiled in"))
    }
}

#[cfg(not(feature = "rocm"))]
impl GpuDistanceCompute {
    fn detect_rocm_devices() -> Result<Vec<GpuDevice>> {
        Ok(vec![])
    }
    
    async fn calculate_distance_rocm(
        &self,
        _vec_a: &[f32],
        _vec_b: &[f32],
        _metric: DistanceMetric,
    ) -> Result<f32> {
        Err(anyhow!("ROCm support not compiled in"))
    }
    
    async fn calculate_batch_rocm(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("ROCm support not compiled in"))
    }
}

#[cfg(not(all(target_os = "macos", feature = "metal")))]
impl GpuDistanceCompute {
    fn detect_mps_devices() -> Result<Vec<GpuDevice>> {
        Ok(vec![])
    }
    
    async fn calculate_distance_mps(
        &self,
        _vec_a: &[f32],
        _vec_b: &[f32],
        _metric: DistanceMetric,
    ) -> Result<f32> {
        Err(anyhow!("Metal Performance Shaders not available on this platform"))
    }
    
    async fn calculate_batch_mps(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("Metal Performance Shaders not available on this platform"))
    }
}

#[cfg(not(feature = "opencl"))]
impl GpuDistanceCompute {
    fn detect_opencl_devices() -> Result<Vec<GpuDevice>> {
        Ok(vec![])
    }
    
    async fn calculate_distance_opencl(
        &self,
        _vec_a: &[f32],
        _vec_b: &[f32],
        _metric: DistanceMetric,
    ) -> Result<f32> {
        Err(anyhow!("OpenCL support not compiled in"))
    }
    
    async fn calculate_batch_opencl(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!("OpenCL support not compiled in"))
    }
}

/// GPU-accelerated distance calculator wrapper
pub struct GpuDistanceCalculator {
    gpu_compute: Arc<GpuDistanceCompute>,
    metric: DistanceMetric,
}

impl GpuDistanceCalculator {
    pub fn new(metric: DistanceMetric) -> Result<Self> {
        let gpu_compute = Arc::new(GpuDistanceCompute::new()?);
        Ok(Self { gpu_compute, metric })
    }
}

#[async_trait::async_trait]
impl DistanceCompute for GpuDistanceCalculator {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        // Synchronous wrapper for async GPU computation
        tokio::runtime::Handle::current()
            .block_on(async {
                self.gpu_compute
                    .calculate_distance_gpu(a, b, self.metric)
                    .await
            })
            .unwrap_or_else(|e| {
                warn!("GPU distance calculation failed: {}, falling back to CPU", e);
                // Fallback to CPU implementation
                use super::similarity::create_distance_calculator;
                let cpu_calc = create_distance_calculator(self.metric.clone());
                cpu_calc.calculate_distance(a, b, &self.metric)
            })
    }

    fn distance_batch(&self, query: &[f32], vectors: &[&[f32]]) -> Vec<f32> {
        // Convert to owned vectors for GPU transfer
        let owned_vectors: Vec<Vec<f32>> = vectors.iter().map(|v| v.to_vec()).collect();
        
        tokio::runtime::Handle::current()
            .block_on(async {
                self.gpu_compute
                    .calculate_batch_gpu(query, &owned_vectors, self.metric)
                    .await
            })
            .unwrap_or_else(|e| {
                warn!("GPU batch calculation failed: {}, falling back to CPU", e);
                // Fallback to CPU implementation
                use super::similarity::create_distance_calculator;
                let cpu_calc = create_distance_calculator(self.metric.clone());
                cpu_calc.distance_batch(query, vectors)
            })
    }

    fn is_similarity(&self) -> bool {
        match self.metric {
            DistanceMetric::DotProduct => true,
            _ => false,
        }
    }

    fn metric(&self) -> DistanceMetric {
        self.metric.clone()
    }
}

#[cfg(test)]
#[path = "gpu_detection_tests.rs"]
mod gpu_detection_tests;