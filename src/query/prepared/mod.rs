//! # Prepared Statements Module
//!
//! Provides prepared statement support for ProximaDB, enabling the "parse once, execute many"
//! pattern that is essential for high-performance agentic AI workloads.
//!
//! ## Key Features
//!
//! - **Thread-safe statement cache**: Uses DashMap for concurrent access
//! - **Parse once, execute many**: Avoids repeated parsing overhead
//! - **Parameter binding**: Safe query execution with bound parameters
//! - **TTL-based expiration**: Automatic cleanup of unused statements
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                PreparedStatementCache                        │
//! │  ┌─────────────────────────────────────────────────────┐    │
//! │  │           DashMap<StatementId, CachedStatement>     │    │
//! │  │                                                      │    │
//! │  │  id_1: {parsed_query, optimized_plan, bindings, ttl}│    │
//! │  │  id_2: {parsed_query, optimized_plan, bindings, ttl}│    │
//! │  │  ...                                                 │    │
//! │  └─────────────────────────────────────────────────────┘    │
//! │                                                              │
//! │  Background cleanup task (removes expired statements)        │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ### REST API
//!
//! ```bash
//! # Prepare a statement
//! curl -X POST http://localhost:5678/api/v1/unified/prepare \
//!   -H "Content-Type: application/json" \
//!   -d '{"sql": "SELECT * FROM VECTOR_SEARCH($1, $2, 10)"}'
//!
//! # Execute prepared statement
//! curl -X POST http://localhost:5678/api/v1/unified/execute/stmt_abc123 \
//!   -H "Content-Type: application/json" \
//!   -d '{"params": ["embeddings", "[0.1, 0.2, 0.3]"]}'
//!
//! # Delete prepared statement
//! curl -X DELETE http://localhost:5678/api/v1/unified/prepared/stmt_abc123
//! ```
//!
//! ### Embedded API (Rust)
//!
//! ```rust,ignore
//! let db = EmbeddedProximaDB::new(config)?;
//!
//! // Prepare a statement
//! let stmt_id = db.prepare_statement("SELECT * FROM VECTOR_SEARCH($1, $2, 10)")?;
//!
//! // Execute with different parameters
//! let results1 = db.execute_prepared(&stmt_id, &["embeddings", "[0.1, 0.2]"])?;
//! let results2 = db.execute_prepared(&stmt_id, &["products", "[0.3, 0.4]"])?;
//!
//! // Cleanup
//! db.drop_prepared(&stmt_id)?;
//! ```
//!
//! ## Performance Benefits
//!
//! | Operation | Without Prepared | With Prepared |
//! |-----------|------------------|---------------|
//! | Parse     | ~1ms per query   | Once only     |
//! | Optimize  | ~2-5ms per query | Once only     |
//! | Execute   | Full pipeline    | Execute only  |
//!
//! For agentic AI workloads with repetitive query patterns, prepared statements
//! can provide 5-10x speedup.

pub mod statement;

// Re-export main types
pub use statement::{
    CachedStatement, ParameterBinding, ParameterValue, PreparedStatement, PreparedStatementCache,
    PreparedStatementCacheStats, PreparedStatementConfig, PreparedStatementError,
    PreparedStatementId,
};
