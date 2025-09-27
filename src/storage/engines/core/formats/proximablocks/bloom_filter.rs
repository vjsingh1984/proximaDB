//! Row-Based Bloom Filter Compatibility Module
//!
//! This module provides compatibility re-exports for the unified bloom filter
//! implementation in core::bloom. All bloom filter functionality has been
//! moved to the core module for code reuse across storage engines.

// Re-export all bloom filter types from core module
pub use crate::core::bloom::{
    BloomFilterBuilder, BloomFilterConfig, BloomFilterStats, BloomFilterStrategy, BloomStrategy,
    HashAlgorithm, HierarchicalBloomConfig, SerializedBloomFilter, SerializedSstableBloomFilter,
    SstableBloomFilter, hash, json_to_metadata_item, serialize_metadata_value,
};

// Re-export factory
pub use crate::core::bloom::factory;

// Re-export strategies
pub use crate::core::bloom::strategies;

// Type aliases for compatibility
pub type BloomFilter = crate::core::bloom::BloomFilter;
pub type CompositeBloomFilter = crate::core::bloom::CompositeBloomFilter;
