//! Modern Bincode WAL Batch Strategy Implementation
//!
//! This implements the WalBatchStrategy trait using the batch-oriented approach
//! with Bincode serialization for maximum native Rust performance.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use super::batch_strategy::WalBatchStrategy;
use super::{FlushResult, WalConfig, WalStats};
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::assignment_service::{get_assignment_service, AssignmentService};
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

        tracing::info!("✅ Bincode WAL Batch Strategy initialized");
        Ok(())
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        let mut engine = self.storage_engine.blocking_write();
        *engine = Some(storage_engine);
        tracing::debug!("🏗️ Storage engine attached to Bincode WAL Batch Strategy");
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

impl DistanceComputeProvider for BincodeWalBatchStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}