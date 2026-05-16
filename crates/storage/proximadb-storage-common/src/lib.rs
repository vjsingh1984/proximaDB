//! Shared storage primitives for ProximaDB storage engines and modality storage.
//!
//! Keep this crate narrow. Storage helpers belong here only when they are reused
//! by multiple storage engines or modality storage implementations.

pub mod bitmap;
pub mod cache_config;
pub mod engine_constants;
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
pub use engine_constants::{
    BLOOM_FILTER_EXT, ENGINE_COLUMNAR, ENGINE_HELIX, ENGINE_NOVA, ENGINE_RAPTOR, ENGINE_SST,
    ENGINE_SWIFT, ENGINE_VIPER, HELIX_FILE_EXT, HELIX_MAGIC, INDEX_EXT, METADATA_EXT,
    NOVA_FILE_EXT, NOVA_MAGIC, PRISM_FILE_EXT, RAPTOR_FILE_EXT, RAPTOR_MAGIC, SST_FILE_EXT,
    SST_MAGIC, STATS_EXT, SWIFT_FILE_EXT, SWIFT_MAGIC, VIPER_FILE_EXT, VIPER_MAGIC,
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
