//! # Materialized Views Module
//!
//! Provides materialized view support for ProximaDB, enabling precomputed query results
//! for complex dashboard queries and cross-model joins.
//!
//! ## Key Features
//!
//! - **Precomputed Query Results**: Store and serve complex query results efficiently
//! - **Multiple Refresh Strategies**: Manual, Periodic, and On-Change refresh modes
//! - **Cross-Model Support**: Materialize results from VECTOR_SEARCH, GRAPH_QUERY, etc.
//! - **Catalog Integration**: Persist MV definitions to the internal schema registry
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                MaterializedViewManager                          │
//! │  ┌─────────────────────────────────────────────────────────┐    │
//! │  │         DashMap<String, MaterializedView>                │    │
//! │  │                                                          │    │
//! │  │  "user_matches": {query, strategy, schema, data}        │    │
//! │  │  "product_recs": {query, strategy, schema, data}        │    │
//! │  │  ...                                                     │    │
//! │  └─────────────────────────────────────────────────────────┘    │
//! │                                                                  │
//! │  ┌─────────────────────────────────────────────────────────┐    │
//! │  │              RefreshScheduler                             │    │
//! │  │  - Periodic refresh (cron-like scheduling)               │    │
//! │  │  - On-change triggers (CDC integration)                  │    │
//! │  │  - Debouncing for high-frequency changes                 │    │
//! │  └─────────────────────────────────────────────────────────┘    │
//! │                                                                  │
//! │  MaterializedViewStorage (catalog persistence)                  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## SQL Syntax
//!
//! ```sql
//! -- Create a materialized view with periodic refresh
//! CREATE MATERIALIZED VIEW user_product_matches AS
//! SELECT u.id, v.product_id, v.score
//! FROM users u
//! JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 100) v ON true
//! WITH REFRESH PERIODIC INTERVAL '1 hour';
//!
//! -- Create a materialized view with manual refresh
//! CREATE MATERIALIZED VIEW top_products AS
//! SELECT * FROM VECTOR_SEARCH('products', '[0.1,0.2,0.3]', 100)
//! WITH REFRESH MANUAL;
//!
//! -- Create a materialized view with on-change refresh
//! CREATE MATERIALIZED VIEW user_graph AS
//! SELECT * FROM GRAPH_QUERY('MATCH (u:User)-[:KNOWS]->(f) RETURN u, f')
//! WITH REFRESH ON CHANGE DEBOUNCE '5 seconds';
//!
//! -- Refresh a materialized view
//! REFRESH MATERIALIZED VIEW user_product_matches;
//!
//! -- Drop a materialized view
//! DROP MATERIALIZED VIEW user_product_matches;
//! ```
//!
//! ## Usage
//!
//! ### Rust API
//!
//! ```rust,ignore
//! use proximadb::query::materialized_view::{
//!     MaterializedViewManager, MaterializedViewDefinition, RefreshStrategy
//! };
//! use std::time::Duration;
//!
//! // Create manager
//! let manager = MaterializedViewManager::new(catalog)?;
//!
//! // Create a materialized view
//! let definition = MaterializedViewDefinition::new(
//!     "user_matches",
//!     "SELECT u.id, v.product_id FROM users u JOIN LATERAL VECTOR_SEARCH(...) v ON true"
//! )
//! .with_refresh_strategy(RefreshStrategy::Periodic {
//!     interval: Duration::from_secs(3600),
//! });
//!
//! manager.create(definition).await?;
//!
//! // Manually refresh
//! manager.refresh("user_matches").await?;
//!
//! // Query the materialized view (uses cached results)
//! let results = manager.query("user_matches", Some("user_id = 'u1'")).await?;
//!
//! // Drop the view
//! manager.drop("user_matches").await?;
//! ```
//!
//! ## Performance Benefits
//!
//! | Query Type | Without MV | With MV (cached) |
//! |------------|------------|------------------|
//! | Vector Search | 10-100ms | <1ms |
//! | Graph Traversal | 50-500ms | <1ms |
//! | Cross-Model Join | 100-1000ms | <1ms |
//!
//! Materialized views are particularly beneficial for:
//! - Dashboard queries with complex aggregations
//! - Cross-model joins (vector + graph + document)
//! - AI/ML feature serving with predictable latency
//! - High-concurrency read workloads

pub mod definition;
pub mod refresh;

// Re-export main types
pub use definition::{
    ColumnDef, MaterializedView, MaterializedViewConfig, MaterializedViewDefinition,
    MaterializedViewError, MaterializedViewId, MaterializedViewParser, MaterializedViewResult,
    MaterializedViewState, MaterializedViewStatement, MaterializedViewStats,
};

pub use refresh::{
    RefreshContext, RefreshEvent, RefreshEventType, RefreshResult, RefreshScheduler,
    RefreshStrategy,
};
