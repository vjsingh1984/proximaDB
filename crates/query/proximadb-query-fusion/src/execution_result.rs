//! Federated execution result types, extracted from root `query/federated/execution`
//! (TD-DECOMP-32). These are the Arrow-backed result containers for federated queries.

use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use proximadb_data_model::DataModel as ModelType;
use std::sync::Arc;

/// Backwards-compat alias for [`FederatedExecutionResult`].
pub type ExecutionResult = FederatedExecutionResult;

/// Execution result containing Arrow record batches
#[derive(Debug, Clone)]
pub struct FederatedExecutionResult {
    /// Result batches
    pub batches: Vec<RecordBatch>,
    /// Result schema
    pub schema: Arc<Schema>,
    /// Execution statistics
    pub stats: FederatedExecutionStats,
}

impl FederatedExecutionResult {
    /// Create an empty result
    pub fn empty() -> Self {
        let schema = Arc::new(Schema::empty());
        Self {
            batches: vec![],
            schema,
            stats: FederatedExecutionStats::default(),
        }
    }

    /// Create a result with a single batch
    pub fn from_batch(batch: RecordBatch) -> Self {
        let schema = batch.schema();
        let rows = batch.num_rows();
        Self {
            batches: vec![batch],
            schema,
            stats: FederatedExecutionStats {
                rows_produced: rows,
                ..Default::default()
            },
        }
    }

    /// Create an empty result with a known schema
    pub fn empty_with_schema(schema: Arc<Schema>) -> Self {
        Self {
            batches: vec![],
            schema,
            stats: FederatedExecutionStats::default(),
        }
    }

    /// Get total row count
    pub fn row_count(&self) -> usize {
        self.batches.iter().map(|b| b.num_rows()).sum()
    }
}

/// Federated query execution statistics.
#[derive(Debug, Default, Clone)]
pub struct FederatedExecutionStats {
    /// Total rows produced
    pub rows_produced: usize,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Bytes scanned
    pub bytes_scanned: u64,
    /// Models queried
    pub models_queried: Vec<ModelType>,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
}
