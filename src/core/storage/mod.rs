//! Storage Types for ProximaDB Unified Schema
//!
//! This module provides all storage-related types including compression,
//! engines, configuration, and compaction settings.

pub mod compaction;
pub mod configuration;
pub mod engines;

// Re-export all storage types
pub use compaction::*;
pub use configuration::*;
pub use engines::*;
pub use proximadb_compression_types::{CompressionAlgorithm, CompressionConfig};
