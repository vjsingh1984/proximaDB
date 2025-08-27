//! Search services
//! 
//! Various search implementations including streaming and batch search

pub mod streaming;

#[cfg(test)]
pub mod comprehensive_test;

pub use streaming::{
    StreamingSearchService as StreamingSearch,
    StreamingSearchConfig as StreamConfig,
    StreamingSearchStats as StreamStats,
    SearchResultStream as ResultStream,
    SearchResultBatch as ResultBatch,
    SearchMetadata as Metadata,
};