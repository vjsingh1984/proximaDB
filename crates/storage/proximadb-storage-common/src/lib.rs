//! Shared storage primitives for ProximaDB storage engines and modality storage.
//!
//! Keep this crate narrow. Storage helpers belong here only when they are reused
//! by multiple storage engines or modality storage implementations.

pub mod bitmap;
pub mod storage_path;

pub use bitmap::{BitmapError, BitmapIteratorAll, RoaringBitmap};
pub use storage_path::StoragePath;
