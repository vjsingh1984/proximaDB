# SIMD/GPU Integration Design for ProximaCodec

## Executive Summary

This document outlines the design for integrating SIMD and GPU acceleration into ProximaCodec, leveraging existing ProximaDB infrastructure (VectorMemoryPool, batching framework) for optimal performance with zero additional allocations.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     ProximaCodec                            │
│                  (Public API Layer)                         │
└───────────────────────┬─────────────────────────────────────┘
                        │
        ┌───────────────┴───────────────┐
        ▼                               ▼
┌──────────────────┐          ┌──────────────────┐
│ Wire Format      │          │ Acceleration     │
│ Manager          │          │ Router           │
│ - Headers        │          │ - Hardware detect│
│ - Versioning     │          │ - Backend select │
└──────────────────┘          └────────┬─────────┘
                                       │
                    ┌──────────────────┼──────────────────┐
                    ▼                  ▼                  ▼
            ┌───────────────┐  ┌───────────────┐  ┌───────────────┐
            │ GPU Backend   │  │ SIMD Backend  │  │ Scalar        │
            │ (feature=gpu) │  │ (cfg: arch)   │  │ (always)      │
            └───────┬───────┘  └───────┬───────┘  └───────┬───────┘
                    │                  │                  │
                    └──────────────────┴──────────────────┘
                                       │
                    ┌──────────────────┴──────────────────┐
                    ▼                                     ▼
            ┌──────────────────┐              ┌──────────────────┐
            │ VectorMemoryPool │              │ Batching         │
            │ - vector_buffers │              │ Framework        │
            │ - compression    │              │ - Parallel ops   │
            │ - metadata       │              │ - Optimal size   │
            └──────────────────┘              └──────────────────┘
```

## Design Principles

### 1. **Zero-Allocation Hot Paths**
- Use `VectorMemoryPool` for all intermediate buffers
- Pre-allocate working buffers per batch
- Reuse buffers across encode/decode operations
- RAII pattern: `PooledItem<T>` returns buffer automatically

### 2. **Batch-Aware Operations**
- Small batches (<100 vectors): Scalar
- Medium batches (100-1000 vectors): SIMD
- Large batches (>1000 vectors): GPU
- Automatic batch size detection and routing

### 3. **Cfg-Driven Compilation**
- GPU backends: `#[cfg(feature = "gpu")]`
- SIMD backends: `#[cfg(target_arch = "...")]`
- Always compile scalar fallback
- No compilation failures on unsupported platforms

### 4. **Graceful Degradation**
- GPU unavailable → SIMD
- SIMD unavailable → Scalar
- Hardware detection at runtime
- Never fail due to missing acceleration

## Memory Pooling Strategy

### VectorMemoryPool Integration

```rust
pub struct AcceleratedEncoder {
    /// Shared memory pool (Arc for cheap cloning across threads)
    memory_pool: Arc<VectorMemoryPool>,

    /// Hardware backend (cached once)
    backend: AccelerationBackend,

    /// Pool configuration (workload-specific)
    pool_config: PoolConfig,
}

impl AcceleratedEncoder {
    pub fn encode_with_pooling(&self, values: &[f32], scheme: &ProximaScheme) -> Result<Vec<u8>> {
        // Acquire pooled buffers (zero-allocation)
        let mut int_buffer = self.memory_pool.vector_buffers.acquire();
        let mut work_buffer = self.memory_pool.compression_buffers.acquire();

        // Buffers automatically returned on drop (RAII)
        match self.backend {
            AccelerationBackend::AVX2 => self.simd_encode_avx2(values, scheme, &mut int_buffer, &mut work_buffer),
            AccelerationBackend::CUDA => self.gpu_encode_cuda(values, scheme, &mut int_buffer, &mut work_buffer),
            AccelerationBackend::Scalar => self.scalar_encode(values, scheme),
        }
    }
}
```

### Pool Configuration by Backend

```rust
// GPU backend: Large buffers, high throughput
let gpu_config = PoolConfig {
    initial_size: 32,     // More buffers (parallel GPU streams)
    max_size: 512,        // Large pool (high throughput)
    min_size: 16,
    growth_factor: 2.0,   // Aggressive growth
    ..Default::default()
};

// SIMD backend: Medium buffers, balanced
let simd_config = PoolConfig {
    initial_size: 16,     // Moderate buffers
    max_size: 256,        // Medium pool
    min_size: 8,
    growth_factor: 1.5,   // Balanced growth
    ..Default::default()
};

// Scalar backend: Small buffers, minimal overhead
let scalar_config = PoolConfig {
    initial_size: 4,      // Few buffers
    max_size: 64,         // Small pool
    min_size: 2,
    growth_factor: 1.25,  // Conservative growth
    ..Default::default()
};
```

## Batching Framework Integration

### Batch Size Optimization

```rust
pub struct BatchOptimizer {
    backend: AccelerationBackend,
}

impl BatchOptimizer {
    /// Determine optimal batch size based on backend and data characteristics
    pub fn optimal_batch_size(&self, total_vectors: usize, dimension: usize) -> usize {
        match self.backend {
            // GPU: Large batches to amortize kernel launch overhead
            AccelerationBackend::CUDA | AccelerationBackend::ROCm => {
                // Target: ~1ms of GPU work per batch
                // Rule of thumb: 10K-100K vectors per batch
                let min_batch = 10_000;
                let max_batch = 100_000;
                total_vectors.min(max_batch).max(min_batch)
            }

            // MPS (Apple Metal): Medium-large batches
            AccelerationBackend::MPS => {
                // Target: Unified memory efficiency
                let min_batch = 5_000;
                let max_batch = 50_000;
                total_vectors.min(max_batch).max(min_batch)
            }

            // SIMD: Medium batches (cache-friendly)
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 |
            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                // Target: L3 cache fit (typically 8-32MB)
                // 1000 vectors × 768 dims × 4 bytes = ~3MB
                let cache_size = 8 * 1024 * 1024; // 8MB conservative
                let bytes_per_vector = dimension * 4;
                let cache_vectors = cache_size / bytes_per_vector;
                total_vectors.min(cache_vectors).max(100)
            }

            // Scalar: Small batches (minimize overhead)
            AccelerationBackend::Scalar => {
                total_vectors.min(1000).max(10)
            }
        }
    }

    /// Split data into optimal batches
    pub fn create_batches<T>(&self, data: &[T], dimension: usize) -> Vec<&[T]> {
        let batch_size = self.optimal_batch_size(data.len(), dimension);
        data.chunks(batch_size).collect()
    }
}
```

### Parallel Batch Processing

```rust
use rayon::prelude::*;

impl AcceleratedEncoder {
    /// Encode large dataset with automatic batching and parallel processing
    pub fn encode_dataset(&self, vectors: &[Vec<f32>], scheme: &ProximaScheme) -> Result<Vec<Vec<u8>>> {
        let optimizer = BatchOptimizer::new(self.backend);
        let batches = optimizer.create_batches(vectors, vectors[0].len());

        // Parallel batch encoding (when beneficial)
        let results: Result<Vec<_>> = if self.should_use_parallel(vectors.len()) {
            batches.par_iter()
                .map(|batch| self.encode_batch(batch, scheme))
                .collect()
        } else {
            batches.iter()
                .map(|batch| self.encode_batch(batch, scheme))
                .collect()
        };

        results
    }

    fn should_use_parallel(&self, total_vectors: usize) -> bool {
        match self.backend {
            // GPU: Don't use Rayon parallel (GPU handles parallelism)
            AccelerationBackend::CUDA | AccelerationBackend::ROCm |
            AccelerationBackend::MPS | AccelerationBackend::OpenCL => false,

            // SIMD: Use Rayon for large datasets
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 |
            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                total_vectors > 10_000
            }

            // Scalar: Use Rayon for medium+ datasets
            AccelerationBackend::Scalar => total_vectors > 5_000,
        }
    }
}
```

## Implementation Phases

### Phase 1: Infrastructure (Current)
- ✅ Backend detection (GPU, SIMD, Scalar)
- ✅ Cfg-driven compilation
- ✅ Basic SIMD Delta encoding
- ✅ Basic SIMD BitPacked encoding
- ⏳ VectorMemoryPool integration
- ⏳ Batching framework integration

### Phase 2: SIMD Encoders
- Delta (f32→i64, AVX2/NEON/SSE)
- BitPacked (variable bit-width)
- PForDelta (patched frame-of-reference)
- Zigzag (signed integer interleaving)
- Simple8b (variable bit-width in 32-bit words)
- VByte (variable-byte encoding)
- DoubleDelta (delta of deltas)
- Sparse (SparseBitmap, SparseCOO)

### Phase 3: SIMD Decoders
- Matching decoders for all encoders
- Pooled buffer management
- Batch decoding support

### Phase 4: GPU Encoders (feature="gpu")
- CUDA backend (NVIDIA)
- ROCm backend (AMD)
- MPS backend (Apple Metal)
- OpenCL backend (cross-platform)

### Phase 5: Integration & Optimization
- Integrate into ProximaCodec::encode()/decode()
- Automatic backend selection
- Performance tuning
- Comprehensive benchmarks

## Performance Targets

### Memory Allocation
- **Baseline**: 100 allocations per 1000 vectors
- **Target with pooling**: 0 allocations (hot path)
- **Measurement**: `#[cfg(test)] track_allocations!`

### Encoding Throughput
- **Scalar baseline**: 1x (reference)
- **SIMD (AVX2)**: 3-5x faster
- **SIMD (AVX-512)**: 5-8x faster
- **GPU (CUDA)**: 10-50x faster (large batches)

### Batch Size Sweet Spots
- **Scalar**: 10-1000 vectors
- **SIMD**: 100-10,000 vectors
- **GPU**: >1,000 vectors (ideal: 10K-100K)

## Testing Strategy

### Unit Tests
- Pooled buffer lifecycle (acquire/release)
- Batch size optimization
- Backend selection logic
- Memory pool statistics

### Integration Tests
- Round-trip encoding/decoding with pooling
- Cross-backend compatibility (SIMD vs GPU vs Scalar)
- Batch processing correctness
- Memory leak detection

### Performance Benchmarks
- Throughput by backend (ops/sec)
- Latency by batch size
- Memory allocation count
- Pool hit rate
- Batch size sensitivity

### Stress Tests
- Pool exhaustion handling
- Concurrent access (thread safety)
- Large dataset handling (>1M vectors)
- Mixed workloads (various batch sizes)

## Configuration

### TOML Configuration
```toml
[proximacodec]
# Backend selection (auto-detect by default)
backend = "auto"  # Options: auto, cuda, rocm, mps, opencl, avx512, avx2, neon, sse, scalar

# Memory pool configuration
[proximacodec.memory_pool]
initial_size = 16
max_size = 256
min_size = 4
max_idle_duration_secs = 300
growth_factor = 1.5
enable_stats = true

# Batching configuration
[proximacodec.batching]
auto_batch = true
min_batch_size = 100
max_batch_size = 100000
parallel_threshold = 10000  # Use Rayon above this size

# GPU-specific settings (requires feature="gpu")
[proximacodec.gpu]
device_id = 0
stream_count = 4  # Parallel GPU streams
unified_memory = true  # For MPS/unified memory architectures
```

## API Examples

### Basic Usage (Automatic)
```rust
let codec = ProximaCodec::global();
let values = vec![1.0f32, 2.0, 3.0];
let encoded = codec.encode(&values, ProximaScheme::Delta { base: 0 })?;
let decoded: Vec<f32> = codec.decode(&encoded)?;
// Backend automatically selected, pooling/batching transparent
```

### Advanced Usage (Explicit Backend)
```rust
let config = AcceleratorConfig {
    backend: AccelerationBackend::AVX2,
    pool_config: PoolConfig::default(),
    batch_size: Some(1000),
};

let encoder = AcceleratedEncoder::with_config(config);
let encoded = encoder.encode_batch(&vectors, &scheme)?;
```

### Batch Processing
```rust
let large_dataset: Vec<Vec<f32>> = load_million_vectors();
let codec = ProximaCodec::global();

// Automatically batched and parallelized
let encoded_batches = codec.encode_dataset(&large_dataset, scheme)?;
```

## Migration Path

1. **Phase 1**: Add pooling to existing SIMD functions (non-breaking)
2. **Phase 2**: Add batching support (optional parameter)
3. **Phase 3**: Enable by default with opt-out
4. **Phase 4**: Remove old non-pooled code paths

## Success Metrics

- ✅ Zero allocations on hot path
- ✅ 3-5x SIMD speedup vs scalar
- ✅ 10-50x GPU speedup vs scalar (large batches)
- ✅ <1% memory overhead (pooling)
- ✅ 100% compatibility (scalar fallback works)
- ✅ <5% performance variance across runs

## References

- `src/core/memory/pool.rs` - VectorMemoryPool implementation
- `src/storage/engines/core/formats/columnar/batch_operations.rs` - Batching framework
- `src/storage/engines/core/ops/unified_proxima_simd.obsolete/` - Previous SIMD implementation
- `docs/PERFORMANCE_COMPREHENSIVE.adoc` - Performance benchmarks
