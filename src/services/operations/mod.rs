//! Vector and data operations
//!
//! Core operations on vectors including search, insert, update, delete

pub mod vectors;
pub mod batch_result;

#[cfg(test)]
pub mod vectors_test;

pub use vectors::{UnifiedSearchConfig as SearchConfig, VectorOperationsService as VectorOps};
pub use batch_result::{BatchOperationResult, OperationMetrics};
