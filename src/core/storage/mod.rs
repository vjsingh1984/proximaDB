//! Storage Types for ProximaDB Unified Schema
//!
//! This module provides all storage-related types including compression,
//! engines, configuration, and compaction settings.

pub mod compression;
pub mod engines;
pub mod configuration;
pub mod compaction;

// Re-export all storage types
pub use compression::*;
pub use engines::*;
pub use configuration::*;
pub use compaction::*;