//! LSM Bloom Filter Module
//! 
//! This module provides bloom filter functionality for LSM storage engine
//! using the unified bloom filter design from core::bloom.

pub use crate::core::bloom::{
    BloomFilterStrategy,
    BloomFilterConfig,
    BloomStrategy,
    HashAlgorithm,
    MetadataBloomFilter,
    factory::BloomFilterFactory,
    strategies::{
        ByteAlignedBloomFilter,
        CompositeBloomFilter,
    }
};

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use anyhow::Result;

/// Combined bloom filter for SSTable (keys + metadata)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableBloomFilter {
    /// Key filter configuration
    pub key_filter_config: BloomFilterConfig,
    /// Key filter data
    pub key_filter_data: Vec<u8>,
    /// Metadata filter data  
    pub metadata_filter_data: Vec<u8>,
    /// Statistics
    pub stats: HashMap<String, u64>,
}

impl SstableBloomFilter {
    /// Create new SSTable bloom filter from actual filters
    pub fn new(key_filter: &dyn BloomFilterStrategy, metadata_filter: &CompositeBloomFilter) -> Result<Self> {
        let mut stats = HashMap::new();
        stats.insert("key_count".to_string(), key_filter.num_elements() as u64);
        stats.insert("metadata_columns".to_string(), metadata_filter.metadata_columns() as u64);
        stats.insert("total_keys".to_string(), 0);
        stats.insert("key_lookups_saved".to_string(), 0);
        stats.insert("metadata_queries_saved".to_string(), 0);
        
        // Create default config for deserialization later
        let key_filter_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: key_filter.num_elements(),
            bits_per_key: 10, // Default
            enabled: true,
            ..Default::default()
        };
        
        Ok(Self {
            key_filter_config,
            key_filter_data: key_filter.serialize()?,
            metadata_filter_data: BloomFilterStrategy::serialize(metadata_filter)?,
            stats,
        })
    }
    
    /// Check if key might exist
    pub fn might_contain_key(&self, key: &str) -> Result<bool> {
        // Create SerializedBloomFilter structure for deserialization
        let serialized = crate::core::bloom::SerializedBloomFilter {
            strategy_type: self.key_filter_config.strategy,
            version: crate::core::bloom::SerializedBloomFilter::CURRENT_VERSION,
            config: self.key_filter_config.clone(),
            data: self.key_filter_data.clone(),
            metadata: HashMap::new(),
        };
        let filter = BloomFilterFactory::from_serialized(&serialized)?;
        Ok(filter.might_contain(key.as_bytes()))
    }
    
    /// Check if metadata might match
    pub fn might_match_metadata(&self, column: &str, value: &str) -> Result<bool> {
        let filter = CompositeBloomFilter::from_bytes(&self.metadata_filter_data)?;
        Ok(filter.might_match_metadata(column, value))
    }
    
    /// Check if entry might match query conditions
    pub fn might_match_query(
        &self, 
        key: Option<&str>, 
        metadata_conditions: Option<&HashMap<String, String>>
    ) -> Result<bool> {
        // Check key if provided
        if let Some(k) = key {
            if !self.might_contain_key(k)? {
                return Ok(false);
            }
        }
        
        // Check metadata conditions if provided
        if let Some(conditions) = metadata_conditions {
            let filter = CompositeBloomFilter::from_bytes(&self.metadata_filter_data)?;
            for (column, value) in conditions {
                if !filter.might_match_metadata(column, value) {
                    return Ok(false);
                }
            }
        }
        
        Ok(true)
    }
    
    /// Get total size in bytes
    pub fn total_size_bytes(&self) -> usize {
        self.key_filter_data.len() + self.metadata_filter_data.len()
    }
    
    /// Get efficiency statistics
    pub fn efficiency_stats(&self) -> &HashMap<String, u64> {
        &self.stats
    }
}