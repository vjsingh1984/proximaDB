/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Simple bloom filter implementation for small datasets and testing

use anyhow::Result;

use crate::{BloomFilterConfig, BloomFilterStrategy, hash};

/// Simple bloom filter using boolean array - fast for small datasets
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SimpleBloomFilter {
    /// Direct boolean array for simplicity
    bits: Vec<bool>,
    /// Number of hash functions (fixed at 3 for simplicity)
    num_hashes: u32,
    /// Number of elements inserted
    num_elements: usize,
}

impl SimpleBloomFilter {
    /// Create a new simple bloom filter
    pub fn new(size: usize, _config: &BloomFilterConfig) -> Self {
        Self {
            bits: vec![false; size.max(64)],
            num_hashes: 3,
            num_elements: 0,
        }
    }

    /// Create from serialized data
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize SimpleBloomFilter: {}", e))
    }
}

impl BloomFilterStrategy for SimpleBloomFilter {
    fn insert(&mut self, key: &[u8]) {
        let positions = hash::double_hash(key, self.num_hashes, self.bits.len());
        for pos in positions {
            self.bits[pos] = true;
        }
        self.num_elements += 1;
    }

    fn might_contain(&self, key: &[u8]) -> bool {
        if self.bits.is_empty() {
            return true; // Always return true for empty filter to avoid false negatives
        }
        let positions = hash::double_hash(key, self.num_hashes, self.bits.len());
        positions.iter().all(|&pos| self.bits[pos])
    }

    fn bit_count(&self) -> usize {
        self.bits.len()
    }

    fn hash_count(&self) -> usize {
        self.num_hashes as usize
    }

    fn serialize(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))
    }

    fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>() + self.bits.capacity()
    }

    fn clear(&mut self) {
        self.bits.fill(false);
        self.num_elements = 0;
    }

    fn false_positive_rate(&self) -> f64 {
        if self.num_elements == 0 {
            return 0.0;
        }

        let bits_set = self.bits.iter().filter(|&&b| b).count();
        let fill_ratio = bits_set as f64 / self.bits.len() as f64;
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
    fn test_simple_bloom() {
        let config = BloomFilterConfig::default();
        let mut filter = SimpleBloomFilter::new(1000, &config);

        // Basic functionality
        assert!(!filter.might_contain(b"test"));
        filter.insert(b"test");
        assert!(filter.might_contain(b"test"));

        // Clear functionality
        filter.clear();
        assert!(!filter.might_contain(b"test"));
        assert_eq!(filter.num_elements(), 0);
    }
}
