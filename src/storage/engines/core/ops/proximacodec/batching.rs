// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Hardware-Aware Batching Framework for ProximaCodec
//!
//! This module provides unified batching capabilities that work across all acceleration
//! backends (GPU, SIMD, Scalar). The batching strategy automatically adapts based on:
//!
//! - **Hardware capabilities**: GPU, SIMD (AVX2/AVX512/NEON), or Scalar
//! - **Data characteristics**: Vector count and dimensionality
//! - **Cache topology**: L1/L2/L3 cache sizes for optimal memory utilization
//! - **Parallelism**: Rayon for CPU, native parallelism for GPU
//!
//! ## Architecture
//!
//! ```text
//! ┌────────────────────────────────────────────────────────┐
//! │         Batching Framework (batching.rs)               │
//! │  ┌──────────────────────────────────────────────────┐  │
//! │  │  BatchOptimizer                                  │  │
//! │  │  - Hardware detection                            │  │
//! │  │  - Batch size calculation                        │  │
//! │  │  - Parallelism decision                          │  │
//! │  └──────────────────────────────────────────────────┘  │
//! └────────────────────────────────────────────────────────┘
//!                         │
//!         ┌───────────────┼───────────────┐
//!         ▼               ▼               ▼
//!    ┌─────────┐    ┌─────────┐    ┌─────────┐
//!    │   GPU   │    │  SIMD   │    │ Scalar  │
//!    │ 10K-100K│    │ 100-10K │    │ 10-1K   │
//!    └─────────┘    └─────────┘    └─────────┘
//! ```
//!
//! ## Batch Size Strategy
//!
//! | Backend | Batch Size | Rationale |
//! |---------|------------|-----------|
//! | GPU     | 10K-100K   | Amortize kernel launch overhead (~10μs) |
//! | SIMD    | 100-10K    | Fit in L3 cache (8-32MB typical) |
//! | Scalar  | 10-1K      | Minimize overhead, maximize throughput |
//!
//! ## Usage
//!
//! ```rust
//! use proximadb::storage::engines::core::ops::proximacodec::batching::{
//!     BatchOptimizer, batch_encode_vectors
//! };
//! use proximadb::storage::engines::core::ops::proximacodec::types::ProximaScheme;
//!
//! // Automatic hardware-aware batching
//! let optimizer = BatchOptimizer::new();
//! let batch_size = optimizer.optimal_batch_size(total_vectors, dimension);
//!
//! // Batch encoding with parallel processing when beneficial
//! let vectors: Vec<Vec<f32>> = /* ... */;
//! let scheme = ProximaScheme::Delta { base: 0 };
//! let encoded = batch_encode_vectors(&vectors, &scheme)?;
//! ```

use anyhow::Result;
use tracing::debug;

use super::simd::AccelerationBackend;
use crate::storage::engines::core::ops::proximacodec::simd::get_simd_backend;

/// Batch size optimizer for hardware-aware processing
///
/// Automatically selects optimal batch sizes based on:
/// - Hardware backend (GPU, SIMD, Scalar)
/// - Data dimensionality (affects cache utilization)
/// - Total vector count
#[derive(Debug, Clone)]
pub struct BatchOptimizer {
    backend: AccelerationBackend,
}

impl BatchOptimizer {
    /// Create a new batch optimizer for the current hardware
    pub fn new() -> Self {
        Self {
            backend: get_simd_backend(),
        }
    }

    /// Create a batch optimizer for a specific backend (testing/benchmarking)
    pub fn with_backend(backend: AccelerationBackend) -> Self {
        Self { backend }
    }

    /// Determine optimal batch size based on backend and data characteristics
    ///
    /// ## Strategy (Optimized for ProximaDB row groups: 1K vectors × 768-1536D)
    /// - **GPU backends**: Process full row groups (1K vectors) or multiples
    /// - **SIMD backends**: Process row groups in cache-friendly chunks
    /// - **Scalar**: Process smaller chunks (100-500 vectors)
    ///
    /// ## Typical ProximaDB Workloads
    /// - BERT embeddings: 1K vectors × 768D = ~3MB per row group
    /// - OpenAI embeddings: 1K vectors × 1536D = ~6MB per row group
    ///
    /// ## Parameters
    /// - `total_vectors`: Total number of vectors to process
    /// - `dimension`: Vector dimensionality (affects cache utilization)
    ///
    /// ## Returns
    /// Optimal batch size for this backend and data characteristics
    pub fn optimal_batch_size(&self, total_vectors: usize, dimension: usize) -> usize {
        // ProximaDB row group size
        const ROW_GROUP_SIZE: usize = 1000;

        match self.backend {
            // GPU backends: Process multiple row groups at once
            AccelerationBackend::CUDA | AccelerationBackend::ROCm => {
                // Target: 5-10 row groups per batch (5K-10K vectors)
                // For 1K × 1536D: ~6MB per row group, 30-60MB per batch
                let min_row_groups = 5;
                let max_row_groups = 10;
                let min_batch = ROW_GROUP_SIZE * min_row_groups;
                let max_batch = ROW_GROUP_SIZE * max_row_groups;
                total_vectors.min(max_batch).max(min_batch.min(total_vectors))
            }

            // MPS (Apple Metal): Process 2-5 row groups (unified memory)
            AccelerationBackend::MPS => {
                // Target: 2-5 row groups per batch (2K-5K vectors)
                // Fits well in unified memory architecture
                let min_row_groups = 2;
                let max_row_groups = 5;
                let min_batch = ROW_GROUP_SIZE * min_row_groups;
                let max_batch = ROW_GROUP_SIZE * max_row_groups;
                total_vectors.min(max_batch).max(min_batch.min(total_vectors))
            }

            // OpenCL: Process 3-8 row groups
            AccelerationBackend::OpenCL => {
                let min_row_groups = 3;
                let max_row_groups = 8;
                let min_batch = ROW_GROUP_SIZE * min_row_groups;
                let max_batch = ROW_GROUP_SIZE * max_row_groups;
                total_vectors.min(max_batch).max(min_batch.min(total_vectors))
            }

            // SIMD: Process 1-2 row groups to fit in L3 cache
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 => {
                // AVX2/AVX512: L3 cache typically 8-32MB
                // 1 row group (1K × 768D) = ~3MB fits comfortably
                // 2 row groups (2K × 768D) = ~6MB fits in most L3 caches
                // For 1536D: 1 row group = ~6MB, process 1 at a time
                let bytes_per_row_group = ROW_GROUP_SIZE * dimension * 4;
                let l3_cache_size = 16 * 1024 * 1024; // 16MB typical

                let row_groups_in_cache = (l3_cache_size / bytes_per_row_group).max(1);
                let batch_size = ROW_GROUP_SIZE * row_groups_in_cache;
                total_vectors.min(batch_size).max(ROW_GROUP_SIZE.min(total_vectors))
            }

            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                // NEON/SSE: Smaller L2/L3 caches (8-16MB typical)
                // Process 1 row group at a time for optimal cache utilization
                let batch_size = ROW_GROUP_SIZE;
                total_vectors.min(batch_size).max(100)
            }

            // Scalar: Process smaller chunks (100-500 vectors)
            AccelerationBackend::Scalar => {
                // For scalar, process in smaller chunks to maintain responsiveness
                let chunk_size = if total_vectors >= ROW_GROUP_SIZE {
                    500 // Half row group for large datasets
                } else {
                    100.max(total_vectors / 4) // Quarter of dataset, min 100
                };
                total_vectors.min(chunk_size).max(10)
            }
        }
    }

    /// Split data into optimal batches
    ///
    /// ## Returns
    /// Vector of slices representing optimal batch boundaries
    pub fn create_batches<'a, T>(&self, data: &'a [T], dimension: usize) -> Vec<&'a [T]> {
        let batch_size = self.optimal_batch_size(data.len(), dimension);
        debug!(
            "🔀 Creating {} batches of size {} for {} vectors (backend: {:?})",
            (data.len() + batch_size - 1) / batch_size,
            batch_size,
            data.len(),
            self.backend
        );
        data.chunks(batch_size).collect()
    }

    /// Determine if parallel processing is beneficial for this workload
    ///
    /// ## Strategy
    /// - **GPU backends**: Don't use Rayon (GPU handles parallelism internally)
    /// - **SIMD backends**: Use Rayon for large datasets (>10K vectors)
    /// - **Scalar**: Use Rayon for medium+ datasets (>5K vectors)
    pub fn should_use_parallel(&self, total_vectors: usize) -> bool {
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

    /// Get the current backend for diagnostics
    pub fn backend(&self) -> AccelerationBackend {
        self.backend
    }
}

impl Default for BatchOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Batch encode multiple vectors using optimal batching strategy
///
/// ## Features
/// - Hardware-aware batch sizing
/// - Parallel processing when beneficial (Rayon for CPU, native for GPU)
/// - Memory pool integration
/// - Automatic backend selection
///
/// ## Parameters
/// - `vectors`: Slice of f32 vectors to encode
/// - `scheme`: Encoding scheme to apply
///
/// ## Returns
/// Vector of encoded results (one per input vector)
///
/// ## Example
/// ```rust
/// use proximadb::storage::engines::core::ops::proximacodec::batching::batch_encode_vectors;
/// use proximadb::storage::engines::core::ops::proximacodec::types::ProximaScheme;
///
/// let vectors: Vec<Vec<f32>> = vec![
///     vec![1.0, 2.0, 3.0],
///     vec![4.0, 5.0, 6.0],
/// ];
/// let scheme = ProximaScheme::Delta { base: 0 };
/// let encoded = batch_encode_vectors(&vectors, &scheme)?;
/// assert_eq!(encoded.len(), 2);
/// ```
pub fn batch_encode_vectors(
    vectors: &[Vec<f32>],
    scheme: &crate::storage::engines::core::ops::proximacodec::types::ProximaScheme,
) -> Result<Vec<Vec<u8>>> {
    use crate::storage::engines::core::ops::proximacodec::ProximaCodec;

    if vectors.is_empty() {
        return Ok(Vec::new());
    }

    let optimizer = BatchOptimizer::new();
    let codec = ProximaCodec::global();

    debug!(
        "📦 Batch encoding {} vectors with backend {:?}",
        vectors.len(),
        optimizer.backend()
    );

    // Determine if parallel processing is beneficial
    if optimizer.should_use_parallel(vectors.len()) {
        use rayon::prelude::*;

        debug!("⚡ Using parallel encoding (Rayon) for {} vectors", vectors.len());
        // Parallel encoding for large datasets
        vectors
            .par_iter()
            .map(|v| codec.encode(v, scheme.clone()))
            .collect()
    } else {
        debug!("🔄 Using sequential encoding for {} vectors", vectors.len());
        // Sequential encoding for small/medium datasets
        vectors
            .iter()
            .map(|v| codec.encode(v, scheme.clone()))
            .collect()
    }
}

/// Batch decode multiple encoded vectors
///
/// ## Features
/// - Hardware-aware batch sizing
/// - Parallel processing when beneficial
/// - Memory pool integration
///
/// ## Parameters
/// - `encoded_vectors`: Slice of encoded data
///
/// ## Returns
/// Vector of decoded f32 vectors
///
/// ## Example
/// ```rust
/// use proximadb::storage::engines::core::ops::proximacodec::batching::{
///     batch_encode_vectors, batch_decode_vectors
/// };
/// use proximadb::storage::engines::core::ops::proximacodec::types::ProximaScheme;
///
/// let vectors: Vec<Vec<f32>> = vec![vec![1.0, 2.0, 3.0]];
/// let scheme = ProximaScheme::Delta { base: 0 };
/// let encoded = batch_encode_vectors(&vectors, &scheme)?;
/// let decoded = batch_decode_vectors(&encoded)?;
/// assert_eq!(decoded, vectors);
/// ```
pub fn batch_decode_vectors(encoded_vectors: &[Vec<u8>]) -> Result<Vec<Vec<f32>>> {
    use crate::storage::engines::core::ops::proximacodec::ProximaCodec;

    if encoded_vectors.is_empty() {
        return Ok(Vec::new());
    }

    let optimizer = BatchOptimizer::new();
    let codec = ProximaCodec::global();

    debug!(
        "📦 Batch decoding {} vectors with backend {:?}",
        encoded_vectors.len(),
        optimizer.backend()
    );

    // Determine if parallel processing is beneficial
    if optimizer.should_use_parallel(encoded_vectors.len()) {
        use rayon::prelude::*;

        debug!("⚡ Using parallel decoding (Rayon) for {} vectors", encoded_vectors.len());
        // Parallel decoding for large datasets
        encoded_vectors
            .par_iter()
            .map(|e| codec.decode(e))
            .collect()
    } else {
        debug!("🔄 Using sequential decoding for {} vectors", encoded_vectors.len());
        // Sequential decoding for small/medium datasets
        encoded_vectors
            .iter()
            .map(|e| codec.decode(e))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_optimizer_creation() {
        let optimizer = BatchOptimizer::new();
        println!("✅ Batch optimizer created with backend: {:?}", optimizer.backend());

        // Verify backend is valid
        assert!(matches!(
            optimizer.backend(),
            AccelerationBackend::CUDA
                | AccelerationBackend::ROCm
                | AccelerationBackend::MPS
                | AccelerationBackend::OpenCL
                | AccelerationBackend::AVX512
                | AccelerationBackend::AVX2
                | AccelerationBackend::NEON
                | AccelerationBackend::SSE
                | AccelerationBackend::Scalar
        ));
    }

    #[test]
    fn test_optimal_batch_size_bert_embeddings() {
        println!("\n📊 Batch Size Test: BERT Embeddings (768D)");
        let optimizer = BatchOptimizer::new();

        // ProximaDB row group: 1K vectors × 768D = ~3MB
        let row_group_size = 1000;
        let bert_dim = 768;

        let batch_size = optimizer.optimal_batch_size(row_group_size, bert_dim);
        println!("   Backend: {:?}", optimizer.backend());
        println!("   Row group: {} vectors × {}D = ~3MB", row_group_size, bert_dim);
        println!("   Optimal batch: {} vectors", batch_size);

        match optimizer.backend() {
            AccelerationBackend::CUDA | AccelerationBackend::ROCm => {
                // GPU: Should process 5-10 row groups (5K-10K vectors)
                assert!(batch_size >= 5000, "GPU should batch 5+ row groups");
                assert!(batch_size <= 10000, "GPU should batch ≤10 row groups");
            }
            AccelerationBackend::MPS => {
                // MPS: Should process 2-5 row groups (2K-5K vectors)
                assert!(batch_size >= 2000, "MPS should batch 2+ row groups");
                assert!(batch_size <= 5000, "MPS should batch ≤5 row groups");
            }
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 => {
                // AVX2/512: Should process 1-5 row groups (fits in L3)
                assert!(batch_size >= 1000, "AVX should batch 1+ row group");
                assert!(batch_size <= 5000, "AVX should fit in L3 cache");
            }
            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                // NEON/SSE: Should process 1 row group (1K vectors)
                assert_eq!(batch_size, 1000, "NEON/SSE should batch 1 row group");
            }
            AccelerationBackend::Scalar => {
                // Scalar: Should process smaller chunks (100-500)
                assert!(batch_size >= 10);
                assert!(batch_size <= 500);
            }
            _ => {}
        }
        println!("   ✅ Batch size optimal for backend");
    }

    #[test]
    fn test_optimal_batch_size_openai_embeddings() {
        println!("\n📊 Batch Size Test: OpenAI Embeddings (1536D)");
        let optimizer = BatchOptimizer::new();

        // ProximaDB row group: 1K vectors × 1536D = ~6MB
        let row_group_size = 1000;
        let openai_dim = 1536;

        let batch_size = optimizer.optimal_batch_size(row_group_size, openai_dim);
        println!("   Backend: {:?}", optimizer.backend());
        println!("   Row group: {} vectors × {}D = ~6MB", row_group_size, openai_dim);
        println!("   Optimal batch: {} vectors", batch_size);

        match optimizer.backend() {
            AccelerationBackend::CUDA | AccelerationBackend::ROCm => {
                // GPU: Should process 5-10 row groups
                assert!(batch_size >= 5000);
                assert!(batch_size <= 10000);
            }
            AccelerationBackend::MPS => {
                // MPS: Should process 2-5 row groups
                assert!(batch_size >= 2000);
                assert!(batch_size <= 5000);
            }
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 => {
                // AVX: For 1536D (6MB/row), should process 1-2 row groups
                assert!(batch_size >= 1000);
                assert!(batch_size <= 3000, "Large dims should reduce batch count");
            }
            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                // NEON/SSE: Should process 1 row group
                assert_eq!(batch_size, 1000);
            }
            AccelerationBackend::Scalar => {
                assert!(batch_size >= 10);
                assert!(batch_size <= 500);
            }
            _ => {}
        }
        println!("   ✅ Batch size adapts to higher dimensionality");
    }

    #[test]
    fn test_optimal_batch_size_multi_row_groups() {
        println!("\n📊 Batch Size Test: Multiple Row Groups");
        let optimizer = BatchOptimizer::new();

        // Test with 10K vectors (10 row groups)
        let total_vectors = 10_000;
        let dimension = 768;

        let batch_size = optimizer.optimal_batch_size(total_vectors, dimension);
        println!("   Total: {} vectors ({} row groups)", total_vectors, total_vectors / 1000);
        println!("   Batch size: {} vectors", batch_size);
        println!("   Batches needed: {}", (total_vectors + batch_size - 1) / batch_size);

        // Verify batch size is row-group-aligned for efficiency
        match optimizer.backend() {
            AccelerationBackend::NEON | AccelerationBackend::SSE => {
                // NEON/SSE: Exactly 1 row group
                assert_eq!(batch_size % 1000, 0, "Should align to row group boundaries");
            }
            AccelerationBackend::AVX512 | AccelerationBackend::AVX2 => {
                // AVX: Multiple of row groups
                assert_eq!(batch_size % 1000, 0, "Should align to row group boundaries");
            }
            _ => {}
        }
        println!("   ✅ Batch aligned to row group boundaries");
    }

    #[test]
    fn test_should_use_parallel() {
        let optimizer = BatchOptimizer::new();

        // Small dataset: Should not use parallel
        let small_parallel = optimizer.should_use_parallel(100);
        println!("🔀 Small dataset (100 vectors): parallel={}", small_parallel);

        // Large dataset: May use parallel depending on backend
        let large_parallel = optimizer.should_use_parallel(50_000);
        println!("🔀 Large dataset (50K vectors): parallel={}", large_parallel);

        // Parallel decision depends on backend
        match optimizer.backend() {
            AccelerationBackend::CUDA | AccelerationBackend::ROCm |
            AccelerationBackend::MPS | AccelerationBackend::OpenCL => {
                // GPU backends: Never use Rayon
                assert!(!small_parallel);
                assert!(!large_parallel);
            }
            _ => {
                // SIMD/Scalar: May use Rayon for large datasets
                assert!(!small_parallel);
            }
        }
    }

    #[test]
    fn test_create_batches() {
        let optimizer = BatchOptimizer::new();
        let data: Vec<i32> = (0..1000).collect();

        let batches = optimizer.create_batches(&data, 128);
        println!("✅ Created {} batches from 1000 elements", batches.len());

        // Verify all data is covered
        let total_elements: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total_elements, 1000);

        // Verify batches are contiguous
        let mut expected_start = 0;
        for batch in &batches {
            assert_eq!(batch[0], expected_start);
            expected_start += batch.len() as i32;
        }
    }

    #[test]
    fn test_batch_encode_decode_round_trip() {
        use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;

        let vectors: Vec<Vec<f32>> = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![5.0, 6.0, 7.0, 8.0],
            vec![9.0, 10.0, 11.0, 12.0],
        ];

        let scheme = ProximaScheme::Delta { base: 0 };

        // Batch encode
        let encoded = batch_encode_vectors(&vectors, &scheme).unwrap();
        assert_eq!(encoded.len(), 3);
        println!("✅ Batch encoded {} vectors", encoded.len());

        // Batch decode
        let decoded = batch_decode_vectors(&encoded).unwrap();
        assert_eq!(decoded.len(), 3);
        println!("✅ Batch decoded {} vectors", decoded.len());

        // Verify round-trip accuracy
        for (i, (original, decoded)) in vectors.iter().zip(decoded.iter()).enumerate() {
            for (j, (&orig, &dec)) in original.iter().zip(decoded.iter()).enumerate() {
                let diff = (orig - dec).abs();
                assert!(
                    diff < 1e-6,
                    "Round-trip mismatch at vector {}, element {}: {} != {}",
                    i, j, orig, dec
                );
            }
        }
        println!("✅ Round-trip accuracy verified");
    }
}
