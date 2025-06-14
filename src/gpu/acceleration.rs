// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! GPU acceleration abstractions

use std::sync::Arc;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};

use crate::core::Result;
use super::{DeviceInfo, MemoryUsage};

/// GPU acceleration types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccelerationType {
    /// NVIDIA CUDA acceleration
    Cuda,
    /// OpenCL acceleration
    OpenCL,
    /// Auto-detect best available
    Auto,
}

impl Default for AccelerationType {
    fn default() -> Self {
        Self::Auto
    }
}

/// Trait for GPU accelerator implementations
#[async_trait]
pub trait GpuAccelerator {
    /// Initialize the accelerator
    async fn initialize(&self) -> Result<()>;
    
    /// Get device information
    async fn get_device_info(&self) -> Result<DeviceInfo>;
    
    /// Allocate GPU memory
    async fn allocate_memory(&self, size: usize) -> Result<GpuMemoryHandle>;
    
    /// Copy data from host to device
    async fn copy_to_device(&self, handle: &GpuMemoryHandle, data: &[u8]) -> Result<()>;
    
    /// Copy data from device to host
    async fn copy_from_device(&self, handle: &GpuMemoryHandle, data: &mut [u8]) -> Result<()>;
    
    /// Free GPU memory
    async fn free_memory(&self, handle: GpuMemoryHandle) -> Result<()>;
    
    /// Launch a kernel
    async fn launch_kernel(&self, kernel: &GpuKernel, grid_size: (u32, u32, u32), block_size: (u32, u32, u32)) -> Result<()>;
    
    /// Synchronize device (wait for all operations to complete)
    async fn synchronize(&self) -> Result<()>;
    
    /// Get memory usage information
    async fn get_memory_usage(&self) -> Result<MemoryUsage>;
    
    /// Check if this accelerator type is available
    fn is_available() -> bool;
    
    /// Get accelerator type
    fn accelerator_type(&self) -> AccelerationType;
}

/// Handle to GPU memory allocation
#[derive(Debug, Clone)]
pub struct GpuMemoryHandle {
    pub ptr: u64, // Device pointer (implementation-specific)
    pub size: usize,
    pub alignment: usize,
}

/// GPU kernel representation
#[derive(Debug, Clone)]
pub struct GpuKernel {
    pub name: String,
    pub source: String,
    pub entry_point: String,
    pub parameters: Vec<KernelParameter>,
}

/// Kernel parameter types
#[derive(Debug, Clone)]
pub enum KernelParameter {
    Buffer(GpuMemoryHandle),
    Scalar(ScalarValue),
}

/// Scalar parameter values
#[derive(Debug, Clone)]
pub enum ScalarValue {
    Int32(i32),
    UInt32(u32),
    Float32(f32),
    Float64(f64),
}

/// GPU context for managing resources
pub struct GpuContext {
    pub device_id: u32,
    pub context_handle: u64, // Implementation-specific context
}

/// Performance profiler for GPU operations
pub struct GpuProfiler {
    events: Vec<GpuEvent>,
}

/// GPU profiling event
#[derive(Debug, Clone)]
pub struct GpuEvent {
    pub name: String,
    pub start_time: std::time::Instant,
    pub duration: std::time::Duration,
    pub memory_used: usize,
    pub kernel_name: Option<String>,
}

impl GpuProfiler {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
        }
    }
    
    pub fn start_event(&mut self, name: String) -> usize {
        let event = GpuEvent {
            name,
            start_time: std::time::Instant::now(),
            duration: std::time::Duration::new(0, 0),
            memory_used: 0,
            kernel_name: None,
        };
        self.events.push(event);
        self.events.len() - 1
    }
    
    pub fn end_event(&mut self, event_id: usize) {
        if let Some(event) = self.events.get_mut(event_id) {
            event.duration = event.start_time.elapsed();
        }
    }
    
    pub fn get_events(&self) -> &[GpuEvent] {
        &self.events
    }
    
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// GPU resource manager
pub struct GpuResourceManager {
    accelerator: Arc<dyn GpuAccelerator + Send + Sync>,
    allocated_buffers: std::collections::HashMap<u64, GpuMemoryHandle>,
    total_allocated: usize,
    peak_allocated: usize,
}

impl GpuResourceManager {
    pub fn new(accelerator: Arc<dyn GpuAccelerator + Send + Sync>) -> Self {
        Self {
            accelerator,
            allocated_buffers: std::collections::HashMap::new(),
            total_allocated: 0,
            peak_allocated: 0,
        }
    }
    
    pub async fn allocate(&mut self, size: usize) -> Result<u64> {
        let handle = self.accelerator.allocate_memory(size).await?;
        let id = handle.ptr;
        
        self.allocated_buffers.insert(id, handle);
        self.total_allocated += size;
        self.peak_allocated = self.peak_allocated.max(self.total_allocated);
        
        Ok(id)
    }
    
    pub async fn deallocate(&mut self, id: u64) -> Result<()> {
        if let Some(handle) = self.allocated_buffers.remove(&id) {
            self.total_allocated -= handle.size;
            self.accelerator.free_memory(handle).await?;
        }
        Ok(())
    }
    
    pub fn get_handle(&self, id: u64) -> Option<&GpuMemoryHandle> {
        self.allocated_buffers.get(&id)
    }
    
    pub fn total_allocated(&self) -> usize {
        self.total_allocated
    }
    
    pub fn peak_allocated(&self) -> usize {
        self.peak_allocated
    }
    
    pub async fn cleanup(&mut self) -> Result<()> {
        let handles: Vec<_> = self.allocated_buffers.drain().collect();
        for (_, handle) in handles {
            self.accelerator.free_memory(handle).await?;
        }
        self.total_allocated = 0;
        Ok(())
    }
}

/// GPU capability detection
pub struct GpuCapabilities {
    pub max_work_group_size: u32,
    pub max_compute_units: u32,
    pub local_memory_size: u64,
    pub global_memory_size: u64,
    pub supports_double_precision: bool,
    pub supports_half_precision: bool,
    pub supports_unified_memory: bool,
    pub compute_capability: (u32, u32),
}

impl GpuCapabilities {
    /// Check if the GPU supports the required operations
    pub fn supports_vector_operations(&self) -> bool {
        self.max_work_group_size >= 256 && 
        self.global_memory_size >= 1024 * 1024 * 1024 // At least 1GB
    }
    
    /// Get optimal work group size for vector operations
    pub fn optimal_work_group_size(&self, vector_count: usize) -> u32 {
        let suggested_size = (vector_count as f64).sqrt() as u32;
        std::cmp::min(suggested_size.next_power_of_two(), self.max_work_group_size)
    }
    
    /// Check if mixed precision is beneficial
    pub fn should_use_mixed_precision(&self) -> bool {
        self.supports_half_precision && 
        self.compute_capability.0 >= 7 // Tensor cores available
    }
}

/// GPU operation queue for batching
pub struct GpuOperationQueue {
    operations: Vec<QueuedOperation>,
    max_batch_size: usize,
}

#[derive(Debug)]
struct QueuedOperation {
    kernel: GpuKernel,
    grid_size: (u32, u32, u32),
    block_size: (u32, u32, u32),
    priority: OperationPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl GpuOperationQueue {
    pub fn new(max_batch_size: usize) -> Self {
        Self {
            operations: Vec::new(),
            max_batch_size,
        }
    }
    
    pub fn enqueue(&mut self, operation: QueuedOperation) {
        self.operations.push(operation);
        
        // Sort by priority
        self.operations.sort_by_key(|op| std::cmp::Reverse(op.priority));
    }
    
    pub fn should_flush(&self) -> bool {
        self.operations.len() >= self.max_batch_size
    }
    
    pub fn flush(&mut self) -> Vec<QueuedOperation> {
        std::mem::take(&mut self.operations)
    }
}

/// Error recovery for GPU operations
pub struct GpuErrorRecovery {
    retry_count: u32,
    max_retries: u32,
    fallback_to_cpu: bool,
}

impl GpuErrorRecovery {
    pub fn new(max_retries: u32, fallback_to_cpu: bool) -> Self {
        Self {
            retry_count: 0,
            max_retries,
            fallback_to_cpu,
        }
    }
    
    pub fn should_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
    
    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
    }
    
    pub fn should_fallback_to_cpu(&self) -> bool {
        self.fallback_to_cpu && self.retry_count >= self.max_retries
    }
    
    pub fn reset(&mut self) {
        self.retry_count = 0;
    }
}