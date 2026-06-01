//! # DataSource Connectors Module
//!
//! This module provides Spark DataSource V2-style connector interfaces for external system
//! integration with ProximaDB. It enables Hadoop-style storage-compute separation by defining
//! standardized traits for data readers, writers, and pushdown optimization.
//!
//! ## Overview
//!
//! The connectors module implements a plugin architecture for integrating ProximaDB with
//! external data processing systems like Apache Spark, Apache Flink, or other query engines.
//! This enables ProximaDB to act as both a data source and data sink in larger data pipelines.
//!
//! ## Key Components
//!
//! - **DataSourceConnector**: Main trait for table discovery and reader/writer creation
//! - **DataReader**: Streaming interface for reading Arrow RecordBatches from tables
//! - **DataWriter**: Transactional interface for writing Arrow RecordBatches to tables
//! - **Pushdown Protocol**: Negotiation protocol for predicate and projection pushdown
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        External Query Engine                             │
//! │                    (Spark, Flink, Trino, etc.)                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//!                                     │
//!                        ┌────────────┼────────────┐
//!                        ▼            ▼            ▼
//!              ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
//!              │ list_tables │ │ get_table   │ │ negotiate_  │
//!              │             │ │             │ │ pushdown    │
//!              └─────────────┘ └─────────────┘ └─────────────┘
//!                        │            │            │
//!                        └────────────┼────────────┘
//!                                     ▼
//!                        ┌─────────────────────────┐
//!                        │   DataSourceConnector   │
//!                        └─────────────────────────┘
//!                           │                   │
//!                           ▼                   ▼
//!               ┌─────────────────┐   ┌─────────────────┐
//!               │   DataReader    │   │   DataWriter    │
//!               │  (streaming)    │   │ (transactional) │
//!               └─────────────────┘   └─────────────────┘
//!                           │                   │
//!                           ▼                   ▼
//!               ┌─────────────────────────────────────┐
//!               │     ProximaDB Storage Engines       │
//!               │    (SST, VIPER, HELIX, etc.)        │
//!               └─────────────────────────────────────┘
//! ```
//!
//! ## Pushdown Optimization
//!
//! The connector supports aggressive pushdown of operations to ProximaDB:
//!
//! - **Filter Pushdown**: SQL predicates are pushed down to storage engines
//! - **Projection Pushdown**: Only requested columns are read
//! - **Aggregate Pushdown**: Aggregations can be computed at the storage layer
//! - **Vector Search Pushdown**: KNN queries are executed natively
//! - **Graph Traversal Pushdown**: Graph queries are executed by ORION/PULSAR engines
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::connectors::{DataSourceConnector, ReadContext, WriteContext};
//!
//! // Connect to ProximaDB as a data source
//! let connector = ProximaDBConnector::new(config);
//!
//! // Discover available tables
//! let tables = connector.list_tables().await?;
//!
//! // Create a reader with filter pushdown
//! let ctx = ReadContext::new()
//!     .with_projection(vec!["id", "embedding", "metadata"])
//!     .with_filter("category = 'science'");
//!
//! let mut reader = connector.create_reader("vectors", &ctx)?;
//!
//! // Stream data in Arrow batches
//! while let Some(batch) = reader.next_batch().await? {
//!     process_batch(batch);
//! }
//! ```
//!
//! ## Integration Points
//!
//! - **Storage Layer**: Connectors delegate to `UnifiedStorageFormat` implementations
//! - **Index Layer**: AXIS engine handles vector search pushdown
//! - **Graph Layer**: ORION/PULSAR engines handle graph traversal pushdown
//! - **WAL System**: Writers integrate with WAL for transactional guarantees

pub mod pushdown;
pub mod traits;
pub mod types;

// Compute engine connectors
pub mod duckdb;
pub mod spark;
pub mod trino;

// Legacy Hadoop compatibility
pub mod hadoop;

// Re-export main types for convenient access
pub use pushdown::{
    AggExpr, Expr, GraphTraversalPushdown, PushdownRequest, PushdownResponse, VectorSearchPushdown,
};
pub use traits::{DataReader, DataSourceConnector, DataWriter, ReadContext, WriteContext};
pub use types::{ColumnStatistics, Statistics, TableInfo, TableStatistics, WriteResult};

// Spark DataSource V2 connector
pub use spark::{
    SparkConnectorConfig, SparkDataWriter, SparkFilter, SparkFilterType, SparkInputPartition,
    SparkPartitionReader, SparkScanBuilder, SparkTable, SparkWriteBuilder, SparkWriteCommitMessage,
    SparkWriteError, SparkWriteMode,
};

// Trino SPI connector
pub use trino::{
    TrinoBlock, TrinoColumnMetadata, TrinoConnectorConfig, TrinoConnectorSession, TrinoDomain,
    TrinoError, TrinoErrorCode, TrinoHostAddress, TrinoPage, TrinoPageSink, TrinoPageSource,
    TrinoRange, TrinoSchema, TrinoSplit, TrinoSplitManager, TrinoTable, TrinoTableLayout,
    TrinoTupleDomain, TrinoWriteSummary,
};

// DuckDB extension connector
pub use duckdb::{
    DuckDBBindData, DuckDBColumnRef, DuckDBColumnStats, DuckDBConnectorConfig, DuckDBCopy,
    DuckDBCopyResult, DuckDBError, DuckDBErrorType, DuckDBFilter, DuckDBFilterType,
    DuckDBGlobalState, DuckDBInitData, DuckDBInsert, DuckDBInsertResult, DuckDBLocalState,
    DuckDBScanStatistics, DuckDBTableScan, DuckDBVectorSearch, DuckDBVectorSearchParams,
    DuckDBWriteMode,
};

// Hadoop compatibility shim (for Hive, EMR, legacy MapReduce)
pub use hadoop::{
    HadoopError, HadoopErrorCode, HadoopInputSplit, HadoopShimConfig, HadoopWritable, HiveType,
    ProximaInputFormat, ProximaOutputCommitter, ProximaOutputFormat, ProximaRecordReader,
    ProximaRecordWriter, ProximaSerDe,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_pushdown_request_creation() {
        let request = PushdownRequest {
            filters: vec![],
            projections: vec!["id".to_string(), "embedding".to_string()],
            aggregates: vec![],
            limit: Some(100),
            vector_search: None,
            graph_traversal: None,
        };

        assert_eq!(request.projections.len(), 2);
        assert_eq!(request.limit, Some(100));
    }

    #[test]
    fn test_table_info_creation() {
        use arrow::datatypes::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 128),
                false,
            ),
        ]));

        let table_info = TableInfo {
            name: "embeddings".to_string(),
            schema,
            partitioning: Some(vec!["date".to_string()]),
            properties: HashMap::new(),
            statistics: None,
        };

        assert_eq!(table_info.name, "embeddings");
        assert!(table_info.partitioning.is_some());
    }

    #[test]
    fn test_vector_search_pushdown() {
        let pushdown = VectorSearchPushdown {
            collection: "vectors".to_string(),
            query_vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            threshold: Some(0.8),
            metric: "cosine".to_string(),
        };

        assert_eq!(pushdown.collection, "vectors");
        assert_eq!(pushdown.top_k, 10);
        assert_eq!(pushdown.metric, "cosine");
    }

    #[test]
    fn test_graph_traversal_pushdown() {
        let pushdown = GraphTraversalPushdown {
            graph: "knowledge_graph".to_string(),
            start_nodes: vec!["node_1".to_string(), "node_2".to_string()],
            edge_types: vec!["RELATED_TO".to_string()],
            direction: "outbound".to_string(),
            max_depth: 3,
        };

        assert_eq!(pushdown.graph, "knowledge_graph");
        assert_eq!(pushdown.start_nodes.len(), 2);
        assert_eq!(pushdown.max_depth, 3);
    }

    #[test]
    fn test_write_result() {
        let result = WriteResult {
            rows_written: 1000,
            bytes_written: 102400,
            files_created: vec![
                "part-00000.parquet".to_string(),
                "part-00001.parquet".to_string(),
            ],
            partitions_written: None,
            latency_us: Some(1500),
            transaction_id: None,
            commit_timestamp: None,
            version: Some(1),
        };

        assert_eq!(result.rows_written, 1000);
        assert_eq!(result.files_created.len(), 2);
    }
}
