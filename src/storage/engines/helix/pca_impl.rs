//! PCA implementation for HELIX engine
//!
//! This module re-exports the shared PCA model from the core infrastructure.
//! The actual implementation is now in `storage::engines::core::pca::model`.
//!
//! ## Migration Note
//!
//! This module now re-exports from the shared PCA infrastructure to enable
//! code reuse across SST, HELIX, and SWIFT engines. All functionality remains
//! the same - existing code using `helix::pca_impl::EnhancedPCAModel` will
//! continue to work unchanged.

// Re-export the shared PCA model for backward compatibility
pub use crate::storage::engines::core::pca::model::{EnhancedPCAModel, ModelQuality};

// Re-export the in-memory manager for backward compatibility
pub use crate::storage::engines::core::pca::manager::InMemoryPCAManager as PCAModelManager;
