//! # Query Module - SQL and Vector Search Engine
//!
//! This module provides ProximaDB's sophisticated query processing engine with support
//! for both SQL queries and native vector searches. It implements intelligent query
//! optimization, predicate pushdown, and adaptive execution strategies.
//!
//! ## Role in ProximaDB Architecture
//!
//! The query engine orchestrates search execution:
//! ```text
//! Query Request (SQL or Vector)
//!           ↓
//!    Query Parser & Analyzer (sql_frontend)
//!           ↓
//!    Query Optimizer (Cost-Based)
//!           ↓
//! ┌─────────────────────────────────┐
//! │    Execution Plan Selection      │
//! ├─────────────────────────────────┤
//! │ Index │ Storage │ Compute │ Cache│
//! └─────────────────────────────────┘
//!           ↓
//!    Parallel Execution
//!           ↓
//!    Result Aggregation
//! ```
//!
//! ## Key Features
//!
//! ### 1. **SQL Support**
//! Full SQL interface for vector operations:
//! ```sql
//! -- Create collection with schema
//! CREATE COLLECTION products (
//!     id TEXT PRIMARY KEY,
//!     embedding VECTOR(384),
//!     category TEXT,
//!     price FLOAT
//! );
//!
//! -- Vector similarity search with filters
//! SELECT id, category, COSINE_DISTANCE(embedding, [0.1, 0.2, ...]) as score
//! FROM products
//! WHERE category = 'electronics' AND price < 1000
//! ORDER BY score DESC
//! LIMIT 10;
//! ```
//!
//! ### 2. **Query Optimization**
//! Cost-based optimizer with multiple strategies:
//! - **Predicate Pushdown**: Filter at storage level
//! - **Index Selection**: Choose optimal index (HNSW, IVF, etc.)
//! - **Join Reordering**: Optimize multi-collection queries
//! - **Parallel Execution**: Distribute across cores
//!
//! ### 3. **Hybrid Search**
//! Combine vector and metadata filtering:
//! - Pre-filtering: Apply metadata filters before vector search
//! - Post-filtering: Apply filters after similarity computation
//! - Adaptive: Choose strategy based on selectivity
//!
//! ### 4. **Execution Strategies**
//! Multiple execution paths for different workloads:
//! - **Index-First**: Use AXIS index for fast approximate search
//! - **Storage-First**: Scan storage with filters
//! - **Hybrid**: Combine index and storage scans
//! - **Cached**: Use cached results when available
//!
//! ## Performance Characteristics
//!
//! - **SQL Parsing**: < 1ms for typical queries
//! - **Optimization Time**: 1-5ms for plan generation
//! - **Vector Search**: < 10ms for 1M vectors (indexed)
//! - **Metadata Filtering**: 100K+ filters/sec
//! - **Result Aggregation**: Near-zero overhead with streaming
//!
//! ## Module Organization
//!
//! - **`sql_frontend/`**: SQL parsing and lowering to internal AST.
//! - **`execution/`**: Unified execution engine for executing lowered AST.
//! - **`vector_search/`**: Native vector search
//!   - `query.rs`: Search query structures
//!   - `executor.rs`: Search execution
//!   - `algorithms.rs`: Search algorithms
//!
//! - **`unified_query_optimizer/`**: Query optimization
//!   - `cost_model.rs`: Cost estimation
//!   - `statistics.rs`: Collection statistics
//!   - `rules.rs`: Optimization rules
//!   - `plan.rs`: Execution plan structures
//!
//! ## SQL Extensions for Vectors
//!
//! ProximaDB extends SQL with vector operations:
//!
//! | Function | Description | Example |
//! |----------|-------------|---------|
//! | `VECTOR(n)` | Vector column type | `embedding VECTOR(384)` |
//! | `COSINE_DISTANCE` | Cosine similarity | `COSINE_DISTANCE(v1, v2)` |
//! | `EUCLIDEAN_DISTANCE` | L2 distance | `EUCLIDEAN_DISTANCE(v1, v2)` |
//! | `DOT_PRODUCT` | Inner product | `DOT_PRODUCT(v1, v2)` |
//! | `VECTOR_DIMS` | Get dimensions | `VECTOR_DIMS(embedding)` |
//! | `VECTOR_NORM` | L2 norm | `VECTOR_NORM(embedding)` |
//!
//! ## Query Optimization Rules
//!
//! The optimizer applies these rules:
//! 1. **Filter Selectivity**: Push selective filters down
//! 2. **Index Availability**: Use indexes when beneficial
//! 3. **Data Locality**: Minimize data movement
//! 4. **Parallelism**: Distribute work across cores
//! 5. **Memory Budget**: Stay within memory limits
//!

pub mod ast;
pub mod execution; // New unified execution engine
pub mod explain;
pub mod rl_planner; // RL-based adaptive query planner
pub mod semantic_analysis;
pub mod sql_frontend;
pub mod unified_query_optimizer;
pub mod vector_search;

#[cfg(test)]
pub mod tests;

// Re-export main types
pub use unified_query_optimizer::{
    UnifiedCostWeights, UnifiedExecutionPlan as QueryPlan, UnifiedMetadataFilter as MetadataFilter,
    UnifiedOptimizerConfig, UnifiedQueryOptimizer as QueryOptimizer,
};
pub use vector_search::{SearchParameters, VectorSearchQuery, VectorSearchResult};
