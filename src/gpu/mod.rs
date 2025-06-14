// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! GPU acceleration module for ProximaDB

pub mod cuda;
pub mod opencl;
pub mod acceleration;
pub mod memory;
pub mod kernels;

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;

pub use acceleration::{GpuAccelerator, AccelerationType};
pub use memory::{GpuMemoryManager, GpuBuffer};
pub use kernels::{VectorKernels, SimilarityKernel};

/// GPU configuration for ProximaDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    /// Enable GPU acceleration
    pub enabled: bool,
    
    /// Preferred acceleration type
    pub acceleration_type: AccelerationType,
    
    /// Device ID to use (for multi-GPU systems)
    pub device_id: Option<u32>,
    
    /// Memory allocation size in MB
    pub memory_pool_size_mb: usize,
    
    /// Batch size for GPU operations
    pub batch_size: usize,
    
    /// Enable mixed precision
    pub mixed_precision: bool,
    
    /// Minimum vector count to use GPU
    pub min_vectors_for_gpu: usize,
    
    /// Enable asynchronous GPU operations
    pub async_operations: bool,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default
            acceleration_type: AccelerationType::Auto,
            device_id: None,
            memory_pool_size_mb: 1024, // 1GB default
            batch_size: 10000,
            mixed_precision: true,
            min_vectors_for_gpu: 1000,
            async_operations: true,
        }
    }
}

/// Main GPU manager for ProximaDB
pub struct GpuManager {
    /// Configuration
    config: GpuConfig,
    
    /// GPU accelerator implementation
    accelerator: Option<Arc<dyn GpuAccelerator + Send + Sync>>,
    
    /// Memory manager
    memory_manager: Option<Arc<GpuMemoryManager>>,
    
    /// Vector operation kernels
    kernels: Option<Arc<VectorKernels>>,
    
    /// Device information
    device_info: Option<DeviceInfo>,
    
    /// Performance statistics
    stats: Arc<RwLock<GpuStats>>,
}

/// GPU device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub compute_capability: (u32, u32),
    pub total_memory: u64,
    pub available_memory: u64,
    pub multiprocessor_count: u32,
    pub max_threads_per_block: u32,
    pub max_block_dimensions: (u32, u32, u32),
    pub max_grid_dimensions: (u32, u32, u32),
    pub warp_size: u32,
    pub clock_rate: u32,
    pub memory_clock_rate: u32,
    pub memory_bus_width: u32,
}

/// GPU performance statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuStats {
    pub operations_total: u64,
    pub operations_gpu: u64,
    pub operations_cpu_fallback: u64,
    pub avg_gpu_time_ms: f64,
    pub avg_cpu_time_ms: f64,
    pub memory_allocated_mb: f64,
    pub memory_peak_mb: f64,
    pub gpu_utilization_percent: f64,
    pub kernel_launch_failures: u64,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

impl GpuManager {
    /// Create a new GPU manager
    pub async fn new(config: GpuConfig) -> Result<Self> {
        let mut manager = Self {
            config,
            accelerator: None,
            memory_manager: None,
            kernels: None,
            device_info: None,
            stats: Arc::new(RwLock::new(GpuStats::default())),
        };
        
        if manager.config.enabled {
            manager.initialize().await?;
        }
        
        Ok(manager)
    }
    
    /// Initialize GPU acceleration
    pub async fn initialize(&mut self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        
        tracing::info!("Initializing GPU acceleration...");
        
        // Detect and initialize accelerator
        let accelerator = self.detect_and_create_accelerator().await?;
        
        // Get device information
        let device_info = accelerator.get_device_info().await?;
        tracing::info!("GPU Device: {}, Memory: {:.1} GB", 
                  device_info.name, device_info.total_memory as f64 / 1024.0 / 1024.0 / 1024.0);
        
        // Initialize memory manager
        let memory_manager = Arc::new(GpuMemoryManager::new(
            accelerator.clone(),
            self.config.memory_pool_size_mb,
        ).await?);
        
        // Initialize kernels
        let kernels = Arc::new(VectorKernels::new(
            accelerator.clone(),
            memory_manager.clone(),
        ).await?);
        
        self.accelerator = Some(accelerator);
        self.memory_manager = Some(memory_manager);
        self.kernels = Some(kernels);
        self.device_info = Some(device_info);
        
        tracing::info!("GPU acceleration initialized successfully");
        Ok(())
    }
    
    /// Check if GPU acceleration is available and enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.accelerator.is_some()
    }
    
    /// Perform vector similarity search using GPU
    pub async fn similarity_search(
        &self,
        query_vectors: &[Vec<f32>],
        dataset_vectors: &[Vec<f32>],
        k: usize,
    ) -> Result<Vec<Vec<(usize, f32)>>> {
        if !self.is_enabled() {
            return Err(ProximaDBError::Gpu("GPU acceleration not available".to_string()));
        }
        
        // Check if we should use GPU based on data size
        if dataset_vectors.len() < self.config.min_vectors_for_gpu {
            return Err(ProximaDBError::Gpu("Dataset too small for GPU acceleration".to_string()));
        }
        
        let start_time = std::time::Instant::now();
        
        let kernels = self.kernels.as_ref().ok_or_else(|| {
            ProximaDBError::Gpu("GPU kernels not initialized".to_string())
        })?;
        
        let results = kernels.cosine_similarity_search(
            query_vectors,
            dataset_vectors,
            k,
        ).await?;
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.operations_total += 1;
            stats.operations_gpu += 1;
            let elapsed = start_time.elapsed().as_millis() as f64;
            stats.avg_gpu_time_ms = (stats.avg_gpu_time_ms * (stats.operations_gpu - 1) as f64 + elapsed) / stats.operations_gpu as f64;
            stats.last_updated = chrono::Utc::now();
        }
        
        Ok(results)
    }
    
    /// Perform batch vector operations using GPU
    pub async fn batch_vector_operations(
        &self,
        vectors: &[Vec<f32>],
        operation: VectorOperation,
    ) -> Result<Vec<Vec<f32>>> {
        if !self.is_enabled() {
            return Err(ProximaDBError::Gpu("GPU acceleration not available".to_string()));
        }
        
        let kernels = self.kernels.as_ref().ok_or_else(|| {
            ProximaDBError::Gpu("GPU kernels not initialized".to_string())
        })?;
        
        match operation {
            VectorOperation::Normalize => kernels.normalize_vectors(vectors).await,
            VectorOperation::DotProduct { other_vectors } => {
                kernels.batch_dot_product(vectors, &other_vectors).await
            }
            VectorOperation::Add { other_vectors } => {
                kernels.vector_addition(vectors, &other_vectors).await
            }
            VectorOperation::Scale { factor } => {
                kernels.scale_vectors(vectors, factor).await
            }
        }
    }
    
    /// Get GPU device information
    pub fn get_device_info(&self) -> Option<&DeviceInfo> {
        self.device_info.as_ref()
    }
    
    /// Get GPU performance statistics
    pub async fn get_stats(&self) -> GpuStats {
        self.stats.read().await.clone()
    }
    
    /// Check GPU memory usage
    pub async fn get_memory_usage(&self) -> Result<MemoryUsage> {
        if let Some(ref memory_manager) = self.memory_manager {
            memory_manager.get_memory_usage().await
        } else {
            Err(ProximaDBError::Gpu("Memory manager not initialized".to_string()))
        }
    }
    
    /// Cleanup GPU resources
    pub async fn cleanup(&mut self) -> Result<()> {
        if let Some(ref memory_manager) = self.memory_manager {
            memory_manager.cleanup().await?;
        }
        
        self.accelerator = None;
        self.memory_manager = None;
        self.kernels = None;
        
        tracing::info!("GPU resources cleaned up");
        Ok(())
    }
    
    /// Detect available GPU acceleration and create accelerator
    async fn detect_and_create_accelerator(&self) -> Result<Arc<dyn GpuAccelerator + Send + Sync>> {
        match self.config.acceleration_type {
            AccelerationType::Cuda => {
                if cuda::is_available() {
                    Ok(Arc::new(cuda::CudaAccelerator::new(self.config.device_id).await?))
                } else {
                    Err(ProximaDBError::Gpu("CUDA not available".to_string()))
                }
            }
            AccelerationType::OpenCL => {
                if opencl::is_available() {
                    Ok(Arc::new(opencl::OpenCLAccelerator::new(self.config.device_id).await?))
                } else {
                    Err(ProximaDBError::Gpu("OpenCL not available".to_string()))
                }
            }
            AccelerationType::Auto => {
                // Try CUDA first, then OpenCL
                if cuda::is_available() {
                    tracing::info!("Auto-detected CUDA acceleration");
                    Ok(Arc::new(cuda::CudaAccelerator::new(self.config.device_id).await?))
                } else if opencl::is_available() {
                    tracing::info!("Auto-detected OpenCL acceleration");
                    Ok(Arc::new(opencl::OpenCLAccelerator::new(self.config.device_id).await?))
                } else {
                    Err(ProximaDBError::Gpu("No GPU acceleration available".to_string()))
                }
            }
        }
    }
}

/// Vector operations that can be performed on GPU
#[derive(Debug, Clone)]
pub enum VectorOperation {
    Normalize,
    DotProduct { other_vectors: Vec<Vec<f32>> },
    Add { other_vectors: Vec<Vec<f32>> },
    Scale { factor: f32 },
}

/// GPU memory usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub total_memory: u64,
    pub used_memory: u64,
    pub free_memory: u64,
    pub allocated_buffers: usize,
    pub peak_usage: u64,
}

/// GPU benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuBenchmark {
    pub operation: String,
    pub vector_count: usize,
    pub dimension: usize,
    pub gpu_time_ms: f64,
    pub cpu_time_ms: f64,
    pub speedup: f64,
    pub throughput_vectors_per_sec: f64,
}

impl GpuManager {
    /// Run GPU benchmarks
    pub async fn run_benchmarks(&self) -> Result<Vec<GpuBenchmark>> {
        if !self.is_enabled() {
            return Err(ProximaDBError::Gpu("GPU not available for benchmarking".to_string()));
        }
        
        let mut benchmarks = Vec::new();
        
        // Benchmark similarity search
        let test_sizes = vec![(1000, 128), (10000, 256), (50000, 512)];
        
        for (vector_count, dimension) in test_sizes {
            let benchmark = self.benchmark_similarity_search(vector_count, dimension).await?;
            benchmarks.push(benchmark);
        }
        
        Ok(benchmarks)
    }
    
    /// Benchmark similarity search performance
    async fn benchmark_similarity_search(&self, vector_count: usize, dimension: usize) -> Result<GpuBenchmark> {
        // Generate test data
        let query_vectors = vec![vec![0.1; dimension]; 10]; // 10 query vectors
        let dataset_vectors = (0..vector_count)
            .map(|i| (0..dimension).map(|j| (i + j) as f32 * 0.01).collect())
            .collect::<Vec<Vec<f32>>>();
        
        // GPU timing
        let gpu_start = std::time::Instant::now();
        let _gpu_results = self.similarity_search(&query_vectors, &dataset_vectors, 10).await?;
        let gpu_time = gpu_start.elapsed().as_millis() as f64;
        
        // CPU timing (fallback implementation)
        let cpu_start = std::time::Instant::now();
        let _cpu_results = self.cpu_similarity_search(&query_vectors, &dataset_vectors, 10);
        let cpu_time = cpu_start.elapsed().as_millis() as f64;
        
        let speedup = if gpu_time > 0.0 { cpu_time / gpu_time } else { 1.0 };
        let throughput = (vector_count as f64 * query_vectors.len() as f64) / (gpu_time / 1000.0);
        
        Ok(GpuBenchmark {
            operation: "similarity_search".to_string(),
            vector_count,
            dimension,
            gpu_time_ms: gpu_time,
            cpu_time_ms: cpu_time,
            speedup,
            throughput_vectors_per_sec: throughput,
        })
    }
    
    /// CPU fallback implementation for benchmarking
    fn cpu_similarity_search(
        &self,
        query_vectors: &[Vec<f32>],
        dataset_vectors: &[Vec<f32>],
        k: usize,
    ) -> Vec<Vec<(usize, f32)>> {
        query_vectors.iter().map(|query| {
            let mut similarities: Vec<(usize, f32)> = dataset_vectors
                .iter()
                .enumerate()
                .map(|(idx, vector)| {
                    let similarity = cosine_similarity(query, vector);
                    (idx, similarity)
                })
                .collect();
            
            similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            similarities.truncate(k);
            similarities
        }).collect()
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}