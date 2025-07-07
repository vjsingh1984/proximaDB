//! Search module for ProximaDB storage-aware search implementations

pub mod lsm_search;
pub mod multi_tier_deduplication;
pub mod storage_aware;
pub mod viper_search;

// Re-export main types
pub use multi_tier_deduplication::{
    DeduplicationStats, MultiTierDeduplicator, StorageTier, TieredSearchResult, 
    DeduplicationStorageEngine, MetadataFilter,
};
pub use storage_aware::{
    ClusteringHints, QuantizationLevel, SearchCapabilities, SearchEngineFactory, SearchHints,
    SearchMetrics, SearchValidator, StorageSearchEngine,
};
