//! Shared storage primitives for ProximaDB storage engines and modality storage.
//!
//! Keep this crate narrow. Storage helpers belong here only when they are reused
//! by multiple storage engines or modality storage implementations.

pub mod bitmap;
pub mod cache_config;
pub mod engine_type;
pub mod format_conversion;
pub mod glob;
pub mod query_metrics;
pub mod storage_path;
pub mod wal_entry;

pub use bitmap::{BitmapError, BitmapIteratorAll, RoaringBitmap};
pub use cache_config::{
    AlertThresholds, CacheConfig, CoordinationConfig, EvictionPolicy, FilterCacheConfig,
    GlobalCacheConfig, IndexCacheConfig, MetadataStoreConfig, MonitoringConfig, QueryCacheConfig,
    VectorCacheConfig,
};
pub use format_conversion::{
    CompressionFormat, ConversionError, ConversionResult, ConversionStatistics, FormatConverter,
    QuantizedFormat, StorageFormat,
};
pub use engine_type::StorageEngineType;
pub use glob::{GlobError, GlobMatcher, GlobPattern, glob_match};
pub use query_metrics::{QueryStatistics, StatisticsCollector};
pub use storage_path::StoragePath;
pub use wal_entry::{
    CanonicalOperation, CanonicalWalEntry, CdcLogicalView, CdcOperation, CdcRecordEvent, EdgeRef,
    ProjectionDirective, ProjectionFreshness, ProjectionRebuilder, RecoveryResult,
    SnapshotManifest, latest_checkpoint, recover_from_canonical_wal,
};
