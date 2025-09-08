//! Bitmap Infrastructure for ProximaDB
//!
//! This module provides shared bitmap infrastructure that can be used across
//! all storage engines, cache layers, and query optimization components.

// Use internal bitmap implementation instead of duplicate
pub use crate::utils::bitmap::{RoaringBitmap, BitmapError};

// Additional bitmap types if needed
#[derive(Debug, Clone, Default)]
pub struct BitmapIndexStats {
    pub total_bitmaps: usize,
    pub total_bytes: usize,
    pub compression_ratio: f64,
}

// Placeholder for RoaringBitmapIndex if needed
pub type RoaringBitmapIndex = RoaringBitmap;
