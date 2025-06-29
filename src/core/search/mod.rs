//! Search module for ProximaDB storage-aware search implementations

pub mod storage_aware;
pub mod viper_search;
pub mod lsm_search;

// Re-export main types
pub use storage_aware::{
    StorageSearchEngine, SearchHints, SearchCapabilities, ClusteringHints,
    QuantizationLevel, SearchMetrics, SearchValidator, SearchEngineFactory
};