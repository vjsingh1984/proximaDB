//! # ProximaDB Document Modality
//!
//! This crate contains document storage, full-text search, and JSON document
//! operations for the ProximaDB vector database.
//!
//! ## Architecture
//!
//! The document modality is organized into several key modules:
//!
//! - **`query`** - Document query expressions (JSON path filters, full-text search)
//! - **`index`** - Full-text indexing (Tantivy-based)
//! - **`storage`** - Document storage and retrieval
//!
//! ## Foundation
//!
//! This crate serves as the foundation for document operations across ProximaDB,
//! providing reusable contracts and implementations for:
//!
//! - Storage engines that need document similarity search
//! - Query executors that need document operations
//! - Index builders that need full-text search
//!
//! ## Dependencies
//!
//! - `proximadb-kernel` - Core error types and foundational contracts
//! - `proximadb-proto` - Protocol buffer types
//! - `proximadb-query-filter` - Filter expression contracts
//! - `arrow` - Columnar data structures for document operations

pub mod query;

// Re-export common types for convenience
pub use query::{DocumentQueryExpr, DocumentSort, PathFilter};

// TODO: Move these from src/storage/document and src/schema/document
// pub mod index;
// pub mod storage;
// pub mod aggregation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_module_imports() {
        // Basic test to verify the module structure is working
        let _expr = DocumentQueryExpr {
            collection: "test".to_string(),
            path_filters: vec![],
            text_search: None,
            projection: vec![],
            sort: None,
            limit: None,
        };
        // More comprehensive tests will be added as modules are extracted
    }
}
