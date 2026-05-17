//! Shared storage primitives for ProximaDB storage engines and modality storage.
//!
//! Keep this crate narrow. Storage helpers belong here only when they are reused
//! by multiple storage engines or modality storage implementations.

pub mod bitmap;
pub mod cache_config;
pub mod collection_path;
pub mod column_projector;
pub mod columnar_constants;
pub mod engine_constants;
pub mod engine_profile;
pub mod engine_type;
pub mod flush_integration;
pub mod format_conversion;
pub mod format_traits;
pub mod glob;
pub mod hilbert_curve;
pub mod id_index;
pub mod metadata_collector;
pub mod native_metadata;
pub mod observability_rollups;
pub mod query_metrics;
pub mod storage_error;
pub mod storage_path;
pub mod transaction_isolation;
pub mod two_phase_commit;
pub mod wal_entry;
pub mod writer_statistics;

pub use bitmap::{BitmapError, BitmapIteratorAll, RoaringBitmap};
pub use cache_config::{
    AlertThresholds, CacheConfig, CoordinationConfig, EvictionPolicy, FilterCacheConfig,
    GlobalCacheConfig, IndexCacheConfig, MetadataStoreConfig, MonitoringConfig, QueryCacheConfig,
    VectorCacheConfig,
};
pub use collection_path::slug_for;
pub use column_projector::{ColumnProjection, ProjectionBuilder};
pub use columnar_constants::{
    DEFAULT_PAGE_SIZE, DEFAULT_ROW_GROUP_SIZE, DEFAULT_WRITE_BATCH_SIZE, FIELD_COLLECTION_ID,
    FIELD_EXPIRES_AT, FIELD_EXTRA_META, FIELD_ID, FIELD_IS_DELETED, FIELD_Q_BINARY, FIELD_Q_INT8,
    FIELD_Q_PQ4, FIELD_Q_PQ8, FIELD_Q_PQ16, FIELD_Q_PQ32, FIELD_QP_BINARY_THRESHOLD,
    FIELD_QP_INT8_MAX, FIELD_QP_INT8_MIN, FIELD_QP_INT8_SCALE, FIELD_QP_PQ_CENTROIDS,
    FIELD_QP_PQ_SUBQUANTIZERS, FIELD_ROW_GROUP_OFFSET, FIELD_ROW_INDEX, FIELD_SCHEMA_VERSION,
    FIELD_SOURCE, FIELD_TIMESTAMP, FIELD_UPDATED_AT, FIELD_VECTOR_FP32, FIELD_VERSION,
    PARQUET_EXTENSION, QUANTIZATION_COLUMNS, QUANTIZATION_PARAMETER_COLUMNS,
    QUANTIZED_VECTOR_COLUMNS, REQUIRED_COLUMNS, TEMPORAL_COLUMNS, VIPER_FILE_EXTENSION,
};
pub use engine_constants::{
    BLOOM_FILTER_EXT, DEFAULT_BLOCK_METADATA_OVERHEAD_BYTES, DEFAULT_TARGET_BLOCK_SIZE_BYTES,
    ENGINE_COLUMNAR, ENGINE_HELIX, ENGINE_NOVA, ENGINE_RAPTOR, ENGINE_SST, ENGINE_SWIFT,
    ENGINE_VIPER, HELIX_FILE_EXT, HELIX_MAGIC, INDEX_EXT, MAX_TARGET_BLOCK_SIZE_BYTES,
    METADATA_EXT, MIN_TARGET_BLOCK_SIZE_BYTES, NOVA_FILE_EXT, NOVA_MAGIC, PRISM_FILE_EXT,
    RAPTOR_FILE_EXT, RAPTOR_MAGIC, SST_FILE_EXT, SST_MAGIC, STATS_EXT, SWIFT_FILE_EXT, SWIFT_MAGIC,
    VIPER_FILE_EXT, VIPER_MAGIC,
};
pub use engine_profile::EngineProfile;
pub use engine_type::StorageEngineType;
pub use flush_integration::{FlushConfig, FlushIntegration, FlushStats};
pub use format_conversion::{
    CompressionFormat, ConversionError, ConversionResult, ConversionStatistics, FormatConverter,
    QuantizedFormat, StorageFormat,
};
pub use format_traits::{
    ColumnStats, CompactionContext, CompactionResult, ComparisonOp, DefaultFormatDetector,
    FileEntry, FileStats, FilterExpression as FormatFilterExpression, FormatDetector,
    FormatStatistics, FormatType, InternalFormat, MergeAction, OpenTableFormat, OptimizeContext,
    OptimizeResult, ReadContext, RecordBatchStream, Snapshot, StorageFormat as StorageFormatTrait,
    VectorBatch, VectorBatchStream, VectorReadContext, VectorWriteContext, WriteContext, WriteMode,
    WriteResult,
};
pub use glob::{GlobError, GlobMatcher, GlobPattern, glob_match};
pub use hilbert_curve::{HilbertCurve, HilbertStats, HilbertUtils};
pub use id_index::{
    BloomFilter, ColumnarIdIndex, IndexStats, PageIdIndex, ParquetLocation, RowGroupIdIndex,
};
pub use metadata_collector::{MetadataCollectionConfig, MetadataCollector, NoOpCollector};
pub use native_metadata::{
    FieldStatistics, MetadataFieldType, NativeMetadataHandler, NativeMetadataQueryOptimizer,
    NativeMetadataStats, NativePredicate, OptimizedFilter, PredicateOperator,
};
pub use observability_rollups::{
    AggregationFunction, RollupConfig, RollupInterval, RollupManager, RollupView,
};
pub use query_metrics::{QueryStatistics, StatisticsCollector};
pub use storage_error::{ErrorContext, StorageError, StorageErrorKind};
pub use storage_path::StoragePath;
pub use transaction_isolation::{IsolationLevel, IsolationManager, ReadSnapshot, WriteSet};
pub use two_phase_commit::{
    CommitResult, ParticipantState, ParticipantType, PrepareResult, TransactionState,
    TwoPhaseCommitConfig, TwoPhaseCommitProtocol, TwoPhaseCommitStats, TwoPhaseParticipant,
    TwoPhaseTransaction,
};
pub use wal_entry::{
    CanonicalOperation, CanonicalWalEntry, CdcLogicalView, CdcOperation, CdcRecordEvent, EdgeRef,
    ProjectionDirective, ProjectionFreshness, ProjectionRebuilder, RecoveryResult,
    SnapshotManifest, latest_checkpoint, recover_from_canonical_wal,
};
pub use writer_statistics::{AggregatedBatchStats, BatchWriteStats, StreamingParquetWriterStats};
