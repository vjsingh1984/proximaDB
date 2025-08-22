//! RAPTOR Storage Engine - Row-Aligned Predicated Tensor Optimized Repository
//! 
//! Architecture:
//! - RAPTOR builds HNSW graphs during flush/compact for locality optimization
//! - Graphs are stored as connected segments in RAPTOR format for cache efficiency
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
pub mod consolidated_reader;
pub mod consolidated_compactor;
pub mod engine;
pub mod writer;
pub mod ivf_manager;
pub mod rowgroup_manager;
pub mod smart_rowgroup_sizing;
pub mod artus_bloom;

#[cfg(test)]
mod tests;

// Re-export commonly used types from common module
pub use common::{
    RowGroup, RowGroupMetadata, VectorStats, ColumnStats, 
    MetadataColumn, MetadataValue, MetadataDataType,
    HnswGraph, HnswEdge, LocalHnswSegment, HnswGraphMetadata,
    RaptorFileMetadata, SchemaDescriptor, FieldDescriptor,
    SearchResult, Predicate, PredicateOp,
    FastLanesScheme, VectorEncoding, ColumnEncoding,
    IoStrategy, CachePolicy, ReadPattern,
    LocalityCluster, BloomFilterMetadata,
    RowGroupBloomFilter, ColumnnarIdIndex,  // Added bloom filter types
};

// Export consolidated modules instead of deprecated ones
pub use config::{RaptorConfig, CompactionConfig};
pub use engine::RaptorEngine;
pub use writer::RaptorWriter;
pub use consolidated_reader::RaptorReader;      // Use consolidated reader
pub use consolidated_compactor::RaptorCompactor; // Use consolidated compactor
pub use ivf_manager::IvfManager;
pub use rowgroup_manager::{RowGroupManager, HybridRowGroup, ColumnarBlock, TransposedVectors};
pub use smart_rowgroup_sizing::{SmartRowGroupSizer, OptimalRowGroupSize, CloudIOProfile, CommonConfigurations};