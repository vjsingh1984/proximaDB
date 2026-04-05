//! HNSW (Hierarchical Navigable Small World) Index Module
//!
//! This module provides filtered search capabilities for HNSW indexes,
//! implementing filter-aware graph traversal for efficient hybrid search.

pub mod filtered;

// Re-export filtered search types
pub use filtered::{
    FilteredHNSWIndex, HNSWFilteredResult, HNSWFilteredSearchParams, HNSWIndexStats,
    HNSWConnection, HNSWNode,
};
