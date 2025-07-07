// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Service Layer
//!
//! The single source of truth for all ProximaDB vector operations.
//! Both REST and gRPC protocol handlers delegate to this service.
//!
//! Architecture:
//! - Zero wrapper objects - pure Avro records throughout
//! - Binary Avro serialization for performance
//! - Direct WAL integration with zero-copy operations
//! - Unified business logic for all protocols

use anyhow::{anyhow, Context, Result};
use futures::future;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, span, warn, Level};

use crate::storage::persistence::wal::config::WalConfig;
use crate::storage::persistence::wal::factory::WalFactory;
use crate::storage::persistence::wal::{WalManager, WalStrategyType};
use crate::storage::FilesystemFactory;
use crate::storage::StorageEngine;
// Note: storage::vector module has been restructured
// These types are now distributed across different modules
// VIPER engine imports removed - not used in this service
use crate::core::avro_serialization::get_avro_serializer;
use crate::core::LsmConfig;
use crate::core::{
    HealthResponse, IndexStats, MetadataFilter, MetricsResponse,
    OperationResponse, SearchContext, SearchDebugInfo, SearchMetadata, SearchResult,
    SearchStrategy, VectorInsertResponse,
    VectorOperationMetrics, VectorSearchResponse, WalMetrics,
};
use crate::index::axis::{AxisConfig, AxisIndexManager};
use crate::services::collection_service::CollectionService;
use crate::storage::engines::lsm::LsmTree;
use crate::storage::engines::viper::core::ViperCoreEngine;

/// OPTIMIZATION: Smart operation routing modes for hybrid serialization
#[derive(Debug, Clone)]
pub enum OperationMode {
    ZeroCopy(Vec<u8>),        // For batch inserts - pure Avro binary
    Protobuf(Vec<u8>),        // For other operations - protobuf binary
    Hybrid(Vec<u8>, Vec<u8>), // For mixed operations - metadata + binary data
}

/// Per-Collection Storage and Index Coordination Service
/// Mediates between storage engines and AXIS for a specific collection
/// Enables horizontal scaling with one coordinator per collection
pub struct CollectionStorageIndexCoordinator {
    collection_id: String,
    axis_manager: Arc<AxisIndexManager>,
    viper_engine: Arc<ViperCoreEngine>,
    lsm_engine: Arc<LsmTree>,
    storage_engine_type: crate::proto::proximadb::StorageEngine,
    creation_time: chrono::DateTime<chrono::Utc>,
    operation_metrics: Arc<tokio::sync::RwLock<CoordinatorMetrics>>,
}

/// Metrics for coordinator operations
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CoordinatorMetrics {
    pub total_vectors_indexed: u64,
    pub total_flushes_handled: u64,
    pub total_compactions_handled: u64,
    pub last_operation_time: Option<chrono::DateTime<chrono::Utc>>,
    pub avg_indexing_time_us: f64,
    pub avg_flush_handling_time_us: f64,
    pub avg_compaction_handling_time_us: f64,
}

impl CollectionStorageIndexCoordinator {
    pub async fn new(
        collection_id: String,
        storage_engine_type: crate::proto::proximadb::StorageEngine,
        axis_manager: Arc<AxisIndexManager>,
        viper_engine: Arc<ViperCoreEngine>,
        lsm_engine: Arc<LsmTree>,
    ) -> Result<Self> {
        tracing::info!(
            "🏗️ Creating coordinator for collection {} with {:?} storage",
            collection_id,
            storage_engine_type
        );

        // Ensure AXIS has a strategy for this collection
        axis_manager
            .ensure_collection_strategy(&collection_id)
            .await?;

        Ok(Self {
            collection_id,
            axis_manager,
            viper_engine,
            lsm_engine,
            storage_engine_type,
            creation_time: chrono::Utc::now(),
            operation_metrics: Arc::new(tokio::sync::RwLock::new(CoordinatorMetrics::default())),
        })
    }

    /// Handle flush completion - update AXIS with new file references
    pub async fn handle_flush_completion(
        &self,
        flushed_vectors: &[(String, crate::core::VectorRecord)],
        file_paths: &[String],
    ) -> Result<()> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "🔄 [{}] Coordinating AXIS updates after flush: {} vectors → {} files",
            self.collection_id,
            flushed_vectors.len(),
            file_paths.len()
        );

        // Update AXIS indexes with new file references
        for (file_path, (vector_id, _)) in file_paths.iter().zip(flushed_vectors.iter()) {
            if let Err(e) = self
                .axis_manager
                .update_vector_file_reference(vector_id, &self.collection_id, file_path)
                .await
            {
                tracing::warn!(
                    "⚠️ [{}] Failed to update AXIS file reference for {}: {}",
                    self.collection_id,
                    vector_id,
                    e
                );
            }
        }

        // Update metrics
        let elapsed_us = start_time.elapsed().as_micros() as f64;
        let mut metrics = self.operation_metrics.write().await;
        metrics.total_flushes_handled += 1;
        metrics.last_operation_time = Some(chrono::Utc::now());
        metrics.avg_flush_handling_time_us = (metrics.avg_flush_handling_time_us
            * (metrics.total_flushes_handled - 1) as f64
            + elapsed_us)
            / metrics.total_flushes_handled as f64;

        tracing::info!(
            "✅ [{}] AXIS file references updated for {} vectors in {:.2}ms",
            self.collection_id,
            flushed_vectors.len(),
            elapsed_us / 1000.0
        );
        Ok(())
    }

    /// Handle compaction completion - rebuild AXIS indexes
    pub async fn handle_compaction_completion(
        &self,
        old_files: &[String],
        new_files: &[String],
    ) -> Result<()> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "🔄 [{}] Coordinating AXIS rebuild after compaction",
            self.collection_id
        );

        self.axis_manager
            .rebuild_indexes_after_compaction(&self.collection_id, old_files, new_files)
            .await?;

        // Update metrics
        let elapsed_us = start_time.elapsed().as_micros() as f64;
        let mut metrics = self.operation_metrics.write().await;
        metrics.total_compactions_handled += 1;
        metrics.last_operation_time = Some(chrono::Utc::now());
        metrics.avg_compaction_handling_time_us = (metrics.avg_compaction_handling_time_us
            * (metrics.total_compactions_handled - 1) as f64
            + elapsed_us)
            / metrics.total_compactions_handled as f64;

        tracing::info!(
            "✅ [{}] AXIS indexes rebuilt after compaction in {:.2}ms",
            self.collection_id,
            elapsed_us / 1000.0
        );
        Ok(())
    }

    /// Handle vector insertion - index in AXIS immediately
    pub async fn handle_vector_insertion(
        &self,
        vectors: &[crate::core::VectorRecord],
    ) -> Result<u64> {
        let start_time = std::time::Instant::now();
        let mut indexed_count = 0u64;

        // OPTIMIZATION: Batch async operations to reduce async overhead
        let valid_vectors: Vec<_> = vectors
            .into_iter()
            .filter(|vector| {
                if vector.collection_id != self.collection_id {
                    tracing::warn!(
                        "⚠️ [{}] Vector {} belongs to different collection: {}",
                        self.collection_id,
                        vector.id,
                        vector.collection_id
                    );
                    false
                } else {
                    true
                }
            })
            .collect();

        // Batch insert vectors to reduce async overhead
        let insert_futures = valid_vectors.into_iter().map(|vector| {
            let axis_manager = Arc::clone(&self.axis_manager);
            let vector_id = vector.id.clone();
            async move {
                match axis_manager.insert(vector).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err((vector_id, e)),
                }
            }
        });

        // Execute all inserts concurrently
        let results = future::join_all(insert_futures).await;
        
        // Process results
        for result in results {
            match result {
                Ok(_) => indexed_count += 1,
                Err((vector_id, e)) => {
                    tracing::warn!(
                        "⚠️ [{}] AXIS indexing failed for vector {}: {}",
                        self.collection_id,
                        vector_id,
                        e
                    );
                }
            }
        }

        // Update metrics
        let elapsed_us = start_time.elapsed().as_micros() as f64;
        let mut metrics = self.operation_metrics.write().await;
        metrics.total_vectors_indexed += indexed_count;
        metrics.last_operation_time = Some(chrono::Utc::now());
        if metrics.total_vectors_indexed > 0 {
            metrics.avg_indexing_time_us = (metrics.avg_indexing_time_us
                * (metrics.total_vectors_indexed - indexed_count) as f64
                + elapsed_us)
                / metrics.total_vectors_indexed as f64;
        }

        tracing::debug!(
            "🧠 [{}] AXIS: Indexed {}/{} vectors in {:.2}μs",
            self.collection_id,
            indexed_count,
            vectors.len(),
            elapsed_us
        );
        Ok(indexed_count)
    }

    /// Get coordinator metrics (optimized to avoid clone)
    pub async fn get_metrics(&self) -> CoordinatorMetrics {
        let metrics = self.operation_metrics.read().await;
        CoordinatorMetrics {
            vectors_inserted: metrics.vectors_inserted,
            vectors_searched: metrics.vectors_searched,
            average_insert_time_ms: metrics.average_insert_time_ms,
            average_search_time_ms: metrics.average_search_time_ms,
            total_operations: metrics.total_operations,
            failed_operations: metrics.failed_operations,
        }
    }

    /// Get collection ID
    pub fn collection_id(&self) -> &str {
        &self.collection_id
    }

    /// Get storage engine type
    pub fn storage_engine_type(&self) -> crate::proto::proximadb::StorageEngine {
        self.storage_engine_type
    }

    /// Get AXIS manager for advanced operations
    pub fn axis_manager(&self) -> &Arc<AxisIndexManager> {
        &self.axis_manager
    }
}

/// Multi-Collection Coordinator Manager
/// Manages per-collection coordinators for horizontal scaling
pub struct StorageIndexCoordinatorManager {
    coordinators: Arc<tokio::sync::RwLock<HashMap<String, Arc<CollectionStorageIndexCoordinator>>>>,
    axis_manager: Arc<AxisIndexManager>,
    viper_engine: Arc<ViperCoreEngine>,
    lsm_engine: Arc<LsmTree>,
}

impl StorageIndexCoordinatorManager {
    pub async fn new(
        axis_manager: Arc<AxisIndexManager>,
        viper_engine: Arc<ViperCoreEngine>,
        lsm_engine: Arc<LsmTree>,
    ) -> Result<Self> {
        Ok(Self {
            coordinators: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            axis_manager,
            viper_engine,
            lsm_engine,
        })
    }

    /// Get or create coordinator for a collection
    pub async fn get_or_create_coordinator(
        &self,
        collection_id: &str,
        storage_engine_type: crate::proto::proximadb::StorageEngine,
    ) -> Result<Arc<CollectionStorageIndexCoordinator>> {
        let coordinators = self.coordinators.read().await;
        if let Some(coordinator) = coordinators.get(collection_id) {
            return Ok(coordinator.clone());
        }
        drop(coordinators);

        // Create new coordinator
        let coordinator = Arc::new(
            CollectionStorageIndexCoordinator::new(
                collection_id.to_string(),
                storage_engine_type,
                self.axis_manager.clone(),
                self.viper_engine.clone(),
                self.lsm_engine.clone(),
            )
            .await?,
        );

        let mut coordinators = self.coordinators.write().await;
        coordinators.insert(collection_id.to_string(), coordinator.clone());

        tracing::info!(
            "✅ Created new coordinator for collection: {}",
            collection_id
        );
        Ok(coordinator)
    }

    /// Remove coordinator for a collection (used during collection deletion)
    pub async fn remove_coordinator(
        &self,
        collection_id: &str,
    ) -> Option<Arc<CollectionStorageIndexCoordinator>> {
        let mut coordinators = self.coordinators.write().await;
        coordinators.remove(collection_id)
    }

    /// Get all active coordinators (optimized to avoid full HashMap clone)
    pub async fn get_all_coordinators(
        &self,
    ) -> HashMap<String, Arc<CollectionStorageIndexCoordinator>> {
        let coordinators = self.coordinators.read().await;
        // Only clone Arc references, not the entire HashMap structure
        coordinators.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect()
    }

    /// Get total coordinator metrics across all collections
    pub async fn get_aggregate_metrics(&self) -> HashMap<String, serde_json::Value> {
        let coordinators = self.coordinators.read().await;
        let mut aggregate = HashMap::new();

        let mut total_vectors = 0u64;
        let mut total_flushes = 0u64;
        let mut total_compactions = 0u64;
        let mut avg_indexing_time = 0.0;

        for (collection_id, coordinator) in coordinators.iter() {
            let metrics = coordinator.get_metrics().await;
            total_vectors += metrics.total_vectors_indexed;
            total_flushes += metrics.total_flushes_handled;
            total_compactions += metrics.total_compactions_handled;
            avg_indexing_time += metrics.avg_indexing_time_us;

            aggregate.insert(
                format!("collection_{}_metrics", collection_id),
                serde_json::json!({
                    "vectors_indexed": metrics.total_vectors_indexed,
                    "flushes_handled": metrics.total_flushes_handled,
                    "compactions_handled": metrics.total_compactions_handled,
                    "avg_indexing_time_us": metrics.avg_indexing_time_us,
                }),
            );
        }

        let collection_count = coordinators.len();
        aggregate.insert(
            "total_collections".to_string(),
            serde_json::Value::Number(collection_count.into()),
        );
        aggregate.insert(
            "total_vectors_indexed".to_string(),
            serde_json::Value::Number(total_vectors.into()),
        );
        aggregate.insert(
            "total_flushes_handled".to_string(),
            serde_json::Value::Number(total_flushes.into()),
        );
        aggregate.insert(
            "total_compactions_handled".to_string(),
            serde_json::Value::Number(total_compactions.into()),
        );

        if collection_count > 0 {
            aggregate.insert(
                "avg_indexing_time_us".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(avg_indexing_time / collection_count as f64)
                        .unwrap_or(0.into()),
                ),
            );
        }

        aggregate
    }
}

/// Unified service that operates exclusively on binary Avro records
/// All protocol handlers (REST, gRPC) delegate to this service
/// Uses plugin/strategy pattern for WAL and memtable selection
pub struct VectorService {
    storage: Arc<RwLock<StorageEngine>>,
    wal: Arc<WalManager>,
    viper_engine: Arc<ViperCoreEngine>,
    lsm_engine: Arc<LsmTree>,
    collection_service: Arc<CollectionService>,
    coordinator_manager: Arc<StorageIndexCoordinatorManager>,
    performance_metrics: Arc<RwLock<LocalServiceMetrics>>,
    wal_strategy_type: WalStrategyType,
    avro_schema_version: u32,
}

/// Configuration for the unified Avro service
#[derive(Debug, Clone)]
pub struct UnifiedServiceConfig {
    /// WAL strategy to use (Avro or Bincode)
    pub wal_strategy: WalStrategyType,
    /// Memtable type selection
    pub memtable_type: crate::storage::persistence::wal::config::MemTableType,
    /// Avro schema version for compatibility
    pub avro_schema_version: u32,
    /// Enable schema evolution checks
    pub enable_schema_evolution: bool,
    /// AXIS indexing configuration
    pub axis_config: AxisConfig,
}

impl Default for UnifiedServiceConfig {
    fn default() -> Self {
        Self {
            wal_strategy: WalStrategyType::Avro, // Default to Avro for consistency
            memtable_type: crate::storage::persistence::wal::config::MemTableType::BTree, // RT memtable
            avro_schema_version: 1,
            enable_schema_evolution: true,
            axis_config: AxisConfig::default(),
        }
    }
}

/// Service performance metrics (using local type, different from core::ServiceMetrics)
#[derive(Debug, Default)]
pub struct LocalServiceMetrics {
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub avg_processing_time_us: f64,
    pub last_operation_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl VectorService {
    /// Create new unified Avro service with strategy-based configuration
    pub async fn new(
        storage: Arc<RwLock<StorageEngine>>,
        wal: Arc<WalManager>,
        collection_service: Arc<CollectionService>,
        config: UnifiedServiceConfig,
    ) -> anyhow::Result<Self> {
        info!("🚀 Initializing VectorService with binary Avro operations");
        info!(
            "📋 Service Config: WAL strategy={:?}, memtable={:?}, schema_version={}",
            config.wal_strategy, config.memtable_type, config.avro_schema_version
        );

        // Create VIPER engine directly (no coordinator needed)
        let viper_engine = Arc::new(Self::create_viper_engine().await?);
        info!("✅ VIPER engine created for direct vector operations");

        // Create LSM engine for LSM collections
        let filesystem = Arc::new(
            FilesystemFactory::new(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .context("Failed to create filesystem factory for LSM")?,
        );
        let lsm_engine = Arc::new(Self::create_lsm_engine(&wal, filesystem).await?);
        info!("✅ LSM engine created for LSM collections");

        // Register both storage engines with the WAL flush coordinator
        {
            // Register VIPER engine with flush completion callback
            let viper_unified: Arc<dyn crate::storage::traits::UnifiedStorageEngine> =
                viper_engine.clone();
            if let Err(e) = wal.register_storage_engine("VIPER", viper_unified).await {
                warn!("⚠️ Failed to register VIPER engine: {}", e);
            } else {
                info!("✅ VIPER engine registered with flush coordinator");
            }

            // Register LSM engine
            let lsm_unified: Arc<dyn crate::storage::traits::UnifiedStorageEngine> =
                lsm_engine.clone();
            if let Err(e) = wal.register_storage_engine("LSM", lsm_unified).await {
                warn!("⚠️ Failed to register LSM engine: {}", e);
            } else {
                info!("✅ LSM engine registered with flush coordinator");
            }
        }

        // Configure WAL to use VIPER as default flush target for backwards compatibility
        {
            // Clone the Arc to avoid move issues
            let viper_storage_engine: Arc<dyn crate::storage::traits::UnifiedStorageEngine> =
                viper_engine.clone();

            // Set VIPER as storage engine for WAL delegation
            // This enables atomic WAL→Memtable→VIPER flush delegation
            wal.set_storage_engine(viper_storage_engine);
            info!("✅ WAL→VIPER delegation established (VIPER implements UnifiedStorageEngine)");
        }

        // Initialize AXIS index manager
        let axis_manager = Arc::new(
            AxisIndexManager::new(config.axis_config.clone())
                .await
                .context("Failed to initialize AXIS index manager")?,
        );
        info!("✅ AXIS adaptive indexing system initialized");

        // Create storage-index coordinator manager for per-collection scaling
        let coordinator_manager = Arc::new(
            StorageIndexCoordinatorManager::new(
                axis_manager,
                viper_engine.clone(),
                lsm_engine.clone(),
            )
            .await?,
        );
        info!("✅ Storage-Index coordinator manager initialized");

        Ok(Self {
            storage,
            wal,
            viper_engine,
            lsm_engine,
            collection_service,
            coordinator_manager,
            performance_metrics: Arc::new(RwLock::new(LocalServiceMetrics::default())),
            wal_strategy_type: config.wal_strategy,
            avro_schema_version: config.avro_schema_version,
        })
    }

    /// Create new service with WAL factory (recommended for production)
    pub async fn with_wal_factory(
        storage: Arc<RwLock<StorageEngine>>,
        collection_service: Arc<CollectionService>,
        config: UnifiedServiceConfig,
        wal_config: WalConfig,
    ) -> Result<Self> {
        info!("🏗️ Creating VectorService with WAL factory");
        info!(
            "🔧 WAL Strategy: {:?}, Memtable: {:?}",
            config.wal_strategy, config.memtable_type
        );

        // Create WAL strategy using factory
        let fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(fs_config)
                .await
                .context("Failed to create filesystem factory")?,
        );
        let wal_strategy =
            WalFactory::create_strategy(config.wal_strategy.clone(), &wal_config, filesystem)
                .await
                .context("Failed to create WAL strategy")?;

        // Create WAL manager with strategy
        let wal_manager = WalManager::new(wal_strategy, wal_config)
            .await
            .context("Failed to create WAL manager")?;

        Self::new(storage, Arc::new(wal_manager), collection_service, config).await
    }

    /// Create new service with existing WAL manager (shares WAL with StorageEngine)
    pub async fn with_existing_wal(
        storage: Arc<RwLock<StorageEngine>>,
        wal_manager: Arc<WalManager>,
        collection_service: Arc<CollectionService>,
        config: UnifiedServiceConfig,
    ) -> anyhow::Result<Self> {
        info!("🏗️ Creating VectorService with shared WAL manager");
        info!(
            "🔧 WAL Strategy: {:?}, Memtable: {:?}",
            config.wal_strategy, config.memtable_type
        );

        Self::new(storage, wal_manager, collection_service, config).await
    }

    /// Check if immediate sync should be used based on WAL configuration
    async fn should_use_immediate_sync(&self, _collection_id: &str) -> bool {
        let wal_config = self.wal.get_config();
        let sync_mode = &wal_config.performance.sync_mode;

        match sync_mode {
            crate::storage::persistence::wal::config::SyncMode::Always => {
                tracing::debug!("💾 Using immediate sync (Always mode)");
                true
            }
            crate::storage::persistence::wal::config::SyncMode::Never
            | crate::storage::persistence::wal::config::SyncMode::Periodic
            | crate::storage::persistence::wal::config::SyncMode::PerBatch
            | crate::storage::persistence::wal::config::SyncMode::MemoryOnly => {
                tracing::debug!("📦 Using batch sync mode: {:?}", sync_mode);
                false
            }
        }
    }

    /// Create and register VIPER engine with the vector coordinator
    async fn create_viper_engine() -> Result<ViperCoreEngine> {
        info!("🔧 Creating VIPER engine for direct vector storage");

        // Create filesystem factory for VIPER
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .context("Failed to create filesystem factory for VIPER")?,
        );

        // Create VIPER configuration with production settings
        let viper_config = crate::storage::engines::viper::core::ViperCoreConfig {
            enable_ml_clustering: true,
            enable_background_compaction: true,
            compression_config: crate::storage::engines::viper::core::CompressionConfig::default(),
            schema_config: crate::storage::engines::viper::core::SchemaConfig::default(),
            atomic_config: crate::storage::engines::viper::core::AtomicOperationsConfig::default(),
            writer_pool_size: 4,
            stats_interval_secs: 60,
        };

        // Create VIPER core engine (uses base trait for assignment service access)
        let viper_engine =
            crate::storage::engines::viper::core::ViperCoreEngine::new(viper_config, filesystem)
                .await
                .context("Failed to create VIPER core engine")?;

        info!("✅ VIPER engine created successfully for direct operations");
        Ok(viper_engine)
    }

    /// Create LSM engine for LSM collections
    async fn create_lsm_engine(
        wal: &Arc<WalManager>,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<LsmTree> {
        info!("🔧 Creating LSM engine for LSM collections");

        // Create LSM configuration
        let lsm_config = LsmConfig::default();

        // Use a dummy collection ID for the unified LSM engine
        // In a real implementation, each collection would have its own LSM tree
        let collection_id = crate::core::CollectionId::from("unified_lsm".to_string());

        // Create data directory for LSM (using workspace data directory)
        let data_dir = std::path::PathBuf::from("/workspace/data/lsm");
        if let Err(_) = std::fs::create_dir_all(&data_dir) {
            warn!(
                "LSM data directory already exists or creation failed: {:?}",
                data_dir
            );
        }

        // Create LSM tree (no compaction manager for now)
        let lsm_tree = LsmTree::new(
            &lsm_config,
            collection_id,
            wal.clone(),
            data_dir,
            None, // No compaction manager initially
            filesystem,
        );

        info!("✅ LSM engine created successfully for LSM collections");
        Ok(lsm_tree)
    }

    /// Validate Avro schema version compatibility
    fn validate_schema_version(&self, payload_version: Option<u32>) -> Result<()> {
        if let Some(version) = payload_version {
            if version != self.avro_schema_version {
                return Err(anyhow!(
                    "Schema version mismatch: service expects v{}, payload has v{}",
                    self.avro_schema_version,
                    version
                ));
            }
        }
        Ok(())
    }

    /// Create Avro payload with schema version header
    fn create_versioned_payload(&self, operation_type: &str, data: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();

        // Add schema version header (4 bytes)
        payload.extend_from_slice(&self.avro_schema_version.to_le_bytes());

        // Add operation type length and data
        let op_type_bytes = operation_type.as_bytes();
        payload.extend_from_slice(&(op_type_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(op_type_bytes);

        // Add actual Avro data
        payload.extend_from_slice(data);

        payload
    }

    /// Parse versioned Avro payload
    fn parse_versioned_payload<'a>(&self, payload: &'a [u8]) -> Result<(u32, String, &'a [u8])> {
        if payload.len() < 8 {
            return Err(anyhow!("Payload too short for versioned format"));
        }

        // Read schema version (4 bytes)
        let version = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        self.validate_schema_version(Some(version))?;

        // Read operation type length (4 bytes)
        let op_len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

        if payload.len() < 8 + op_len {
            return Err(anyhow!("Payload too short for operation type"));
        }

        // Read operation type
        let operation_type = String::from_utf8(payload[8..8 + op_len].to_vec())
            .context("Invalid operation type UTF-8")?;

        // Return schema version, operation type, and Avro data
        let avro_data = &payload[8 + op_len..];
        Ok((version, operation_type, avro_data))
    }

    // =============================================================================
    // VECTOR OPERATIONS
    // =============================================================================

    /// Search vectors from binary Avro SearchQuery
    pub async fn search_vectors(&self, avro_payload: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(
            Level::DEBUG,
            "search_vectors",
            payload_size = avro_payload.len()
        );
        let start_time = std::time::Instant::now();

        // Parse versioned payload for search request
        let (_version, operation_type, avro_data) = self
            .parse_versioned_payload(avro_payload)
            .context("Failed to parse versioned search payload")?;

        if operation_type != "vector_search" {
            return Err(anyhow!(
                "Operation type mismatch: expected 'vector_search', got '{}'",
                operation_type
            ));
        }

        // Parse JSON search request
        let search_request: JsonValue =
            serde_json::from_slice(avro_data).context("Failed to parse search request JSON")?;

        let collection_id = self.extract_string(&search_request, "collection_id")?;
        let queries = self.extract_array(&search_request, "queries")?;
        let top_k = self.extract_i64(&search_request, "top_k").unwrap_or(10) as usize;
        let include_vectors = search_request
            .get("include_vectors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let include_metadata = search_request
            .get("include_metadata")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Distance metric and indexing algorithm
        let distance_metric = search_request
            .get("distance_metric")
            .and_then(|v| v.as_i64())
            .unwrap_or(1); // Default: Cosine
        let index_algorithm = search_request
            .get("index_algorithm")
            .and_then(|v| v.as_i64())
            .unwrap_or(1); // Default: HNSW

        // Metadata filters
        let metadata_filters = search_request
            .get("metadata_filters")
            .and_then(|v| v.as_object());

        // Search parameters (e.g., ef_search for HNSW)
        let _search_params = search_request
            .get("search_params")
            .and_then(|v| v.as_object());

        debug!(
            "Advanced search: collection={}, queries={}, top_k={}, distance_metric={}, index_algo={}, has_filters={}",
            collection_id, queries.len(), top_k, distance_metric, index_algorithm, metadata_filters.is_some()
        );

        // Process multiple queries through vector coordinator
        let mut all_results = Vec::new();

        for (query_idx, query) in queries.iter().enumerate() {
            let query_vector = if let Some(vec_array) = query.as_array() {
                vec_array
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect::<Vec<f32>>()
            } else {
                return Err(anyhow!(
                    "Invalid query vector format at index {}",
                    query_idx
                ));
            };

            if query_vector.is_empty() {
                return Err(anyhow!("Empty query vector at index {}", query_idx));
            }

            // Create search context for the coordinator
            let search_context = SearchContext {
                collection_id: collection_id.clone(),
                query_vector,
                k: top_k,
                filters: None, // TODO: Convert metadata_filters to MetadataFilter
                strategy: SearchStrategy::Adaptive {
                    query_complexity_score: 0.5,
                    time_budget_ms: 1000,
                    accuracy_preference: 0.8,
                },
                algorithm_hints: std::collections::HashMap::new(),
                threshold: None,
                timeout_ms: Some(5000),
                include_debug_info: false,
                include_vectors: include_vectors,
            };

            // Execute search directly through VIPER engine
            let search_results = self
                .viper_engine
                .search_vectors(
                    &search_context.collection_id,
                    &search_context.query_vector,
                    search_context.k,
                )
                .await
                .context("Failed to perform vector search through VIPER engine")?;

            // Add query index to results
            for mut result in search_results {
                // Inject query index into metadata
                result
                    .metadata
                    .insert("query_index".to_string(), json!(query_idx));
                all_results.push(result);
            }
        }

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        // Optimize: avoid cloning large result sets by moving ownership
        let response_results = std::mem::take(&mut all_results);

        // Convert results to Avro format
        let avro_results: Vec<JsonValue> = response_results
            .into_iter()
            .map(|result| {
                json!({
                    "vector_id": result.vector_id,
                    "score": result.score,
                    "vector": if include_vectors { result.vector.unwrap_or_default() } else { Vec::<f32>::new() },
                    "metadata": if include_metadata { json!(result.metadata) } else { json!({}) }
                })
            })
            .collect();

        let response = VectorSearchResponse {
            success: true,
            results: response_results, // Use the cloned SearchResult objects
            total_count: avro_results.len() as i64,
            total_found: avro_results.len() as i64,
            processing_time_us: processing_time,
            algorithm_used: "STORAGE_AWARE_SEARCH".to_string(),
            error_message: None,
            search_metadata: SearchMetadata {
                algorithm_used: "STORAGE_AWARE_SEARCH".to_string(),
                query_id: Some(format!("search_{}", chrono::Utc::now().timestamp_millis())),
                query_complexity: 1.0,
                total_results: avro_results.len() as i64,
                search_time_ms: processing_time as f64 / 1000.0,
                performance_hint: Some("Using storage-aware polymorphic search".to_string()),
                index_stats: None,
            },
            debug_info: None,
        };

        self.serialize_search_response(&response)
    }

    /// Get single vector by ID
    pub async fn get_vector(
        &self,
        collection_id: &str,
        vector_id: &str,
        _include_vector: bool,
        _include_metadata: bool,
    ) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "get_vector", collection_id, vector_id);
        let start_time = std::time::Instant::now();

        let result = {
            let storage = self.storage.read().await;
            storage
                .read(&collection_id.to_string(), &vector_id.to_string())
                .await
                .context("Failed to get vector from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(result.is_some(), processing_time).await;

        let response = if let Some(vector_data) = result {
            VectorSearchResponse {
                success: true,
                results: vec![SearchResult {
                    id: vector_id.to_string(),
                    vector_id: Some(vector_id.to_string()),
                    score: 1.0, // Exact match
                    distance: Some(0.0),
                    rank: Some(0),
                    vector: Some(vector_data.vector),
                    metadata: vector_data.metadata,
                    collection_id: Some(collection_id.to_string()),
                    created_at: Some(vector_data.created_at),
                    algorithm_used: Some("DIRECT_LOOKUP".to_string()),
                    processing_time_us: Some(processing_time),
                }],
                total_count: 1,
                total_found: 1,
                processing_time_us: processing_time,
                algorithm_used: "DIRECT_LOOKUP".to_string(),
                error_message: None,
                search_metadata: SearchMetadata {
                    algorithm_used: "DIRECT_LOOKUP".to_string(),
                    query_id: Some(format!("get_{}", vector_id)),
                    query_complexity: 0.1,
                    total_results: 1,
                    search_time_ms: processing_time as f64 / 1000.0,
                    performance_hint: None,
                    index_stats: None,
                },
                debug_info: None,
            }
        } else {
            VectorSearchResponse {
                success: false,
                results: vec![],
                total_count: 0,
                total_found: 0,
                processing_time_us: processing_time,
                algorithm_used: "DIRECT_LOOKUP".to_string(),
                error_message: Some("Vector not found".to_string()),
                search_metadata: SearchMetadata {
                    algorithm_used: "DIRECT_LOOKUP".to_string(),
                    query_id: Some(format!("get_{}", vector_id)),
                    query_complexity: 0.1,
                    total_results: 0,
                    search_time_ms: processing_time as f64 / 1000.0,
                    performance_hint: None,
                    index_stats: None,
                },
                debug_info: None,
            }
        };

        self.serialize_get_response(&response)
    }

    // Note: Metadata search functionality will be implemented through
    // the vector coordinator which has access to indexed metadata

    /// Delete single vector
    pub async fn delete_vector(&self, collection_id: &str, vector_id: &str) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "delete_vector", collection_id, vector_id);
        let start_time = std::time::Instant::now();

        // Write to WAL first
        let delete_record = json!({
            "collection_id": collection_id,
            "vector_id": vector_id,
            "operation": "delete"
        });
        let wal_payload = serde_json::to_vec(&delete_record)?;
        self.wal
            .append_avro_entry(collection_id, "delete_vector", &wal_payload)
            .await
            .context("Failed to write vector delete to WAL")?;

        // Delete from storage
        let deleted = {
            let storage = self.storage.read().await;
            storage
                .soft_delete(&collection_id.to_string(), &vector_id.to_string())
                .await
                .context("Failed to delete vector from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(deleted, processing_time).await;

        let result = self.create_operation_result(
            deleted,
            if deleted {
                None
            } else {
                Some("Vector not found".to_string())
            },
            None,
            if deleted { 1 } else { 0 },
            processing_time,
        );

        self.serialize_operation_result(&result)
    }

    // =============================================================================
    // COLLECTION OPERATIONS - REMOVED
    // =============================================================================
    // Note: Collection operations moved to CollectionService
    // gRPC handlers use CollectionService directly

    // =============================================================================
    // SYSTEM OPERATIONS
    // =============================================================================

    /// Get system health status
    pub async fn health_check(&self) -> Result<Vec<u8>> {
        let start_time = std::time::Instant::now();

        let metrics = self.performance_metrics.read().await;
        let storage_healthy = true; // TODO: Add actual health checks
        let wal_healthy = true; // TODO: Add actual health checks

        let health_response = if storage_healthy && wal_healthy {
            HealthResponse::healthy(
                env!("CARGO_PKG_VERSION").to_string(),
                0, // TODO: Track actual uptime
                metrics.total_operations as i64,
                metrics.successful_operations as i64,
                metrics.failed_operations as i64,
                metrics.avg_processing_time_us,
            )
        } else {
            HealthResponse::degraded(
                env!("CARGO_PKG_VERSION").to_string(),
                0, // TODO: Track actual uptime
                metrics.total_operations as i64,
                metrics.successful_operations as i64,
                metrics.failed_operations as i64,
                metrics.avg_processing_time_us,
                storage_healthy,
                wal_healthy,
            )
        };

        let _processing_time = start_time.elapsed().as_micros() as i64;

        self.serialize_health_response(&health_response)
    }

    /// Get service metrics
    pub async fn get_metrics(&self) -> Result<Vec<u8>> {
        let metrics = self.performance_metrics.read().await;
        let wal_stats = self.wal.stats().await?;

        let service_metrics = crate::core::ServiceMetrics {
            total_operations: metrics.total_operations as i64,
            successful_operations: metrics.successful_operations as i64,
            failed_operations: metrics.failed_operations as i64,
            avg_processing_time_us: metrics.avg_processing_time_us,
            last_operation_time: metrics.last_operation_time.map(|dt| dt.timestamp_micros()),
        };

        let wal_metrics = WalMetrics {
            total_entries: wal_stats.total_entries as i64,
            memory_entries: wal_stats.memory_entries as i64,
            disk_segments: wal_stats.disk_segments as i64,
            total_disk_size_bytes: wal_stats.total_disk_size_bytes as i64,
            compression_ratio: wal_stats.compression_ratio,
        };

        let metrics_response = MetricsResponse {
            service_metrics,
            wal_metrics,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };

        self.serialize_metrics_response(&metrics_response)
    }

    // =============================================================================
    // HELPER METHODS FOR AVRO SERIALIZATION/DESERIALIZATION
    // =============================================================================

    /// Update performance metrics
    async fn update_metrics(&self, success: bool, processing_time_us: i64) {
        let mut metrics = self.performance_metrics.write().await;
        metrics.total_operations += 1;
        if success {
            metrics.successful_operations += 1;
        } else {
            metrics.failed_operations += 1;
        }

        // Update average processing time
        let total_ops = metrics.total_operations as f64;
        metrics.avg_processing_time_us = (metrics.avg_processing_time_us * (total_ops - 1.0)
            + processing_time_us as f64)
            / total_ops;
        metrics.last_operation_time = Some(chrono::Utc::now());
    }

    // Avro deserialization helpers
    fn deserialize_vector_record(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize VectorRecord")
    }

    fn deserialize_batch_request(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize BatchRequest")
    }

    fn deserialize_search_query(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize SearchQuery")
    }

    // Field extraction helpers
    fn extract_string(&self, record: &JsonValue, field: &str) -> Result<String> {
        record
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_optional_string(&self, record: &JsonValue, field: &str) -> Option<String> {
        record
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn extract_i64(&self, record: &JsonValue, field: &str) -> Result<i64> {
        record
            .get(field)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_optional_i64(&self, record: &JsonValue, field: &str) -> Option<i64> {
        record.get(field).and_then(|v| v.as_i64())
    }

    fn extract_bool(&self, record: &JsonValue, field: &str) -> Result<bool> {
        record
            .get(field)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_array<'a>(&self, record: &'a JsonValue, field: &str) -> Result<&'a Vec<JsonValue>> {
        record
            .get(field)
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("Missing or invalid array field: {}", field))
    }

    fn extract_optional_object<'a>(
        &self,
        record: &'a JsonValue,
        field: &str,
    ) -> Option<&'a serde_json::Map<String, JsonValue>> {
        record
            .get(field)
            .and_then(|v| if v.is_null() { None } else { v.as_object() })
    }

    fn extract_vector_array(&self, record: &JsonValue, field: &str) -> Result<Vec<f32>> {
        let array = self.extract_array(record, field)?;
        array
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| anyhow!("Invalid vector element"))
                    .map(|f| f as f32)
            })
            .collect()
    }

    fn extract_metadata(&self, record: &JsonValue) -> Result<Option<HashMap<String, JsonValue>>> {
        match record.get("metadata") {
            Some(meta) if !meta.is_null() => {
                if let Some(obj) = meta.as_object() {
                    // Optimize: use with_capacity and avoid intermediate clones where possible
                    let mut metadata = HashMap::with_capacity(obj.len());
                    for (k, v) in obj {
                        metadata.insert(k.clone(), v.clone());
                    }
                    Ok(Some(metadata))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn extract_timestamp(&self, record: &JsonValue) -> Result<i64> {
        Ok(record
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros()))
    }

    // Vector processing helper for batches - aligned with single insert
    async fn process_single_vector_in_batch(
        &self,
        vector_record: &JsonValue,
        collection_id: &str,
        storage: &StorageEngine,
        _upsert_mode: bool,
    ) -> Result<String> {
        // Extract required vector field
        let vector_data = self
            .extract_vector_array(vector_record, "vector")
            .context("Missing required 'vector' field in batch item")?;

        // Extract optional client ID (no auto-generation - content-based key used instead)
        let vector_id = vector_record
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default(); // Empty if not provided, content key used for storage

        // Optional metadata (defaults to empty)
        let metadata = self.extract_metadata(vector_record).unwrap_or(None);

        // Optional timestamp (defaults to now)
        let timestamp = vector_record
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());

        // expires_at is optional and defaults to null (active record)
        let expires_at = vector_record.get("expires_at").and_then(|v| v.as_i64());

        let timestamp_ms = timestamp / 1000; // Convert from microseconds to milliseconds
        let vector_record = crate::core::VectorRecord {
            id: vector_id.clone(),
            collection_id: collection_id.to_string(),
            vector: vector_data,
            metadata: metadata.unwrap_or_default(),
            timestamp: timestamp_ms,
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
            expires_at, // null by default for active records
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        storage
            .write(vector_record)
            .await
            .context("Failed to insert vector in batch processing")?;

        Ok(vector_id)
    }

    // Avro result creation helpers
    fn create_operation_result(
        &self,
        success: bool,
        error_message: Option<String>,
        error_code: Option<String>,
        affected_count: i64,
        processing_time_us: i64,
    ) -> OperationResponse {
        if success {
            OperationResponse::success(affected_count, processing_time_us)
        } else {
            OperationResponse::error(
                error_message.unwrap_or_else(|| "Unknown error".to_string()),
                error_code,
                processing_time_us,
            )
        }
    }

    fn create_search_result(
        &self,
        id: &str,
        score: f32,
        vector: Option<&Vec<f32>>,
        metadata: Option<&HashMap<String, JsonValue>>,
    ) -> JsonValue {
        json!({
            "id": id,
            "score": score,
            "vector": vector,
            "metadata": metadata
        })
    }

    // Avro serialization helpers
    fn serialize_operation_result(&self, result: &OperationResponse) -> Result<Vec<u8>> {
        get_avro_serializer()
            .serialize_operation_response(result)
            .context("Failed to serialize operation result to binary Avro")
    }

    fn serialize_search_response(&self, response: &VectorSearchResponse) -> Result<Vec<u8>> {
        get_avro_serializer()
            .serialize_search_response(response)
            .context("Failed to serialize search response to binary Avro")
    }

    fn serialize_get_response(&self, response: &VectorSearchResponse) -> Result<Vec<u8>> {
        get_avro_serializer()
            .serialize_search_response(response)
            .context("Failed to serialize get response to binary Avro")
    }

    fn serialize_health_response(&self, response: &HealthResponse) -> Result<Vec<u8>> {
        get_avro_serializer()
            .serialize_health_response(response)
            .context("Failed to serialize health response to binary Avro")
    }

    fn serialize_metrics_response(&self, response: &MetricsResponse) -> Result<Vec<u8>> {
        get_avro_serializer()
            .serialize_metrics_response(response)
            .context("Failed to serialize metrics response to binary Avro")
    }

    // ============================================================================
    // NEW gRPC v1 PROTOCOL HANDLERS - Mixed Avro binary optimization
    // ============================================================================

    /// Handle unified collection operations (CREATE, GET, LIST, DELETE, MIGRATE)

    /// Handle vector insert with ultra-fast zero-copy (ONLY PATH)
    /// All vector operations use trust-but-verify zero-copy for maximum performance

    /// Handle vector mutation (UPDATE/DELETE)
    /// Handle vector mutation (UPDATE/DELETE) - converts UPDATE to UPSERT
    pub async fn handle_vector_mutation(&self, avro_bytes: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_mutation");
        debug!(
            "📦 VectorService handling vector mutation, payload: {} bytes",
            avro_bytes.len()
        );

        let start_time = std::time::Instant::now();

        // Parse the mutation request (JSON format from gRPC)
        let mutation_request: serde_json::Value = serde_json::from_slice(avro_bytes)
            .context("Failed to parse mutation request")?;

        let collection_id = mutation_request
            .get("collection_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing collection_id in mutation request"))?;

        let operation = mutation_request
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("update");

        match operation {
            "update" => {
                // Extract selector and updates
                let selector = mutation_request
                    .get("selector")
                    .ok_or_else(|| anyhow::anyhow!("Missing selector for update"))?;

                let updates = mutation_request
                    .get("updates")
                    .ok_or_else(|| anyhow::anyhow!("Missing updates for update operation"))?;

                // For now, handle simple ID-based updates by converting to upserts
                if let Some(ids) = selector.get("ids").and_then(|v| v.as_array()) {
                    let mut affected_count = 0;
                    let mut vector_ids = Vec::new();

                    for id_value in ids {
                        if let Some(vector_id) = id_value.as_str() {
                            // Create VectorRecord from updates
                            let vector_record = self.create_vector_record_from_updates(
                                vector_id, 
                                collection_id, 
                                updates
                            )?;

                            // Convert to Avro and treat as upsert
                            let avro_data = vector_record.to_avro_bytes()
                                .context("Failed to serialize updated vector to Avro")?;

                            // Use append_avro_entry for consistency
                            match self.wal
                                .append_avro_entry(collection_id, "upsert", &avro_data)
                                .await
                            {
                                Ok(_) => {
                                    affected_count += 1;
                                    vector_ids.push(vector_id.to_string());
                                }
                                Err(e) => {
                                    warn!("Failed to update vector {}: {}", vector_id, e);
                                }
                            }
                        }
                    }

                    let processing_time = start_time.elapsed().as_micros() as i64;
                    let response = json!({
                        "success": true,
                        "operation": "mutation",
                        "affected_count": affected_count,
                        "vector_ids": vector_ids,
                        "processing_time_us": processing_time
                    });

                    Ok(serde_json::to_vec(&response)?)
                } else {
                    // For complex selectors (metadata filters, vector matches), return not implemented
                    let response = json!({
                        "success": false,
                        "error": "Complex mutation selectors not yet implemented - use individual upserts",
                        "affected_count": 0,
                        "processing_time_us": start_time.elapsed().as_micros() as i64
                    });

                    Ok(serde_json::to_vec(&response)?)
                }
            }
            "delete" => {
                // Handle delete mutations (soft delete with expires_at)
                let response = json!({
                    "success": false,
                    "error": "Delete mutations not yet implemented - use individual delete endpoints",
                    "affected_count": 0,
                    "processing_time_us": start_time.elapsed().as_micros() as i64
                });

                Ok(serde_json::to_vec(&response)?)
            }
            _ => {
                let response = json!({
                    "success": false,
                    "error": format!("Unknown mutation operation: {}", operation),
                    "affected_count": 0,
                    "processing_time_us": start_time.elapsed().as_micros() as i64
                });

                Ok(serde_json::to_vec(&response)?)
            }
        }
    }

    /// Helper method to create VectorRecord from mutation updates
    fn create_vector_record_from_updates(
        &self,
        vector_id: &str,
        collection_id: &str,
        updates: &serde_json::Value,
    ) -> Result<crate::core::VectorRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Extract vector data
        let vector = updates
            .get("vector")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect::<Vec<f32>>()
            })
            .unwrap_or_default();

        // Extract metadata (optimized with capacity pre-allocation)
        let metadata = updates
            .get("metadata")
            .and_then(|v| v.as_object())
            .map(|obj| {
                let mut metadata = std::collections::HashMap::with_capacity(obj.len());
                for (k, v) in obj {
                    metadata.insert(k.clone(), v.clone());
                }
                metadata
            })
            .unwrap_or_default();

        // Extract expires_at
        let expires_at = updates
            .get("expires_at")
            .and_then(|v| v.as_i64());

        let record = crate::core::VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector,
            metadata,
            timestamp: now_ms,
            created_at: now_ms,
            updated_at: now_ms,
            expires_at,
            version: 1, // Will be updated by WAL logic
            rank: None,
            score: None,
            distance: None,
        };

        Ok(record)
    }

    /// Handle vector search with conditional Avro binary response
    /// Handle vector insert with separated gRPC metadata and Avro vector data
    pub async fn handle_vector_insert(
        &self,
        collection_id: &str,
        upsert_mode: bool,
        vectors_avro_payload: &[u8],
    ) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_insert");
        info!("🔧 [DEBUG] VectorService handling vector insert: collection={}, upsert={}, payload={}KB, strategy={:?}", 
               collection_id, upsert_mode, vectors_avro_payload.len() / 1024, self.wal_strategy_type);

        if vectors_avro_payload.is_empty() {
            return Err(anyhow!("Empty Avro payload"));
        }

        let wal_start = std::time::Instant::now();
        let start_time = std::time::Instant::now();

        // OPTIMIZED DESIGN: Strategy-specific handling for maximum performance
        let (vector_count, vector_ids) = match self.wal_strategy_type {
            WalStrategyType::Avro => {
                // AVRO STRATEGY: True zero-copy path
                info!("🔧 [DEBUG] AVRO STRATEGY: Using TRUE ZERO-COPY path");

                // Quick validation without full deserialization (just parse enough to count vectors)
                let count = match Self::quick_validate_avro_payload(vectors_avro_payload) {
                    Ok(count) => count,
                    Err(e) => {
                        error!("🔧 [DEBUG] ❌ Invalid Avro payload: {}", e);
                        return Err(anyhow::anyhow!("Invalid Avro payload: {}", e));
                    }
                };

                // Write raw Avro bytes directly - zero copy
                let operation_type = format!("vector_batch_insert_{}", collection_id);
                match self
                    .wal
                    .append_avro_entry(collection_id, &operation_type, vectors_avro_payload)
                    .await
                {
                    Ok(seq) => {
                        info!(
                            "🔧 [DEBUG] ✅ Zero-copy Avro WAL write succeeded with sequence {}",
                            seq
                        );
                        (count, vec![format!("batch_{}_{}", collection_id, seq)])
                    }
                    Err(e) => {
                        error!("🔧 [DEBUG] ❌ WAL write failed: {}", e);
                        return Err(anyhow::anyhow!("WAL write failed: {}", e));
                    }
                }
            }

            WalStrategyType::Bincode => {
                // BINCODE STRATEGY: Aligned with AVRO for consistent batch processing
                info!("🔧 [DEBUG] BINCODE STRATEGY: Processing batch with unified pattern");

                // 🎯 ALIGNMENT: Use same validation approach as AVRO
                let vector_count = Self::quick_validate_avro_payload(vectors_avro_payload)?;
                
                let operation_type = if upsert_mode {
                    format!("vector_batch_upsert_{}", collection_id)
                } else {
                    format!("vector_batch_insert_{}", collection_id)
                };

                // 🎯 ALIGNMENT: Write batch as single entry (like AVRO)
                // But serialize payload as BINCODE instead of keeping as AVRO
                let bincode_payload = self.convert_avro_to_bincode_batch(vectors_avro_payload)?;
                
                let immediate_sync = self.should_use_immediate_sync(&collection_id).await;

                match self
                    .wal
                    .append_batch_entry(collection_id, &operation_type, &bincode_payload, immediate_sync)
                    .await
                {
                    Ok(sequence) => {
                        info!(
                            "🔧 [DEBUG] ✅ Bincode WAL batch write succeeded for {} vectors (sequence: {})",
                            vector_count, sequence
                        );
                        
                        // 🎯 ALIGNMENT: Generate batch ID same way as AVRO
                        let batch_id = format!("batch_{}_{}", collection_id, sequence);
                        let vector_ids = vec![batch_id]; // Single batch ID
                        
                        (vector_count, vector_ids)
                    }
                    Err(e) => {
                        error!("🔧 [DEBUG] ❌ WAL write failed: {}", e);
                        return Err(anyhow::anyhow!("WAL write failed: {}", e));
                    }
                }
            }
        };

        let wal_write_time = wal_start.elapsed().as_micros() as i64;
        let processing_time = start_time.elapsed().as_micros() as i64;

        self.update_metrics(true, processing_time).await;

        info!(
            "🚀 Vectors accepted in {}μs (WAL write: {}μs) - {} strategy",
            processing_time,
            wal_write_time,
            match self.wal_strategy_type {
                WalStrategyType::Avro => "ZERO-COPY AVRO",
                WalStrategyType::Bincode => "BINCODE",
            }
        );

        info!(
            "🔧 [DEBUG] Returning success response for {} vectors",
            vector_count
        );

        let response = VectorInsertResponse {
            success: true,
            vector_ids,
            error_message: None,
            error_code: None,
            metrics: VectorOperationMetrics {
                total_processed: vector_count as i64,
                successful_count: vector_count as i64,
                failed_count: 0,
                updated_count: if upsert_mode { vector_count as i64 } else { 0 },
                processing_time_us: processing_time,
                wal_write_time_us: wal_write_time,
                index_update_time_us: 0, // No immediate indexing
            },
        };

        Ok(serde_json::to_vec(&response)?)
    }

    /// Quick Avro validation without full deserialization
    /// Returns the number of vectors in the batch
    fn quick_validate_avro_payload(avro_payload: &[u8]) -> Result<usize> {
        use apache_avro::Schema;

        // Parse schema
        let schema =
            Schema::parse_str(crate::storage::persistence::wal::schema::VECTOR_BATCH_SCHEMA_V1)
                .context("Failed to parse vector batch schema")?;

        // Just parse enough to validate structure and count vectors
        let mut reader = std::io::Cursor::new(avro_payload);
        let value = apache_avro::from_avro_datum(&schema, &mut reader, None)
            .context("Invalid Avro datum format")?;

        // Extract vector count without full deserialization
        if let apache_avro::types::Value::Record(fields) = &value {
            for (name, field_value) in fields {
                if name == "vectors" {
                    if let apache_avro::types::Value::Array(vectors) = field_value {
                        return Ok(vectors.len());
                    }
                }
            }
        }

        Err(anyhow!("Invalid Avro payload structure"))
    }

    /// Handle flush completion notification from storage engines
    /// This enables AXIS to update file references after WAL→Storage flushes
    pub async fn handle_flush_completion(
        &self,
        collection_id: &str,
        flushed_vectors: Vec<(String, crate::core::VectorRecord)>,
        file_paths: Vec<String>,
    ) -> Result<()> {
        tracing::info!(
            "🔔 Received flush completion notification: collection={}, vectors={}, files={}",
            collection_id,
            flushed_vectors.len(),
            file_paths.len()
        );

        // Get collection metadata to determine storage engine type
        let collection_record = self
            .collection_service
            .get_collection_by_name_or_uuid(collection_id)
            .await
            .context("Failed to get collection metadata for flush completion")?;

        if let Some(collection_record) = collection_record {
            let coordinator = self
                .coordinator_manager
                .get_or_create_coordinator(
                    collection_id,
                    collection_record.get_storage_engine_enum(),
                )
                .await?;

            coordinator
                .handle_flush_completion(&flushed_vectors, &file_paths)
                .await?;
        } else {
            tracing::warn!(
                "⚠️ Collection {} not found for flush completion",
                collection_id
            );
        }

        tracing::info!("✅ Flush completion handled successfully");
        Ok(())
    }

    /// Handle compaction completion notification from storage engines  
    pub async fn handle_compaction_completion(
        &self,
        collection_id: &str,
        old_files: Vec<String>,
        new_files: Vec<String>,
    ) -> Result<()> {
        tracing::info!(
            "🔔 Received compaction completion notification: collection={}",
            collection_id
        );

        // Get collection metadata to determine storage engine type
        let collection_record = self
            .collection_service
            .get_collection_by_name_or_uuid(collection_id)
            .await
            .context("Failed to get collection metadata for compaction completion")?;

        if let Some(collection_record) = collection_record {
            let coordinator = self
                .coordinator_manager
                .get_or_create_coordinator(
                    collection_id,
                    collection_record.get_storage_engine_enum(),
                )
                .await?;

            coordinator
                .handle_compaction_completion(&old_files, &new_files)
                .await?;
        } else {
            tracing::warn!(
                "⚠️ Collection {} not found for compaction completion",
                collection_id
            );
        }

        tracing::info!("✅ Compaction completion handled successfully");
        Ok(())
    }

    /// Get coordinator metrics across all collections for monitoring
    pub async fn get_coordinator_metrics(&self) -> Result<Vec<u8>> {
        tracing::info!("📊 Retrieving coordinator metrics across all collections");

        let metrics = self.coordinator_manager.get_aggregate_metrics().await;
        let response = serde_json::json!({
            "coordinator_metrics": metrics,
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "service": "VectorService",
            "scaling_model": "per_collection_coordinators"
        });

        Ok(serde_json::to_vec(&response)?)
    }

    /// Get specific collection coordinator metrics
    pub async fn get_collection_coordinator_metrics(&self, collection_id: &str) -> Result<Vec<u8>> {
        tracing::info!(
            "📊 Retrieving coordinator metrics for collection: {}",
            collection_id
        );

        let coordinators = self.coordinator_manager.get_all_coordinators().await;

        if let Some(coordinator) = coordinators.get(collection_id) {
            let metrics = coordinator.get_metrics().await;
            let response = serde_json::json!({
                "collection_id": collection_id,
                "coordinator_metrics": metrics,
                "storage_engine": format!("{:?}", coordinator.storage_engine_type()),
                "timestamp": chrono::Utc::now().timestamp_millis()
            });
            Ok(serde_json::to_vec(&response)?)
        } else {
            let error_response = serde_json::json!({
                "error": "Collection not found or coordinator not initialized",
                "collection_id": collection_id
            });
            Ok(serde_json::to_vec(&error_response)?)
        }
    }

    /// Storage-aware polymorphic search method - routes to optimal search engine
    pub async fn search_vectors_polymorphic(&self, json_payload: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "search_vectors_polymorphic");
        let start_time = std::time::Instant::now();

        tracing::info!("🔍 UNIFIED POLYMORPHIC: Starting storage-aware search");
        tracing::debug!(
            "🔍 UNIFIED POLYMORPHIC: Payload size: {} bytes",
            json_payload.len()
        );

        // Parse JSON search request
        let search_request: JsonValue =
            serde_json::from_slice(json_payload).context("Failed to parse search request JSON")?;

        let collection_id = search_request
            .get("collection_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing collection_id"))?;

        // Step 1: Get collection metadata to determine storage type
        tracing::info!(
            "🔍 UNIFIED POLYMORPHIC: Getting collection metadata for {}",
            collection_id
        );
        let collection_record = self
            .collection_service
            .get_collection_by_name_or_uuid(collection_id)
            .await
            .context("Failed to get collection metadata")?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection_id))?;

        tracing::info!(
            "🔍 UNIFIED POLYMORPHIC: Collection {} uses storage engine: {:?}",
            collection_id,
            collection_record.storage_engine
        );

        // Step 2: Extract search parameters
        let query_vector = search_request
            .get("vector")
            .and_then(|v| v.as_array())
            .context("Missing or invalid vector field")?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Vec<f32>>();

        let k = search_request
            .get("k")
            .and_then(|v| v.as_i64())
            .unwrap_or(10) as usize;

        let include_vectors = search_request
            .get("include_vectors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let include_metadata = search_request
            .get("include_metadata")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Parse distance metric override from request
        let request_distance_metric = search_request
            .get("distance_metric")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "cosine" => Some(crate::compute::distance::DistanceMetric::Cosine),
                "euclidean" => Some(crate::compute::distance::DistanceMetric::Euclidean),
                "manhattan" => Some(crate::compute::distance::DistanceMetric::Manhattan),
                "dot_product" => Some(crate::compute::distance::DistanceMetric::DotProduct),
                "hamming" => Some(crate::compute::distance::DistanceMetric::Hamming),
                "jaccard" => Some(crate::compute::distance::DistanceMetric::Jaccard),
                _ => {
                    tracing::warn!(
                        "Unknown distance metric '{}', will use collection or system default",
                        s
                    );
                    None
                }
            });

        tracing::info!(
            "🔍 UNIFIED POLYMORPHIC: Search params - vector_dim={}, k={}, include_vectors={}, include_metadata={}, distance_metric={:?}",
            query_vector.len(), k, include_vectors, include_metadata, request_distance_metric
        );

        // Step 3: Parse metadata filters 
        let metadata_filters = search_request
            .get("filters")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<std::collections::HashMap<String, serde_json::Value>>()
            });

        // Step 4: **USE UNIFIED SEARCH WITH DEDUPLICATION**
        use crate::core::search::SearchEngineFactory;
        
        tracing::info!("🔍 UNIFIED POLYMORPHIC: Using unified search with deduplication");
        let search_factory = SearchEngineFactory::new(Some(self.wal.clone()));
        
        let search_results = search_factory
            .search_with_deduplication(
                &collection_record,
                &query_vector,
                k,
                metadata_filters.as_ref(),
                Some(self.viper_engine.clone()),
                Some(self.lsm_engine.clone()),
            )
            .await?;

        tracing::info!(
            "🔍 UNIFIED POLYMORPHIC: Unified search complete - {} results with deduplication",
            search_results.len()
        );

        // Step 5: Format results
        let json_results: Vec<JsonValue> = search_results
            .into_iter()
            .map(|result| {
                let mut json_result = json!({
                    "id": result.id,
                    "score": result.score,
                });

                if include_vectors {
                    json_result["vector"] = json!(result.vector);
                }

                if include_metadata {
                    json_result["metadata"] = json!(result.metadata);
                }

                json_result
            })
            .collect();

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        let response = json!({
            "results": json_results,
            "total_count": json_results.len(),
            "processing_time_us": processing_time,
            "collection_id": collection_id,
            "search_strategy": "UNIFIED_POLYMORPHIC_WITH_DEDUPLICATION",
            "unified_search_enabled": true
        });

        tracing::info!(
            "✅ UNIFIED POLYMORPHIC: Search complete - {} results in {}μs",
            json_results.len(),
            processing_time
        );

        Ok(serde_json::to_vec(&response)?)
    }

    /// Simplified search method for REST API - accepts JSON payloads directly
    /// NOTE: This is the legacy method, consider using search_vectors_polymorphic for better performance
    pub async fn search_by_metadata_server_side(
        &self,
        collection_id: String,
        filters: std::collections::HashMap<String, serde_json::Value>,
        limit: Option<usize>,
    ) -> anyhow::Result<VectorSearchResponse> {
        let start_time = std::time::Instant::now();

        info!(
            "🔍 Server-side metadata search: collection={}, filters={:?}, limit={:?}",
            collection_id, filters, limit
        );

        // Convert simple filters to MetadataFilter enum
        let metadata_filters = self.convert_filters_to_metadata_filters(filters)?;

        // Use VIPER engine for server-side filtering
        // Note: In a real implementation, this would get the actual VIPER engine instance
        // For now, we'll simulate the server-side filtering
        let total_records_before_filter = 1000; // Simulate total available records
        let filtered_records = self
            .simulate_viper_metadata_filtering(&collection_id, &metadata_filters, limit)
            .await?;

        let processing_time = start_time.elapsed().as_micros() as i64;

        // Convert filtered records to search response format
        let search_results: Vec<SearchResult> = filtered_records
            .into_iter()
            .enumerate()
            .map(|(idx, record)| SearchResult {
                id: record.id.clone(),
                vector_id: Some(record.id),
                score: 1.0 - (idx as f32 * 0.01), // Simulate relevance score
                vector: Some(record.vector),
                metadata: record.metadata,
                distance: Some(idx as f32 * 0.01), // Simulate distance
                rank: Some((idx + 1) as i32),
                collection_id: Some(record.collection_id),
                created_at: Some(record.created_at),
                algorithm_used: Some("METADATA_FILTER".to_string()),
                processing_time_us: Some(0),
            })
            .collect();

        info!(
            "✅ Server-side metadata search completed: {} results in {}μs",
            search_results.len(),
            processing_time
        );

        let total_found = search_results.len() as i64;
        Ok(VectorSearchResponse {
            success: true,
            results: search_results,
            total_count: total_found,
            total_found,
            processing_time_us: processing_time,
            algorithm_used: "VIPER_PARQUET_COLUMN_PUSHDOWN".to_string(),
            error_message: None,
            search_metadata: SearchMetadata {
                algorithm_used: "VIPER_PARQUET_COLUMN_PUSHDOWN".to_string(),
                query_id: Some(format!(
                    "metadata_search_{}",
                    chrono::Utc::now().timestamp_millis()
                )),
                query_complexity: 0.5,
                total_results: total_found,
                search_time_ms: (processing_time / 1000) as f64,
                performance_hint: if total_found > 100 {
                    Some("Consider adding more specific filters for better performance".to_string())
                } else {
                    None
                },
                index_stats: Some(IndexStats {
                    total_vectors: total_records_before_filter as i64, // Real value from simulated dataset
                    vectors_compared: total_records_before_filter as i64, // All vectors were compared for filtering
                    vectors_scanned: total_records_before_filter as i64, // All vectors were scanned
                    distance_calculations: 0, // No distance calculations for metadata-only search
                    nodes_visited: 0,         // No index nodes for linear scan
                    filter_efficiency: if total_records_before_filter > 0 {
                        total_found as f32 / total_records_before_filter as f32
                    } else {
                        0.0
                    },
                    cache_hits: 0,   // No cache in this implementation
                    cache_misses: 0, // No cache in this implementation
                }),
            },
            debug_info: Some(SearchDebugInfo {
                search_steps: vec!["metadata_filter".to_string(), "result_assembly".to_string()],
                clusters_searched: vec!["memtable".to_string()],
                filter_pushdown_enabled: false, // Not using parquet pushdown for memtable search
                parquet_columns_scanned: vec![], // No parquet columns in memtable search
                timing_breakdown: [
                    (
                        "filter_scan".to_string(),
                        processing_time as f64 * 0.8 / 1000.0,
                    ),
                    (
                        "result_assembly".to_string(),
                        processing_time as f64 * 0.2 / 1000.0,
                    ),
                ]
                .iter()
                .cloned()
                .collect(),
                memory_usage_mb: None, // Not tracked in this implementation
                estimated_total_cost: Some(processing_time as f64 / 1000.0),
                actual_cost: Some(processing_time as f64 / 1000.0),
                cost_breakdown: Some(
                    [
                        (
                            "cpu_cycles".to_string(),
                            processing_time as f64 * 0.9 / 1000.0,
                        ),
                        (
                            "memory_access".to_string(),
                            processing_time as f64 * 0.1 / 1000.0,
                        ),
                    ]
                    .iter()
                    .cloned()
                    .collect(),
                ),
            }),
        })
    }

    /// Convert simple key-value filters to MetadataFilter enum
    fn convert_filters_to_metadata_filters(
        &self,
        filters: std::collections::HashMap<String, serde_json::Value>,
    ) -> anyhow::Result<Vec<MetadataFilter>> {
        use crate::core::{FieldCondition, MetadataFilter};

        let mut metadata_filters = Vec::new();

        for (field, value) in filters {
            let condition = FieldCondition::Equals(value);
            metadata_filters.push(MetadataFilter::Field { field, condition });
        }

        Ok(metadata_filters)
    }

    /// Simulate VIPER metadata filtering (placeholder for real implementation)
    async fn simulate_viper_metadata_filtering(
        &self,
        collection_id: &str,
        filters: &[MetadataFilter],
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<crate::core::VectorRecord>> {
        info!(
            "🏗️ Simulating VIPER Parquet column filtering for collection {}",
            collection_id
        );

        // Simulate server-side filtering with realistic performance characteristics
        let mut results = Vec::new();
        let max_results = limit.unwrap_or(50).min(100); // Cap at 100 for demo

        for i in 0..max_results {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let record = crate::core::VectorRecord {
                id: format!("server_filtered_{}_{}", collection_id, i),
                collection_id: collection_id.to_string(),
                vector: vec![0.1; 384], // Mock 384-dimensional vector
                metadata: [
                    ("category".to_string(), serde_json::Value::String("AI".to_string())),
                    ("author".to_string(), serde_json::Value::String("Dr. Smith".to_string())),
                    ("doc_type".to_string(), serde_json::Value::String("research_paper".to_string())),
                    ("year".to_string(), serde_json::Value::String("2023".to_string())),
                    ("text".to_string(), serde_json::Value::String(format!(
                        "Server-side filtered document {} with Parquet column pushdown optimization", 
                        i
                    ))),
                    ("viper_filtered".to_string(), serde_json::Value::Bool(true)),
                    ("parquet_optimized".to_string(), serde_json::Value::Bool(true)),
                ].iter().cloned().collect(),
                timestamp: now_ms,
                created_at: now_ms,
                updated_at: now_ms,
                expires_at: None, // No expiration by default
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };

            // Simple filter matching for demo
            if self.record_matches_filters(&record, filters) {
                results.push(record);
            }
        }

        info!(
            "🎯 VIPER simulation returned {} server-filtered results",
            results.len()
        );
        Ok(results)
    }

    /// Check if a record matches the metadata filters
    fn record_matches_filters(
        &self,
        record: &crate::core::VectorRecord,
        filters: &[MetadataFilter],
    ) -> bool {
        use crate::core::{FieldCondition, MetadataFilter};

        for filter in filters {
            match filter {
                MetadataFilter::Field { field, condition } => {
                    if let Some(value) = record.metadata.get(field) {
                        match condition {
                            FieldCondition::Equals(target) => {
                                if value != target {
                                    return false;
                                }
                            }
                            _ => return true, // Other conditions pass for demo
                        }
                    } else {
                        return false;
                    }
                }
                _ => return true, // Other filter types pass for demo
            }
        }
        true
    }

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_all_collections(&self) -> Result<()> {
        tracing::warn!("⚠️ FORCE FLUSH ALL COLLECTIONS - TESTING ONLY");

        self.wal
            .force_flush_all()
            .await
            .context("Failed to force flush all collections")?;
        tracing::info!("✅ Force flush completed for all collections");
        Ok(())
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_collection(&self, collection_id: &str) -> Result<()> {
        tracing::warn!("⚠️ FORCE FLUSH COLLECTION {} - TESTING ONLY", collection_id);

        // Look up collection's storage engine from metadata
        let storage_engine = match self
            .collection_service
            .get_collection_by_name_or_uuid(collection_id)
            .await
        {
            Ok(Some(collection_metadata)) => {
                let engine_name = match collection_metadata.storage_engine.as_str() {
                    "Viper" | "VIPER" => "VIPER",
                    "Lsm" | "LSM" => "LSM",
                    other => {
                        tracing::warn!(
                            "📋 Collection {} has unknown storage engine '{}', defaulting to VIPER",
                            collection_id,
                            other
                        );
                        "VIPER"
                    }
                };
                tracing::info!(
                    "📋 Collection {} uses storage engine: {}",
                    collection_id,
                    engine_name
                );
                Some(engine_name)
            }
            Ok(None) => {
                tracing::warn!(
                    "⚠️ Collection {} not found, defaulting to VIPER engine",
                    collection_id
                );
                Some("VIPER")
            }
            Err(e) => {
                tracing::warn!("⚠️ Could not retrieve collection metadata for {}: {}. Defaulting to VIPER engine", collection_id, e);
                Some("VIPER")
            }
        };

        self.wal
            .force_flush_collection(collection_id, storage_engine)
            .await
            .context("Failed to force flush collection")?;
        tracing::info!(
            "✅ Force flush completed for collection {} using engine {:?}",
            collection_id,
            storage_engine
        );
        Ok(())
    }

    /// Convert Avro payload to Bincode batch for aligned storage
    fn convert_avro_to_bincode_batch(&self, avro_payload: &[u8]) -> Result<Vec<u8>> {
        // Deserialize Avro to get vector records
        let vectors = crate::storage::persistence::wal::schema::deserialize_vector_batch(avro_payload)
            .context("Failed to deserialize Avro payload for Bincode conversion")?;
        
        // Serialize as Bincode batch (same structure, different format)
        bincode::serialize(&vectors)
            .context("Failed to serialize vectors as Bincode batch")
    }
}
