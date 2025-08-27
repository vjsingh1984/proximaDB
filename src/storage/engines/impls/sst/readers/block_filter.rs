/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Intelligent DataBlock filtering using hierarchical bloom filters and metadata
//! 
//! Provides efficient block skipping for reads while allowing full streaming for compaction

use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tracing::{debug, trace};

use crate::core::bloom::SstableBloomFilter;
use crate::storage::engines::impls::sst::IndexEntry;

/// Query filter for intelligent block skipping
#[derive(Debug, Clone)]
pub struct BlockFilter {
    /// ID to search for (point query)
    pub target_id: Option<String>,
    /// ID range for range queries
    pub id_range: Option<(String, String)>,
    /// Metadata filters (column -> value)
    pub metadata_filters: HashMap<String, MetadataFilter>,
    /// Query type for optimization
    pub query_type: QueryType,
}

/// Metadata filter types
#[derive(Debug, Clone)]
pub enum MetadataFilter {
    Equals(JsonValue),
    Range(JsonValue, JsonValue), // min, max
    In(Vec<JsonValue>),
    NotNull,
    IsNull,
}

/// Query type for optimization
#[derive(Debug, Clone, PartialEq)]
pub enum QueryType {
    PointQuery,
    RangeQuery,
    MetadataFilter,
    FullScan,
    Compaction, // Skip all filtering
}

/// Block filtering strategy
pub struct BlockFilterStrategy {
    /// Whether to use bloom filters
    pub use_bloom_filters: bool,
    /// Whether to use min/max statistics
    pub use_min_max_stats: bool,
    /// Whether to skip all filtering (for compaction)
    pub skip_all_filtering: bool,
}

impl Default for BlockFilterStrategy {
    fn default() -> Self {
        Self {
            use_bloom_filters: true,
            use_min_max_stats: true,
            skip_all_filtering: false,
        }
    }
}

impl BlockFilterStrategy {
    /// Create strategy for compaction (no filtering)
    pub fn for_compaction() -> Self {
        Self {
            use_bloom_filters: false,
            use_min_max_stats: false,
            skip_all_filtering: true,
        }
    }
    
    /// Create strategy for point queries (bloom filters only)
    pub fn for_point_query() -> Self {
        Self {
            use_bloom_filters: true,
            use_min_max_stats: false,
            skip_all_filtering: false,
        }
    }
    
    /// Create strategy for range/metadata queries (min/max stats)
    pub fn for_range_query() -> Self {
        Self {
            use_bloom_filters: false,
            use_min_max_stats: true,
            skip_all_filtering: false,
        }
    }
}

/// Intelligent block filter for DataBlock skipping
pub struct IntelligentBlockFilter {
    search_strategy: BlockFilterStrategy,
}

impl IntelligentBlockFilter {
    /// Create a new block filter with given strategy
    pub fn new(strategy: BlockFilterStrategy) -> Self {
        Self { strategy }
    }
    
    /// Create filter for specific query type
    pub fn for_query_type(query_type: &QueryType) -> Self {
        let strategy = match query_type {
            QueryType::Compaction => BlockFilterStrategy::for_compaction(),
            QueryType::PointQuery => BlockFilterStrategy::for_point_query(),
            QueryType::RangeQuery | QueryType::MetadataFilter => BlockFilterStrategy::for_range_query(),
            QueryType::FullScan => BlockFilterStrategy::default(),
        };
        Self::new(strategy)
    }
    
    /// Check if a block should be read based on its index entry
    pub fn should_read_block(
        &self,
        index_entry: &IndexEntry,
        filter: &BlockFilter,
        global_bloom: Option<&SstableBloomFilter>,
    ) -> Result<bool> {
        // Skip all filtering for compaction
        if self.search_strategy.skip_all_filtering || filter.query_type == QueryType::Compaction {
            trace!("🔄 Compaction mode: reading all blocks without filtering");
            return Ok(true);
        }
        
        // Check point query with bloom filter
        if let Some(ref target_id) = filter.target_id {
            if self.search_strategy.use_bloom_filters {
                // Check global bloom first
                if let Some(bloom) = global_bloom {
                    if !bloom.might_contain_key(target_id)? {
                        debug!("🚫 Global bloom filter: ID '{}' not in file", target_id);
                        return Ok(false);
                    }
                }
                
                // Check block-level bloom if available
                if let Some(ref block_bloom_bytes) = index_entry.block_key_bloom {
                    let block_bloom: SstableBloomFilter = bincode::deserialize(block_bloom_bytes)?;
                    if !block_bloom.might_contain_key(target_id)? {
                        debug!("🚫 Block {} bloom filter: ID '{}' not in block", 
                               index_entry.block_id, target_id);
                        return Ok(false);
                    }
                }
            }
            
            // Check key range (blocks are sorted)
            if target_id < &index_entry.key {
                debug!("🚫 Block {} key range: target '{}' < min key '{}'", 
                       index_entry.block_id, target_id, index_entry.key);
                return Ok(false);
            }
        }
        
        // Check range query with min/max
        if let Some((ref min_id, ref max_id)) = filter.id_range {
            if self.search_strategy.use_min_max_stats {
                // Block's minimum key is after our max range
                if &index_entry.key > max_id {
                    debug!("🚫 Block {} range: min key '{}' > max range '{}'", 
                           index_entry.block_id, index_entry.key, max_id);
                    return Ok(false);
                }
                // Note: We can't skip if block's max < min_id without storing max_key per block
            }
        }
        
        // Check metadata filters with min/max statistics
        if !filter.metadata_filters.is_empty() && self.search_strategy.use_min_max_stats {
            for (column, filter_value) in &filter.metadata_filters {
                if !self.check_metadata_filter(index_entry, column, filter_value)? {
                    debug!("🚫 Block {} metadata filter: column '{}' doesn't match", 
                           index_entry.block_id, column);
                    return Ok(false);
                }
            }
        }
        
        // Block passes all filters
        trace!("✅ Block {} passes all filters, will be read", index_entry.block_id);
        Ok(true)
    }
    
    /// Check if metadata filter matches block statistics
    fn check_metadata_filter(
        &self,
        index_entry: &IndexEntry,
        column: &str,
        filter: &MetadataFilter,
    ) -> Result<bool> {
        match filter {
            MetadataFilter::Equals(value) => {
                // Check if value is within min/max range
                if let (Some(min), Some(max)) = (
                    index_entry.metadata_min_values.get(column),
                    index_entry.metadata_max_values.get(column),
                ) {
                    // If value is outside [min, max], block can be skipped
                    if !Self::value_in_range(value, min, max) {
                        return Ok(false);
                    }
                }
            }
            
            MetadataFilter::Range(filter_min, filter_max) => {
                // Check if ranges overlap
                if let (Some(block_min), Some(block_max)) = (
                    index_entry.metadata_min_values.get(column),
                    index_entry.metadata_max_values.get(column),
                ) {
                    // No overlap if block_max < filter_min or block_min > filter_max
                    if Self::compare_json_values(block_max, filter_min) == std::cmp::Ordering::Less ||
                       Self::compare_json_values(block_min, filter_max) == std::cmp::Ordering::Greater {
                        return Ok(false);
                    }
                }
            }
            
            MetadataFilter::In(values) => {
                // Check if any value could be in block's range
                if let (Some(min), Some(max)) = (
                    index_entry.metadata_min_values.get(column),
                    index_entry.metadata_max_values.get(column),
                ) {
                    let any_in_range = values.iter().any(|v| Self::value_in_range(v, min, max));
                    if !any_in_range {
                        return Ok(false);
                    }
                }
            }
            
            MetadataFilter::NotNull => {
                // Check if all values are null
                if let Some(null_count) = index_entry.metadata_null_counts.get(column) {
                    // This would require knowing total records in block
                    // For now, we can't skip based on this alone
                }
            }
            
            MetadataFilter::IsNull => {
                // Check if there are any nulls
                if let Some(null_count) = index_entry.metadata_null_counts.get(column) {
                    if *null_count == 0 {
                        return Ok(false);
                    }
                }
            }
        }
        
        Ok(true)
    }
    
    /// Check if a value is within the min/max range
    fn value_in_range(value: &JsonValue, min: &JsonValue, max: &JsonValue) -> bool {
        Self::compare_json_values(value, min) != std::cmp::Ordering::Less &&
        Self::compare_json_values(value, max) != std::cmp::Ordering::Greater
    }
    
    /// Compare two JSON values for ordering
    fn compare_json_values(a: &JsonValue, b: &JsonValue) -> std::cmp::Ordering {
        use serde_json::Value;
        match (a, b) {
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Null, _) => std::cmp::Ordering::Less,
            (_, Value::Null) => std::cmp::Ordering::Greater,
            
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            
            (Value::Number(a), Value::Number(b)) => {
                if let (Some(a_f64), Some(b_f64)) = (a.as_f64(), b.as_f64()) {
                    a_f64.partial_cmp(&b_f64)
                } else if let (Some(a_i64), Some(b_i64)) = (a.as_i64(), b.as_i64()) {
                    a_i64.cmp(&b_i64)
                } else {
                    std::cmp::Ordering::Equal
                }
            }
            
            (Value::String(a), Value::String(b)) => a.cmp(b),
            
            _ => std::cmp::Ordering::Equal, // Can't compare arrays/objects simply
        }
    }
    
    /// Get blocks that should be read for a query
    pub fn filter_blocks<'a>(
        &self,
        index_entries: &'a [IndexEntry],
        filter: &BlockFilter,
        global_bloom: Option<&SstableBloomFilter>,
    ) -> Result<Vec<&'a IndexEntry>> {
        let mut selected_blocks = Vec::new();
        
        for entry in index_entries {
            if self.should_read_block(entry, filter, global_bloom)? {
                selected_blocks.push(entry);
            }
        }
        
        debug!(
            "📊 Block filtering: {} of {} blocks selected for reading",
            selected_blocks.len(),
            index_entries.len()
        );
        
        Ok(selected_blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_block_filter_for_compaction() {
        let filter = IntelligentBlockFilter::for_query_type(&QueryType::Compaction);
        let index_entry = IndexEntry {
            key: "test".to_string(),
            offset: 0,
            size: 100,
            block_id: 1,
            block_offset: 0,
            compressed: false,
            metadata_min_values: HashMap::new(),
            metadata_max_values: HashMap::new(),
            metadata_null_counts: HashMap::new(),
            block_key_bloom: None,
            block_metadata_bloom: None,
            vector_format: crate::storage::engines::impls::sst::VectorFormat::Variable,
            // REMOVED: compression_ratio
        };
        
        let block_filter = BlockFilter {
            target_id: Some("test".to_string()),
            id_range: None,
            metadata_filters: HashMap::new(),
            query_type: QueryType::Compaction,
        };
        
        // Compaction should read all blocks
        assert!(filter.should_read_block(&index_entry, &block_filter, None).unwrap());
    }
    
    #[test]
    fn test_block_filter_point_query() {
        let filter = IntelligentBlockFilter::for_query_type(&QueryType::PointQuery);
        let mut index_entry = IndexEntry {
            key: "aaa".to_string(),
            offset: 0,
            size: 100,
            block_id: 1,
            block_offset: 0,
            compressed: false,
            metadata_min_values: HashMap::new(),
            metadata_max_values: HashMap::new(),
            metadata_null_counts: HashMap::new(),
            block_key_bloom: None,
            block_metadata_bloom: None,
            vector_format: crate::storage::engines::impls::sst::VectorFormat::Variable,
            // REMOVED: compression_ratio
        };
        
        // Query for ID before block's minimum
        let block_filter = BlockFilter {
            target_id: Some("000".to_string()),
            id_range: None,
            metadata_filters: HashMap::new(),
            query_type: QueryType::PointQuery,
        };
        
        // Should skip block since target < min key
        assert!(!filter.should_read_block(&index_entry, &block_filter, None).unwrap());
        
        // Query for ID after block's minimum
        let block_filter2 = BlockFilter {
            target_id: Some("bbb".to_string()),
            id_range: None,
            metadata_filters: HashMap::new(),
            query_type: QueryType::PointQuery,
        };
        
        // Should read block since target >= min key
        assert!(filter.should_read_block(&index_entry, &block_filter2, None).unwrap());
    }
}