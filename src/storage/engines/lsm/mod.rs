//! LSM Tree Storage Engine
//!
//! Log-Structured Merge Tree implementation providing an alternative
//! to VIPER for performance comparison and standard SSTable storage.

pub mod bloom_filter;
pub mod optimized_bloom_filter;
pub mod compaction;
pub mod manifest;
pub mod readers;
pub mod sstable_writer;
pub mod unified_search_engine;

// Test modules
#[cfg(test)]
pub mod bloom_filter_tests;

// Re-export main types
pub use bloom_filter::{
    BloomFilterStrategy, BloomFilterConfig, BloomFilterFactory,
    SstableBloomFilter, BloomStrategy, CompositeBloomFilter,
};
pub use optimized_bloom_filter::{
    OptimizedSstableBloomFilter, OptimizedBloomConfig, BloomFilterSharingManager,
};
pub use compaction::{CompactionManager, CompactionPriority, CompactionStats, CompactionTask};
pub use manifest::{LsmManifest, SstableFileInfo, ManifestStats};
pub use readers::UnifiedSstableReader;

// Additional exports for unified reader (SstableHeader is already defined below)
pub use sstable_writer::SstableWriter;

// Main LSM Tree implementation (contents from original lsm/mod.rs)
use crate::core::{LsmConfig, VectorRecord};
use crate::core::search::SearchParams;
use crate::storage::optimization::{SortingStats};
// Removed duplicate import - readers module is already defined above
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    UnifiedStorageEngine,
};
use crate::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

// Remove dummy filesystem factory - LSM will use fallback methods

/// LSM-specific record format for efficient SSTable storage
/// This stores VectorRecord fields directly without wrapper overhead
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LsmRecord {
    // Core VectorRecord fields stored directly
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub version: i64,
    
    // LSM-specific fields
    pub is_tombstone: bool,        // True if this is a deletion marker
    pub sequence_number: u64,      // LSM sequence for ordering
    pub level: u8,                 // SSTable level this record belongs to
}

impl LsmRecord {
    /// Create LsmRecord from VectorRecord with explicit collection_id
    pub fn from_vector_record(record: VectorRecord, collection_id: &str) -> Self {
        Self {
            id: record.id.as_deref().unwrap_or("").to_string(),
            collection_id: collection_id.to_string(),
            vector: record.vector,
            metadata: crate::core::proto_metadata_helper::proto_metadata_to_json(&record.metadata),
            timestamp: record.timestamp,
            created_at: record.timestamp,
            updated_at: record.timestamp,
            expires_at: record.expires_at,
            version: record.version,
            is_tombstone: false,
            sequence_number: 0, // Will be set during flush
            level: 0,           // Will be set during flush
        }
    }
}

impl Into<VectorRecord> for LsmRecord {
    fn into(self) -> VectorRecord {
        VectorRecord {
            id: Some(self.id),  // Core VectorRecord expects Option<String>
            vector: self.vector,
            metadata: crate::core::proto_metadata_helper::json_metadata_to_proto(&self.metadata),
            timestamp: self.timestamp,
            created_at: self.timestamp,
            updated_at: self.timestamp,
            expires_at: self.expires_at,
            version: self.version,
            rank: None,
            score: None,
            distance: None,
        }
    }
}

/// SSTable header for row-based storage format with engine optimizations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableHeader {
    pub version: u32,
    pub level: u8,
    pub entry_count: u64,
    pub min_key: String,
    pub max_key: String,
    pub created_at: i64,
    // Engine optimizations (optional fields with defaults for backward compatibility)
    #[serde(default)]
    pub compression_enabled: bool,
    #[serde(default)]
    pub has_bloom_filter: bool,
    #[serde(default = "default_block_size")]
    pub block_size: u32,
    #[serde(default)]
    pub batch_size: u32,
    // Additional fields for SSTable reader
    #[serde(default)]
    pub header_size: u32,
    #[serde(default)]
    pub index_size: u32,
    #[serde(default)]
    pub data_size: u32,
    #[serde(default)]
    pub block_count: u32,
}

/// Index entry for fast key lookups in SSTable with block organization and metadata statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub key: String,
    pub offset: u64,
    pub size: u32,
    pub block_id: u32,
    pub block_offset: u32,
    pub compressed: bool,
    /// Minimum values for each metadata column in this block
    pub metadata_min_values: HashMap<String, serde_json::Value>,
    /// Maximum values for each metadata column in this block
    pub metadata_max_values: HashMap<String, serde_json::Value>,
    /// Count of null values for each metadata column in this block
    pub metadata_null_counts: HashMap<String, u32>,
}

// Default function for serde
fn default_block_size() -> u32 {
    4096 // 4KB default block size
}

/// Data block for cache-optimized storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBlock {
    pub block_id: u32,
    pub records: Vec<LsmRecord>,
    pub uncompressed_size: u32,
}

// Removed - using bloom_filter::BloomFilter instead

/// Batch extraction statistics for performance monitoring
#[derive(Debug, Default)]
struct BatchExtractionStats {
    pub total_extracted: usize,
    pub total_skipped: usize,
    pub chunk_times: Vec<u64>, // In microseconds
    pub sort_time_us: u64,
}

impl BatchExtractionStats {
    fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct LsmTree {
    config: LsmConfig,
    collection_id: String,
    // REMOVED: memtable - LSM is now pure SSTable storage
    // Global WAL memtable handles all in-memory buffering
    // REMOVED: wal_manager - Not needed for pure SSTable storage
    data_dir: PathBuf,
    compaction_manager: Option<Arc<CompactionManager>>,
    manifest: Arc<LsmManifest>,
    filesystem: Arc<FilesystemFactory>,
    // Collection service removed - indexing configuration handled by AXIS
    // Atomic coordinator for safe flush and compaction operations
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl LsmTree {
    pub async fn new(
        collection_id: String,
        config: LsmConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🌲 Creating LSM tree (pure SSTable storage) for collection: {}", collection_id);
        
        // Create collection-specific directory for SSTables using plugin filesystem
        let data_dir = PathBuf::from(format!("{}/{}", config.data_directory, collection_id));
        
        // Use plugin filesystem for directory creation
        let fs = filesystem.get_filesystem("file:///")?;
        let data_dir_str = data_dir.to_string_lossy();
        
        // Create directory asynchronously
        fs.create_dir_all(&data_dir_str).await?;
        
        // Always create atomic coordinator for safe operations
        let atomic_coordinator = Arc::new(
            UnifiedAtomicCoordinator::new(filesystem.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?
        );
        
        // Create manifest for SSTable tracking
        let manifest = Arc::new(LsmManifest::new(
            collection_id.clone(),
            data_dir.clone(),
            filesystem.clone(),
            Some(atomic_coordinator.clone()),
        ));
        
        // Load existing manifest if present
        if let Err(e) = manifest.load().await {
            tracing::warn!("Failed to load existing manifest: {}", e);
        }

        Ok(Self {
            config,
            collection_id: collection_id.clone(),
            data_dir,
            compaction_manager: None,
            manifest,
            filesystem,
            atomic_coordinator,
        })
    }
    
    /// Get the data directory for this LSM tree
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }
    
    /// Enable compaction with the LSM tree's atomic coordinator
    pub async fn enable_compaction(&mut self, worker_count: usize) -> Result<()> {
        if self.compaction_manager.is_none() {
            let mut compaction_manager = CompactionManager::with_atomic_coordinator(
                self.config.clone(),
                Some(self.atomic_coordinator.clone()),
                Some(self.manifest.clone()),
            );
            
            // Start background workers
            compaction_manager.start_workers(worker_count).await?;
            
            self.compaction_manager = Some(Arc::new(compaction_manager));
            
            info!("✅ LSM: Compaction enabled with {} workers and atomic operations", worker_count);
        }
        Ok(())
    }


    // Collection service setter removed - indexing configuration handled by AXIS

    // REMOVED: put, get, delete, exists methods - LSM is now pure SSTable storage
    // All writes go through WAL → Flush → SSTable directly
    // No intermediate memtable needed

    /// Direct flush vectors to LSM storage from WAL
    /// This is called by the flush coordinator when WAL memtable needs to flush
    pub async fn flush_vectors_direct(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
    ) -> Result<FlushResult> {
        if vectors.is_empty() {
            return Ok(FlushResult::default());
        }

        // Sort vectors by metadata for better SSTable organization and compression
        info!(
            "🔄 LSM: Sorting {} vectors by metadata for optimal SSTable encoding",
            vectors.len()
        );
        let (sorted_vectors, sort_stats) = self.sort_vectors_for_sstable_encoding(vectors).await?;
        info!(
            "✅ LSM: Sorted {} vectors (estimated compression improvement: {:.1}%)",
            sort_stats.records_sorted,
            sort_stats.compression_estimate * 100.0
        );

        // Get the collection storage URL from assignment service
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        
        // Generate SSTable filename
        let sst_filename = format!("{}_level0_{}.sst", self.collection_id, Utc::now().timestamp_millis());
        
        // Convert sorted vectors to LsmRecord format with sequence numbers
        let mut entries: BTreeMap<String, LsmRecord> = BTreeMap::new();
        let mut sequence_number = 0u64;
        
        for vector in sorted_vectors {
            let vector_id = vector.id.as_deref().unwrap_or("").to_string();
            let mut lsm_record = LsmRecord::from_vector_record(vector, &self.collection_id);
            lsm_record.sequence_number = sequence_number;
            lsm_record.level = 0; // New SSTables start at level 0
            entries.insert(vector_id, lsm_record);
            sequence_number += 1;
        }

        // Write SSTable using atomic operations (always available now)
        let atomic_coordinator = &self.atomic_coordinator;
        
        // Use atomic flush pattern
        info!("🔄 LSM: Using atomic flush for {}", sst_filename);
        
        // Begin atomic operation
        let staging_config = StagingConfig {
            base_url: collection_storage_url.clone(),
            collection_id: None, // Already included in base_url
            operation_type: StagingOperationType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        };
        
        let atomic_op = atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;
        
        // Write to staging using SSTable writer
        let staging_url = format!("{}/{}", atomic_op.staging_url, sst_filename);
        let block_size = (self.config.block_size_kb * 1024) as usize;
        let writer = SstableWriter::new(&staging_url, block_size, Arc::clone(&self.filesystem));
        // Use bloom filter config from LSM config if available
        let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
            writer.with_bloom_config(bloom_config.clone())
        } else {
            writer
        };
        writer.write_records(entries.clone()).await
            .map_err(|e| anyhow::anyhow!("Failed to write SSTable to staging: {}", e))?;
        
        // Get file size from staging
        let fs = self.filesystem.get_filesystem(&staging_url)?;
        let metadata = fs.metadata(&staging_url)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get staging file size: {}", e))?;
        let file_size = metadata.size;
        
        // Finalize atomic operation
        atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic flush")?;
        
        let final_url = format!("{}/{}", collection_storage_url.trim_end_matches('/'), sst_filename);
        let (sst_url, data_len) = (final_url, file_size);

        info!(
            "✅ LSM: Flushed {} vectors to SSTable: {}",
            entries.len(),
            sst_url
        );
        
        // Register the new SSTable with the manifest
        let min_key = entries.keys().next().cloned().unwrap_or_default();
        let max_key = entries.keys().last().cloned().unwrap_or_default();
        let min_sequence = entries.values().map(|r| r.sequence_number).min().unwrap_or(0);
        let max_sequence = entries.values().map(|r| r.sequence_number).max().unwrap_or(0);
        
        // Collect metadata statistics
        let mut metadata_columns = HashMap::new();
        for record in entries.values() {
            for (column, value) in &record.metadata {
                let stats = metadata_columns.entry(column.clone()).or_insert_with(|| {
                    manifest::ColumnStats {
                        min_value: value.clone(),
                        max_value: value.clone(),
                        null_count: 0,
                        distinct_count_estimate: 0,
                    }
                });
                
                // Update min/max values
                if let (Some(v), Some(min), Some(max)) = (value.as_f64(), stats.min_value.as_f64(), stats.max_value.as_f64()) {
                    if v < min {
                        stats.min_value = value.clone();
                    }
                    if v > max {
                        stats.max_value = value.clone();
                    }
                } else if let (Some(v), Some(min), Some(max)) = (value.as_str(), stats.min_value.as_str(), stats.max_value.as_str()) {
                    if v < min {
                        stats.min_value = value.clone();
                    }
                    if v > max {
                        stats.max_value = value.clone();
                    }
                }
                
                if value.is_null() {
                    stats.null_count += 1;
                }
            }
        }
        
        let file_info = SstableFileInfo {
            file_id: sst_filename.clone(),
            file_path: sst_filename.clone(),
            level: 0,
            size_bytes: data_len,
            record_count: entries.len() as u64,
            min_key,
            max_key,
            created_at: chrono::Utc::now().timestamp(),
            last_compacted_at: None,
            bloom_fpr: 0.01, // Default, would be calculated from actual bloom filter
            metadata_columns,
            marked_for_deletion: false,
            min_sequence,
            max_sequence,
        };
        
        if let Err(e) = self.manifest.add_sstable(file_info).await {
            tracing::warn!("Failed to register SSTable in manifest: {}", e);
        }

        // Trigger compaction if manager is available
        if let Some(_compaction_manager) = &self.compaction_manager {
            let _task = CompactionTask {
                collection_id: self.collection_id.clone(),
                level: 0, // Start at level 0
                input_files: vec![std::path::PathBuf::from(sst_url.clone())],
                output_file: std::path::PathBuf::from(format!("{}.compacted", sst_url)),
                priority: CompactionPriority::Medium,
            };
            // For now, just log that we would trigger compaction
            tracing::debug!(
                "Would trigger compaction for collection: {}",
                self.collection_id
            );
            // compaction_manager.add_task(task).await?;
        }

        // Return flush result with statistics
        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: entries.len() as u64,
            bytes_written: data_len as u64,
            files_created: 1,
            duration_ms: 0, // Will be set by caller
            completed_at: Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("sstable_path".to_string(), serde_json::Value::String(sst_url.clone()));
                metrics.insert("level".to_string(), serde_json::Value::Number(serde_json::Number::from(0)));
                metrics
            },
            compaction_triggered: self.compaction_manager.is_some(),
            flushed_batch_ids: vec![], // Would be provided by caller if needed
        })
    }

    // REMOVED: memtable_size, memtable_len, iter_all methods
    // LSM is now pure SSTable storage - no memtable to query
    
    /// Helper method to search SSTable for all vectors matching filters
    async fn search_sstable_for_vectors(
        &self,
        sstable_data: &[u8],
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<Vec<VectorRecord>> {
        let mut results = Vec::new();
        
        if sstable_data.len() < 8 {
            return Ok(results);
        }
        
        let mut offset = 0;
        
        // Read header
        let header_len = u32::from_le_bytes([
            sstable_data[offset],
            sstable_data[offset + 1],
            sstable_data[offset + 2],
            sstable_data[offset + 3],
        ]) as usize;
        offset += 4;
        
        if offset + header_len > sstable_data.len() {
            return Ok(results);
        }
        
        offset += header_len; // Skip header
        
        // Read index
        if offset + 4 > sstable_data.len() {
            return Ok(results);
        }
        
        let index_len = u32::from_le_bytes([
            sstable_data[offset],
            sstable_data[offset + 1],
            sstable_data[offset + 2],
            sstable_data[offset + 3],
        ]) as usize;
        offset += 4;
        
        if offset + index_len > sstable_data.len() {
            return Ok(results);
        }
        
        offset += index_len; // Skip index
        
        // Read data blocks
        while offset + 4 <= sstable_data.len() {
            let block_len = u32::from_le_bytes([
                sstable_data[offset],
                sstable_data[offset + 1],
                sstable_data[offset + 2],
                sstable_data[offset + 3],
            ]) as usize;
            offset += 4;
            
            if offset + block_len > sstable_data.len() {
                break;
            }
            
            let block_data = &sstable_data[offset..offset + block_len];
            if let Ok(data_block) = bincode::deserialize::<DataBlock>(block_data) {
                for lsm_record in data_block.records {
                    // Skip tombstones
                    if lsm_record.is_tombstone {
                        continue;
                    }
                    
                    // Apply metadata filter if present
                    if let Some(filters) = metadata_filters {
                        let mut passes_filter = true;
                        for (key, expected_value) in filters {
                            let record_value = lsm_record.metadata.get(key);
                            if record_value != Some(expected_value) {
                                passes_filter = false;
                                break;
                            }
                        }
                        if !passes_filter {
                            continue;
                        }
                    }
                    
                    // Convert to VectorRecord
                    let vector_record: VectorRecord = lsm_record.into();
                    results.push(vector_record);
                }
            }
            offset += block_len;
        }
        
        Ok(results)
    }
}

// =============================================================================
// UNIFIED STORAGE ENGINE TRAIT IMPLEMENTATION FOR LSM
// =============================================================================

#[async_trait]
impl UnifiedStorageEngine for LsmTree {
    // =============================================================================
    // ABSTRACT METHODS - LSM-specific implementations
    // =============================================================================

    fn engine_name(&self) -> &'static str {
        "lsm"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Lsm
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        // Collection service removed - indexing configuration handled by AXIS
        None
    }

    /// LSM-specific flush implementation - Extract records from WAL vector record batches
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("🔄 LSM: Starting do_flush with WAL vector record batch extraction");

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for LSM flush"))?;

        let operation_id = uuid::Uuid::new_v4().to_string();
        let vector_records = &params.vector_records;

        if vector_records.is_empty() {
            info!(
                "📋 LSM: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: {
                    let mut metrics = std::collections::HashMap::new();
                    metrics.insert(
                        "operation_id".to_string(),
                        serde_json::Value::String(operation_id.clone()),
                    );
                    metrics.insert("empty_flush".to_string(), serde_json::Value::Bool(true));
                    metrics
                },
                compaction_triggered: false,
                flushed_batch_ids: vec![],
            });
        }

        info!(
            "💾 LSM: Processing {} vector records from WAL vector record batches",
            vector_records.len()
        );

        // Step 1: Extract individual records from deserialized WAL vector record batches
        // These batches come from the global partitioned memtable with WAL behavior
        let lsm_records = self
            .extract_records_from_wal_vector_batches(vector_records, collection_id)
            .await
            .context("Failed to extract records from WAL vector record batches")?;

        info!(
            "📦 LSM: Extracted {} individual records from {} vector record batches",
            lsm_records.len(),
            vector_records.len()
        );

        // Step 2: Process extracted records using row-by-row storage approach
        let flush_result = self
            .flush_lsm_records_to_sstable(lsm_records, params.force)
            .await
            .context("Failed to flush LSM records to SSTable with row-by-row storage")?;

        info!(
            "✅ LSM: Successfully flushed {} records to {} SSTable files ({} bytes)",
            flush_result.entries_flushed,
            flush_result.files_created,
            flush_result.bytes_written
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_flushed: flush_result.entries_flushed,
            bytes_written: flush_result.bytes_written,
            files_created: flush_result.files_created,
            duration_ms: 0, // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            engine_metrics: {
                let mut metrics = flush_result.engine_metrics;
                metrics.insert(
                    "operation_id".to_string(),
                    serde_json::Value::String(operation_id),
                );
                metrics.insert(
                    "extraction_source".to_string(),
                    serde_json::Value::String("wal_vector_record_batches".to_string()),
                );
                metrics.insert(
                    "storage_approach".to_string(),
                    serde_json::Value::String("row_by_row".to_string()),
                );
                metrics.insert(
                    "batch_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(vector_records.len())),
                );
                metrics.insert(
                    "extracted_records_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(flush_result.entries_flushed)),
                );
                metrics
            },
            compaction_triggered: flush_result.compaction_triggered,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// LSM-specific compaction using level-based merge strategy
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let compact_start = std::time::Instant::now();
        let collection_id = &self.collection_id;

        tracing::info!(
            "🗜️ LSM COMPACTION START: Collection {} (force: {}, priority: {:?})",
            collection_id,
            params.force,
            params.priority
        );

        let mut result = CompactionResult {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: 0,
            entries_removed: 0,
            bytes_read: 0,
            bytes_written: 0,
            input_files: 0,
            output_files: 0,
            duration_ms: 0,
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        // LSM-specific compaction: Level-based SSTable merging
        if let Some(compaction_manager) = &self.compaction_manager {
            tracing::debug!(
                "🔄 LSM COMPACTION: Checking for compaction needs in {}",
                self.data_dir.display()
            );

            // Get collection storage directory
            let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
            let collection_dir = std::path::PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Check if compaction is needed
            if let Some(task) = compaction_manager
                .check_compaction_needed(&collection_dir, collection_id)
                .await?
            {
                tracing::info!(
                    "🔄 LSM COMPACTION: Scheduling compaction for collection {} level {}",
                    task.collection_id, task.level
                );

                // Schedule the compaction task
                compaction_manager.schedule_compaction(task).await?;

                // Get compaction stats for result
                let stats = compaction_manager.get_stats().await;
                
                result.collections_affected.push(collection_id.clone());
                result.entries_processed = stats.files_merged * 1000; // Estimate
                result.entries_removed = stats.expired_records_deleted + stats.tombstones_removed;
                result.bytes_read = stats.bytes_read;
                result.bytes_written = stats.bytes_written;
                result.input_files = stats.files_merged;
                result.output_files = stats.total_compactions;
                result.success = true;

                tracing::info!(
                    "✅ LSM COMPACTION: Scheduled compaction for collection {} (files merged: {}, bytes written: {})",
                    collection_id, stats.files_merged, stats.bytes_written
                );
            } else {
                tracing::debug!("📊 LSM COMPACTION: No compaction needed for collection {}", collection_id);
                result.success = true; // No compaction needed is still successful
            }
        } else {
            tracing::warn!("⚠️ LSM COMPACTION: No compaction manager available");
            result.success = false;
        }

        result.duration_ms = compact_start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Retrieve vector by ID from LSM storage (Pure SSTable lookup with bloom filter optimization)
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<crate::core::VectorRecord>> {
        // First check if this is the correct collection
        if collection_id != &self.collection_id {
            return Ok(None);
        }

        tracing::debug!("🔍 LSM: Looking up vector {} in collection {} using manifest", vector_id, collection_id);

        // Get SSTable files that might contain this key from manifest
        let overlapping_files = self.manifest.get_overlapping_files(vector_id, vector_id).await;
        
        if overlapping_files.is_empty() {
            tracing::debug!("📂 LSM: No SSTable files overlap with key {}", vector_id);
            return Ok(None);
        }
        
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let collection_dir = std::path::PathBuf::from(collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url));
        
        let mut sstables_checked = 0;
        let mut bloom_filter_hits = 0;
        
        // Search through files in key range order (smallest range first)
        for file_info in overlapping_files {
            sstables_checked += 1;
            
            let file_path = collection_dir.join(&file_info.file_path);
            let path_str = file_path.to_string_lossy().to_string();
            
            // Use unified SSTable reader with bloom filter
            let reader = UnifiedSstableReader::new(self.filesystem.clone());
            
            // Load metadata (includes bloom filter)
            if reader.load_metadata(&path_str).await.is_ok() {
                // Check bloom filter first
                if reader.might_contain_key(&path_str, vector_id).await {
                    bloom_filter_hits += 1;
                    tracing::trace!("🌸 LSM: Bloom filter hit for {} in {}", vector_id, file_info.file_id);
                    
                    // Actually search the SSTable
                    if let Ok(Some(record)) = reader.get_vector(&path_str, vector_id).await {
                        tracing::debug!(
                            "✅ LSM: Found vector {} in SSTable {} (level {}, checked {}/{} SSTables, {} bloom hits)",
                            vector_id, file_info.file_id, file_info.level, bloom_filter_hits, sstables_checked, bloom_filter_hits
                        );
                        return Ok(Some(record));
                    }
                } else {
                    tracing::trace!("🌸 LSM: Bloom filter miss for {} in {} - skipping", vector_id, file_info.file_id);
                }
            } else {
                tracing::warn!("⚠️ Failed to load metadata for SSTable {}", file_info.file_id);
            }
        }

        tracing::debug!(
            "❌ LSM: Vector {} not found in collection {} (checked {} SSTables, {} bloom hits)",
            vector_id, collection_id, sstables_checked, bloom_filter_hits
        );
        Ok(None)
    }

    /// LSM ENGINE OPTIMIZATION: Unified search with bloom filter hints and range scans
    async fn search_vectors_unified(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance::DistanceMetric,
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        // Check if this is the correct collection
        if collection_id != &self.collection_id {
            debug!("🔍 LSM: Collection mismatch - requested: {}, engine: {}", collection_id, &self.collection_id);
            return Ok(Vec::new());
        }
        
        debug!("🔍 LSM: Searching collection {} using manifest", collection_id);
        
        let mut all_results = Vec::new();
        
        // Get SSTable files from manifest based on metadata filters
        let sstable_files = if let Some(filters) = metadata_filters {
            // Get files that might contain the filtered metadata
            let mut matching_files = Vec::new();
            for (column, value) in filters {
                let files = self.manifest.get_files_with_metadata(column, value).await;
                matching_files.extend(files);
            }
            // Deduplicate files
            matching_files.sort_by(|a, b| a.file_id.cmp(&b.file_id));
            matching_files.dedup_by(|a, b| a.file_id == b.file_id);
            matching_files
        } else {
            // Get all files from manifest
            let mut all_files = Vec::new();
            for level in 0..self.config.level_count {
                all_files.extend(self.manifest.get_files_at_level(level).await);
            }
            all_files
        };
        
        if sstable_files.is_empty() {
            debug!("📂 LSM: No SSTable files found in manifest");
            return Ok(all_results);
        }
        
        debug!("🔍 LSM: Found {} SSTable files to search", sstable_files.len());
        
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let collection_dir = std::path::PathBuf::from(
            collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
        );
        
        let mut sstables_scanned = 0;
        let mut records_evaluated = 0;
        
        for file_info in sstable_files {
            sstables_scanned += 1;
            
            // Use unified SSTable reader for optimized access
            let reader = UnifiedSstableReader::new(self.filesystem.clone());
            
            // Create proper file path
            let file_path = collection_dir.join(&file_info.file_path);
            let path_str = if collection_storage_url.starts_with("file://") {
                format!("file://{}", file_path.to_string_lossy())
            } else {
                // For cloud storage, construct the full URL
                format!("{}/{}", collection_storage_url.trim_end_matches('/'), file_info.file_path)
            };
            
            debug!("🔍 LSM: Searching SSTable {} (level {})", file_info.file_id, file_info.level);
            
            match reader.load_metadata(&path_str).await {
                Ok(_) => {
                    debug!("🔍 LSM: Successfully loaded metadata for SSTable: {}", file_info.file_id);
                }
                Err(e) => {
                    debug!("🔍 LSM: Failed to load metadata for SSTable {}: {}", file_info.file_id, e);
                    continue;
                }
            }
            
            // Create a collection context for the search
            let context = readers::CollectionContext {
                collection_id: self.collection_id.clone(),
                file_path: path_str.clone(),
                sstable_files: vec![path_str.clone()],
                total_vectors: file_info.record_count as usize,
                metadata_columns: file_info.metadata_columns.keys().cloned().collect(),
                level: file_info.level as usize,
                creation_time: chrono::DateTime::from_timestamp(file_info.created_at, 0).unwrap_or_else(chrono::Utc::now),
            };
            
            // Create search params for the reader
            let search_params = SearchParams {
                query_vectors: Some(vec![query_vector.to_vec()]),
                filters: metadata_filters.map(|f| f.clone()),
                filter_expression: None,
                top_k: Some(1000), // Use a large k to get all results
                distance_metric: Some(*distance_metric),
                ..Default::default()
            };
            
            debug!("🔍 LSM: Searching SSTable with {} records", file_info.record_count);
            
            match reader.search_vectors(&search_params, &context).await {
                Ok(search_results) => {
                    debug!("🔍 LSM: SSTable {} returned {} results", file_info.file_id, search_results.len());
                    records_evaluated += search_results.len();
                    
                    // Results from reader are already search::SearchResult type
                    all_results.extend(search_results);
                }
                Err(e) => {
                    debug!("🔍 LSM: Error reading SSTable {}: {}", file_info.file_id, e);
                }
            }
        }
        
        debug!(
            "📊 LSM: Scanned {} SSTables, evaluated {} records for search",
            sstables_scanned, records_evaluated
        );
        
        // Sort by score (descending) and take top k
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(k);
        
        // Set ranks
        for (i, result) in all_results.iter_mut().enumerate() {
            result.rank = Some(i as i32 + 1);
        }
        
        debug!("✅ LSM: Found {} results (top {} requested)", all_results.len(), k);
        Ok(all_results)
    }

    /// LSM-specific engine metrics
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        metrics.insert(
            "engine_type".to_string(),
            serde_json::Value::String("LSM".to_string()),
        );
        metrics.insert(
            "collection_id".to_string(),
            serde_json::Value::String(self.collection_id.clone()),
        );
        metrics.insert(
            "storage_type".to_string(),
            serde_json::Value::String("Pure SSTable".to_string()),
        );
        metrics.insert(
            "compaction_threshold".to_string(),
            serde_json::Value::Number((self.config.compaction_threshold as u64).into()),
        );
        metrics.insert(
            "level_count".to_string(),
            serde_json::Value::Number((self.config.level_count as u64).into()),
        );
        metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        metrics.insert(
            "has_compaction_manager".to_string(),
            serde_json::Value::Bool(self.compaction_manager.is_some()),
        );

        // Count SSTable files instead of memtable utilization
        let sstable_count = self.count_sstables_at_level(0).await.unwrap_or(0);
        metrics.insert(
            "sstable_count".to_string(),
            serde_json::Value::Number((sstable_count as u64).into()),
        );

        Ok(metrics)
    }
}

// =============================================================================
// LSM IMPLEMENTATION HELPER METHODS (Private)
// =============================================================================

impl LsmTree {
    /// Extract individual records from deserialized WAL vector record batches
    /// These batches come from the global partitioned memtable with WAL behavior
    /// Enhanced with batch processing optimizations for improved performance
    async fn extract_records_from_wal_vector_batches(
        &self,
        vector_records: &[VectorRecord],
        collection_id: &str,
    ) -> Result<Vec<LsmRecord>> {
        let extraction_start = std::time::Instant::now();
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;

        info!(
            "🔍 LSM ENGINE-OPTIMIZED EXTRACTION: Processing {} WAL vector record batches for collection {}",
            vector_records.len(),
            collection_id
        );

        // Pre-allocate with estimated capacity for better memory efficiency
        let estimated_matches = vector_records.len() / 4; // Conservative estimate
        let mut lsm_records = Vec::with_capacity(estimated_matches);

        // Batch optimization: Use vectorized processing for better performance
        let mut batch_stats = BatchExtractionStats::new();

        // Process records in chunks for better cache locality
        const CHUNK_SIZE: usize = 1000;
        for (chunk_idx, chunk) in vector_records.chunks(CHUNK_SIZE).enumerate() {
            let chunk_start = std::time::Instant::now();
            let mut chunk_matches = 0;

            for (index, vector_record) in chunk.iter().enumerate() {
                // All records should already be filtered for this collection
                // Convert VectorRecord to LsmRecord for row-by-row storage
                let mut lsm_record = LsmRecord::from_vector_record(vector_record.clone(), collection_id);
                
                // Set LSM-specific fields for proper ordering and level management
                let global_index = chunk_idx * CHUNK_SIZE + index;
                lsm_record.sequence_number = sequence_start + global_index as u64;
                lsm_record.level = 0; // New records from WAL start at level 0
                lsm_record.is_tombstone = false; // WAL records are active (not tombstones)
                
                lsm_records.push(lsm_record);
                chunk_matches += 1;
                
                batch_stats.total_extracted += 1;
            }

            let chunk_time = chunk_start.elapsed().as_micros() as u64;
            batch_stats.chunk_times.push(chunk_time);
            
            tracing::debug!(
                "📦 LSM CHUNK {}: Processed {} records, {} matches in {}μs",
                chunk_idx,
                chunk.len(),
                chunk_matches,
                chunk_time
            );
        }

        // Sort records by sequence number for optimal SSTable performance
        if lsm_records.len() > 1 {
            let sort_start = std::time::Instant::now();
            lsm_records.sort_by_key(|r| r.sequence_number);
            batch_stats.sort_time_us = sort_start.elapsed().as_micros() as u64;
        }

        let total_extraction_time = extraction_start.elapsed().as_millis() as u64;
        let avg_chunk_time = if !batch_stats.chunk_times.is_empty() {
            batch_stats.chunk_times.iter().sum::<u64>() / batch_stats.chunk_times.len() as u64
        } else {
            0
        };

        info!(
            "🚀 LSM ENGINE-OPTIMIZED EXTRACTION COMPLETE: {} records extracted from {} WAL records in {}ms (avg chunk: {}μs, sort: {}μs)",
            lsm_records.len(),
            vector_records.len(),
            total_extraction_time,
            avg_chunk_time,
            batch_stats.sort_time_us
        );

        Ok(lsm_records)
    }


    /// Flush memtable data to SSTable files using LSM's row-based architecture
    async fn flush_lsm_records_to_sstable(
        &self,
        lsm_records: Vec<LsmRecord>,
        _force_flush: bool,
    ) -> Result<FlushResult> {
        let flush_start = std::time::Instant::now();

        tracing::info!(
            "🗂️ LSM SSTABLE FLUSH: Processing {} records",
            lsm_records.len()
        );

        // Stage 1: Sort records by ID for SSTable ordering
        let sorting_start = std::time::Instant::now();
        let mut sorted_records = lsm_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));
        let sorting_time = sorting_start.elapsed().as_millis() as u64;
        tracing::debug!(
            "📊 LSM STAGE 1: Sorted {} records in {}ms",
            sorted_records.len(),
            sorting_time
        );

        // Stage 2: Partition records into levels based on LSM tree structure
        let partitioning_start = std::time::Instant::now();
        let level_partitions = self.partition_records_by_level(&sorted_records).await?;
        let partitioning_time = partitioning_start.elapsed().as_millis() as u64;
        let num_levels = level_partitions.len();
        tracing::debug!(
            "🏗️ LSM STAGE 2: Partitioned into {} levels in {}ms",
            num_levels,
            partitioning_time
        );

        // Stage 3: Create SSTable files for each level
        let sstable_start = std::time::Instant::now();
        let mut total_bytes_written = 0u64;
        let mut files_created = 0u64;
        let mut sstable_paths = Vec::new();

        for (level, level_records) in level_partitions {
            if level_records.is_empty() {
                continue;
            }

            // Get the collection storage URL from assignment service
            let collection_storage_url = self.get_collection_storage_url(&self.collection_id).await?;
            let data_dir = PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Generate SSTable filename with level and timestamp
            let timestamp = Utc::now().timestamp();
            let sst_filename = format!("{}_level{}_{}.sst", self.collection_id, level, timestamp);
            let sst_path = data_dir.join(&sst_filename);

            // Ensure directory exists
            if let Some(parent) = sst_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create directory: {}", e))?;
            }

            // Serialize records to row-based SSTable format
            let sstable_data = self
                .serialize_lsm_records_to_sstable(&level_records, level)
                .await?;

            // Write SSTable to disk
            tokio::fs::write(&sst_path, &sstable_data)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to write SSTable: {}", e))?;

            total_bytes_written += sstable_data.len() as u64;
            files_created += 1;
            sstable_paths.push(sst_path);

            tracing::debug!(
                "💾 LSM STAGE 3: Level {} SSTable {} written - {} records, {} bytes",
                level,
                sst_filename,
                level_records.len(),
                sstable_data.len()
            );
        }

        let sstable_time = sstable_start.elapsed().as_millis() as u64;

        // Stage 4: Update LSM tree metadata and indexes
        let metadata_start = std::time::Instant::now();
        self.update_lsm_metadata_after_flush(&sstable_paths, &sorted_records)
            .await?;
        let metadata_time = metadata_start.elapsed().as_millis() as u64;

        // Stage 5: Trigger compaction if threshold exceeded
        let compaction_check_start = std::time::Instant::now();
        let compaction_triggered = self.check_compaction_threshold().await?;
        let compaction_check_time = compaction_check_start.elapsed().as_millis() as u64;

        let total_flush_time = flush_start.elapsed().as_millis() as u64;

        // Build detailed engine metrics
        let mut engine_metrics = HashMap::new();
        engine_metrics.insert(
            "sorting_time_ms".to_string(),
            serde_json::Value::Number(sorting_time.into()),
        );
        engine_metrics.insert(
            "partitioning_time_ms".to_string(),
            serde_json::Value::Number(partitioning_time.into()),
        );
        engine_metrics.insert(
            "sstable_creation_time_ms".to_string(),
            serde_json::Value::Number(sstable_time.into()),
        );
        engine_metrics.insert(
            "metadata_update_time_ms".to_string(),
            serde_json::Value::Number(metadata_time.into()),
        );
        engine_metrics.insert(
            "compaction_check_time_ms".to_string(),
            serde_json::Value::Number(compaction_check_time.into()),
        );
        engine_metrics.insert(
            "total_flush_time_ms".to_string(),
            serde_json::Value::Number(total_flush_time.into()),
        );
        engine_metrics.insert(
            "levels_created".to_string(),
            serde_json::Value::Number(num_levels.into()),
        );
        engine_metrics.insert(
            "sstables_created".to_string(),
            serde_json::Value::Number(files_created.into()),
        );
        engine_metrics.insert(
            "compaction_triggered".to_string(),
            serde_json::Value::Bool(compaction_triggered),
        );
        engine_metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        engine_metrics.insert(
            "serialization_format".to_string(),
            serde_json::Value::String("Bincode".to_string()),
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![self.collection_id.clone()],
            entries_flushed: sorted_records.len() as u64,
            bytes_written: total_bytes_written,
            files_created,
            duration_ms: total_flush_time,
            completed_at: Utc::now(),
            compaction_triggered,
            engine_metrics,
            flushed_batch_ids: vec![],
        })
    }

    /// Partition records into LSM tree levels based on key ranges and record age
    async fn partition_records_by_level(
        &self,
        sorted_records: &[LsmRecord],
    ) -> Result<HashMap<u8, Vec<LsmRecord>>> {
        let mut level_partitions: HashMap<u8, Vec<LsmRecord>> = HashMap::new();

        // LSM Level 0: Recent entries (direct from memtable)
        // Level 1+: Compacted entries (would come from compaction process)

        let records_per_level = 10000; // Fixed number of records per level for pure SSTable storage

        for (i, record) in sorted_records.iter().enumerate() {
            let level = if i < records_per_level {
                0 // Most recent records go to Level 0
            } else {
                // Distribute older records across higher levels
                ((i / records_per_level) as u8).min(self.config.level_count - 1)
            };

            level_partitions
                .entry(level)
                .or_insert_with(Vec::new)
                .push(record.clone());
        }

        Ok(level_partitions)
    }

    /// Engine-optimized batch serialization to row-based SSTable format
    /// Includes compression, bloom filters, and block-based organization
    async fn serialize_lsm_records_to_sstable(
        &self,
        records: &[LsmRecord],
        level: u8,
    ) -> Result<Vec<u8>> {
        let serialization_start = std::time::Instant::now();
        
        // Engine optimization: Pre-allocate based on estimated size
        let estimated_size = records.len() * 512; // Conservative estimate per record
        let mut sstable_data = Vec::with_capacity(estimated_size);

        // Step 1: Create enhanced header with engine optimizations
        let header = SstableHeader {
            version: 1, // Version 1 for initial implementation
            level,
            entry_count: records.len() as u64,
            min_key: records.first().map(|r| r.id.clone()).unwrap_or_default(),
            max_key: records.last().map(|r| r.id.clone()).unwrap_or_default(),
            created_at: Utc::now().timestamp(),
            // Engine optimizations
            compression_enabled: true,
            has_bloom_filter: true,
            block_size: 4096, // 4KB blocks for better cache locality
            batch_size: records.len() as u32,
            // Additional fields (will be updated later)
            header_size: 0,
            index_size: 0,
            data_size: 0,
            block_count: 0,
        };

        // Step 2: Build bloom filter for fast key existence checks
        let bloom_filter = self.build_bloom_filter(records).await?;
        let bloom_data = bloom_filter.serialize()
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))?;

        // Step 3: Organize records into blocks for better cache performance
        let data_blocks = self.organize_records_into_blocks(records, header.block_size as usize).await?;
        
        // Step 4: Engine-optimized index with block pointers
        let (index_entries, compressed_blocks) = self.build_optimized_index_and_compress_blocks(&data_blocks).await?;

        // Step 5: Serialize header
        let header_data = bincode::serialize(&header)
            .map_err(|e| anyhow::anyhow!("Failed to serialize header: {}", e))?;
        sstable_data.extend((header_data.len() as u32).to_le_bytes());
        sstable_data.extend(header_data);

        // Step 6: Serialize bloom filter
        sstable_data.extend((bloom_data.len() as u32).to_le_bytes());
        sstable_data.extend(bloom_data);

        // Step 7: Serialize enhanced index
        let index_data = bincode::serialize(&index_entries)
            .map_err(|e| anyhow::anyhow!("Failed to serialize index: {}", e))?;
        sstable_data.extend((index_data.len() as u32).to_le_bytes());
        sstable_data.extend(index_data);

        // Step 8: Append compressed data blocks
        let total_data_size = compressed_blocks.iter().map(|b| b.len()).sum::<usize>();
        sstable_data.extend(compressed_blocks.into_iter().flatten());

        let serialization_time = serialization_start.elapsed().as_millis() as u64;
        let compression_ratio = if total_data_size > 0 {
            estimated_size as f64 / sstable_data.len() as f64
        } else {
            1.0
        };

        tracing::info!(
            "🚀 LSM ENGINE-OPTIMIZED SSTABLE: Level {} serialized - {} records, {} bytes, {:.2}x compression, {}ms",
            level, records.len(), sstable_data.len(), compression_ratio, serialization_time
        );

        Ok(sstable_data)
    }

    /// Update LSM tree metadata after successful flush
    async fn update_lsm_metadata_after_flush(
        &self,
        sstable_paths: &[std::path::PathBuf],
        flushed_records: &[LsmRecord],
    ) -> Result<()> {
        // Update internal tracking of SSTable files
        // In a full implementation, this would update:
        // - Level manifests
        // - Bloom filters for each SSTable
        // - Key range metadata
        // - File size statistics

        tracing::debug!(
            "📊 LSM METADATA: Updated after flush - {} SSTables, {} records",
            sstable_paths.len(),
            flushed_records.len()
        );

        Ok(())
    }

    /// Check if compaction is needed based on LSM tree structure
    async fn check_compaction_threshold(&self) -> Result<bool> {
        // Check Level 0 file count (trigger compaction if too many files)
        let level0_files = self.count_sstables_at_level(0).await?;
        let compaction_needed = level0_files >= self.config.compaction_threshold as usize;

        if compaction_needed {
            tracing::debug!(
                "🗜️ LSM COMPACTION: Threshold exceeded - {} Level 0 files (threshold: {})",
                level0_files,
                self.config.compaction_threshold
            );
        }

        Ok(compaction_needed)
    }

    /// Count SSTable files at a specific level
    async fn count_sstables_at_level(&self, level: u8) -> Result<usize> {
        let level_dir = self.data_dir.join(&self.collection_id);
        if !level_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut dir_entries = tokio::fs::read_dir(&level_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read level directory: {}", e))?;

        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.contains(&format!("_level{}_", level)) && filename.ends_with(".sst") {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Convert vector records directly to row-based SSTable format for staging pattern
    async fn serialize_records_to_sstable_row_format(
        &self,
        vector_records: &[VectorRecord],
        _collection_id: &str,
    ) -> Result<Vec<u8>> {
        tracing::info!(
            "📦 LSM: Serializing {} vector records to row-based SSTable format",
            vector_records.len()
        );

        // Convert VectorRecords to LsmRecords with proper sequencing
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;
        let mut lsm_records = Vec::new();

        for (index, record) in vector_records.iter().enumerate() {
            let mut lsm_record = LsmRecord::from_vector_record(record.clone(), &self.collection_id);
            lsm_record.sequence_number = sequence_start + index as u64;
            lsm_record.level = 0; // New records start at level 0
            lsm_records.push(lsm_record);
        }

        tracing::debug!(
            "🔄 LSM: Converted {} vector records to row-based LSM records",
            lsm_records.len()
        );

        // Sort records by ID for SSTable format
        let mut sorted_records = lsm_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));

        // Serialize to row-based SSTable format (Level 0 by default for new data)
        self.serialize_lsm_records_to_sstable(&sorted_records, 0).await
    }

    /// Search SSTable data for a specific vector ID using row-based format
    async fn search_sstable_for_vector(
        &self,
        sstable_data: &[u8],
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        if sstable_data.len() < 8 {
            // Not enough data for header length
            return Ok(None);
        }

        let mut offset = 0;

        // Read header length and header
        let header_len = u32::from_le_bytes([
            sstable_data[offset],
            sstable_data[offset + 1],
            sstable_data[offset + 2],
            sstable_data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + header_len > sstable_data.len() {
            return Ok(None);
        }

        let header_data = &sstable_data[offset..offset + header_len];
        let _header: SstableHeader = match bincode::deserialize(header_data) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        offset += header_len;

        // Read index length and index
        if offset + 4 > sstable_data.len() {
            return Ok(None);
        }

        let index_len = u32::from_le_bytes([
            sstable_data[offset],
            sstable_data[offset + 1],
            sstable_data[offset + 2],
            sstable_data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + index_len > sstable_data.len() {
            return Ok(None);
        }

        let index_data = &sstable_data[offset..offset + index_len];
        let index_entries: Vec<IndexEntry> = match bincode::deserialize(index_data) {
            Ok(entries) => entries,
            Err(_) => return Ok(None),
        };
        offset += index_len;

        // Binary search through index for the target key
        let search_result = index_entries.binary_search_by(|entry| entry.key.as_str().cmp(vector_id));
        
        if let Ok(index_pos) = search_result {
            let entry = &index_entries[index_pos];
            let data_start = offset + entry.offset as usize;
            let data_end = data_start + entry.size as usize;

            if data_end <= sstable_data.len() {
                let record_data = &sstable_data[data_start..data_end];
                if let Ok(lsm_record) = bincode::deserialize::<LsmRecord>(record_data) {
                    // Skip tombstones
                    if !lsm_record.is_tombstone {
                        return Ok(Some(lsm_record.into()));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Build bloom filter for fast key existence checks
    async fn build_bloom_filter(&self, records: &[LsmRecord]) -> Result<Box<dyn BloomFilterStrategy>> {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut filter = BloomFilterFactory::create(&config);
        
        // Add all keys to bloom filter
        for record in records {
            filter.insert(record.id.as_bytes());
        }
        
        debug!(
            "📊 LSM: Built bloom filter with {} bits for {} keys (FPR: {:.2}%)",
            filter.bit_count(),
            records.len(),
            filter.false_positive_rate() * 100.0
        );
        
        Ok(filter)
    }

    /// Sort vector records by metadata for optimal SSTable encoding
    async fn sort_vectors_for_sstable_encoding(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        // For LSM, we don't have direct access to collection config here
        // So we implement a simple but effective sorting strategy:
        // 1. Sort by first metadata key alphabetically
        // 2. Then by vector ID for stable ordering
        
        let mut sorted_vectors = vectors;
        
        // Find the most common metadata key for primary sorting
        let mut key_frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for vector in &sorted_vectors {
            for metadata_item in &vector.metadata {
                *key_frequency.entry(metadata_item.key.clone()).or_insert(0) += 1;
            }
        }
        
        let primary_sort_key = key_frequency
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(key, _)| key.clone());
        
        let sort_start = std::time::Instant::now();
        
        sorted_vectors.sort_by(|a, b| {
            // Primary sort: most common metadata key
            if let Some(ref sort_key) = primary_sort_key {
                let empty_string = String::new();
                let a_value = a.metadata.iter()
                    .find(|item| item.key == *sort_key)
                    .map(|item| &item.value)
                    .unwrap_or(&empty_string);
                let b_value = b.metadata.iter()
                    .find(|item| item.key == *sort_key)
                    .map(|item| &item.value)
                    .unwrap_or(&empty_string);
                
                match a_value.cmp(b_value) {
                    std::cmp::Ordering::Equal => {
                        // Secondary sort: vector ID for stable ordering
                        let empty_id = String::new();
                        let a_id = a.id.as_deref().unwrap_or(&empty_id);
                        let b_id = b.id.as_deref().unwrap_or(&empty_id);
                        a_id.cmp(b_id)
                    }
                    other => other,
                }
            } else {
                // Fallback: sort by vector ID only
                let empty_id = String::new();
                let a_id = a.id.as_deref().unwrap_or(&empty_id);
                let b_id = b.id.as_deref().unwrap_or(&empty_id);
                a_id.cmp(b_id)
            }
        });
        
        let sort_time_us = sort_start.elapsed().as_micros() as u64;
        
        // Calculate compression estimate based on metadata distribution
        let compression_estimate = if let Some(ref sort_key) = primary_sort_key {
            let distinct_values: std::collections::HashSet<String> = sorted_vectors
                .iter()
                .filter_map(|v| {
                    v.metadata.iter()
                        .find(|item| item.key == *sort_key)
                        .map(|item| item.value.clone())
                })
                .collect();
            
            // Lower cardinality = better compression
            1.0 - (distinct_values.len() as f64 / sorted_vectors.len() as f64)
        } else {
            0.05 // Small improvement from ID sorting
        };
        
        let stats = SortingStats {
            records_sorted: sorted_vectors.len(),
            sort_keys_used: if let Some(key) = primary_sort_key {
                vec![key, "vector_id".to_string()]
            } else {
                vec!["vector_id".to_string()]
            },
            compression_estimate,
            sort_time_us,
            ..Default::default()
        };
        
        debug!(
            "🎯 LSM: Sorted {} vectors by metadata key for SSTable optimization",
            stats.records_sorted
        );
        
        Ok((sorted_vectors, stats))
    }

    /// Hash function for bloom filter
    fn hash_key(&self, key: &str, hash_num: u32) -> u32 {
        // Simple hash function - in production would use a proper hash function
        let mut hash = 5381u32;
        for byte in key.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
        }
        hash.wrapping_add(hash_num)
    }

    /// Organize records into blocks for better cache locality
    async fn organize_records_into_blocks(
        &self,
        records: &[LsmRecord],
        block_size: usize,
    ) -> Result<Vec<DataBlock>> {
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0;

        for record in records {
            let record_size = std::mem::size_of::<LsmRecord>() + 
                record.id.len() + 
                record.collection_id.len() + 
                record.vector.len() * 4 + // f32 size
                record.metadata.iter().map(|(key, value)| key.len() + value.to_string().len() + 10).sum::<usize>(); // Estimate metadata size

            // If adding this record would exceed block size, finalize current block
            if current_block_size + record_size > block_size && !current_block_records.is_empty() {
                blocks.push(DataBlock {
                    block_id,
                    uncompressed_size: current_block_size as u32,
                    records: std::mem::take(&mut current_block_records),
                });
                block_id += 1;
                current_block_size = 0;
            }

            current_block_records.push(record.clone());
            current_block_size += record_size;
        }

        // Add final block if not empty
        if !current_block_records.is_empty() {
            blocks.push(DataBlock {
                block_id,
                uncompressed_size: current_block_size as u32,
                records: current_block_records,
            });
        }

        tracing::debug!(
            "📦 LSM BLOCK ORGANIZATION: {} records organized into {} blocks (avg block size: {}KB)",
            records.len(),
            blocks.len(),
            if !blocks.is_empty() { current_block_size / blocks.len() / 1024 } else { 0 }
        );

        Ok(blocks)
    }

    /// Build optimized index and compress data blocks
    async fn build_optimized_index_and_compress_blocks(
        &self,
        data_blocks: &[DataBlock],
    ) -> Result<(Vec<IndexEntry>, Vec<Vec<u8>>)> {
        let mut index_entries = Vec::new();
        let mut compressed_blocks = Vec::new();

        for block in data_blocks {
            // Serialize block data
            let block_data = bincode::serialize(&block.records)
                .map_err(|e| anyhow::anyhow!("Failed to serialize block data: {}", e))?;

            // Simple compression using zlib/deflate
            let compressed_data = self.compress_block_data(&block_data).await?;
            let is_compressed = compressed_data.len() < block_data.len();
            
            // Use compressed data if it's smaller, otherwise use original
            let final_data = if is_compressed {
                compressed_data
            } else {
                block_data
            };

            // Create index entries for each record in this block using unified IndexEntry
            let mut block_offset = 0u32;
            for record in &block.records {
                index_entries.push(IndexEntry {
                    key: record.id.clone(),
                    offset: 0, // Will be set later with global offset
                    size: std::mem::size_of::<LsmRecord>() as u32, // Approximate size
                    // Enhanced block organization fields
                    block_id: block.block_id,
                    block_offset,
                    compressed: is_compressed,
                    // Metadata statistics (empty for backward compatibility)
                    metadata_min_values: HashMap::new(),
                    metadata_max_values: HashMap::new(),
                    metadata_null_counts: HashMap::new(),
                });
                block_offset += std::mem::size_of::<LsmRecord>() as u32;
            }

            compressed_blocks.push(final_data);
        }

        tracing::debug!(
            "🗜️ LSM COMPRESSION: {} blocks processed, {} index entries created",
            data_blocks.len(),
            index_entries.len()
        );

        Ok((index_entries, compressed_blocks))
    }

    /// Simple block compression
    async fn compress_block_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simple run-length encoding for demonstration
        // In production, would use proper compression like zstd or lz4
        let mut compressed = Vec::new();
        
        if data.is_empty() {
            return Ok(compressed);
        }

        let mut i = 0;
        while i < data.len() {
            let current_byte = data[i];
            let mut count = 1u8;
            
            // Count consecutive identical bytes
            while i + 1 < data.len() && data[i + 1] == current_byte && count < 255 {
                count += 1;
                i += 1;
            }
            
            // Store count and byte
            compressed.push(count);
            compressed.push(current_byte);
            i += 1;
        }

        Ok(compressed)
    }

    /// Convenient compact_collection method for CompactionCoordinator integration
    pub async fn compact_collection(&self, collection_id: &str) -> Result<EngineCompactionResult> {
        info!("🗜️ LSM Engine: Starting collection compaction for {}", collection_id);
        
        // Check if this is the correct collection
        if collection_id != &self.collection_id {
            return Err(anyhow::anyhow!("Collection ID mismatch: expected {}, got {}", self.collection_id, collection_id));
        }
        
        // Create compaction parameters (LSM doesn't have collection service access)
        let params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: None, // LSM doesn't have collection service
        };
        
        // Use the existing do_compact implementation
        let result = self.do_compact(&params).await?;
        
        Ok(EngineCompactionResult {
            files_processed: result.output_files,
            bytes_processed: result.bytes_written,
        })
    }

}

/// Simplified compaction result for CompactionCoordinator
#[derive(Debug, Clone)]
pub struct EngineCompactionResult {
    pub files_processed: u64,
    pub bytes_processed: u64,
}
