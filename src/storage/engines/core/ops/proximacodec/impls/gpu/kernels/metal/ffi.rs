// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Metal FFI Bindings
//!
//! This module provides Rust FFI bindings to Metal framework for GPU acceleration.
//! Uses the `metal` crate for safe Rust bindings to Apple's Metal API.
//!
//! ## Memory Safety
//!
//! Metal uses automatic reference counting (ARC) and the `metal` crate provides
//! safe Rust wrappers. All GPU memory is managed through Metal's buffer system.
//!
//! The safe wrapper functions in metal.rs handle:
//! - Device selection and initialization
//! - Command queue creation
//! - Buffer allocation and data transfer
//! - Compute pipeline compilation and execution
//! - Synchronization and error handling

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
use metal::*;

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
use std::sync::Arc;

/// Metal device and command queue wrapper
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub struct MetalContext {
    pub device: Device,
    pub command_queue: CommandQueue,
    pub library: Library,
}

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
impl MetalContext {
    /// Create new Metal context with compiled shader library
    pub fn new() -> Result<Self, String> {
        // Get default Metal device (Apple GPU)
        let device = Device::system_default()
            .ok_or("No Metal device found")?;

        // Create command queue
        let command_queue = device.new_command_queue();

        // Load compiled Metal library (will be compiled by build.rs)
        let library_path = std::env::var("METAL_LIBRARY_PATH")
            .unwrap_or_else(|_| "target/metal/libproximadb_metal.metallib".to_string());

        let library = if std::path::Path::new(&library_path).exists() {
            device.new_library_with_file(&library_path)
                .map_err(|e| format!("Failed to load Metal library: {}", e))?
        } else {
            // Fallback: compile from source (slower, for development)
            let source = include_str!("kernels.metal");
            let options = CompileOptions::new();
            device.new_library_with_source(source, &options)
                .map_err(|e| format!("Failed to compile Metal shaders: {}", e))?
        };

        Ok(Self {
            device,
            command_queue,
            library,
        })
    }

    /// Get compute pipeline for a kernel function
    pub fn get_pipeline(&self, function_name: &str) -> Result<ComputePipelineState, String> {
        let function = self.library.get_function(function_name, None)
            .map_err(|e| format!("Function '{}' not found: {}", function_name, e))?;

        self.device.new_compute_pipeline_state_with_function(&function)
            .map_err(|e| format!("Failed to create pipeline: {}", e))
    }
}

/// Helper to create Metal buffer from slice
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub fn create_buffer<T>(device: &Device, data: &[T]) -> Buffer {
    let size = (data.len() * std::mem::size_of::<T>()) as u64;
    let buffer = device.new_buffer(size, MTLResourceOptions::StorageModeShared);

    unsafe {
        let ptr = buffer.contents() as *mut T;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
    }

    buffer
}

/// Helper to create empty Metal buffer
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub fn create_empty_buffer<T>(device: &Device, count: usize) -> Buffer {
    let size = (count * std::mem::size_of::<T>()) as u64;
    device.new_buffer(size, MTLResourceOptions::StorageModeShared)
}

/// Helper to read data from Metal buffer
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub fn read_buffer<T: Clone>(buffer: &Buffer, count: usize) -> Vec<T> {
    let mut result = Vec::with_capacity(count);
    unsafe {
        let ptr = buffer.contents() as *const T;
        result.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    result
}

/// Execute Metal compute kernel
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub fn execute_kernel(
    context: &MetalContext,
    pipeline: &ComputePipelineState,
    buffers: &[&Buffer],
    thread_count: usize,
) -> Result<(), String> {
    let command_buffer = context.command_queue.new_command_buffer();
    let encoder = command_buffer.new_compute_command_encoder();

    encoder.set_compute_pipeline_state(pipeline);

    // Set buffers
    for (i, buffer) in buffers.iter().enumerate() {
        encoder.set_buffer(i as u64, Some(*buffer), 0);
    }

    // Calculate threadgroup size
    let threadgroup_size = MTLSize {
        width: 256.min(thread_count as u64),
        height: 1,
        depth: 1,
    };

    let threadgroups = MTLSize {
        width: ((thread_count as u64 + 255) / 256),
        height: 1,
        depth: 1,
    };

    encoder.dispatch_thread_groups(threadgroups, threadgroup_size);
    encoder.end_encoding();

    command_buffer.commit();
    command_buffer.wait_until_completed();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
    fn test_metal_context_creation() {
        let result = MetalContext::new();
        // May fail if no GPU, but should not panic
        if let Ok(ctx) = result {
            assert!(!ctx.device.name().is_empty());
        }
    }

    #[test]
    #[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
    fn test_buffer_creation() {
        if let Ok(ctx) = MetalContext::new() {
            let data = vec![1.0f32, 2.0, 3.0, 4.0];
            let buffer = create_buffer(&ctx.device, &data);
            assert!(buffer.length() >= (data.len() * 4) as u64);

            let read_data: Vec<f32> = read_buffer(&buffer, data.len());
            assert_eq!(data, read_data);
        }
    }
}
