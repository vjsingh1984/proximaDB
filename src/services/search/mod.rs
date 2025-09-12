//! Search services
//!
//! Various search implementations including streaming and batch search

pub mod streaming;

#[cfg(test)]
pub mod comprehensive_test;

pub use streaming::{
    SearchMetadata as Metadata, SearchResultBatch as ResultBatch,
    SearchResultStream as ResultStream, StreamingSearchConfig as StreamConfig,
    StreamingSearchService as StreamingSearch, StreamingSearchStats as StreamStats,
};
