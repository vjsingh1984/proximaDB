//! # Multi-Model Store Implementations
//!
//! This module contains specialized store wrappers that combine storage engines
//! for optimal performance per data model.

pub mod document_store;
pub mod graph_store;
pub mod observability_store;
pub mod rdbms_store;
pub mod vector_store;

// Re-exports - Stores
pub use document_store::DocumentStore;
pub use graph_store::GraphStore;
pub use observability_store::ObservabilityStore;
pub use rdbms_store::RDBMSStore;
pub use vector_store::VectorStore;

// Re-exports - Configs
pub use document_store::{DocumentStoreConfig, SchemaValidationMode};
pub use graph_store::GraphStoreConfig;
pub use observability_store::ObservabilityStoreConfig;
pub use rdbms_store::{QueryType, RDBMSStoreConfig};
pub use vector_store::{QuantizationType, VectorStoreConfig};
