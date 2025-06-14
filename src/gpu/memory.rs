// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! GPU memory management

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;
use super::{GpuAccelerator, GpuMemoryHandle, MemoryUsage};

/// GPU memory pool manager
pub struct GpuMemoryManager {
    /// GPU accelerator reference
    accelerator: Arc<dyn GpuAccelerator + Send + Sync>,
    
    /// Memory pool configuration
    config: MemoryPoolConfig,
    
    /// Active memory allocations
    active_allocations: Arc<RwLock<HashMap<u64, AllocationInfo>>>,
    
    /// Memory pool for small allocations
    small_pool: Arc<RwLock<Vec<PooledBuffer>>>,
    
    /// Memory pool for large allocations
    large_pool: Arc<RwLock<Vec<PooledBuffer>>>,
    
    /// Memory statistics
    stats: Arc<RwLock<MemoryStats>>,
    
    /// Next allocation ID
    next_allocation_id: Arc<RwLock<u64>>,
}

/// Memory pool configuration
#[derive(Debug, Clone)]
pub struct MemoryPoolConfig {
    /// Total pool size in bytes
    pub total_size_bytes: usize,
    
    /// Small allocation threshold
    pub small_allocation_threshold: usize,
    
    /// Number of small buffers to pre-allocate
    pub small_buffer_count: usize,
    
    /// Number of large buffers to pre-allocate
    pub large_buffer_count: usize,
    
    /// Enable automatic cleanup
    pub auto_cleanup: bool,
    
    /// Cleanup interval in seconds
    pub cleanup_interval_secs: u64,
}

impl Default for MemoryPoolConfig {
    fn default() -> Self {
        Self {
            total_size_bytes: 1024 * 1024 * 1024, // 1GB
            small_allocation_threshold: 1024 * 1024, // 1MB
            small_buffer_count: 100,
            large_buffer_count: 10,
            auto_cleanup: true,
            cleanup_interval_secs: 60,
        }
    }
}

/// Information about an active allocation
#[derive(Debug, Clone)]
struct AllocationInfo {
    pub handle: GpuMemoryHandle,
    pub allocated_at: chrono::DateTime<chrono::Utc>,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub access_count: u64,
    pub is_pooled: bool,
}

/// Pooled buffer for reuse
#[derive(Debug, Clone)]
struct PooledBuffer {
    pub handle: GpuMemoryHandle,
    pub is_available: bool,
    pub last_used: chrono::DateTime<chrono::Utc>,
}

/// Memory usage statistics
#[derive(Debug, Clone, Default)]
struct MemoryStats {
    pub total_allocated: usize,
    pub peak_allocated: usize,
    pub allocation_count: u64,
    pub deallocation_count: u64,
    pub pool_hits: u64,
    pub pool_misses: u64,
    pub cleanup_runs: u64,
}

/// GPU buffer wrapper for automatic cleanup
pub struct GpuBuffer {
    allocation_id: u64,
    handle: GpuMemoryHandle,
    manager: Arc<GpuMemoryManager>,
    is_dropped: Arc<RwLock<bool>>,
}

impl GpuMemoryManager {
    /// Create a new GPU memory manager
    pub async fn new(
        accelerator: Arc<dyn GpuAccelerator + Send + Sync>,
        pool_size_mb: usize,
    ) -> Result<Self> {
        let config = MemoryPoolConfig {
            total_size_bytes: pool_size_mb * 1024 * 1024,
            ..Default::default()
        };
        
        let manager = Self {
            accelerator,
            config,
            active_allocations: Arc::new(RwLock::new(HashMap::new())),
            small_pool: Arc::new(RwLock::new(Vec::new())),
            large_pool: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(MemoryStats::default())),
            next_allocation_id: Arc::new(RwLock::new(1)),
        };
        
        // Pre-allocate pool buffers
        manager.initialize_pools().await?;
        
        // Start cleanup task if auto cleanup is enabled
        if manager.config.auto_cleanup {
            manager.start_cleanup_task().await;
        }
        
        Ok(manager)
    }
    
    /// Allocate GPU memory with automatic pooling
    pub async fn allocate(&self, size: usize) -> Result<GpuBuffer> {
        let allocation_id = {
            let mut next_id = self.next_allocation_id.write().await;
            let id = *next_id;
            *next_id += 1;
            id
        };
        
        // Try to get from pool first
        let handle = if let Some(pooled_handle) = self.try_get_from_pool(size).await? {
            {
                let mut stats = self.stats.write().await;
                stats.pool_hits += 1;
            }
            pooled_handle
        } else {
            // Allocate new memory
            let handle = self.accelerator.allocate_memory(size).await?;
            {
                let mut stats = self.stats.write().await;
                stats.pool_misses += 1;
                stats.allocation_count += 1;
                stats.total_allocated += size;
                stats.peak_allocated = stats.peak_allocated.max(stats.total_allocated);
            }
            handle
        };
        
        // Track allocation
        let allocation_info = AllocationInfo {
            handle: handle.clone(),
            allocated_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            access_count: 0,
            is_pooled: false,
        };
        
        {
            let mut allocations = self.active_allocations.write().await;
            allocations.insert(allocation_id, allocation_info);
        }
        
        Ok(GpuBuffer {
            allocation_id,
            handle,
            manager: Arc::new(self.clone()),
            is_dropped: Arc::new(RwLock::new(false)),
        })
    }
    
    /// Deallocate GPU memory
    pub async fn deallocate(&self, allocation_id: u64) -> Result<()> {
        let allocation_info = {
            let mut allocations = self.active_allocations.write().await;
            allocations.remove(&allocation_id)
        };
        
        if let Some(info) = allocation_info {
            // Try to return to pool if it's a good candidate
            if self.should_return_to_pool(&info) {
                self.return_to_pool(info.handle).await?;
            } else {
                // Free the memory directly
                self.accelerator.free_memory(info.handle).await?;
                {
                    let mut stats = self.stats.write().await;
                    stats.deallocation_count += 1;
                    stats.total_allocated = stats.total_allocated.saturating_sub(info.handle.size);
                }
            }
        }
        
        Ok(())
    }
    
    /// Get memory usage information
    pub async fn get_memory_usage(&self) -> Result<MemoryUsage> {
        let stats = self.stats.read().await;
        let device_usage = self.accelerator.get_memory_usage().await?;
        
        Ok(MemoryUsage {
            total_memory: device_usage.total_memory,
            used_memory: device_usage.used_memory,
            free_memory: device_usage.free_memory,
            allocated_buffers: stats.allocation_count - stats.deallocation_count,
            peak_usage: stats.peak_allocated as u64,
        })
    }
    
    /// Cleanup all allocations and pools
    pub async fn cleanup(&self) -> Result<()> {
        // Free all active allocations
        let allocations = {
            let mut allocations = self.active_allocations.write().await;
            std::mem::take(&mut *allocations)
        };
        
        for (_, info) in allocations {
            self.accelerator.free_memory(info.handle).await?;
        }
        
        // Clean up pools
        let small_pool = {
            let mut pool = self.small_pool.write().await;
            std::mem::take(&mut *pool)
        };
        
        for buffer in small_pool {
            self.accelerator.free_memory(buffer.handle).await?;
        }
        
        let large_pool = {
            let mut pool = self.large_pool.write().await;
            std::mem::take(&mut *pool)
        };
        
        for buffer in large_pool {
            self.accelerator.free_memory(buffer.handle).await?;
        }
        
        // Reset stats
        {
            let mut stats = self.stats.write().await;
            *stats = MemoryStats::default();
        }
        
        tracing::info!("GPU memory cleanup completed");
        Ok(())
    }
    
    /// Initialize memory pools
    async fn initialize_pools(&self) -> Result<()> {
        // Initialize small buffer pool
        let mut small_pool = self.small_pool.write().await;
        for _ in 0..self.config.small_buffer_count {
            let handle = self.accelerator.allocate_memory(self.config.small_allocation_threshold).await?;
            small_pool.push(PooledBuffer {
                handle,
                is_available: true,
                last_used: chrono::Utc::now(),
            });
        }
        
        // Initialize large buffer pool
        let mut large_pool = self.large_pool.write().await;
        let large_buffer_size = self.config.total_size_bytes / self.config.large_buffer_count;
        for _ in 0..self.config.large_buffer_count {
            let handle = self.accelerator.allocate_memory(large_buffer_size).await?;
            large_pool.push(PooledBuffer {
                handle,
                is_available: true,
                last_used: chrono::Utc::now(),
            });
        }
        
        tracing::info!("GPU memory pools initialized: {} small, {} large buffers", 
                  self.config.small_buffer_count, self.config.large_buffer_count);
        
        Ok(())
    }
    
    /// Try to get a suitable buffer from the pool
    async fn try_get_from_pool(&self, size: usize) -> Result<Option<GpuMemoryHandle>> {
        if size <= self.config.small_allocation_threshold {
            // Try small pool first
            let mut small_pool = self.small_pool.write().await;
            for buffer in small_pool.iter_mut() {
                if buffer.is_available && buffer.handle.size >= size {
                    buffer.is_available = false;
                    buffer.last_used = chrono::Utc::now();
                    return Ok(Some(buffer.handle.clone()));
                }
            }
        }
        
        // Try large pool
        let mut large_pool = self.large_pool.write().await;
        for buffer in large_pool.iter_mut() {
            if buffer.is_available && buffer.handle.size >= size {
                buffer.is_available = false;
                buffer.last_used = chrono::Utc::now();
                return Ok(Some(buffer.handle.clone()));
            }
        }
        
        Ok(None)
    }
    
    /// Return a buffer to the appropriate pool
    async fn return_to_pool(&self, handle: GpuMemoryHandle) -> Result<()> {
        if handle.size <= self.config.small_allocation_threshold {
            let mut small_pool = self.small_pool.write().await;
            for buffer in small_pool.iter_mut() {
                if buffer.handle.ptr == handle.ptr {
                    buffer.is_available = true;
                    buffer.last_used = chrono::Utc::now();
                    return Ok(());
                }
            }
        } else {
            let mut large_pool = self.large_pool.write().await;
            for buffer in large_pool.iter_mut() {
                if buffer.handle.ptr == handle.ptr {
                    buffer.is_available = true;
                    buffer.last_used = chrono::Utc::now();
                    return Ok(());
                }
            }
        }
        
        // If not found in pools, free directly
        self.accelerator.free_memory(handle).await?;
        Ok(())
    }
    
    /// Check if an allocation should be returned to pool
    fn should_return_to_pool(&self, info: &AllocationInfo) -> bool {
        // Return to pool if it's a commonly used size and was accessed recently
        let size_suitable = info.handle.size <= self.config.small_allocation_threshold * 4;
        let recently_used = chrono::Utc::now().signed_duration_since(info.last_accessed).num_seconds() < 300; // 5 minutes
        let frequently_used = info.access_count > 2;
        
        size_suitable && (recently_used || frequently_used)
    }
    
    /// Start automatic cleanup task
    async fn start_cleanup_task(&self) {
        let manager = self.clone();
        let interval = self.config.cleanup_interval_secs;
        
        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(tokio::time::Duration::from_secs(interval));
            
            loop {
                interval_timer.tick().await;
                
                if let Err(e) = manager.cleanup_stale_allocations().await {
                    tracing::warn!("GPU memory cleanup error: {}", e);
                }
            }
        });
    }
    
    /// Clean up stale allocations
    async fn cleanup_stale_allocations(&self) -> Result<()> {
        let cutoff_time = chrono::Utc::now() - chrono::Duration::seconds(self.config.cleanup_interval_secs as i64 * 2);
        
        let mut to_cleanup = Vec::new();
        {
            let allocations = self.active_allocations.read().await;
            for (&id, info) in allocations.iter() {
                if info.last_accessed < cutoff_time && info.access_count == 0 {
                    to_cleanup.push(id);
                }
            }
        }
        
        for id in to_cleanup {
            self.deallocate(id).await?;
        }
        
        {
            let mut stats = self.stats.write().await;
            stats.cleanup_runs += 1;
        }
        
        Ok(())
    }
}

impl Clone for GpuMemoryManager {
    fn clone(&self) -> Self {
        Self {
            accelerator: self.accelerator.clone(),
            config: self.config.clone(),
            active_allocations: self.active_allocations.clone(),
            small_pool: self.small_pool.clone(),
            large_pool: self.large_pool.clone(),
            stats: self.stats.clone(),
            next_allocation_id: self.next_allocation_id.clone(),
        }
    }
}

impl GpuBuffer {
    /// Get the underlying GPU memory handle
    pub fn handle(&self) -> &GpuMemoryHandle {
        &self.handle
    }
    
    /// Get the size of the buffer
    pub fn size(&self) -> usize {
        self.handle.size
    }
    
    /// Copy data to the GPU buffer
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        if data.len() > self.handle.size {
            return Err(ProximaDBError::Gpu("Data size exceeds buffer size".to_string()));
        }
        
        // Update access statistics
        if let Ok(mut allocations) = self.manager.active_allocations.try_write() {
            if let Some(info) = allocations.get_mut(&self.allocation_id) {
                info.last_accessed = chrono::Utc::now();
                info.access_count += 1;
            }
        }
        
        self.manager.accelerator.copy_to_device(&self.handle, data).await
    }
    
    /// Copy data from the GPU buffer
    pub async fn read(&self, data: &mut [u8]) -> Result<()> {
        if data.len() > self.handle.size {
            return Err(ProximaDBError::Gpu("Data size exceeds buffer size".to_string()));
        }
        
        // Update access statistics
        if let Ok(mut allocations) = self.manager.active_allocations.try_write() {
            if let Some(info) = allocations.get_mut(&self.allocation_id) {
                info.last_accessed = chrono::Utc::now();
                info.access_count += 1;
            }
        }
        
        self.manager.accelerator.copy_from_device(&self.handle, data).await
    }
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        // Mark as dropped to prevent double-free
        if let Ok(mut is_dropped) = self.is_dropped.try_write() {
            if !*is_dropped {
                *is_dropped = true;
                
                // Schedule deallocation (cannot await in Drop)
                let manager = self.manager.clone();
                let allocation_id = self.allocation_id;
                
                tokio::spawn(async move {
                    if let Err(e) = manager.deallocate(allocation_id).await {
                        tracing::warn!("Failed to deallocate GPU buffer {}: {}", allocation_id, e);
                    }
                });
            }
        }
    }
}