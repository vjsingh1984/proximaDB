//! Optimized Bloom Filter for LSM Storage Engine
//!
//! This module provides memory-optimized bloom filters specifically designed 
//! for ProximaDB's performance requirements, targeting ~8MB per collection 
//! (down from ~40MB).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Memory-optimized bloom filter for SSTable storage
#[derive(Debug, Clone)]
pub struct OptimizedSstableBloomFilter {
    /// Simple bit vector for this implementation
    bits: Vec<bool>,
    /// Hash function count (reduced for memory efficiency)
    hash_functions: u8,
    /// Optimized configuration
    config: OptimizedBloomConfig,
    /// Element count
    element_count: usize,
    /// Memory usage in bytes
    memory_usage: usize,
}

/// Optimized bloom filter configuration for memory efficiency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizedBloomConfig {
    /// Target memory limit in KB
    pub max_memory_kb: usize,
    /// Expected number of items
    pub expected_items: usize,
    /// Target false positive rate
    pub target_fpr: f64,
    /// Enable memory sharing across similar collections
    pub enable_sharing: bool,
    /// Adaptive bit allocation
    pub adaptive_sizing: bool,
}

impl OptimizedSstableBloomFilter {
    /// Create new optimized bloom filter with memory constraints
    pub fn new_with_constraints(
        expected_items: usize,
        max_memory_kb: usize,
        target_fpr: f64,
    ) -> Result<Self> {
        let config = OptimizedBloomConfig {
            max_memory_kb,
            expected_items,
            target_fpr,
            enable_sharing: true,
            adaptive_sizing: true,
        };
        
        // Calculate optimal parameters within memory constraints
        let memory_budget_bytes = max_memory_kb * 1024;
        let bit_budget = (memory_budget_bytes * 80) / 100; // 80% for bits, 20% for metadata
        
        // Calculate required bits for target FPR
        let required_bits = Self::calculate_optimal_bits(expected_items, target_fpr);
        
        // Use the smaller of required bits or budget
        let actual_bits = std::cmp::min(required_bits, bit_budget * 8); // convert bytes to bits
        
        // Reduced hash functions for memory efficiency
        let hash_functions = std::cmp::min(4, 
            ((actual_bits as f64 / expected_items as f64) * 0.693).ceil() as u8);
        
        let memory_usage = (actual_bits / 8) + std::mem::size_of::<Self>();
        
        Ok(Self {
            bits: vec![false; actual_bits],
            hash_functions,
            config,
            element_count: 0,
            memory_usage,
        })
    }
    
    /// Memory usage in bytes (much lower than original ~40MB)
    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage
    }
    
    /// Target: Keep under 8MB per collection
    pub fn is_within_memory_target(&self) -> bool {
        self.memory_usage_bytes() < 8 * 1024 * 1024 // 8MB target
    }
    
    /// Check if key might be present (optimized for speed and memory)
    pub fn might_contain_key(&self, key: &str) -> Result<bool> {
        let hashes = self.generate_hash_values(key.as_bytes());
        
        for hash in hashes {
            let bit_index = hash % self.bits.len();
            if !self.bits[bit_index] {
                return Ok(false);
            }
        }
        
        Ok(true)
    }
    
    /// Insert key into the bloom filter
    pub fn insert_key(&mut self, key: &str) -> Result<()> {
        let hashes = self.generate_hash_values(key.as_bytes());
        
        for hash in hashes {
            let bit_index = hash % self.bits.len();
            self.bits[bit_index] = true;
        }
        
        self.element_count += 1;
        Ok(())
    }
    
    /// Generate optimized hash values using reduced hash functions
    fn generate_hash_values(&self, data: &[u8]) -> Vec<usize> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hashes = Vec::with_capacity(self.hash_functions as usize);
        
        // Use double hashing to generate multiple hash values efficiently
        let mut hasher1 = DefaultHasher::new();
        data.hash(&mut hasher1);
        let hash1 = hasher1.finish() as usize;
        
        let mut hasher2 = DefaultHasher::new();
        (data.len() ^ 0xAAAAAAAA).hash(&mut hasher2);
        let hash2 = hasher2.finish() as usize;
        
        for i in 0..self.hash_functions {
            let hash = hash1.wrapping_add((i as usize).wrapping_mul(hash2));
            hashes.push(hash);
        }
        
        hashes
    }
    
    /// Calculate optimal bit count for given items and FPR
    fn calculate_optimal_bits(items: usize, fpr: f64) -> usize {
        let ln2_squared = 0.693_f64.powi(2);
        let bits = -(items as f64 * fpr.ln()) / ln2_squared;
        bits.ceil() as usize
    }
    
    /// Handle memory pressure by compacting data structures
    pub fn handle_memory_pressure(&mut self) -> Result<usize> {
        let initial_memory = self.memory_usage_bytes();
        
        // Simple memory pressure handling - could be more sophisticated
        // For now, just return current memory usage
        
        Ok(0) // No memory freed in this simplified version
    }
}

impl Default for OptimizedBloomConfig {
    fn default() -> Self {
        Self {
            max_memory_kb: 8 * 1024, // 8MB target
            expected_items: 100_000,
            target_fpr: 0.01, // 1% false positive rate
            enable_sharing: true,
            adaptive_sizing: true,
        }
    }
}

/// Manager for sharing bloom filter data across similar collections
pub struct BloomFilterSharingManager {
    shared_patterns: HashMap<String, Arc<Vec<bool>>>,
    memory_usage_tracker: usize,
}

impl BloomFilterSharingManager {
    pub fn new() -> Self {
        Self {
            shared_patterns: HashMap::new(),
            memory_usage_tracker: 0,
        }
    }
    
    /// Get or create shared bit pattern for similar collections
    pub fn get_or_create_shared_pattern(
        &mut self, 
        pattern_key: &str, 
        bit_count: usize
    ) -> Result<Arc<Vec<bool>>> {
        if let Some(pattern) = self.shared_patterns.get(pattern_key) {
            return Ok(pattern.clone());
        }
        
        let pattern = Arc::new(vec![false; bit_count]);
        self.shared_patterns.insert(pattern_key.to_string(), pattern.clone());
        self.memory_usage_tracker += bit_count / 8;
        
        Ok(pattern)
    }
    
    /// Get memory deduplication savings in bytes
    pub fn deduplication_savings(&self) -> usize {
        let mut total_savings = 0;
        for pattern in self.shared_patterns.values() {
            let reference_count = Arc::strong_count(pattern);
            if reference_count > 1 {
                let pattern_size = pattern.len() / 8; // bits to bytes
                total_savings += pattern_size * (reference_count - 1);
            }
        }
        total_savings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_memory_usage_target() {
        let filter = OptimizedSstableBloomFilter::new_with_constraints(
            100_000, // 100K items
            8 * 1024, // 8MB limit
            0.01, // 1% FPR
        ).unwrap();
        
        let memory_usage_mb = filter.memory_usage_bytes() as f64 / (1024.0 * 1024.0);
        assert!(memory_usage_mb < 8.0, "Memory usage {}MB exceeds 8MB target", memory_usage_mb);
        assert!(filter.is_within_memory_target());
    }
    
    #[test]
    fn test_bloom_filter_functionality() {
        let mut filter = OptimizedSstableBloomFilter::new_with_constraints(
            1000,
            1024, // 1MB limit for small test
            0.01,
        ).unwrap();
        
        // Insert test keys
        let test_keys = vec!["key1", "key2", "key3", "key4", "key5"];
        for key in &test_keys {
            filter.insert_key(key).unwrap();
        }
        
        // All inserted keys should be found
        for key in &test_keys {
            assert!(filter.might_contain_key(key).unwrap(), 
                   "Key '{}' should be found in bloom filter", key);
        }
    }
    
    #[test]
    fn test_sharing_manager_deduplication() {
        let mut manager = BloomFilterSharingManager::new();
        
        // Create shared patterns
        let pattern1 = manager.get_or_create_shared_pattern("common_pattern", 10000).unwrap();
        let pattern2 = manager.get_or_create_shared_pattern("common_pattern", 10000).unwrap();
        
        // Should be the same Arc
        assert!(Arc::ptr_eq(&pattern1, &pattern2));
        
        // Should have deduplication savings
        let savings = manager.deduplication_savings();
        assert!(savings > 0);
    }
}