// Document storage layer
//
// Provides document-specific storage formats and compression.

pub mod cold_tier;
pub mod collection_metadata;
pub use proximadb_document_compression::compression;
pub mod document_block;

pub use cold_tier::{
    ColdTierRetriever, DocumentMetadataFilterBuilder, StorageEngineColdTierRetriever,
};
pub use compression::DocumentCompressor;
pub use document_block::DocumentBlock;
