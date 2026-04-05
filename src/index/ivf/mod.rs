//! IVF (Inverted File) Index Module
//!
//! This module provides filtered search capabilities for IVF indexes,
//! implementing filter-aware inverted list search for efficient hybrid search.

pub mod filtered;

// Re-export filtered search types
pub use filtered::{
    FilteredIVFIndex, IVFFilteredResult, IVFFilteredSearchParams, IVFIndexStats,
    IVFInvertedList, IVFVector,
};
