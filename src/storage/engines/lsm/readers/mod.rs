//! LSM Reader implementations
//!
//! This module contains optimized readers for LSM storage:
//! - UnifiedSstableReader: Main reader with strategy selection
//! - Block-level caching and optimization
//! - Metadata bloom filters for efficient filtering

pub mod unified_sstable_reader;

// Test modules
#[cfg(test)]
pub mod tests;

pub use unified_sstable_reader::{
    UnifiedSstableReader,
    SstableReadingStrategy,
    BlockCache,
    IndexCache,
    CollectionContext,
};

