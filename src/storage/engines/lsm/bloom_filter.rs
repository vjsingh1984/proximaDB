/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Bloom Filter implementation for LSM Tree
//! 
//! Provides probabilistic key existence checking to reduce disk I/O during searches.
//! Used to quickly determine if a key might exist in an SSTable before reading it.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Bloom filter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterConfig {
    /// Target false positive rate (default: 0.01 = 1%)
    pub false_positive_rate: f64,
    /// Minimum number of elements (to avoid too small filters)
    pub min_elements: usize,
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            false_positive_rate: 0.01,
            min_elements: 100,
        }
    }
}

/// Bloom filter for probabilistic key existence checking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilter {
    /// Bit vector storing the filter
    pub bits: Vec<u8>,
    /// Number of hash functions to use
    pub num_hashes: u32,
    /// Total number of bits in the filter
    pub num_bits: u32,
    /// Estimated number of elements inserted
    pub num_elements: u32,
}

impl BloomFilter {
    /// Create a new bloom filter with optimal parameters
    pub fn new(num_elements: usize, config: &BloomFilterConfig) -> Self {
        let num_elements = num_elements.max(config.min_elements).min(1_000_000) as u32; // Cap at 1M elements
        let false_positive_rate = config.false_positive_rate.max(0.0001).min(0.5); // Reasonable bounds
        
        // Calculate optimal bloom filter size
        // m = -n * ln(p) / (ln(2)^2)
        let num_bits = ((-1.0 * num_elements as f64 * false_positive_rate.ln()) 
            / (2.0_f64.ln().powi(2))).ceil() as u32;
        
        // Cap the number of bits to prevent overflow
        let num_bits = num_bits.min(16_777_216); // Cap at 16M bits (2MB)
        
        // Calculate optimal number of hash functions
        // k = (m/n) * ln(2)
        let num_hashes = ((num_bits as f64 / num_elements as f64) * 2.0_f64.ln()).ceil() as u32;
        
        // Ensure reasonable bounds for hash functions
        let num_hashes = num_hashes.max(1).min(32);
        
        // Allocate bit vector (rounded up to byte boundary)
        let byte_count = ((num_bits + 7) / 8) as usize;
        let bits = vec![0u8; byte_count];
        
        Self {
            bits,
            num_hashes,
            num_bits,
            num_elements,
        }
    }
    
    /// Insert a key into the bloom filter
    pub fn insert(&mut self, key: &str) {
        for hash_num in 0..self.num_hashes {
            let hash = self.hash_key(key, hash_num);
            let bit_index = hash % self.num_bits;
            self.set_bit(bit_index);
        }
    }
    
    /// Check if a key might exist in the filter
    /// Returns false if key definitely doesn't exist
    /// Returns true if key might exist (with false_positive_rate probability of being wrong)
    pub fn might_contain(&self, key: &str) -> bool {
        for hash_num in 0..self.num_hashes {
            let hash = self.hash_key(key, hash_num);
            let bit_index = hash % self.num_bits;
            if !self.get_bit(bit_index) {
                return false;
            }
        }
        true
    }
    
    /// Get the size of the bloom filter in bytes
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }
    
    /// Get the theoretical false positive rate
    pub fn false_positive_rate(&self) -> f64 {
        // (1 - e^(-k*n/m))^k
        let k = self.num_hashes as f64;
        let n = self.num_elements as f64;
        let m = self.num_bits as f64;
        
        (1.0 - (-k * n / m).exp()).powf(k)
    }
    
    /// Hash a key with a specific hash function number
    fn hash_key(&self, key: &str, hash_num: u32) -> u32 {
        use std::collections::hash_map::DefaultHasher;
        
        // Use double hashing technique: h(i) = h1(x) + i * h2(x)
        let mut hasher1 = DefaultHasher::new();
        key.hash(&mut hasher1);
        let hash1 = hasher1.finish();
        
        let mut hasher2 = DefaultHasher::new();
        hash_num.hash(&mut hasher2);
        key.hash(&mut hasher2);
        let hash2 = hasher2.finish();
        
        // Combine hashes with wrapping operations to prevent overflow
        let combined = hash1.wrapping_add((hash_num as u64).wrapping_mul(hash2));
        
        // Ensure num_bits is not zero to prevent division by zero
        if self.num_bits == 0 {
            return 0;
        }
        
        (combined % self.num_bits as u64) as u32
    }
    
    /// Set a bit in the bit vector
    fn set_bit(&mut self, bit_index: u32) {
        let byte_index = (bit_index / 8) as usize;
        let bit_offset = bit_index % 8;
        if byte_index < self.bits.len() {
            self.bits[byte_index] |= 1 << bit_offset;
        }
    }
    
    /// Get a bit from the bit vector
    fn get_bit(&self, bit_index: u32) -> bool {
        let byte_index = (bit_index / 8) as usize;
        let bit_offset = bit_index % 8;
        if byte_index < self.bits.len() {
            (self.bits[byte_index] & (1 << bit_offset)) != 0
        } else {
            false
        }
    }
}

/// Builder for creating bloom filters from multiple keys
pub struct BloomFilterBuilder {
    config: BloomFilterConfig,
    keys: Vec<String>,
}

impl BloomFilterBuilder {
    pub fn new(config: BloomFilterConfig) -> Self {
        Self {
            config,
            keys: Vec::new(),
        }
    }
    
    pub fn add_key(&mut self, key: String) {
        self.keys.push(key);
    }
    
    pub fn add_keys<I, S>(&mut self, keys: I) 
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keys.extend(keys.into_iter().map(|k| k.into()));
    }
    
    pub fn build(self) -> BloomFilter {
        let mut filter = BloomFilter::new(self.keys.len(), &self.config);
        for key in &self.keys {
            filter.insert(key);
        }
        filter
    }
}

/// Multi-column bloom filter for metadata filtering
/// Each column gets its own bloom filter to optimize metadata queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataBloomFilter {
    /// Per-column bloom filters
    pub column_filters: HashMap<String, BloomFilter>,
    /// Configuration used for all column filters
    pub config: BloomFilterConfig,
}

impl MetadataBloomFilter {
    /// Create a new metadata bloom filter
    pub fn new(config: BloomFilterConfig) -> Self {
        Self {
            column_filters: HashMap::new(),
            config,
        }
    }

    /// Add a column bloom filter
    pub fn add_column(&mut self, column_name: String, values: Vec<String>) {
        let mut filter = BloomFilter::new(values.len(), &self.config);
        for value in values {
            filter.insert(&value);
        }
        self.column_filters.insert(column_name, filter);
    }

    /// Check if a metadata query might match any records
    pub fn might_match_metadata(&self, column: &str, value: &str) -> bool {
        if let Some(filter) = self.column_filters.get(column) {
            filter.might_contain(value)
        } else {
            // If no filter exists for this column, assume it might match
            true
        }
    }

    /// Check if multiple metadata conditions might match
    pub fn might_match_conditions(&self, conditions: &HashMap<String, String>) -> bool {
        for (column, value) in conditions {
            if !self.might_match_metadata(column, value) {
                return false;
            }
        }
        true
    }

    /// Get total size of all bloom filters
    pub fn total_size_bytes(&self) -> usize {
        self.column_filters.values().map(|f| f.size_bytes()).sum()
    }

    /// Get number of columns with filters
    pub fn num_columns(&self) -> usize {
        self.column_filters.len()
    }
}

/// Builder for metadata bloom filters
pub struct MetadataBloomFilterBuilder {
    config: BloomFilterConfig,
    column_data: HashMap<String, Vec<String>>,
}

impl MetadataBloomFilterBuilder {
    pub fn new(config: BloomFilterConfig) -> Self {
        Self {
            config,
            column_data: HashMap::new(),
        }
    }

    /// Add values for a specific column
    pub fn add_column_values(&mut self, column: String, values: Vec<String>) {
        self.column_data.insert(column, values);
    }

    /// Add a single value for a column
    pub fn add_value(&mut self, column: String, value: String) {
        self.column_data.entry(column).or_insert_with(Vec::new).push(value);
    }

    /// Build the metadata bloom filter
    pub fn build(self) -> MetadataBloomFilter {
        let mut filter = MetadataBloomFilter::new(self.config);
        for (column, values) in self.column_data {
            filter.add_column(column, values);
        }
        filter
    }
}

/// Combined bloom filter for LSM SSTable blocks
/// Includes both key and metadata filtering capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableBloomFilter {
    /// Primary key bloom filter
    pub key_filter: BloomFilter,
    /// Metadata bloom filters per column
    pub metadata_filter: MetadataBloomFilter,
    /// Statistics for optimization
    pub stats: BloomFilterStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterStats {
    /// Total number of keys inserted
    pub total_keys: u64,
    /// Number of metadata queries saved
    pub metadata_queries_saved: u64,
    /// Number of key lookups saved
    pub key_lookups_saved: u64,
}

impl SstableBloomFilter {
    /// Create a new SSTable bloom filter
    pub fn new(key_filter: BloomFilter, metadata_filter: MetadataBloomFilter) -> Self {
        Self {
            key_filter,
            metadata_filter,
            stats: BloomFilterStats {
                total_keys: 0,
                metadata_queries_saved: 0,
                key_lookups_saved: 0,
            },
        }
    }

    /// Check if a key might exist in the SSTable
    pub fn might_contain_key(&mut self, key: &str) -> bool {
        let result = self.key_filter.might_contain(key);
        if !result {
            self.stats.key_lookups_saved += 1;
        }
        result
    }

    /// Check if metadata conditions might match
    pub fn might_match_metadata(&mut self, conditions: &HashMap<String, String>) -> bool {
        let result = self.metadata_filter.might_match_conditions(conditions);
        if !result {
            self.stats.metadata_queries_saved += 1;
        }
        result
    }

    /// Check if both key and metadata conditions might match
    pub fn might_match_query(&mut self, key: Option<&str>, metadata: Option<&HashMap<String, String>>) -> bool {
        if let Some(key) = key {
            if !self.might_contain_key(key) {
                return false;
            }
        }
        
        if let Some(metadata) = metadata {
            if !self.might_match_metadata(metadata) {
                return false;
            }
        }
        
        true
    }

    /// Get total size of all bloom filters
    pub fn total_size_bytes(&self) -> usize {
        self.key_filter.size_bytes() + self.metadata_filter.total_size_bytes()
    }

    /// Get efficiency statistics
    pub fn efficiency_stats(&self) -> (f64, f64) {
        let key_efficiency = if self.stats.total_keys > 0 {
            self.stats.key_lookups_saved as f64 / self.stats.total_keys as f64
        } else {
            0.0
        };
        
        let metadata_efficiency = if self.stats.total_keys > 0 {
            self.stats.metadata_queries_saved as f64 / self.stats.total_keys as f64
        } else {
            0.0
        };
        
        (key_efficiency, metadata_efficiency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bloom_filter_basic() {
        let mut filter = BloomFilter::new(1000, &BloomFilterConfig::default());
        
        // Insert some keys
        filter.insert("key1");
        filter.insert("key2");
        filter.insert("key3");
        
        // Check they might exist
        assert!(filter.might_contain("key1"));
        assert!(filter.might_contain("key2"));
        assert!(filter.might_contain("key3"));
        
        // Check non-existent keys
        // These should mostly return false, but some might return true (false positives)
        let non_existent = ["key4", "key5", "random", "nothere"];
        let false_positives = non_existent.iter()
            .filter(|k| filter.might_contain(k))
            .count();
        
        // With 1% false positive rate, we expect 0-1 false positives out of 4
        assert!(false_positives <= 1);
    }
    
    #[test]
    fn test_bloom_filter_builder() {
        let mut builder = BloomFilterBuilder::new(BloomFilterConfig::default());
        builder.add_keys(vec!["apple", "banana", "cherry", "date"]);
        
        let filter = builder.build();
        
        assert!(filter.might_contain("apple"));
        assert!(filter.might_contain("banana"));
        assert!(filter.might_contain("cherry"));
        assert!(filter.might_contain("date"));
    }
    
    #[test]
    fn test_bloom_filter_parameters() {
        let config = BloomFilterConfig {
            false_positive_rate: 0.001, // 0.1%
            min_elements: 10,
        };
        
        let filter = BloomFilter::new(10000, &config);
        
        // With stricter false positive rate, we should have more bits and hashes
        assert!(filter.num_bits > 100000); // Should be around 143,775 bits
        assert!(filter.num_hashes >= 9); // Should be around 10 hash functions
        
        // Theoretical false positive rate should be close to target
        let theoretical_fpr = filter.false_positive_rate();
        assert!((theoretical_fpr - 0.001).abs() < 0.0005);
    }
}