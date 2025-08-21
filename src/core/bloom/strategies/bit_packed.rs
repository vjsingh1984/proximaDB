/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Bit-packed bloom filter implementation optimized for memory efficiency

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::core::bloom::{BloomFilterStrategy, BloomFilterConfig, hash};

/// Memory-efficient bit-packed bloom filter using u64 array
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitPackedBloomFilter {
    /// Bit storage using u64 for efficient bit operations
    bits: Vec<u64>,
    /// Total number of bits
    num_bits: usize,
    /// Number of hash functions
    num_hashes: u32,
    /// Number of elements inserted
    num_elements: usize,
    /// Hash seed for additional randomization
    hash_seed: u64,
}

impl BitPackedBloomFilter {
    /// Create a new bit-packed bloom filter
    pub fn new(expected_elements: usize, config: &BloomFilterConfig) -> Self {
        let bits_per_key = config.bits_per_key.max(1).min(32);
        let expected_elements = expected_elements.max(1); // Ensure at least 1 element
        let num_bits = (expected_elements * bits_per_key as usize).max(64); // Minimum 64 bits
        let num_u64s = (num_bits + 63) / 64;
        let num_hashes = ((bits_per_key as f64 * 0.69).round() as u32).max(1);
        
        Self {
            bits: vec![0u64; num_u64s],
            num_bits,
            num_hashes,
            num_elements: 0,
            hash_seed: rand::random(),
        }
    }
    
    /// Create from serialized data
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize BitPackedBloomFilter: {}", e))
    }
    
    /// Set a bit at the given index
    #[inline]
    fn set_bit(&mut self, index: usize) {
        if index >= self.num_bits {
            return;
        }
        let word_index = index / 64;
        let bit_index = index % 64;
        if word_index < self.bits.len() {
            self.bits[word_index] |= 1u64 << bit_index;
        }
    }
    
    /// Check if a bit is set at the given index
    #[inline]
    fn is_bit_set(&self, index: usize) -> bool {
        if index >= self.num_bits {
            return false;
        }
        let word_index = index / 64;
        let bit_index = index % 64;
        word_index < self.bits.len() && (self.bits[word_index] & (1u64 << bit_index)) != 0
    }
    
    /// Count the number of set bits (population count)
    fn popcount(&self) -> usize {
        self.bits.iter().map(|&word| word.count_ones() as usize).sum()
    }
}

impl BloomFilterStrategy for BitPackedBloomFilter {
    fn insert(&mut self, key: &[u8]) {
        if self.num_bits == 0 {
            return; // No-op for empty filter
        }
        // Add hash seed for additional randomization
        let mut key_with_seed = Vec::with_capacity(key.len() + 8);
        key_with_seed.extend_from_slice(key);
        key_with_seed.extend_from_slice(&self.hash_seed.to_le_bytes());
        
        let positions = hash::double_hash(&key_with_seed, self.num_hashes, self.num_bits);
        for pos in positions {
            self.set_bit(pos);
        }
        self.num_elements += 1;
    }
    
    fn might_contain(&self, key: &[u8]) -> bool {
        if self.num_bits == 0 {
            return true; // Always return true for empty filter to avoid false negatives
        }
        let mut key_with_seed = Vec::with_capacity(key.len() + 8);
        key_with_seed.extend_from_slice(key);
        key_with_seed.extend_from_slice(&self.hash_seed.to_le_bytes());
        
        let positions = hash::double_hash(&key_with_seed, self.num_hashes, self.num_bits);
        positions.iter().all(|&pos| self.is_bit_set(pos))
    }
    
    fn bit_count(&self) -> usize {
        self.num_bits
    }
    
    fn hash_count(&self) -> u32 {
        self.num_hashes
    }
    
    fn serialize(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))
    }
    
    fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>() + self.bits.capacity() * std::mem::size_of::<u64>()
    }
    
    fn clear(&mut self) {
        self.bits.fill(0);
        self.num_elements = 0;
    }
    
    fn false_positive_rate(&self) -> f64 {
        if self.num_elements == 0 {
            return 0.0;
        }
        
        // Use exact formula based on actual bit usage
        let bits_set = self.popcount();
        let fill_ratio = bits_set as f64 / self.num_bits as f64;
        
        // Approximate false positive rate
        fill_ratio.powf(self.num_hashes as f64)
    }
    
    fn num_elements(&self) -> usize {
        self.num_elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bit_packed_basic() {
        let config = BloomFilterConfig {
            // strategy removed -  crate::storage::engines::row_based::bloom_filter::BloomStrategy::BitPacked,
            bits_per_key: 10,
            expected_items: 100,
            ..Default::default()
        };
        
        let mut filter = BitPackedBloomFilter::new(100, &config);
        
        // Test basic operations
        assert_eq!(filter.num_elements(), 0);
        
        filter.insert(b"test");
        assert!(filter.might_contain(b"test"));
        assert_eq!(filter.num_elements(), 1);
        
        // Test false negatives don't occur
        for i in 0..50 {
            let key = format!("key_{}", i);
            filter.insert(key.as_bytes());
            assert!(filter.might_contain(key.as_bytes()));
        }
    }
    
    #[test]
    fn test_memory_efficiency() {
        let config = BloomFilterConfig::default();
        let filter = BitPackedBloomFilter::new(1000, &config);
        
        // Verify memory usage is reasonable
        let memory = filter.memory_usage();
        let expected_bits = 1000 * config.bits_per_key as usize;
        let expected_u64s = (expected_bits + 63) / 64;
        let expected_memory = expected_u64s * 8 + std::mem::size_of::<BitPackedBloomFilter>();
        
        assert!(memory <= expected_memory + 100); // Allow some overhead
    }
}