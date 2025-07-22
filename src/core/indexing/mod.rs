//! Indexing Data Structures for ProximaDB
//!
//! This module provides high-performance indexing data structures optimized
//! for different storage engines and search patterns.

// bloom_filter module removed - use core::bloom module for unified polymorphic design
pub mod roaring_bitmap;

// Re-export main types for convenience
pub use roaring_bitmap::{BitmapIndexStats, RoaringBitmapIndex};
