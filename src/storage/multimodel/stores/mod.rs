//! # Multi-Model Store Implementations
//!
//! This module contains specialized store wrappers that combine storage engines
//! for optimal performance per data model.

pub mod vector_store;
pub mod document_store;
pub mod graph_store;
pub mod rdbms_store;
pub mod observability_store;

// Re-exports - Stores
pub use vector_store::VectorStore;
pub use document_store::DocumentStore;
pub use graph_store::GraphStore;
pub use rdbms_store::RDBMSStore;
pub use observability_store::ObservabilityStore;

// Re-exports - Configs
pub use vector_store::{VectorStoreConfig, QuantizationType};
pub use document_store::{DocumentStoreConfig, SchemaValidationMode};
pub use graph_store::GraphStoreConfig;
pub use rdbms_store::{RDBMSStoreConfig, QueryType};
pub use observability_store::ObservabilityStoreConfig;
