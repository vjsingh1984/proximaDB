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

use std::collections::HashMap;
use std::sync::Arc;
use anyhow::{Context, Result};
use tracing::{info, debug};

use crate::storage::engines::impls::sst::{
    SstConfig, SstError,
    compaction::Compaction,
    readers::UnifiedSstableReader,
    decompression_cache,
};
use crate::storage::persistence::filesystem::{
    FilesystemFactory, FileSystem
};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::transaction_coordinator::TransactionCoordinator;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::compute::quantization::{
    unified::{UnifiedQuantizationEngine, CodebookStore, InMemoryCodebookStore},
    storage_engine::StorageQuantizationEngine,
};
use crate::storage::engines::core::ops::{
    UniversalPerformanceOptimizer, UniversalOptimizationStrategy
};

/// SST Engine - Row-based, write-optimized storage with three-stage filtering
///
/// The SST (Sorted String Table) engine is designed for:
/// - Real-time queries with low latency
/// - Frequent updates and writes
/// - Three-stage filtering pipeline (bloom → row → vector)
/// - LSM-tree architecture with compaction
pub struct SstEngine {
    /// Engine configuration
    config: SstConfig,

    /// Compaction manager for background optimization
    compaction_manager: Option<Arc<Compaction>>,

    /// Filesystem factory for creating filesystem instances
    filesystem: Arc<FilesystemFactory>,

    /// Unified caching filesystem for SSTable operations
    unified_fs: Option<Arc<dyn FileSystem>>,

    /// Atomic coordinator for safe flush and compaction operations
    atomic_coordinator: Arc<TransactionCoordinator>,

    /// Shared SSTable reader across all collections
    sstable_reader: Arc<UnifiedSstableReader>,

    /// Distance computation engine for vector operations
    distance_compute: Arc<UnifiedDistanceCompute>,

    /// Shared decompression cache across all collections
    decompression_cache: Arc<decompression_cache::DecompressionCache>,

    /// Storage-aware quantization engine for persistent collection-based PQ
    storage_quantization_engine: Arc<StorageQuantizationEngine>,

    /// Fallback stateless quantization engine for ad-hoc queries
    fallback_quantization_engine: Arc<UnifiedQuantizationEngine>,

    /// Universal performance optimizer
    universal_optimizer: UniversalPerformanceOptimizer,

    /// Optional Cross-Cache Orchestrator for metadata/filter tracking
    orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
}

impl SstEngine {
    /// Create a new SST engine instance (stateless)
    ///
    /// Collection info comes from FlushParameters and StorageQueryContext at runtime.
    /// The engine is designed as a singleton that can handle multiple collections.
    pub async fn new() -> Result<Self> {
        let config = SstConfig::default();
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
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
        let base_fs = filesystem.get_filesystem("file://").map_err(|e| {
            SstError::Internal(format!("Failed to get base filesystem: {}", e))
        })?;
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
        let universal_optimizer = UniversalPerformanceOptimizer::with_strategy(
            UniversalOptimizationStrategy::Balanced
        )
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
    ) -> Result<(Arc<StorageQuantizationEngine>, Arc<UnifiedQuantizationEngine>)> {
        // Create storage-aware quantization engine for persistent collection-based PQ
        let codebook_store: Arc<dyn CodebookStore> = Arc::new(InMemoryCodebookStore::new());
        let unified_quantization = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store.clone(),
        ));

        let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig::default();
        let storage_quantization_engine = Arc::new(
            StorageQuantizationEngine::new(
                unified_quantization.clone(),
                distance_compute.clone(),
                storage_config,
            )
        );

        // Create fallback stateless quantization engine for ad-hoc queries
        let fallback_codebook_store: Arc<dyn CodebookStore> = Arc::new(InMemoryCodebookStore::new());
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
        if let Some(ref orch) = crate::storage::cache::orchestrator::CrossCacheOrchestrator::global() {
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
    pub fn orchestrator(&self) -> Option<&Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>> {
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
        assert_eq!(engine.config().block_size_kb, SstConfig::default().block_size_kb);
    }

    #[tokio::test]
    async fn test_sst_engine_with_custom_config() {
        let mut config = SstConfig::default();
        config.block_size_kb = 128; // Custom block size

        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        let engine = SstEngine::new_with_config(config.clone(), filesystem, distance_compute)
            .await
            .expect("Failed to create SST engine with custom config");

        assert_eq!(engine.config().block_size_kb, 128);
    }
}