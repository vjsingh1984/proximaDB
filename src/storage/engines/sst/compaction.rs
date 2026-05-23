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

use super::SstableWriter; // OPTIMIZED: Removed SstRecord import
use super::block_format::BlockFormat;
use super::compactor_impl::{SstCompactor, ZeroCopyCompactionStats};
use crate::core::search::mvcc_resolution::MvccResolver;
use crate::core::{SstConfig, String}; // OPTIMIZED: VectorRecord imported above
use crate::proto::proximadb_v1::VectorRecord; // OPTIMIZED: Added VectorRecord import
use crate::storage::Result;
// Removed ZeroCopyIOSystem - using UnifiedCachingFilesystem instead
use crate::storage::engines::core::formats::arrow_block::{ArrowBlockConfig, ArrowBlockWriter};
use crate::storage::engines::sst::readers::sst_query_engine::UnifiedSstableReader;
use crate::storage::optimization::{MetadataSorter, SortingStats};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::filesystem::caching_filesystem::UnifiedCachingFilesystem;
use crate::storage::transaction_coordinator::{
    StagingConfig, TransactionCoordinator, TransactionStageType,
};
use proximadb_records::ProximaRecord;
// Quantization now handled by unified compute module
// Import unified level-based compaction framework
use crate::storage::common::compaction_utils::{
    CompactionTaskBuilder, StorageEngineType as CompactionEngineType,
};
use crate::storage::common::*;

/// Temporary compatibility structure for level-based compaction task
#[derive(Debug, Clone)]
pub struct LevelBasedCompactionTask {
    pub level: u32,
    pub input_files: Vec<String>,
    pub target_level: u32,
    pub extension: String,
}
use chrono::Utc;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Compaction task to be processed by background workers
#[derive(Debug, Clone)]
pub struct CompactionTask {
    pub level: u8,
    pub input_files: Vec<PathBuf>,
    pub output_file: PathBuf,
    pub priority: CompactionPriority,
    /// Block size in KB for compacted output (uses server default if None)
    pub block_size_kb: Option<u32>,
    /// Compression configuration (uses server default if None)
    pub compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
}

/// Priority levels for compaction tasks
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionPriority {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3, // When storage is nearly full
}

/// Backwards-compat alias for [`SstCompactionStats`].
pub type CompactionStats = SstCompactionStats;

/// Statistics for compaction operations
#[derive(Debug, Clone, Default)]
pub struct SstCompactionStats {
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
pub struct EnhancedSstCompactionStats {
    /// Basic compaction statistics
    pub base_stats: SstCompactionStats,

    /// Vector IDs that were deleted (expired or tombstoned)
    pub deleted_vector_ids: Vec<String>,

    /// Vectors that were merged/updated
    pub merged_vectors: Vec<VectorRecord>,

    /// Whether a full index rebuild is recommended
    pub recommend_full_rebuild: bool,
}

/// Manages background compaction of SST files
pub struct Compaction {
    config: SstConfig,
    task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
    task_notify: Arc<Notify>,
    worker_handles: Vec<JoinHandle<()>>,
    shutdown_signal: Arc<AtomicBool>,
    stats: Arc<RwLock<SstCompactionStats>>,
    active_compactions: Arc<RwLock<HashMap<String, CompactionTask>>>,
    #[allow(dead_code)]
    atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    unified_reader: Arc<UnifiedSstableReader>,
    sst_compactor: Option<Arc<SstCompactor>>,
    filesystem_factory: Arc<FilesystemFactory>,
    /// New compaction orchestrator
    #[allow(dead_code)]
    compaction_orchestrator: Option<Arc<CompactionOrchestrator>>,
    // manifest: Option<Arc<super::SstManifest>>, // Removed - using directory discovery
}

impl std::fmt::Debug for Compaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compaction")
            .field("config", &self.config)
            .field("task_queue", &"<task_queue>")
            .field("task_notify", &"<task_notify>")
            .field("worker_handles", &self.worker_handles.len())
            .field("shutdown_signal", &self.shutdown_signal)
            .field("stats", &"<stats>")
            .field("active_compactions", &"<active_compactions>")
            .field("atomic_coordinator", &self.atomic_coordinator.is_some())
            .field("unified_reader", &"<unified_reader>")
            .field("sst_compactor", &self.sst_compactor.is_some())
            .field("filesystem_factory", &"<filesystem_factory>")
            .finish()
    }
}

impl Compaction {
    /// Extract collection ID from file paths
    #[allow(dead_code)]
    fn extract_collection_id_from_paths(&self, paths: &[PathBuf]) -> Result<String> {
        if paths.is_empty() {
            return Ok("unknown".to_string());
        }

        // Extract collection ID from path like: /path/to/collection_id/data/level0/file.sst
        if let Some(path) = paths.first()
            && let Some(_parent) = path.parent()
            && let Some(_parent_parent) = _parent.parent()
            && let Some(parent_parent_parent) = _parent_parent.parent()
            && let Some(_collection_id) = parent_parent_parent.file_name()
        {
            return Ok(_collection_id.to_string_lossy().to_string());
        }

        Ok("unknown".to_string())
    }

    /// Create a new compaction manager
    pub async fn new(config: SstConfig) -> Result<Self> {
        Self::with_atomic_coordinator(config, None).await
    }

    /// Create a new compaction manager with atomic coordinator
    pub async fn with_atomic_coordinator(
        config: SstConfig,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    ) -> Result<Self> {
        // Create unified reader with filesystem factory
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?,
        );
        // Create unified caching filesystem for compaction
        let base_fs = filesystem_factory.get_filesystem("file://").map_err(|e| {
            crate::core::StorageError::SstEngine(format!("Failed to get base filesystem: {}", e))
        })?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
            base_fs,
            "compaction".to_string(), // collection_id
            "sst_compaction".to_string(),
        ));
        let unified_reader = Arc::new(UnifiedSstableReader::new(
            filesystem_factory.clone(),
            unified_fs,
            String::from("compaction"),
        ));

        // Initialize zero-copy compactor with proper block size from config
        let sst_compactor = if let Some(ref _coord) = atomic_coordinator {
            // Create MVCC resolver for the compactor
            let mvcc_resolver = Arc::new(MvccResolver::new());
            debug!(
                "🔍 COMPACTION_MANAGER: Creating SstCompactor with block_size_kb: {}",
                config.block_size_kb
            );
            Some(Arc::new(SstCompactor::with_block_size(
                filesystem_factory.clone(),
                Some(mvcc_resolver),
                config.block_size_kb,
            )))
        } else {
            // No atomic coordinator, create compactor without MVCC resolver
            debug!(
                "🔍 COMPACTION_MANAGER: Creating SstCompactor with block_size_kb: {}",
                config.block_size_kb
            );
            Some(Arc::new(SstCompactor::with_block_size(
                filesystem_factory.clone(),
                None,
                config.block_size_kb,
            )))
        };

        // Initialize new compaction orchestrator with SST-specific configuration
        let compaction_config = CompactionConfig {
            level0_threshold: config.compaction_threshold as usize,
            level_threshold: (config.compaction_threshold * 2) as usize,
            max_level: config.level_count as u32,
            max_concurrent_per_collection: 1,
            global_max_concurrent: 4,
            operation_timeout: std::time::Duration::from_secs(3600),
            queue_aware_compaction: true,
            max_queue_wait: std::time::Duration::from_secs(60),
            urgency_threshold: 0.8,
        };

        let orchestrator = Some(Arc::new(CompactionOrchestrator::new(
            filesystem_factory.clone(),
            compaction_config,
        )));

        Ok(Self {
            config,
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            task_notify: Arc::new(Notify::new()),
            worker_handles: Vec::new(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(SstCompactionStats::default())),
            active_compactions: Arc::new(RwLock::new(HashMap::new())),
            atomic_coordinator,
            unified_reader,
            sst_compactor,
            filesystem_factory,
            compaction_orchestrator: orchestrator,
        })
    }

    // Quantization sorting now handled by unified compute module
    pub async fn with_quantization_sorting(mut self) -> Result<Self> {
        // PQ-based sorting is now integrated directly into the compactor
        // using the unified quantization engine
        if let Some(ref mut _compactor) = self.sst_compactor {
            // Create unified quantization engine for PQ-based sorting
            let distance_compute = Arc::new(
                crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
            );
            let codebook_store = Arc::new(
                crate::compute::quantization::quantization_engine::InMemoryCodebookStore::new(),
            );
            let unified_engine = Arc::new(
                crate::compute::quantization::quantization_engine::UnifiedQuantizationEngine::new(
                    distance_compute,
                    codebook_store,
                ),
            );
            // Create distance compute engine
            let distance_compute = Arc::new(
                crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                ),
            );
            // Create quantization config
            let quant_config =
                crate::compute::quantization::storage_engine::StorageQuantizationConfig::default();
            let quantization_engine = Arc::new(
                crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                    unified_engine,
                    distance_compute,
                    quant_config,
                ),
            );

            let new_compactor = Arc::new(
                SstCompactor::with_block_size(
                    self.filesystem_factory.clone(),
                    self.atomic_coordinator
                        .as_ref()
                        .map(|_coord| Arc::new(MvccResolver::new())),
                    self.config.block_size_kb,
                )
                .with_pq_sorting(quantization_engine),
            );

            self.sst_compactor = Some(new_compactor);

            info!(
                "🎯 Compaction configured with PQ-based similarity sorting for better compression"
            );
        } else {
            warn!("⚠️ Cannot enable PQ sorting: no SST compactor available");
        }

        Ok(self)
    }

    /// Start background compaction workers
    pub async fn start_workers(&mut self, worker_count: usize) -> Result<()> {
        info!("Starting {} compaction workers", worker_count);

        for worker_id in 0..worker_count {
            let task_queue = Arc::clone(&self.task_queue);
            let task_notify = Arc::clone(&self.task_notify);
            let shutdown_signal = Arc::clone(&self.shutdown_signal);
            let stats = Arc::clone(&self.stats);
            let active_compactions = Arc::clone(&self.active_compactions);
            let atomic_coordinator = self.atomic_coordinator.clone();
            let config = self.config.clone();

            let handle = tokio::spawn(async move {
                Self::worker_loop(
                    worker_id,
                    task_queue,
                    task_notify,
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
        self.task_notify.notify_waiters();

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
            "Scheduling compaction for level {} with {} input files",
            task.level,
            task.input_files.len()
        );

        // Use the output file path as a unique key for active compactions
        // This prevents multiple compactions writing to the same output file
        let compaction_key = task.output_file.to_string_lossy().to_string();

        // Check if there's already an active compaction for this output file
        {
            let active = self.active_compactions.read().await;
            if active.contains_key(&compaction_key) {
                debug!(
                    "Skipping compaction - already active for output file {}",
                    compaction_key
                );
                return Ok(());
            }
        }

        let mut queue = self.task_queue.lock().await;

        // Insert task in priority order
        let insert_pos = queue
            .iter()
            .position(|existing_task| existing_task.priority < task.priority)
            .unwrap_or(queue.len()); // Insert at end if no lower priority task found

        queue.insert(insert_pos, task);
        drop(queue);
        self.task_notify.notify_one();

        debug!(
            "Compaction task queued (position: {}, queue size: {})",
            insert_pos,
            self.task_queue.lock().await.len()
        );

        Ok(())
    }

    /// Check if compaction is needed for the given collection and level
    pub async fn check_compaction_needed(
        &self,
        collection_id: &str,
        collection_dir: &Path,
    ) -> Result<Option<CompactionTask>> {
        debug!(
            "🔍 SST COMPACTION: Delegating to unified framework for collection: {}",
            collection_id
        );

        let collection_path = collection_dir.to_string_lossy();

        // Use SST-specific config if available, otherwise use defaults
        let compaction_config = self
            .config
            .compaction_config
            .as_ref()
            .cloned()
            .unwrap_or_else(crate::core::config::CompactionConfig::default);

        // Use unified compaction task builder with configuration
        let task_info = CompactionTaskBuilder::check_and_build_compaction_task(
            collection_id,
            &collection_path,
            "sst",
            CompactionEngineType::SST,
            &compaction_config,
            self.filesystem_factory.clone(),
        )
        .await
        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

        let unified_task = task_info.map(|info| LevelBasedCompactionTask {
            level: info.source_level,
            input_files: info.input_files,
            target_level: info.target_level,
            extension: info.extension,
        });

        if let Some(task) = unified_task {
            debug!(
                "🔄 SST COMPACTION: Converting unified task to SST-specific format for collection {} level {}",
                collection_id, task.level
            );

            // Convert unified task to SST CompactionTask
            let input_files: Vec<PathBuf> =
                task.input_files.into_iter().map(PathBuf::from).collect();

            let output_file = self.generate_output_file_path(
                collection_id,
                collection_dir,
                task.target_level as u8,
            );

            let priority = if input_files.len() >= (self.config.compaction_threshold * 2) as usize {
                CompactionPriority::High
            } else {
                CompactionPriority::Medium
            };

            return Ok(Some(CompactionTask {
                level: task.level as u8,
                input_files,
                output_file,
                priority,
                block_size_kb: None,      // Use server default
                compression_config: None, // Use server default
            }));
        }

        debug!(
            "📋 COMPACTION: Unified framework reports no compaction needed for collection: {}",
            collection_id
        );
        Ok(None)
    }

    /// Get compaction statistics
    pub async fn stats(&self) -> SstCompactionStats {
        self.stats.read().await.clone()
    }

    /// Worker loop for processing compaction tasks
    async fn worker_loop(
        worker_id: usize,
        task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
        task_notify: Arc<Notify>,
        shutdown_signal: Arc<AtomicBool>,
        stats: Arc<RwLock<SstCompactionStats>>,
        active_compactions: Arc<RwLock<HashMap<String, CompactionTask>>>,
        config: SstConfig,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    ) {
        debug!("Compaction worker {} started", worker_id);

        loop {
            if shutdown_signal.load(Ordering::SeqCst) {
                break;
            }

            // Register for notification before checking the queue to avoid lost wakeups.
            let notified = task_notify.notified();

            // Get next task from queue
            let task = {
                let mut queue = task_queue.lock().await;
                queue.pop_front()
            };

            if let Some(task) = task {
                debug!(
                    "Worker {} processing compaction for level {} with {} files -> {}",
                    worker_id,
                    task.level,
                    task.input_files.len(),
                    task.output_file.display()
                );

                // Mark as active using output file as key
                let compaction_key = task.output_file.to_string_lossy().to_string();
                {
                    let mut active = active_compactions.write().await;
                    active.insert(compaction_key.clone(), task.clone());
                }

                let start_time = std::time::Instant::now();

                // Perform compaction - create a temporary manager for SSTable parsing
                let temp_manager = match Compaction::with_atomic_coordinator(
                    config.clone(),
                    atomic_coordinator.clone(),
                )
                .await
                {
                    Ok(manager) => manager,
                    Err(e) => {
                        error!("Failed to create compaction manager: {}", e);
                        continue;
                    }
                };
                match temp_manager
                    .perform_compaction(&task, &config, atomic_coordinator.clone())
                    .await
                {
                    Ok(compaction_stats) => {
                        info!(
                            "Compaction completed for level {} in {}ms: {} files merged -> {}",
                            task.level,
                            start_time.elapsed().as_millis(),
                            compaction_stats.files_merged,
                            task.output_file.display()
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
                            "Compaction failed for level {} -> {}: {}",
                            task.level,
                            task.output_file.display(),
                            e
                        );
                    }
                }

                // Remove from active compactions
                {
                    let mut active = active_compactions.write().await;
                    active.remove(&compaction_key);
                }
            } else {
                notified.await;
            }
        }

        debug!("Compaction worker {} stopped", worker_id);
    }

    /// Perform the actual compaction operation
    async fn perform_compaction(
        &self,
        task: &CompactionTask,
        _config: &SstConfig,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    ) -> Result<SstCompactionStats> {
        let enhanced_stats = self
            .perform_compaction_enhanced(task, _config, atomic_coordinator, None)
            .await?;
        Ok(enhanced_stats.base_stats)
    }

    /// UNIFIED compaction using VectorRecord natively (eliminates dual paths)
    /// OPTIMIZATION: Single path for all compaction, reuses existing reader/writer infrastructure
    pub async fn perform_compaction_enhanced(
        &self,
        task: &CompactionTask,
        _config: &SstConfig,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<EnhancedSstCompactionStats> {
        debug!(
            "🚀 UNIFIED COMPACTION: Single optimized path for {} files at level {}",
            task.input_files.len(),
            task.level
        );

        // SINGLE PATH: Use unified VectorRecord path (fastest, no SstRecord conversions)
        self.perform_unified_vectorrecord_compaction(
            task,
            _config,
            atomic_coordinator,
            compression_config,
        )
        .await
    }

    /// Convert zero-copy stats to enhanced stats format
    #[allow(dead_code)]
    fn convert_zero_copy_stats_to_enhanced(
        &self,
        stats: ZeroCopyCompactionStats,
    ) -> EnhancedSstCompactionStats {
        // For zero-copy compaction, we don't have VectorRecords but we DO have counts
        // Create placeholder VectorRecords just for counting purposes
        let merged_vectors = (0..stats.records_written)
            .map(|_| VectorRecord::default())
            .collect();

        EnhancedSstCompactionStats {
            base_stats: SstCompactionStats {
                total_compactions: 1,
                bytes_written: stats.bytes_written,
                bytes_read: stats.bytes_read,
                files_merged: stats.files_compacted as u64,
                avg_compaction_time_ms: stats.compaction_time_ms,
                last_compaction_time: Some(chrono::Utc::now()),
                expired_records_deleted: stats.records_deleted,
                tombstones_removed: stats.tombstoned_ids.len() as u64,
            },
            deleted_vector_ids: stats.deleted_vector_ids,
            merged_vectors, // Use placeholder records for accurate counting
            recommend_full_rebuild: stats.recommend_index_rebuild,
        }
    }

    /// Original enhanced compaction implementation (now used as fallback)
    /// UNIFIED VectorRecord compaction (eliminates SstRecord conversions completely)
    /// OPTIMIZATION: Single streaming path, no dual conversions, fastest performance
    async fn perform_unified_vectorrecord_compaction(
        &self,
        task: &CompactionTask,
        _config: &SstConfig,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
        compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    ) -> Result<EnhancedSstCompactionStats> {
        debug!(
            "🚀 UNIFIED COMPACTION: VectorRecord-only path with compression: {:?}",
            compression_config
                .as_ref()
                .map(|c| format!("algorithm={}, level={:?}", c.algorithm, c.level))
        );
        let start_time = std::time::Instant::now();

        // OPTIMIZATION: Direct VectorRecord collection, no SstRecord conversions
        let mut all_vector_records: Vec<VectorRecord> = Vec::new();
        let mut bytes_read = 0u64;

        debug!(
            "🚀 UNIFIED: Merging {} input files for level {} (VectorRecord-only path)",
            task.input_files.len(),
            task.level
        );

        // Read and merge all input files using existing infrastructure
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?,
        );
        let fs = filesystem_factory
            .get_filesystem("file:///")
            .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

        for input_file in &task.input_files {
            let input_path = input_file.to_string_lossy();

            // OPTIMIZED: Direct VectorRecord extraction (no SstRecord conversions)
            match self.read_all_records_from_file_unified(&input_path).await {
                Ok(records) => {
                    info!(
                        "✅ Extracted {} VectorRecords from {} (no conversions)",
                        records.len(),
                        input_path
                    );
                    // Estimate file size for statistics
                    if let Ok(metadata) = fs.metadata(&input_path).await {
                        bytes_read += metadata.size;
                    }

                    if records.is_empty() {
                        warn!("No records extracted from SST file: {}", input_path);
                    }

                    // FASTEST: Direct VectorRecord append (no conversions!)
                    all_vector_records.extend(records);
                }
                Err(e) => {
                    warn!(
                        "Failed to read records from {} using unified reader: {}",
                        input_path, e
                    );
                    continue;
                }
            }
        }

        info!(
            "✅ Collected {} total VectorRecords from {} input files (no conversions)",
            all_vector_records.len(),
            task.input_files.len()
        );

        // OPTIMIZED: Sort VectorRecords directly by (id, version, timestamp) for merge deduplication
        all_vector_records.sort_by(|a, b| {
            let id_a = &a.id;
            let id_b = &b.id;

            // First sort by ID
            match id_a.cmp(id_b) {
                std::cmp::Ordering::Equal => {
                    // For same ID, sort by version (newer versions first)
                    match b.version.unwrap_or(0).cmp(&a.version.unwrap_or(0)) {
                        std::cmp::Ordering::Equal => {
                            // For same version, sort by timestamp (newer timestamp first)
                            b.timestamp.cmp(&a.timestamp)
                        }
                        other => other,
                    }
                }
                other => other,
            }
        });

        // OPTIMIZED: Merge-deduplicate legacy SST records directly, then lower
        // through canonical ProximaRecord for MVCC resolution.
        let mut merged_vector_records: Vec<VectorRecord> = Vec::new();
        let mut last_id = String::new();

        for record in all_vector_records {
            let record_id = &record.id;

            // For append-only vectors (empty IDs), keep all records
            if record_id.is_empty() || record_id.starts_with("__append_only_") {
                merged_vector_records.push(record);
            } else if record_id != &last_id {
                // New ID, add it
                last_id = record_id.clone();
                merged_vector_records.push(record);
            } else {
                // Same ID, skip (we already have the latest version due to sorting)
            }
        }

        info!(
            "🔍 UNIFIED COMPACTION: Merged to {} unique VectorRecords after deduplication",
            merged_vector_records.len()
        );

        let resolver = MvccResolver::new();
        let canonical_records: Vec<ProximaRecord> = merged_vector_records
            .iter()
            .map(ProximaRecord::from)
            .collect();
        let resolved_records: Vec<VectorRecord> = resolver
            .resolve_batch(canonical_records)
            .into_iter()
            .map(VectorRecord::from)
            .collect();
        info!(
            "🔍 UNIFIED COMPACTION: MVCC resolution: {} records after resolution",
            resolved_records.len()
        );

        // Convert merged data to vectors for sorting
        let mut vector_records = Vec::new();
        let current_time = chrono::Utc::now().timestamp_millis();
        let mut expired_records_count = 0;
        let mut tombstones_removed_count = 0;

        // Track deleted vectors for AXIS
        let mut deleted_vector_ids = Vec::new();
        let mut merged_vectors = Vec::new();

        // Get current time in seconds for consistent comparison
        let current_time_secs = current_time / 1000;

        for vector_record in &resolved_records {
            // Tombstone detection: empty vector + expires_at in past (including 0)
            // expires_at = 0 means "epoch time" which is always in the past = tombstone marker
            let is_tombstone = vector_record.vector.is_empty()
                && vector_record
                    .expires_at
                    .is_some_and(|e| e <= current_time_secs);

            // Check if record is expired (TTL-based expiry for non-tombstones)
            // This is different from tombstones - these are regular records that have expired
            let is_expired = !is_tombstone
                && vector_record
                    .expires_at
                    .is_some_and(|expires_at| expires_at > 0 && expires_at < current_time_secs);

            // Skip expired records completely - they are physically deleted
            if is_expired {
                expired_records_count += 1;
                // Track deleted vector for AXIS
                deleted_vector_ids.push(vector_record.id.clone());
                continue;
            }
            let should_keep = if is_tombstone {
                // Keep tombstones that are less than 1 hour old
                let age = (current_time / 1000) - vector_record.timestamp.unwrap_or(0); // Both in seconds
                let keep_tombstone = age < (60 * 60); // 1 hour in seconds

                if !keep_tombstone {
                    tombstones_removed_count += 1;
                    // Track deleted vector for AXIS
                    deleted_vector_ids.push(vector_record.id.clone());
                }

                keep_tombstone
            } else {
                true // Keep all active, non-expired records
            };

            if should_keep {
                // OPTIMIZED: Direct VectorRecord usage (already a VectorRecord)

                // Track merged vectors for AXIS (non-tombstone records)
                if !is_tombstone {
                    merged_vectors.push(vector_record.clone());
                }

                vector_records.push(vector_record.clone());
            }
        }

        // Log cleanup statistics
        if expired_records_count > 0 || tombstones_removed_count > 0 {
            info!(
                "🧹 LSM COMPACTION CLEANUP: {} expired records deleted, {} old tombstones removed",
                expired_records_count, tombstones_removed_count
            );
        }

        // OPTIMIZED: Sort VectorRecords by metadata for optimal encoding (no conversions)
        let resolved_records_len = resolved_records.len();
        info!(
            "🔄 UNIFIED COMPACTION: Sorting {} VectorRecords by metadata for optimal encoding",
            resolved_records_len
        );
        let (sorted_vectors, sort_stats) =
            Self::sort_vectors_for_compaction(resolved_records).await?;
        info!(
            "✅ UNIFIED COMPACTION: Sorted records (estimated compression improvement: {:.1}%)",
            sort_stats.compression_estimate * 100.0
        );

        // FASTEST: Direct VectorRecord to writer (no SstRecord conversions!)
        let mut sorted_vector_records: Vec<(String, VectorRecord)> = Vec::new();
        for (seq, vector) in sorted_vectors.into_iter().enumerate() {
            let vector_id = if vector.id.is_empty() {
                String::new()
            } else {
                vector.id.clone()
            };

            // Handle append-only vectors (empty/null IDs) specially
            let key = if vector_id.is_empty() {
                // For append-only vectors, use sequence number as unique key
                let append_only_key = format!("__append_only_seq_{}", seq);
                info!(
                    "🔍 UNIFIED: Append-only vector at sequence {}, using key='{}'",
                    seq, append_only_key
                );
                append_only_key
            } else {
                vector_id
            };

            // NO CONVERSIONS: Direct VectorRecord use
            sorted_vector_records.push((key, vector));
        }

        info!(
            "🔍 UNIFIED COMPACTION: Prepared {} sorted VectorRecords (zero conversions)",
            sorted_vector_records.len()
        );

        // Handle empty records case - return early without writing SSTable
        if sorted_vector_records.is_empty() {
            info!(
                "📋 SST COMPACTION: No records to compact after merging. Returning without writing SSTable."
            );
            return Ok(EnhancedSstCompactionStats {
                base_stats: SstCompactionStats {
                    total_compactions: 1,
                    bytes_written: 0,
                    bytes_read,
                    files_merged: task.input_files.len() as u64,
                    avg_compaction_time_ms: start_time.elapsed().as_millis() as u64,
                    last_compaction_time: Some(chrono::Utc::now()),
                    expired_records_deleted: 0,
                    tombstones_removed: 0,
                },
                merged_vectors: Vec::new(),
                deleted_vector_ids: Vec::new(),
                recommend_full_rebuild: false,
            });
        }

        // Convert to BTreeMap for SSTable writer (temporary, until we update writer)
        let mut btree_records = BTreeMap::new();
        for (key, record) in sorted_vector_records {
            btree_records.insert(key, record);
        }

        // Use task-specific block size if provided, otherwise fall back to server config
        let block_size_kb = task.block_size_kb.unwrap_or(64); // Default to 64KB blocks
        let block_size = (block_size_kb * 1024) as usize;

        // Deferred: Pass filesystem from compaction manager - for now create a new factory
        let filesystem_factory = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?,
        );

        let bytes_written = if let Some(ref coordinator) = atomic_coordinator {
            // Use atomic operations for compaction
            info!("🔒 LSM COMPACTION: Using atomic operations for compaction_info");

            // Create staging configuration
            // Don't include collection_id in StagingConfig - the base_url already points to the final location
            let staging_config = StagingConfig {
                base_url: task
                    .output_file
                    .parent()
                    .ok_or_else(|| {
                        crate::core::StorageError::SstEngine("Invalid output file path".to_string())
                    })?
                    .to_string_lossy()
                    .to_string(),
                collection_id: None, // Don't add /collections/{id} structure
                operation_type: TransactionStageType::Compaction,
                skip_uuid_subdir: true, // Use simple __compact directory without UUID subdirectory
                ..Default::default()
            };

            // Begin atomic operation
            let atomic_op = coordinator
                .begin_atomic_operation(&staging_config)
                .await
                .map_err(|e| {
                    crate::core::StorageError::SstEngine(format!(
                        "Failed to begin atomic operation: {}",
                        e
                    ))
                })?;

            debug!(
                "Started atomic operation {} for compaction_info",
                atomic_op.operation_id
            );

            // Get the staging filename
            let staging_filename = task
                .output_file
                .file_name()
                .ok_or_else(|| {
                    crate::core::StorageError::SstEngine("Invalid output filename".to_string())
                })?
                .to_string_lossy()
                .to_string();

            // Write SSTable directly to staging path
            // Strip file:// prefix if present for local filesystem operations
            let staging_path = if atomic_op.staging_url.starts_with("file://") {
                atomic_op
                    .staging_url
                    .strip_prefix("file://")
                    .unwrap_or(&atomic_op.staging_url)
            } else {
                &atomic_op.staging_url
            };
            let staging_file_path = PathBuf::from(format!("{}/{}", staging_path, staging_filename));
            debug!("Writing to staging path: {}", staging_file_path.display());

            // Check block format and use appropriate writer
            let block_format = BlockFormat::parse_block_format(&self.config.block_format);
            debug!("🔍 COMPACTION: Using block format: {:?}", block_format);

            match block_format {
                BlockFormat::ArrowBlock => {
                    // Use ArrowBlockWriter for Arrow IPC format
                    debug!("🔍 COMPACTION: Using ArrowBlockWriter for Arrow format");

                    // Infer dimension from first record
                    let dimension = btree_records
                        .values()
                        .next()
                        .map_or(128, |r| r.vector.len() as u32);

                    // Ensure parent directory exists
                    if let Some(parent) = staging_file_path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(crate::core::StorageError::DiskIO)?;
                    }

                    let config = ArrowBlockConfig::new(dimension);
                    let mut writer = ArrowBlockWriter::new(&staging_file_path, config)
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

                    // ArrowBlock writes canonical records; this branch is the
                    // boundary from legacy SST compaction records.
                    let records: Vec<ProximaRecord> =
                        btree_records.values().map(ProximaRecord::from).collect();

                    writer
                        .write_block(&records)
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;
                    writer
                        .finalize()
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

                    info!(
                        "✅ COMPACTION: Wrote {} records to Arrow block",
                        records.len()
                    );
                }
                BlockFormat::ProximaBlocks => {
                    // Use SstableWriter for ProximaBlocks format
                    debug!("🔍 COMPACTION: Creating SstableWriter with compression config");
                    let writer = if let Some(ref compression) = compression_config {
                        debug!(
                            "   Using compression: algorithm={}, level={:?}",
                            compression.algorithm, compression.level
                        );
                        SstableWriter::with_compression(
                            &staging_file_path,
                            block_size,
                            filesystem_factory.clone(),
                            Some(compression.clone()),
                        )
                    } else {
                        debug!("   No compression - using default writer");
                        SstableWriter::new(
                            &staging_file_path,
                            block_size,
                            filesystem_factory.clone(),
                        )
                    };
                    let record_count = btree_records.len();
                    let sorted_records_iter = btree_records.into_iter();
                    writer
                        .write_sorted_vector_records(sorted_records_iter, record_count)
                        .await
                        .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;
                }
            }

            // Get file size for stats
            let metadata = fs
                .metadata(&staging_file_path.to_string_lossy())
                .await
                .map_err(|e| {
                    crate::core::StorageError::DiskIO(std::io::Error::other(e.to_string()))
                })?;
            let written_bytes = metadata.size;

            // Finalize atomic operation - this moves the file from staging to final location
            coordinator
                .finalize_atomic_operation(&atomic_op.operation_id)
                .await
                .map_err(|e| {
                    crate::core::StorageError::SstEngine(format!(
                        "Failed to finalize atomic operation: {}",
                        e
                    ))
                })?;

            info!(
                "✅ LSM COMPACTION: Atomic operation {} completed successfully",
                atomic_op.operation_id
            );
            debug!("File should be at final location: {}", atomic_op.final_url);

            written_bytes
        } else {
            // Fallback to direct write (non-atomic)
            debug!("Writing directly to: {}", task.output_file.display());

            // Check block format and use appropriate writer
            let block_format = BlockFormat::parse_block_format(&self.config.block_format);
            debug!(
                "🔍 COMPACTION (non-atomic): Using block format: {:?}",
                block_format
            );

            match block_format {
                BlockFormat::ArrowBlock => {
                    // Use ArrowBlockWriter for Arrow IPC format
                    debug!("🔍 COMPACTION (non-atomic): Using ArrowBlockWriter for Arrow format");

                    // Infer dimension from first record
                    let dimension = btree_records
                        .values()
                        .next()
                        .map_or(128, |r| r.vector.len() as u32);

                    // Ensure parent directory exists
                    if let Some(parent) = task.output_file.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(crate::core::StorageError::DiskIO)?;
                    }

                    let config = ArrowBlockConfig::new(dimension);
                    let mut writer = ArrowBlockWriter::new(&task.output_file, config)
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

                    // ArrowBlock writes canonical records; this branch is the
                    // boundary from legacy SST compaction records.
                    let records: Vec<ProximaRecord> =
                        btree_records.values().map(ProximaRecord::from).collect();

                    writer
                        .write_block(&records)
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;
                    writer
                        .finalize()
                        .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?;

                    info!(
                        "✅ COMPACTION (non-atomic): Wrote {} records to Arrow block",
                        records.len()
                    );
                }
                BlockFormat::ProximaBlocks => {
                    // Use SstableWriter for ProximaBlocks format
                    debug!(
                        "🔍 COMPACTION (non-atomic): Creating SstableWriter with compression config"
                    );
                    let writer = if let Some(ref compression) = compression_config {
                        debug!(
                            "   Using compression: algorithm={}, level={:?}",
                            compression.algorithm, compression.level
                        );
                        SstableWriter::with_compression(
                            &task.output_file,
                            block_size,
                            filesystem_factory,
                            Some(compression.clone()),
                        )
                    } else {
                        debug!("   No compression - using default writer");
                        SstableWriter::new(&task.output_file, block_size, filesystem_factory)
                    };
                    let record_count = btree_records.len();
                    let sorted_records_iter = btree_records.into_iter();
                    writer
                        .write_sorted_vector_records(sorted_records_iter, record_count)
                        .await
                        .map_err(|e| crate::core::StorageError::Serialization(e.to_string()))?;
                }
            }

            let output_path = task.output_file.to_string_lossy();
            let metadata = fs.metadata(&output_path).await.map_err(|e| {
                crate::core::StorageError::DiskIO(std::io::Error::other(e.to_string()))
            })?;
            metadata.size
        };

        debug!(
            "Wrote {} bytes to output file {}",
            bytes_written,
            task.output_file.display()
        );

        // NOTE: Manifest removed - using directory-based discovery instead
        // Files are automatically discovered by scanning collection directories
        debug!(
            "Compaction completed - files will be discovered automatically by directory scanning"
        );

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
            read_throughput_mb_sec,
            write_throughput_mb_sec,
            compression_ratio
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
            tracing::info!(
                "💡 DESIGN INSIGHT: LSM compaction ({}ms) much slower than VIPER flush target (<200ms) - async compaction recommended",
                total_time.as_millis()
            );
        }

        debug!(
            "🗜️ LSM compaction stats: {}MB read, {}MB written, {:.1}x compression, {} records merged, {} expired deleted, {} tombstones removed",
            bytes_read / 1024 / 1024,
            bytes_written / 1024 / 1024,
            compression_ratio,
            resolved_records_len,
            expired_records_count,
            tombstones_removed_count
        );

        Ok(EnhancedSstCompactionStats {
            base_stats: SstCompactionStats {
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

    /// Get SST files organized by level using unified framework
    #[allow(dead_code)]
    async fn get_sst_files_by_level(
        &self,
        collection_dir: &Path,
    ) -> Result<HashMap<u8, Vec<PathBuf>>> {
        debug!(
            "🔍 COMPACTION: Using unified framework to discover SST files in: {}",
            collection_dir.display()
        );

        let _collection_path = collection_dir.to_string_lossy();

        // Use new orchestrator if available, otherwise fall back to direct file discovery
        let unified_files = if let Some(ref orchestrator) = self.compaction_orchestrator {
            orchestrator
                .registry
                .discover_files(&orchestrator.filesystem, &_collection_path, "sst")
                .await
                .map_err(|e| crate::core::StorageError::SstEngine(e.to_string()))?
        } else {
            // Fallback to empty result if no orchestrator
            HashMap::new()
        };

        // Convert from unified format to legacy format for compatibility
        let mut files_by_level = HashMap::new();
        for (level, metadata_list) in unified_files {
            let paths: Vec<PathBuf> = metadata_list
                .into_iter()
                .map(|metadata| PathBuf::from(&metadata.path))
                .collect();

            if !paths.is_empty() {
                files_by_level.insert(level as u8, paths);
            }
        }

        debug!(
            "📊 COMPACTION: Found SST files by level using unified framework: {:?}",
            files_by_level
                .iter()
                .map(|(level, files)| (*level, files.len()))
                .collect::<Vec<_>>()
        );

        Ok(files_by_level)
    }

    // OPTIMIZED: Removed deprecated read_data_blocks_from_sstable method (legacy code elimination)
    /*
    async fn REMOVED_read_data_blocks_from_sstable(
        &self,
        file_data: &[u8],
        file_path: &str,
    ) -> std::result::Result<BTreeMap<String, VectorRecord>, crate::core::StorageError> {
        debug!("Parsing SSTable format for {} ({} bytes)", file_path, file_data.len());

        if file_data.len() < 16 {
            return Err(crate::core::StorageError::SstEngine(
                format!("SSTable file too small: {} bytes", file_data.len())
            ));
        }

        // SSTable format: [magic:4][header_len:4][header][bloom_len:4][bloom][index_len:4][index][data_blocks...]
        // Skip magic, header, bloom, and index to reach data blocks
        let mut offset = 0;

        // Skip magic header (SST1)
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstEngine("File too small for magic header".into()));
        }
        let magic = &file_data[offset..offset + 4];
        if magic != b"SST1" {
            return Err(crate::core::StorageError::SstEngine("Invalid SSTable magic header".into()));
        }
        offset += 4;

        // Skip header: [header_len:4][header_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstEngine("File too small for header length".into()));
        }
        let header_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + header_len;

        // Skip bloom filter: [bloom_len:4][bloom_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstEngine("File too small for bloom length".into()));
        }
        let bloom_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + bloom_len;

        // Skip index: [index_len:4][index_data]
        if offset + 4 > file_data.len() {
            return Err(crate::core::StorageError::SstEngine("File too small for index length".into()));
        }
        let index_len = u32::from_le_bytes([
            file_data[offset], file_data[offset + 1], file_data[offset + 2], file_data[offset + 3]
        ]) as usize;
        offset += 4 + index_len;

        debug!("Skipped header ({} bytes), bloom ({} bytes), index ({} bytes) - data blocks start at offset {}",
              header_len, bloom_len, index_len, offset);

        // Now we're at the data blocks section
        let data_blocks_bytes = &file_data[offset..];
        debug!("Reading {} bytes of data blocks from {}", data_blocks_bytes.len(), file_path);

        if data_blocks_bytes.is_none() {
            warn!("No data blocks found in SST file: {}", file_path);
            return Ok(BTreeMap::new());
        }

        // OPTIMIZED: Fast bulk record extraction for compaction
        // Since compaction needs ALL records, we use a streaming approach
        let mut all_records = BTreeMap::new();
        let mut blocks_processed = 0;
        let mut total_records = 0;

        let mut block_offset = 0;
        while block_offset < data_blocks_bytes.len() {
            // Read block size
            if block_offset + 4 > data_blocks_bytes.len() {
                debug!("Not enough bytes for block size header at offset {}", block_offset);
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
                warn!("Block extends beyond data: {} + {} > {}",
                      block_offset, block_size, data_blocks_bytes.len());
                break;
            }

            let block_data = &data_blocks_bytes[block_offset..block_offset + block_size];

            // Use standard DataBlock deserialization (bincode-based)
            match super::DataBlock::deserialize(block_data) {
                Ok(data_block) => {
                    let record_count = data_block.records.len();
                    total_records += record_count;

                    debug!("Block {} parsed successfully - {} records", blocks_processed, record_count);

                    // OPTIMIZED: DataBlock already contains VectorRecord, no conversion needed
                    for sst_record in data_block.records {
                        let vector_id = sst_record.id.clone();
                        all_records.insert(vector_id, sst_record);
                    }
                }
                Err(e) => {
                    warn!("Failed to parse block {} at offset {}: {}", blocks_processed, block_offset, e);
                }
            }

            block_offset += block_size;
            blocks_processed += 1;
        }

        info!("Extracted {} total records from {} blocks in {}", all_records.len(), blocks_processed, file_path);

        Ok(all_records)
    }
    */ // End of removed deprecated method

    /// Fast parsing of data block optimized for compaction bulk reads
    /// Avoids full DataBlock struct deserialization for better performance
    #[allow(dead_code)]
    fn fast_parse_data_block(
        &self,
        block_data: &[u8],
        block_id: usize,
    ) -> std::result::Result<BTreeMap<String, VectorRecord>, crate::core::StorageError> {
        // OPTIMIZED: Return VectorRecord directly
        use std::io::Read;

        let mut cursor = std::io::Cursor::new(block_data);
        let mut records = BTreeMap::new();

        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic).map_err(|e| {
            crate::core::StorageError::SstEngine(format!("Failed to read magic header: {}", e))
        })?;

        if &magic != b"BLK1" {
            return Err(crate::core::StorageError::SstEngine(format!(
                "Invalid DataBlock format - expected BLK1 magic, got {:?}",
                magic
            )));
        }

        // Read DataBlock header: [magic:4][block_id:4][uncompressed_size:4][record_count:4]
        let mut header = [0u8; 12];
        cursor.read_exact(&mut header).map_err(|e| {
            crate::core::StorageError::SstEngine(format!("Failed to read block header: {}", e))
        })?;

        let stored_block_id = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let _uncompressed_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let record_count = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);

        debug!(
            "Block header - id: {}, uncompressed_size: {}, record_count: {}",
            stored_block_id, _uncompressed_size, record_count
        );

        if stored_block_id as usize != block_id {
            warn!(
                "🔍 COMPACTION: Block ID mismatch: expected {}, got {}",
                block_id, stored_block_id
            );
        }

        // STREAMING PARSE: Read records one by one without creating intermediate DataBlock
        debug!(
            "Starting to parse {} records from block {}",
            record_count, block_id
        );
        for record_idx in 0..record_count {
            // Read record length
            let mut len_buf = [0u8; 4];
            if cursor.read_exact(&mut len_buf).is_err() {
                warn!(
                    "🔍 COMPACTION: Failed to read record {} length in block {}",
                    record_idx, block_id
                );
                break;
            }

            let record_len = u32::from_le_bytes(len_buf) as usize;
            if record_len == 0 || record_len > 1024 * 1024 {
                // Sanity check: max 1MB per record
                warn!(
                    "🔍 COMPACTION: Invalid record length {} for record {} in block {}",
                    record_len, record_idx, block_id
                );
                break;
            }

            // Read record data
            let mut record_data = vec![0u8; record_len];
            if cursor.read_exact(&mut record_data).is_err() {
                warn!(
                    "🔍 COMPACTION: Failed to read record {} data in block {}",
                    record_idx, block_id
                );
                break;
            }

            // OPTIMIZED: Use direct VectorRecord protobuf deserialization
            use prost::Message;
            match VectorRecord::decode(&record_data[..]) {
                Ok(record) => {
                    let record_id = record.id.clone();
                    debug!(
                        "Successfully deserialized record {} with id: {:?}",
                        record_idx, record.id
                    );
                    records.insert(record_id, record);
                }
                Err(e) => {
                    warn!(
                        "Failed to deserialize VectorRecord {} in block {}: {}",
                        record_idx, block_id, e
                    );
                    warn!(
                        "🔍 COMPACTION: Failed to deserialize VectorRecord {} in block {}: {}",
                        record_idx, block_id, e
                    );
                }
            }
        }

        debug!(
            "fast_parse_data_block completed - {} records extracted from block {}",
            records.len(),
            block_id
        );
        Ok(records)
    }

    /// Generate output file path for compacted file
    /// Uses unified FilenameCodec for consistency with compaction framework
    /// Respects the configured block_format setting for file extension
    fn generate_output_file_path(
        &self,
        _collection_id: &str,
        collection_dir: &Path,
        level: u8,
    ) -> PathBuf {
        // Use unified FilenameCodec directly from compaction framework
        use super::block_format::BlockFormat;
        use crate::storage::common::compaction_orchestrator::FilenameCodec;

        let codec = FilenameCodec::new();
        // Check block_format from config and use appropriate extension
        let block_format = BlockFormat::parse_block_format(&self.config.block_format);
        let extension = match block_format {
            BlockFormat::ArrowBlock => "arrow",
            BlockFormat::ProximaBlocks => "sst",
        };
        let filename = codec.generate(level as u32, extension);
        collection_dir.join(filename)
    }

    /// 🚀 NEW: Read all records from SSTable using unified reader with compaction optimizations
    async fn read_all_records_from_file_unified(
        &self,
        file_path: &str,
    ) -> Result<Vec<VectorRecord>> {
        info!(
            "🔥 COMPACTION UNIFIED: Reading {} with optimized strategy",
            file_path
        );

        // Use compaction-optimized reading strategy
        match self
            .unified_reader
            .read_all_records_for_compaction(&[file_path.to_string()])
            .await
        {
            Ok(records) => {
                info!(
                    "✅ COMPACTION UNIFIED: Successfully read {} records from {}",
                    records.len(),
                    file_path
                );

                // Debug: print sample records
                for (i, record) in records.iter().take(3).enumerate() {
                    debug!(
                        "  UNIFIED Record {}: id={:?}, vector_len={}, metadata_len={}",
                        i,
                        record.oid,
                        record
                            .embeddings
                            .first()
                            .map_or(0, |embedding| embedding.values.len()),
                        record.props.len()
                    );
                }

                Ok(records.into_iter().map(VectorRecord::from).collect())
            }
            Err(e) => {
                warn!("❌ COMPACTION UNIFIED: Failed to read {}: {}", file_path, e);
                Err(crate::core::StorageError::SstEngine(format!(
                    "Unified reader failed for {}: {}",
                    file_path, e
                )))
            }
        }
    }

    /// Sort vector records by metadata for optimal compaction encoding
    /// Uses same sorting strategy as flush operations to maintain consistency
    async fn sort_vectors_for_compaction(
        vector_records: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        debug!(
            "🔄 Sorting {} vectors for optimal SSTable compaction encoding",
            vector_records.len()
        );

        if vector_records.is_empty() {
            return Ok((vector_records, SortingStats::default()));
        }

        // Create metadata sorter for optimal SSTable encoding
        let sorter = MetadataSorter::new(Default::default());

        // Sort records to optimize for:
        // 1. Sequential access patterns in SSTable blocks
        // 2. Better compression ratios in block-based storage
        // 3. Improved bloom filter effectiveness
        let canonical_records: Vec<ProximaRecord> =
            vector_records.iter().map(ProximaRecord::from).collect();
        let (sorted_records, sort_stats) = sorter.sort_for_encoding(canonical_records)?;
        let sorted_records: Vec<VectorRecord> =
            sorted_records.into_iter().map(VectorRecord::from).collect();

        debug!(
            "✅ LSM compaction sorting complete: {} records sorted for optimal SSTable encoding",
            sorted_records.len()
        );

        Ok((sorted_records, sort_stats))
    }
}

impl Drop for Compaction {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::SeqCst);

        // Abort remaining worker handles
        for handle in &self.worker_handles {
            handle.abort();
        }
    }
}

// Test module for vector tracking during compaction
// Deferred: Missing test file - compaction_vector_tracking_tests.rs
// #[cfg(test)]
// #[path = "compaction_vector_tracking_tests.rs"]
// mod vector_tracking_tests;

#[cfg(test)]
#[cfg_attr(test, path = "compaction_tests.rs")]
mod tests;
