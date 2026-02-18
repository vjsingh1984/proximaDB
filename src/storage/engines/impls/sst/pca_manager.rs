//! PCA Model Manager for SST engine
//!
//! This module re-exports the shared PCA manager from the core infrastructure.
//! The actual implementation is now in `storage::engines::core::pca::manager`.
//!
//! ## Usage for SST Engine
//!
//! SST uses PCA for Z-Order spatial encoding during block pruning.
//! The PCA model is:
//! - Trained during flush (on new vectors)
//! - Persisted to `{collection_dir}/__model/pca_model.bin`
//! - Loaded once at search initialization
//! - Reused for all subsequent queries
//!
//! This eliminates the 40ms+ per-query PCA training overhead.

// Re-export all types from the shared PCA manager module
pub use crate::storage::engines::core::pca::manager::{
    DriftMetrics, InMemoryPCAManager, ModelVersion, PCAModelManager,
};

// Re-export configuration
pub use crate::storage::engines::core::pca::config::{PCAConfig, PCAManagerConfig};

// Re-export model types for convenience
pub use crate::storage::engines::core::pca::model::{EnhancedPCAModel, ModelQuality};

// Legacy alias for backward compatibility
pub use PCAModelManager as SstPCAManager;

// Re-export spatial clustering types for Z-Order encoding
pub use crate::storage::engines::core::formats::proximablocks::spatial_clustering::{
    AdaptivePcaConfig, ZOrderEncoder,
};
