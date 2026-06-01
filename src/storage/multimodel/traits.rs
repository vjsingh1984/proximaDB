//! # Multi-Model Storage Traits
//!
//! Defines the core traits for the unified multi-model storage system.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::graph::engines::GraphEngine;
use crate::storage::traits::{
    DocumentStorageOperations, MultiModelStats, ObservabilityStorageOperations,
    UnifiedStorageFormat,
};

/// Model type discriminator for routing operations — alias for DataModel.
pub type ModelType = proximadb_data_model::DataModel;

/// Capabilities of a store
#[derive(Debug, Clone)]
pub struct StoreCapabilities {
    /// Model type this store handles
    pub model_type: ModelType,
    /// Whether the store supports transactions
    pub supports_transactions: bool,
    /// Whether the store supports secondary indexes
    pub supports_secondary_indexes: bool,
    /// Whether the store supports ACID guarantees
    pub supports_acid: bool,
    /// Whether the store supports streaming reads
    pub supports_streaming: bool,
    /// Maximum recommended record count
    pub max_recommended_records: Option<u64>,
    /// Description of the store
    pub description: String,
}

/// Unified multi-model storage engine trait
///
/// This trait combines all model-specific operations into a single interface,
/// allowing the unified query engine to route operations to appropriate stores.
#[async_trait]
pub trait MultiModelStorageEngine: Send + Sync {
    // ======================
    // Store Access
    // ======================

    /// Get the vector store for embedding operations
    fn vector_store(&self) -> Option<Arc<dyn UnifiedStorageFormat>>;

    /// Get the document store for JSON document operations
    fn document_store(&self) -> Option<Arc<dyn DocumentStorageOperations>>;

    /// Get the graph store for node/edge operations
    fn graph_store(&self) -> Option<Arc<dyn GraphEngine>>;

    /// Get the observability store for logs/metrics/traces
    fn observability_store(&self) -> Option<Arc<dyn ObservabilityStorageOperations>>;

    /// Get the RDBMS store for relational operations
    fn rdbms_store(&self) -> Option<Arc<dyn UnifiedStorageFormat>>;

    // ======================
    // Capabilities
    // ======================

    /// Get supported model types
    fn supported_models(&self) -> Vec<ModelType>;

    /// Get capabilities for a specific model type
    fn get_capabilities(&self, model_type: ModelType) -> Option<StoreCapabilities>;

    /// Check if a model type is supported
    fn supports_model(&self, model_type: ModelType) -> bool {
        self.supported_models().contains(&model_type)
    }

    // ======================
    // Statistics
    // ======================

    /// Get unified statistics across all models
    async fn get_multi_model_stats(&self) -> Result<MultiModelStats>;

    /// Get storage size by model type
    async fn get_storage_size(&self, model_type: ModelType) -> Result<u64>;

    // ======================
    // Lifecycle
    // ======================

    /// Flush all stores
    async fn flush_all(&self) -> Result<()>;

    /// Compact all stores
    async fn compact_all(&self) -> Result<()>;

    /// Stop all stores gracefully
    async fn stop(&self) -> Result<()>;
}

/// Builder for MultiModelStorageEngine
pub struct MultiModelStorageEngineBuilder {
    vector_store: Option<Arc<dyn UnifiedStorageFormat>>,
    document_store: Option<Arc<dyn DocumentStorageOperations>>,
    graph_store: Option<Arc<dyn GraphEngine>>,
    observability_store: Option<Arc<dyn ObservabilityStorageOperations>>,
    rdbms_store: Option<Arc<dyn UnifiedStorageFormat>>,
}

impl MultiModelStorageEngineBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            vector_store: None,
            document_store: None,
            graph_store: None,
            observability_store: None,
            rdbms_store: None,
        }
    }

    /// Set the vector store
    pub fn with_vector_store(mut self, store: Arc<dyn UnifiedStorageFormat>) -> Self {
        self.vector_store = Some(store);
        self
    }

    /// Set the document store
    pub fn with_document_store(mut self, store: Arc<dyn DocumentStorageOperations>) -> Self {
        self.document_store = Some(store);
        self
    }

    /// Set the graph store
    pub fn with_graph_store(mut self, store: Arc<dyn GraphEngine>) -> Self {
        self.graph_store = Some(store);
        self
    }

    /// Set the observability store
    pub fn with_observability_store(
        mut self,
        store: Arc<dyn ObservabilityStorageOperations>,
    ) -> Self {
        self.observability_store = Some(store);
        self
    }

    /// Set the RDBMS store
    pub fn with_rdbms_store(mut self, store: Arc<dyn UnifiedStorageFormat>) -> Self {
        self.rdbms_store = Some(store);
        self
    }

    /// Build the storage engine configuration
    pub fn build(self) -> MultiModelStorageConfig {
        MultiModelStorageConfig {
            vector_store: self.vector_store,
            document_store: self.document_store,
            graph_store: self.graph_store,
            observability_store: self.observability_store,
            rdbms_store: self.rdbms_store,
        }
    }
}

impl Default for MultiModelStorageEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for multi-model storage
pub struct MultiModelStorageConfig {
    pub vector_store: Option<Arc<dyn UnifiedStorageFormat>>,
    pub document_store: Option<Arc<dyn DocumentStorageOperations>>,
    pub graph_store: Option<Arc<dyn GraphEngine>>,
    pub observability_store: Option<Arc<dyn ObservabilityStorageOperations>>,
    pub rdbms_store: Option<Arc<dyn UnifiedStorageFormat>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::traits::DataModel;

    #[test]
    fn test_model_type_is_data_model() {
        // ModelType and DataModel share the canonical data-model enum.
        let mt: ModelType = ModelType::Vector;
        let dm: DataModel = mt; // zero-cost: same type
        assert_eq!(dm, DataModel::Vector);
        assert_eq!(dm, ModelType::Vector);
    }

    #[test]
    fn test_all_variants_accessible() {
        let variants = [
            ModelType::Vector,
            ModelType::Document,
            ModelType::Graph,
            ModelType::Relational,
            ModelType::Observability,
            ModelType::TimeSeries,
            ModelType::Event,
        ];
        assert_eq!(variants.len(), 7);
    }
}
