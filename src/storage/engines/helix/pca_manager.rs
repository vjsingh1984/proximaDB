//! PCA Model Manager for HELIX engine
//!
//! This module re-exports the shared PCA manager from the core infrastructure.
//! The actual implementation is now in `storage::engines::core::pca::manager`.
//!
//! ## Migration Note
//!
//! This module now re-exports from the shared PCA infrastructure to enable
//! code reuse across SST, HELIX, and SWIFT engines. All functionality remains
//! the same - existing code using `helix::pca_manager::*` will continue to
//! work unchanged.

// Re-export all types from the shared PCA manager module
pub use crate::storage::engines::core::pca::manager::{
    DriftMetrics, InMemoryPCAManager, ModelVersion, PCAModelManager,
};

// Re-export configuration
pub use crate::storage::engines::core::pca::config::{PCAConfig, PCAManagerConfig};

// Re-export model types for convenience
pub use crate::storage::engines::core::pca::model::{EnhancedPCAModel, ModelQuality};

// Legacy alias for backward compatibility with existing HELIX code
pub use PCAModelManager as EnhancedPCAManager;
