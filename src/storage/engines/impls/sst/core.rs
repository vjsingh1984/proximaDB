/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SST Engine Core Module
//!
//! Contains the main SstEngine struct definition and initialization logic.
//! This module is responsible for:
//! - SstEngine struct definition with all components
//! - Engine construction and configuration
//! - Component initialization and coordination
//! - Core trait implementations

use anyhow::Result;
use std::sync::Arc;
use tracing::info;

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::{
    storage_engine::StorageQuantizationEngine,
    unified::{CodebookStore, InMemoryCodebookStore, UnifiedQuantizationEngine},
};
use crate::storage::engines::core::ops::{
    UniversalOptimizationStrategy, UniversalPerformanceOptimizer,
};
use crate::storage::engines::impls::sst::{
    SstConfig, SstError, compaction::Compaction, decompression_cache, readers::UnifiedSstableReader,
};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::transaction_coordinator::TransactionCoordinator;

/// SST Engine - Hybrid columnar (ProximaBlocks), write-optimized storage with three-stage filtering
///
/// # Architecture Overview
///
/// The SST (Sorted String Table) engine implements an LSM-tree based storage system
/// optimized for OLTP workloads with real-time query requirements. It uses a multi-stage
/// filtering pipeline to minimize I/O and maximize query performance.
///
/// ## Design Principles
///
/// - **Row-Oriented Storage**: Each vector record stored as complete row for fast point queries
/// - **Three-Stage Filtering**: Progressive elimination (bloom → quantized → full precision)
/// - **Write Optimization**: LSM-tree with MemTable → SSTable → Compaction flow
/// - **Singleton Pattern**: Single engine instance handles multiple collections efficiently
/// - **Lock-Free Reads**: Concurrent reads without blocking using Arc-based sharing
///
/// ## Data Flow
///
/// 1. **Write Path**: Records → WAL → MemTable → Flush → L0 SSTables → Background Compaction
/// 2. **Read Path**: Bloom Filter → Quantized Scan → Full Vector Retrieval → Distance Compute
/// 3. **Compaction**: Multi-level merge using transaction coordinator for atomicity
///
/// ## Performance Characteristics
///
/// - **Write Latency**: ~1-5ms (MemTable insertion + WAL)
/// - **Point Query**: ~2-10ms (bloom filter + 1-2 SSTable reads)
/// - **Range Query**: ~10-50ms (multiple SSTable scans with quantization)
/// - **Memory Overhead**: ~100MB base + (MemTable size × num_collections)
///
pub struct SstEngine {
    /// **Engine Configuration**
    ///
    /// Contains tuning parameters for:
    /// - MemTable size thresholds (default: 64MB)
    /// - Compaction strategy (size-tiered or leveled)
    /// - Bloom filter false positive rate (default: 1%)
    /// - Cache sizes and eviction policies
    ///
    /// Loaded from config/config.toml or defaults
    config: SstConfig,

    /// **Compaction Manager** (Optional)
    ///
    /// Background compaction orchestrator that:
    /// - Monitors SSTable count and triggers merges
    /// - Implements size-tiered compaction strategy
    /// - Manages compaction scheduling and resources
    /// - Handles tombstone cleanup and space reclamation
    ///
    /// None during initialization, Some after start_compaction() called
    compaction_manager: Option<Arc<Compaction>>,

    /// **Filesystem Factory**
    ///
    /// Creates filesystem instances for different storage backends:
    /// - Local filesystem (file://)
    /// - S3 (s3://)
    /// - Azure Blob (azure://)
    /// - GCS (gs://)
    ///
    /// Shared across all filesystem operations for consistency
    filesystem: Arc<FilesystemFactory>,

    /// **Unified Caching Filesystem** (Optional)
    ///
    /// Wraps base filesystem with intelligent caching layer:
    /// - Transparent read-through disk cache
    /// - Prefetch engine for sequential access
    /// - Metadata caching for file stats
    /// - LRU eviction for memory management
    ///
    /// None until initialized, then Some for SSTable I/O
    unified_fs: Option<Arc<dyn FileSystem>>,

    /// **Transaction Coordinator**
    ///
    /// Ensures atomic flush and compaction operations:
    /// - ACID guarantees for SSTable writes
    /// - Two-phase commit for compaction
    /// - Crash recovery using WAL
    /// - Prevents torn writes during system failures
    ///
    /// Always present, wraps filesystem with transactional semantics
    atomic_coordinator: Arc<TransactionCoordinator>,

    /// **SSTable Reader** (Shared)
    ///
    /// Unified reader for all SSTable formats:
    /// - Parses SSTable headers and index blocks
    /// - Performs bloom filter checks
    /// - Reads and decompresses data blocks
    /// - Handles both legacy and modern formats
    ///
    /// Shared across collections to reduce memory overhead
    sstable_reader: Arc<UnifiedSstableReader>,

    /// **Distance Computation Engine**
    ///
    /// Hardware-accelerated distance calculations:
    /// - Auto-detects SIMD capabilities (AVX2/AVX512/NEON)
    /// - Supports multiple metrics (L2, cosine, dot product)
    /// - Batch processing for throughput optimization
    /// - Fallback to scalar for unsupported architectures
    ///
    /// Shared singleton for all distance operations
    distance_compute: Arc<UnifiedDistanceCompute>,

    /// **Decompression Cache** (Shared)
    ///
    /// LRU cache for decompressed SSTable blocks:
    /// - Caches frequently accessed blocks (hot data)
    /// - Adaptive sizing based on access patterns
    /// - Reduces CPU overhead from repeated decompression
    /// - Memory bounded with configurable limits
    ///
    /// Shared across all collections for better hit rates
    decompression_cache: Arc<decompression_cache::DecompressionCache>,

    /// **Storage Quantization Engine** (Collection-Aware)
    ///
    /// Persistent quantization with collection-specific codebooks:
    /// - Stores PQ codebooks in filesystem
    /// - Trains once, reuses across queries
    /// - Supports binary, INT8, PQ4/8/16/32
    /// - Automatically selects best quantization level
    ///
    /// Used for consistent quantization across flush/search
    storage_quantization_engine: Arc<StorageQuantizationEngine>,

    /// **Fallback Quantization Engine** (Stateless)
    ///
    /// In-memory quantization for ad-hoc queries:
    /// - No persistent codebooks needed
    /// - Faster for one-off quantization
    /// - Uses k-means++ clustering
    /// - Falls back when codebook unavailable
    ///
    /// Used when storage engine doesn't have trained codebooks
    fallback_quantization_engine: Arc<UnifiedQuantizationEngine>,

    /// **Universal Performance Optimizer**
    ///
    /// Dynamic query optimization system:
    /// - Analyzes query patterns and data distribution
    /// - Selects optimal execution strategy
    /// - Adaptive index selection (bloom, quantized, full)
    /// - Cost-based decision making
    ///
    /// Updates strategy based on runtime metrics
    universal_optimizer: UniversalPerformanceOptimizer,

    /// **Cross-Cache Orchestrator** (Optional)
    ///
    /// Coordinates metadata and filter caches:
    /// - Invalidates dependent caches on updates
    /// - Propagates filter pushdowns to storage
    /// - Tracks cache dependencies and relationships
    /// - Manages cache memory budgets
    ///
    /// None if caching disabled, Some in production
    orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
}

impl SstEngine {
    /// Create a new SST engine instance (stateless)
    ///
    /// Collection info comes from FlushParameters and StorageQueryContext at runtime.
    /// The engine is designed as a singleton that can handle multiple collections.
    pub async fn new() -> Result<Self> {
        let config = SstConfig::default();
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await?);
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        Self::new_with_config(config, filesystem, distance_compute).await
    }

    /// Create SST engine with specific configuration
    ///
    /// This method allows for custom configuration of the SST engine components.
    /// Used internally and for testing with specific configurations.
    pub async fn new_with_config(
        config: SstConfig,
        filesystem: Arc<FilesystemFactory>,
        distance_compute: Arc<UnifiedDistanceCompute>,
    ) -> Result<Self> {
        info!("🌲 Creating SST storage engine (collection-agnostic singleton)");

        // Create atomic coordinator for safe operations
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem.clone(), None)
                .await
                .map_err(|e| {
                    SstError::Internal(format!("Failed to create atomic coordinator: {}", e))
                })?,
        );

        // Create UnifiedCachingFilesystem for transparent cloud storage support
        let base_fs = filesystem
            .get_filesystem("file://")
            .map_err(|e| SstError::Internal(format!("Failed to get base filesystem: {}", e)))?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            String::new(), // Empty collection_id for singleton
            "sst".to_string(),
        ));

        // Create SSTable reader - using empty collection_id as SST is now singleton
        let sstable_reader = Arc::new(UnifiedSstableReader::new(
            filesystem.clone(),
            unified_fs.clone(),
            String::new(), // Empty collection_id for singleton
        ));

        // Initialize quantization engines
        let (storage_quantization_engine, fallback_quantization_engine) =
            Self::initialize_quantization_engines(distance_compute.clone()).await?;

        // Initialize decompression cache with default size (64MB)
        let decompression_cache = Arc::new(decompression_cache::DecompressionCache::new(
            64 * 1024 * 1024, // Default to 64MB cache
        ));

        // Register cache providers with orchestrator
        let orchestrator = Self::register_cache_providers(decompression_cache.clone()).await?;

        // Initialize compaction manager (always enabled)
        let compaction_manager = Some(Arc::new(Compaction::new(config.clone()).await.map_err(
            |e| SstError::Internal(format!("Failed to create compaction manager: {}", e)),
        )?));

        // Initialize universal performance optimization
        let universal_optimizer =
            UniversalPerformanceOptimizer::with_strategy(UniversalOptimizationStrategy::Balanced)
                .await
                .map_err(|e| {
                    SstError::Internal(format!(
                        "Failed to create universal performance optimizer: {}",
                        e
                    ))
                })?;

        Ok(Self {
            config,
            compaction_manager,
            filesystem,
            unified_fs: None, // Created per collection
            atomic_coordinator,
            sstable_reader,
            distance_compute,
            decompression_cache,
            storage_quantization_engine,
            fallback_quantization_engine,
            universal_optimizer,
            orchestrator,
        })
    }

    /// Initialize quantization engines (storage-aware and fallback)
    async fn initialize_quantization_engines(
        distance_compute: Arc<UnifiedDistanceCompute>,
    ) -> Result<(
        Arc<StorageQuantizationEngine>,
        Arc<UnifiedQuantizationEngine>,
    )> {
        // Create storage-aware quantization engine for persistent collection-based PQ
        let codebook_store: Arc<dyn CodebookStore> = Arc::new(InMemoryCodebookStore::new());
        let unified_quantization = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store.clone(),
        ));

        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig::default();
        let storage_quantization_engine = Arc::new(StorageQuantizationEngine::new(
            unified_quantization.clone(),
            distance_compute.clone(),
            storage_config,
        ));

        // Create fallback stateless quantization engine for ad-hoc queries
        let fallback_codebook_store: Arc<dyn CodebookStore> =
            Arc::new(InMemoryCodebookStore::new());
        let fallback_quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            fallback_codebook_store,
        ));

        Ok((storage_quantization_engine, fallback_quantization_engine))
    }

    /// Register cache providers with the orchestrator
    async fn register_cache_providers(
        decompression_cache: Arc<decompression_cache::DecompressionCache>,
    ) -> Result<Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>> {
        if let Some(ref orch) =
            crate::storage::cache::orchestrator::CrossCacheOrchestrator::global()
        {
            use crate::storage::cache::orchestrator::{CacheStatsProvider, CacheType};

            // Register decompression cache provider (VectorData)
            let provider: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(crate::storage::engines::impls::sst::decompression_cache::DecompressionCacheStatsProvider::new(
                    decompression_cache.clone(),
                ));
            orch.register_cache_provider(CacheType::VectorData, provider);

            // Register lightweight providers for FilterBitmap and Metadata
            struct SstStaticProvider;
            impl CacheStatsProvider for SstStaticProvider {
                fn snapshot(&self) -> crate::storage::cache::orchestrator::UsageStats {
                    crate::storage::cache::orchestrator::UsageStats {
                        hit_rate: 0.0,
                        avg_entry_size: 4096,
                        access_frequency: 0.0,
                        last_rebalance: std::time::SystemTime::now(),
                    }
                }
            }

            let provider2: Arc<dyn CacheStatsProvider + Send + Sync> = Arc::new(SstStaticProvider);
            orch.register_cache_provider(CacheType::FilterBitmap, provider2.clone());
            orch.register_cache_provider(CacheType::Metadata, provider2);

            Ok(Some(orch.clone()))
        } else {
            Ok(None)
        }
    }

    // Getter methods for accessing engine components

    /// Get engine configuration
    pub fn config(&self) -> &SstConfig {
        &self.config
    }

    /// Get compaction manager
    pub fn compaction_manager(&self) -> Option<&Arc<Compaction>> {
        self.compaction_manager.as_ref()
    }

    /// Get filesystem factory
    pub fn filesystem(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem
    }

    /// Get unified filesystem (if initialized)
    pub fn unified_fs(&self) -> Option<&Arc<dyn FileSystem>> {
        self.unified_fs.as_ref()
    }

    /// Get atomic coordinator
    pub fn atomic_coordinator(&self) -> &Arc<TransactionCoordinator> {
        &self.atomic_coordinator
    }

    /// Get SSTable reader
    pub fn sstable_reader(&self) -> &Arc<UnifiedSstableReader> {
        &self.sstable_reader
    }

    /// Get distance computation engine
    pub fn distance_compute(&self) -> &Arc<UnifiedDistanceCompute> {
        &self.distance_compute
    }

    /// Get decompression cache
    pub fn decompression_cache(&self) -> &Arc<decompression_cache::DecompressionCache> {
        &self.decompression_cache
    }

    /// Get storage quantization engine
    pub fn storage_quantization_engine(&self) -> &Arc<StorageQuantizationEngine> {
        &self.storage_quantization_engine
    }

    /// Get fallback quantization engine
    pub fn fallback_quantization_engine(&self) -> &Arc<UnifiedQuantizationEngine> {
        &self.fallback_quantization_engine
    }

    /// Get universal optimizer
    pub fn universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }

    /// Get cache orchestrator
    pub fn orchestrator(
        &self,
    ) -> Option<&Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>> {
        self.orchestrator.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sst_engine_creation() {
        let engine = SstEngine::new().await.expect("Failed to create SST engine");

        // Verify core components are initialized
        assert!(engine.compaction_manager().is_some());
        assert!(engine.orchestrator().is_some());

        // Verify configuration is set
        assert_eq!(
            engine.config().block_size_kb,
            SstConfig::default().block_size_kb
        );
    }

    #[tokio::test]
    async fn test_sst_engine_with_custom_config() {
        let mut config = SstConfig::default();
        config.block_size_kb = 128; // Custom block size

        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        let engine = SstEngine::new_with_config(config.clone(), filesystem, distance_compute)
            .await
            .expect("Failed to create SST engine with custom config");

        assert_eq!(engine.config().block_size_kb, 128);
    }
}
