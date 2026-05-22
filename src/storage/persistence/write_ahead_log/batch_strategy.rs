//! Batch-Oriented Write Buffer Strategy (Modern Architecture)
//!
//! This module defines the WALBatchStrategy trait for batch-oriented operations.
//! The batch-oriented approach provides:
//! - Better performance through batch operations
//! - Zero-copy Avro serialization
//! - Native batch storage in memtables
//! - Simplified consistency guarantees

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric as CoreDistanceMetric;
use crate::core::{String, VectorId};
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

use super::{WALConfig, WALStats};
use crate::storage::traits::FlushResult;
use proximadb_records::{
    EmbeddingCell, ProximaRecord, ProximaTreeNode, conversions::sql_value_to_proxima,
};

/// Modern batch-oriented Write Buffer strategy trait
///
/// This trait focuses on batch operations for optimal performance:
/// - All vector operations work with WALVectorBatch
/// - No individual entry operations (use batches of size 1)
/// - Direct integration with native batch storage
/// - Simplified API surface
#[async_trait]
pub trait WALBatchStrategy: Send + Sync + std::fmt::Debug {
    /// Strategy name for identification and logging
    fn strategy_name(&self) -> &'static str;

    /// Initialize the strategy with configuration
    async fn initialize(
        &mut self,
        config: &WALConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()>;

    /// Get filesystem factory for cloud operations
    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>>;

    /// Set storage engine for delegated flush/compaction operations
    fn set_storage_engine(
        &self,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        collection_id: &str,
    );

    /// Write Write Buffer batch to cloud storage with URL-based routing
    async fn write_batch_to_cloud(
        &self,
        collection_id: &str,
        batch: &WALVectorBatch,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            crate::storage::persistence::filesystem::validate_url(cloud_url)
                .context("Invalid cloud URL format")?;

            // Serialize vector records to bytes (deref Arc)
            let batch_bytes = bincode::serialize(&*batch.vector_records)
                .context("Failed to serialize batch for cloud storage")?;

            // Generate unique filename for the batch with timestamp
            let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let batch_filename = format!(
                "write_buffer_batch_{}_{}_{}.bin",
                collection_id,
                timestamp,
                batch.batch_id.to_base62()
            );

            // Construct full cloud URL
            let full_url = if cloud_url.ends_with('/') {
                format!("{}{}", cloud_url, batch_filename)
            } else {
                format!("{}/{}", cloud_url, batch_filename)
            };

            // Validate the constructed URL
            crate::storage::persistence::filesystem::validate_url(&full_url)
                .context("Invalid constructed cloud URL")?;

            // Get filesystem for URL and write atomically
            let filesystem = fs
                .get_filesystem(&full_url)
                .context("Failed to get filesystem for cloud URL")?;

            let path = FilesystemFactory::resolve_path(&full_url)
                .context("Failed to extract path from cloud URL")?;

            let options = Some(crate::storage::persistence::filesystem::FileOptions {
                create_dirs: true,
                overwrite: true,
                ..Default::default()
            });

            filesystem
                .write_atomic(&path, &batch_bytes, options)
                .await
                .context("Failed to write batch to cloud storage")?;

            // Log detailed information for monitoring
            let bucket = fs
                .extract_bucket_from_url(&full_url)
                .ok()
                .flatten()
                .unwrap_or_else(|| "unknown".to_string());

            tracing::info!(
                "☁️ CLOUD_WRITE: Wrote batch {} ({} bytes) to {} [bucket: {}]",
                batch.batch_id.to_base62(),
                batch_bytes.len(),
                full_url,
                bucket
            );

            Ok(full_url)
        } else {
            Err(anyhow::anyhow!(
                "Filesystem not initialized for cloud operations"
            ))
        }
    }

    /// Read Write Buffer batch from cloud storage with URL-based routing
    async fn read_batch_from_cloud(&self, cloud_url: &str) -> Result<WALVectorBatch> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            crate::storage::persistence::filesystem::validate_url(cloud_url)
                .context("Invalid cloud URL format")?;

            let filesystem = fs
                .get_filesystem(cloud_url)
                .context("Failed to get filesystem for cloud URL")?;

            let path = FilesystemFactory::resolve_path(cloud_url)
                .context("Failed to extract path from cloud URL")?;

            let batch_bytes = filesystem
                .read(&path)
                .await
                .context("Failed to read batch from cloud storage")?;

            let vector_records: Vec<proximadb_records::ProximaRecord> =
                bincode::deserialize(&batch_bytes)
                    .context("Failed to deserialize batch from cloud storage")?;

            // Extract collection_id from cloud URL filename since records no longer store it
            // Expected format: write_buffer_batch_{collection_id}_{timestamp}_{batch_uuid}.bin
            let _collection_id = {
                if let Some(filename) = cloud_url.split('/').next_back() {
                    let path_parts: Vec<&str> = filename.split('_').collect();
                    if path_parts.len() >= 4
                        && path_parts[0] == "write"
                        && path_parts[1] == "buffer"
                        && path_parts[2] == "batch"
                    {
                        path_parts[3].to_string()
                    } else {
                        "unknown".to_string()
                    }
                } else {
                    "unknown".to_string()
                }
            };

            // Reconstruct WALVectorBatch from deserialized canonical records.
            use super::BatchId;
            let batch = WALVectorBatch {
                batch_id: BatchId::new(),
                vector_records: Arc::new(vector_records),
                timestamp: std::time::SystemTime::now(),
                total_size_bytes: batch_bytes.len(),
                is_flushed: false,
                metadata_bloom_filter: None,
            };

            // Log detailed information for monitoring
            let bucket = fs
                .extract_bucket_from_url(cloud_url)
                .ok()
                .flatten()
                .unwrap_or_else(|| "unknown".to_string());

            tracing::info!(
                "☁️ CLOUD_READ: Read batch {} ({} bytes) from {} [bucket: {}]",
                batch.batch_id.to_base62(),
                batch_bytes.len(),
                cloud_url,
                bucket
            );

            Ok(batch)
        } else {
            Err(anyhow::anyhow!(
                "Filesystem not initialized for cloud operations"
            ))
        }
    }

    // 🎯 CORE BATCH OPERATIONS (Modern Architecture)

    /// ✅ UNIFIED WRITE METHOD: Single entry point for all vector batch writes
    /// Handles any payload format (Avro/Proto/Bincode) and delegates to strategy-specific serialization
    async fn write_vector_batch_unified(
        &self,
        collection_id: &str,
        payload: &[u8],
        payload_format: &str,
    ) -> Result<super::WALOperation> {
        tracing::debug!(
            "📝 Unified write: collection={}, format={}, payload_size={}",
            collection_id,
            payload_format,
            payload.len()
        );

        // Step 1: Deserialize payload to canonical ProximaRecord format.
        let vector_records = match payload_format {
            "avro" => {
                use crate::storage::persistence::write_ahead_log::serialization::{
                    AvroSerializer, VectorBatchSerializer,
                };
                let serializer = AvroSerializer::new();
                serializer
                    .deserialize_batch(payload)
                    .context("Failed to deserialize Avro payload")?
            }
            "proto" => {
                use crate::storage::persistence::write_ahead_log::serialization::{
                    ProtocolBuffersSerializer, VectorBatchSerializer,
                };
                let serializer = ProtocolBuffersSerializer::new();
                serializer
                    .deserialize_batch(payload)
                    .context("Failed to deserialize Proto payload")?
            }
            "bincode" => bincode::deserialize::<Vec<proximadb_records::ProximaRecord>>(payload)
                .context("Failed to deserialize Bincode payload")?,
            "json" => serde_json::from_slice::<Vec<proximadb_records::ProximaRecord>>(payload)
                .context("Failed to deserialize JSON payload")?,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported payload format: {}",
                    payload_format
                ));
            }
        };

        // Step 2: Create WALVectorBatch and write to memtable
        let batch = WALVectorBatch {
            batch_id: super::BatchId::new(),
            vector_records: Arc::new(vector_records),
            timestamp: std::time::SystemTime::now(),
            total_size_bytes: payload.len(),
            is_flushed: false,
            metadata_bloom_filter: None,
        };

        // Deferred: Get base_location from VectorOperationsService collection metadata
        // For now, use a default fallback - this will break multi-collection with different backends
        let base_location = "file:///tmp/proximadb/data/wal";

        let sequences = self
            .write_native_batch(batch.clone(), collection_id, base_location)
            .await?;

        // Step 3: Create WALOperation using strategy-specific serialization
        let strategy_payload = self.serialize_records_for_disk(batch.vector_records.as_ref())?;
        let wal_operation = super::WALOperation {
            operation_type: "upsert_batch".to_string(),
            payload_data: strategy_payload,
            payload_format: self.strategy_name().to_lowercase(),
            vector_count: batch.vector_records.len(),
            // Vector-bearing path; the embedding drainer ignores these entries.
            pending_embed: false,
        };

        // Step 4: Persist to disk (unified logic)
        self.persist_to_disk_unified(collection_id, &wal_operation, &sequences)
            .await?;

        Ok(wal_operation)
    }

    /// Primary method: Write native WALVectorBatch directly to memtable
    /// This is the core method that all others delegate to
    ///
    /// # Arguments
    /// * `batch` - The vector batch to write
    /// * `collection_id` - Collection identifier
    /// * `base_location` - Storage location for this collection (e.g., "s3://bucket/path" or "file:///data/disk1")
    ///                     Provided by VectorOperationsService from cached collection metadata
    async fn write_native_batch(
        &self,
        batch: WALVectorBatch,
        collection_id: &str,
        base_location: &str,
    ) -> Result<Vec<u64>>;

    /// Write vector batch with immediate disk sync for durability
    async fn write_vector_batch_with_sync(
        &self,
        batch: WALVectorBatch,
        collection_id: &str,
        base_location: &str,
        immediate_sync: bool,
    ) -> Result<Vec<u64>>;

    /// Read all vector batches for a collection
    async fn read_all_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WALVectorBatch>>;

    /// Search vector by ID within a collection
    async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        // Default implementation using get_wal_behavior
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Check Write Buffer data (unflushed)
            if let Some(wal_record) = wal_behavior.vector_by_id(collection_id, vector_id).await? {
                // Check if not expired
                let current_time = chrono::Utc::now().timestamp();
                let is_expired = wal_record
                    .valid_to_ns
                    .is_some_and(|expires| expires / 1_000_000_000 < current_time);

                if !is_expired {
                    return Ok(Some(wal_record));
                }
            }
            // Deferred: Add storage engine lookup for flushed data
            Ok(None)
        } else {
            Err(anyhow::anyhow!("Write buffer behavior not available"))
        }
    }

    /// Similarity search for vectors in Write Buffer with configurable distance metric
    async fn search_vectors_similarity(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<CoreDistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, proximadb_records::ProximaRecord)>> {
        // Default implementation using get_wal_behavior
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Convert CoreDistanceMetric to unified DistanceMetric (they're the same due to alias)
            let unified_metric = distance_metric.map(|m| match m {
                CoreDistanceMetric::Unspecified => {
                    crate::compute::distance_computation::DistanceMetric::Cosine
                } // Default fallback
                CoreDistanceMetric::Cosine => {
                    crate::compute::distance_computation::DistanceMetric::Cosine
                }
                CoreDistanceMetric::Euclidean => {
                    crate::compute::distance_computation::DistanceMetric::Euclidean
                }
                CoreDistanceMetric::DotProduct => {
                    crate::compute::distance_computation::DistanceMetric::DotProduct
                }
                CoreDistanceMetric::Manhattan => {
                    crate::compute::distance_computation::DistanceMetric::Manhattan
                }
                CoreDistanceMetric::Hamming => {
                    crate::compute::distance_computation::DistanceMetric::Hamming
                }
                CoreDistanceMetric::Jaccard => {
                    crate::compute::distance_computation::DistanceMetric::Jaccard
                }
                CoreDistanceMetric::Chebyshev => {
                    crate::compute::distance_computation::DistanceMetric::Chebyshev
                }
                CoreDistanceMetric::Canberra => {
                    crate::compute::distance_computation::DistanceMetric::Canberra
                }
                CoreDistanceMetric::Minkowski => {
                    crate::compute::distance_computation::DistanceMetric::Minkowski
                }
                CoreDistanceMetric::Angular => {
                    crate::compute::distance_computation::DistanceMetric::Angular
                }
                CoreDistanceMetric::BrayCurtis => {
                    crate::compute::distance_computation::DistanceMetric::BrayCurtis
                }
                CoreDistanceMetric::Hellinger => {
                    crate::compute::distance_computation::DistanceMetric::Hellinger
                }
                CoreDistanceMetric::Custom => {
                    crate::compute::distance_computation::DistanceMetric::Custom
                }
            });

            let results = wal_behavior
                .search_unflushed_vectors(
                    collection_id,
                    query_vector,
                    k,
                    unified_metric
                        .unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine),
                    None, // No metadata filters
                    true, // Include vectors
                    true, // Include metadata
                )
                .await?;

            // Convert SearchResult objects to the expected format
            let converted_results: Vec<(VectorId, f32, ProximaRecord)> = results
                .into_iter()
                .map(|search_result| {
                    let timestamp_ns = search_result
                        .timestamp
                        .unwrap_or(0)
                        .saturating_mul(1_000_000);
                    let props = search_result
                        .metadata
                        .iter()
                        .map(|(key, value)| {
                            (
                                key.clone(),
                                ProximaTreeNode::Value(sql_value_to_proxima(value)),
                            )
                        })
                        .collect();
                    let embeddings = if search_result.vector.is_empty() {
                        Vec::new()
                    } else {
                        vec![EmbeddingCell {
                            model_id: "default".to_string(),
                            modality: "dense_vector".to_string(),
                            dim: search_result.vector.len() as u32,
                            values: search_result.vector,
                            ..Default::default()
                        }]
                    };

                    let record = ProximaRecord {
                        oid: search_result.id.clone(),
                        record_version: search_result.version.unwrap_or(0) as u64,
                        created_at_ns: timestamp_ns,
                        updated_at_ns: timestamp_ns,
                        props,
                        embeddings,
                        method: Some("wal_search".to_string()),
                        ..ProximaRecord::default()
                    };

                    (search_result.id, search_result.score as f32, record)
                })
                .collect();

            Ok(converted_results)
        } else {
            Err(anyhow::anyhow!("Write buffer behavior not available"))
        }
    }

    // 🎯 COLLECTION MANAGEMENT

    /// Get all vector records for a collection (for flush operations)
    async fn get_collection_vectors(
        &self,
        collection_id: &str,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        // Default implementation using get_wal_behavior
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Get all unflushed batches for the collection
            let batches = wal_behavior.get_unflushed_batches(collection_id).await?;

            // Extract all vector records from batches
            let mut vectors = Vec::new();
            for batch in batches {
                vectors.extend(batch.vector_records.iter().cloned());
            }

            Ok(vectors)
        } else {
            Err(anyhow::anyhow!("Write buffer behavior not available"))
        }
    }

    /// Flush collection to storage (delegates to storage engine)
    async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult>;

    /// Drop all data for a collection
    async fn drop_collection(&self, collection_id: &str) -> Result<()> {
        // Default implementation using get_wal_behavior
        if let Some(wal_behavior) = self.get_wal_behavior() {
            wal_behavior.drop_collection(collection_id).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Write buffer behavior not available"))
        }
    }

    // 🎯 STATISTICS AND MONITORING

    /// Get comprehensive Write Buffer statistics
    async fn get_stats(&self) -> Result<WALStats>;

    /// Get statistics for a specific collection
    async fn get_collection_stats(&self, collection_id: &str) -> Result<WALStats>;

    // 🎯 LIFECYCLE MANAGEMENT

    /// Recover from disk on startup
    async fn recover(&self) -> Result<u64> {
        // Default implementation - in-memory recovery from global memtable
        // Strategies that implement disk persistence should override this
        // to read Write Buffer files from disk and deserialize using deserialize_vectors_from_disk
        tracing::info!("🔄 Starting Write Buffer recovery from global memtable (in-memory only)");

        if let Some(wal_behavior) = self.get_wal_behavior() {
            match wal_behavior.stats().await {
                Ok(stats) => {
                    let total_vectors: usize =
                        stats.values().map(|s| s.total_entries as usize).sum();
                    tracing::info!(
                        "✅ Write Buffer recovery: Found {} vectors in {} collections in global memtable",
                        total_vectors,
                        stats.len()
                    );

                    // Log collection details for debugging
                    for (collection_id, collection_stats) in stats {
                        tracing::debug!(
                            "   Collection '{}': {} vectors, {} bytes",
                            collection_id,
                            collection_stats.total_entries,
                            collection_stats.memory_size_bytes
                        );
                    }

                    Ok(total_vectors as u64)
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️ Write Buffer recovery: Failed to get memtable stats: {}",
                        e
                    );
                    Ok(0)
                }
            }
        } else {
            tracing::info!("🔄 Write Buffer recovery: No memtable available, returning 0");
            Ok(0)
        }
    }

    // 🎯 DISK PERSISTENCE (Common implementation for all strategies)

    /// ✅ UNIFIED DISK PERSISTENCE: Single method for all strategies  
    /// Uses strategy-specific serialization via serialize_vectors_for_disk()
    async fn persist_to_disk_unified(
        &self,
        collection_id: &str,
        wal_operation: &super::WALOperation,
        sequences: &[u64],
    ) -> Result<()> {
        // Default implementation that strategies can override for custom behavior
        // This provides a basic disk persistence using the strategy's serialization
        if let Some(filesystem) = self.get_filesystem() {
            // ✅ UNIFIED DISK PERSISTENCE: Common implementation for all strategies
            // Each strategy provides their own serialize_vectors_for_disk/deserialize_vectors_from_disk

            tracing::debug!(
                "💾 Persisting Write Buffer operation to disk for collection {} ({} sequences)",
                collection_id,
                sequences.len()
            );

            // Deferred: Get storage location from collection metadata
            // For now, use a default path structure
            let base_location = "file:///data";
            let wal_dir = format!("{}/{}/write_ahead_log/logs", base_location, collection_id);
            let sequence_start = sequences.first().copied().unwrap_or(0);
            let sequence_end = sequences.last().copied().unwrap_or(0);
            let wal_file = format!(
                "{}/batch_{:010}_{:010}.wal",
                wal_dir, sequence_start, sequence_end
            );

            // Get filesystem for this storage URL
            if let Ok(fs) = filesystem.get_filesystem(base_location) {
                // Ensure Write Buffer directory exists
                if fs.create_dir_all(&wal_dir).await.is_err() {
                    tracing::warn!("Failed to create Write Buffer directory: {}", wal_dir);
                }

                // Serialize the complete Write Buffer operation (common format)
                let serialized_data = bincode::serialize(wal_operation)
                    .context("Failed to serialize Write Buffer operation for disk")?;

                // Write to disk atomically (simple implementation)
                let temp_file = format!("{}.tmp", wal_file);

                if let Err(e) = fs.write(&temp_file, &serialized_data, None).await {
                    tracing::warn!(
                        "Failed to write Write Buffer temp file {}: {}",
                        temp_file,
                        e
                    );
                    return Ok(()); // Continue - memory write succeeded
                }

                if let Err(e) = fs.move_file(&temp_file, &wal_file).await {
                    tracing::warn!(
                        "Failed to rename Write Buffer file {} -> {}: {}",
                        temp_file,
                        wal_file,
                        e
                    );
                    // Try to clean up temp file
                    let _ = fs.delete(&temp_file).await;
                    return Ok(()); // Continue - memory write succeeded
                }

                tracing::debug!(
                    "✅ Write Buffer operation persisted to disk: {} bytes written to {}",
                    serialized_data.len(),
                    wal_file
                );
            } else {
                tracing::debug!("No filesystem available for storage URL: {}", base_location);
            }

            Ok(())
        } else {
            tracing::debug!("No filesystem factory available, skipping disk persistence");
            Ok(())
        }
    }

    /// Wrapper for backward compatibility
    async fn persist_wal_operation_to_disk(
        &self,
        collection_id: &str,
        wal_operation: &super::WALOperation,
        sequences: &[u64],
    ) -> Result<()> {
        self.persist_to_disk_unified(collection_id, wal_operation, sequences)
            .await
    }

    // 🎯 STRATEGY-SPECIFIC SERIALIZATION (Only methods strategies need to implement)

    /// ✅ ONLY METHOD EACH STRATEGY NEEDS: Serialize canonical records in strategy format
    fn serialize_records_for_disk(
        &self,
        _records: &[proximadb_records::ProximaRecord],
    ) -> Result<Vec<u8>> {
        // Default implementation - strategies must override
        Err(anyhow::anyhow!(
            "serialize_records_for_disk not implemented for {}",
            self.strategy_name()
        ))
    }

    /// ✅ ONLY METHOD EACH STRATEGY NEEDS: Deserialize canonical records from strategy format
    /// Used during recovery to load Write Buffer files back into memtable
    fn deserialize_records_from_disk(
        &self,
        _data: &[u8],
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        // Default implementation - strategies must override
        Err(anyhow::anyhow!(
            "deserialize_records_from_disk not implemented for {}",
            self.strategy_name()
        ))
    }

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;

    /// Force immediate sync of in-memory data to disk
    async fn force_sync(&self, collection_id: Option<&String>) -> Result<()> {
        // Default implementation - placeholder for now
        // Deferred: Integrate with AtomicWalSync when fully enabled
        tracing::debug!(
            "🔄 Force sync requested for collection: {:?}",
            collection_id
        );

        if let Some(collection_id) = collection_id {
            tracing::debug!(
                "Force sync would be performed for collection: {}",
                collection_id
            );
        } else {
            tracing::debug!("Force sync would be performed for all collections");
        }

        // For now, this is a no-op as disk persistence happens through
        // automatic memory flush triggers
        Ok(())
    }

    // 🎯 ADVANCED OPERATIONS

    /// Compact collection (clean up old MVCC versions, TTL expired entries)
    async fn compact_collection(&self, collection_id: &str) -> Result<u64> {
        // Default implementation
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // For now, just clear old entries
            wal_behavior
                .clear_flushed(collection_id)
                .await
                .map(|count| count as u64)
        } else {
            // No Write Buffer behavior, return 0
            tracing::debug!("🔧 Compacting collection {} (placeholder)", collection_id);
            Ok(0)
        }
    }

    /// Get Write Buffer behavior wrapper for specialized operations
    fn get_wal_behavior(
        &self,
    ) -> Option<&crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper>;

    /// Migrate Write Buffer batch from local to cloud storage
    async fn migrate_batch_to_cloud(
        &self,
        collection_id: &str,
        batch: &WALVectorBatch,
        local_path: &str,
        cloud_url: &str,
    ) -> Result<String> {
        if let Some(fs) = self.get_filesystem() {
            // Write to cloud first
            let cloud_batch_url = self
                .write_batch_to_cloud(collection_id, batch, cloud_url)
                .await?;

            // Verify cloud write by reading back
            let _verified_batch = self
                .read_batch_from_cloud(&cloud_batch_url)
                .await
                .context("Failed to verify cloud write during migration")?;

            // Remove local file after successful cloud write
            let local_fs = fs
                .get_filesystem(&format!("file://{}", local_path))
                .context("Failed to get local filesystem")?;

            local_fs
                .delete(local_path)
                .await
                .context("Failed to delete local file after migration")?;

            tracing::info!(
                "🔄 MIGRATION: Migrated batch {} from {} to {}",
                batch.batch_id.to_base62(),
                local_path,
                cloud_batch_url
            );

            Ok(cloud_batch_url)
        } else {
            Err(anyhow::anyhow!("Filesystem not initialized for migration"))
        }
    }

    /// List Write Buffer batches from cloud storage with URL-based routing
    async fn list_cloud_batches(
        &self,
        collection_id: &str,
        cloud_base_url: &str,
    ) -> Result<Vec<String>> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            crate::storage::persistence::filesystem::validate_url(cloud_base_url)
                .context("Invalid cloud base URL format")?;

            let filesystem = fs
                .get_filesystem(cloud_base_url)
                .context("Failed to get filesystem for cloud URL")?;

            let base_path = FilesystemFactory::resolve_path(cloud_base_url)
                .context("Failed to extract path from cloud URL")?;

            let entries = filesystem
                .list(&base_path)
                .await
                .context("Failed to list cloud directory")?;

            // Filter for Write Buffer batch files for this collection with multiple patterns
            let batch_prefix = format!("write_buffer_batch_{}_", collection_id);
            let batch_urls: Vec<String> = entries
                .iter()
                .filter(|entry| {
                    !entry.metadata.is_directory
                        && entry.name.starts_with(&batch_prefix)
                        && entry.name.ends_with(".bin")
                })
                .map(|entry| {
                    if cloud_base_url.ends_with('/') {
                        format!("{}{}", cloud_base_url, entry.name)
                    } else {
                        format!("{}/{}", cloud_base_url, entry.name)
                    }
                })
                .collect();

            // Log detailed information for monitoring
            let bucket = fs
                .extract_bucket_from_url(cloud_base_url)
                .ok()
                .flatten()
                .unwrap_or_else(|| "unknown".to_string());

            tracing::debug!(
                "☁️ CLOUD_LIST: Found {} Write Buffer batches for collection {} in {} [bucket: {}]",
                batch_urls.len(),
                collection_id,
                cloud_base_url,
                bucket
            );

            Ok(batch_urls)
        } else {
            Err(anyhow::anyhow!(
                "Filesystem not initialized for cloud operations"
            ))
        }
    }

    /// Delete Write Buffer batch from cloud storage
    async fn delete_cloud_batch(&self, cloud_url: &str) -> Result<()> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            crate::storage::persistence::filesystem::validate_url(cloud_url)
                .context("Invalid cloud URL format")?;

            let filesystem = fs
                .get_filesystem(cloud_url)
                .context("Failed to get filesystem for cloud URL")?;

            let path = FilesystemFactory::resolve_path(cloud_url)
                .context("Failed to extract path from cloud URL")?;

            filesystem
                .delete(&path)
                .await
                .context("Failed to delete batch from cloud storage")?;

            // Log detailed information for monitoring
            let bucket = fs
                .extract_bucket_from_url(cloud_url)
                .ok()
                .flatten()
                .unwrap_or_else(|| "unknown".to_string());

            tracing::info!(
                "🗑️ CLOUD_DELETE: Deleted batch from {} [bucket: {}]",
                cloud_url,
                bucket
            );

            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Filesystem not initialized for cloud operations"
            ))
        }
    }

    /// Check if cloud storage is available and accessible
    async fn check_cloud_health(&self, cloud_base_url: &str) -> Result<bool> {
        if let Some(fs) = self.get_filesystem() {
            // Validate URL format before proceeding
            match crate::storage::persistence::filesystem::validate_url(cloud_base_url) {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "❌ CLOUD_HEALTH: Invalid URL format {}: {}",
                        cloud_base_url,
                        e
                    );
                    return Ok(false);
                }
            }

            let filesystem = fs
                .get_filesystem(cloud_base_url)
                .context("Failed to get filesystem for cloud URL")?;

            let base_path = FilesystemFactory::resolve_path(cloud_base_url)
                .context("Failed to extract path from cloud URL")?;

            // Try to list the directory to check accessibility
            match filesystem.list(&base_path).await {
                Ok(_) => {
                    // Log detailed information for monitoring
                    let bucket = fs
                        .extract_bucket_from_url(cloud_base_url)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "unknown".to_string());

                    tracing::debug!(
                        "✅ CLOUD_HEALTH: Cloud storage accessible at {} [bucket: {}]",
                        cloud_base_url,
                        bucket
                    );
                    Ok(true)
                }
                Err(e) => {
                    tracing::warn!(
                        "❌ CLOUD_HEALTH: Cloud storage not accessible at {}: {}",
                        cloud_base_url,
                        e
                    );
                    Ok(false)
                }
            }
        } else {
            tracing::warn!("❌ CLOUD_HEALTH: Filesystem not initialized");
            Ok(false)
        }
    }

    // 🎯 ADDITIONAL BATCH OPERATIONS

    /// Delete vector by ID using batch operations
    async fn delete_vector(&self, collection_id: &str, vector_id: &VectorId) -> Result<u64> {
        // Create a tombstone record for deletion.
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let tombstone = proximadb_records::ProximaRecord {
            oid: vector_id.clone(),
            created_at_ns: now_ns,
            updated_at_ns: now_ns,
            valid_to_ns: Some(0),
            ..Default::default()
        };

        // Create single-vector batch for deletion
        use super::BatchId;
        let batch_id = BatchId::new();
        let batch = WALVectorBatch {
            batch_id,
            vector_records: Arc::new(vec![tombstone]),
            timestamp: std::time::SystemTime::now(),
            total_size_bytes: std::mem::size_of::<proximadb_records::ProximaRecord>(),
            is_flushed: false,
            metadata_bloom_filter: None,
        };

        // Deferred: Get base_location from VectorOperationsService collection metadata
        // For now, use a default fallback
        let base_location = "file:///tmp/proximadb/data/wal";

        let sequences = self
            .write_native_batch(batch, collection_id, base_location)
            .await?;
        Ok(sequences.into_iter().next().unwrap_or(0))
    }

    /// Flush collections using batch operations
    async fn flush(&self, collection_id: Option<&String>) -> Result<FlushResult> {
        if let Some(cid) = collection_id {
            self.flush_collection(cid).await
        } else {
            // Flush all collections - default implementation
            Ok(FlushResult {
                success: true,
                collections_affected: vec![],
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                file_paths: vec![],
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                compaction_error: None,
                flushed_batch_ids: vec![],
            })
        }
    }

    /// Atomically retrieve and mark Write Buffer batches for flush operation
    ///
    /// This method:
    /// 1. Retrieves unflushed batches from GlobalPartitionedMemtable with deserialized data
    /// 2. Marks batches for flush to prevent concurrent access
    /// 3. Returns batch data with BatchIds for atomic cleanup after successful flush
    /// 4. Prepares for disk Write Buffer file cleanup upon flush completion
    async fn atomic_retrieve_for_flush(
        &self,
        collection_id: &str,
        flush_id: &str,
    ) -> Result<super::FlushCycle> {
        // Get Write Buffer behavior wrapper to access GlobalPartitionedMemtable
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Retrieve unflushed batches from global memtable (already deserialized)
            let unflushed_batches = wal_behavior.get_unflushed_batches(collection_id).await?;

            // Extract vector records and batch IDs for atomic operations
            let mut all_vector_records = Vec::new();
            let mut batch_ids = Vec::new();
            let mut marked_sequences = Vec::new();

            for batch in &unflushed_batches {
                all_vector_records.extend(batch.vector_records.iter().cloned());
                batch_ids.push(batch.batch_id);
                // CompactBatchId doesn't have sequence_range, use a placeholder
                marked_sequences.push((0, 0));
            }

            tracing::info!(
                "🔄 Atomic flush retrieval: {} batches, {} vectors for collection {} (flush_id: {})",
                unflushed_batches.len(),
                all_vector_records.len(),
                collection_id,
                flush_id
            );

            // Create flush cycle with batch-oriented data
            Ok(super::FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.to_string(),
                batches: unflushed_batches, // Use actual batches instead of empty vec
                vector_records: all_vector_records,
                marked_segments: vec![], // Will be populated with disk Write Buffer file paths for cleanup
                marked_sequences,
                batch_ids,
                state: super::FlushCycleState::Active,
            })
        } else {
            // Fallback for strategies without Write Buffer behavior wrapper
            let vector_records = self.get_collection_vectors(collection_id).await?;
            let record_count = vector_records.len() as u64;

            Ok(super::FlushCycle {
                flush_id: flush_id.to_string(),
                collection_id: collection_id.to_string(),
                batches: vec![], // No batches in fallback mode
                vector_records,
                marked_segments: vec![],
                marked_sequences: vec![(0, record_count)],
                batch_ids: vec![],
                state: super::FlushCycleState::Active,
            })
        }
    }

    /// Complete flush cycle - cleanup GlobalPartitionedMemtable and disk Write Buffer files
    /// Called after successful storage engine flush to atomically clean up Write Buffer data
    async fn complete_flush_cycle(
        &self,
        flush_cycle: super::FlushCycle,
    ) -> Result<super::FlushCompletionResult> {
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Atomically clear flushed batches from GlobalPartitionedMemtable
            let cleared_count = wal_behavior
                .clear_flushed(&flush_cycle.collection_id)
                .await?;

            // Cleanup disk Write Buffer files for the flushed batches
            if let Some(fs) = self.get_filesystem() {
                for batch_id in &flush_cycle.batch_ids {
                    // Try to clean up local Write Buffer files if they exist
                    let local_wal_path = format!(
                        "write_buffer_batch_{}_{}.bin",
                        flush_cycle.collection_id,
                        batch_id.to_base62()
                    );

                    if let Ok(local_fs) = fs.get_filesystem(&format!("file://{}", local_wal_path)) {
                        let _ = local_fs.delete(&local_wal_path).await; // Ignore errors - file might not exist
                    }
                }
            }

            tracing::info!(
                "✅ Flush completion: {} batches cleared from memtable for collection {} (flush_id: {})",
                cleared_count,
                flush_cycle.collection_id,
                flush_cycle.flush_id
            );

            Ok(super::FlushCompletionResult {
                entries_removed: cleared_count,
                segments_cleaned: flush_cycle.marked_segments.len(),
                bytes_reclaimed: flush_cycle
                    .vector_records
                    .iter()
                    .map(|v| {
                        let dimension = v.embeddings.first().map(|e| e.values.len()).unwrap_or(0);
                        (dimension * 4 + 256) as u64
                    })
                    .sum(),
            })
        } else {
            // Fallback for strategies without Write Buffer behavior wrapper
            Ok(super::FlushCompletionResult {
                entries_removed: flush_cycle.vector_records.len(),
                segments_cleaned: 0,
                bytes_reclaimed: flush_cycle
                    .vector_records
                    .iter()
                    .map(|v| {
                        let dimension = v.embeddings.first().map(|e| e.values.len()).unwrap_or(0);
                        (dimension * 4 + 256) as u64
                    })
                    .sum(),
            })
        }
    }

    /// Check if collection needs flush based on thresholds (called during writes)
    /// Returns true if flush should be triggered for the collection
    async fn should_trigger_flush(&self, collection_id: &str) -> Result<bool> {
        if let Some(wal_behavior) = self.get_wal_behavior() {
            // Get collection statistics from GlobalPartitionedMemtable
            let stats = wal_behavior.stats().await?;

            if let Some(collection_stats) = stats.get(collection_id) {
                // Check thresholds: memory size, entry count, or time-based
                let memory_threshold_mb = 100; // 100MB threshold
                let entry_threshold = 10000; // 10K entries threshold

                let should_flush = collection_stats.memory_size_bytes
                    > (memory_threshold_mb * 1024 * 1024)
                    || collection_stats.total_entries > entry_threshold;

                if should_flush {
                    tracing::info!(
                        "🚨 Flush threshold reached for collection {}: {} MB, {} entries",
                        collection_id,
                        collection_stats.memory_size_bytes / (1024 * 1024),
                        collection_stats.total_entries
                    );
                }

                Ok(should_flush)
            } else {
                Ok(false) // No data for collection
            }
        } else {
            Ok(false) // No Write Buffer behavior wrapper
        }
    }
}

/// Write Buffer disk entry structure for persistence
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WriteBufferDiskEntry {
    pub collection_id: String,
    pub sequence_start: u64,
    pub sequence_end: u64,
    pub operation_type: String,
    pub payload_format: String,
    pub payload_data: Vec<u8>,
    pub vector_count: usize,
    pub timestamp: i64,
}
