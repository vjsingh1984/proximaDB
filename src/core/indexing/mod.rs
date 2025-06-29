//! Indexing Data Structures for ProximaDB
//!
//! This module provides high-performance indexing data structures optimized
//! for different storage engines and search patterns.

pub mod bloom_filter;
pub mod roaring_bitmap;

// Re-export main types for convenience
pub use bloom_filter::{BloomFilter, BloomFilterCollection, BloomFilterStats};
pub use roaring_bitmap::{RoaringBitmapIndex, BitmapIndexStats};