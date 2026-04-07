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

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tracing::{debug, info, warn};

// Import DistanceMetric from proto module
use crate::compute::distance_computation::engine::{GpuAccelerator, HardwareBackend};
use crate::core::hardware_capabilities::{
    GpuBackend, GpuDevice, HardwareCapabilities, get_hardware_capabilities,
};
use crate::proto::proximadb_v1::DistanceMetric;

// Using central GpuBackend enum from hardware_capabilities module

// Re-export the central GpuDevice struct so callers don’t create a divergent type.
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

/// Detect available GPU backend and devices without initializing the accelerator
pub fn detect_gpu_capabilities() -> Result<(GpuBackend, Vec<GpuDevice>)> {
    GpuDistanceCompute::detect_gpu_backend()
}

/// Create a GPU accelerator wrapped in an Arc for reuse
pub fn create_gpu_accelerator() -> Result<Arc<dyn GpuAccelerator>> {
    let caps = get_hardware_capabilities();

    if !caps.has_gpu_distance() {
        return Err(anyhow!(
            "GPU acceleration disabled or unavailable in hardware configuration"
        ));
    }

    // Prefer cached detection results to avoid redundant probing
    if let Some(accel) = GpuDistanceCompute::from_capabilities(&caps) {
        if accel.is_available() {
            return Ok(Arc::new(accel));
        }
        warn!(
            "Cached GPU backend {:?} reported but no usable devices were found",
            accel.backend
        );
    }

    // Fall back to a fresh detection pass
    let accel = GpuDistanceCompute::new()?;
    if accel.is_available() {
        return Ok(Arc::new(accel));
    }

    Err(anyhow!(
        "GPU acceleration requested but no GPU devices are available after detection"
    ))
}

// Implement GpuAccelerator trait for GpuDistanceCompute
#[async_trait::async_trait]
impl GpuAccelerator for GpuDistanceCompute {
    fn backend(&self) -> HardwareBackend {
        match self.backend {
            GpuBackend::CUDA => HardwareBackend::CUDA,
            GpuBackend::ROCm => HardwareBackend::ROCm,
            GpuBackend::MPS => HardwareBackend::MPS,
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
        self.calculate_distance_gpu_internal(vec_a, vec_b, metric)
            .await
    }

    async fn calculate_batch_gpu(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        self.calculate_batch_gpu_internal(query, vectors, metric)
            .await
    }
}

impl GpuDistanceCompute {
    /// Create a new GPU distance compute manager
    pub fn new() -> Result<Self> {
        let (backend, devices) = Self::detect_gpu_backend()?;

        info!("🚀 GPU backend detected: {:?}", backend);
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

    /// Construct from already-detected hardware capabilities (avoids re-running probes).
    pub fn from_capabilities(caps: &HardwareCapabilities) -> Option<Self> {
        if !caps.has_gpu_distance() || caps.gpu.backend == GpuBackend::None {
            return None;
        }
        if caps.gpu.devices.is_empty() {
            return None;
        }

        let selected_device = caps.gpu.primary_device.or_else(|| Some(0));

        Some(Self {
            backend: caps.gpu.backend,
            devices: caps.gpu.devices.clone(),
            selected_device,
            memory_pool_size: 1024 * 1024 * 1024, // Align with default; tune later via config
        })
    }

    /// Detect available GPU backend and devices
    pub(crate) fn detect_gpu_backend() -> Result<(GpuBackend, Vec<GpuDevice>)> {
        // Try CUDA first (most common)
        #[cfg(feature = "cuda")]
        if let Ok(devices) = Self::detect_cuda_devices() {
            if !devices.is_none() {
                return Ok((GpuBackend::CUDA, devices));
            }
        }

        // Try ROCm for AMD GPUs
        #[cfg(feature = "rocm")]
        if let Ok(devices) = Self::detect_rocm_devices() {
            if !devices.is_none() {
                return Ok((GpuBackend::ROCm, devices));
            }
        }

        // Try Metal Performance Shaders on macOS
        #[cfg(all(target_os = "macos", feature = "metal"))]
        if let Ok(devices) = Self::detect_mps_devices() {
            if !devices.is_empty() {
                return Ok((GpuBackend::MPS, devices));
            }
        }

        // Fall back to OpenCL
        #[cfg(feature = "opencl")]
        if let Ok(devices) = Self::detect_opencl_devices() {
            if !devices.is_none() {
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
            GpuBackend::CUDA => self.calculate_distance_cuda(vec_a, vec_b, metric).await,
            GpuBackend::ROCm => self.calculate_distance_rocm(vec_a, vec_b, metric).await,
            GpuBackend::MPS => self.calculate_distance_mps(vec_a, vec_b, metric).await,
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
            GpuBackend::CUDA => self.calculate_batch_cuda(query, vectors, metric).await,
            GpuBackend::ROCm => self.calculate_batch_rocm(query, vectors, metric).await,
            GpuBackend::MPS => self.calculate_batch_mps(query, vectors, metric).await,
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
                backend: GpuBackend::CUDA,
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

        let device = CudaDevice::new(self.selected_device as i32)?;

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

        let device = CudaDevice::new(self.selected_device as i32)?;
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
        use metal::Device;

        let mut devices = Vec::new();

        // Get all Metal devices
        let metal_devices = Device::all();

        for (idx, device) in metal_devices.iter().enumerate() {
            // Check if device supports MPS
            if device.supports_family(metal::MTLGPUFamily::Mac1) {
                devices.push(GpuDevice {
                    id: idx,
                    name: device.name().to_string(),
                    total_memory: device.recommended_max_working_set_size(),
                    available_memory: device.recommended_max_working_set_size(), // Approximate
                    compute_capability: None,
                    backend: GpuBackend::MPS,
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
        use metal::{Buffer, MTLResourceOptions};
        use metal::{ComputePipelineState, Device, Function, Library};

        let device = Device::system_default().ok_or_else(|| anyhow!("No Metal device found"))?;

        // Deferred: Create distance.metal shader and compile to .metallib
        // For now, return error - this is separate from ProximaCodec GPU work
        return Err(anyhow!(
            "Metal distance shaders not yet implemented - use CPU fallback"
        ));

        /* DEFERRED: Uncomment when Metal distance shaders are implemented
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
        compute_encoder.set_bytes(
            3,
            std::mem::size_of::<u32>() as u64,
            &length as *const _ as *const _,
        );

        // Dispatch threads
        let thread_group_size = pipeline.thread_execution_width();
        let thread_groups = (vec_a.len() + thread_group_size - 1) / thread_group_size;

        compute_encoder.dispatch_thread_groups(
            metal::MTLSize {
                width: thread_groups as u64,
                height: 1,
                depth: 1,
            },
            metal::MTLSize {
                width: thread_group_size as u64,
                height: 1,
                depth: 1,
            },
        );

        compute_encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();

        // Read result
        let result_ptr = result_buffer.contents() as *const f32;
        let result = unsafe { *result_ptr };

        Ok(result)
        */
    }

    async fn calculate_batch_mps(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        use metal::{Device, MTLResourceOptions, MTLSize};
        use std::ffi::c_void;

        let device = Device::system_default().ok_or_else(|| anyhow!("No Metal device found"))?;

        let num_vectors = vectors.len();
        if num_vectors == 0 {
            return Ok(Vec::new());
        }

        let dimension = query.len();

        // Flatten vectors into contiguous buffer for GPU transfer
        let flattened: Vec<f32> = vectors.iter().flatten().copied().collect();

        // Pre-compute query norm for cosine similarity
        let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();

        // Load Metal shader library from source
        let shader_source = include_str!("kernels/distance.metal");
        let library = device
            .new_library_with_source(shader_source, &metal::CompileOptions::new())
            .map_err(|e| anyhow!("Failed to compile Metal shader: {}", e))?;

        // Select kernel based on metric
        let function_name = match metric {
            DistanceMetric::Euclidean => "euclidean_distance_batch",
            DistanceMetric::Cosine => "cosine_similarity_batch",
            DistanceMetric::DotProduct => "dot_product_batch",
            DistanceMetric::Manhattan => "manhattan_distance_batch",
            _ => return Err(anyhow!("Unsupported metric for MPS: {:?}", metric)),
        };

        let function = library
            .get_function(function_name, None)
            .map_err(|e| anyhow!("Failed to get kernel function '{}': {}", function_name, e))?;

        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| anyhow!("Failed to create pipeline: {}", e))?;

        // Create Metal buffers
        let query_buffer = device.new_buffer_with_data(
            query.as_ptr() as *const c_void,
            (query.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );

        let vectors_buffer = device.new_buffer_with_data(
            flattened.as_ptr() as *const c_void,
            (flattened.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );

        let results_buffer = device.new_buffer(
            (num_vectors * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );

        // Create command queue and buffer
        let command_queue = device.new_command_queue();
        let command_buffer = command_queue.new_command_buffer();
        let compute_encoder = command_buffer.new_compute_command_encoder();

        // Set pipeline and buffers
        compute_encoder.set_compute_pipeline_state(&pipeline);
        compute_encoder.set_buffer(0, Some(&query_buffer), 0);
        compute_encoder.set_buffer(1, Some(&vectors_buffer), 0);
        compute_encoder.set_buffer(2, Some(&results_buffer), 0);

        // Set dimension and n_vectors as uint constants
        let dim_u32 = dimension as u32;
        let n_vectors_u32 = num_vectors as u32;
        compute_encoder.set_bytes(
            3,
            std::mem::size_of::<u32>() as u64,
            &dim_u32 as *const u32 as *const c_void,
        );
        compute_encoder.set_bytes(
            4,
            std::mem::size_of::<u32>() as u64,
            &n_vectors_u32 as *const u32 as *const c_void,
        );

        // For cosine similarity, pass pre-computed query norm
        if matches!(metric, DistanceMetric::Cosine) {
            compute_encoder.set_bytes(
                5,
                std::mem::size_of::<f32>() as u64,
                &query_norm as *const f32 as *const c_void,
            );
        }

        // Dispatch threads - one thread per vector
        let thread_execution_width = pipeline.thread_execution_width();
        let threads_per_threadgroup = MTLSize {
            width: thread_execution_width,
            height: 1,
            depth: 1,
        };
        let num_threadgroups = MTLSize {
            width: ((num_vectors as u64 + thread_execution_width - 1) / thread_execution_width),
            height: 1,
            depth: 1,
        };

        compute_encoder.dispatch_thread_groups(num_threadgroups, threads_per_threadgroup);
        compute_encoder.end_encoding();

        // Execute and wait
        command_buffer.commit();
        command_buffer.wait_until_completed();

        // Read results back
        let results_ptr = results_buffer.contents() as *const f32;
        let results: Vec<f32> =
            unsafe { std::slice::from_raw_parts(results_ptr, num_vectors).to_vec() };

        debug!(
            "MPS batch distance: {} vectors x {}D, metric={:?}",
            num_vectors, dimension, metric
        );

        Ok(results)
    }

    /// Calculate pairwise distance matrix (P² matrix) using GPU MPS
    /// Computes all N×N pairwise distances in a single GPU dispatch
    /// Returns flattened Vec<f32> of size N×N
    pub async fn calculate_pairwise_matrix_mps(
        &self,
        vectors: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        use metal::{Device, MTLResourceOptions, MTLSize};
        use std::ffi::c_void;

        let device = Device::system_default().ok_or_else(|| anyhow!("No Metal device found"))?;

        let num_vectors = vectors.len();
        if num_vectors == 0 {
            return Ok(Vec::new());
        }

        let dimension = vectors[0].len();

        // Flatten vectors into contiguous buffer for GPU transfer
        let flattened: Vec<f32> = vectors.iter().flatten().copied().collect();

        // Load Metal shader library
        let shader_source = include_str!("kernels/distance.metal");
        let library = device
            .new_library_with_source(shader_source, &metal::CompileOptions::new())
            .map_err(|e| anyhow!("Failed to compile Metal shader: {}", e))?;

        // Select pairwise kernel based on metric
        let function_name = match metric {
            DistanceMetric::Euclidean => "pairwise_euclidean_matrix",
            DistanceMetric::Cosine => "pairwise_cosine_matrix",
            DistanceMetric::DotProduct => "pairwise_dot_product_matrix",
            _ => return Err(anyhow!("Unsupported metric for pairwise MPS: {:?}", metric)),
        };

        let function = library
            .get_function(function_name, None)
            .map_err(|e| anyhow!("Failed to get kernel function '{}': {}", function_name, e))?;

        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| anyhow!("Failed to create pipeline: {}", e))?;

        // Create vectors buffer
        let vectors_buffer = device.new_buffer_with_data(
            flattened.as_ptr() as *const c_void,
            (flattened.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );

        // For cosine, we also need pre-computed norms
        let norms_buffer = if matches!(metric, DistanceMetric::Cosine) {
            let norms: Vec<f32> = vectors
                .iter()
                .map(|v| v.iter().map(|x| x * x).sum::<f32>().sqrt())
                .collect();
            Some(device.new_buffer_with_data(
                norms.as_ptr() as *const c_void,
                (norms.len() * std::mem::size_of::<f32>()) as u64,
                MTLResourceOptions::CPUCacheModeDefaultCache,
            ))
        } else {
            None
        };

        // Output buffer: N×N distances
        let output_size = num_vectors * num_vectors;
        let results_buffer = device.new_buffer(
            (output_size * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );

        // Create command queue and buffer
        let command_queue = device.new_command_queue();
        let command_buffer = command_queue.new_command_buffer();
        let compute_encoder = command_buffer.new_compute_command_encoder();

        // Set pipeline and buffers
        compute_encoder.set_compute_pipeline_state(&pipeline);

        match metric {
            DistanceMetric::Cosine => {
                // Cosine uses: vectors, norms, distances, dim, n_vectors
                compute_encoder.set_buffer(0, Some(&vectors_buffer), 0);
                // Convert Option<Buffer> to Option<&BufferRef> for Metal API
                let norms_ref = norms_buffer.as_ref().map(|b| b as &metal::BufferRef);
                compute_encoder.set_buffer(1, norms_ref, 0);
                compute_encoder.set_buffer(2, Some(&results_buffer), 0);
                let dim_u32 = dimension as u32;
                let n_vectors_u32 = num_vectors as u32;
                compute_encoder.set_bytes(
                    3,
                    std::mem::size_of::<u32>() as u64,
                    &dim_u32 as *const u32 as *const c_void,
                );
                compute_encoder.set_bytes(
                    4,
                    std::mem::size_of::<u32>() as u64,
                    &n_vectors_u32 as *const u32 as *const c_void,
                );
            }
            _ => {
                // Euclidean/DotProduct use: vectors, distances, dim, n_vectors
                compute_encoder.set_buffer(0, Some(&vectors_buffer), 0);
                compute_encoder.set_buffer(1, Some(&results_buffer), 0);
                let dim_u32 = dimension as u32;
                let n_vectors_u32 = num_vectors as u32;
                compute_encoder.set_bytes(
                    2,
                    std::mem::size_of::<u32>() as u64,
                    &dim_u32 as *const u32 as *const c_void,
                );
                compute_encoder.set_bytes(
                    3,
                    std::mem::size_of::<u32>() as u64,
                    &n_vectors_u32 as *const u32 as *const c_void,
                );
            }
        }

        // Dispatch 2D grid: one thread per matrix element
        let thread_execution_width = pipeline.thread_execution_width();
        let max_threads_per_threadgroup = pipeline.max_total_threads_per_threadgroup();

        // Calculate optimal 2D threadgroup size
        let threads_per_side = (max_threads_per_threadgroup as f64).sqrt() as u64;
        let threads_per_threadgroup = MTLSize {
            width: threads_per_side.min(thread_execution_width),
            height: threads_per_side,
            depth: 1,
        };

        // Total threads needed
        let num_threadgroups = MTLSize {
            width: ((num_vectors as u64 + threads_per_threadgroup.width - 1)
                / threads_per_threadgroup.width),
            height: ((num_vectors as u64 + threads_per_threadgroup.height - 1)
                / threads_per_threadgroup.height),
            depth: 1,
        };

        compute_encoder.dispatch_thread_groups(num_threadgroups, threads_per_threadgroup);
        compute_encoder.end_encoding();

        // Execute and wait
        command_buffer.commit();
        command_buffer.wait_until_completed();

        // Read results back
        let results_ptr = results_buffer.contents() as *const f32;
        let results: Vec<f32> =
            unsafe { std::slice::from_raw_parts(results_ptr, output_size).to_vec() };

        info!(
            "MPS pairwise matrix: {}×{} vectors x {}D = {} distances computed on GPU",
            num_vectors, num_vectors, dimension, output_size
        );

        Ok(results)
    }

    /// Calculate batch distances with GPU memory reuse for repeated queries
    /// This is optimized for the common case: same collection, different queries
    async fn calculate_batch_mps_with_cache(
        &self,
        query: &[f32],
        vectors_buffer: &metal::Buffer,
        num_vectors: usize,
        dimension: usize,
        metric: DistanceMetric,
        device: &metal::Device,
        pipeline: &metal::ComputePipelineState,
        command_queue: &metal::CommandQueue,
    ) -> Result<Vec<f32>> {
        use metal::{MTLResourceOptions, MTLSize};
        use std::ffi::c_void;

        // Pre-compute query norm for cosine similarity
        let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();

        // Create query buffer (small, changes per query)
        let query_buffer = device.new_buffer_with_data(
            query.as_ptr() as *const c_void,
            (query.len() * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::CPUCacheModeDefaultCache,
        );

        // Results buffer
        let results_buffer = device.new_buffer(
            (num_vectors * std::mem::size_of::<f32>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );

        let command_buffer = command_queue.new_command_buffer();
        let compute_encoder = command_buffer.new_compute_command_encoder();

        compute_encoder.set_compute_pipeline_state(pipeline);
        compute_encoder.set_buffer(0, Some(&query_buffer), 0);
        compute_encoder.set_buffer(1, Some(vectors_buffer), 0);
        compute_encoder.set_buffer(2, Some(&results_buffer), 0);

        let dim_u32 = dimension as u32;
        let n_vectors_u32 = num_vectors as u32;
        compute_encoder.set_bytes(
            3,
            std::mem::size_of::<u32>() as u64,
            &dim_u32 as *const u32 as *const c_void,
        );
        compute_encoder.set_bytes(
            4,
            std::mem::size_of::<u32>() as u64,
            &n_vectors_u32 as *const u32 as *const c_void,
        );

        if matches!(metric, DistanceMetric::Cosine) {
            compute_encoder.set_bytes(
                5,
                std::mem::size_of::<f32>() as u64,
                &query_norm as *const f32 as *const c_void,
            );
        }

        let thread_execution_width = pipeline.thread_execution_width();
        let threads_per_threadgroup = MTLSize {
            width: thread_execution_width,
            height: 1,
            depth: 1,
        };
        let num_threadgroups = MTLSize {
            width: ((num_vectors as u64 + thread_execution_width - 1) / thread_execution_width),
            height: 1,
            depth: 1,
        };

        compute_encoder.dispatch_thread_groups(num_threadgroups, threads_per_threadgroup);
        compute_encoder.end_encoding();

        command_buffer.commit();
        command_buffer.wait_until_completed();

        let results_ptr = results_buffer.contents() as *const f32;
        let results: Vec<f32> =
            unsafe { std::slice::from_raw_parts(results_ptr, num_vectors).to_vec() };

        Ok(results)
    }
}

// OpenCL implementation
#[cfg(feature = "opencl")]
impl GpuDistanceCompute {
    fn detect_opencl_devices() -> Result<Vec<GpuDevice>> {
        use opencl3::device::{CL_DEVICE_TYPE_GPU, Device};
        use opencl3::platform::Platform;

        let mut devices = Vec::new();

        // Get all platforms
        let platforms = Platform::list();

        for platform in platforms {
            // Get GPU devices for this platform
            let platform_devices = platform.get_devices(CL_DEVICE_TYPE_GPU).clone();

            for (idx, device) in platform_devices.iter().enumerate() {
                let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
                let total_memory = device.global_mem_size();

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
        use opencl3::command_queue::{CL_QUEUE_PROFILING_ENABLE, CommandQueue};
        use opencl3::context::Context;
        use opencl3::kernel::Kernel;
        use opencl3::memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_WRITE_ONLY};
        use opencl3::program::Program;

        // Get device
        let device_idx = self.selected_device;
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
        let vec_a_buffer = Buffer::<f32>::create(&context, CL_MEM_READ_ONLY, vec_a.len(), None)?;

        let vec_b_buffer = Buffer::<f32>::create(&context, CL_MEM_READ_ONLY, vec_b.len(), None)?;

        let result_buffer = Buffer::<f32>::create(&context, CL_MEM_WRITE_ONLY, 1, None)?;

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
        Err(anyhow!(
            "OpenCL batch calculation not fully implemented yet"
        ))
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
        Err(anyhow!(
            "Metal Performance Shaders not available on this platform"
        ))
    }

    async fn calculate_batch_mps(
        &self,
        _query: &[f32],
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!(
            "Metal Performance Shaders not available on this platform"
        ))
    }

    /// Pairwise matrix fallback for non-MPS platforms
    pub async fn calculate_pairwise_matrix_mps(
        &self,
        _vectors: &[Vec<f32>],
        _metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        Err(anyhow!(
            "Metal Performance Shaders not available on this platform"
        ))
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
        Ok(Self {
            gpu_compute,
            metric,
        })
    }
}

// Deferred: Implement DistanceCompute trait when similarity module is ready
/*
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
                warn!(
                    "GPU distance calculation failed: {}, returning error (CPU fallback not yet implemented)",
                    e
                );
                // Deferred: Implement CPU fallback via distance_computation module
                Err(e)
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
                warn!("GPU batch calculation failed: {}, returning empty vec (CPU fallback not yet implemented)", e);
                // Deferred: Implement CPU fallback via distance_computation module
                vec![]
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
*/

#[cfg(test)]
mod tests {
    use super::*;
    // GPU detection tests would go here
}
