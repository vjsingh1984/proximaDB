# GPU Acceleration Phase 2: Real Kernel Implementation Guide

## Overview

This document provides a step-by-step guide for implementing real GPU kernel compilation to replace the CPU fallbacks in Phase 1.

**Phase 1 Status**: ✅ Complete - Infrastructure with CPU fallbacks
**Phase 2 Goal**: Replace CPU fallbacks with real GPU kernel execution

---

## Prerequisites

### Development Environment

#### For CUDA (NVIDIA)
- CUDA Toolkit 11.0+ installed
- `nvcc` compiler in PATH
- NVIDIA GPU with compute capability 5.0+
- `cuda-sys` crate

#### For ROCm (AMD)
- ROCm 5.0+ installed
- `hipcc` compiler in PATH
- AMD GPU (Radeon RX 5000+, or MI series)
- `hip-sys` crate (or create FFI bindings)

#### For Metal (Apple)
- macOS 11.0+ (Big Sur or later)
- Xcode Command Line Tools
- Apple Silicon (M1/M2/M3/M4) or compatible GPU
- `metal-rs` crate

#### For OpenCL (Cross-platform)
- OpenCL 1.2+ runtime
- Platform-specific SDK (Intel, AMD, or NVIDIA)
- `opencl3` or `ocl` crate

---

## Phase 2 Implementation Plan

### Week 1-2: CUDA Implementation

#### Step 1.1: Create CUDA Kernel Files

Create `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/cuda/kernels.cu`:

```cuda
// CUDA kernel for Delta encoding
__global__ void delta_encode_f32_kernel(
    const float* input,
    int64_t* output,
    float base,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = (int64_t)(input[idx] - base);
    }
}

// CUDA kernel for Delta decoding
__global__ void delta_decode_f32_kernel(
    const int64_t* input,
    float* output,
    float base,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = (float)input[idx] + base;
    }
}

// BitPacked encoding kernel
__global__ void bitpack_encode_f32_kernel(
    const float* input,
    uint8_t* output,
    int bits,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        uint32_t val = __float_as_uint(input[idx]);
        uint32_t mask = (1u << bits) - 1u;
        uint32_t packed = val & mask;

        int bit_offset = idx * bits;
        int byte_offset = bit_offset / 8;
        int bit_in_byte = bit_offset % 8;

        atomicOr(&output[byte_offset], packed << bit_in_byte);
    }
}

// Add similar kernels for:
// - bitpack_decode_f32_kernel
// - frame_of_reference_encode_f32_kernel
// - frame_of_reference_decode_f32_kernel
// - zigzag_encode_f32_kernel
// - zigzag_decode_f32_kernel
```

#### Step 1.2: Create Build Script

Create `build.rs`:

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        build_cuda_kernels();
    }
}

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
fn build_cuda_kernels() {
    use std::process::Command;

    println!("cargo:rerun-if-changed=src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/cuda/kernels.cu");

    let out_dir = env::var("OUT_DIR").unwrap();
    let cuda_file = "src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/cuda/kernels.cu";

    // Compile CUDA kernels
    let status = Command::new("nvcc")
        .args(&[
            "--ptx",
            "-O3",
            "--gpu-architecture=compute_50",
            "--gpu-code=sm_50,sm_60,sm_70,sm_80",
            "-o",
            &format!("{}/cuda_kernels.ptx", out_dir),
            cuda_file,
        ])
        .status()
        .expect("Failed to compile CUDA kernels");

    assert!(status.success(), "CUDA compilation failed");

    // Link CUDA runtime
    println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
    println!("cargo:rustc-link-lib=cudart");
}
```

#### Step 1.3: Create FFI Bindings

Update `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/cuda.rs`:

```rust
use cuda_runtime_sys::{
    cudaMalloc, cudaMemcpy, cudaMemcpyKind, cudaFree,
    cudaMemcpyHostToDevice, cudaMemcpyDeviceToHost,
};

// Add at module level
static CUDA_MODULE: OnceLock<CudaModule> = OnceLock::new();

struct CudaModule {
    module: CUmodule,
    delta_encode_kernel: CUfunction,
    delta_decode_kernel: CUfunction,
    // ... other kernels
}

impl CudaModule {
    fn init() -> Result<Self> {
        // Load PTX from build output
        let ptx = include_bytes!(concat!(env!("OUT_DIR"), "/cuda_kernels.ptx"));

        // Initialize CUDA
        unsafe {
            cuInit(0);

            let mut module = std::ptr::null_mut();
            cuModuleLoadData(&mut module, ptx.as_ptr() as *const _);

            // Get kernel functions
            let mut delta_encode_kernel = std::ptr::null_mut();
            cuModuleGetFunction(&mut delta_encode_kernel, module,
                              b"delta_encode_f32_kernel\0".as_ptr() as *const _);

            Ok(Self {
                module,
                delta_encode_kernel,
                // ... initialize other kernels
            })
        }
    }
}

// Replace CPU fallback in cuda_delta_encode_f32
pub fn cuda_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    let module = CUDA_MODULE.get_or_init(|| CudaModule::init().unwrap());

    unsafe {
        // Allocate device memory
        let mut d_input: *mut f32 = std::ptr::null_mut();
        let mut d_output: *mut i64 = std::ptr::null_mut();

        cudaMalloc(&mut d_input as *mut _ as *mut _, values.len() * 4);
        cudaMalloc(&mut d_output as *mut _ as *mut _, values.len() * 8);

        // Copy input to device
        cudaMemcpy(
            d_input as *mut _,
            values.as_ptr() as *const _,
            values.len() * 4,
            cudaMemcpyHostToDevice,
        );

        // Launch kernel
        let block_size = 256;
        let grid_size = (values.len() + block_size - 1) / block_size;

        let args = [
            &d_input as *const _ as *mut _,
            &d_output as *const _ as *mut _,
            &base as *const _ as *mut _,
            &values.len() as *const _ as *mut _,
        ];

        cuLaunchKernel(
            module.delta_encode_kernel,
            grid_size as u32, 1, 1,
            block_size as u32, 1, 1,
            0, std::ptr::null_mut(),
            args.as_ptr() as *mut _,
            std::ptr::null_mut(),
        );

        // Copy result back
        let mut result = vec![0i64; values.len()];
        cudaMemcpy(
            result.as_mut_ptr() as *mut _,
            d_output as *const _,
            values.len() * 8,
            cudaMemcpyDeviceToHost,
        );

        // Free device memory
        cudaFree(d_input as *mut _);
        cudaFree(d_output as *mut _);

        Ok(result)
    }
}
```

---

### Week 3-4: Metal/MPS Implementation

#### Step 2.1: Create Metal Shader File

Create `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/metal/kernels.metal`:

```metal
#include <metal_stdlib>
using namespace metal;

// Delta encoding kernel
kernel void delta_encode_f32(
    device const float* input [[buffer(0)]],
    device long* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    output[gid] = (long)(input[gid] - base);
}

// Delta decoding kernel
kernel void delta_decode_f32(
    device const long* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    output[gid] = (float)input[gid] + base;
}

// BitPacked encoding kernel
kernel void bitpack_encode_f32(
    device const float* input [[buffer(0)]],
    device atomic_uint* output [[buffer(1)]],
    constant uint& bits [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    uint val = as_type<uint>(input[gid]);
    uint mask = (1u << bits) - 1u;
    uint packed = val & mask;

    uint bit_offset = gid * bits;
    uint byte_offset = bit_offset / 8;
    uint bit_in_byte = bit_offset % 8;

    atomic_fetch_or_explicit(&output[byte_offset], packed << bit_in_byte,
                            memory_order_relaxed);
}

// Add similar kernels for other schemes
```

#### Step 2.2: Compile Metal Shaders

Update `build.rs`:

```rust
#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
fn build_metal_shaders() {
    use std::process::Command;

    let out_dir = env::var("OUT_DIR").unwrap();
    let metal_file = "src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/metal/kernels.metal";

    // Compile Metal shaders to .air
    Command::new("xcrun")
        .args(&[
            "-sdk", "macosx",
            "metal",
            "-c",
            metal_file,
            "-o",
            &format!("{}/kernels.air", out_dir),
        ])
        .status()
        .expect("Failed to compile Metal shaders");

    // Create .metallib
    Command::new("xcrun")
        .args(&[
            "-sdk", "macosx",
            "metallib",
            &format!("{}/kernels.air", out_dir),
            "-o",
            &format!("{}/kernels.metallib", out_dir),
        ])
        .status()
        .expect("Failed to create Metal library");
}
```

#### Step 2.3: Metal FFI Integration

Update `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/metal.rs`:

```rust
use metal::{Device, CommandQueue, Library, ComputePipelineState, Buffer};

static METAL_CONTEXT: OnceLock<MetalContext> = OnceLock::new();

struct MetalContext {
    device: Device,
    queue: CommandQueue,
    library: Library,
    delta_encode_pipeline: ComputePipelineState,
    // ... other pipelines
}

impl MetalContext {
    fn init() -> Result<Self> {
        let device = Device::system_default()
            .ok_or_else(|| anyhow::anyhow!("No Metal device found"))?;

        let queue = device.new_command_queue();

        // Load compiled metal library
        let lib_data = include_bytes!(concat!(env!("OUT_DIR"), "/kernels.metallib"));
        let library = device.new_library_with_data(lib_data)?;

        // Create compute pipeline states
        let delta_encode_fn = library.get_function("delta_encode_f32", None)?;
        let delta_encode_pipeline = device.new_compute_pipeline_state_with_function(&delta_encode_fn)?;

        Ok(Self {
            device,
            queue,
            library,
            delta_encode_pipeline,
        })
    }
}

// Replace CPU fallback
pub fn metal_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    let ctx = METAL_CONTEXT.get_or_init(|| MetalContext::init().unwrap());

    // Create buffers
    let input_buffer = ctx.device.new_buffer_with_data(
        values.as_ptr() as *const _,
        (values.len() * 4) as u64,
        metal::MTLResourceOptions::StorageModeShared,
    );

    let output_buffer = ctx.device.new_buffer(
        (values.len() * 8) as u64,
        metal::MTLResourceOptions::StorageModeShared,
    );

    let base_buffer = ctx.device.new_buffer_with_data(
        &base as *const _ as *const _,
        4,
        metal::MTLResourceOptions::StorageModeShared,
    );

    // Create command buffer and encoder
    let command_buffer = ctx.queue.new_command_buffer();
    let encoder = command_buffer.new_compute_command_encoder();

    encoder.set_compute_pipeline_state(&ctx.delta_encode_pipeline);
    encoder.set_buffer(0, Some(&input_buffer), 0);
    encoder.set_buffer(1, Some(&output_buffer), 0);
    encoder.set_buffer(2, Some(&base_buffer), 0);

    // Calculate thread groups
    let thread_group_size = metal::MTLSize::new(256, 1, 1);
    let thread_groups = metal::MTLSize::new(
        (values.len() as u64 + 255) / 256,
        1,
        1,
    );

    encoder.dispatch_thread_groups(thread_groups, thread_group_size);
    encoder.end_encoding();

    command_buffer.commit();
    command_buffer.wait_until_completed();

    // Read results
    let result_ptr = output_buffer.contents() as *const i64;
    let result = unsafe {
        std::slice::from_raw_parts(result_ptr, values.len()).to_vec()
    };

    Ok(result)
}
```

---

### Week 5-6: OpenCL Implementation

#### Step 3.1: Create OpenCL Kernel Files

Create `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/opencl/kernels.cl`:

```opencl
// Delta encoding kernel
__kernel void delta_encode_f32(
    __global const float* input,
    __global long* output,
    const float base,
    const int n
) {
    int gid = get_global_id(0);
    if (gid < n) {
        output[gid] = (long)(input[gid] - base);
    }
}

// Delta decoding kernel
__kernel void delta_decode_f32(
    __global const long* input,
    __global float* output,
    const float base,
    const int n
) {
    int gid = get_global_id(0);
    if (gid < n) {
        output[gid] = (float)input[gid] + base;
    }
}

// Add similar kernels for other schemes
```

#### Step 3.2: OpenCL Runtime Integration

Update `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/opencl.rs`:

```rust
use opencl3::platform::Platform;
use opencl3::device::{Device, CL_DEVICE_TYPE_GPU};
use opencl3::context::Context;
use opencl3::command_queue::{CommandQueue, CL_QUEUE_PROFILING_ENABLE};
use opencl3::kernel::{Kernel, ExecuteKernel};
use opencl3::memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_WRITE_ONLY};
use opencl3::program::Program;

static OPENCL_CONTEXT: OnceLock<OpenCLContext> = OnceLock::new();

struct OpenCLContext {
    context: Context,
    queue: CommandQueue,
    program: Program,
}

impl OpenCLContext {
    fn init() -> Result<Self> {
        // Get platform and device
        let platform = Platform::default();
        let device = Device::new(platform.devices(CL_DEVICE_TYPE_GPU)?[0]);

        // Create context and queue
        let context = Context::from_device(&device)?;
        let queue = CommandQueue::create_with_properties(
            &context,
            device.id(),
            CL_QUEUE_PROFILING_ENABLE,
            0,
        )?;

        // Load and build kernel source
        let kernel_source = include_str!("opencl/kernels.cl");
        let program = Program::create_and_build_from_source(&context, kernel_source, "")?;

        Ok(Self { context, queue, program })
    }
}

// Replace CPU fallback
pub fn opencl_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    let ctx = OPENCL_CONTEXT.get_or_init(|| OpenCLContext::init().unwrap());

    // Create buffers
    let input_buffer = Buffer::<f32>::create(&ctx.context, CL_MEM_READ_ONLY, values.len(), None)?;
    let output_buffer = Buffer::<i64>::create(&ctx.context, CL_MEM_WRITE_ONLY, values.len(), None)?;

    // Write input data
    ctx.queue.enqueue_write_buffer(&mut input_buffer, CL_NON_BLOCKING, 0, values, &[])?;

    // Create and execute kernel
    let kernel = Kernel::create(&ctx.program, "delta_encode_f32")?;

    let kernel_event = ExecuteKernel::new(&kernel)
        .set_arg(&input_buffer)
        .set_arg(&output_buffer)
        .set_arg(&base)
        .set_arg(&(values.len() as i32))
        .set_global_work_size(values.len())
        .set_local_work_size(256)
        .enqueue_nd_range(&ctx.queue)?;

    kernel_event.wait()?;

    // Read results
    let mut result = vec![0i64; values.len()];
    ctx.queue.enqueue_read_buffer(&output_buffer, CL_BLOCKING, 0, &mut result, &[])?;

    Ok(result)
}
```

---

### Week 7-8: ROCm/HIP Implementation

Similar to CUDA but using HIP:

#### Step 4.1: Create HIP Kernel Files

Create `src/storage/engines/core/ops/proximacodec/impls/gpu/kernels/rocm/kernels.hip`:

```cpp
// HIP kernels (similar to CUDA)
__global__ void delta_encode_f32_kernel(
    const float* input,
    int64_t* output,
    float base,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = (int64_t)(input[idx] - base);
    }
}

// Add similar kernels
```

#### Step 4.2: Build with hipcc

Update `build.rs` to compile HIP kernels using `hipcc`.

---

## Testing Strategy

### Unit Tests per Backend

```rust
#[cfg(all(test, feature = "gpu"))]
mod gpu_tests {
    use super::*;

    #[test]
    fn test_cuda_real_kernel() {
        if !is_cuda_available() {
            return;
        }

        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = cuda_delta_encode_f32(&values, 0.0).unwrap();

        assert_eq!(result.len(), 4);
        assert_eq!(result[0], 1);
        assert_eq!(result[1], 2);
    }

    // Add similar tests for other backends
}
```

### Integration Tests

```rust
#[test]
fn test_gpu_encoder_with_real_kernels() {
    let encoder = GpuEncoder;
    let values = vec![1.0f32; 10000];
    let scheme = ProximaScheme::Delta { base: 0 };

    let encoded = encoder.encode_f32(&values, &scheme).unwrap();
    assert!(encoded.len() > 0);
}
```

---

## Performance Benchmarking

Create `benches/gpu_acceleration_bench.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

fn benchmark_encoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("encoding");

    for size in [1000, 10000, 100000].iter() {
        let values: Vec<f32> = (0..*size).map(|i| i as f32).collect();

        // SIMD baseline
        group.bench_with_input(BenchmarkId::new("SIMD", size), size, |b, _| {
            b.iter(|| {
                simd_delta_encode_f32(&values, 0.0)
            });
        });

        // GPU acceleration
        group.bench_with_input(BenchmarkId::new("GPU", size), size, |b, _| {
            b.iter(|| {
                cuda_delta_encode_f32(&values, 0.0)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_encoding);
criterion_main!(benches);
```

---

## Rollout Plan

### Phase 2.1: CUDA Only (Weeks 1-2)
- Implement CUDA kernels
- Test on NVIDIA GPUs
- Benchmark vs SIMD
- Fix issues

### Phase 2.2: Add Metal (Weeks 3-4)
- Implement Metal shaders
- Test on Apple Silicon
- Benchmark vs SIMD
- Fix issues

### Phase 2.3: Add OpenCL (Weeks 5-6)
- Implement OpenCL kernels
- Test on multiple platforms
- Benchmark vs SIMD
- Fix issues

### Phase 2.4: Add ROCm (Weeks 7-8)
- Implement HIP kernels
- Test on AMD GPUs
- Benchmark vs SIMD
- Fix issues

### Phase 2.5: Optimization & Polish (Weeks 9-10)
- Performance tuning
- Memory optimization
- Documentation updates
- Final benchmarks

---

## Success Criteria

- ✅ All GPU backends compile and run
- ✅ GPU kernels faster than SIMD (5-15x speedup)
- ✅ All tests passing on real hardware
- ✅ Memory usage within acceptable limits
- ✅ Graceful fallback still works
- ✅ Documentation complete

---

## License

Copyright (C) 2025 ProximaDB
SPDX-License-Identifier: Apache-2.0
