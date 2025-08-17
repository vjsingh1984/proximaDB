// RAPTOR Storage Engine - Row-Aligned Predicated Tensor Optimized Repository
// Combines Google Artus concepts with advanced vector database requirements

pub mod config;
pub mod engine;
pub mod rowgroup;
pub mod writer;
pub mod reader;
pub mod compaction;
pub mod hnsw_manager;
pub mod simd_ops;
pub mod metadata;

pub use config::RaptorConfig;
pub use engine::RaptorEngine;
pub use rowgroup::{RowGroup, RowGroupManager};
pub use writer::RaptorWriter;
pub use reader::RaptorReader;

use anyhow::Result;