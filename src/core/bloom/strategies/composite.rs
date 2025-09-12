/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Composite bloom filter supporting both keys and metadata

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::ByteAlignedBloomFilter;
use crate::core::bloom::{BloomFilterConfig, BloomFilterStrategy, MetadataBloomFilter};

/// Composite bloom filter combining key and metadata filters
#[derive(Debug, Clone)]
pub struct CompositeBloomFilter {
    /// Primary filter for keys
    key_filter: ByteAlignedBloomFilter,
    /// Metadata filters by column name
    metadata_filters: HashMap<String, ByteAlignedBloomFilter>,
    /// Configuration
    config: BloomFilterConfig,
    /// Total elements across all filters
    total_elements: usize,
}

impl CompositeBloomFilter {
    /// Create a new composite bloom filter
    pub fn new(expected_elements: usize, config: &BloomFilterConfig) -> Self {
        let key_filter = ByteAlignedBloomFilter::new(expected_elements, config);

        Self {
            key_filter,
            metadata_filters: HashMap::new(),
            config: config.clone(),
            total_elements: 0,
        }
    }

    /// Create from serialized data
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize CompositeBloomFilter: {}", e))
    }

    /// Get or create a metadata filter for a column
    fn get_or_create_metadata_filter(&mut self, column: &str) -> &mut ByteAlignedBloomFilter {
        self.metadata_filters
            .entry(column.to_string())
            .or_insert_with(|| {
                ByteAlignedBloomFilter::new(
                    self.config.expected_items / 10, // Assume fewer unique values per column
                    &self.config,
                )
            })
    }

    /// Get number of metadata columns
    pub fn num_columns(&self) -> usize {
        self.metadata_filters.len()
    }

    /// Get number of metadata columns (alias for compatibility)
    pub fn metadata_columns(&self) -> usize {
        self.num_columns()
    }
}

impl BloomFilterStrategy for CompositeBloomFilter {
    fn insert(&mut self, key: &[u8]) {
        self.key_filter.insert(key);
        self.total_elements += 1;
    }

    fn might_contain(&self, key: &[u8]) -> bool {
        self.key_filter.might_contain(key)
    }

    fn bit_count(&self) -> usize {
        self.key_filter.bit_count()
            + self
                .metadata_filters
                .values()
                .map(|f| f.bit_count())
                .sum::<usize>()
    }

    fn hash_count(&self) -> usize {
        self.key_filter.hash_count()
    }

    fn serialize(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))
    }

    fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.key_filter.memory_usage()
            + self
                .metadata_filters
                .values()
                .map(|f| f.memory_usage())
                .sum::<usize>()
    }

    fn clear(&mut self) {
        self.key_filter.clear();
        self.metadata_filters.clear();
        self.total_elements = 0;
    }

    fn false_positive_rate(&self) -> f64 {
        self.key_filter.false_positive_rate()
    }

    fn num_elements(&self) -> usize {
        self.total_elements
    }
}

impl MetadataBloomFilter for CompositeBloomFilter {
    fn insert_metadata(&mut self, column: &str, item: &crate::proto::proximadb_v1::MetadataItem) {
        let filter = self.get_or_create_metadata_filter(column);
        // Serialize the metadata item for consistent hashing
        let serialized = crate::core::bloom::serialize_metadata_value(item);
        filter.insert(serialized.as_bytes());
    }

    fn might_match_metadata(
        &self,
        column: &str,
        item: &crate::proto::proximadb_v1::MetadataItem,
    ) -> bool {
        self.metadata_filters
            .get(column)
            .map(|filter| {
                let serialized = crate::core::bloom::serialize_metadata_value(item);
                filter.might_contain(serialized.as_bytes())
            })
            .unwrap_or(false)
    }

    fn num_columns(&self) -> usize {
        self.metadata_filters.len()
    }
}

/// Builder for composite bloom filters with metadata
pub struct CompositeBloomFilterBuilder {
    config: BloomFilterConfig,
    key_filter: ByteAlignedBloomFilter,
    metadata_values: HashMap<String, Vec<crate::proto::proximadb_v1::MetadataItem>>,
}

impl CompositeBloomFilterBuilder {
    /// Create a new builder
    pub fn new(config: BloomFilterConfig) -> Self {
        let key_filter = ByteAlignedBloomFilter::new(config.expected_items, &config);

        Self {
            config,
            key_filter,
            metadata_values: HashMap::new(),
        }
    }

    /// Add a key
    pub fn add_key(&mut self, key: &str) {
        self.key_filter.insert(key.as_bytes());
    }

    /// Add a metadata value
    pub fn add_metadata_item(
        &mut self,
        column: String,
        item: crate::proto::proximadb_v1::MetadataItem,
    ) {
        self.metadata_values
            .entry(column)
            .or_insert_with(Vec::new)
            .push(item);
    }

    /// Build the composite filter
    pub fn build(self) -> CompositeBloomFilter {
        let total_elements = self.key_filter.num_elements();
        let mut filter = CompositeBloomFilter {
            key_filter: self.key_filter,
            metadata_filters: HashMap::new(),
            config: self.config.clone(),
            total_elements,
        };

        // Create metadata filters
        for (column, items) in self.metadata_values {
            for item in items {
                filter.insert_metadata(&column, &item);
            }
        }

        filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bloom::BloomStrategy;

    #[test]
    fn test_composite_filter() {
        let config = BloomFilterConfig {
            // strategy removed -  BloomStrategy::Composite,
            expected_items: 1000,
            ..Default::default()
        };

        let mut filter = CompositeBloomFilter::new(1000, &config);

        // Test key operations
        filter.insert(b"key1");
        assert!(filter.might_contain(b"key1"));
        assert!(!filter.might_contain(b"key2"));

        // Test metadata operations - create MetadataItem instances for testing
        let electronics_item = crate::proto::proximadb_v1::MetadataItem {
            key: "category".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                    "electronics".to_string(),
                ),
            ),
        };
        let books_item = crate::proto::proximadb_v1::MetadataItem {
            key: "category".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue("books".to_string()),
            ),
        };
        let clothing_item = crate::proto::proximadb_v1::MetadataItem {
            key: "category".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                    "clothing".to_string(),
                ),
            ),
        };
        let premium_item = crate::proto::proximadb_v1::MetadataItem {
            key: "type".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                    "premium".to_string(),
                ),
            ),
        };
        let basic_item = crate::proto::proximadb_v1::MetadataItem {
            key: "type".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue("basic".to_string()),
            ),
        };

        filter.insert_metadata("category", &electronics_item);
        filter.insert_metadata("category", &books_item);
        filter.insert_metadata("type", &premium_item);

        assert!(filter.might_match_metadata("category", &electronics_item));
        assert!(filter.might_match_metadata("category", &books_item));
        assert!(!filter.might_match_metadata("category", &clothing_item));
        assert!(filter.might_match_metadata("type", &premium_item));
        assert!(!filter.might_match_metadata("type", &basic_item));

        assert_eq!(filter.num_columns(), 2);
    }

    #[test]
    fn test_builder() {
        let config = BloomFilterConfig::default();
        let mut builder = CompositeBloomFilterBuilder::new(config);

        builder.add_key("product_1");
        builder.add_key("product_2");

        // Create MetadataItems for testing
        let electronics_item = crate::proto::proximadb_v1::MetadataItem {
            key: "category".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                    "electronics".to_string(),
                ),
            ),
        };
        let books_item = crate::proto::proximadb_v1::MetadataItem {
            key: "category".to_string(),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue("books".to_string()),
            ),
        };

        builder.add_metadata_item("category".to_string(), electronics_item.clone());
        builder.add_metadata_item("category".to_string(), books_item.clone());

        let filter = builder.build();

        assert!(filter.might_contain(b"product_1"));
        assert!(filter.might_contain(b"product_2"));
        assert!(filter.might_match_metadata("category", &electronics_item));
        assert!(filter.might_match_metadata("category", &books_item));
    }
}
