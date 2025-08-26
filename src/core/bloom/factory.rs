/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Bloom filter factory for creating appropriate implementations

use anyhow::Result;
use std::sync::Arc;

use super::{
    BloomFilterStrategy, BloomFilterConfig, BloomStrategy, SerializedBloomFilter
};
use super::strategies::{
    BitPackedBloomFilter, ByteAlignedBloomFilter, SimpleBloomFilter, CompositeBloomFilter
};

/// Factory for creating bloom filter instances
pub struct BloomFilterFactory;

impl BloomFilterFactory {
    /// Create a bloom filter based on configuration
    pub fn create(config: &BloomFilterConfig) -> Box<dyn BloomFilterStrategy> {
        if !config.enabled || config.expected_items == 0 {
            return Box::new(NoOpBloomFilter);
        }
        
        // Default to ByteAligned strategy for general use
        Box::new(ByteAlignedBloomFilter::new(config.expected_items, config))
    }
    
    /// Create a bloom filter for SSTable usage
    pub fn for_sstable(expected_items: usize) -> Box<dyn BloomFilterStrategy> {
        let config = BloomFilterConfig::for_sstable(expected_items);
        Self::create(&config)
    }
    
    /// Create a bloom filter for memtable usage
    pub fn for_memtable(expected_items: usize) -> Box<dyn BloomFilterStrategy> {
        let config = BloomFilterConfig::for_memtable(expected_items);
        Self::create(&config)
    }
    
    /// Create from serialized data
    pub fn from_serialized(serialized: &SerializedBloomFilter) -> Result<Box<dyn BloomFilterStrategy>> {
        if serialized.version != SerializedBloomFilter::CURRENT_VERSION {
            return Err(anyhow::anyhow!(
                "Unsupported bloom filter version: {}",
                serialized.version
            ));
        }
        
        let filter: Box<dyn BloomFilterStrategy> = match serialized.strategy_type {
            BloomStrategy::BitPacked => {
                Box::new(BitPackedBloomFilter::from_bytes(&serialized.data)?)
            }
            BloomStrategy::ByteAligned => {
                Box::new(ByteAlignedBloomFilter::from_bytes(&serialized.data)?)
            }
            BloomStrategy::Simple => {
                Box::new(SimpleBloomFilter::from_bytes(&serialized.data)?)
            }
            BloomStrategy::Composite => {
                Box::new(CompositeBloomFilter::from_bytes(&serialized.data)?)
            }
        };
        
        Ok(filter)
    }
    
    /// Create a thread-safe bloom filter
    pub fn create_concurrent(config: &BloomFilterConfig) -> Arc<dyn BloomFilterStrategy> {
        Arc::from(Self::create(config))
    }
}

/// No-op bloom filter for when bloom filters are disabled
#[derive(Debug)]
struct NoOpBloomFilter;

impl BloomFilterStrategy for NoOpBloomFilter {
    fn insert(&mut self, _key: &[u8]) {}
    
    fn might_contain(&self, _key: &[u8]) -> bool {
        true // Always return true to avoid false negatives
    }
    
    fn bit_count(&self) -> usize {
        0
    }
    
    fn hash_count(&self) -> usize {
        0
    }
    
    fn serialize(&self) -> Result<Vec<u8>> {
        Ok(vec![])
    }
    
    fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    
    fn clear(&mut self) {}
    
    fn false_positive_rate(&self) -> f64 {
        1.0
    }
    
    fn num_elements(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_factory_creation() {
        // Test each strategy
        let strategies = vec![
            BloomStrategy::BitPacked,
            BloomStrategy::ByteAligned,
            BloomStrategy::Simple,
            BloomStrategy::Composite,
        ];
        
        for strategy in strategies {
            let config = BloomFilterConfig {
                strategy,
                expected_items: 100,
                ..Default::default()
            };
            
            let filter = BloomFilterFactory::create(&config);
            assert_eq!(filter.num_elements(), 0);
        }
    }
    
    #[test]
    fn test_specialized_factories() {
        let sstable_filter = BloomFilterFactory::for_sstable(1000);
        assert!(sstable_filter.bit_count() > 0);
        
        let memtable_filter = BloomFilterFactory::for_memtable(500);
        assert!(memtable_filter.bit_count() > 0);
    }
    
    #[test]
    fn test_serialization_roundtrip() {
        let config = BloomFilterConfig::default();
        let mut filter = BloomFilterFactory::create(&config);
        
        // Add some data
        filter.insert(b"test1");
        filter.insert(b"test2");
        
        // Serialize
        let serialized = SerializedBloomFilter::from_filter(filter.as_ref(), config.clone()).unwrap();
        
        // Deserialize
        let restored = BloomFilterFactory::from_serialized(&serialized).unwrap();
        
        // Verify
        assert!(restored.might_contain(b"test1"));
        assert!(restored.might_contain(b"test2"));
        assert!(!restored.might_contain(b"test3"));
    }
}