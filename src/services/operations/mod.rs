//! Vector and data operations
//!
//! Core operations on vectors including search, insert, update, delete

pub mod batch_result;
pub mod bulk_write_router;
pub mod catalog_bulk_write;
pub mod secure_operations;
pub mod vectors;

#[cfg(test)]
pub mod vectors_test;

pub use batch_result::{BatchOperationResult, OperationMetrics};
pub use bulk_write_router::{BulkWriteConfig, BulkWriteDecision, BulkWriteRouter};
pub use catalog_bulk_write::{
    BulkWriteMode, BulkWriteTransaction, CatalogBulkWriteConfig, CatalogBulkWriteResult,
    CatalogBulkWriteService, IsolationLevel,
};
pub use secure_operations::{SecureVectorOperations, combine_filters};
// Re-export from vectors module (now decomposed)
pub use vectors::{
    SearchPlanHints, UnifiedSearchConfig as SearchConfig, VectorOperationsService as VectorOps,
};
