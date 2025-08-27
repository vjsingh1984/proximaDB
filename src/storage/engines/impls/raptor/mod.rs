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
pub mod consolidated_reader;
pub mod consolidated_compactor;
pub mod engine;
pub mod matrix_builder;
pub mod metadata_serializer;
pub mod writer;
// ivf_manager removed - obsolete with Matrix Trinity (P² + K² + P×K)
pub mod rowgroup_manager;
pub mod smart_rowgroup_sizing;
pub mod artus_bloom;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod p2_matrix_tests;

#[cfg(test)]
mod boundary_spillover_tests;

// Re-export commonly used types from common module
pub use common::{
    RowGroup, RowGroupMetadata, VectorStats, ColumnStats, 
    MetadataColumn, MetadataValue, MetadataData,
    RaptorFileMetadata, SchemaDescriptor, FieldDescriptor,
    SearchResult, Predicate, PredicateOp,
    FastLanesScheme, VectorEncoding, ColumnEncoding,
    IoStrategy, CachePolicy, ReadPattern,
    LocalityCluster, BloomFilterMetadata,
    RowGroupBloomFilter, ColumnnarIdIndex,  // Added bloom filter types
    SpilloverInfo, ConfidenceAssessment, ConfidenceSignals,
    CorrectionStrategy, BoostingStrategy,  // Added boundary/correction types
    P2Matrix, K2Matrix, VectorCentroidMatrix,  // Matrix Trinity architecture
    ColumnPageMetadata, ColumnType,  // Added columnar page types
};

// Export consolidated modules instead of deprecated ones
pub use config::{RaptorConfig, CompactionConfig, AccuracyLevel, PxKStrategy, CompressionStrategy};
pub use engine::RaptorEngine;
pub use writer::RaptorWriter;
pub use consolidated_reader::RaptorReader;      // Use consolidated reader
pub use consolidated_compactor::RaptorCompactor; // Use consolidated compactor
// IvfManager removed - Matrix Trinity handles clustering via centroids
pub use rowgroup_manager::RowGroups;
pub use common::{ColumnarBlock, TransposedVectors, FastLanesEncodedData, 
                 QuantizedColumnarData, QuantizationParams, MetadataColumns};
pub use smart_rowgroup_sizing::{SmartRowGroupSizer, OptimalRowGroupSize, CloudIOProfile, CommonConfigurations};
pub use adaptive_pxk::{AdaptivePxKStorage, VectorSelection, SelectionReason, BoundaryInfo};