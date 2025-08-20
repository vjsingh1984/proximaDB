// RAPTOR Storage Engine - Row-Aligned Predicated Tensor Optimized Repository
// Combines Google Artus concepts with advanced vector database requirements

/// Magic constant for RAPTOR files (4 bytes)
pub const RAPTOR_MAGIC: [u8; 4] = *b"RPTR";

pub mod config;
pub mod engine;
pub mod rowgroup;
pub mod writer;
pub mod reader;
pub mod compaction;
pub mod hnsw_manager;
pub mod hnsw_compaction;
pub mod unified_reader;     // Consolidated reader
pub mod simd_ops;
pub mod metadata;

#[cfg(test)]
mod tests;

pub use config::RaptorConfig;
pub use engine::RaptorEngine;
pub use rowgroup::{RowGroup, RowGroupManager};
pub use writer::RaptorWriter;
pub use unified_reader::RaptorUnifiedReader;  // Export the unified reader

use anyhow::Result;