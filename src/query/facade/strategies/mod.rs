//! # Query Execution Strategies
//!
//! This module provides real implementations of `QueryStrategy` that wrap
//! the existing query execution systems, routing queries through the unified facade.
//!
//! ## Available Strategies
//!
//! - `VectorSearchStrategy`: Wraps `VectorOpsService` for vector similarity search
//! - `SqlStrategy`: Wraps `FederatedQueryContext` for SQL queries
//! - `GraphStrategy`: Wraps `GraphOperationsService` for graph traversals
//! - `ColumnarStrategy`: Uses `ColumnarReadProvider` for analytical queries

pub mod columnar;
pub mod graph;
pub mod sql;
pub mod vector;

pub use columnar::ColumnarStrategy;
pub use graph::GraphStrategy;
pub use sql::SqlStrategy;
pub use vector::VectorSearchStrategy;
