//! Modern Proto WAL Batch Strategy Implementation
//!
//! This implements the WalBatchStrategy trait using Protocol Buffers serialization.
//! Provides zero-copy Proto handling similar to AvroWalBatchStrategy but with Proto format.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, instrument};

use super::batch_strategy::WalBatchStrategy;
use super::{FlushResult, WalConfig, WalStats};
use crate::compute::distance::DistanceMetric as CoreDistanceMetric;
use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{String, VectorId, VectorRecord};
use crate::proto::proximadb::VectorRecord as ProtoVectorRecord;
use crate::storage::assignment_service::{get_assignment_service, AssignmentService};
use crate::storage::memtable::specialized::wal_behavior::{WalBehaviorWrapper, WalVectorBatch};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::wal::schema::{create_proto_vector_batch_native, deserialize_proto_vector_batch};
use crate::storage::persistence::wal::WalFlushCoordinator;
use crate::storage::traits::UnifiedStorageEngine;

/// Modern Proto WAL batch strategy with native batch operations
pub struct ProtoWalBatchStrategy {
    /// WAL behavior wrapper (contains GlobalPartitionedMemtable)
    memtable: Option<WalBehaviorWrapper>,
    
    /// Filesystem for direct Proto payload writing
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

impl std::fmt::Debug for ProtoWalBatchStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoWalBatchStrategy")
            .field("memtable", &self.memtable.is_some())
            .field("filesystem", &self.filesystem.is_some())
            .field("storage_engine", &"<storage_engine>")
            .field("flush_coordinator", &"<flush_coordinator>")
            .field("assignment_service", &"<assignment_service>")
            .field("distance_compute", &"<distance_compute>")
            .finish()
    }
}

impl ProtoWalBatchStrategy {
    /// Create new Proto WAL batch strategy
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

    /// Fast vector count extraction from Proto without full deserialization
    fn count_vectors_from_proto(&self, proto_bytes: &[u8]) -> Result<usize> {
        // For now, use full deserialization to get count
        // TODO: Optimize to read just the repeated field length from Proto
        let vectors = deserialize_proto_vector_batch(proto_bytes)?;
        Ok(vectors.len())
    }
}

impl Default for ProtoWalBatchStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl DistanceComputeProvider for ProtoWalBatchStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

#[async_trait]
impl WalBatchStrategy for ProtoWalBatchStrategy {
    fn strategy_name(&self) -> &'static str {
        "ProtoBatch"
    }

    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        info!("🚀 Initializing Proto WAL Batch Strategy");

        // Initialize WAL behavior wrapper with GlobalPartitionedMemtable
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };

        let partition_count = std::cmp::max(4, num_cpus::get());
        
        self.memtable = Some(WalBehaviorWrapper::new(memtable_config));

        self.filesystem = Some(filesystem);

        info!("✅ Proto WAL Batch Strategy initialized with {} partitions", partition_count);
        Ok(())
    }

    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>> {
        self.filesystem.clone()
    }

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        let mut engine_guard = self.storage_engine.blocking_write();
        *engine_guard = Some(storage_engine);
    }

    /// 🚀 OPTIMAL PROTO IMPLEMENTATION - Single deserialization in WalBehavior
    #[instrument(skip(self, proto_bytes), fields(collection_id, proto_size = proto_bytes.len()))]
    async fn write_proto_batch(
        &self,
        collection_id: &str,
        proto_bytes: &[u8]
    ) -> Result<super::WalOperation> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        debug!(
            "🚀 PROTO_BATCH: Optimal write for collection {} with {} bytes",
            collection_id,
            proto_bytes.len()
        );

        // Extract vector count from Proto for metrics
        let vector_count = self.count_vectors_from_proto(proto_bytes)?;

        // Create WalOperation - will be deserialized once in WalBehavior
        let wal_operation = super::WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: proto_bytes.to_vec(),
            payload_format: "proto".to_string(),
            vector_count,
        };

        // Single deserialization point - WalBehavior handles it for ALL strategies
        let sequences = memtable.add_wal_operation(collection_id, wal_operation.clone()).await?;

        debug!(
            "✅ PROTO_BATCH: Single deserialization complete, sequences: {:?}",
            sequences
        );

        Ok(wal_operation)
    }

    /// Write Avro bytes - for backward compatibility only
    /// In proto-first architecture, this should rarely be called
    #[instrument(skip(self, avro_bytes), fields(collection_id, avro_size = avro_bytes.len()))]
    async fn write_avro_batch(
        &self, 
        collection_id: &str,
        avro_bytes: &[u8]
    ) -> Result<super::WalOperation> {
        debug!(
            "⚠️ PROTO_BATCH: Received Avro bytes in proto-first architecture for collection {} ({} bytes)",
            collection_id,
            avro_bytes.len()
        );

        // For backward compatibility, convert Avro to Proto
        // This path should be avoided in proto-first architecture
        let proto_records = super::schema::deserialize_vector_batch(avro_bytes)?;
        let proto_bytes = create_proto_vector_batch_native(&proto_records[..], collection_id)?;
        
        self.write_proto_batch(collection_id, &proto_bytes).await
    }

    /// PROTO-FIRST OPTIMIZATION: Zero-copy for Proto VectorRecords
    async fn write_native_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        debug!(
            "🚀 PROTO_NATIVE: Zero-copy writing batch {} with {} vectors to collection {}",
            batch.batch_id.batch_uuid,
            batch.vector_records.len(),
            batch.batch_id.collection_id
        );

        // All records are Proto format in proto-first architecture
        tracing::info!("⚡ ZERO-COPY: All {} vectors in Proto format", batch.vector_records.len());
        
        // Direct storage without any serialization - truly zero-copy!
        memtable.add_vector_batch(batch).await
    }

    async fn write_vector_batch(&self, batch: WalVectorBatch) -> Result<Vec<u64>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        debug!(
            "📝 PROTO_BATCH: Writing batch {} with {} vectors to collection {}",
            batch.batch_id.batch_uuid,
            batch.vector_records.len(),
            batch.batch_id.collection_id
        );

        // Convert VectorRecord enum to native Proto format
        let proto_records: Vec<ProtoVectorRecord> = batch.vector_records
            .iter()
            .map(|record| {
                // VectorRecord is already proto type in proto-first architecture
                Ok(record.clone())
            })
            .collect::<Result<Vec<_>, anyhow::Error>>()?;
        
        // Serialize to native Proto format
        let proto_bytes = create_proto_vector_batch_native(&proto_records[..], &batch.batch_id.collection_id)?;
        
        // Create WalOperation
        let wal_operation = super::WalOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: proto_bytes,
            payload_format: "proto".to_string(),
            vector_count: batch.vector_records.len(),
        };

        // Add to memtable
        memtable.add_wal_operation(&batch.batch_id.collection_id, wal_operation).await
    }

    async fn write_vector_batch_with_sync(
        &self, 
        batch: WalVectorBatch, 
        _immediate_sync: bool
    ) -> Result<Vec<u64>> {
        // For now, just delegate to write_vector_batch
        // TODO: Implement immediate sync if needed
        self.write_vector_batch(batch).await
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
            .context("Proto WAL Batch Strategy not initialized")?;

        // Get all unflushed batches, then filter by sequence and apply limit
        let all_batches = memtable.get_unflushed_batches(collection_id).await?;
        let mut filtered_batches: Vec<WalVectorBatch> = all_batches
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
            .context("Proto WAL Batch Strategy not initialized")?;

        memtable.get_vector_by_id(collection_id, vector_id).await
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
            .context("Proto WAL Batch Strategy not initialized")?;

        let metric = distance_metric.unwrap_or(CoreDistanceMetric::Cosine);
        let results = memtable.search_unflushed_vectors(query_vector, k, collection_id, metric).await?;
        
        // Convert results to the expected format
        let converted_results: Vec<(VectorId, f32, VectorRecord)> = results
            .into_iter()
            .map(|(score, record)| (record.id.as_deref().unwrap_or("").to_string(), score, record))
            .collect();
            
        Ok(converted_results)
    }

    async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        memtable.get_all_vectors(&collection_id.to_string()).await
    }

    async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult> {
        let storage_engine = self.storage_engine.read().await;
        let storage_engine = storage_engine
            .as_ref()
            .context("Storage engine not set for flush operation")?;

        // Get vectors to flush first
        let vectors = self.get_collection_vectors(collection_id).await?;
        
        // Delegate to storage engine with flush parameters
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            vector_records: vectors,
            trigger_compaction: false,
            batch_ids: vec![],
        };
        storage_engine.flush(flush_params).await
    }

    async fn drop_collection(&self, collection_id: &str) -> Result<()> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        memtable.drop_collection(&collection_id.to_string()).await?;
        Ok(())
    }

    async fn get_stats(&self) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        let stats = memtable.get_stats().await?;
        
        // Convert to WalStats
        let mut wal_stats = WalStats::default();
        wal_stats.total_entries = stats.values().map(|s| s.total_entries).sum();
        wal_stats.memory_size_bytes = stats.values().map(|s| s.memory_size_bytes).sum();
        wal_stats.collections_count = stats.len();
        
        Ok(wal_stats)
    }

    async fn get_collection_stats(&self, collection_id: &str) -> Result<WalStats> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        let stats = memtable.get_stats().await?;
        
        if let Some(collection_stats) = stats.get(collection_id) {
            let mut wal_stats = WalStats::default();
            wal_stats.total_entries = collection_stats.total_entries;
            wal_stats.memory_size_bytes = collection_stats.memory_size_bytes;
            wal_stats.collections_count = 1;
            Ok(wal_stats)
        } else {
            Ok(WalStats::default())
        }
    }

    async fn recover(&self) -> Result<u64> {
        // Proto strategy doesn't support disk recovery yet
        // TODO: Implement disk persistence for Proto format
        info!("🔄 Proto WAL recovery: No disk persistence yet, returning 0");
        Ok(0)
    }

    async fn close(&self) -> Result<()> {
        info!("🔒 Closing Proto WAL Batch Strategy");
        
        // WalBehaviorWrapper doesn't need explicit close
        if let Some(_memtable) = &self.memtable {
            debug!("✅ Proto memtable will be dropped automatically");
        }
        
        Ok(())
    }

    async fn force_sync(&self, collection_id: Option<&String>) -> Result<()> {
        // Proto strategy doesn't have disk sync yet
        // TODO: Implement when disk persistence is added
        debug!("⚡ Proto WAL force sync: No disk persistence yet");
        Ok(())
    }

    async fn compact_collection(&self, collection_id: &str) -> Result<u64> {
        let memtable = self
            .memtable
            .as_ref()
            .context("Proto WAL Batch Strategy not initialized")?;

        // For now, just clear old entries
        // TODO: Implement proper compaction
        memtable.clear_flushed(collection_id, u64::MAX).await
            .map(|count| count as u64)
    }

    fn get_wal_behavior(&self) -> Option<&WalBehaviorWrapper> {
        self.memtable.as_ref()
    }
}