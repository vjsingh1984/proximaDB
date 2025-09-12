//! Indexing Data Structures for ProximaDB
//!
//! This module provides high-performance indexing data structures optimized
//! for different storage engines and search patterns.

// bloom_filter module removed - use core::bloom module for unified polymorphic design
// roaring_bitmap module moved to storage::common::bitmap for better shared access

// Re-export roaring bitmap types from their new location for backward compatibility
pub use crate::storage::common::bitmap::{BitmapIndexStats, RoaringBitmapIndex};
