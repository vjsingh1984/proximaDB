//! Bitmap Infrastructure for ProximaDB
//!
//! This module provides shared bitmap infrastructure that can be used across
//! all storage engines, cache layers, and query optimization components.

pub mod roaring_bitmap;

// Re-export main types for convenient access
pub use roaring_bitmap::{BitmapIndexStats, RoaringBitmap, RoaringBitmapIndex};