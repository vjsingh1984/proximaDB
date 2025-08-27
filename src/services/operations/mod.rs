//! Vector and data operations
//! 
//! Core operations on vectors including search, insert, update, delete

pub mod vectors;

#[cfg(test)]
pub mod vectors_test;

pub use vectors::{
    VectorOperationsService as VectorOps,
    UnifiedSearchConfig as SearchConfig,
};