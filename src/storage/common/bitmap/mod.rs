//! Bitmap Infrastructure for ProximaDB
//!
//! This module provides shared bitmap infrastructure that can be used across
//! all storage engines, cache layers, and query optimization components.

// Use internal bitmap implementation instead of duplicate
pub use crate::utils::bitmap::{BitmapError, RoaringBitmap};

/// Statistics for bitmap index usage and performance
///
/// Tracks memory usage and compression effectiveness of bitmap
/// indexes across the storage system.
#[derive(Debug, Clone, Default)]
pub struct BitmapIndexStats {
    /// Total number of bitmap indexes currently maintained
    pub total_bitmaps: usize,
    /// Total memory consumption in bytes across all bitmaps
    pub total_bytes: usize,
    /// Compression ratio achieved (compressed_size / uncompressed_size)
    pub compression_ratio: f64,
}

// Placeholder for RoaringBitmapIndex if needed
pub type RoaringBitmapIndex = RoaringBitmap;
