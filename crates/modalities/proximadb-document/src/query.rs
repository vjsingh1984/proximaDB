//! # Document Query Expressions
//!
//! This module provides document query APIs for JSON document search.
//!
//! ## Query Types
//!
//! - **JSON Path Filters** - Path-based filtering (e.g., `$.user.name == "John"`)
//! - **Full-Text Search** - Text search across document fields
//! - **Aggregation** - Document aggregation operations
//!
//! ## Transitional Note
//!
//! This module currently re-exports from `proximadb-document-query` during transition.
//! The implementations will be consolidated here in future iterations.

// Re-export from document-query crate during transition
pub use proximadb_document_query::{DocumentQueryExpr, DocumentSort, PathFilter};

// Additional document-specific types will be added here as extraction continues

// TODO: Move these from src/storage/document/query
// pub mod aggregation;
// pub mod fulltext;
