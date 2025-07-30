/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Polymorphic Bloom Filter Design with Strategy Pattern
//!
//! This module provides a unified bloom filter architecture that consolidates
//! multiple implementations into a single, extensible design using the strategy pattern.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod strategies;
pub mod factory;

/// Core trait for all bloom filter implementations
pub trait BloomFilterStrategy: Send + Sync {
    /// Insert a key into the bloom filter
    fn insert(&mut self, key: &[u8]);
    
    /// Check if a key might exist in the bloom filter
    fn might_contain(&self, key: &[u8]) -> bool;
    
    /// Get the number of bits used by the filter
    fn bit_count(&self) -> usize;
    
    /// Get the number of hash functions used
    fn hash_count(&self) -> u32;
    
    /// Serialize the bloom filter to bytes
    fn serialize(&self) -> Result<Vec<u8>>;
    
    /// Get memory usage in bytes
    fn memory_usage(&self) -> usize;
    
    /// Clear all bits in the filter
    fn clear(&mut self);
    
    /// Get the false positive rate (estimated)
    fn false_positive_rate(&self) -> f64;
    
    /// Get the number of elements inserted
    fn num_elements(&self) -> usize;
}

/// Trait for metadata-aware bloom filters with type-safe operations
pub trait MetadataBloomFilter: BloomFilterStrategy {
    /// Insert a metadata item with proper type handling
    fn insert_metadata(&mut self, column: &str, item: &crate::proto::proximadb::MetadataItem);
    
    /// Check if metadata might match with proper type handling
    fn might_match_metadata(&self, column: &str, item: &crate::proto::proximadb::MetadataItem) -> bool;
    
    /// Get the number of metadata columns tracked
    fn num_columns(&self) -> usize;
}

/// Serialize metadata value for bloom filter hashing
/// This ensures consistent serialization across all types
pub fn serialize_metadata_value(item: &crate::proto::proximadb::MetadataItem) -> String {
    use crate::proto::proximadb::metadata_item::Value;
    
    match &item.value {
        Some(Value::StringValue(s)) => s.clone(),
        Some(Value::NumberValue(n)) => {
            // Use consistent number formatting to avoid precision issues
            // For integers, format without decimal point
            if n.fract() == 0.0 && n.is_finite() {
                format!("{:.0}", n)
            } else {
                // Use scientific notation for consistent representation
                format!("{:e}", n)
            }
        }
        Some(Value::BoolValue(b)) => b.to_string(),
        None => String::new(),
    }
}

/// Convert JSON value to MetadataItem for bloom filter operations
pub fn json_to_metadata_item(key: &str, value: &serde_json::Value) -> crate::proto::proximadb::MetadataItem {
    use crate::proto::proximadb::metadata_item::Value as ProtoValue;
    
    let proto_value = match value {
        serde_json::Value::String(s) => Some(ProtoValue::StringValue(s.clone())),
        serde_json::Value::Number(n) => {
            Some(ProtoValue::NumberValue(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::Bool(b) => Some(ProtoValue::BoolValue(*b)),
        _ => None, // Null, Array, Object not supported in MetadataItem
    };
    
    crate::proto::proximadb::MetadataItem {
        key: key.to_string(),
        value: proto_value,
    }
}

/// Bloom filter strategy types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BloomStrategy {
    /// Memory efficient, bit-packed storage (best for in-memory operations)
    BitPacked,
    /// Byte-aligned storage (best for disk persistence)
    ByteAligned,
    /// Simple boolean array (fast for small datasets)
    Simple,
    /// Composite filter combining multiple strategies
    Composite,
}

/// Hash algorithm selection
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum HashAlgorithm {
    /// MurmurHash3 - fast and good distribution
    Murmur3,
    /// xxHash - extremely fast
    XXHash,
    /// CityHash - good for short keys
    CityHash,
}

impl Default for HashAlgorithm {
    fn default() -> Self {
        Self::Murmur3
    }
}

/// Enhanced bloom filter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterConfig {
    /// Strategy to use
    pub strategy: BloomStrategy,
    
    /// Bits per key (for ByteAligned/BitPacked)
    pub bits_per_key: u32,
    
    /// Target false positive rate (alternative to bits_per_key)
    pub false_positive_rate: Option<f64>,
    
    /// Expected number of items
    pub expected_items: usize,
    
    /// Enable bloom filter
    pub enabled: bool,
    
    /// Hash function selection
    pub hash_algorithm: HashAlgorithm,
}

impl Default for BloomFilterConfig {
    fn default() -> Self {
        Self {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 10,
            false_positive_rate: None,
            expected_items: 10000,
            enabled: true,
            hash_algorithm: HashAlgorithm::default(),
        }
    }
}

impl BloomFilterConfig {
    /// Create config for SSTable usage
    pub fn for_sstable(expected_items: usize) -> Self {
        Self {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 10,
            expected_items,
            ..Default::default()
        }
    }
    
    /// Create config for memtable usage
    pub fn for_memtable(expected_items: usize) -> Self {
        Self {
            strategy: BloomStrategy::BitPacked,
            bits_per_key: 8,
            expected_items,
            ..Default::default()
        }
    }
    
    /// Calculate bits per key from false positive rate
    pub fn bits_from_fpr(false_positive_rate: f64) -> u32 {
        let ln2_squared = 0.4804530139182014;
        let bits = (-(false_positive_rate.ln() / ln2_squared)) as u32;
        bits.max(1).min(32)
    }
}

/// Serialized bloom filter with type information
#[derive(Serialize, Deserialize)]
pub struct SerializedBloomFilter {
    /// Type tag for polymorphic deserialization
    pub strategy_type: BloomStrategy,
    
    /// Version for forward compatibility
    pub version: u32,
    
    /// Configuration used to create the filter
    pub config: BloomFilterConfig,
    
    /// Serialized filter data
    pub data: Vec<u8>,
    
    /// Metadata about the filter
    pub metadata: HashMap<String, serde_json::Value>,
}

impl SerializedBloomFilter {
    /// Current serialization version
    pub const CURRENT_VERSION: u32 = 1;
    
    /// Create from a bloom filter instance
    pub fn from_filter(
        filter: &dyn BloomFilterStrategy,
        config: BloomFilterConfig,
    ) -> Result<Self> {
        let mut metadata = HashMap::new();
        metadata.insert(
            "num_elements".to_string(),
            serde_json::Value::Number(filter.num_elements().into()),
        );
        metadata.insert(
            "false_positive_rate".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(filter.false_positive_rate())
                    .unwrap_or(serde_json::Number::from(0)),
            ),
        );
        
        Ok(Self {
            strategy_type: config.strategy,
            version: Self::CURRENT_VERSION,
            config,
            data: filter.serialize()?,
            metadata,
        })
    }
}

/// Hash functions for bloom filters
pub mod hash {
    use std::hash::{Hash, Hasher};
    
    /// MurmurHash3 32-bit implementation
    pub fn murmur3_32(key: &[u8], seed: u32) -> u32 {
        let mut h = seed;
        let mut chunks = key.chunks_exact(4);
        
        // Process 4-byte chunks
        for chunk in &mut chunks {
            let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            k = k.wrapping_mul(0xcc9e2d51);
            k = k.rotate_left(15);
            k = k.wrapping_mul(0x1b873593);
            
            h ^= k;
            h = h.rotate_left(13);
            h = h.wrapping_mul(5).wrapping_add(0xe6546b64);
        }
        
        // Process remaining bytes
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            let mut k = 0u32;
            for (i, &byte) in remainder.iter().enumerate() {
                k |= (byte as u32) << (i * 8);
            }
            
            k = k.wrapping_mul(0xcc9e2d51);
            k = k.rotate_left(15);
            k = k.wrapping_mul(0x1b873593);
            h ^= k;
        }
        
        // Finalization
        h ^= key.len() as u32;
        h ^= h >> 16;
        h = h.wrapping_mul(0x85ebca6b);
        h ^= h >> 13;
        h = h.wrapping_mul(0xc2b2ae35);
        h ^= h >> 16;
        
        h
    }
    
    /// Simple hash function for testing
    pub fn simple_hash(key: &[u8], seed: u32) -> u32 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        key.hash(&mut hasher);
        hasher.finish() as u32
    }
    
    /// Double hashing to generate multiple hash values
    pub fn double_hash(key: &[u8], num_hashes: u32, bit_count: usize) -> Vec<usize> {
        if bit_count == 0 {
            return vec![0; num_hashes as usize]; // Return zeros for empty filter
        }
        
        let h1 = murmur3_32(key, 0);
        let h2 = murmur3_32(key, h1);
        
        (0..num_hashes)
            .map(|i| {
                let hash = h1.wrapping_add(i.wrapping_mul(h2));
                (hash as usize) % bit_count
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bits_from_fpr() {
        // Test conversion from false positive rate to bits per key
        assert_eq!(BloomFilterConfig::bits_from_fpr(0.01), 9); // ~1% FPR needs ~9 bits
        assert_eq!(BloomFilterConfig::bits_from_fpr(0.001), 14); // ~0.1% FPR needs ~14 bits
        assert_eq!(BloomFilterConfig::bits_from_fpr(0.1), 4); // ~10% FPR needs ~4 bits
    }
    
    #[test]
    fn test_murmur_hash() {
        let key = b"test_key";
        let hash1 = hash::murmur3_32(key, 0);
        let hash2 = hash::murmur3_32(key, 1);
        
        // Different seeds should produce different hashes
        assert_ne!(hash1, hash2);
        
        // Same input should produce same hash
        assert_eq!(hash::murmur3_32(key, 0), hash1);
    }
}