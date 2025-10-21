//! Vector and data operations
//!
//! Core operations on vectors including search, insert, update, delete

pub mod batch_result;
pub mod vectors;

#[cfg(test)]
pub mod vectors_test;

pub use batch_result::{BatchOperationResult, OperationMetrics};
pub use vectors::{UnifiedSearchConfig as SearchConfig, VectorOperationsService as VectorOps};
