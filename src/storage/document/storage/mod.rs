// Document storage layer
//
// Provides document-specific storage formats and compression.

pub mod compression;
pub mod document_block;

pub use compression::DocumentCompressor;
pub use document_block::DocumentBlock;
