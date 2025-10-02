# SIMD/GPU Testing Guide for ProximaCodec

## Platform-Specific Test Guide

This document provides testing instructions for ProximaCodec's hardware-accelerated encoding/decoding across different platforms.

## Apple M4 MacBook Pro (ARM64)

### Expected Backends
1. **Primary**: MPS (Metal Performance Shaders) - GPU acceleration
2. **Fallback**: NEON - ARM SIMD (4x f32 parallelism)
3. **Last Resort**: Scalar

### Enabling MPS (Metal) Support

MPS requires the `gpu` feature flag to be enabled:

```bash
# Build with GPU support
cargo build --features gpu

# Test with GPU support
cargo test --features gpu --lib storage::engines::core::ops::proximacodec
```

### Without GPU Feature (NEON Only)

```bash
# Default build (NEON SIMD)
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_backend_detection -- --exact --nocapture

# Expected output:
# Backend: NEON
# Vector width: 4x f32
# Is SIMD: true
```

### Batching Tests (Optimized for 1K row groups)

```bash
# BERT embeddings (768D)
cargo test --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_bert_embeddings -- --exact --nocapture

# Expected for NEON:
# Row group: 1000 vectors × 768D = ~3MB
# Optimal batch: 1000 vectors (1 row group fits in cache)

# Expected for MPS (with --features gpu):
# Optimal batch: 2000-5000 vectors (2-5 row groups)
```

```bash
# OpenAI embeddings (1536D)
cargo test --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_openai_embeddings -- --exact --nocapture

# Expected for NEON:
# Row group: 1000 vectors × 1536D = ~6MB
# Optimal batch: 1000 vectors (1 row group)

# Expected for MPS (with --features gpu):
# Optimal batch: 2000-5000 vectors
```

### NEON SIMD Encoding Tests

```bash
# Delta encoding (NEON)
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_delta_encode_neon_small -- --exact --nocapture
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_delta_encode_neon_large -- --exact --nocapture

# BitPacked encoding (NEON)
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_bitpack_encode_neon_8bit -- --exact --nocapture
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_bitpack_encode_neon_variable_widths -- --exact --nocapture
```

---

## Intel/AMD x86_64 (AVX2/AVX512)

### Expected Backends
1. **Primary**: AVX-512 (if CPU supports) - 16x f32 parallelism
2. **Fallback**: AVX2 - 8x f32 parallelism
3. **Fallback**: SSE - 4x f32 parallelism
4. **Last Resort**: Scalar

### Backend Detection

```bash
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_backend_detection -- --exact --nocapture

# Expected output (AVX2 system):
# Backend: AVX2
# Vector width: 8x f32
# Is SIMD: true

# Expected output (AVX-512 system):
# Backend: AVX512
# Vector width: 16x f32
# Is SIMD: true
```

### Batching Tests (Optimized for L3 Cache)

```bash
# BERT embeddings (768D)
cargo test --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_bert_embeddings -- --exact --nocapture

# Expected for AVX2:
# Row group: 1000 vectors × 768D = ~3MB
# Optimal batch: 1000-5000 vectors (1-5 row groups fit in 16MB L3)

# Expected for AVX-512:
# Optimal batch: 1000-5000 vectors (similar, larger L3 cache)
```

```bash
# OpenAI embeddings (1536D)
cargo test --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_openai_embeddings -- --exact --nocapture

# Expected for AVX2:
# Row group: 1000 vectors × 1536D = ~6MB
# Optimal batch: 1000-2000 vectors (1-2 row groups, cache-limited)

# Expected for AVX-512:
# Optimal batch: 1000-3000 vectors (more L3 cache available)
```

### AVX2/AVX-512 Encoding Tests

```bash
# Delta encoding (AVX2)
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_delta_encode_neon_small -- --exact --nocapture
# Note: Test names say "neon" but automatically use AVX2 on x86_64

# BitPacked encoding (AVX2)
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests::test_bitpack_encode_neon_8bit -- --exact --nocapture
```

---

## NVIDIA GPU (CUDA)

### Prerequisites
- NVIDIA GPU with CUDA support
- CUDA toolkit installed
- `gpu` feature enabled

### Enabling CUDA Support

```bash
# Build with GPU support
cargo build --features gpu

# Run tests
cargo test --features gpu --lib storage::engines::core::ops::proximacodec
```

### Backend Detection

```bash
cargo test --features gpu --lib storage::engines::core::ops::proximacodec::simd::tests::test_backend_detection -- --exact --nocapture

# Expected output:
# Backend: CUDA
# Vector width: 1024 (GPU warp size)
# Is GPU: true
```

### Batching Tests (GPU Optimized)

```bash
# BERT embeddings (768D)
cargo test --features gpu --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_bert_embeddings -- --exact --nocapture

# Expected for CUDA:
# Row group: 1000 vectors × 768D = ~3MB
# Optimal batch: 5000-10000 vectors (5-10 row groups)
# Rationale: Amortize kernel launch overhead (~10μs)
```

```bash
# OpenAI embeddings (1536D)
cargo test --features gpu --lib storage::engines::core::ops::proximacodec::batching::tests::test_optimal_batch_size_openai_embeddings -- --exact --nocapture

# Expected for CUDA:
# Row group: 1000 vectors × 1536D = ~6MB
# Optimal batch: 5000-10000 vectors (30-60MB)
```

---

## AMD GPU (ROCm)

### Prerequisites
- AMD GPU with ROCm support
- ROCm toolkit installed
- `gpu` feature enabled

### Enabling ROCm Support

```bash
# Build with GPU support (ROCm auto-detected if available)
cargo build --features gpu

# Run tests
cargo test --features gpu --lib storage::engines::core::ops::proximacodec
```

### Expected Behavior
- Backend: ROCm
- Batch sizes: Similar to CUDA (5K-10K vectors)
- Performance: 10-50x vs scalar for large batches

---

## Cross-Platform Compatibility Tests

### Round-Trip Compatibility

These tests verify that SIMD/GPU encodings are bitwise-compatible with baseline scalar:

```bash
# Run on any platform
cargo test --lib storage::engines::core::ops::proximacodec::simd::tests --nocapture | grep "Round-trip"

# Expected output:
# ✅ Round-trip Delta encoding: SIMD matches baseline
# ✅ Round-trip BitPacked encoding: SIMD matches baseline
```

### Batch Round-Trip

```bash
cargo test --lib storage::engines::core::ops::proximacodec::batching::tests::test_batch_encode_decode_round_trip -- --exact --nocapture

# Expected output:
# ✅ Batch encoded 3 vectors
# ✅ Batch decoded 3 vectors
# ✅ Round-trip accuracy verified
```

---

## Performance Benchmarks

### SIMD vs Scalar

```bash
# Run encoding benchmarks
cargo bench --bench bench_encoding_simd

# Expected speedups:
# - Delta encoding: 2-4x (NEON), 3-5x (AVX2), 5-8x (AVX-512)
# - BitPacked: 3-6x (NEON), 4-8x (AVX2), 6-12x (AVX-512)
```

### GPU vs SIMD

```bash
# Run GPU benchmarks (requires --features gpu)
cargo bench --features gpu --bench bench_encoding_gpu

# Expected speedups (vs SIMD):
# - Batch < 1K vectors: SIMD faster (lower latency)
# - Batch 1K-10K vectors: GPU 2-5x faster
# - Batch > 10K vectors: GPU 10-50x faster
```

---

## Quick Test Suite

### Run All Tests (Current Platform)

```bash
# Without GPU
cargo test --lib storage::engines::core::ops::proximacodec

# With GPU
cargo test --features gpu --lib storage::engines::core::ops::proximacodec
```

### Run Specific Test Categories

```bash
# Backend detection
cargo test --lib proximacodec::simd::tests::test_backend -- --nocapture

# Batching tests
cargo test --lib proximacodec::batching::tests -- --nocapture

# SIMD encoding tests
cargo test --lib proximacodec::simd::tests::test_.*_encode -- --nocapture

# Memory pool tests
cargo test --lib proximacodec::simd::tests::test_memory_pool -- --nocapture
```

---

## Troubleshooting

### MPS Not Detected on M4

**Problem**: Backend shows Scalar instead of MPS

**Solution**: Enable GPU feature flag
```bash
cargo test --features gpu --lib storage::engines::core::ops::proximacodec
```

### NEON Not Detected on ARM64

**Problem**: Scalar backend on Apple Silicon

**Cause**: This is expected on some systems where SIMD is not exposed

**Verification**:
```bash
# Check CPU features
rustc --print cfg | grep neon

# If empty, NEON is not available at compile time
```

### AVX2 Not Used on Intel

**Problem**: SSE or Scalar used instead of AVX2

**Cause**: CPU doesn't support AVX2, or not enabled

**Verification**:
```bash
# Check CPU features
rustc --print cfg | grep avx

# Verify hardware support
cat /proc/cpuinfo | grep avx2  # Linux
sysctl -a | grep avx  # macOS
```

---

## Performance Expectations

### ProximaDB Row Group Workload (1K vectors)

| Backend | BERT (768D) Batch | OpenAI (1536D) Batch | Speedup vs Scalar |
|---------|-------------------|----------------------|-------------------|
| Scalar  | 100-500 vectors   | 100-500 vectors      | 1x (baseline)     |
| NEON    | 1K vectors        | 1K vectors           | 2-4x              |
| AVX2    | 1K-5K vectors     | 1K-2K vectors        | 3-5x              |
| AVX-512 | 1K-5K vectors     | 1K-3K vectors        | 5-8x              |
| MPS     | 2K-5K vectors     | 2K-5K vectors        | 5-15x             |
| CUDA    | 5K-10K vectors    | 5K-10K vectors       | 10-50x            |

### Memory Overhead

| Component | Overhead | Notes |
|-----------|----------|-------|
| VectorMemoryPool | <1% | Zero-allocation after warmup |
| Batching Framework | 0% | No additional allocations |
| SIMD Operations | 0% | In-place transformations |
| GPU Transfers | 10-20% | Pinned memory pools |

---

## Summary

- **M4 MacBook Pro**: Use `--features gpu` for MPS, otherwise NEON
- **Intel/AMD**: Automatic AVX2/AVX-512 detection
- **NVIDIA GPU**: Build with `--features gpu` for CUDA
- **AMD GPU**: Build with `--features gpu` for ROCm
- **Row Groups**: Optimized for 1K vectors × 768D/1536D
- **Batch Sizes**: Automatically adapted to backend and cache size

All backends are bitwise-compatible and share the same wire format.
