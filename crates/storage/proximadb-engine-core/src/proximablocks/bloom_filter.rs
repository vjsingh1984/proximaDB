//! Row-Based Bloom Filter Compatibility Module
//!
//! This module provides compatibility re-exports for the unified bloom filter
//! implementation in core::bloom. All bloom filter functionality has been
//! moved to the core module for code reuse across storage engines.

// Re-export all bloom filter types from core module
pub use proximadb_bloom::{
    BloomFilterBuilder, BloomFilterConfig, BloomFilterStats, BloomFilterStrategy, BloomStrategy,
    HashAlgorithm, HierarchicalBloomConfig, SerializedBloomFilter, SerializedSstableBloomFilter,
    SstableBloomFilter, hash, json_to_metadata_item, serialize_metadata_value,
};

// Re-export factory
pub use proximadb_bloom::factory;

// Re-export strategies
pub use proximadb_bloom::strategies;

// Type aliases for compatibility
pub type BloomFilter = proximadb_bloom::BloomFilter;
pub type CompositeBloomFilter = proximadb_bloom::CompositeBloomFilter;
