//! # Vector Store
//!
//! Combines HELIX (high-dimensional vectors with Hilbert curve locality) and
//! SST (write-optimized real-time) engines for optimal vector storage.
//!
//! ## Engine Selection Strategy
//!
//! - **High-dimensional vectors (>512D)**: Route to HELIX for Hilbert curve clustering
//! - **Real-time/low-latency writes**: Route to SST for fast memtable access
//! - **Mixed workloads**: Use SST as hot tier, HELIX as warm/cold tier

use std::sync::Arc;

use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::storage::traits::UnifiedStorageEngine;
use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
use crate::index::axis::AxisManager;

use super::super::traits::{ModelType, StoreCapabilities};

/// Configuration for the vector store
#[derive(Debug, Clone)]
pub struct VectorStoreConfig {
    /// Dimension threshold for routing to HELIX vs SST
    pub dimension_threshold: usize,
    /// Maximum vectors before auto-flush
    pub max_vectors_in_memory: usize,
    /// Enable quantization for storage efficiency
    pub enable_quantization: bool,
    /// Default quantization type
    pub default_quantization: QuantizationType,
}

/// Quantization type for vector compression
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationType {
    /// No quantization (full precision)
    None,
    /// INT8 scalar quantization
    Int8,
    /// Binary quantization
    Binary,
    /// Product quantization
    ProductQuantization,
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        Self {
            dimension_threshold: 512,
            max_vectors_in_memory: 100_000,
            enable_quantization: true,
            default_quantization: QuantizationType::Int8,
        }
    }
}

/// VectorStore combines HELIX and SST engines for optimal vector storage
///
/// ## Architecture
///
/// ```text
/// ┌─────────────────────────────────────────┐
/// │            VectorStore                   │
/// │  ┌─────────────────────────────────────┐│
/// │  │         Router                       ││
/// │  │  - Dimension-based routing          ││
/// │  │  - Latency requirements             ││
/// │  └─────────────────────────────────────┘│
/// │              │                │          │
/// │    ┌─────────▼───────┐ ┌─────▼────────┐ │
/// │    │   SST Engine    │ │ HELIX Engine │ │
/// │    │  (Hot Tier)     │ │ (Warm/Cold)  │ │
/// │    │  - Real-time    │ │ - High-dim   │ │
/// │    │  - Low-latency  │ │ - Hilbert    │ │
/// │    └─────────────────┘ └──────────────┘ │
/// └─────────────────────────────────────────┘
/// ```
pub struct VectorStore {
    /// Primary engine for high-dimensional vectors (HELIX)
    helix_engine: Option<Arc<dyn UnifiedStorageEngine>>,
    /// Hot tier engine for real-time vectors (SST)
    sst_engine: Option<Arc<dyn UnifiedStorageEngine>>,
    /// Dimension threshold for engine routing
    dimension_threshold: usize,
    /// Shared quantization engine
    quantizer: Option<Arc<UnifiedQuantizationEngine>>,
    /// Index manager for HNSW/IVF indexes
    index_manager: Option<Arc<AxisManager>>,
    /// Cache orchestrator
    cache_orchestrator: Option<Arc<CrossCacheOrchestrator>>,
    /// Configuration
    config: VectorStoreConfig,
}

impl VectorStore {
    /// Create a new VectorStore with the given configuration
    pub fn new(config: VectorStoreConfig) -> Self {
        Self {
            helix_engine: None,
            sst_engine: None,
            dimension_threshold: config.dimension_threshold,
            quantizer: None,
            index_manager: None,
            cache_orchestrator: None,
            config,
        }
    }

    /// Set the HELIX engine for high-dimensional vectors
    pub fn with_helix_engine(mut self, engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        self.helix_engine = Some(engine);
        self
    }

    /// Set the SST engine for real-time operations
    pub fn with_sst_engine(mut self, engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        self.sst_engine = Some(engine);
        self
    }

    /// Create a VectorStore with a single engine (used for federated query integration)
    ///
    /// This is a convenience constructor that sets the given engine as the SST engine,
    /// making it available as the primary engine for vector operations in federated queries.
    pub fn with_engine(engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        Self::new(VectorStoreConfig::default()).with_sst_engine(engine)
    }

    /// Set the quantization engine
    pub fn with_quantizer(mut self, quantizer: Arc<UnifiedQuantizationEngine>) -> Self {
        self.quantizer = Some(quantizer);
        self
    }

    /// Set the index manager
    pub fn with_index_manager(mut self, manager: Arc<AxisManager>) -> Self {
        self.index_manager = Some(manager);
        self
    }

    /// Set the cache orchestrator
    pub fn with_cache(mut self, cache: Arc<CrossCacheOrchestrator>) -> Self {
        self.cache_orchestrator = Some(cache);
        self
    }

    /// Get store capabilities
    pub fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            model_type: ModelType::Vector,
            supports_transactions: false,
            supports_secondary_indexes: true, // AXIS indexes
            supports_acid: false,
            supports_streaming: true,
            max_recommended_records: Some(100_000_000), // 100M vectors
            description: "Vector embeddings storage with HELIX (high-dim) + SST (real-time)".to_string(),
        }
    }

    /// Route to appropriate engine based on vector dimension
    pub fn route_engine(&self, dimension: usize) -> Option<&Arc<dyn UnifiedStorageEngine>> {
        if dimension > self.dimension_threshold {
            // High-dimensional: prefer HELIX
            self.helix_engine.as_ref().or(self.sst_engine.as_ref())
        } else {
            // Low-dimensional or real-time: prefer SST
            self.sst_engine.as_ref().or(self.helix_engine.as_ref())
        }
    }

    /// Get the primary engine (SST for writes, HELIX for high-dim)
    pub fn primary_engine(&self) -> Option<&Arc<dyn UnifiedStorageEngine>> {
        self.sst_engine.as_ref().or(self.helix_engine.as_ref())
    }

    /// Get the SST engine directly
    pub fn sst_engine(&self) -> Option<&Arc<dyn UnifiedStorageEngine>> {
        self.sst_engine.as_ref()
    }

    /// Get the HELIX engine directly
    pub fn helix_engine(&self) -> Option<&Arc<dyn UnifiedStorageEngine>> {
        self.helix_engine.as_ref()
    }

    /// Get the quantizer
    pub fn quantizer(&self) -> Option<&Arc<UnifiedQuantizationEngine>> {
        self.quantizer.as_ref()
    }

    /// Get the index manager
    pub fn index_manager(&self) -> Option<&Arc<AxisManager>> {
        self.index_manager.as_ref()
    }

    /// Get the cache orchestrator
    pub fn cache_orchestrator(&self) -> Option<&Arc<CrossCacheOrchestrator>> {
        self.cache_orchestrator.as_ref()
    }

    /// Get configuration
    pub fn config(&self) -> &VectorStoreConfig {
        &self.config
    }

    /// Check if store is operational
    pub fn is_operational(&self) -> bool {
        self.sst_engine.is_some() || self.helix_engine.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_store_config_default() {
        let config = VectorStoreConfig::default();
        assert_eq!(config.dimension_threshold, 512);
        assert_eq!(config.max_vectors_in_memory, 100_000);
        assert!(config.enable_quantization);
    }

    #[test]
    fn test_vector_store_capabilities() {
        let store = VectorStore::new(VectorStoreConfig::default());
        let caps = store.capabilities();

        assert_eq!(caps.model_type, ModelType::Vector);
        assert!(caps.supports_secondary_indexes);
        assert!(caps.supports_streaming);
    }
}
