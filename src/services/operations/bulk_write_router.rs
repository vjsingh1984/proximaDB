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
//! - **Bulk lane**: Large-batch WAL-backed append today; future direct segment
//!   commit only after durability proof is accepted
//!
//! ## Decision Logic
//!
//! A batch is routed to the bulk lane if either:
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
//! 1K vectors @ 768D = ~3MB → Bulk lane
//! 100 vectors @ 768D = ~300KB → WAL path
//! 500 vectors @ 512D = ~2MB → Bulk lane (at threshold)
//! ```

use proximadb_records::ProximaRecord;

/// Result of bulk write route decision
#[derive(Debug, Clone)]
pub struct BulkWriteDecision {
    /// Whether to use the large-batch lane (WAL-backed bulk path).
    ///
    /// True means the batch meets the size/count threshold for bulk-lane routing.
    /// The implementation writes through WAL for durability; a future direct
    /// segment commit path is separate and requires an accepted durability proof.
    pub use_bulk_lane: bool,
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
    /// Minimum vector count to trigger the bulk lane (default: 500)
    pub vector_threshold: usize,
    /// Minimum estimated size in bytes to trigger the bulk lane (default: 2MB)
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
/// - Large-batch lane (currently WAL-backed; future direct segment commit)
///
/// This optimization centralizes the threshold decision for bulk inserts. It
/// does not by itself authorize skipping WAL.
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

    /// Decide whether to use the bulk lane for a batch of canonical records.
    ///
    /// Arrow Flight and catalog-aware writes should use this path so routing is
    /// based on the `ProximaRecord` envelope instead of downgrading through
    /// legacy vector-only records.
    pub fn route_records(&self, records: &[ProximaRecord]) -> BulkWriteDecision {
        let record_count = records.len();
        let estimated_size = if self.config.enabled {
            Some(self.estimate_record_batch_size(records))
        } else {
            None
        };
        self.route_decision(record_count, estimated_size, "Record", "records")
    }

    fn route_decision(
        &self,
        record_count: usize,
        estimated_size: Option<usize>,
        count_label: &str,
        unit_label: &str,
    ) -> BulkWriteDecision {
        // If disabled, always use WAL path
        if !self.config.enabled {
            return BulkWriteDecision {
                use_bulk_lane: false,
                reason: "Bulk write optimization disabled".to_string(),
                estimated_size_bytes: 0,
                vector_count: record_count,
            };
        }

        // Check vector count threshold first (fast check)
        if record_count >= self.config.vector_threshold {
            let estimated_size = estimated_size.unwrap_or(0);
            return BulkWriteDecision {
                use_bulk_lane: true,
                reason: format!(
                    "{} count {} >= threshold {}",
                    count_label, record_count, self.config.vector_threshold
                ),
                estimated_size_bytes: estimated_size,
                vector_count: record_count,
            };
        }

        // Check size threshold
        let estimated_size = estimated_size.unwrap_or(0);
        if estimated_size >= self.config.size_threshold_bytes {
            return BulkWriteDecision {
                use_bulk_lane: true,
                reason: format!(
                    "Estimated size {} bytes >= threshold {} bytes",
                    estimated_size, self.config.size_threshold_bytes
                ),
                estimated_size_bytes: estimated_size,
                vector_count: record_count,
            };
        }

        // Use standard WAL path for small batches
        BulkWriteDecision {
            use_bulk_lane: false,
            reason: format!(
                "Small batch: {} {}, {} bytes (thresholds: {} records, {} bytes)",
                record_count,
                unit_label,
                estimated_size,
                self.config.vector_threshold,
                self.config.size_threshold_bytes
            ),
            estimated_size_bytes: estimated_size,
            vector_count: record_count,
        }
    }

    /// Estimate the size of a batch of canonical records in bytes.
    ///
    /// Counts all embeddings and approximates rich property payloads so
    /// relational, document, graph, and observability rows can use the same
    /// large-batch routing surface as vector-only records.
    pub fn estimate_record_batch_size(&self, records: &[ProximaRecord]) -> usize {
        if records.is_empty() {
            return 0;
        }

        const PROPERTY_OVERHEAD_PER_RECORD: usize = 256;
        const ID_OVERHEAD_PER_RECORD: usize = 64;
        const F32_SIZE: usize = 4;

        records
            .iter()
            .map(|record| {
                let embedding_size: usize = record
                    .embeddings
                    .iter()
                    .map(|embedding| embedding.values.len() * F32_SIZE)
                    .sum();
                let props_size = record.props.len() * 96 + PROPERTY_OVERHEAD_PER_RECORD;
                let id_size = record.oid.len() + ID_OVERHEAD_PER_RECORD;

                embedding_size + props_size + id_size
            })
            .sum()
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

    /// Check if a batch of given size would trigger the bulk lane.
    pub fn would_use_bulk_lane(&self, vector_count: usize, dimension: usize) -> bool {
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
    use proximadb_data_model::ProximaValue;
    use proximadb_records::{EmbeddingCell, ProximaTreeNode};

    fn create_test_record(id: &str, dimension: usize) -> ProximaRecord {
        let mut record = ProximaRecord {
            oid: id.to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "dense_vector".to_string(),
                dim: dimension as u32,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.0; dimension]),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        };
        record.props.insert(
            "category".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("bulk".to_string())),
        );
        record
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
        let records: Vec<ProximaRecord> = (0..10)
            .map(|i| create_test_record(&format!("rec_{}", i), 128))
            .collect();

        let decision = router.route_records(&records);
        assert!(!decision.use_bulk_lane);
        assert_eq!(decision.vector_count, 10);
    }

    #[test]
    fn test_large_count_uses_bulk_lane() {
        let router = BulkWriteRouter::new();
        let records: Vec<ProximaRecord> = (0..500)
            .map(|i| create_test_record(&format!("rec_{}", i), 128))
            .collect();

        let decision = router.route_records(&records);
        assert!(decision.use_bulk_lane);
    }

    #[test]
    fn test_canonical_record_size_uses_all_embeddings_and_props() {
        let router = BulkWriteRouter::with_config(BulkWriteConfig {
            vector_threshold: 10_000,
            size_threshold_bytes: 1_000,
            enabled: true,
        });
        let records = vec![create_test_record("rec_1", 512)];

        let decision = router.route_records(&records);

        assert!(decision.use_bulk_lane);
        assert_eq!(decision.vector_count, 1);
        assert!(decision.estimated_size_bytes >= 512 * 4);
    }

    #[test]
    fn test_disabled_always_uses_wal() {
        let mut router = BulkWriteRouter::new();
        router.set_enabled(false);

        let records: Vec<ProximaRecord> = (0..1000)
            .map(|i| create_test_record(&format!("rec_{}", i), 768))
            .collect();

        let decision = router.route_records(&records);
        assert!(!decision.use_bulk_lane);
        assert!(decision.reason.contains("disabled"));
    }

    #[test]
    fn test_quick_estimate() {
        let router = BulkWriteRouter::new();
        let estimated = router.estimate_batch_size_quick(1000, 768);
        assert!(estimated > 3_000_000);
        assert!(estimated < 4_000_000);
    }

    #[test]
    fn test_would_use_bulk_lane() {
        let router = BulkWriteRouter::new();
        assert!(!router.would_use_bulk_lane(10, 128));
        assert!(router.would_use_bulk_lane(500, 128));
        assert!(router.would_use_bulk_lane(1000, 768));
    }

    #[test]
    fn test_custom_thresholds() {
        let config = BulkWriteConfig {
            vector_threshold: 100,
            size_threshold_bytes: 1024 * 1024,
            enabled: true,
        };
        let router = BulkWriteRouter::with_config(config);
        let records: Vec<ProximaRecord> = (0..100)
            .map(|i| create_test_record(&format!("rec_{}", i), 128))
            .collect();

        let decision = router.route_records(&records);
        assert!(decision.use_bulk_lane);
    }

    #[test]
    fn test_empty_batch() {
        let router = BulkWriteRouter::new();
        let records: Vec<ProximaRecord> = vec![];

        let decision = router.route_records(&records);
        assert!(!decision.use_bulk_lane);
        assert_eq!(decision.estimated_size_bytes, 0);
        assert_eq!(decision.vector_count, 0);
    }
}
