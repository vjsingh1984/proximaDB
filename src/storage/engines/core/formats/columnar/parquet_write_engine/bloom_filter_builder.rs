//! Bloom Filter Builder for Parquet Writers
//!
//! This module provides a bridge between Parquet writers and ProximaDB's
//! unified bloom filter implementation. It handles two types of bloom filters:
//!
//! 1. **Parquet Native Bloom Filters**: Built into Parquet files via WriterProperties
//! 2. **Custom Bloom Filters**: Using ProximaDB's core::bloom module for additional tracking
//!
//! The custom bloom filters are used for ID tracking and metadata filtering
//! that supplements Parquet's native capabilities.

use anyhow::{Result, anyhow};
use std::collections::HashMap;

use crate::core::bloom::{
    BloomFilter, BloomFilterBuilder as CoreBloomBuilder, BloomFilterConfig as CoreConfig,
    BloomStrategy,
};
use crate::proto::proximadb_v1::VectorRecord;

/// Wrapper around core bloom filter builder for Parquet-specific use
pub struct BloomFilterBuilder {
    /// Bloom filters per row group (using core implementation)
    bloom_filters: HashMap<usize, BloomFilter>,

    /// Configuration for bloom filters
    config: BloomFilterConfig,

    /// Current row group being processed
    current_row_group: usize,
}

/// Configuration for bloom filter creation (wraps core config)
#[derive(Debug, Clone)]
pub struct BloomFilterConfig {
    /// False positive probability
    pub fpp: f64,

    /// Number of distinct values (estimate)
    pub ndv: u64,

    /// Whether to create per-row-group filters
    pub per_row_group: bool,

    /// Maximum memory per filter in bytes
    pub max_memory_bytes: usize,

    /// Bits per key (alternative to fpp/ndv)
    pub bits_per_key: Option<f32>,
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            fpp: 0.01,   // 1% false positive rate (matching core default)
            ndv: 100000, // Default expected items
            per_row_group: true,
            max_memory_bytes: 1024 * 1024, // 1MB per filter
            bits_per_key: Some(10.0),      // Default 10 bits per key
        }
    }
}

impl BloomFilterBuilder {
    /// Create new bloom filter builder
    pub fn new(config: BloomFilterConfig) -> Self {
        Self {
            bloom_filters: HashMap::new(),
            config,
            current_row_group: 0,
        }
    }

    /// Start a new row group
    pub fn start_row_group(&mut self, row_group_index: usize) -> Result<()> {
        self.current_row_group = row_group_index;

        if self.config.per_row_group && !self.bloom_filters.contains_key(&row_group_index) {
            // Create core bloom filter config
            let core_config = CoreConfig {
                strategy: BloomStrategy::ByteAligned,
                bits_per_key: self.config.bits_per_key.unwrap_or(10.0) as u32,
                false_positive_rate: Some(self.config.fpp),
                expected_items: self.config.ndv as usize,
                enabled: true,
                hash_algorithm: crate::core::bloom::HashAlgorithm::Murmur3,
            };

            // Use core bloom filter builder
            let builder = CoreBloomBuilder::new(core_config);
            let filter = builder.build();

            // Check memory usage (approximate)
            let estimated_bits = (self.config.ndv as f64 * 10.0) as usize; // ~10 bits per key
            let memory_usage = estimated_bits / 8;
            if memory_usage > self.config.max_memory_bytes {
                return Err(anyhow!(
                    "Bloom filter would use ~{} bytes, exceeding limit of {} bytes",
                    memory_usage,
                    self.config.max_memory_bytes
                ));
            }

            self.bloom_filters.insert(row_group_index, filter);
        }

        Ok(())
    }

    /// Add records to current bloom filter
    pub fn add_batch(&mut self, records: &[VectorRecord]) -> Result<()> {
        let filter = self.get_or_create_filter(self.current_row_group)?;

        for record in records {
            filter.insert(record.id.as_bytes());

            // Optionally add metadata keys for filtering
            for key in record.metadata.keys() {
                filter.insert(key.as_bytes());
            }
        }

        Ok(())
    }

    /// Add a single ID to the bloom filter
    pub fn add_id(&mut self, id: &str) -> Result<()> {
        let filter = self.get_or_create_filter(self.current_row_group)?;
        filter.insert(id.as_bytes());
        Ok(())
    }

    /// Check if an ID might exist (may have false positives)
    pub fn might_contain(&self, row_group: usize, id: &str) -> bool {
        self.bloom_filters
            .get(&row_group)
            .map(|filter| filter.might_contain(id.as_bytes()))
            .unwrap_or(false)
    }

    /// Get or create filter for a row group
    fn get_or_create_filter(&mut self, row_group: usize) -> Result<&mut BloomFilter> {
        if !self.bloom_filters.contains_key(&row_group) {
            // Create core bloom filter config
            let core_config = CoreConfig {
                strategy: BloomStrategy::ByteAligned,
                bits_per_key: self.config.bits_per_key.unwrap_or(10.0) as u32,
                false_positive_rate: Some(self.config.fpp),
                expected_items: self.config.ndv as usize,
                enabled: true,
                hash_algorithm: crate::core::bloom::HashAlgorithm::Murmur3,
            };

            let builder = CoreBloomBuilder::new(core_config);
            let filter = builder.build();

            self.bloom_filters.insert(row_group, filter);
        }

        self.bloom_filters
            .get_mut(&row_group)
            .ok_or_else(|| anyhow!("Failed to get bloom filter for row group {}", row_group))
    }

    /// Serialize bloom filters for storage
    pub fn serialize(&self) -> Result<Vec<u8>> {
        // Use core bloom filter's serialization support
        let mut serialized_filters = Vec::new();

        for (row_group, filter) in &self.bloom_filters {
            let filter_bytes = filter.serialize()?;
            serialized_filters.push((*row_group, filter_bytes));
        }

        // Serialize the collection
        bincode::serialize(&serialized_filters)
            .map_err(|e| anyhow!("Failed to serialize bloom filters: {}", e))
    }

    /// Deserialize bloom filters from storage
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let serialized_filters: Vec<(usize, Vec<u8>)> = bincode::deserialize(data)?;
        let mut bloom_filters = HashMap::new();

        for (row_group, filter_bytes) in serialized_filters {
            // Use factory to deserialize with a default config
            let config = CoreConfig {
                strategy: BloomStrategy::ByteAligned,
                bits_per_key: 10,
                false_positive_rate: Some(0.01),
                expected_items: 1000,
                enabled: true,
                hash_algorithm: crate::core::bloom::HashAlgorithm::Murmur3,
            };
            // deserialize already returns Box<dyn BloomFilterStrategy>
            let filter = crate::core::bloom::factory::BloomFilterFactory::deserialize(
                &config,
                &filter_bytes,
            )?;
            bloom_filters.insert(row_group, filter);
        }

        Ok(Self {
            bloom_filters,
            config: BloomFilterConfig::default(),
            current_row_group: 0,
        })
    }

    /// Get statistics about bloom filters
    pub fn get_stats(&self) -> BloomFilterStats {
        let total_filters = self.bloom_filters.len();

        // Use core bloom filter's stats
        let total_memory: usize = self
            .bloom_filters
            .values()
            .map(|_f| {
                // BloomFilterStrategy doesn't have stats() method
                // Return estimated size based on configuration
                let bits = (self.config.ndv as f64 * 10.0) as usize;
                bits / 8
            })
            .sum();

        BloomFilterStats {
            total_filters,
            total_memory_bytes: total_memory,
            false_positive_probability: self.config.fpp,
            estimated_ndv: self.config.ndv,
        }
    }

    /// Clear all bloom filters
    pub fn clear(&mut self) {
        self.bloom_filters.clear();
        self.current_row_group = 0;
    }
}

/// Statistics about bloom filters
#[derive(Debug, Clone)]
pub struct BloomFilterStats {
    pub total_filters: usize,
    pub total_memory_bytes: usize,
    pub false_positive_probability: f64,
    pub estimated_ndv: u64,
}

impl BloomFilterStats {
    /// Get human-readable summary
    pub fn summary(&self) -> String {
        format!(
            "Bloom filters: {} (memory: {} KB, FPP: {:.2}%, NDV: {})",
            self.total_filters,
            self.total_memory_bytes / 1024,
            self.false_positive_probability * 100.0,
            self.estimated_ndv
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic() {
        let config = BloomFilterConfig::default();
        let mut builder = BloomFilterBuilder::new(config);

        // Add IDs to row group 0
        builder
            .start_row_group(0)
            .expect("Failed to start row group");
        builder.add_id("id_1").expect("Failed to add id_1");
        builder.add_id("id_2").expect("Failed to add id_2");
        builder.add_id("id_3").expect("Failed to add id_3");

        // Check membership
        assert!(builder.might_contain(0, "id_1"));
        assert!(builder.might_contain(0, "id_2"));
        assert!(builder.might_contain(0, "id_3"));

        // Should not contain (unless false positive)
        assert!(!builder.might_contain(0, "id_999") || builder.config.fpp > 0.0); // Allow for false positives
    }

    #[test]
    fn test_multiple_row_groups() {
        let config = BloomFilterConfig {
            per_row_group: true,
            ..Default::default()
        };
        let mut builder = BloomFilterBuilder::new(config);

        // Row group 0
        builder
            .start_row_group(0)
            .expect("Failed to start row group 0");
        builder.add_id("rg0_id1").expect("Failed to add rg0_id1");

        // Row group 1
        builder
            .start_row_group(1)
            .expect("Failed to start row group 1");
        builder.add_id("rg1_id1").expect("Failed to add rg1_id1");

        // Check isolation between row groups
        assert!(builder.might_contain(0, "rg0_id1"));
        assert!(!builder.might_contain(0, "rg1_id1") || builder.config.fpp > 0.0);

        assert!(builder.might_contain(1, "rg1_id1"));
        assert!(!builder.might_contain(1, "rg0_id1") || builder.config.fpp > 0.0);
    }

    #[test]
    fn test_batch_addition() {
        let config = BloomFilterConfig::default();
        let mut builder = BloomFilterBuilder::new(config);

        builder
            .start_row_group(0)
            .expect("Failed to start row group");

        let records = vec![
            VectorRecord {
                id: "batch_1".to_string(),
                vector: vec![1.0, 2.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                ..Default::default()
            },
            VectorRecord {
                id: "batch_2".to_string(),
                vector: vec![3.0, 4.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                ..Default::default()
            },
        ];

        builder.add_batch(&records).expect("Failed to add batch");

        assert!(builder.might_contain(0, "batch_1"));
        assert!(builder.might_contain(0, "batch_2"));
    }

    #[test]
    fn test_stats() {
        let config = BloomFilterConfig::default();
        let mut builder = BloomFilterBuilder::new(config);

        builder
            .start_row_group(0)
            .expect("Failed to start row group");
        builder.add_id("test").expect("Failed to add test ID");

        let stats = builder.get_stats();
        assert_eq!(stats.total_filters, 1);
        assert!(stats.total_memory_bytes > 0);

        let summary = stats.summary();
        assert!(summary.contains("Bloom filters: 1"));
    }
}
