//! RAPTOR Storage Engine - Row-Aligned Predicated Tensor Optimized Repository
//!
//! Architecture:
//! - RAPTOR uses Matrix Trinity (P² + K² + P×K) for navigation instead of HNSW
//! - Matrices are stored for O(1) centroid lookup and fast intra-rowgroup search
//! - Smart parameter selection based on vector count and dimension for optimal recall
//! - AXIS can still override or enhance indexes via EventLog events
//! - Collections without index configs skip AXIS processing entirely
//!
//! This dual approach provides:
//! 1. Fast search from pre-built graphs in storage
//! 2. Flexibility for AXIS to add other index types
//! 3. Memory efficiency when indexes aren't needed

/// Magic constant for RAPTOR files (4 bytes)
pub const RAPTOR_MAGIC: [u8; 4] = *b"RPTR";

// Common types module - MUST be first to avoid circular dependencies
pub mod common;
pub mod config;
pub mod constants;

// Core modules
pub mod adaptive_pxk;
pub mod consolidated_compactor;
pub mod consolidated_reader;
pub mod engine;
pub mod matrix_builder;
pub mod metadata_serializer;
pub mod writer;
// ivf_manager removed - obsolete with Matrix Trinity (P² + K² + P×K)
pub mod artus_bloom;
pub mod rowgroup_manager;
pub mod smart_rowgroup_sizing;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod p2_matrix_tests;

#[cfg(test)]
mod boundary_spillover_tests;

// Re-export commonly used types from common module
pub use common::{
    BloomFilterMetadata,
    BoostingStrategy, // Added boundary/correction types
    CachePolicy,
    ColumnEncoding,
    ColumnPageMetadata,
    ColumnStats,
    ColumnType,       // Added columnar page types
    ColumnnarIdIndex, // Added bloom filter types
    ConfidenceAssessment,
    ConfidenceSignals,
    CorrectionStrategy,
    FastLanesScheme,
    FieldDescriptor,
    IoStrategy,
    K2Matrix,
    LocalityCluster,
    MetadataColumn,
    MetadataData,
    MetadataValue,
    P2Matrix,
    Predicate,
    PredicateOp,
    RaptorFileMetadata,
    ReadPattern,
    RowGroup,
    RowGroupBloomFilter,
    RowGroupMetadata,
    SchemaDescriptor,
    SearchResult,
    SpilloverInfo,
    VectorCentroidMatrix, // Matrix Trinity architecture
    VectorEncoding,
    VectorStats,
};

// Export consolidated modules instead of deprecated ones
pub use config::{AccuracyLevel, CompactionConfig, CompressionStrategy, PxKStrategy, RaptorConfig};
pub use consolidated_compactor::RaptorCompactor;
pub use consolidated_reader::RaptorReader; // Use consolidated reader
pub use engine::RaptorEngine;
pub use writer::RaptorWriter; // Use consolidated compactor
// IvfManager removed - Matrix Trinity handles clustering via centroids
pub use adaptive_pxk::{AdaptivePxKStorage, BoundaryInfo, SelectionReason, VectorSelection};
pub use common::{
    ColumnarBlock, FastLanesEncodedData, MetadataColumns, QuantizationParams,
    QuantizedColumnarData, TransposedVectors,
};
pub use rowgroup_manager::RowGroups;
pub use smart_rowgroup_sizing::{
    CloudIOProfile, CommonConfigurations, OptimalRowGroupSize, SmartRowGroupSizer,
};
