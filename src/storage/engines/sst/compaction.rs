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

//! SST File Compaction for LSM Tree
//!
//! Implements level-based compaction strategy to prevent unbounded growth
//! of SST files. Uses background workers to merge files when thresholds are exceeded.

use super::{SstRecord, SstableWriter};
use crate::core::{String, SstConfig, VectorId, VectorRecord};
use crate::storage::optimization::{MetadataSorter, SortingStats};
use crate::storage::Result;
use crate::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};
use chrono::Utc;
use rand::Rng;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Compaction task to be processed by background workers
#[derive(Debug, Clone)]
pub struct CompactionTask {
    pub collection_id: String,
    pub level: u8,
    pub input_files: Vec<PathBuf>,
    pub output_file: PathBuf,
    pub priority: CompactionPriority,
}

/// Priority levels for compaction tasks
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionPriority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3, // When storage is nearly full
}

/// Statistics for compaction operations
#[derive(Debug, Clone, Default)]
pub struct CompactionStats {
    pub total_compactions: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub files_merged: u64,
    pub avg_compaction_time_ms: u64,
    pub last_compaction_time: Option<chrono::DateTime<chrono::Utc>>,
    pub expired_records_deleted: u64,
    pub tombstones_removed: u64,
}

/// Enhanced compaction statistics with vector tracking for AXIS integration
#[derive(Debug, Clone, Default)]
pub struct EnhancedCompactionStats {
    /// Basic compaction statistics
    pub base_stats: CompactionStats,
    
    /// Vector IDs that were deleted (expired or tombstoned)
    pub deleted_vector_ids: Vec<String>,
    
    /// Vectors that were merged/updated
    pub merged_vectors: Vec<VectorRecord>,
    
    /// Whether a full index rebuild is recommended
    pub recommend_full_rebuild: bool,
}

/// Manages background compaction of SST files
#[derive(Debug)]
pub struct CompactionManager {
    config: SstConfig,
    task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
    worker_handles: Vec<JoinHandle<()>>,
    shutdown_signal: Arc<AtomicBool>,
    stats: Arc<RwLock<CompactionStats>>,
    active_compactions: Arc<RwLock<HashMap<String, CompactionTask>>>,
    atomic_coordinator: Option<Arc<UnifiedAtomicCoordinator>>,
    // manifest: Option<Arc<super::SstManifest>>, // Removed - using directory discovery
}

impl CompactionManager {
    /// Create a new compaction manager
    pub fn new(config: SstConfig) -> Self {
        Self::with_atomic_coordinator(config, None)
    }
    
    /// Create a new compaction manager with atomic coordinator
    pub fn with_atomic_coordinator(
        config: SstConfig,
        atomic_coordinator: Option<Arc<UnifiedAtomicCoordinator>>,
    ) -> Self {
        Self {
            config,
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            worker_handles: Vec::new(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(CompactionStats::default())),
            active_compactions: Arc::new(RwLock::new(HashMap::new())),
            atomic_coordinator,
        }
    }

    /// Start background compaction workers
    pub async fn start_workers(&mut self, worker_count: usize) -> Result<()> {
        info!("Starting {} compaction workers", worker_count);

        for worker_id in 0..worker_count {
            let task_queue = Arc::clone(&self.task_queue);
            let shutdown_signal = Arc::clone(&self.shutdown_signal);
            let stats = Arc::clone(&self.stats);
            let active_compactions = Arc::clone(&self.active_compactions);
            let atomic_coordinator = self.atomic_coordinator.clone();
            let config = SstConfig {
                memtable_size_mb: self.config.memtable_size_mb,
                level_count: self.config.level_count,
                compaction_threshold: self.config.compaction_threshold,
                block_size_kb: self.config.block_size_kb,
                memory_flush_size_bytes: self.config.memory_flush_size_bytes,
                memtable_type: self.config.memtable_type.clone(),
                compaction_strategy: self.config.compaction_strategy.clone(),
                compression: self.config.compression.clone(),
                bloom_filter_config: self.config.bloom_filter_config.clone(),
                cache_size_mb: self.config.cache_size_mb,
                write_buffer_size_mb: self.config.write_buffer_size_mb,
                max_files_per_level: self.config.max_files_per_level,
                level_size_multiplier: self.config.level_size_multiplier,
                max_levels: self.config.max_levels,
                background_thread_count: self.config.background_thread_count,
                sync_mode: self.config.sync_mode.clone(),
                enable_write_buffer: self.config.enable_write_buffer,
                write_buffer_directory: self.config.write_buffer_directory.clone(),
                data_directory: self.config.data_directory.clone(),
                mmap_enabled: self.config.mmap_enabled,
                prefetch_enabled: self.config.prefetch_enabled,
                prefetch_size_kb: self.config.prefetch_size_kb,
            };

            let handle = tokio::spawn(async move {
                Self::worker_loop(
                    worker_id,
                    task_queue,
                    shutdown_signal,
                    stats,
                    active_compactions,
                    config,
                    atomic_coordinator,
                )
                .await;
            });

            self.worker_handles.push(handle);
        }

        Ok(())
    }

    /// Stop all compaction workers gracefully
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping compaction manager");

        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Wait for all workers to finish
        for handle in self.worker_handles.drain(..) {
            if let Err(e) = handle.await {
                warn!("Compaction worker failed to shutdown cleanly: {}", e);
            }
        }

        // Complete any remaining compactions
        let remaining_tasks = {
            let queue = self.task_queue.lock().await;
            queue.len()
        };

        if remaining_tasks > 0 {
            warn!(
                "Compaction manager stopped with {} pending tasks",
                remaining_tasks
            );
        }

        info!("Compaction manager stopped successfully");
        Ok(())
    }

    /// Schedule a compaction task
    pub async fn schedule_compaction(&self, task: CompactionTask) -> Result<()> {
        debug!(
            "Scheduling compaction for collection {} level {}",
            task.collection_id, task.level
        );

        // Check if there's already an active compaction for this collection
        {
            let active = self.active_compactions.read().await;
            if active.contains_key(&task.collection_id) {
                debug!(
                    "Skipping compaction - already active for collection {}",
                    task.collection_id
                );
                return Ok(());
            }
        }

        let mut queue = self.task_queue.lock().await;

        // Insert task in priority order
        let insert_pos = queue
            .iter()
            .position(|existing_task| existing_task.priority < task.priority)
            .unwrap_or(queue.len());

        queue.insert(insert_pos, task);

        debug!(
            "Compaction task queued (position: {}, queue size: {})",
            insert_pos,
            queue.len()
        );

        Ok(())
    }

    /// Check if compaction is needed for the given collection and level
    pub async fn check_compaction_needed(
        &self,
        collection_dir: &Path,
        collection_id: &str,
    ) -> Result<Option<CompactionTask>> {
        let sst_files = self.get_sst_files_by_level(collection_dir).await?;

        for level in 0..self.config.level_count {
            let files_at_level = sst_files.get(&level).map(|v| v.len()).unwrap_or(0);

            if files_at_level >= self.config.compaction_threshold as usize {
                info!(
                    "Compaction needed for collection {} level {} ({} files >= {})",
                    collection_id, level, files_at_level, self.config.compaction_threshold
                );

                let input_files = sst_files.get(&level).cloned().unwrap_or_default();
                let output_file = self.generate_output_file_path(collection_id, collection_dir, level + 1);

                let priority = if files_at_level >= (self.config.compaction_threshold * 2) as usize
                {
                    CompactionPriority::High
                } else {
                    CompactionPriority::Medium
                };

                return Ok(Some(CompactionTask {
                    collection_id: collection_id.to_string(),
                    level,
                    input_files,
                    output_file,
                    priority,
                }));
            }
        }

        Ok(None)
    }

    /// Get compaction statistics
    pub async fn get_stats(&self) -> CompactionStats {
        self.stats.read().await.clone()
    }

    /// Worker loop for processing compaction tasks
    async fn worker_loop(
        worker_id: usize,
        task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
        shutdown_signal: Arc<AtomicBool>,
        stats: Arc<RwLock<CompactionStats>>,
        active_compactions: Arc<RwLock<HashMap<String, CompactionTask>>>,
        config: SstConfig,
        atomic_coordinator: Option<Arc<UnifiedAtomicCoordinator>>,
    ) {
        debug!("Compaction worker {} started", worker_id);

        loop {
            if shutdown_signal.load(Ordering::SeqCst) {
                break;
            }

            // Get next task from queue
            let task = {
                let mut queue = task_queue.lock().await;
                queue.pop_front()
            };

            if let Some(task) = task {
                debug!(
                    "Worker {} processing compaction for collection {} level {}",
                    worker_id, task.collection_id, task.level
                );

                // Mark as active
                {
                    let mut active = active_compactions.write().await;
                    active.insert(task.collection_id.clone(), task.clone());
                }

                let start_time = std::time::Instant::now();

                // Perform compaction - create a temporary manager for SSTable parsing
                let temp_manager = CompactionManager::with_atomic_coordinator(config.clone(), atomic_coordinator.clone());
                match temp_manager.perform_compaction(&task, &config, atomic_coordinator.clone()).await {
                    Ok(compaction_stats) => {
                        info!(
                            "Compaction completed for collection {} level {} in {}ms",
                            task.collection_id,
                            task.level,
                            start_time.elapsed().as_millis()
                        );

                        // Update statistics
                        {
                            let mut stats_guard = stats.write().await;
                            stats_guard.total_compactions += 1;
                            stats_guard.bytes_written += compaction_stats.bytes_written;
                            stats_guard.bytes_read += compaction_stats.bytes_read;
                            stats_guard.files_merged += compaction_stats.files_merged;
                            stats_guard.last_compaction_time = Some(Utc::now());

                            // Update average compaction time
                            let elapsed_ms = start_time.elapsed().as_millis() as u64;
                            if stats_guard.total_compactions == 1 {
                                stats_guard.avg_compaction_time_ms = elapsed_ms;
                            } else {
                                stats_guard.avg_compaction_time_ms =
                                    (stats_guard.avg_compaction_time_ms + elapsed_ms) / 2;
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Compaction failed for collection {} level {}: {}",
                            task.collection_id, task.level, e
                        );
                    }
                }

                // Remove from active compactions
                {
                    let mut active = active_compactions.write().await;
                    active.remove(&task.collection_id);
                }
            } else {
                // No tasks available, wait a bit
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }

        debug!("Compaction worker {} stopped", worker_id);
    }

    /// Perform the actual compaction operation
    async fn perform_compaction(
        &self,
        task: &CompactionTask,
        _config: &SstConfig,
        atomic_coordinator: Option<Arc<UnifiedAtomicCoordinator>>,
    ) -> Result<CompactionStats> {
        let enhanced_stats = self.perform_compaction_enhanced(task, _config, atomic_coordinator).await?;
        Ok(enhanced_stats.base_stats)
    }
    
    /// Enhanced compaction that tracks vector changes for AXIS integration
    pub async fn perform_compaction_enhanced(
        &self,
        task: &CompactionTask,
        _config: &SstConfig,
        atomic_coordinator: Option<Arc<UnifiedAtomicCoordinator>>,
    ) -> Result<EnhancedCompactionStats> {
        let start_time = std::time::Instant::now();
        let mut merged_data = BTreeMap::<VectorId, SstRecord>::new();
        let mut bytes_read = 0u64;

        debug!(
            "Merging {} input files for level {}",
            task.input_files.len(),
            task.level
        );

        // Read and merge all input files using plugin filesystem
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            ).await.map_err(|e| crate::core::StorageError::SstStorage(e.to_string()))?
        );
        let fs = filesystem_factory.get_filesystem("file:///")
            .map_err(|e| crate::core::StorageError::SstStorage(e.to_string()))?;
        
        for input_file in &task.input_files {
            let input_path = input_file.to_string_lossy();
            info!("🔍 COMPACTION DEBUG: Reading file: {}", input_path);
            
            let file_data = fs.read(&input_path)
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

            info!("🔍 COMPACTION DEBUG: File size: {} bytes", file_data.len());
            bytes_read += file_data.len() as u64;

            if file_data.len() < 16 {
                warn!("🔍 COMPACTION DEBUG: File too small ({} bytes), skipping", file_data.len());
                continue;
            }

            // Smart SSTable parsing: Skip header/bloom/index, read data blocks directly
            // This is optimal for compaction since we need ALL records anyway
            match self.read_data_blocks_from_sstable(&file_data, &input_path).await {
                Ok(records) => {
                    info!("🔍 COMPACTION: Extracted {} records from {}", records.len(), input_path);
                    for (id, record) in records {
                        let vector_id = VectorId::from(id);
                        // Handle merge logic for LSM records
                        match merged_data.get(&vector_id) {
                            Some(existing_record) => {
                                if should_replace_record(existing_record, &record) {
                                    merged_data.insert(vector_id, record);
                                }
                            }
                            None => {
                                merged_data.insert(vector_id, record);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("🔍 COMPACTION: Failed to read data blocks from {}: {}", input_path, e);
                    continue;
                }
            }
        }

        debug!("Merged {} unique records", merged_data.len());

        // Convert merged data to vectors for sorting
        let mut vector_records = Vec::new();
        let current_time = chrono::Utc::now().timestamp_millis();
        let mut expired_records_count = 0;
        let mut tombstones_removed_count = 0;
        
        // Track deleted vectors for AXIS
        let mut deleted_vector_ids = Vec::new();
        let mut merged_vectors = Vec::new();
        
        for (id, lsm_record) in merged_data.iter() {
            // Check if record is expired (TTL-based expiry)
            let is_expired = if let Some(expires_at) = lsm_record.expires_at {
                expires_at < current_time
            } else {
                false
            };
            
            // Skip expired records completely - they are physically deleted
            if is_expired {
                expired_records_count += 1;
                debug!("⏰ LSM COMPACTION: Physically deleting expired record {} (expired at {})", 
                      id, lsm_record.expires_at.unwrap());
                // Track deleted vector for AXIS
                deleted_vector_ids.push(id.to_string());
                continue;
            }
            
            // Handle tombstone cleanup
            let should_keep = if lsm_record.is_tombstone {
                // Keep tombstones that are less than 1 hour old
                let age = current_time - lsm_record.timestamp;
                let keep_tombstone = age < (60 * 60 * 1000); // 1 hour in milliseconds
                
                if !keep_tombstone {
                    tombstones_removed_count += 1;
                    debug!("🗑️ LSM COMPACTION: Removing old tombstone {} (age: {}ms)", 
                          id, age);
                    // Track deleted vector for AXIS
                    deleted_vector_ids.push(id.to_string());
                }
                
                keep_tombstone
            } else {
                true // Keep all active, non-expired records
            };

            if should_keep {
                // Convert SstRecord to VectorRecord for sorting
                let vector_record: VectorRecord = lsm_record.clone().into();
                
                // Track merged vectors for AXIS (non-tombstone records)
                if !lsm_record.is_tombstone {
                    merged_vectors.push(vector_record.clone());
                }
                
                vector_records.push(vector_record);
            }
        }
        
        // Log cleanup statistics
        if expired_records_count > 0 || tombstones_removed_count > 0 {
            info!("🧹 LSM COMPACTION CLEANUP: {} expired records deleted, {} old tombstones removed", 
                  expired_records_count, tombstones_removed_count);
        }

        // Sort records by metadata for optimal encoding
        info!("🔄 LSM COMPACTION: Sorting {} records by metadata for optimal encoding", vector_records.len());
        let (sorted_vectors, sort_stats) = Self::sort_vectors_for_compaction(vector_records).await?;
        info!("✅ LSM COMPACTION: Sorted records (estimated compression improvement: {:.1}%)", 
              sort_stats.compression_estimate * 100.0);

        // Convert back to SstRecord format with preserved metadata sorting
        let mut sorted_lsm_records: BTreeMap<String, SstRecord> = BTreeMap::new();
        for (seq, vector) in sorted_vectors.into_iter().enumerate() {
            let vector_id = vector.id.as_deref().unwrap_or("").to_string();
            let mut lsm_record = SstRecord::from_vector_record(vector, &task.collection_id);
            lsm_record.sequence_number = seq as u64; // Update sequence for compacted order
            lsm_record.level = task.level + 1; // Increment level after compaction
            sorted_lsm_records.insert(vector_id, lsm_record);
        }

        // Use optimized SSTable writer for compacted output with atomic writes
        let block_size = (_config.block_size_kb * 1024) as usize;
        
        // TODO: Pass filesystem from compaction manager - for now create a new factory
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            ).await.map_err(|e| crate::core::StorageError::SstStorage(e.to_string()))?
        );
        
        let bytes_written = if let Some(ref coordinator) = atomic_coordinator {
            // Use atomic operations for compaction
            info!("🔒 LSM COMPACTION: Using atomic operations for compaction");
            
            // Create staging configuration
            // Don't include collection_id in StagingConfig - the base_url already points to the final location
            let staging_config = StagingConfig {
                base_url: task.output_file.parent()
                    .ok_or_else(|| crate::core::StorageError::SstStorage("Invalid output file path".to_string()))?
                    .to_string_lossy()
                    .to_string(),
                collection_id: None,  // Don't add /collections/{id} structure
                operation_type: StagingOperationType::Compaction,
                ..Default::default()
            };
            
            // Begin atomic operation
            let atomic_op = coordinator.begin_atomic_operation(&staging_config).await
                .map_err(|e| crate::core::StorageError::SstStorage(format!("Failed to begin atomic operation: {}", e)))?;
            
            debug!("Started atomic operation {} for compaction", atomic_op.operation_id);
            
            // Get the staging filename
            let staging_filename = task.output_file.file_name()
                .ok_or_else(|| crate::core::StorageError::SstStorage("Invalid output filename".to_string()))?
                .to_string_lossy()
                .to_string();
            
            // Write SSTable directly to staging path
            // Strip file:// prefix if present for local filesystem operations
            let staging_path = if atomic_op.staging_url.starts_with("file://") {
                atomic_op.staging_url.strip_prefix("file://").unwrap()
            } else {
                &atomic_op.staging_url
            };
            let staging_file_path = PathBuf::from(format!("{}/{}", staging_path, staging_filename));
            debug!("Writing SSTable to staging path: {}", staging_file_path.display());
            let writer = SstableWriter::new(&staging_file_path, block_size, filesystem_factory.clone());
            writer.write_records(sorted_lsm_records).await
                .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;
            
            // Get file size for stats
            let metadata = fs.metadata(&staging_file_path.to_string_lossy())
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
            let written_bytes = metadata.size;
            
            // Finalize atomic operation - this moves the file from staging to final location
            coordinator.finalize_atomic_operation(&atomic_op.operation_id).await
                .map_err(|e| crate::core::StorageError::SstStorage(format!("Failed to finalize atomic operation: {}", e)))?;
            
            info!("✅ LSM COMPACTION: Atomic operation {} completed successfully", atomic_op.operation_id);
            debug!("File should be at final location: {}", atomic_op.final_url);
            
            written_bytes
        } else {
            // Fallback to direct write (non-atomic)
            debug!("Writing SSTable directly to: {}", task.output_file.display());
            let writer = SstableWriter::new(&task.output_file, block_size, filesystem_factory);
            writer.write_records(sorted_lsm_records).await
                .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;

            let output_path = task.output_file.to_string_lossy();
            let metadata = fs.metadata(&output_path)
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
            metadata.size
        };

        debug!(
            "Wrote {} bytes to output file {}",
            bytes_written,
            task.output_file.display()
        );
        
        // NOTE: Manifest removed - using directory-based discovery instead
        // Files are automatically discovered by scanning collection directories
        debug!("Compaction completed - files will be discovered automatically by directory scanning");

        // Remove input files after successful compaction using plugin filesystem
        for input_file in &task.input_files {
            let input_path = input_file.to_string_lossy();
            if let Err(e) = fs.delete(&input_path).await {
                warn!(
                    "Failed to remove input file {}: {}",
                    input_file.display(),
                    e
                );
            }
        }

        // DETAILED COMPACTION PERFORMANCE ANALYSIS
        let total_time = start_time.elapsed();
        let input_files_count = task.input_files.len();
        let compression_ratio = if bytes_read > 0 {
            bytes_written as f64 / bytes_read as f64
        } else {
            1.0
        };
        let read_throughput_mb_sec =
            (bytes_read as f64 / 1024.0 / 1024.0) / total_time.as_secs_f64();
        let write_throughput_mb_sec =
            (bytes_written as f64 / 1024.0 / 1024.0) / total_time.as_secs_f64();

        tracing::info!(
            "🗜️ [LSM COMPACTION] Level {} complete: {} files → 1 file in {:?}",
            task.level,
            input_files_count,
            total_time
        );

        tracing::info!(
            "⚡ [LSM COMPACTION PERFORMANCE] Read: {:.1}MB/s, Write: {:.1}MB/s, Compression: {:.1}x",
            read_throughput_mb_sec, write_throughput_mb_sec, compression_ratio
        );

        // COMPACTION PERFORMANCE WARNINGS (compaction can be slower than flush)
        if total_time.as_millis() > 5000 {
            // >5s is very slow for compaction
            tracing::warn!(
                "⚠️ SLOW LSM COMPACTION: {}ms for {} files. Consider:",
                total_time.as_millis(),
                input_files_count
            );
            tracing::warn!("   • Moving compaction to dedicated background process");
            tracing::warn!("   • Using faster storage for compaction temp files");
            tracing::warn!("   • Reducing compaction scope/frequency");
        }

        if read_throughput_mb_sec < 50.0 {
            // <50MB/s read is slow
            tracing::warn!(
                "⚠️ LSM COMPACTION READ WARNING: {:.1}MB/s below target 50MB/s",
                read_throughput_mb_sec
            );
        }

        if write_throughput_mb_sec < 30.0 {
            // <30MB/s write is slow
            tracing::warn!(
                "⚠️ LSM COMPACTION WRITE WARNING: {:.1}MB/s below target 30MB/s",
                write_throughput_mb_sec
            );
        }

        // DESIGN INSIGHT: Comparison with flush performance
        if total_time.as_millis() > 1000 {
            tracing::info!("💡 DESIGN INSIGHT: LSM compaction ({}ms) much slower than VIPER flush target (<200ms) - async compaction recommended", 
                          total_time.as_millis());
        }

        debug!(
            "🗜️ LSM compaction stats: {}MB read, {}MB written, {:.1}x compression, {} records merged, {} expired deleted, {} tombstones removed",
            bytes_read / 1024 / 1024, bytes_written / 1024 / 1024, compression_ratio, merged_data.len(), expired_records_count, tombstones_removed_count
        );

        Ok(EnhancedCompactionStats {
            base_stats: CompactionStats {
                total_compactions: 1,
                bytes_written,
                bytes_read,
                files_merged: task.input_files.len() as u64,
                avg_compaction_time_ms: start_time.elapsed().as_millis() as u64,
                last_compaction_time: Some(Utc::now()),
                expired_records_deleted: expired_records_count,
                tombstones_removed: tombstones_removed_count,
            },
            deleted_vector_ids,
            merged_vectors,
            recommend_full_rebuild: false,
        })
    }

    /// Get SST files organized by level
    async fn get_sst_files_by_level(
        &self,
        collection_dir: &Path,
    ) -> Result<HashMap<u8, Vec<PathBuf>>> {
        let mut files_by_level = HashMap::new();

        if !collection_dir.exists() {
            return Ok(files_by_level);
        }

        // Use plugin filesystem for directory listing
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            ).await.map_err(|e| crate::core::StorageError::SstStorage(e.to_string()))?
        );
        let fs = filesystem_factory.get_filesystem("file:///")
            .map_err(|e| crate::core::StorageError::SstStorage(e.to_string()))?;
        
        let collection_path = collection_dir.to_string_lossy();
        let entries = fs.list(&collection_path)
            .await
            .map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        for entry in entries {
            if !entry.metadata.is_directory {
                if let Some(filename) = std::path::Path::new(&entry.name).file_name().and_then(|f| f.to_str()) {
                    if filename.ends_with(".sst") && filename.contains("_level") {
                        // Parse level from filename format: {collection_id}_level{level}_{timestamp}_{random}.sst
                        let level = if let Some(level_start) = filename.find("_level") {
                            let level_str = &filename[level_start + 6..]; // Skip "_level"
                            if let Some(level_end) = level_str.find('_') {
                                level_str[..level_end].parse::<u8>().unwrap_or(0)
                            } else {
                                0
                            }
                        } else {
                            0
                        };
                        
                        
                        let path = PathBuf::from(&entry.url);
                        files_by_level.entry(level).or_insert_with(Vec::new).push(path);
                    }
                }
            }
        }

        Ok(files_by_level)
    }

    /// Read data blocks directly from SSTable, skipping header/bloom/index
    /// This is optimal for compaction since we need all records anyway
    async fn read_data_blocks_from_sstable(
        &self,
        file_data: &[u8],
        file_path: &str,
    ) -> std::result::Result<BTreeMap<String, super::SstRecord>, crate::core::StorageError> {
        info!("🔍 COMPACTION: Parsing SSTable format for {}", file_path);
        
        if file_data.len() < 16 {
            return Err(crate::core::StorageError::SstStorage(
                format!("SSTable file too small: {} bytes", file_data.len())
            ));
        }

        // SSTable format: [header_len:4][header][bloom_len:4][bloom][index_len:4][index][data_blocks...]
        // Skip header, bloom, and index to reach data blocks
        let mut offset = 0;
        
        // Skip header: [header_len:4][header_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstStorage("File too small for header length".into()));
        }
        let header_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + header_len;
        
        // Skip bloom filter: [bloom_len:4][bloom_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstStorage("File too small for bloom length".into()));
        }
        let bloom_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + bloom_len;
        
        // Skip index: [index_len:4][index_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstStorage("File too small for index length".into()));
        }
        let index_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + index_len;
        
        info!("🔍 COMPACTION: Skipped header ({} bytes), bloom ({} bytes), index ({} bytes) - data blocks start at offset {}", 
              header_len, bloom_len, index_len, offset);
        
        // Now we're at the data blocks section
        let data_blocks_bytes = &file_data[offset..];
        info!("🔍 COMPACTION: Reading {} bytes of data blocks", data_blocks_bytes.len());
        debug!("Reading {} bytes of data blocks from file {}", data_blocks_bytes.len(), file_path);
        
        // OPTIMIZED: Fast bulk record extraction for compaction
        // Since compaction needs ALL records, we use a streaming approach
        let mut all_records = BTreeMap::new();
        let mut blocks_processed = 0;
        let mut total_records = 0;
        
        let mut block_offset = 0;
        while block_offset < data_blocks_bytes.len() {
            // Read block size
            if block_offset + 4 > data_blocks_bytes.len() {
                break;
            }
            
            let block_size = u32::from_le_bytes([
                data_blocks_bytes[block_offset],
                data_blocks_bytes[block_offset + 1], 
                data_blocks_bytes[block_offset + 2],
                data_blocks_bytes[block_offset + 3],
            ]) as usize;
            
            debug!("Block {} - size {} bytes, offset {}", blocks_processed, block_size, block_offset);
            
            block_offset += 4;
            
            if block_offset + block_size > data_blocks_bytes.len() {
                warn!("🔍 COMPACTION: Block extends beyond data: {} + {} > {}", 
                      block_offset, block_size, data_blocks_bytes.len());
                break;
            }
            
            let block_data = &data_blocks_bytes[block_offset..block_offset + block_size];
            
            // FAST PATH: Parse data block with streaming deserialization
            debug!("About to parse block {} with {} bytes", blocks_processed, block_data.len());
            match self.fast_parse_data_block(block_data, blocks_processed) {
                Ok(block_records) => {
                    let record_count = block_records.len();
                    total_records += record_count;
                    
                    debug!("Block {} parsed successfully - {} records", blocks_processed, record_count);
                    
                    // Bulk insert with iterator efficiency
                    for (id, record) in block_records {
                        all_records.insert(id, record);
                    }
                    
                    if blocks_processed % 10 == 0 || record_count > 0 {
                        info!("🔍 COMPACTION: Block {} - {} records (total: {})", 
                              blocks_processed, record_count, total_records);
                    }
                }
                Err(e) => {
                    warn!("Failed to parse block {} at offset {}: {}", blocks_processed, block_offset, e);
                    warn!("🔍 COMPACTION: Failed to parse block {} at offset {}: {}", 
                          blocks_processed, block_offset, e);
                }
            }
            
            block_offset += block_size;
            blocks_processed += 1;
        }
        
        info!("🔍 COMPACTION: Extracted {} total records from {} blocks", all_records.len(), blocks_processed);
        Ok(all_records)
    }

    /// Fast parsing of data block optimized for compaction bulk reads
    /// Avoids full DataBlock struct deserialization for better performance
    fn fast_parse_data_block(
        &self,
        block_data: &[u8],
        block_id: usize,
    ) -> std::result::Result<BTreeMap<String, super::SstRecord>, crate::core::StorageError> {
        use std::io::Read;
        
        let mut cursor = std::io::Cursor::new(block_data);
        let mut records = BTreeMap::new();
        
        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic).map_err(|e| 
            crate::core::StorageError::SstStorage(format!("Failed to read magic header: {}", e)))?;
        
        if &magic != b"BLK1" {
            return Err(crate::core::StorageError::SstStorage(
                format!("Invalid DataBlock format - expected BLK1 magic, got {:?}", magic)
            ));
        }
        
        // Read DataBlock header: [magic:4][block_id:4][uncompressed_size:4][record_count:4]
        let mut header = [0u8; 12];
        cursor.read_exact(&mut header).map_err(|e| 
            crate::core::StorageError::SstStorage(format!("Failed to read block header: {}", e)))?;
        
        let stored_block_id = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let _uncompressed_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let record_count = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        
        debug!("Block header - id: {}, uncompressed_size: {}, record_count: {}", stored_block_id, _uncompressed_size, record_count);
        
        if stored_block_id as usize != block_id {
            warn!("🔍 COMPACTION: Block ID mismatch: expected {}, got {}", block_id, stored_block_id);
        }
        
        // STREAMING PARSE: Read records one by one without creating intermediate DataBlock
        debug!("Starting to parse {} records from block {}", record_count, block_id);
        for record_idx in 0..record_count {
            // Read record length
            let mut len_buf = [0u8; 4];
            if cursor.read_exact(&mut len_buf).is_err() {
                warn!("🔍 COMPACTION: Failed to read record {} length in block {}", record_idx, block_id);
                break;
            }
            
            let record_len = u32::from_le_bytes(len_buf) as usize;
            if record_len == 0 || record_len > 1024 * 1024 { // Sanity check: max 1MB per record
                warn!("🔍 COMPACTION: Invalid record length {} for record {} in block {}", 
                      record_len, record_idx, block_id);
                break;
            }
            
            // Read record data
            let mut record_data = vec![0u8; record_len];
            if cursor.read_exact(&mut record_data).is_err() {
                warn!("🔍 COMPACTION: Failed to read record {} data in block {}", record_idx, block_id);
                break;
            }
            
            // FAST DESERIALIZE: Use custom SstRecord deserialization
            match super::SstRecord::deserialize(&record_data) {
                Ok(record) => {
                    debug!("Successfully deserialized record {} with id: {}", record_idx, record.id);
                    records.insert(record.id.clone(), record);
                }
                Err(e) => {
                    warn!("Failed to deserialize record {} in block {}: {}", record_idx, block_id, e);
                    warn!("🔍 COMPACTION: Failed to deserialize record {} in block {}: {}", 
                          record_idx, block_id, e);
                }
            }
        }
        
        debug!("fast_parse_data_block completed - {} records extracted from block {}", records.len(), block_id);
        Ok(records)
    }

    /// Generate output file path for compacted SST
    /// Uses the same naming pattern as flush: {collection_id}_level{level}_{timestamp}_{random}.sst
    fn generate_output_file_path(&self, collection_id: &str, collection_dir: &Path, level: u8) -> PathBuf {
        let timestamp = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let random_id: u32 = rand::thread_rng().gen();
        
        // Use the same naming pattern as flush to ensure consistency
        let filename = format!("{}_level{}_{}_{}.sst", collection_id, level, timestamp, random_id);
        collection_dir.join(filename)
    }

    /// Sort vector records by metadata for optimal compaction encoding
    /// Uses same sorting strategy as flush operations to maintain consistency
    async fn sort_vectors_for_compaction(
        vector_records: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        debug!("🔄 Sorting {} vectors for optimal SSTable compaction encoding", vector_records.len());
        
        if vector_records.is_empty() {
            return Ok((vector_records, SortingStats::default()));
        }

        // Create metadata sorter for optimal SSTable encoding
        let sorter = MetadataSorter::new(Default::default());
        
        // Sort records to optimize for:
        // 1. Sequential access patterns in SSTable blocks
        // 2. Better compression ratios in block-based storage
        // 3. Improved bloom filter effectiveness
        let (sorted_records, sort_stats) = sorter.sort_for_encoding(vector_records)?;
        
        debug!("✅ LSM compaction sorting complete: {} records sorted for optimal SSTable encoding", 
               sorted_records.len());
        
        Ok((sorted_records, sort_stats))
    }
}

/// Determine if a new record should replace an existing record during compaction
fn should_replace_record(existing: &SstRecord, new: &SstRecord) -> bool {
    // LSM compaction rule: newer records (higher sequence number) replace older ones
    // For records with same sequence number, prefer by timestamp
    if new.sequence_number != existing.sequence_number {
        new.sequence_number > existing.sequence_number
    } else {
        new.timestamp > existing.timestamp
    }
}

impl Drop for CompactionManager {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Abort remaining worker handles
        for handle in &self.worker_handles {
            handle.abort();
        }
    }
}

// Test module for vector tracking during compaction
#[cfg(test)]
#[path = "compaction_vector_tracking_tests.rs"]
mod vector_tracking_tests;

#[cfg(test)]
mod tests {
    use super::*;
    

    #[tokio::test]
    async fn test_compaction_manager_basic() {
        let mut config = SstConfig::default();
        config.memtable_size_mb = 1;
        config.level_count = 3;
        config.compaction_threshold = 2;
        config.block_size_kb = 4;

        let mut manager = CompactionManager::new(config);
        assert!(manager.start_workers(1).await.is_ok());
        assert!(manager.stop().await.is_ok());
    }

    #[tokio::test]
    async fn test_compaction_task_scheduling() {
        let mut config = SstConfig::default();
        config.memtable_size_mb = 1;
        config.level_count = 3;
        config.compaction_threshold = 2;
        config.block_size_kb = 4;

        let manager = CompactionManager::new(config);

        let task = CompactionTask {
            collection_id: "test_collection".to_string(),
            level: 0,
            input_files: vec![],
            output_file: PathBuf::from("/tmp/output.db"),
            priority: CompactionPriority::Medium,
        };

        assert!(manager.schedule_compaction(task).await.is_ok());
    }
}
