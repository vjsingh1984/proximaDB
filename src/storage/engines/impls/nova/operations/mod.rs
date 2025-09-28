//! NOVA Engine Operations Module
//!
//! This module contains the core operations for the NOVA engine:
//! - Flush operations: Writing data to disk with hierarchical statistics
//! - Compaction operations: Merging and optimizing stored files
//! - Search operations: Query execution with progressive refinement

pub mod flush;
pub mod compaction;
pub mod search;

pub use flush::NovaFlushOperations;
pub use compaction::NovaCompactionOperations;
pub use search::NovaSearchOperations;