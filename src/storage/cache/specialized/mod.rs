//! Specialized cache implementations for different data types
//!
//! This module provides optimized cache implementations for specific
//! data types used throughout ProximaDB:
//!
//! - **VectorCache**: High-performance caching for vector data with SIMD support
//! - **QueryCache**: Result caching for repeated queries
//! - **MetadataStore**: Collection and index metadata caching
//! - **IndexNodeCache**: Graph and tree node caching
//! - **BitmapFilterCache**: Bloom filter and bitmap caching
//! - **FilesystemMetadataStore**: Filesystem metadata caching

pub mod bitmap_filter_cache;
pub mod filesystem_metadata_store;
pub mod index_node_cache;
pub mod metadata_store;
pub mod query_cache;
pub mod vector_cache;

// Re-export specialized cache types
pub use bitmap_filter_cache::BitmapFilterCache;
pub use filesystem_metadata_store::{FilesystemMetadata, FilesystemMetadataStore};
pub use index_node_cache::IndexNodeCache;
pub use metadata_store::MetadataStore;
pub use query_cache::QueryCache;
pub use vector_cache::{CachedVector, VectorCache, VectorCacheKey};
