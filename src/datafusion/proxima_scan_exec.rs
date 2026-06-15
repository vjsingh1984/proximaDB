//! # ProximaDB Execution Plan for DataFusion
//!
//! Implements a custom DataFusion ExecutionPlan that reads from ProximaDB splits.
//! This plan supports parallel partition scanning with filter and projection pushdown.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                         PROXIMA SCAN EXEC                                   │
//! │  ┌───────────────────────────────────────────────────────────────────────┐  │
//! │  │  ProximaScanExec                                                      │  │
//! │  │  - schema: SchemaRef                                                  │  │
//! │  │  - splits: Vec<FileSplit>                                             │  │
//! │  │  - projection: Option<Vec<usize>>                                     │  │
//! │  │  - filters: Vec<Expr>                                                 │  │
//! │  │  - limit: Option<usize>                                               │  │
//! │  │  - reader: Arc<dyn SplitReader>                                       │  │
//! │  └───────────────────────────────────────────────────────────────────────┘  │
//! │                                      │                                       │
//! │                                      ▼                                       │
//! │  ┌───────────────────────────────────────────────────────────────────────┐  │
//! │  │  SplitReader Implementations                                          │  │
//! │  │  - SstSplitReader                                                     │  │
//! │  │  - HelixSplitReader                                                   │  │
//! │  │  - ViperSplitReader (Parquet)                                         │  │
//! │  │  - NovaSplitReader                                                    │  │
//! │  │  - SwiftSplitReader                                                   │  │
//! │  │  - RaptorSplitReader                                                  │  │
//! │  └───────────────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let scan_exec = ProximaScanExec::builder()
//!     .schema(schema)
//!     .splits(splits)
//!     .projection(Some(vec![0, 1, 2]))
//!     .reader(Arc::new(SstSplitReader::new()))
//!     .build()?;
//!
//! // Execute partition 0
//! let stream = scan_exec.execute(0, context)?;
//! ```

use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::Statistics;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties};
use futures::{Stream, StreamExt, TryStreamExt};
use tracing::debug;

use super::proxima_table_provider::EngineType;
use crate::storage::formats::FileSplit;

// ============================================================================
// Split Reader Trait
// ============================================================================

/// Trait for reading data from splits.
///
/// Each storage engine implements this trait to provide engine-specific
/// split reading logic with support for projection and batch sizing.
#[async_trait]
pub trait SplitReader: Send + Sync + Debug {
    /// Read a split and return a record batch stream.
    ///
    /// # Arguments
    /// * `split` - The split to read
    /// * `projection` - Optional column indices to read (None = all columns)
    /// * `batch_size` - Target number of rows per batch
    ///
    /// # Returns
    /// * A stream of RecordBatches
    async fn read_split(
        &self,
        split: &FileSplit,
        projection: Option<&[usize]>,
        batch_size: usize,
    ) -> DFResult<SendableRecordBatchStream>;

    /// Get the schema for this reader.
    fn schema(&self) -> SchemaRef;

    /// Get the engine type for this reader.
    fn engine_type(&self) -> EngineType;

    /// Check if this reader supports filter pushdown.
    fn supports_filter_pushdown(&self) -> bool {
        false
    }

    /// Check if this reader supports projection pushdown.
    fn supports_projection_pushdown(&self) -> bool {
        true
    }
}

// ============================================================================
// Proxima Scan Execution Plan
// ============================================================================

/// DataFusion ExecutionPlan for scanning ProximaDB collections.
///
/// This plan divides work into partitions based on splits, allowing
/// parallel execution across multiple threads or nodes.
pub struct ProximaScanExec {
    /// Output schema (after projection)
    schema: SchemaRef,
    /// Original schema (before projection)
    #[allow(dead_code)] // reserved field for planned engine read-path wiring (unused in --lib)
    original_schema: SchemaRef,
    /// Splits to read, organized by partition
    partitions: Vec<Vec<FileSplit>>,
    /// Column projection (indices)
    projection: Option<Vec<usize>>,
    /// Pushed-down filters (as DataFusion expressions)
    filters: Vec<datafusion::logical_expr::Expr>,
    /// Row limit (if any)
    limit: Option<usize>,
    /// Engine-specific split reader
    reader: Arc<dyn SplitReader>,
    /// Batch size for reading
    batch_size: usize,
    /// Collection name for logging
    collection_name: String,
    /// Plan properties
    properties: PlanProperties,
    /// Cached statistics
    statistics: Option<Statistics>,
}

impl ProximaScanExec {
    /// Create a new ProximaScanExec using the builder pattern.
    pub fn builder() -> ProximaScanExecBuilder {
        ProximaScanExecBuilder::default()
    }

    /// Create a simple scan exec with minimal configuration.
    // build() cannot fail here: the builder has every required field set, so the
    // Result is infallible by construction (keeping `new` panic-on-bug, infallible API).
    #[allow(clippy::expect_used)]
    pub fn new(schema: SchemaRef, splits: Vec<FileSplit>, reader: Arc<dyn SplitReader>) -> Self {
        Self::builder()
            .schema(schema)
            .splits(splits)
            .reader(reader)
            .build()
            .expect("Failed to build ProximaScanExec")
    }

    /// Get the number of partitions.
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }

    /// Get splits for a specific partition.
    pub fn partition_splits(&self, partition: usize) -> Option<&[FileSplit]> {
        self.partitions.get(partition).map(|v| v.as_slice())
    }

    /// Get total split count across all partitions.
    pub fn total_split_count(&self) -> usize {
        self.partitions.iter().map(|p| p.len()).sum()
    }

    /// Get the engine type.
    pub fn engine_type(&self) -> EngineType {
        self.reader.engine_type()
    }

    /// Get the collection name.
    pub fn collection_name(&self) -> &str {
        &self.collection_name
    }

    /// Get the batch size.
    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Get the projection.
    pub fn projection(&self) -> Option<&[usize]> {
        self.projection.as_deref()
    }

    /// Get the limit.
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Apply schema projection.
    fn project_schema(schema: &SchemaRef, projection: &Option<Vec<usize>>) -> SchemaRef {
        if let Some(proj) = projection {
            let fields: Vec<_> = proj
                .iter()
                .filter_map(|&i| schema.fields().get(i))
                .map(|f| f.as_ref().clone())
                .collect();
            Arc::new(arrow_schema::Schema::new(fields))
        } else {
            schema.clone()
        }
    }
}

impl Debug for ProximaScanExec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProximaScanExec")
            .field("collection", &self.collection_name)
            .field("partitions", &self.partitions.len())
            .field("total_splits", &self.total_split_count())
            .field("projection", &self.projection)
            .field("limit", &self.limit)
            .field("engine", &self.reader.engine_type())
            .finish()
    }
}

impl DisplayAs for ProximaScanExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default
            | DisplayFormatType::Verbose
            | DisplayFormatType::TreeRender => {
                write!(
                    f,
                    "ProximaScanExec: collection={}, engine={}, partitions={}, splits={}, projection={:?}, filters={}, limit={:?}",
                    self.collection_name,
                    self.reader.engine_type(),
                    self.partitions.len(),
                    self.total_split_count(),
                    self.projection,
                    self.filters.len(),
                    self.limit
                )
            }
        }
    }
}

impl ExecutionPlan for ProximaScanExec {
    fn name(&self) -> &str {
        "ProximaScanExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn properties(&self) -> &PlanProperties {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![] // Leaf node - no children
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        // Leaf node - no children to replace
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> DFResult<SendableRecordBatchStream> {
        debug!(
            "Executing ProximaScanExec partition {} of {} for collection '{}' ({} splits)",
            partition,
            self.partitions.len(),
            self.collection_name,
            self.partitions.get(partition).map(|p| p.len()).unwrap_or(0)
        );

        let splits = self
            .partitions
            .get(partition)
            .ok_or_else(|| {
                DataFusionError::Execution(format!(
                    "Invalid partition {} for collection '{}' (max: {})",
                    partition,
                    self.collection_name,
                    self.partitions.len()
                ))
            })?
            .clone();

        let reader = self.reader.clone();
        let projection = self.projection.clone();
        let batch_size = self.batch_size;
        let limit = self.limit;
        let schema = self.schema.clone();

        // Drive each split's async `read_split` lazily and flatten the per-split batch
        // streams into one. Each split's future owns its `FileSplit` + an `Arc` clone of
        // the reader, so the resulting stream is `'static` and `Send`.
        let batches = futures::stream::iter(splits)
            .then(move |split| {
                let reader = reader.clone();
                let projection = projection.clone();
                async move {
                    reader
                        .read_split(&split, projection.as_deref(), batch_size)
                        .await
                }
            })
            .try_flatten();

        // Honor the row limit without over-reading: stop once `limit` rows are emitted.
        let limited = batches.scan(0usize, move |emitted, item| {
            let out = match (item, limit) {
                (Ok(batch), Some(lim)) => {
                    if *emitted >= lim {
                        None
                    } else {
                        let remaining = lim - *emitted;
                        let batch = if batch.num_rows() > remaining {
                            batch.slice(0, remaining)
                        } else {
                            batch
                        };
                        *emitted += batch.num_rows();
                        Some(Ok(batch))
                    }
                }
                (Ok(batch), None) => Some(Ok(batch)),
                (Err(e), _) => Some(Err(e)),
            };
            futures::future::ready(out)
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, limited)))
    }

    fn statistics(&self) -> DFResult<Statistics> {
        Ok(self.statistics.clone().unwrap_or_default())
    }
}

// ============================================================================
// Builder Pattern
// ============================================================================

/// Builder for ProximaScanExec.
#[derive(Default)]
pub struct ProximaScanExecBuilder {
    schema: Option<SchemaRef>,
    splits: Option<Vec<FileSplit>>,
    projection: Option<Vec<usize>>,
    filters: Vec<datafusion::logical_expr::Expr>,
    limit: Option<usize>,
    reader: Option<Arc<dyn SplitReader>>,
    batch_size: usize,
    collection_name: String,
    target_partitions: usize,
    statistics: Option<Statistics>,
}

impl ProximaScanExecBuilder {
    /// Set the output schema.
    pub fn schema(mut self, schema: SchemaRef) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Set the splits to read.
    pub fn splits(mut self, splits: Vec<FileSplit>) -> Self {
        self.splits = Some(splits);
        self
    }

    /// Set column projection.
    pub fn projection(mut self, projection: Option<Vec<usize>>) -> Self {
        self.projection = projection;
        self
    }

    /// Set pushed-down filters.
    pub fn filters(mut self, filters: Vec<datafusion::logical_expr::Expr>) -> Self {
        self.filters = filters;
        self
    }

    /// Set row limit.
    pub fn limit(mut self, limit: Option<usize>) -> Self {
        self.limit = limit;
        self
    }

    /// Set the split reader.
    pub fn reader(mut self, reader: Arc<dyn SplitReader>) -> Self {
        self.reader = Some(reader);
        self
    }

    /// Set batch size for reading.
    pub fn batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set collection name for logging.
    pub fn collection_name(mut self, name: String) -> Self {
        self.collection_name = name;
        self
    }

    /// Set target number of partitions.
    pub fn target_partitions(mut self, partitions: usize) -> Self {
        self.target_partitions = partitions;
        self
    }

    /// Set statistics.
    pub fn statistics(mut self, stats: Statistics) -> Self {
        self.statistics = Some(stats);
        self
    }

    /// Build the ProximaScanExec.
    pub fn build(self) -> DFResult<ProximaScanExec> {
        let schema = self.schema.ok_or_else(|| {
            DataFusionError::Plan("Schema is required for ProximaScanExec".to_string())
        })?;

        let reader = self.reader.ok_or_else(|| {
            DataFusionError::Plan("Reader is required for ProximaScanExec".to_string())
        })?;

        let splits = self.splits.unwrap_or_default();
        let batch_size = if self.batch_size > 0 {
            self.batch_size
        } else {
            8192
        };
        let target_partitions = if self.target_partitions > 0 {
            self.target_partitions
        } else {
            num_cpus::get()
        };

        // Partition splits for parallel execution
        let partitions = partition_splits(splits, target_partitions);

        // Apply projection to schema
        let projected_schema = ProximaScanExec::project_schema(&schema, &self.projection);

        // Create plan properties
        let partitioning = Partitioning::UnknownPartitioning(partitions.len().max(1));
        let eq_properties = EquivalenceProperties::new(projected_schema.clone());
        let properties = PlanProperties::new(
            eq_properties,
            partitioning,
            datafusion::physical_plan::execution_plan::EmissionType::Incremental,
            datafusion::physical_plan::execution_plan::Boundedness::Bounded,
        );

        Ok(ProximaScanExec {
            schema: projected_schema,
            original_schema: schema,
            partitions,
            projection: self.projection,
            filters: self.filters,
            limit: self.limit,
            reader,
            batch_size,
            collection_name: self.collection_name,
            properties,
            statistics: self.statistics,
        })
    }
}

/// Partition splits for parallel execution using a greedy algorithm.
fn partition_splits(splits: Vec<FileSplit>, target_partitions: usize) -> Vec<Vec<FileSplit>> {
    if splits.is_empty() {
        return vec![vec![]];
    }

    if target_partitions <= 1 {
        return vec![splits];
    }

    let mut partitions: Vec<Vec<FileSplit>> = vec![vec![]; target_partitions];
    let mut partition_costs: Vec<u64> = vec![0; target_partitions];

    // Sort splits by cost (descending) for better load balancing
    let mut sorted_splits = splits;
    sorted_splits.sort_by_key(|s| std::cmp::Reverse(s.estimated_cost()));

    // Greedy assignment to partition with lowest cost
    for split in sorted_splits {
        let cost = split.estimated_cost();
        let min_idx = partition_costs
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| *c)
            .map(|(i, _)| i)
            .unwrap_or(0);

        partitions[min_idx].push(split);
        partition_costs[min_idx] += cost;
    }

    // Remove empty partitions
    partitions.retain(|p| !p.is_empty());

    // Ensure at least one partition
    if partitions.is_empty() {
        partitions.push(vec![]);
    }

    partitions
}

// ============================================================================
// Null Split Reader for Testing
// ============================================================================

/// Null implementation of SplitReader for testing.
#[derive(Debug)]
pub struct NullSplitReader {
    schema: SchemaRef,
    engine_type: EngineType,
}

impl NullSplitReader {
    /// Create a new null reader for testing.
    pub fn new(schema: SchemaRef, engine_type: EngineType) -> Self {
        Self {
            schema,
            engine_type,
        }
    }
}

#[async_trait]
impl SplitReader for NullSplitReader {
    async fn read_split(
        &self,
        _split: &FileSplit,
        _projection: Option<&[usize]>,
        _batch_size: usize,
    ) -> DFResult<SendableRecordBatchStream> {
        // Return an empty stream for testing
        Ok(Box::pin(EmptyRecordBatchStream::new(self.schema.clone())))
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn engine_type(&self) -> EngineType {
        self.engine_type
    }
}

/// Empty RecordBatchStream for testing.
pub struct EmptyRecordBatchStream {
    schema: SchemaRef,
}

impl EmptyRecordBatchStream {
    /// Create an empty record-batch stream that yields no rows for `schema`.
    pub fn new(schema: SchemaRef) -> Self {
        Self { schema }
    }
}

impl Stream for EmptyRecordBatchStream {
    type Item = DFResult<RecordBatch>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

impl RecordBatchStream for EmptyRecordBatchStream {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Schema};

    fn test_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::FixedSizeBinary(512), false),
            Field::new("metadata", DataType::Utf8, true),
        ]))
    }

    #[test]
    fn test_partition_splits_empty() {
        let partitions = partition_splits(vec![], 4);
        assert_eq!(partitions.len(), 1);
        assert!(partitions[0].is_empty());
    }

    #[test]
    fn test_partition_splits_single() {
        let splits = vec![FileSplit::new_block("/f1.sst".to_string(), 0, 0, 1000, 100)];
        let partitions = partition_splits(splits, 1);
        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].len(), 1);
    }

    #[test]
    fn test_partition_splits_balanced() {
        let splits = vec![
            FileSplit::new_block("/f1.sst".to_string(), 0, 0, 1000, 100),
            FileSplit::new_block("/f1.sst".to_string(), 1, 1000, 1000, 100),
            FileSplit::new_block("/f2.sst".to_string(), 0, 0, 1000, 100),
            FileSplit::new_block("/f2.sst".to_string(), 1, 1000, 1000, 100),
        ];
        let partitions = partition_splits(splits, 2);
        assert_eq!(partitions.len(), 2);
        // Greedy algorithm should balance approximately
        assert!(partitions[0].len() >= 1 && partitions[1].len() >= 1);
    }

    #[test]
    fn test_proxima_scan_exec_builder() {
        let schema = test_schema();
        let reader = Arc::new(NullSplitReader::new(schema.clone(), EngineType::Sst));
        let splits = vec![FileSplit::new_block("/f1.sst".to_string(), 0, 0, 1000, 100)];

        let exec = ProximaScanExec::builder()
            .schema(schema)
            .splits(splits)
            .reader(reader)
            .collection_name("test".to_string())
            .batch_size(1024)
            .target_partitions(2)
            .build()
            .unwrap();

        assert_eq!(exec.collection_name(), "test");
        assert_eq!(exec.batch_size(), 1024);
        assert_eq!(exec.engine_type(), EngineType::Sst);
        assert!(exec.partition_count() >= 1);
    }

    #[test]
    fn test_proxima_scan_exec_new() {
        let schema = test_schema();
        let reader = Arc::new(NullSplitReader::new(schema.clone(), EngineType::Viper));
        let splits = vec![FileSplit::new_row_group(
            "/f1.parquet".to_string(),
            0,
            0,
            65536,
            10000,
        )];

        let exec = ProximaScanExec::new(schema, splits, reader);

        assert_eq!(exec.engine_type(), EngineType::Viper);
        assert_eq!(exec.total_split_count(), 1);
    }

    #[test]
    fn test_proxima_scan_exec_projection() {
        let schema = test_schema();
        let reader = Arc::new(NullSplitReader::new(schema.clone(), EngineType::Nova));

        let exec = ProximaScanExec::builder()
            .schema(schema)
            .splits(vec![])
            .reader(reader)
            .projection(Some(vec![0, 2])) // Select id and metadata
            .build()
            .unwrap();

        // Projected schema should have 2 fields
        assert_eq!(exec.schema.fields().len(), 2);
        assert_eq!(exec.projection(), Some(&[0, 2][..]));
    }

    #[test]
    fn test_proxima_scan_exec_limit() {
        let schema = test_schema();
        let reader = Arc::new(NullSplitReader::new(schema.clone(), EngineType::Swift));

        let exec = ProximaScanExec::builder()
            .schema(schema)
            .splits(vec![])
            .reader(reader)
            .limit(Some(100))
            .build()
            .unwrap();

        assert_eq!(exec.limit(), Some(100));
    }

    #[test]
    fn test_proxima_scan_exec_display() {
        let schema = test_schema();
        let reader = Arc::new(NullSplitReader::new(schema.clone(), EngineType::Raptor));
        let splits = vec![
            FileSplit::new_block("/f1.sst".to_string(), 0, 0, 1000, 100),
            FileSplit::new_block("/f1.sst".to_string(), 1, 1000, 1000, 100),
        ];

        let exec = ProximaScanExec::builder()
            .schema(schema)
            .splits(splits)
            .reader(reader)
            .collection_name("test_collection".to_string())
            .build()
            .unwrap();

        let display = format!("{:?}", exec);
        assert!(display.contains("test_collection"));
        assert!(display.contains("ProximaScanExec"));
    }

    #[test]
    fn test_null_split_reader() {
        let schema = test_schema();
        let reader = NullSplitReader::new(schema.clone(), EngineType::Helix);

        assert_eq!(reader.engine_type(), EngineType::Helix);
        assert_eq!(reader.schema().fields().len(), 3);
        assert!(!reader.supports_filter_pushdown());
        assert!(reader.supports_projection_pushdown());
    }

    #[tokio::test]
    async fn test_null_split_reader_read() {
        let schema = test_schema();
        let reader = NullSplitReader::new(schema.clone(), EngineType::Sst);
        let split = FileSplit::new_block("/f1.sst".to_string(), 0, 0, 1000, 100);

        let stream = reader.read_split(&split, None, 1024).await.unwrap();

        // Should return empty stream
        let schema = stream.schema();
        assert_eq!(schema.fields().len(), 3);
    }

    #[test]
    fn test_empty_record_batch_stream() {
        let schema = test_schema();
        let stream = EmptyRecordBatchStream::new(schema.clone());
        assert_eq!(stream.schema().fields().len(), 3);
    }
}
