/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Byte-aligned bloom filter implementation optimized for disk storage

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::core::bloom::{BloomFilterStrategy, BloomFilterConfig, hash};

/// Byte-aligned bloom filter optimized for SSTable storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByteAlignedBloomFilter {
    /// Bit storage using byte array
    bits: Vec<u8>,
    /// Total number of bits
    num_bits: u32,
    /// Number of hash functions
    num_hashes: u32,
    /// Number of elements inserted
    num_elements: usize,
    /// Bits per key configuration
    bits_per_key: u32,
}

impl ByteAlignedBloomFilter {
    /// Create a new byte-aligned bloom filter
    pub fn new(expected_elements: usize, config: &BloomFilterConfig) -> Self {
        let bits_per_key = config.bits_per_key.max(1).min(32);
        let expected_elements = expected_elements.max(1); // Ensure at least 1 element
        let num_bits = ((expected_elements as u64 * bits_per_key as u64) as u32).max(8); // Minimum 8 bits
        let num_bytes = ((num_bits + 7) / 8) as usize;
        let num_hashes = ((bits_per_key as f64 * 0.69).round() as u32).max(1);
        
        Self {
            bits: vec![0; num_bytes],
            num_bits,
            num_hashes,
            num_elements: 0,
            bits_per_key,
        }
    }
    
    /// Create from serialized data
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize ByteAlignedBloomFilter: {}", e))
    }
    
    /// Set a bit at the given index
    #[inline]
    fn set_bit(&mut self, index: usize) {
        let byte_index = index / 8;
        let bit_index = index % 8;
        if byte_index < self.bits.len() {
            self.bits[byte_index] |= 1 << bit_index;
        }
    }
    
    /// Check if a bit is set at the given index
    #[inline]
    fn is_bit_set(&self, index: usize) -> bool {
        let byte_index = index / 8;
        let bit_index = index % 8;
        byte_index < self.bits.len() && (self.bits[byte_index] & (1 << bit_index)) != 0
    }
}

impl BloomFilterStrategy for ByteAlignedBloomFilter {
    fn insert(&mut self, key: &[u8]) {
        if self.num_bits == 0 {
            return; // No-op for empty filter
        }
        let positions = hash::double_hash(key, self.num_hashes, self.num_bits as usize);
        for pos in positions {
            self.set_bit(pos);
        }
        self.num_elements += 1;
    }
    
    fn might_contain(&self, key: &[u8]) -> bool {
        if self.num_bits == 0 {
            return true; // Always return true for empty filter to avoid false negatives
        }
        let positions = hash::double_hash(key, self.num_hashes, self.num_bits as usize);
        positions.iter().all(|&pos| self.is_bit_set(pos))
    }
    
    fn bit_count(&self) -> usize {
        self.num_bits as usize
    }
    
    fn hash_count(&self) -> u32 {
        self.num_hashes
    }
    
    fn serialize(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))
    }
    
    fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>() + self.bits.capacity()
    }
    
    fn clear(&mut self) {
        self.bits.fill(0);
        self.num_elements = 0;
    }
    
    fn false_positive_rate(&self) -> f64 {
        if self.num_elements == 0 {
            return 0.0;
        }
        
        // Calculate actual false positive rate based on fill ratio
        let bits_set = self.bits.iter()
            .map(|&byte| byte.count_ones() as usize)
            .sum::<usize>();
        
        let fill_ratio = bits_set as f64 / self.num_bits as f64;
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
    fn test_byte_aligned_basic() {
        let config = BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            bits_per_key: 10,
            expected_items: 100,
            ..Default::default()
        };
        
        let mut filter = ByteAlignedBloomFilter::new(100, &config);
        
        // Test basic insert and lookup
        filter.insert(b"test_key");
        assert!(filter.might_contain(b"test_key"));
        assert!(!filter.might_contain(b"unknown_key"));
        
        // Test multiple inserts
        for i in 0..10 {
            filter.insert(&format!("key_{}", i).into_bytes());
        }
        
        for i in 0..10 {
            assert!(filter.might_contain(&format!("key_{}", i).into_bytes()));
        }
    }
    
    #[test]
    fn test_serialization() {
        let config = BloomFilterConfig::default();
        let mut filter = ByteAlignedBloomFilter::new(50, &config);
        
        // Add some data
        filter.insert(b"key1");
        filter.insert(b"key2");
        
        // Serialize
        let serialized = BloomFilterStrategy::serialize(&filter).unwrap();
        
        // Deserialize
        let restored = ByteAlignedBloomFilter::from_bytes(&serialized).unwrap();
        
        // Verify
        assert!(restored.might_contain(b"key1"));
        assert!(restored.might_contain(b"key2"));
        assert!(!restored.might_contain(b"key3"));
        assert_eq!(restored.num_elements(), 2);
    }
}