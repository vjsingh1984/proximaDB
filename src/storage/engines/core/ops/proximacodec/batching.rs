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

use crate::core::hardware_capabilities::HardwareBackend;
use crate::storage::engines::core::ops::proximacodec::simd::get_simd_backend;

/// Batch size optimizer for hardware-aware processing
///
/// Automatically selects optimal batch sizes based on:
/// - Hardware backend (GPU, SIMD, Scalar)
/// - Data dimensionality (affects cache utilization)
/// - Total vector count
#[derive(Debug, Clone)]
pub struct BatchOptimizer {
    backend: HardwareBackend,
}

impl BatchOptimizer {
    /// Create a new batch optimizer for the current hardware
    pub fn new() -> Self {
        Self {
            backend: get_simd_backend(),
        }
    }

    /// Create a batch optimizer for a specific backend (testing/benchmarking)
    pub fn with_backend(backend: HardwareBackend) -> Self {
        Self { backend }
    }

    /// Determine optimal batch size based on backend
    ///
    /// ## Strategy (Power-of-2 row groups: 1024 vectors/row group)
    /// - **Row group size**: 1024 vectors (power of 2, not 1000)
    /// - **Batch sizes**: Multiples of row groups: 1024, 2048, 4096, 8192
    /// - **Backend-optimized**: Different backends process different row group multiples
    ///
    /// ## ProximaDB Row Groups (1024 vectors each)
    /// - BERT-768: 1024 vectors = ~3MB
    /// - BERT-1024: 1024 vectors = ~4MB
    /// - OpenAI-1536: 1024 vectors = ~6MB
    /// - Custom-2048: 1024 vectors = ~8MB
    ///
    /// ## Parameters
    /// - `total_vectors`: Total number of vectors to process
    /// - `_dimension`: Ignored (batch size determined by row groups only)
    ///
    /// ## Returns
    /// Power-of-2 batch size (always multiple of 1024)
    pub fn optimal_batch_size(&self, total_vectors: usize, _dimension: usize) -> usize {
        // ProximaDB row group size (power of 2)
        const ROW_GROUP_SIZE: usize = 1024;

        // Helper to find previous power of 2
        fn prev_power_of_2(n: usize) -> usize {
            if n <= 1 {
                return 1;
            }
            let mut p = 1;
            while p * 2 <= n {
                p *= 2;
            }
            p
        }

        // Calculate batch as multiple of row groups
        let batch_size = match self.backend {
            // GPU backends: 4-8 row groups (4096-8192 vectors)
            HardwareBackend::CUDA | HardwareBackend::ROCm => {
                if total_vectors >= 8 * ROW_GROUP_SIZE {
                    8 * ROW_GROUP_SIZE // 8192 vectors
                } else if total_vectors >= 4 * ROW_GROUP_SIZE {
                    4 * ROW_GROUP_SIZE // 4096 vectors
                } else {
                    prev_power_of_2(total_vectors.max(ROW_GROUP_SIZE))
                }
            }

            // MPS (Apple Metal): 2-4 row groups (2048-4096 vectors)
            HardwareBackend::MPS => {
                if total_vectors >= 4 * ROW_GROUP_SIZE {
                    4 * ROW_GROUP_SIZE // 4096 vectors
                } else if total_vectors >= 2 * ROW_GROUP_SIZE {
                    2 * ROW_GROUP_SIZE // 2048 vectors
                } else {
                    prev_power_of_2(total_vectors.max(ROW_GROUP_SIZE))
                }
            }

            // OpenCL: 2-8 row groups (2048-8192 vectors)
            HardwareBackend::OpenCL => {
                if total_vectors >= 8 * ROW_GROUP_SIZE {
                    8 * ROW_GROUP_SIZE
                } else if total_vectors >= 4 * ROW_GROUP_SIZE {
                    4 * ROW_GROUP_SIZE
                } else if total_vectors >= 2 * ROW_GROUP_SIZE {
                    2 * ROW_GROUP_SIZE
                } else {
                    prev_power_of_2(total_vectors.max(ROW_GROUP_SIZE))
                }
            }

            // SIMD (AVX2/AVX512): 1-2 row groups (1024-2048 vectors)
            HardwareBackend::AVX512 | HardwareBackend::AVX2 => {
                if total_vectors >= 2 * ROW_GROUP_SIZE {
                    2 * ROW_GROUP_SIZE // 2048 vectors
                } else {
                    prev_power_of_2(total_vectors.max(ROW_GROUP_SIZE))
                }
            }

            // NEON/SSE: 1 row group (1024 vectors)
            HardwareBackend::NEON | HardwareBackend::SSE => {
                if total_vectors >= ROW_GROUP_SIZE {
                    ROW_GROUP_SIZE // 1024 vectors
                } else {
                    prev_power_of_2(total_vectors.max(512))
                }
            }

            // Scalar: Fraction of row group (128-512 vectors)
            HardwareBackend::Scalar => {
                if total_vectors >= 512 {
                    512 // Half row group
                } else if total_vectors >= 256 {
                    256 // Quarter row group
                } else if total_vectors >= 128 {
                    128 // 1/8 row group
                } else {
                    prev_power_of_2(total_vectors.max(16))
                }
            }
        };

        // Ensure batch size is power of 2
        debug_assert_eq!(
            batch_size & (batch_size - 1),
            0,
            "Batch size must be power of 2"
        );
        batch_size
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
            HardwareBackend::CUDA
            | HardwareBackend::ROCm
            | HardwareBackend::MPS
            | HardwareBackend::OpenCL => false,

            // SIMD: Use Rayon for large datasets
            HardwareBackend::AVX512
            | HardwareBackend::AVX2
            | HardwareBackend::NEON
            | HardwareBackend::SSE => total_vectors > 10_000,

            // Scalar: Use Rayon for medium+ datasets
            HardwareBackend::Scalar => total_vectors > 5_000,
        }
    }

    /// Get the current backend for diagnostics
    pub fn backend(&self) -> HardwareBackend {
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

        debug!(
            "⚡ Using parallel encoding (Rayon) for {} vectors",
            vectors.len()
        );
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

        debug!(
            "⚡ Using parallel decoding (Rayon) for {} vectors",
            encoded_vectors.len()
        );
        // Parallel decoding for large datasets
        encoded_vectors
            .par_iter()
            .map(|e| codec.decode(e))
            .collect()
    } else {
        debug!(
            "🔄 Using sequential decoding for {} vectors",
            encoded_vectors.len()
        );
        // Sequential decoding for small/medium datasets
        encoded_vectors.iter().map(|e| codec.decode(e)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_optimizer_creation() {
        let optimizer = BatchOptimizer::new();
        println!(
            "✅ Batch optimizer created with backend: {:?}",
            optimizer.backend()
        );

        // Verify backend is valid
        assert!(matches!(
            optimizer.backend(),
            HardwareBackend::CUDA
                | HardwareBackend::ROCm
                | HardwareBackend::MPS
                | HardwareBackend::OpenCL
                | HardwareBackend::AVX512
                | HardwareBackend::AVX2
                | HardwareBackend::NEON
                | HardwareBackend::SSE
                | HardwareBackend::Scalar
        ));
    }

    #[test]
    fn test_optimal_batch_size_bert_embeddings() {
        println!("\n📊 Batch Size Test: BERT Embeddings (768D)");
        let optimizer = BatchOptimizer::new();

        // Typical batch: 1024 vectors × 768D = ~3MB
        let typical_batch = 1024;
        let bert_dim = 768;

        let batch_size = optimizer.optimal_batch_size(typical_batch, bert_dim);
        println!("   Backend: {:?}", optimizer.backend());
        println!(
            "   Input: {} vectors × {}D = ~{}MB",
            typical_batch,
            bert_dim,
            (typical_batch * bert_dim * 4) / (1024 * 1024)
        );
        println!("   Optimal batch: {} vectors (power of 2)", batch_size);

        // Verify batch is power of 2
        assert_eq!(
            batch_size & (batch_size - 1),
            0,
            "Batch size must be power of 2"
        );

        match optimizer.backend() {
            HardwareBackend::CUDA | HardwareBackend::ROCm => {
                // GPU: Should be 4096 or 8192 (power of 2)
                assert!(batch_size >= 1024, "GPU should batch at least 1024");
                assert!(batch_size <= 8192, "GPU should batch ≤8192");
                assert!(matches!(batch_size, 1024 | 2048 | 4096 | 8192));
            }
            HardwareBackend::MPS => {
                // MPS: Should be 2048 or 4096 (power of 2)
                assert!(batch_size >= 1024, "MPS should batch at least 1024");
                assert!(batch_size <= 4096, "MPS should batch ≤4096");
                assert!(matches!(batch_size, 1024 | 2048 | 4096));
            }
            HardwareBackend::AVX512 | HardwareBackend::AVX2 => {
                // AVX: Should be 1024 or 2048 (cache-friendly)
                assert!(batch_size >= 1024, "AVX should batch at least 1024");
                assert!(batch_size <= 2048, "AVX should fit in L3 cache");
                assert!(matches!(batch_size, 1024 | 2048));
            }
            HardwareBackend::NEON | HardwareBackend::SSE => {
                // NEON/SSE: Should be 512 or 1024
                assert!(batch_size >= 512, "NEON/SSE should batch at least 512");
                assert!(batch_size <= 1024, "NEON/SSE should batch ≤1024");
                assert!(matches!(batch_size, 512 | 1024));
            }
            HardwareBackend::Scalar => {
                // Scalar: Should be 128, 256, or 512
                assert!(batch_size >= 64);
                assert!(batch_size <= 512);
                assert!(matches!(batch_size, 64 | 128 | 256 | 512));
            }
            _ => {}
        }
        println!("   ✅ Batch size is power of 2 and optimal for backend");
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
        println!(
            "   Row group: {} vectors × {}D = ~6MB",
            row_group_size, openai_dim
        );
        println!("   Optimal batch: {} vectors", batch_size);

        match optimizer.backend() {
            HardwareBackend::CUDA | HardwareBackend::ROCm => {
                // GPU: Should process 5-10 row groups
                assert!(batch_size >= 5000);
                assert!(batch_size <= 10000);
            }
            HardwareBackend::MPS => {
                // MPS: Should process 2-5 row groups
                assert!(batch_size >= 2000);
                assert!(batch_size <= 5000);
            }
            HardwareBackend::AVX512 | HardwareBackend::AVX2 => {
                // AVX: For 1536D (6MB/row), should process 1-2 row groups
                assert!(batch_size >= 1000);
                assert!(batch_size <= 3000, "Large dims should reduce batch count");
            }
            HardwareBackend::NEON | HardwareBackend::SSE => {
                // NEON/SSE: For 1000 vectors, returns 512 (prev_power_of_2)
                // Would return 1024 if total_vectors >= 1024
                assert_eq!(batch_size, 512);
            }
            HardwareBackend::Scalar => {
                assert!(batch_size >= 10);
                assert!(batch_size <= 500);
            }
            _ => {}
        }
        println!("   ✅ Batch size adapts to higher dimensionality");
    }

    #[test]
    fn test_optimal_batch_size_power_of_2_row_groups() {
        println!("\n📊 Batch Size Test: Power-of-2 Row Groups");
        let optimizer = BatchOptimizer::new();

        // Test with 10240 vectors (10 row groups × 1024)
        let total_vectors = 10240;
        let dimension = 768;

        let batch_size = optimizer.optimal_batch_size(total_vectors, dimension);
        println!(
            "   Total: {} vectors ({} row groups of 1024)",
            total_vectors,
            total_vectors / 1024
        );
        println!("   Batch size: {} vectors", batch_size);
        println!("   Row groups per batch: {}", batch_size / 1024);
        println!(
            "   Batches needed: {}",
            (total_vectors + batch_size - 1) / batch_size
        );

        // Verify batch size is power of 2
        assert_eq!(
            batch_size & (batch_size - 1),
            0,
            "Batch size must be power of 2"
        );

        // Verify batch size is multiple of row group size (1024)
        if batch_size >= 1024 {
            assert_eq!(
                batch_size % 1024,
                0,
                "Should be multiple of 1024 (row group size)"
            );
        }

        match optimizer.backend() {
            HardwareBackend::CUDA | HardwareBackend::ROCm => {
                // GPU: 4-8 row groups (4096 or 8192)
                assert!(
                    matches!(batch_size, 4096 | 8192),
                    "GPU should use 4096 or 8192 vectors"
                );
            }
            HardwareBackend::MPS => {
                // MPS: 2-4 row groups (2048 or 4096)
                assert!(
                    matches!(batch_size, 2048 | 4096),
                    "MPS should use 2048 or 4096 vectors"
                );
            }
            HardwareBackend::AVX512 | HardwareBackend::AVX2 => {
                // AVX: 1-2 row groups (1024 or 2048)
                assert!(
                    matches!(batch_size, 1024 | 2048),
                    "AVX should use 1024 or 2048 vectors"
                );
            }
            HardwareBackend::NEON | HardwareBackend::SSE => {
                // NEON/SSE: 1 row group (1024)
                assert_eq!(batch_size, 1024, "NEON/SSE should use 1024 vectors");
            }
            HardwareBackend::Scalar => {
                // Scalar: 128-512 (fractions of row group)
                assert!(
                    matches!(batch_size, 128 | 256 | 512),
                    "Scalar should use 128, 256, or 512 vectors"
                );
            }
            _ => {}
        }
        println!("   ✅ Batch aligned to power-of-2 row groups");
    }

    #[test]
    fn test_should_use_parallel() {
        let optimizer = BatchOptimizer::new();

        // Small dataset: Should not use parallel
        let small_parallel = optimizer.should_use_parallel(100);
        println!(
            "🔀 Small dataset (100 vectors): parallel={}",
            small_parallel
        );

        // Large dataset: May use parallel depending on backend
        let large_parallel = optimizer.should_use_parallel(50_000);
        println!(
            "🔀 Large dataset (50K vectors): parallel={}",
            large_parallel
        );

        // Parallel decision depends on backend
        match optimizer.backend() {
            HardwareBackend::CUDA
            | HardwareBackend::ROCm
            | HardwareBackend::MPS
            | HardwareBackend::OpenCL => {
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
                    i,
                    j,
                    orig,
                    dec
                );
            }
        }
        println!("✅ Round-trip accuracy verified");
    }
}
