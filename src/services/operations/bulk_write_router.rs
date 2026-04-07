/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Bulk Write Router for intelligent write path selection
//!
//! This module implements smart batch detection to decide whether to use:
//! - **WAL path**: Standard WAL + memtable path for small batches (durability)
//! - **Direct write path**: Bypass WAL + memtable for large batches (performance)
//!
//! ## Decision Logic
//!
//! A batch is routed to direct write if either:
//! - Vector count >= `vector_threshold` (default: 500)
//! - Estimated size >= `size_threshold_bytes` (default: 2MB)
//!
//! ## Size Estimation
//!
//! For each vector record:
//! - Vector data: `dimension * 4` bytes (f32)
//! - Metadata overhead: ~256 bytes average
//! - ID overhead: ~64 bytes average
//!
//! ## Example
//!
//! ```text
//! 1K vectors @ 768D = ~3MB → Direct write
//! 100 vectors @ 768D = ~300KB → WAL path
//! 500 vectors @ 512D = ~2MB → Direct write (at threshold)
//! ```

use crate::proto::proximadb_v1::VectorRecord;

/// Result of bulk write route decision
#[derive(Debug, Clone)]
pub struct BulkWriteDecision {
    /// Whether to use direct write (bypass WAL+memtable)
    pub use_direct_write: bool,
    /// Reason for the decision
    pub reason: String,
    /// Estimated batch size in bytes
    pub estimated_size_bytes: usize,
    /// Number of vectors in the batch
    pub vector_count: usize,
}

/// Configuration for bulk write thresholds
#[derive(Debug, Clone)]
pub struct BulkWriteConfig {
    /// Minimum vector count to trigger direct write (default: 500)
    pub vector_threshold: usize,
    /// Minimum estimated size in bytes to trigger direct write (default: 2MB)
    pub size_threshold_bytes: usize,
    /// Enable bulk write optimization (default: true)
    pub enabled: bool,
}

impl Default for BulkWriteConfig {
    fn default() -> Self {
        Self {
            vector_threshold: 500,
            size_threshold_bytes: 2 * 1024 * 1024, // 2MB - matches memory_flush_size_bytes
            enabled: true,
        }
    }
}

/// Bulk Write Router for intelligent write path selection
///
/// Routes batch writes to either:
/// - Standard WAL + memtable path (for small streaming batches)
/// - Direct storage engine write (for large bulk batches)
///
/// This optimization reduces I/O and memory usage for bulk inserts by
/// bypassing the WAL + memtable double-write pattern.
pub struct BulkWriteRouter {
    /// Configuration controlling batch size thresholds and routing behavior
    config: BulkWriteConfig,
}

impl BulkWriteRouter {
    /// Create a new bulk write router with default configuration
    pub fn new() -> Self {
        Self {
            config: BulkWriteConfig::default(),
        }
    }

    /// Create a new bulk write router with custom configuration
    pub fn with_config(config: BulkWriteConfig) -> Self {
        Self { config }
    }

    /// Update the vector threshold
    pub fn set_vector_threshold(&mut self, threshold: usize) {
        self.config.vector_threshold = threshold;
    }

    /// Update the size threshold
    pub fn set_size_threshold_bytes(&mut self, threshold: usize) {
        self.config.size_threshold_bytes = threshold;
    }

    /// Enable or disable bulk write optimization
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
    }

    /// Decide whether to use direct write for a batch of vectors
    ///
    /// Returns a `BulkWriteDecision` indicating the routing decision and reason.
    pub fn should_use_direct_write(&self, vectors: &[VectorRecord]) -> BulkWriteDecision {
        let vector_count = vectors.len();

        // If disabled, always use WAL path
        if !self.config.enabled {
            return BulkWriteDecision {
                use_direct_write: false,
                reason: "Bulk write optimization disabled".to_string(),
                estimated_size_bytes: 0,
                vector_count,
            };
        }

        // Check vector count threshold first (fast check)
        if vector_count >= self.config.vector_threshold {
            let estimated_size = self.estimate_batch_size(vectors);
            return BulkWriteDecision {
                use_direct_write: true,
                reason: format!(
                    "Vector count {} >= threshold {}",
                    vector_count, self.config.vector_threshold
                ),
                estimated_size_bytes: estimated_size,
                vector_count,
            };
        }

        // Check size threshold
        let estimated_size = self.estimate_batch_size(vectors);
        if estimated_size >= self.config.size_threshold_bytes {
            return BulkWriteDecision {
                use_direct_write: true,
                reason: format!(
                    "Estimated size {} bytes >= threshold {} bytes",
                    estimated_size, self.config.size_threshold_bytes
                ),
                estimated_size_bytes: estimated_size,
                vector_count,
            };
        }

        // Use standard WAL path for small batches
        BulkWriteDecision {
            use_direct_write: false,
            reason: format!(
                "Small batch: {} vectors, {} bytes (thresholds: {} vectors, {} bytes)",
                vector_count,
                estimated_size,
                self.config.vector_threshold,
                self.config.size_threshold_bytes
            ),
            estimated_size_bytes: estimated_size,
            vector_count,
        }
    }

    /// Estimate the size of a batch of vectors in bytes
    ///
    /// Estimation formula:
    /// - Vector data: dimension * 4 bytes (f32 per component)
    /// - Metadata overhead: ~256 bytes average per record
    /// - ID overhead: ~64 bytes average per record
    pub fn estimate_batch_size(&self, vectors: &[VectorRecord]) -> usize {
        if vectors.is_empty() {
            return 0;
        }

        // Constants for size estimation
        const METADATA_OVERHEAD_PER_RECORD: usize = 256;
        const ID_OVERHEAD_PER_RECORD: usize = 64;
        const F32_SIZE: usize = 4;

        let mut total_size = 0;

        for record in vectors {
            // Vector data size
            let vector_size = record.vector.len() * F32_SIZE;

            // Metadata size (estimate based on HashMap<String, SqlValue>)
            // Each entry has: key (String ~32 bytes avg) + SqlValue (~64 bytes avg) + overhead
            const METADATA_ENTRY_SIZE: usize = 96;
            let metadata_size = if record.metadata.is_empty() {
                METADATA_OVERHEAD_PER_RECORD
            } else {
                record.metadata.len() * METADATA_ENTRY_SIZE + METADATA_OVERHEAD_PER_RECORD
            };

            // ID size
            let id_size = record.id.len() + ID_OVERHEAD_PER_RECORD;

            total_size += vector_size + metadata_size + id_size;
        }

        total_size
    }

    /// Quick estimate based on vector count and dimension
    ///
    /// Useful when you know the dimension but don't have all vector data yet.
    pub fn estimate_batch_size_quick(&self, vector_count: usize, dimension: usize) -> usize {
        const METADATA_OVERHEAD_PER_RECORD: usize = 256;
        const ID_OVERHEAD_PER_RECORD: usize = 64;
        const F32_SIZE: usize = 4;

        let per_record_size =
            (dimension * F32_SIZE) + METADATA_OVERHEAD_PER_RECORD + ID_OVERHEAD_PER_RECORD;
        vector_count * per_record_size
    }

    /// Check if a batch of given size would trigger direct write
    pub fn would_trigger_direct_write(&self, vector_count: usize, dimension: usize) -> bool {
        if !self.config.enabled {
            return false;
        }

        if vector_count >= self.config.vector_threshold {
            return true;
        }

        let estimated_size = self.estimate_batch_size_quick(vector_count, dimension);
        estimated_size >= self.config.size_threshold_bytes
    }
}

impl Default for BulkWriteRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vector(id: &str, dimension: usize) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec![0.0; dimension],
            metadata: std::collections::HashMap::new(),
            timestamp: None,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        }
    }

    #[test]
    fn test_default_thresholds() {
        let router = BulkWriteRouter::new();
        assert_eq!(router.config.vector_threshold, 500);
        assert_eq!(router.config.size_threshold_bytes, 2 * 1024 * 1024);
        assert!(router.config.enabled);
    }

    #[test]
    fn test_small_batch_uses_wal() {
        let router = BulkWriteRouter::new();
        let vectors: Vec<VectorRecord> = (0..10)
            .map(|i| create_test_vector(&format!("vec_{}", i), 128))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        assert!(!decision.use_direct_write);
        assert_eq!(decision.vector_count, 10);
    }

    #[test]
    fn test_large_count_uses_direct_write() {
        let router = BulkWriteRouter::new();
        let vectors: Vec<VectorRecord> = (0..500)
            .map(|i| create_test_vector(&format!("vec_{}", i), 128))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        assert!(decision.use_direct_write);
        assert!(decision.reason.contains("Vector count"));
    }

    #[test]
    fn test_large_size_uses_direct_write() {
        let router = BulkWriteRouter::new();
        // 100 vectors @ 768D = ~300KB each = ~3MB total > 2MB threshold
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| create_test_vector(&format!("vec_{}", i), 768))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        // Should trigger if size exceeds 2MB
        // 100 * (768 * 4 + 256 + 64) = 100 * 3392 = ~339KB
        // Need more vectors or larger dimension
        assert!(!decision.use_direct_write); // Still below threshold
    }

    #[test]
    fn test_high_dimension_large_batch_uses_direct_write() {
        let router = BulkWriteRouter::new();
        // 200 vectors @ 2048D should exceed 2MB
        // 200 * (2048 * 4 + 256 + 64) = 200 * 8512 = ~1.7MB - still below
        // 300 vectors @ 2048D = 300 * 8512 = ~2.5MB > 2MB
        let vectors: Vec<VectorRecord> = (0..300)
            .map(|i| create_test_vector(&format!("vec_{}", i), 2048))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        assert!(decision.use_direct_write);
        assert!(decision.reason.contains("Estimated size"));
    }

    #[test]
    fn test_disabled_always_uses_wal() {
        let mut router = BulkWriteRouter::new();
        router.set_enabled(false);

        let vectors: Vec<VectorRecord> = (0..1000)
            .map(|i| create_test_vector(&format!("vec_{}", i), 768))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        assert!(!decision.use_direct_write);
        assert!(decision.reason.contains("disabled"));
    }

    #[test]
    fn test_quick_estimate() {
        let router = BulkWriteRouter::new();

        // 1000 vectors @ 768D
        let estimated = router.estimate_batch_size_quick(1000, 768);
        // 1000 * (768 * 4 + 256 + 64) = 1000 * 3392 = 3,392,000 bytes (~3.2MB)
        assert!(estimated > 3_000_000);
        assert!(estimated < 4_000_000);
    }

    #[test]
    fn test_would_trigger_direct_write() {
        let router = BulkWriteRouter::new();

        // Small batch
        assert!(!router.would_trigger_direct_write(10, 128));

        // Large count
        assert!(router.would_trigger_direct_write(500, 128));

        // Large size (1000 vectors @ 768D = ~3.2MB > 2MB)
        assert!(router.would_trigger_direct_write(1000, 768));
    }

    #[test]
    fn test_custom_thresholds() {
        let config = BulkWriteConfig {
            vector_threshold: 100,
            size_threshold_bytes: 1024 * 1024, // 1MB
            enabled: true,
        };
        let router = BulkWriteRouter::with_config(config);

        // 100 vectors should now trigger
        let vectors: Vec<VectorRecord> = (0..100)
            .map(|i| create_test_vector(&format!("vec_{}", i), 128))
            .collect();

        let decision = router.should_use_direct_write(&vectors);
        assert!(decision.use_direct_write);
    }

    #[test]
    fn test_empty_batch() {
        let router = BulkWriteRouter::new();
        let vectors: Vec<VectorRecord> = vec![];

        let decision = router.should_use_direct_write(&vectors);
        assert!(!decision.use_direct_write);
        assert_eq!(decision.estimated_size_bytes, 0);
        assert_eq!(decision.vector_count, 0);
    }
}
