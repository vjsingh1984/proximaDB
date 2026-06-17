//! Typed result structures for batch operations.
//!
//! TD-104 S3: these types were relocated to `proximadb-runtime` so the runtime
//! `RecordOpsPort` contract can return them (the Arrow Flight ingest path now
//! depends on the port, not the concrete root handler). This module re-exports
//! them so existing `crate::services::operations::BatchOperationResult` callers
//! are unaffected.

pub use proximadb_runtime::batch_result::{
    BatchOperationMetrics, BatchOperationResult, OperationMetrics,
};
