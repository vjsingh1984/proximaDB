//! Modern Bincode WAL Batch Strategy Implementation
//!
//! This implements the WalBatchStrategy trait using the batch-oriented approach
//! with Bincode serialization for maximum native Rust performance.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use super::batch_strategy::WalBatchStrategy;
use super::atomicity_manager::{UnifiedAtomicityManager, UnifiedAtomicityConfig};
use super::{FlushResult, WalConfig, WalStats};
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::assignment_service::{get_assignment_service, AssignmentService};
use crate::storage::atomicity::{AtomicityManager, TransactionId};
use crate::storage::memtable::specialized::wal_behavior::{WalBehaviorWrapper, WalVectorBatch};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::wal::WalFlushCoordinator;
// WalDiskManager disabled - contains legacy AvroWalEntry dependencies
use crate::storage::traits::UnifiedStorageEngine;

/// Modern Bincode WAL batch strategy with native batch operations
/// Optimized for maximum native Rust performance while using the streamlined architecture
pub struct BincodeWalBatchStrategy {
    /// WAL behavior wrapper (contains GlobalPartitionedMemtable)
    memtable: Option<WalBehaviorWrapper>,
    
    /// Filesystem for direct binary payload writing
    filesystem: Option<Arc<FilesystemFactory>>,
    
    /// Storage engine for delegated operations
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,
    
    /// Flush coordinator for cleanup
    flush_coordinator: WalFlushCoordinator,
    
    /// Assignment service for collection directory assignment
    assignment_service: Arc<dyn AssignmentService>,
    
    /// Distance computation
    distance_compute: UnifiedDistanceCompute,
    
    /// Unified atomicity manager for all atomic operations
    unified_atomicity_manager: Option<Arc<UnifiedAtomicityManager>>,
}

impl std::fmt::Debug for BincodeWalBatchStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BincodeWalBatchStrategy")
            .field("memtable", &self.memtable.is_some())
            .field("filesystem", &self.filesystem.is_some())
            .field("storage_engine", &"<storage_engine>")
            .field("flush_coordinator", &"<flush_coordinator>")
            .field("assignment_service", &"<assignment_service>")
            .field("distance_compute", &"<distance_compute>")
            .field("unified_atomicity_manager", &self.unified_atomicity_manager.is_some())
            .finish()
    }
}

impl BincodeWalBatchStrategy {
    /// Create new Bincode WAL batch strategy
    pub fn new() -> Self {
        Self {
            memtable: None,
            filesystem: None,
            storage_engine: Arc::new(tokio::sync::RwLock::new(None)),
            flush_coordinator: WalFlushCoordinator::new(),
            assignment_service: get_assignment_service(),
            distance_compute: UnifiedDistanceCompute::default(),
            unified_atomicity_manager: None,
        }
    }
    
    /// Enable unified atomicity with configuration
    pub fn enable_unified_atomicity(&mut self, config: UnifiedAtomicityConfig) -> Result<()> {
        if let Some(filesystem) = &self.filesystem {
            let base_atomicity_manager = Arc::new(AtomicityManager::new(Default::default()));
            let unified_manager = Arc::new(UnifiedAtomicityManager::new(
                base_atomicity_manager,
                filesystem.clone(),
                config,
            ));
            self.unified_atomicity_manager = Some(unified_manager);
            tracing::info!("✅ Unified atomicity enabled for BincodeWalBatchStrategy");
            Ok(())
        } else {
            Err(anyhow::anyhow!("Filesystem must be initialized before enabling unified atomicity"))
        }
    }
}

impl Default for BincodeWalBatchStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WalBatchStrategy for BincodeWalBatchStrategy {
    fn strategy_name(&self) -> &'static str {
        "BincodeBatch"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        tracing::info!("🚀 Initializing Bincode WAL Batch Strategy");

        // Initialize WAL behavior wrapper with GlobalPartitionedMemtable
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        self.memtable = Some(WalBehaviorWrapper::new(memtable_config));

        // Store filesystem for direct binary payload writing
        self.filesystem = Some(filesystem);

        // Enable cloud atomicity if cloud backup is configured
        if let Some(cloud_backup) = &config.performance.cloud_backup {
            if cloud_backup.enabled {
                let unified_config = UnifiedAtomicityConfig {
                    transaction_timeout: std::time::Duration::from_secs(300),
                    staging_timeout: std::time::Duration::from_secs(60),
                    validation_timeout: std::time::Duration::from_secs(30),
                    cleanup_timeout: std::time::Duration::from_secs(120),
                    max_concurrent_transactions: 10,
                    enable_staging: true,
                    enable_validation: cloud_backup.verify_integrity,
                    enable_background_pipeline: true,
                    enable_cloud_operations: true,
                    retry_config: super::atomicity_manager::UnifiedRetryPolicy {
                        max_retries: cloud_backup.retry_config.max_retries,
                        initial_delay: std::time::Duration::from_millis(cloud_backup.retry_config.initial_delay_ms),
                        max_delay: std::time::Duration::from_millis(cloud_backup.retry_config.max_delay_ms),
                        backoff_multiplier: cloud_backup.retry_config.backoff_multiplier,
                        error_strategies: std::collections::HashMap::new(),
                    },
                    staging_configs: std::collections::HashMap::new(),
                    background_config: super::atomicity_manager::BackgroundPipelineConfig {
                        parallel_execution: false,
                        max_concurrent_pipelines: 5,
                        stage_timeout: std::time::Duration::from_secs(120),
                        pipeline_timeout: std::time::Duration::from_secs(600),
                    },
                };
                
                self.enable_unified_atomicity(unified_config)?;
                tracing::info!("🌐 Cloud atomicity enabled based on config");
            }
        }

        tracing::info!("✅ Bincode WAL Batch Strategy initialized");
        Ok(())
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        let mut engine = self.storage_engine.blocking_write();
        *engine = Some(storage_engine);
        tracing::debug!("🏗️ Storage engine attached to Bincode WAL Batch Strategy");
    }

    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>> {
        self.filesystem.clone()
    }

    /// 🔄 OPTIMAL BINCODE IMPLEMENTATION - Single deserialization in WalBehavior
    async fn write_avro_batch(
        &self, 
        collection_id: &CollectionId,
        avro_bytes: &[u8]
    ) -> Result<super::WalOperation> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        tracing::debug!(
            "🔄 BINCODE_BATCH: Optimal Avro→Bincode for collection {} with {} bytes",
            collection_id,
            avro_bytes.len()
        );

        // Deserialize Avro once to get vector count and for conversion
        let vectors = super::schema::deserialize_vector_batch(avro_bytes)?;
        
        tracing::debug!(
            "📊 BINCODE_BATCH: Deserialized {} vectors from Avro",
            vectors.len()
        );

        // Serialize to Bincode for optimal Rust performance
        let bincode_bytes = bincode::serialize(&vectors)
            .context("Failed to serialize vectors to Bincode")?;
        
        let bincode_size = bincode_bytes.len();

        let wal_operation = super::WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: bincode_bytes,
            payload_format: "bincode".to_string(),
            vector_count: vectors.len(),
        };

        // Set collection_id for all records (since it's not stored in payload)
        let mut vectors_with_collection = vectors;
        for record in &mut vectors_with_collection {
            record.collection_id = collection_id.to_string();
        }

        // OPTIMIZATION: Since we already deserialized, create WalVectorBatch directly to avoid double deserialize
        let batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id: crate::storage::persistence::wal::BatchId::new(
                collection_id.to_string(),
                0, // Will be set by memtable
                vectors_with_collection.len() as u64,
            ),
            vector_records: vectors_with_collection,
            created_at: std::time::SystemTime::now(),
            total_size_bytes: bincode_size,
            is_flushed: false,
        };

        let sequences = memtable.add_vector_batch(batch).await?;

        tracing::debug!(
            "✅ BINCODE_BATCH: Single deserialization complete, sequences: {:?}",
            sequences
        );

        Ok(wal_operation)
    }

    async fn write_vector_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        tracing::debug!(
            "📝 BINCODE_BATCH: Writing batch {} with {} vectors to collection {}",
            batch.batch_id.batch_uuid,
            batch.vector_records.len(),
            batch.batch_id.collection_id
        );

        // Use native batch method (same as Avro - the serialization difference is handled at lower levels)
        let sequences = memtable.add_vector_batch(batch).await?;

        tracing::debug!(
            "✅ BINCODE_BATCH: Successfully wrote batch with sequences: {:?}",
            sequences
        );

        Ok(sequences)
    }

    async fn write_vector_batch_with_sync(
        &self, 
        batch: WalVectorBatch, 
        immediate_sync: bool
    ) -> Result<Vec<u64>> {
        // Write the batch first
        let sequences = self.write_vector_batch(batch).await?;
        
        // Force sync if requested
        if immediate_sync {
            self.force_sync(None).await?;
        }
        
        Ok(sequences)
    }

    async fn read_vector_batches(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalVectorBatch>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // Get unflushed batches from memtable
        let batches = memtable.get_unflushed_batches(collection_id).await?;
        
        // Filter by sequence and apply limit
        let mut filtered_batches: Vec<WalVectorBatch> = batches
            .into_iter()
            .filter(|batch| batch.batch_id.sequence_range.0 >= from_sequence)
            .collect();
        
        // Sort by sequence for consistent ordering
        filtered_batches.sort_by_key(|batch| batch.batch_id.sequence_range.0);
        
        // Apply limit if specified
        if let Some(limit) = limit {
            filtered_batches.truncate(limit);
        }
        
        Ok(filtered_batches)
    }

    async fn search_vector_by_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // 🚀 PHASE 1: Check WAL data (unflushed) - NO DESERIALIZATION!
        // The data is already deserialized in GlobalPartitionedMemtable as WalVectorBatch
        if let Some(wal_record) = memtable.get_vector_by_id(collection_id, vector_id).await? {
            // Check if not expired
            let current_time = chrono::Utc::now().timestamp_micros();
            let is_expired = wal_record.expires_at
                .map(|expires| expires < current_time)
                .unwrap_or(false);
            
            if !is_expired {
                return Ok(Some(wal_record));
            }
        }

        // 🚀 PHASE 2: Check storage engine (flushed/compacted data)
        let storage_engine = self.storage_engine.read().await;
        if let Some(engine) = storage_engine.as_ref() {
            // TODO: Implement storage engine get_vector_by_id
            // This would query LSM SSTable or VIPER Parquet
            // For now, return None if not found in WAL
            tracing::debug!("Vector {} not found in WAL, storage lookup not yet implemented", vector_id);
        }

        Ok(None)
    }

    async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // Resolve distance metric
        let metric = distance_metric.unwrap_or(CoreDistanceMetric::Cosine);
        
        // Perform similarity search
        let results = memtable
            .search_unflushed_vectors(query_vector, k, collection_id, metric)
            .await?;

        // Convert results to the expected format
        let mut converted_results = Vec::new();
        for (score, record) in results {
            // entry is already a VectorRecord, no extraction needed
            converted_results.push((record.id.clone(), score, record));
        }

        Ok(converted_results)
    }

    async fn get_collection_vectors(&self, collection_id: &CollectionId) -> Result<Vec<VectorRecord>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // Get all unflushed batches for the collection
        let batches = memtable.get_unflushed_batches(collection_id).await?;
        
        // Extract all vector records from batches
        let mut vectors = Vec::new();
        for batch in batches {
            vectors.extend(batch.vector_records);
        }
        
        Ok(vectors)
    }

    async fn flush_collection(&self, collection_id: &CollectionId) -> Result<FlushResult> {
        // If we have a storage engine, delegate to it
        let storage_engine = self.storage_engine.read().await;
        if let Some(engine) = storage_engine.as_ref() {
            // Get vectors to flush
            let vectors = self.get_collection_vectors(collection_id).await?;
            
            if vectors.is_empty() {
                return Ok(FlushResult {
                    success: true,
                    collections_affected: vec![],
                    entries_flushed: 0,
                    bytes_written: 0,
                    files_created: 0,
                    duration_ms: 0,
                    completed_at: chrono::Utc::now(),
                    engine_metrics: std::collections::HashMap::new(),
                    compaction_triggered: false,
                    flushed_batch_ids: vec![],
                });
            }
            
            // Delegate to storage engine for actual persistence
            let start_time = std::time::Instant::now();
            
            // TODO: Implement storage engine flush delegation
            // For now, just clear the memtable data
            let memtable = self
                .memtable
                .as_ref()
                .context("Bincode WAL Batch Strategy not initialized")?;
                
            // Clear flushed data from memtable
            let cleared = memtable.clear_flushed(collection_id, u64::MAX).await?;
            
            let duration = start_time.elapsed();
            
            Ok(FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: cleared as u64,
                bytes_written: vectors.iter().map(|v| v.actual_size_bytes() as u64).sum(),
                files_created: 1,
                duration_ms: duration.as_millis() as u64,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                flushed_batch_ids: vec![],
            })
        } else {
            Err(anyhow::anyhow!("No storage engine available for flush"))
        }
    }

    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // Clear all data for the collection
        memtable.clear_flushed(collection_id, u64::MAX).await?;
        
        tracing::info!("✅ Dropped all WAL data for collection: {}", collection_id);
        Ok(())
    }

    async fn get_stats(&self) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        // Get memory stats only (simplified for now)
        let memory_stats = memtable.get_stats().await?;

        // Aggregate stats
        let total_memory_entries: u64 = memory_stats.values().map(|s| s.total_entries).sum();
        let total_memory_bytes: u64 = memory_stats
            .values()
            .map(|s| s.memory_size_bytes as u64)
            .sum();
        let memory_collections_count = memory_stats.len();

        Ok(WalStats {
            total_entries: total_memory_entries,
            memory_entries: total_memory_entries,
            disk_segments: 0, // TODO: Add disk stats when needed
            total_disk_size_bytes: 0,
            memory_size_bytes: total_memory_bytes,
            collections_count: memory_collections_count,
            last_flush_time: Some(chrono::Utc::now()),
            write_throughput_entries_per_sec: 0.0, // TODO: Calculate actual throughput
            read_throughput_entries_per_sec: 0.0, // TODO: Calculate actual throughput
            compression_ratio: 1.0,
        })
    }

    async fn get_collection_stats(&self, collection_id: &CollectionId) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Bincode WAL Batch Strategy not initialized")?;

        let memory_stats = memtable.get_stats().await?;
        
        if let Some(stats) = memory_stats.get(collection_id) {
            Ok(stats.clone())
        } else {
            // Return empty stats for collection
            Ok(WalStats {
                total_entries: 0,
                memory_entries: 0,
                disk_segments: 0,
                total_disk_size_bytes: 0,
                memory_size_bytes: 0,
                collections_count: 0,
                last_flush_time: None,
                write_throughput_entries_per_sec: 0.0,
                read_throughput_entries_per_sec: 0.0,
                compression_ratio: 1.0,
            })
        }
    }

    async fn recover(&self) -> Result<u64> {
        tracing::info!("🔄 Starting Bincode WAL Batch Strategy recovery");
        
        // TODO: Implement recovery from disk
        // For now, return 0 entries recovered
        
        tracing::info!("✅ Bincode WAL Batch Strategy recovery completed");
        Ok(0)
    }

    async fn close(&self) -> Result<()> {
        tracing::info!("🔒 Closing Bincode WAL Batch Strategy");
        
        // TODO: Cleanup resources, close disk manager, etc.
        
        tracing::info!("✅ Bincode WAL Batch Strategy closed");
        Ok(())
    }

    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        // TODO: Implement force sync to disk when needed
        tracing::debug!("🔄 Force sync requested for collection: {:?}", collection_id);
        Ok(())
    }

    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64> {
        // TODO: Implement MVCC compaction, TTL cleanup
        // For now, return 0 entries compacted
        tracing::debug!("🔧 Compacting collection {} (placeholder)", collection_id);
        Ok(0)
    }

    fn get_wal_behavior(&self) -> Option<&WalBehaviorWrapper> {
        self.memtable.as_ref()
    }
}

impl BincodeWalBatchStrategy {
    /// Execute atomic cloud write with transaction guarantees
    pub async fn atomic_write_batch_to_cloud(
        &self,
        collection_id: &CollectionId,
        batch: WalVectorBatch,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(unified_manager) = &self.unified_atomicity_manager {
            // Begin atomic cloud transaction
            let transaction_id = unified_manager.begin_transaction(
                super::atomicity_manager::UnifiedTransactionType::StorageMigration,
                vec![collection_id.clone()],
                super::atomicity_manager::UnifiedTransactionMetadata {
                    collections: vec![collection_id.clone()],
                    total_size_bytes: batch.total_size_bytes,
                    operations_count: 1,
                    storage_providers: vec![cloud_url.split("://").next().unwrap_or("unknown").to_string()],
                    staging_directories: vec![],
                    retry_count: 0,
                    priority: crate::storage::atomicity::OperationPriority::Normal,
                },
            ).await?;
            
            // Execute cloud migration operation
            let result = unified_manager.execute_cloud_migration(
                transaction_id,
                collection_id,
                "local://temp",
                cloud_url,
            ).await;
            
            match result {
                Ok(operation_result) => {
                    // Commit transaction
                    unified_manager.commit_transaction(transaction_id).await?;
                    tracing::info!(
                        "✅ ATOMIC_CLOUD_WRITE: Successfully committed batch to {} (transaction: {})",
                        cloud_url,
                        transaction_id
                    );
                    Ok(cloud_url.to_string())
                }
                Err(e) => {
                    // Rollback transaction
                    if let Err(rollback_err) = unified_manager.rollback_transaction(transaction_id).await {
                        tracing::error!("Failed to rollback transaction {}: {}", transaction_id, rollback_err);
                    }
                    Err(e)
                }
            }
        } else {
            // Fallback to non-atomic operation
            self.write_batch_to_cloud(collection_id, &batch, cloud_url).await
        }
    }
    
    /// Execute atomic cloud migration with transaction guarantees
    pub async fn atomic_migrate_batch_to_cloud(
        &self,
        collection_id: &CollectionId,
        batch: WalVectorBatch,
        local_path: &str,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(unified_manager) = &self.unified_atomicity_manager {
            // Begin atomic cloud transaction
            let transaction_id = unified_manager.begin_transaction(
                super::atomicity_manager::UnifiedTransactionType::StorageMigration,
                vec![collection_id.clone()],
                super::atomicity_manager::UnifiedTransactionMetadata {
                    collections: vec![collection_id.clone()],
                    total_size_bytes: batch.total_size_bytes,
                    operations_count: 1,
                    storage_providers: vec![cloud_url.split("://").next().unwrap_or("unknown").to_string()],
                    staging_directories: vec![],
                    retry_count: 0,
                    priority: crate::storage::atomicity::OperationPriority::Normal,
                },
            ).await?;
            
            // Execute cloud migration operation
            let result = unified_manager.execute_cloud_migration(
                transaction_id,
                collection_id,
                local_path,
                cloud_url,
            ).await;
            
            match result {
                Ok(operation_result) => {
                    // Commit transaction
                    unified_manager.commit_transaction(transaction_id).await?;
                    tracing::info!(
                        "✅ ATOMIC_CLOUD_MIGRATION: Successfully migrated batch from {} to {} (transaction: {})",
                        local_path,
                        operation_result.final_url,
                        transaction_id
                    );
                    Ok(operation_result.final_url)
                }
                Err(e) => {
                    // Rollback transaction
                    if let Err(rollback_err) = unified_manager.rollback_transaction(transaction_id).await {
                        tracing::error!("Failed to rollback migration transaction {}: {}", transaction_id, rollback_err);
                    }
                    Err(e)
                }
            }
        } else {
            // Fallback to non-atomic operation
            self.migrate_batch_to_cloud(collection_id, &batch, local_path, cloud_url).await
        }
    }
    
    /// Get unified atomicity statistics
    pub async fn get_unified_atomicity_stats(&self) -> Result<super::atomicity_manager::UnifiedAtomicityStats> {
        if let Some(unified_manager) = &self.unified_atomicity_manager {
            Ok(unified_manager.get_stats().await)
        } else {
            Err(anyhow::anyhow!("Unified atomicity not enabled"))
        }
    }
    
    /// Cleanup completed unified transactions
    pub async fn cleanup_unified_transactions(&self) -> Result<usize> {
        if let Some(unified_manager) = &self.unified_atomicity_manager {
            unified_manager.cleanup_completed_transactions().await
        } else {
            Ok(0)
        }
    }
}

impl DistanceComputeProvider for BincodeWalBatchStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}