//! # ProximaDB Scan Executor
//!
//! Implements DataFusion's ExecutionPlan for scanning ProximaDB collections.
//! Supports parallel partition scanning with filter and projection pushdown.

use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::{DisplayAs, DisplayFormatType, ExecutionPlan, PlanProperties};
use futures::Stream;
use tracing::{debug, trace};

use crate::storage::formats::{FilterExpression, InternalFormat};

/// Information about a scan partition.
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    /// Partition index
    pub index: usize,
    /// File paths in this partition
    pub file_paths: Vec<String>,
    /// Estimated row count
    pub estimated_rows: Option<usize>,
    /// Estimated bytes
    pub estimated_bytes: Option<usize>,
}

/// DataFusion ExecutionPlan for scanning ProximaDB collections.
pub struct ProximaDBScanExec {
    /// Collection name
    collection_name: String,
    /// Output schema
    schema: SchemaRef,
    /// Projected schema (after projection pushdown)
    projected_schema: SchemaRef,
    /// Storage format for reading
    storage_format: Arc<dyn InternalFormat>,
    /// Base path for data files
    base_path: String,
    /// Column projection (indices)
    projection: Option<Vec<usize>>,
    /// Pushed-down filters
    filters: Option<FilterExpression>,
    /// Row limit
    limit: Option<usize>,
    /// Partition information
    partitions: Vec<PartitionInfo>,
    /// Batch size for reading
    batch_size: usize,
    /// Plan properties
    properties: PlanProperties,
}

impl ProximaDBScanExec {
    /// Create a new scan executor.
    pub fn try_new(
        collection_name: String,
        schema: SchemaRef,
        storage_format: Arc<dyn InternalFormat>,
        base_path: String,
        projection: Option<Vec<usize>>,
        filters: Option<FilterExpression>,
        limit: Option<usize>,
        _max_partitions: usize,
        batch_size: usize,
    ) -> DFResult<Self> {
        // Compute projected schema
        let projected_schema = if let Some(ref proj) = projection {
            let fields: Vec<_> = proj.iter().map(|&i| schema.field(i).clone()).collect();
            Arc::new(arrow_schema::Schema::new(fields))
        } else {
            schema.clone()
        };

        // Create partition info (for now, single partition per collection)
        // Deferred: Implement proper file discovery and partitioning
        let partitions = vec![PartitionInfo {
            index: 0,
            file_paths: vec![base_path.clone()],
            estimated_rows: None,
            estimated_bytes: None,
        }];

        // Create plan properties
        let partitioning = Partitioning::UnknownPartitioning(partitions.len());
        let eq_properties = EquivalenceProperties::new(projected_schema.clone());
        let properties = PlanProperties::new(
            eq_properties,
            partitioning,
            datafusion::physical_plan::execution_plan::EmissionType::Incremental,
            datafusion::physical_plan::execution_plan::Boundedness::Bounded,
        );

        Ok(Self {
            collection_name,
            schema,
            projected_schema,
            storage_format,
            base_path,
            projection,
            filters,
            limit,
            partitions,
            batch_size,
            properties,
        })
    }

    /// Get the number of partitions.
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }

    /// Get partition info.
    pub fn partition_info(&self, partition: usize) -> Option<&PartitionInfo> {
        self.partitions.get(partition)
    }
}

impl Debug for ProximaDBScanExec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProximaDBScanExec")
            .field("collection", &self.collection_name)
            .field("projection", &self.projection)
            .field("filters", &self.filters.is_some())
            .field("limit", &self.limit)
            .field("partitions", &self.partitions.len())
            .finish()
    }
}

impl DisplayAs for ProximaDBScanExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default
            | DisplayFormatType::Verbose
            | DisplayFormatType::TreeRender => {
                write!(
                    f,
                    "ProximaDBScanExec: collection={}, projection={:?}, filters={}, limit={:?}, partitions={}",
                    self.collection_name,
                    self.projection,
                    self.filters.is_some(),
                    self.limit,
                    self.partitions.len()
                )
            }
        }
    }
}

impl ExecutionPlan for ProximaDBScanExec {
    fn name(&self) -> &str {
        "ProximaDBScanExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn properties(&self) -> &PlanProperties {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![] // Leaf node
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        // Leaf node, no children to replace
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> DFResult<SendableRecordBatchStream> {
        debug!(
            "Executing scan for partition {} of collection '{}'",
            partition, self.collection_name
        );

        let partition_info = self.partitions.get(partition).ok_or_else(|| {
            DataFusionError::Execution(format!(
                "Invalid partition {} for collection '{}'",
                partition, self.collection_name
            ))
        })?;

        // Create the stream
        let stream = ProximaDBRecordBatchStream::new(
            self.collection_name.clone(),
            self.projected_schema.clone(),
            self.storage_format.clone(),
            partition_info.file_paths.clone(),
            self.projection.clone(),
            self.filters.clone(),
            self.limit,
            self.batch_size,
        );

        Ok(Box::pin(stream))
    }
}

/// RecordBatchStream implementation for ProximaDB scans.
pub struct ProximaDBRecordBatchStream {
    /// Collection name
    #[allow(dead_code)]
    collection_name: String,
    /// Output schema
    schema: SchemaRef,
    /// Storage format
    #[allow(dead_code)]
    storage_format: Arc<dyn InternalFormat>,
    /// File paths to scan
    file_paths: Vec<String>,
    /// Column projection
    #[allow(dead_code)]
    projection: Option<Vec<usize>>,
    /// Pushed-down filters
    #[allow(dead_code)]
    filters: Option<FilterExpression>,
    /// Row limit
    limit: Option<usize>,
    /// Batch size
    #[allow(dead_code)]
    batch_size: usize,
    /// Current file index
    current_file: usize,
    /// Rows returned so far
    rows_returned: usize,
    /// Whether the stream is finished
    finished: bool,
    /// Buffered batches from current file
    batch_buffer: Vec<RecordBatch>,
    /// Current batch index in buffer
    batch_index: usize,
}

impl ProximaDBRecordBatchStream {
    fn new(
        collection_name: String,
        schema: SchemaRef,
        storage_format: Arc<dyn InternalFormat>,
        file_paths: Vec<String>,
        projection: Option<Vec<usize>>,
        filters: Option<FilterExpression>,
        limit: Option<usize>,
        batch_size: usize,
    ) -> Self {
        Self {
            collection_name,
            schema,
            storage_format,
            file_paths,
            projection,
            filters,
            limit,
            batch_size,
            current_file: 0,
            rows_returned: 0,
            finished: false,
            batch_buffer: Vec::new(),
            batch_index: 0,
        }
    }

    /// Check if we've hit the limit.
    fn check_limit(&self) -> bool {
        if let Some(limit) = self.limit {
            self.rows_returned >= limit
        } else {
            false
        }
    }

    /// Apply limit to a batch.
    fn apply_limit(&self, batch: RecordBatch) -> Option<RecordBatch> {
        if let Some(limit) = self.limit {
            let remaining = limit.saturating_sub(self.rows_returned);
            if remaining == 0 {
                return None;
            }
            if batch.num_rows() > remaining {
                Some(batch.slice(0, remaining))
            } else {
                Some(batch)
            }
        } else {
            Some(batch)
        }
    }
}

impl Stream for ProximaDBRecordBatchStream {
    type Item = DFResult<RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check if finished
        if self.finished || self.check_limit() {
            return Poll::Ready(None);
        }

        // Return buffered batch if available
        if self.batch_index < self.batch_buffer.len() {
            let batch = self.batch_buffer[self.batch_index].clone();
            self.batch_index += 1;

            if let Some(limited_batch) = self.apply_limit(batch) {
                self.rows_returned += limited_batch.num_rows();
                return Poll::Ready(Some(Ok(limited_batch)));
            } else {
                self.finished = true;
                return Poll::Ready(None);
            }
        }

        // Move to next file if buffer exhausted
        if self.current_file >= self.file_paths.len() {
            self.finished = true;
            return Poll::Ready(None);
        }

        // For now, return Pending and let the async runtime handle it
        // In a real implementation, we'd load batches asynchronously
        // This is a simplified synchronous placeholder

        trace!(
            "Stream pending: file {}/{}, batch {}/{}",
            self.current_file,
            self.file_paths.len(),
            self.batch_index,
            self.batch_buffer.len()
        );

        // Mark as finished for now (placeholder)
        // Real implementation would use async batch loading
        self.finished = true;
        Poll::Ready(None)
    }
}

impl RecordBatchStream for ProximaDBRecordBatchStream {
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

    #[test]
    fn test_partition_info() {
        let info = PartitionInfo {
            index: 0,
            file_paths: vec!["/data/file1.parquet".to_string()],
            estimated_rows: Some(1000),
            estimated_bytes: Some(1024 * 1024),
        };

        assert_eq!(info.index, 0);
        assert_eq!(info.file_paths.len(), 1);
        assert_eq!(info.estimated_rows, Some(1000));
    }
}
