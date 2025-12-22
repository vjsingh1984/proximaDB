//! Shared PCA infrastructure for spatial clustering
//!
//! This module provides per-collection PCA model management for all storage
//! engines (SST, HELIX, SWIFT). PCA is used to reduce high-dimensional vectors
//! to lower dimensions before applying spatial curve encoding (Z-order, Hilbert,
//! or AdaCurve).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              SpatialClusteringPipeline                       │
//! │  Combines PCA + Spatial Encoding for flush operations        │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    PCA Model Manager                         │
//! │  Per-collection model stored at {collection}/__model/        │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!          ┌───────────────────┼───────────────────┐
//!          ▼                   ▼                   ▼
//!    ┌─────────┐         ┌─────────┐         ┌─────────┐
//!    │   SST   │         │  HELIX  │         │  SWIFT  │
//!    │ Z-Order │         │ Hilbert │         │AdaCurve │
//!    └─────────┘         └─────────┘         └─────────┘
//! ```
//!
//! # Usage
//!
//! ## Spatial Clustering Pipeline (Recommended for Flush)
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::pca::{
//!     SpatialClusteringPipeline, ClusteringConfig, BlockInfo,
//! };
//! use proximadb::storage::engines::core::formats::proximablocks::spatial_traits::CurveType;
//!
//! // Create pipeline for SST (Z-order)
//! let config = ClusteringConfig::for_engine(CurveType::ZOrder);
//! let pipeline = SpatialClusteringPipeline::new(
//!     config, filesystem, "my_collection".into(), collection_dir
//! ).await?;
//!
//! // Train model if enough vectors
//! pipeline.train_model(&vectors).await?;
//!
//! // During flush, cluster blocks
//! let mut blocks: Vec<BlockInfo> = create_blocks(&vectors);
//! let result = pipeline.cluster_blocks(&mut blocks).await?;
//!
//! // Write blocks in clustered order
//! for idx in result.sorted_indices {
//!     write_block(&blocks[idx], result.spatial_codes[idx].clone());
//! }
//! ```
//!
//! ## With PCA Manager Directly
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::pca::{PCAManagerConfig, PCAModelManager};
//!
//! let config = PCAManagerConfig::default();
//! let manager = PCAModelManager::new(
//!     "my_collection".to_string(),
//!     config,
//!     filesystem,
//!     collection_base_dir,
//! )?;
//!
//! manager.initialize().await?;
//!
//! // Train a model during flush
//! let version = manager.train_and_activate(&vectors, 8).await?;
//!
//! // Project vectors for spatial encoding
//! if let Some(projected) = manager.project(&vector).await? {
//!     let spatial_code = encoder.encode(&projected);
//! }
//! ```
//!
//! ## In-Memory (Testing/Short-lived)
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::pca::InMemoryPCAManager;
//!
//! let mut manager = InMemoryPCAManager::new(3);
//! manager.train_new_model(&records, 4)?;
//!
//! if let Some(projected) = manager.project(&vector)? {
//!     // Use projected vector
//! }
//! ```
//!
//! # Model Lifecycle
//!
//! For WORM (Write-Once-Read-Many) workloads, PCA models are updated only during:
//! - **Flush**: When vectors are flushed from memtable to SSTable
//! - **Compaction**: When SSTables are merged
//!
//! This minimizes model update overhead during normal read operations.

pub mod config;
pub mod manager;
pub mod model;
pub mod pipeline;

// Re-export main types for convenience
pub use config::{PCAConfig, PCAManagerConfig};
pub use manager::{DriftMetrics, InMemoryPCAManager, ModelVersion, PCAModelManager};
pub use model::{EnhancedPCAModel, ModelQuality};
pub use pipeline::{
    cluster_blocks_sync, BlockInfo, ClusteringConfig, ClusteringResult,
    SpatialClusteringPipeline, SyncClusteringResult,
};
