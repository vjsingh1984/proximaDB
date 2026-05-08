//! # Query Execution Strategies
//!
//! This module provides real implementations of `QueryStrategy` that wrap
//! the existing query execution systems, routing queries through the unified facade.
//!
//! ## Available Strategies
//!
//! - `VectorSearchStrategy`: Wraps `VectorOpsService` for vector similarity search
//! - `SqlStrategy`: Wraps `FederatedQueryContext` for SQL queries
//! - `GraphStrategy`: Wraps the extracted graph read/query contract for declarative graph queries
//! - `ColumnarStrategy`: Uses `ColumnarReadProvider` for analytical queries
//! - `DocumentStrategy`: Wraps `DocumentService` for JSON document queries
//! - `ObservabilityStrategy`: Wraps `ObservabilityQueryEngine` for logs/metrics/traces

pub mod columnar;
pub mod distributed;
pub mod document;
pub mod external_table;
pub mod graph;
pub mod observability;
pub mod sql;
pub mod vector;

pub use columnar::{ColumnarStrategy, ColumnarStrategyConfig};
pub use distributed::{DistributedQueryStats, DistributedQueryStrategy, DistributedStrategyConfig};
pub use document::DocumentStrategy;
pub use external_table::{ExternalPredicatePushdown, ExternalTableScanner, ExternalTableStrategy};
pub use graph::GraphStrategy;
pub use observability::ObservabilityStrategy;
pub use sql::SqlStrategy;
pub use vector::VectorSearchStrategy;
