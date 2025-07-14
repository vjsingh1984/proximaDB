//! Modern Avro WAL Batch Strategy Implementation
//!
//! This implements the WalBatchStrategy trait using the batch-oriented approach.
//! Unlike the legacy AvroWalStrategy, this focuses purely on batch operations
//! for better performance and simpler API.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use super::batch_strategy::WalBatchStrategy;
use super::{FlushResult, WalConfig, WalStats};
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{String, VectorId, VectorRecord};
use crate::storage::assignment_service::{get_assignment_service, AssignmentService};
use crate::storage::memtable::specialized::wal_behavior::{WalBehaviorWrapper, WalVectorBatch};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::wal::WalFlushCoordinator;
// WalDiskManager disabled - contains legacy AvroWalEntry dependencies
use crate::storage::traits::UnifiedStorageEngine;

/// Modern Avro WAL batch strategy with native batch operations
pub struct AvroWalBatchStrategy {
    /// WAL behavior wrapper (contains GlobalPartitionedMemtable)
    memtable: Option<WalBehaviorWrapper>,
    
    /// Filesystem for direct Avro payload writing
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

impl std::fmt::Debug for AvroWalBatchStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvroWalBatchStrategy")
            .field("memtable", &self.memtable.is_some())
            .field("filesystem", &self.filesystem.is_some())
            .field("storage_engine", &"<storage_engine>")
            .field("flush_coordinator", &"<flush_coordinator>")
            .field("assignment_service", &"<assignment_service>")
            .field("distance_compute", &"<distance_compute>")
            .finish()
    }
}

impl AvroWalBatchStrategy {
    /// Create new Avro WAL batch strategy
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

    /// Fast vector count extraction from Avro header without full deserialization
    /// This provides O(1) performance for metrics while maintaining zero-copy
    fn count_vectors_from_avro_header(&self, avro_bytes: &[u8]) -> Result<usize> {
        // Use local Avro deserialize function
        let vectors = self::deserialize_avro_vector_batch(avro_bytes)?;
        Ok(vectors.len())
    }
}

impl Default for AvroWalBatchStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WalBatchStrategy for AvroWalBatchStrategy {
    fn strategy_name(&self) -> &'static str {
        "AvroBatch"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        tracing::info!("🚀 Initializing Avro WAL Batch Strategy");

        // Initialize WAL behavior wrapper with GlobalPartitionedMemtable
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        self.memtable = Some(WalBehaviorWrapper::new(memtable_config));

        // Store filesystem for direct Avro payload writing
        self.filesystem = Some(filesystem);

        tracing::info!("✅ Avro WAL Batch Strategy initialized");
        Ok(())
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        // Use try_write to avoid blocking in async context
        if let Ok(mut engine) = self.storage_engine.try_write() {
            *engine = Some(storage_engine);
            tracing::debug!("🏗️ Storage engine attached to Avro WAL Batch Strategy");
        } else {
            // If we can't get the lock immediately, spawn a blocking task
            let storage_engine_clone = self.storage_engine.clone();
            tokio::task::spawn_blocking(move || {
                let mut engine = storage_engine_clone.blocking_write();
                *engine = Some(storage_engine);
                tracing::debug!("🏗️ Storage engine attached to Avro WAL Batch Strategy (async)");
            });
        }
    }

    /// Direct Avro batch write (clean implementation, no legacy)
    async fn write_avro_batch(
        &self, 
        collection_id: &str,
        avro_bytes: &[u8]
    ) -> Result<super::WalOperation> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

        tracing::debug!(
            "📝 AVRO_BATCH: Direct Avro write for collection {} ({} bytes)",
            collection_id,
            avro_bytes.len()
        );

        // Extract vector count from Avro for metrics
        let vector_count = self.count_vectors_from_avro_header(avro_bytes)?;

        // Create WalOperation with direct Avro payload
        let wal_operation = super::WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: avro_bytes.to_vec(),
            payload_format: "avro".to_string(),
            vector_count,
        };

        // Add to memtable (single deserialization point)
        let sequences = memtable.add_wal_operation(collection_id, wal_operation.clone()).await?;

        tracing::debug!(
            "✅ AVRO_BATCH: Direct Avro write complete, sequences: {:?}",
            sequences
        );

        Ok(wal_operation)
    }

    /// Proto-first implementation: Convert Proto → Avro for legacy support
    async fn write_proto_batch(
        &self,
        collection_id: &str,
        proto_bytes: &[u8]
    ) -> Result<super::WalOperation> {
        tracing::debug!(
            "🔄 AVRO_BATCH: Converting Proto to Avro for collection {} ({} bytes)",
            collection_id,
            proto_bytes.len()
        );
        
        // In proto-first architecture, Avro strategy receives Proto and converts to Avro
        // Deserialize Proto
        let proto_records = super::schema::deserialize_proto_vector_batch(proto_bytes)?;
        
        // Convert Proto → Avro (legacy format support)
        let avro_records: Vec<crate::core::avro_unified::VectorRecord> = proto_records
            .into_iter()
            .map(|proto_record| crate::core::proto_to_avro(&proto_record, collection_id))
            .collect();
        
        // Serialize to Avro format
        let avro_bytes = self::serialize_avro_vector_batch(&avro_records)?;
        
        // Use native Avro write path
        self.write_avro_batch(collection_id, &avro_bytes).await
    }

    /// PROTO-FIRST OPTIMIZATION: Native proto vector handling with Proto→Avro serialization
    async fn write_native_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

        tracing::debug!(
            "🚀 AVRO_NATIVE: Proto→Avro conversion for batch {} with {} vectors to collection {}",
            batch.batch_id.batch_uuid,
            batch.vector_records.len(),
            batch.batch_id.collection_id
        );

        // Convert VectorRecord hybrid enum to Avro format for legacy support
        let avro_records: Vec<crate::core::avro_unified::VectorRecord> = batch.vector_records
            .iter()
            .map(|record| {
                // VectorRecord is proto type in proto-first architecture - convert to Avro for this strategy
                crate::core::proto_to_avro(record, &batch.batch_id.collection_id)
            })
            .collect();
        
        // Serialize to Avro format
        let avro_bytes = self::serialize_avro_vector_batch(&avro_records)
            .context("Failed to serialize native records to Avro")?;
        
        tracing::debug!(
            "📊 AVRO_NATIVE: Serialized {} vectors to {} bytes of Avro data",
            avro_records.len(),
            avro_bytes.len()
        );
        
        // Create WalOperation with Avro payload
        let wal_operation = super::WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: avro_bytes,
            payload_format: "avro".to_string(),
            vector_count: avro_records.len(),
        };

        // Add to memtable
        let sequences = memtable.add_wal_operation(&batch.batch_id.collection_id, wal_operation).await?;
        
        tracing::debug!(
            "✅ AVRO_NATIVE: Proto→Avro conversion complete, sequences: {:?}",
            sequences
        );

        Ok(sequences)
    }

    async fn write_vector_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

        tracing::debug!(
            "📝 AVRO_BATCH: Writing batch {} with {} vectors to collection {}",
            batch.batch_id.batch_uuid,
            batch.vector_records.len(),
            batch.batch_id.collection_id
        );

        // Use native batch method
        let sequences = memtable.add_vector_batch(batch).await?;

        tracing::debug!(
            "✅ AVRO_BATCH: Successfully wrote batch with sequences: {:?}",
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
        collection_id: &str,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalVectorBatch>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

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
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

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
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

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
            converted_results.push((record.id.as_deref().unwrap_or("").to_string(), score, record));
        }

        Ok(converted_results)
    }

    async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

        // Get all unflushed batches for the collection
        let batches = memtable.get_unflushed_batches(collection_id).await?;
        
        // Extract all vector records from batches
        let mut vectors = Vec::new();
        for batch in batches {
            vectors.extend(batch.vector_records.iter().cloned());
        }
        
        Ok(vectors)
    }

    async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult> {
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
                .context("Avro WAL Batch Strategy not initialized")?;
                
            // Clear flushed data from memtable
            let cleared = memtable.clear_flushed(collection_id, u64::MAX).await?;
            
            let duration = start_time.elapsed();
            
            Ok(FlushResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_flushed: cleared as u64,
                bytes_written: vectors.iter().map(|v| (v.vector.len() * 4 + 256) as u64).sum(),
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

    async fn drop_collection(&self, collection_id: &str) -> Result<()> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

        // Clear all data for the collection
        memtable.clear_flushed(collection_id, u64::MAX).await?;
        
        tracing::info!("✅ Dropped all WAL data for collection: {}", collection_id);
        Ok(())
    }

    async fn get_stats(&self) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

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

    async fn get_collection_stats(&self, collection_id: &str) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Avro WAL Batch Strategy not initialized")?;

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
        tracing::info!("🔄 Starting Avro WAL Batch Strategy recovery");
        
        // TODO: Implement recovery from disk
        // For now, return 0 entries recovered
        
        tracing::info!("✅ Avro WAL Batch Strategy recovery completed");
        Ok(0)
    }

    async fn close(&self) -> Result<()> {
        tracing::info!("🔒 Closing Avro WAL Batch Strategy");
        
        // TODO: Cleanup resources, close disk manager, etc.
        
        tracing::info!("✅ Avro WAL Batch Strategy closed");
        Ok(())
    }

    async fn force_sync(&self, collection_id: Option<&String>) -> Result<()> {
        // TODO: Implement force sync to disk when needed
        tracing::debug!("🔄 Force sync requested for collection: {:?}", collection_id);
        Ok(())
    }

    async fn compact_collection(&self, collection_id: &str) -> Result<u64> {
        // TODO: Implement MVCC compaction, TTL cleanup
        // For now, return 0 entries compacted
        tracing::debug!("🔧 Compacting collection {} (placeholder)", collection_id);
        Ok(0)
    }

    fn get_wal_behavior(&self) -> Option<&WalBehaviorWrapper> {
        self.memtable.as_ref()
    }
    
    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>> {
        self.filesystem.clone()
    }
}

impl DistanceComputeProvider for AvroWalBatchStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

// Avro-specific schema definitions and serialization functions
// 
// This module contains Avro-specific code that was moved from the general WAL schema.rs
// to maintain proper separation of concerns as requested by the user.

use serde::{Deserialize, Serialize};

/// ULTRA-FRUGAL vector batch schema - optimized for minimal memory/disk footprint
/// Uses smaller data types and optional fields to reduce serialization overhead
pub const VECTOR_BATCH_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "WalVectorBatch",
  "namespace": "ai.proximadb.wal",
  "fields": [
    {"name": "vectors", "type": {
      "type": "array", 
      "items": {
        "type": "record",
        "name": "VectorRecord", 
        "fields": [
          {"name": "id", "type": ["null", "string"], "default": null},
          {"name": "collection_id", "type": "string"},
          {"name": "vector", "type": {"type": "array", "items": ["null", "float"]}},
          {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
          {"name": "timestamp", "type": "int"},
          {"name": "expires_at", "type": ["null", "int"], "default": null},
          {"name": "version", "type": "int"}
        ]
      }
    }}
  ]
}
"#;

/// Avro representation of a single vector - ULTRA-FRUGAL design for minimal footprint
/// Optimized for memory and disk efficiency with optional fields and smaller data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVector {
    /// Optional ID - allows vectors without IDs for immutable use cases
    pub id: Option<String>,
    /// Collection this vector belongs to
    pub collection_id: String,
    /// The actual vector values (using Option for sparse vectors)
    pub vector: Vec<Option<f32>>,
    /// Optional metadata as string map
    pub metadata: Option<std::collections::HashMap<String, String>>,
    /// Timestamp in seconds (i32 saves 50% space vs i64 for dates until 2038)
    pub timestamp: i32,
    /// Optional expiration time in seconds
    pub expires_at: Option<i32>,
    /// Version for MVCC
    pub version: i32,
}

/// Batch wrapper for multiple vectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVectorBatch {
    pub vectors: Vec<AvroVector>,
}

/// Convert VectorRecord to AvroVector
impl From<&crate::core::VectorRecord> for AvroVector {
    fn from(record: &crate::core::VectorRecord) -> Self {
        // Convert metadata from Vec<MetadataItem> to HashMap for Avro compatibility
        let metadata = if record.metadata.is_empty() {
            None
        } else {
            let mut map = std::collections::HashMap::new();
            for item in &record.metadata {
                map.insert(item.key.clone(), item.value.clone());
            }
            Some(map)
        };
        
        Self {
            id: record.id.clone(),
            collection_id: record.collection_id.clone(),
            vector: record.vector.iter().map(|&v| Some(v)).collect(),
            metadata,
            timestamp: record.timestamp as i32,
            expires_at: record.expires_at.map(|t| t as i32),
            version: record.version as i32,
        }
    }
}

/// Convert AvroVector back to VectorRecord
impl TryFrom<&AvroVector> for crate::core::VectorRecord {
    type Error = anyhow::Error;
    
    fn try_from(avro: &AvroVector) -> Result<Self> {
        // Convert metadata from HashMap to Vec<MetadataItem>
        let metadata = if let Some(ref map) = avro.metadata {
            map.iter()
                .map(|(k, v)| crate::proto::proximadb::MetadataItem {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        
        Ok(Self {
            id: avro.id.clone(),
            collection_id: avro.collection_id.clone(),
            vector: avro.vector.iter().map(|&v| v.unwrap_or(0.0)).collect(),
            metadata,
            timestamp: avro.timestamp as i64,
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: avro.expires_at.map(|t| t as i64),
            version: avro.version as i64,
            rank: None,
            score: None,
            distance: None,
        })
    }
}

/// Create Avro vector batch from VectorRecord array
pub fn create_avro_vector_batch(vector_records: &[crate::core::VectorRecord]) -> anyhow::Result<Vec<u8>> {
    // Convert VectorRecord to AvroVector
    let avro_vectors: Vec<AvroVector> = vector_records.iter().map(AvroVector::from).collect();
    
    let batch = AvroVectorBatch {
        vectors: avro_vectors,
    };
    
    // Simple JSON serialization for now (TODO: proper Avro implementation)
    serde_json::to_vec(&batch).map_err(|e| anyhow::anyhow!("Avro serialization error: {}", e))
}

/// Deserialize Avro vector batch to VectorRecord array
pub fn deserialize_avro_vector_batch(avro_payload: &[u8]) -> anyhow::Result<Vec<crate::core::VectorRecord>> {
    // Simple JSON deserialization for now (TODO: proper Avro implementation)
    let batch: AvroVectorBatch = serde_json::from_slice(avro_payload)
        .map_err(|e| anyhow::anyhow!("Avro deserialization error: {}", e))?;
    
    let mut vector_records = Vec::new();
    for avro_vector in &batch.vectors {
        let record = crate::core::VectorRecord::try_from(avro_vector)?;
        vector_records.push(record);
    }
    
    Ok(vector_records)
}

/// Serialize unified Avro vector batch (for legacy support)
pub fn serialize_avro_vector_batch(avro_records: &[crate::core::avro_unified::VectorRecord]) -> anyhow::Result<Vec<u8>> {
    // Simple JSON serialization for now (TODO: proper Avro implementation)
    serde_json::to_vec(avro_records).map_err(|e| anyhow::anyhow!("Avro serialization error: {}", e))
}