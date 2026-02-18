//! # DataSource Connector Traits
//!
//! This module defines the core traits for Spark DataSource V2-style connector interfaces.
//! These traits enable external query engines to integrate with ProximaDB for reading and
//! writing data in a standardized, efficient manner.
//!
//! ## Design Principles
//!
//! 1. **Arrow-Native**: All data exchange uses Apache Arrow RecordBatch for zero-copy
//! 2. **Async-First**: All I/O operations are async for maximum concurrency
//! 3. **Pushdown-Aware**: Connectors negotiate pushdown capabilities for optimization
//! 4. **Transactional**: Writers support commit/abort semantics for data integrity
//!
//! ## Trait Hierarchy
//!
//! ```text
//! DataSourceConnector
//!     ├── list_tables()     → Vec<TableInfo>
//!     ├── get_table()       → TableInfo
//!     ├── create_reader()   → Box<dyn DataReader>
//!     ├── create_writer()   → Box<dyn DataWriter>
//!     └── negotiate_pushdown() → PushdownResponse
//!
//! DataReader
//!     ├── schema()          → &Schema
//!     ├── next_batch()      → Option<RecordBatch>
//!     └── statistics()      → Option<Statistics>
//!
//! DataWriter
//!     ├── write_batch()     → ()
//!     ├── commit()          → WriteResult
//!     └── abort()           → ()
//! ```
//!
//! ## Implementation Notes
//!
//! - Implementors should use `anyhow::Result` for error handling
//! - All async methods use the `async_trait` macro for trait object compatibility
//! - The `Send` bound enables multi-threaded execution contexts

use anyhow::Result;
use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::pushdown::{PushdownRequest, PushdownResponse};
use super::types::{Statistics, TableInfo, WriteResult};

/// Context for read operations, containing query hints and execution parameters.
///
/// The `ReadContext` provides information to the connector about how the data will
/// be consumed, enabling optimizations like projection pushdown, filter pushdown,
/// and batch size tuning.
///
/// ## Example
///
/// ```rust,ignore
/// let ctx = ReadContext::new()
///     .with_projection(vec!["id", "embedding"])
///     .with_batch_size(1024)
///     .with_filter_hint("category = 'science'");
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReadContext {
    /// Columns to read (empty means all columns)
    pub projections: Vec<String>,

    /// Target batch size in rows (hint, not guaranteed)
    pub batch_size: Option<usize>,

    /// Filter expression hint (for pushdown negotiation)
    pub filter_hint: Option<String>,

    /// Partition filters (for partition pruning)
    pub partition_filters: HashMap<String, String>,

    /// Maximum number of rows to return (LIMIT pushdown)
    pub limit: Option<u64>,

    /// Offset for pagination (OFFSET pushdown)
    pub offset: Option<u64>,

    /// Vector search context (for vector-first queries)
    pub vector_search: Option<VectorSearchContext>,

    /// Graph traversal context (for graph queries)
    pub graph_traversal: Option<GraphTraversalContext>,

    /// Request specific partitions only
    pub partitions: Vec<PartitionSpec>,

    /// Execution hints for the storage engine
    pub hints: HashMap<String, String>,
}

impl ReadContext {
    /// Create a new empty read context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the columns to project.
    pub fn with_projections(mut self, projections: Vec<String>) -> Self {
        self.projections = projections;
        self
    }

    /// Set the target batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = Some(batch_size);
        self
    }

    /// Set a filter hint for pushdown.
    pub fn with_filter_hint(mut self, filter: impl Into<String>) -> Self {
        self.filter_hint = Some(filter.into());
        self
    }

    /// Set a row limit.
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set an offset for pagination.
    pub fn with_offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Add a partition filter.
    pub fn with_partition_filter(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.partition_filters.insert(key.into(), value.into());
        self
    }

    /// Set vector search context.
    pub fn with_vector_search(mut self, vector_search: VectorSearchContext) -> Self {
        self.vector_search = Some(vector_search);
        self
    }

    /// Set graph traversal context.
    pub fn with_graph_traversal(mut self, graph_traversal: GraphTraversalContext) -> Self {
        self.graph_traversal = Some(graph_traversal);
        self
    }

    /// Add an execution hint.
    pub fn with_hint(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.hints.insert(key.into(), value.into());
        self
    }

    /// Add specific partitions to read.
    pub fn with_partitions(mut self, partitions: Vec<PartitionSpec>) -> Self {
        self.partitions = partitions;
        self
    }
}

/// Vector search context for KNN queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchContext {
    /// Query vector for similarity search
    pub query_vector: Vec<f32>,

    /// Number of nearest neighbors to return
    pub top_k: u32,

    /// Minimum similarity threshold (0.0 to 1.0 for cosine)
    pub threshold: Option<f32>,

    /// Distance metric (cosine, euclidean, dot_product)
    pub metric: String,

    /// Vector column name
    pub vector_column: String,

    /// Enable approximate search (HNSW, IVF)
    pub approximate: bool,

    /// Ef parameter for HNSW search (higher = more accurate, slower)
    pub ef_search: Option<u32>,

    /// Number of probes for IVF search
    pub n_probe: Option<u32>,
}

impl VectorSearchContext {
    /// Create a new vector search context.
    pub fn new(query_vector: Vec<f32>, top_k: u32) -> Self {
        Self {
            query_vector,
            top_k,
            threshold: None,
            metric: "cosine".to_string(),
            vector_column: "embedding".to_string(),
            approximate: true,
            ef_search: None,
            n_probe: None,
        }
    }

    /// Set the distance metric.
    pub fn with_metric(mut self, metric: impl Into<String>) -> Self {
        self.metric = metric.into();
        self
    }

    /// Set a similarity threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = Some(threshold);
        self
    }

    /// Set the vector column name.
    pub fn with_vector_column(mut self, column: impl Into<String>) -> Self {
        self.vector_column = column.into();
        self
    }
}

/// Graph traversal context for graph queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTraversalContext {
    /// Starting node IDs for traversal
    pub start_nodes: Vec<String>,

    /// Edge types to follow (empty means all)
    pub edge_types: Vec<String>,

    /// Traversal direction (outbound, inbound, both)
    pub direction: TraversalDirection,

    /// Maximum traversal depth
    pub max_depth: u32,

    /// Node label filter
    pub node_labels: Vec<String>,

    /// Property filters for nodes
    pub node_filters: HashMap<String, String>,

    /// Property filters for edges
    pub edge_filters: HashMap<String, String>,
}

impl GraphTraversalContext {
    /// Create a new graph traversal context.
    pub fn new(start_nodes: Vec<String>) -> Self {
        Self {
            start_nodes,
            edge_types: Vec::new(),
            direction: TraversalDirection::Outbound,
            max_depth: 3,
            node_labels: Vec::new(),
            node_filters: HashMap::new(),
            edge_filters: HashMap::new(),
        }
    }

    /// Set the edge types to follow.
    pub fn with_edge_types(mut self, edge_types: Vec<String>) -> Self {
        self.edge_types = edge_types;
        self
    }

    /// Set the traversal direction.
    pub fn with_direction(mut self, direction: TraversalDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Set the maximum depth.
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }
}

/// Traversal direction for graph queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraversalDirection {
    /// Follow outgoing edges only
    Outbound,
    /// Follow incoming edges only
    Inbound,
    /// Follow edges in both directions
    Both,
}

impl Default for TraversalDirection {
    fn default() -> Self {
        Self::Outbound
    }
}

impl std::fmt::Display for TraversalDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Outbound => write!(f, "outbound"),
            Self::Inbound => write!(f, "inbound"),
            Self::Both => write!(f, "both"),
        }
    }
}

/// Partition specification for targeted reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionSpec {
    /// Partition key values
    pub values: HashMap<String, String>,

    /// Partition path (if available)
    pub path: Option<String>,

    /// Partition ID (internal)
    pub partition_id: Option<u64>,
}

/// Context for write operations, containing write mode and target configuration.
///
/// The `WriteContext` provides information about how data should be written,
/// including partitioning, compression, and transaction semantics.
///
/// ## Example
///
/// ```rust,ignore
/// let ctx = WriteContext::new()
///     .with_mode(WriteMode::Append)
///     .with_compression("zstd")
///     .with_partition_columns(vec!["date", "region"]);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WriteContext {
    /// Write mode (append, overwrite, error_if_exists)
    pub mode: WriteMode,

    /// Partition columns for data organization
    pub partition_columns: Vec<String>,

    /// Compression algorithm (none, lz4, zstd, snappy)
    pub compression: Option<String>,

    /// Target file size in bytes
    pub target_file_size: Option<u64>,

    /// Target row group size for columnar formats
    pub row_group_size: Option<usize>,

    /// Transaction ID for atomic writes
    pub transaction_id: Option<String>,

    /// Custom properties for the writer
    pub properties: HashMap<String, String>,

    /// Schema evolution mode
    pub schema_evolution: SchemaEvolutionMode,
}

impl WriteContext {
    /// Create a new empty write context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the write mode.
    pub fn with_mode(mut self, mode: WriteMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set partition columns.
    pub fn with_partition_columns(mut self, columns: Vec<String>) -> Self {
        self.partition_columns = columns;
        self
    }

    /// Set the compression algorithm.
    pub fn with_compression(mut self, compression: impl Into<String>) -> Self {
        self.compression = Some(compression.into());
        self
    }

    /// Set the target file size.
    pub fn with_target_file_size(mut self, size: u64) -> Self {
        self.target_file_size = Some(size);
        self
    }

    /// Set the row group size.
    pub fn with_row_group_size(mut self, size: usize) -> Self {
        self.row_group_size = Some(size);
        self
    }

    /// Set a transaction ID for atomic writes.
    pub fn with_transaction_id(mut self, tx_id: impl Into<String>) -> Self {
        self.transaction_id = Some(tx_id.into());
        self
    }

    /// Set the schema evolution mode.
    pub fn with_schema_evolution(mut self, mode: SchemaEvolutionMode) -> Self {
        self.schema_evolution = mode;
        self
    }

    /// Add a custom property.
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }
}

/// Write mode for data ingestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WriteMode {
    /// Append new data to existing data
    #[default]
    Append,
    /// Replace all existing data
    Overwrite,
    /// Fail if the target already has data
    ErrorIfExists,
    /// Merge with existing data (upsert semantics)
    Merge,
}

impl std::fmt::Display for WriteMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Append => write!(f, "append"),
            Self::Overwrite => write!(f, "overwrite"),
            Self::ErrorIfExists => write!(f, "error_if_exists"),
            Self::Merge => write!(f, "merge"),
        }
    }
}

/// Schema evolution mode for writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SchemaEvolutionMode {
    /// Strict schema matching required
    #[default]
    Strict,
    /// Allow adding new columns
    AddColumns,
    /// Allow type coercion within compatible types
    TypeCoercion,
    /// Full schema evolution (add, rename, widen types)
    Full,
}

/// Spark DataSource V2-style connector interface.
///
/// This trait defines the main entry point for integrating ProximaDB with external
/// query engines. Implementations provide table discovery, reader/writer creation,
/// and pushdown negotiation.
///
/// ## Implementation Guidelines
///
/// 1. `list_tables()` should return all accessible tables/collections
/// 2. `get_table()` should return detailed metadata for a specific table
/// 3. `create_reader()` should return a reader configured for the given context
/// 4. `create_writer()` should return a transactional writer
/// 5. `negotiate_pushdown()` should return which operations can be pushed down
///
/// ## Thread Safety
///
/// Implementations must be `Send + Sync` to allow sharing across threads in
/// multi-threaded query engines.
#[async_trait]
pub trait DataSourceConnector: Send + Sync {
    /// Returns the name of this connector for logging and debugging.
    fn connector_name(&self) -> &str;

    /// Discover all tables/collections available through this connector.
    ///
    /// Returns a list of `TableInfo` structures containing schema and statistics
    /// for each table. This is used by query planners for cost estimation.
    async fn list_tables(&self) -> Result<Vec<TableInfo>>;

    /// Get detailed information about a specific table.
    ///
    /// Returns the schema, partitioning, statistics, and other metadata
    /// for the named table.
    async fn get_table(&self, name: &str) -> Result<TableInfo>;

    /// Create a reader for streaming data from a table.
    ///
    /// The reader should respect the projections, filters, and limits
    /// specified in the `ReadContext`. Data is returned as Arrow RecordBatches.
    fn create_reader(&self, table: &str, ctx: &ReadContext) -> Result<Box<dyn DataReader>>;

    /// Create a writer for inserting data into a table.
    ///
    /// The writer supports transactional semantics with explicit commit/abort.
    /// Data is written as Arrow RecordBatches.
    fn create_writer(&self, table: &str, ctx: &WriteContext) -> Result<Box<dyn DataWriter>>;

    /// Negotiate which operations can be pushed down to the storage layer.
    ///
    /// The connector examines the `PushdownRequest` and returns a `PushdownResponse`
    /// indicating which filters, projections, and aggregates it can handle natively.
    /// This enables the query engine to avoid redundant processing.
    fn negotiate_pushdown(&self, table: &str, pushdown: &PushdownRequest) -> PushdownResponse;

    /// Check if the connector supports a specific capability.
    ///
    /// Common capabilities:
    /// - "vector_search": Native KNN search
    /// - "graph_traversal": Native graph queries
    /// - "full_text_search": Full-text indexing
    /// - "transactions": ACID transactions
    /// - "time_travel": Historical queries
    fn supports_capability(&self, capability: &str) -> bool {
        // Default: no special capabilities
        let _ = capability;
        false
    }

    /// Get the connector's supported push down capabilities as a list.
    fn capabilities(&self) -> Vec<String> {
        Vec::new()
    }

    /// Validate that the table exists and is accessible.
    async fn validate_table(&self, table: &str) -> Result<bool> {
        match self.get_table(table).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

/// Streaming reader for Arrow RecordBatches.
///
/// The `DataReader` trait provides an async iterator interface for reading
/// data from ProximaDB tables. Implementations should apply any pushdown
/// optimizations negotiated during reader creation.
///
/// ## Streaming Semantics
///
/// - `next_batch()` returns `Some(batch)` while data is available
/// - `next_batch()` returns `None` when all data has been read
/// - Errors are propagated immediately via `Result`
///
/// ## Example
///
/// ```rust,ignore
/// let mut reader = connector.create_reader("table", &ctx)?;
///
/// while let Some(batch) = reader.next_batch().await? {
///     println!("Read {} rows", batch.num_rows());
/// }
/// ```
#[async_trait]
pub trait DataReader: Send {
    /// Get the schema of the data being read.
    ///
    /// The schema reflects any projection pushdown that was applied.
    fn schema(&self) -> &Arc<Schema>;

    /// Read the next batch of data.
    ///
    /// Returns `Ok(Some(batch))` if data is available, `Ok(None)` if
    /// all data has been read, or `Err` on failure.
    async fn next_batch(&mut self) -> Result<Option<RecordBatch>>;

    /// Get statistics about the data being read.
    ///
    /// Statistics may be available before reading begins (from metadata)
    /// or may become more accurate as reading progresses.
    fn statistics(&self) -> Option<Statistics>;

    /// Get the number of batches remaining (if known).
    fn batches_remaining(&self) -> Option<usize> {
        None
    }

    /// Get progress as a fraction (0.0 to 1.0).
    fn progress(&self) -> Option<f64> {
        None
    }

    /// Hint that the reader should prefetch upcoming batches.
    fn prefetch_hint(&mut self, _batches: usize) {
        // Default: no-op
    }

    /// Cancel any ongoing read operations.
    async fn cancel(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Transactional writer for Arrow RecordBatches.
///
/// The `DataWriter` trait provides a transactional interface for writing
/// data to ProximaDB tables. All writes are staged until `commit()` is called,
/// and can be discarded with `abort()`.
///
/// ## Transaction Semantics
///
/// 1. Create writer with `create_writer()`
/// 2. Write batches with `write_batch()`
/// 3. Finalize with `commit()` or discard with `abort()`
///
/// ## Example
///
/// ```rust,ignore
/// let mut writer = connector.create_writer("table", &ctx)?;
///
/// writer.write_batch(&batch1).await?;
/// writer.write_batch(&batch2).await?;
///
/// let result = writer.commit().await?;
/// println!("Wrote {} rows", result.rows_written);
/// ```
#[async_trait]
pub trait DataWriter: Send {
    /// Write a batch of data.
    ///
    /// The batch is staged for commit. The schema must match the target
    /// table schema (subject to schema evolution mode).
    async fn write_batch(&mut self, batch: &RecordBatch) -> Result<()>;

    /// Commit all staged writes.
    ///
    /// After commit, the data becomes visible to readers. Returns statistics
    /// about the committed data.
    async fn commit(&mut self) -> Result<WriteResult>;

    /// Abort the write transaction.
    ///
    /// Discards all staged writes. The target table is unchanged.
    async fn abort(&mut self) -> Result<()>;

    /// Get the count of rows written so far (staged, not committed).
    fn rows_staged(&self) -> u64 {
        0
    }

    /// Get the count of bytes written so far (staged, not committed).
    fn bytes_staged(&self) -> u64 {
        0
    }

    /// Flush buffered writes to storage without committing.
    ///
    /// This is useful for writers that buffer data in memory before
    /// writing to storage. Flush ensures data is persisted but does
    /// not make it visible to readers.
    async fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
