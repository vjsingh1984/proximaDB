//! High-Performance Bloom Filter Implementation for ProximaDB
//!
//! This module provides a fast, memory-efficient bloom filter implementation
//! optimized for LSM storage engine false positive reduction. The bloom filter
//! helps skip irrelevant SSTables during search operations.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// High-performance bloom filter optimized for LSM storage
/// 
/// Uses multiple hash functions to minimize false positive rate while
/// maintaining fast membership testing. Optimized for string keys (vector IDs)
/// and metadata values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilter {
    /// Bit array for membership testing
    bit_array: Vec<u64>,
    
    /// Number of hash functions used
    hash_functions: usize,
    
    /// Total number of bits in the filter
    total_bits: usize,
    
    /// Number of elements inserted
    element_count: usize,
    
    /// Target false positive rate
    false_positive_rate: f64,
}

impl BloomFilter {
    /// Create a new bloom filter with optimal parameters
    /// 
    /// # Arguments
    /// * `expected_elements` - Expected number of elements to insert
    /// * `false_positive_rate` - Target false positive rate (0.0 - 1.0)
    /// 
    /// # Returns
    /// A new bloom filter optimized for the given parameters
    pub fn new(expected_elements: usize, false_positive_rate: f64) -> Self {
        let total_bits = Self::optimal_bit_count(expected_elements, false_positive_rate);
        let hash_functions = Self::optimal_hash_count(expected_elements, total_bits);
        
        // Round up to nearest multiple of 64 for efficient storage
        let array_size = (total_bits + 63) / 64;
        
        Self {
            bit_array: vec![0u64; array_size],
            hash_functions,
            total_bits,
            element_count: 0,
            false_positive_rate,
        }
    }
    
    /// Create a bloom filter optimized for LSM SSTable usage
    /// 
    /// Uses conservative parameters optimized for vector ID filtering:
    /// - False positive rate: 0.1% (very low for minimal I/O waste)
    /// - Expected elements: Based on typical SSTable size
    pub fn for_sstable(expected_vectors: usize) -> Self {
        Self::new(expected_vectors, 0.001) // 0.1% false positive rate
    }
    
    /// Create a bloom filter for metadata filtering
    /// 
    /// Uses slightly higher false positive rate since metadata filtering
    /// is typically followed by additional verification
    pub fn for_metadata(expected_values: usize) -> Self {
        Self::new(expected_values, 0.01) // 1% false positive rate
    }
    
    /// Insert an element into the bloom filter
    /// 
    /// # Arguments
    /// * `element` - Element to insert (typically vector ID or metadata value)
    pub fn insert<T: Hash>(&mut self, element: &T) {
        let hashes = self.hash_element(element);
        
        for hash in hashes {
            let bit_index = (hash % self.total_bits as u64) as usize;
            let array_index = bit_index / 64;
            let bit_offset = bit_index % 64;
            
            self.bit_array[array_index] |= 1u64 << bit_offset;
        }
        
        self.element_count += 1;
    }
    
    /// Test if an element might be in the set
    /// 
    /// # Returns
    /// * `true` - Element might be in the set (could be false positive)
    /// * `false` - Element is definitely not in the set
    pub fn might_contain<T: Hash>(&self, element: &T) -> bool {
        let hashes = self.hash_element(element);
        
        for hash in hashes {
            let bit_index = (hash % self.total_bits as u64) as usize;
            let array_index = bit_index / 64;
            let bit_offset = bit_index % 64;
            
            if (self.bit_array[array_index] & (1u64 << bit_offset)) == 0 {
                return false; // Definitely not present
            }
        }
        
        true // Might be present
    }
    
    /// Get current false positive probability based on actual load
    pub fn actual_false_positive_rate(&self) -> f64 {
        if self.element_count == 0 {
            return 0.0;
        }
        
        let load_factor = self.element_count as f64 / self.total_bits as f64;
        (1.0 - (-(self.hash_functions as f64) * load_factor).exp()).powi(self.hash_functions as i32)
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        self.bit_array.len() * 8 + std::mem::size_of::<Self>()
    }
    
    /// Get filter statistics for monitoring
    pub fn statistics(&self) -> BloomFilterStats {
        BloomFilterStats {
            total_bits: self.total_bits,
            hash_functions: self.hash_functions,
            element_count: self.element_count,
            target_false_positive_rate: self.false_positive_rate,
            actual_false_positive_rate: self.actual_false_positive_rate(),
            memory_usage_bytes: self.memory_usage(),
            utilization: self.element_count as f64 / self.total_bits as f64,
        }
    }
    
    /// Clear the bloom filter (reset to empty state)
    pub fn clear(&mut self) {
        self.bit_array.fill(0);
        self.element_count = 0;
    }
    
    /// Merge another bloom filter into this one
    /// 
    /// Both filters must have identical parameters (bits, hash functions)
    pub fn merge(&mut self, other: &BloomFilter) -> Result<()> {
        if self.total_bits != other.total_bits || self.hash_functions != other.hash_functions {
            return Err(anyhow::anyhow!(
                "Cannot merge bloom filters with different parameters"
            ));
        }
        
        for i in 0..self.bit_array.len() {
            self.bit_array[i] |= other.bit_array[i];
        }
        
        self.element_count += other.element_count;
        Ok(())
    }
    
    // Private helper methods
    
    /// Calculate optimal number of bits for given parameters
    fn optimal_bit_count(expected_elements: usize, false_positive_rate: f64) -> usize {
        let bits = -(expected_elements as f64 * false_positive_rate.ln()) 
                  / (std::f64::consts::LN_2.powi(2));
        bits.ceil() as usize
    }
    
    /// Calculate optimal number of hash functions
    fn optimal_hash_count(expected_elements: usize, total_bits: usize) -> usize {
        let hash_count = (total_bits as f64 / expected_elements as f64) * std::f64::consts::LN_2;
        hash_count.round().max(1.0) as usize
    }
    
    /// Generate multiple hash values for an element
    fn hash_element<T: Hash>(&self, element: &T) -> Vec<u64> {
        // Use double hashing technique for multiple hash functions
        let mut hasher1 = DefaultHasher::new();
        element.hash(&mut hasher1);
        let hash1 = hasher1.finish();
        
        let mut hasher2 = DefaultHasher::new();
        (element, 0xDEADBEEFu64).hash(&mut hasher2);
        let hash2 = hasher2.finish();
        
        let mut hashes = Vec::with_capacity(self.hash_functions);
        for i in 0..self.hash_functions {
            hashes.push(hash1.wrapping_add((i as u64).wrapping_mul(hash2)));
        }
        
        hashes
    }
}

/// Statistics about bloom filter performance and usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterStats {
    pub total_bits: usize,
    pub hash_functions: usize,
    pub element_count: usize,
    pub target_false_positive_rate: f64,
    pub actual_false_positive_rate: f64,
    pub memory_usage_bytes: usize,
    pub utilization: f64,
}

/// Bloom filter collection for managing multiple filters
/// 
/// Used by LSM engine to maintain separate bloom filters for each SSTable
pub struct BloomFilterCollection {
    filters: std::collections::HashMap<String, BloomFilter>,
    default_expected_elements: usize,
    default_false_positive_rate: f64,
}

impl BloomFilterCollection {
    /// Create a new bloom filter collection
    pub fn new(default_expected_elements: usize, default_false_positive_rate: f64) -> Self {
        Self {
            filters: std::collections::HashMap::new(),
            default_expected_elements,
            default_false_positive_rate,
        }
    }
    
    /// Get or create a bloom filter for a specific key (e.g., SSTable ID)
    pub fn get_or_create(&mut self, key: &str) -> &mut BloomFilter {
        self.filters.entry(key.to_string()).or_insert_with(|| {
            BloomFilter::new(self.default_expected_elements, self.default_false_positive_rate)
        })
    }
    
    /// Get an existing bloom filter
    pub fn get(&self, key: &str) -> Option<&BloomFilter> {
        self.filters.get(key)
    }
    
    /// Check if any filter in the collection might contain the element
    pub fn any_might_contain<T: Hash>(&self, element: &T) -> bool {
        self.filters.values().any(|filter| filter.might_contain(element))
    }
    
    /// Get filters that might contain the element
    pub fn filters_might_contain<T: Hash>(&self, element: &T) -> Vec<&str> {
        self.filters
            .iter()
            .filter(|(_, filter)| filter.might_contain(element))
            .map(|(key, _)| key.as_str())
            .collect()
    }
    
    /// Remove a bloom filter from the collection
    pub fn remove(&mut self, key: &str) -> Option<BloomFilter> {
        self.filters.remove(key)
    }
    
    /// Get collection statistics
    pub fn collection_stats(&self) -> BloomFilterCollectionStats {
        let total_memory = self.filters.values().map(|f| f.memory_usage()).sum();
        let total_elements = self.filters.values().map(|f| f.element_count).sum();
        let avg_false_positive_rate = if self.filters.is_empty() {
            0.0
        } else {
            self.filters.values().map(|f| f.actual_false_positive_rate()).sum::<f64>() 
                / self.filters.len() as f64
        };
        
        BloomFilterCollectionStats {
            filter_count: self.filters.len(),
            total_memory_bytes: total_memory,
            total_elements: total_elements,
            average_false_positive_rate: avg_false_positive_rate,
        }
    }
}

/// Statistics for bloom filter collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterCollectionStats {
    pub filter_count: usize,
    pub total_memory_bytes: usize,
    pub total_elements: usize,
    pub average_false_positive_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic_operations() {
        let mut filter = BloomFilter::new(1000, 0.01);
        
        // Test insertion and lookup
        filter.insert(&"vector_123");
        filter.insert(&"vector_456");
        filter.insert(&"vector_789");
        
        // These should definitely be found
        assert!(filter.might_contain(&"vector_123"));
        assert!(filter.might_contain(&"vector_456"));
        assert!(filter.might_contain(&"vector_789"));
        
        // This should definitely not be found (with very high probability)
        assert!(!filter.might_contain(&"nonexistent_vector"));
    }
    
    #[test]
    fn test_false_positive_rate() {
        let mut filter = BloomFilter::new(100, 0.01); // 1% target FP rate
        
        // Insert known elements
        let known_elements: Vec<String> = (0..100).map(|i| format!("vector_{}", i)).collect();
        for element in &known_elements {
            filter.insert(element);
        }
        
        // Test unknown elements for false positives
        let test_elements: Vec<String> = (1000..2000).map(|i| format!("test_{}", i)).collect();
        let false_positives = test_elements
            .iter()
            .filter(|&element| filter.might_contain(element))
            .count();
        
        let actual_fp_rate = false_positives as f64 / test_elements.len() as f64;
        
        // Should be reasonably close to target (allowing some variance due to randomness)
        assert!(actual_fp_rate < 0.05, "False positive rate too high: {}", actual_fp_rate);
    }
    
    #[test]
    fn test_bloom_filter_statistics() {
        let mut filter = BloomFilter::for_sstable(1000);
        
        for i in 0..500 {
            filter.insert(&format!("vector_{}", i));
        }
        
        let stats = filter.statistics();
        assert_eq!(stats.element_count, 500);
        assert!(stats.memory_usage_bytes > 0);
        assert!(stats.actual_false_positive_rate >= 0.0);
        assert!(stats.utilization > 0.0 && stats.utilization <= 1.0);
    }
    
    #[test]
    fn test_bloom_filter_merge() {
        let mut filter1 = BloomFilter::new(100, 0.01);
        let mut filter2 = BloomFilter::new(100, 0.01);
        
        filter1.insert(&"item1");
        filter1.insert(&"item2");
        
        filter2.insert(&"item3");
        filter2.insert(&"item4");
        
        filter1.merge(&filter2).unwrap();
        
        // All items should be found in merged filter
        assert!(filter1.might_contain(&"item1"));
        assert!(filter1.might_contain(&"item2"));
        assert!(filter1.might_contain(&"item3"));
        assert!(filter1.might_contain(&"item4"));
    }
    
    #[test]
    fn test_bloom_filter_collection() {
        let mut collection = BloomFilterCollection::new(100, 0.01);
        
        // Add elements to different filters
        collection.get_or_create("sstable1").insert(&"vector_1");
        collection.get_or_create("sstable1").insert(&"vector_2");
        collection.get_or_create("sstable2").insert(&"vector_3");
        
        // Test lookups
        assert!(collection.get("sstable1").unwrap().might_contain(&"vector_1"));
        assert!(collection.get("sstable2").unwrap().might_contain(&"vector_3"));
        assert!(!collection.get("sstable1").unwrap().might_contain(&"vector_3"));
        
        // Test collection-wide search
        let filters_with_vector1 = collection.filters_might_contain(&"vector_1");
        assert_eq!(filters_with_vector1, vec!["sstable1"]);
        
        let stats = collection.collection_stats();
        assert_eq!(stats.filter_count, 2);
        assert_eq!(stats.total_elements, 3);
    }
    
    #[test]
    fn test_optimal_parameters() {
        // Test parameter calculation
        let filter = BloomFilter::new(1000, 0.01);
        assert!(filter.hash_functions > 0);
        assert!(filter.total_bits > 1000); // Should be larger than element count
        
        // Smaller false positive rate should require more bits
        let filter_precise = BloomFilter::new(1000, 0.001);
        assert!(filter_precise.total_bits > filter.total_bits);
    }
}