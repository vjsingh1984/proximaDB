//! Advanced Compaction Orchestrator Framework
//!
//! This module provides a robust, trait-based compaction orchestration system
//! with strong concurrency guarantees, deadlock prevention, and engine abstraction.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use once_cell::sync::OnceCell;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use crate::utils::uuid::Uuid;

use crate::storage::persistence::filesystem::FilesystemFactory;

/// Unique identifier for operations
pub type OperationId = String;
pub type CollectionId = String;

/// Queue status for compaction coordination
#[derive(Debug, Clone)]
pub enum QueueStatus {
    /// Queue is empty, safe to proceed with compaction
    Empty,
    /// Queue is draining, wait before compaction
    Draining {
        pending_acks: usize,
        estimated_drain_time: Duration,
    },
    /// Queue is active with pending items
    Active {
        queue_depth: usize,
        oldest_unacked: Instant,
    },
}

/// File metadata trait that engines can implement
pub trait FileMetadata: Send + Sync + Clone + std::fmt::Debug {
    fn path(&self) -> &str;
    fn size_bytes(&self) -> u64;
    fn level(&self) -> u32;
    fn timestamp(&self) -> u64;
    fn extension(&self) -> &str;
}

/// Compaction task trait for engine-specific tasks
pub trait CompactionTask: Send + Sync + Clone + std::fmt::Debug {
    fn operation_id(&self) -> &str;
    fn collection_id(&self) -> &str;
    fn source_level(&self) -> u32;
    fn target_level(&self) -> u32;
    fn input_files(&self) -> &[String];
    fn estimated_duration(&self) -> Duration;
}

/// Compaction result trait for engine-specific results
pub trait CompactionResult: Send + Sync + std::fmt::Debug {
    fn operation_id(&self) -> &str;
    fn files_created(&self) -> &[String];
    fn files_deleted(&self) -> &[String];
    fn bytes_written(&self) -> u64;
    fn records_processed(&self) -> u64;
    fn duration(&self) -> Duration;
}

/// Storage engine trait for compaction operations
pub trait StorageEngine: Send + Sync {
    type FileMetadata: FileMetadata;
    type CompactionTask: CompactionTask;
    type CompactionResult: CompactionResult;

    /// Get the file extension for this engine (e.g., "sst", "parquet")
    fn file_extension(&self) -> &str;

    /// Get compaction configuration for this engine
    fn compaction_config(&self) -> CompactionConfig;

    /// Create a compaction task from discovered files
    fn create_compaction_task(
        &self,
        operation_id: String,
        collection_id: String,
        source_level: u32,
        target_level: u32,
        input_files: Vec<Self::FileMetadata>,
    ) -> Self::CompactionTask;

    /// Execute the actual compaction operation
    async fn execute_compaction(
        &self,
        task: Self::CompactionTask,
    ) -> Result<Self::CompactionResult>;
}

/// Configuration for compaction behavior
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Threshold for Level 0 compaction (number of files)
    pub level0_threshold: usize,
    /// Threshold for higher level compaction (number of files)
    pub level_threshold: usize,
    /// Maximum supported level
    pub max_level: u32,
    /// Maximum concurrent compactions per collection
    pub max_concurrent_per_collection: usize,
    /// Global maximum concurrent compactions
    pub global_max_concurrent: usize,
    /// Compaction timeout duration
    pub operation_timeout: Duration,
    /// Enable queue-aware compaction (delays compaction until AXIS queue is drained)
    pub queue_aware_compaction: bool,
    /// Maximum time to wait for queue to drain before forcing compaction
    pub max_queue_wait: Duration,
    /// Compaction urgency threshold (0.0-1.0, above which compaction is forced)
    pub urgency_threshold: f64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            level0_threshold: 5,
            level_threshold: 10,
            max_level: 7,
            max_concurrent_per_collection: 1,
            global_max_concurrent: 4,
            operation_timeout: Duration::from_secs(3600), // 1 hour
            queue_aware_compaction: true,
            max_queue_wait: Duration::from_secs(300), // 5 minutes
            urgency_threshold: 0.8,
        }
    }
}

/// Type of operation being performed
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OperationType {
    Flush {
        level: u32,
    },
    Compaction {
        source_level: u32,
        target_level: u32,
    },
    Recovery,
    Maintenance,
}

impl OperationType {
    /// Check if this operation conflicts with another
    pub fn conflicts_with(&self, other: &OperationType) -> bool {
        match (self, other) {
            // Flushes conflict with any compaction on the same level
            (
                OperationType::Flush { level: l1 },
                OperationType::Compaction { source_level, .. },
            )
            | (
                OperationType::Compaction { source_level, .. },
                OperationType::Flush { level: l1 },
            ) => *l1 == *source_level,
            // Compactions conflict if they share levels
            (
                OperationType::Compaction {
                    source_level: s1,
                    target_level: t1,
                },
                OperationType::Compaction {
                    source_level: s2,
                    target_level: t2,
                },
            ) => s1 == s2 || s1 == t2 || t1 == s2 || t1 == t2,
            // Recovery and maintenance conflict with everything
            (OperationType::Recovery | OperationType::Maintenance, _)
            | (_, OperationType::Recovery | OperationType::Maintenance) => true,
            // Flushes to different levels don't conflict
            (OperationType::Flush { level: l1 }, OperationType::Flush { level: l2 }) => l1 == l2,
        }
    }

    /// Get priority for operation ordering (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            OperationType::Recovery => 0,
            OperationType::Flush { .. } => 1,
            OperationType::Compaction {
                source_level: 0, ..
            } => 2, // Level 0 compaction
            OperationType::Compaction { .. } => 3,
            OperationType::Maintenance => 4,
        }
    }
}

/// Active operation tracking
#[derive(Debug, Clone)]
pub struct ActiveOperation {
    pub operation_id: OperationId,
    pub operation_type: OperationType,
    pub collection_id: CollectionId,
    pub started_at: Instant,
    pub estimated_completion: Option<Instant>,
    pub dependency_chain: Vec<OperationId>,
}

/// RAII lock for operations - automatically releases on drop
pub struct OperationLock {
    operation_id: OperationId,
    collection_id: CollectionId,
    coordinator: Arc<CompactionCoordinator>,
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        if let Err(e) = self
            .coordinator
            .release_operation(&self.operation_id, &self.collection_id)
        {
            error!(
                "Failed to release operation lock {}: {}",
                self.operation_id, e
            );
        } else {
            debug!("Released operation lock: {}", self.operation_id);
        }
    }
}

/// Global coordination state
#[derive(Debug, Default)]
pub struct GlobalCompactionState {
    pub active_operations_count: usize,
    pub total_operations_completed: u64,
    pub total_operations_failed: u64,
    pub last_global_maintenance: Option<Instant>,
}

/// Tracks compactions that are deferred due to AXIS queue
#[derive(Debug, Clone)]
pub struct DeferredCompaction {
    pub operation_type: OperationType,
    pub deferred_at: Instant,
    pub urgency_score: f64,
    pub defer_count: usize,
    pub estimated_duration: Option<Duration>,
}

/// Concurrency coordinator for compaction operations
pub struct CompactionCoordinator {
    /// Active operations per collection
    active_operations: DashMap<CollectionId, Vec<ActiveOperation>>,
    /// Global state protected by RwLock
    global_state: RwLock<GlobalCompactionState>,
    /// Configuration
    pub config: CompactionConfig,
    /// Queue manager for AXIS integration (optional)
    // TODO: Restore when QueueManager is available
    // queue_manager: Option<Arc<QueueManager>>,
    /// Track deferred compactions due to queue
    deferred_compactions: DashMap<CollectionId, DeferredCompaction>,
}

impl CompactionCoordinator {
    pub fn new(config: CompactionConfig) -> Self {
        Self {
            active_operations: DashMap::new(),
            global_state: RwLock::new(GlobalCompactionState::default()),
            config,
            deferred_compactions: DashMap::new(),
        }
    }

    /// Create coordinator with queue manager for AXIS integration
    // TODO: Restore when QueueManager is available
    /* pub fn new_with_queue_manager(config: CompactionConfig, queue_manager: Arc<QueueManager>) -> Self {
        Self {
            active_operations: DashMap::new(),
            global_state: RwLock::new(GlobalCompactionState::default()),
            config,
            // queue_manager: Some(queue_manager),
            deferred_compactions: DashMap::new(),
        }
    } */

    /// Request permission to start an operation (internal method)
    async fn request_operation_internal(
        &self,
        collection_id: &str,
        operation_type: OperationType,
        estimated_duration: Option<Duration>,
    ) -> Result<String> {
        // Check if this is a compaction and if queue-aware mode is enabled
        if self.config.queue_aware_compaction {
            if let OperationType::Compaction { .. } = &operation_type {
                if let Some(decision) = self
                    .evaluate_queue_aware_compaction(collection_id, &operation_type)
                    .await?
                {
                    return Err(anyhow::anyhow!("{}", decision));
                }
            }
        }

        let operation_id = Uuid::new_v4().to_string();

        // Check global limits
        {
            let global_state = self.global_state.read().await;
            if global_state.active_operations_count >= self.config.global_max_concurrent {
                return Err(anyhow::anyhow!(
                    "Global compaction limit reached: {}/{}",
                    global_state.active_operations_count,
                    self.config.global_max_concurrent
                ));
            }
        }

        // Check collection-specific limits and conflicts
        let mut collection_ops = self
            .active_operations
            .entry(collection_id.to_string())
            .or_insert_with(Vec::new);

        // Check collection limit
        if collection_ops.len() >= self.config.max_concurrent_per_collection {
            return Err(anyhow::anyhow!(
                "Collection compaction limit reached for {}: {}/{}",
                collection_id,
                collection_ops.len(),
                self.config.max_concurrent_per_collection
            ));
        }

        // Check for conflicting operations
        for active_op in collection_ops.iter() {
            if operation_type.conflicts_with(&active_op.operation_type) {
                return Err(anyhow::anyhow!(
                    "Operation {:?} conflicts with active operation {:?} in collection {}",
                    operation_type,
                    active_op.operation_type,
                    collection_id
                ));
            }
        }

        // Create and register operation
        let estimated_completion = estimated_duration.map(|d| Instant::now() + d);
        let active_op = ActiveOperation {
            operation_id: operation_id.clone(),
            operation_type: operation_type.clone(),
            collection_id: collection_id.to_string(),
            started_at: Instant::now(),
            estimated_completion,
            dependency_chain: Vec::new(),
        };

        collection_ops.push(active_op);

        // Update global state
        {
            let mut global_state = self.global_state.write().await;
            global_state.active_operations_count += 1;
        }

        info!(
            "🔒 Acquired operation lock: {} for {:?} in collection {}",
            operation_id, operation_type, collection_id
        );

        Ok(operation_id)
    }

    /// Request permission to start an operation
    pub async fn request_operation(
        coordinator: Arc<Self>,
        collection_id: &str,
        operation_type: OperationType,
        estimated_duration: Option<Duration>,
    ) -> Result<OperationLock> {
        let operation_id = coordinator
            .request_operation_internal(collection_id, operation_type, estimated_duration)
            .await?;

        Ok(OperationLock {
            operation_id,
            collection_id: collection_id.to_string(),
            coordinator,
        })
    }

    /// Release an operation (called by OperationLock::drop)
    fn release_operation(&self, operation_id: &str, collection_id: &str) -> Result<()> {
        if let Some(mut collection_ops) = self.active_operations.get_mut(collection_id) {
            collection_ops.retain(|op| op.operation_id != operation_id);

            // Clean up empty collections
            if collection_ops.is_empty() {
                drop(collection_ops);
                self.active_operations.remove(collection_id);
            }
        }

        // Update global state (async context not available in Drop, so use try_write)
        if let Ok(mut global_state) = self.global_state.try_write() {
            global_state.active_operations_count =
                global_state.active_operations_count.saturating_sub(1);
        }

        Ok(())
    }

    /// Get current active operations for debugging
    pub fn get_active_operations(&self) -> HashMap<CollectionId, Vec<ActiveOperation>> {
        self.active_operations
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Evaluate if compaction should be deferred due to AXIS queue
    async fn evaluate_queue_aware_compaction(
        &self,
        collection_id: &str,
        operation_type: &OperationType,
    ) -> Result<Option<String>> {
        // TODO: Restore when QueueManager is available
        // let queue_manager = match &self.queue_manager {
        //     Some(qm) => qm,
        //     None => return Ok(None), // No queue manager, proceed normally
        // };
        return Ok(None); // Queue manager temporarily disabled

        // TODO: Restore when QueueManager is available
        /*
        // Get queue status for this collection
        let queue_status = queue_manager
            .get_collection_queue_status(collection_id)
            .await?;

        // Check if we have a deferred compaction for this collection
        let defer_info = self.deferred_compactions.get(key);
        let (defer_count, wait_time) = if let Some(deferred) = defer_info {
            (deferred.defer_count, deferred.deferred_at.elapsed())
        } else {
            (0, Duration::from_secs(0))
        };

        match queue_status {
            QueueStatus::Empty => {
                // Queue is empty, allow compaction and clear any deferred state
                self.deferred_compactions.remove(collection_id);
                Ok(None)
            }

            QueueStatus::Draining { pending_acks, estimated_drain_time } => {
                // Check if we've been waiting too long
                if wait_time >= self.config.max_queue_wait {
                    info!("Force compacting {} after waiting {:?} for queue to drain", collection_id, wait_time);
                    self.deferred_compactions.remove(collection_id);
                    return Ok(None); // Allow compaction
                }

                // Defer compaction
                self.defer_compaction(collection_id, operation_type.clone()).await;

                Ok(Some(format!(
                    "Deferring compaction for {} - queue draining ({} pending acks, est {:?} to drain)",
                    collection_id, pending_acks, estimated_drain_time
                )))
            }

            QueueStatus::Active { queue_depth, oldest_unacked } => {
                let queue_age = oldest_unacked.elapsed();

                // If queue is very active and we haven't waited too long, defer
                if queue_depth > 100 && wait_time < self.config.max_queue_wait {
                    self.defer_compaction(collection_id, operation_type.clone()).await;

                    Ok(Some(format!(
                        "Deferring compaction for {} - active queue (depth={}, age={:?})",
                        collection_id, queue_depth, queue_age
                    )))
                } else if wait_time >= self.config.max_queue_wait {
                    info!("Force compacting {} despite active queue after {:?} wait", collection_id, wait_time);
                    self.deferred_compactions.remove(collection_id);
                    Ok(None) // Allow compaction
                } else {
                    Ok(None) // Small queue, allow compaction
                }
            }
        }
        */
    }

    /// Defer a compaction operation
    async fn defer_compaction(&self, collection_id: &str, operation_type: OperationType) {
        let mut entry = self
            .deferred_compactions
            .entry(collection_id.to_string())
            .or_insert_with(|| DeferredCompaction {
                operation_type: operation_type.clone(),
                deferred_at: Instant::now(),
                urgency_score: 0.0,
                defer_count: 0,
                estimated_duration: None,
            });

        entry.defer_count += 1;
        debug!(
            "Deferred compaction for {} (count={}, type={:?})",
            collection_id, entry.defer_count, operation_type
        );
    }

    /// Process deferred compactions that can now run
    pub async fn process_deferred_compactions(&self) -> Result<Vec<String>> {
        // TODO: Restore when QueueManager is available
        // let queue_manager = match &self.queue_manager {
        //     Some(qm) => qm,
        //     None => return Ok(Vec::new()),
        // };
        return Ok(Vec::new()); // Queue manager temporarily disabled

        // TODO: Restore when QueueManager is available
        /*
        let mut processed = Vec::new();
        let mut ready_collections = Vec::new();

        // Find collections with empty queues
        for entry in self.deferred_compactions.iter() {
            let collection_id = entry.key();
            let queue_status = queue_manager
                .get_collection_queue_status(collection_id)
                .await?;

            if matches!(queue_status, QueueStatus::Empty) {
                ready_collections.push(collection_id.clone());
            }
        }

        // Process ready compactions
        for collection_id in ready_collections {
            if let Some((_, deferred)) = self.deferred_compactions.remove(&collection_id) {
                info!(
                    "Processing deferred compaction for {} after {:?} wait (deferred {} times)",
                    collection_id,
                    deferred.deferred_at.elapsed(),
                    deferred.defer_count
                );
                processed.push(collection_id);
            }
        }

        Ok(processed)
        */
    }

    /// Calculate compaction urgency score (0.0 - 1.0)
    pub fn calculate_urgency(file_count: usize, total_size_gb: f64, oldest_file_hours: f64) -> f64 {
        // Normalize factors to 0-1 range
        let file_factor = (file_count as f64 / 50.0).min(1.0);
        let size_factor = (total_size_gb / 100.0).min(1.0);
        let age_factor = (oldest_file_hours / 168.0).min(1.0); // 1 week = 168 hours

        // Weighted average
        (file_factor * 0.4 + size_factor * 0.3 + age_factor * 0.3).min(1.0)
    }
}

impl Clone for CompactionCoordinator {
    fn clone(&self) -> Self {
        Self {
            active_operations: self.active_operations.clone(),
            global_state: RwLock::new(GlobalCompactionState::default()), // Fresh state for clone
            config: self.config.clone(),
            deferred_compactions: self.deferred_compactions.clone(),
        }
    }
}

/// High-performance filename codec with caching
#[derive(Clone)]
pub struct FilenameCodec {
    level_pattern: OnceCell<Regex>,
    timestamp_pattern: OnceCell<Regex>,
    full_pattern: OnceCell<Regex>,
}

impl FilenameCodec {
    pub fn new() -> Self {
        Self {
            level_pattern: OnceCell::new(),
            timestamp_pattern: OnceCell::new(),
            full_pattern: OnceCell::new(),
        }
    }

    /// Generate optimized filename
    /// Format: L{level}_{timestamp}_{uuid}.{extension}
    /// Example: L0_20250814T143052_a7f3c2d1.sst
    pub fn generate(&self, level: u32, extension: &str) -> String {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%S");
        let uuid = Uuid::new_v4().to_string()[..8].to_string();
        format!("L{}_{}_{}.{}", level, timestamp, uuid, extension)
    }

    /// Parse level from filename with caching
    pub fn parse_level(&self, filename: &str) -> u32 {
        let pattern = self
            .level_pattern
            .get_or_init(|| Regex::new(r"^L(\d+)_").unwrap());

        pattern
            .captures(filename)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0)
    }

    /// Parse timestamp from filename with caching
    pub fn parse_timestamp(&self, filename: &str) -> u64 {
        let pattern = self
            .timestamp_pattern
            .get_or_init(|| Regex::new(r"L\d+_(\d{8}T\d{6})_").unwrap());

        pattern
            .captures(filename)
            .and_then(|caps| caps.get(1))
            .and_then(|m| {
                DateTime::parse_from_str(&format!("{}+00:00", m.as_str()), "%Y%m%dT%H%M%S%z")
                    .ok()
                    .map(|dt| dt.timestamp() as u64)
            })
            .unwrap_or(0)
    }

    /// Check if filename follows convention
    pub fn is_tiered_filename(&self, filename: &str, extension: &str) -> bool {
        let pattern = self
            .full_pattern
            .get_or_init(|| Regex::new(r"^L\d+_\d{8}T\d{6}_[a-f0-9]{8}\.\w+$").unwrap());

        pattern.is_match(filename) && filename.ends_with(&format!(".{}", extension))
    }
}

impl Default for FilenameCodec {
    fn default() -> Self {
        Self::new()
    }
}

/// Staging detector for atomic operations
#[derive(Clone)]
pub struct StagingDetector;

impl StagingDetector {
    /// Check if a file/directory is part of staging operations
    pub fn is_staging(&self, name: &str) -> bool {
        name.starts_with("__") || name.contains(".tmp") || name.contains(".staging")
    }

    /// Get staging prefix for atomic operations
    pub fn staging_prefix() -> &'static str {
        "__staging_"
    }
}

/// Tiered file registry for file discovery and metadata
#[derive(Clone)]
pub struct TieredFileRegistry {
    filename_codec: FilenameCodec,
    staging_detector: StagingDetector,
}

impl TieredFileRegistry {
    pub fn new() -> Self {
        Self {
            filename_codec: FilenameCodec::new(),
            staging_detector: StagingDetector,
        }
    }

    /// Discover files organized by level
    pub async fn discover_files(
        &self,
        filesystem: &Arc<FilesystemFactory>,
        data_directory: &str,
        extension: &str,
    ) -> Result<HashMap<u32, Vec<GenericFileMetadata>>> {
        let mut files_by_level: HashMap<u32, Vec<GenericFileMetadata>> = HashMap::new();

        let fs = filesystem.get_filesystem(data_directory)?;

        if !fs.exists(data_directory).await? {
            debug!("📁 Data directory does not exist: {}", data_directory);
            return Ok(files_by_level);
        }

        let entries = fs.list(data_directory).await?;
        debug!(
            "📋 Scanning {} entries in: {}",
            entries.len(),
            data_directory
        );

        for entry in entries {
            // Skip staging files/directories
            if self.staging_detector.is_staging(&entry.name) {
                debug!("⏭️  Skipping staging: {}", entry.name);
                continue;
            }

            // Process tiered files
            if !entry.metadata.is_directory
                && entry.name.ends_with(&format!(".{}", extension))
                && self
                    .filename_codec
                    .is_tiered_filename(&entry.name, extension)
            {
                let level = self.filename_codec.parse_level(&entry.name);
                let timestamp = self.filename_codec.parse_timestamp(&entry.name);

                debug!(
                    "✅ Discovered {} file: {} at level {}",
                    extension, entry.name, level
                );

                let metadata = GenericFileMetadata {
                    path: entry.url.clone(),
                    size_bytes: entry.metadata.size,
                    level,
                    timestamp,
                    extension: extension.to_string(),
                };

                files_by_level
                    .entry(level)
                    .or_insert_with(Vec::new)
                    .push(metadata);
            } else {
                debug!("❌ Skipping non-tiered file: {}", entry.name);
            }
        }

        // Sort files within each level by timestamp (oldest first)
        for files in files_by_level.values_mut() {
            files.sort_by_key(|f| f.timestamp());
        }

        info!(
            "🔍 Files by level: {:?}",
            files_by_level
                .iter()
                .map(|(k, v)| (k, v.len()))
                .collect::<Vec<_>>()
        );

        Ok(files_by_level)
    }
}

impl Default for TieredFileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Execution context for compaction operations
pub struct CompactionExecution<E: StorageEngine> {
    pub task: E::CompactionTask,
    pub metrics: CompactionMetrics,
    _operation_lock: OperationLock, // RAII cleanup
}

impl<E: StorageEngine> CompactionExecution<E> {
    /// Execute the compaction with automatic cleanup
    pub async fn execute(self, engine: &E) -> Result<E::CompactionResult> {
        let start_time = Instant::now();
        info!(
            "🚀 Starting compaction execution: {}",
            self.task.operation_id()
        );

        let result = engine
            .execute_compaction(self.task)
            .await
            .context("Compaction execution failed")?;

        let duration = start_time.elapsed();
        info!(
            "✅ Compaction completed in {:?}: {} files created, {} files deleted",
            duration,
            result.files_created().len(),
            result.files_deleted().len()
        );

        Ok(result)
    }
}

/// Metrics for compaction operations
#[derive(Debug, Clone, Default)]
pub struct CompactionMetrics {
    pub operation_id: String,
    pub collection_id: String,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub files_processed: usize,
    pub bytes_processed: u64,
    pub errors_encountered: usize,
}

/// Main compaction orchestrator
#[derive(Clone)]
pub struct CompactionOrchestrator {
    pub coordinator: Arc<CompactionCoordinator>,
    pub registry: TieredFileRegistry,
    pub filesystem: Arc<FilesystemFactory>,
}

impl CompactionOrchestrator {
    pub fn new(filesystem: Arc<FilesystemFactory>, config: CompactionConfig) -> Self {
        Self {
            coordinator: Arc::new(CompactionCoordinator::new(config)),
            registry: TieredFileRegistry::new(),
            filesystem,
        }
    }

    /// Schedule compaction for a storage engine
    pub async fn schedule_compaction<E: StorageEngine>(
        &self,
        engine: &E,
        collection_id: &str,
        data_directory: &str,
    ) -> Result<Option<CompactionExecution<E>>> {
        debug!(
            "🔍 Checking compaction needs for collection: {}",
            collection_id
        );

        // Discover files using registry
        let files_by_level = self
            .registry
            .discover_files(&self.filesystem, data_directory, engine.file_extension())
            .await?;

        // Check if compaction is needed using engine config
        let config = engine.compaction_config();

        // Check Level 0 first (highest priority)
        if let Some(level0_files) = files_by_level.get(&0) {
            if level0_files.len() >= config.level0_threshold {
                return self
                    .create_compaction_execution(
                        engine,
                        collection_id,
                        0,
                        1,
                        level0_files.clone(),
                        Duration::from_secs(300), // 5 minutes estimated
                    )
                    .await
                    .map(Some);
            }
        }

        // Check higher levels
        for level in 1..=config.max_level {
            if let Some(level_files) = files_by_level.get(&level) {
                if level_files.len() >= config.level_threshold {
                    // For higher levels, compact oldest file
                    let oldest_file = level_files.iter().min_by_key(|f| f.timestamp()).cloned();

                    if let Some(file) = oldest_file {
                        return self
                            .create_compaction_execution(
                                engine,
                                collection_id,
                                level,
                                level + 1,
                                vec![file],
                                Duration::from_secs(600), // 10 minutes estimated
                            )
                            .await
                            .map(Some);
                    }
                }
            }
        }

        debug!("📋 No compaction needed for collection: {}", collection_id);
        Ok(None)
    }

    /// Create compaction execution context
    async fn create_compaction_execution<E: StorageEngine>(
        &self,
        engine: &E,
        collection_id: &str,
        source_level: u32,
        target_level: u32,
        input_files: Vec<GenericFileMetadata>,
        estimated_duration: Duration,
    ) -> Result<CompactionExecution<E>> {
        let operation_type = OperationType::Compaction {
            source_level,
            target_level,
        };

        // Acquire operation lock
        let operation_lock = CompactionCoordinator::request_operation(
            self.coordinator.clone(),
            collection_id,
            operation_type,
            Some(estimated_duration),
        )
        .await?;

        let operation_id = operation_lock.operation_id.clone();

        // Create engine-specific task
        // Note: This is a simplified approach - in practice, engines would need
        // to convert GenericFileMetadata to their specific metadata types
        let converted_files: Vec<E::FileMetadata> = input_files
            .into_iter()
            .filter_map(|_f| {
                // For now, we'll use empty vec - engines will need to implement conversion
                None
            })
            .collect();

        let task = engine.create_compaction_task(
            operation_id.clone(),
            collection_id.to_string(),
            source_level,
            target_level,
            converted_files,
        );

        let metrics = CompactionMetrics {
            operation_id,
            collection_id: collection_id.to_string(),
            started_at: Some(Instant::now()),
            ..Default::default()
        };

        Ok(CompactionExecution {
            task,
            metrics,
            _operation_lock: operation_lock,
        })
    }

    /// Get active operations for monitoring
    pub fn get_active_operations(&self) -> HashMap<CollectionId, Vec<ActiveOperation>> {
        self.coordinator.get_active_operations()
    }
}

/// Generic file metadata implementation for testing and fallback
#[derive(Debug, Clone)]
pub struct GenericFileMetadata {
    pub path: String,
    pub size_bytes: u64,
    pub level: u32,
    pub timestamp: u64,
    pub extension: String,
}

impl FileMetadata for GenericFileMetadata {
    fn path(&self) -> &str {
        &self.path
    }
    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    fn level(&self) -> u32 {
        self.level
    }
    fn timestamp(&self) -> u64 {
        self.timestamp
    }
    fn extension(&self) -> &str {
        &self.extension
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_operation_conflicts() {
        let flush_l0 = OperationType::Flush { level: 0 };
        let flush_l1 = OperationType::Flush { level: 1 };
        let compact_l0_l1 = OperationType::Compaction {
            source_level: 0,
            target_level: 1,
        };
        let compact_l1_l2 = OperationType::Compaction {
            source_level: 1,
            target_level: 2,
        };

        // Flush conflicts with compaction on same level
        assert!(flush_l0.conflicts_with(&compact_l0_l1));
        assert!(compact_l0_l1.conflicts_with(&flush_l0));

        // Flushes to different levels don't conflict
        assert!(!flush_l0.conflicts_with(&flush_l1));

        // Compactions conflict if they share levels
        assert!(compact_l0_l1.conflicts_with(&compact_l1_l2)); // Share level 1

        // Recovery conflicts with everything
        let recovery = OperationType::Recovery;
        assert!(recovery.conflicts_with(&flush_l0));
        assert!(recovery.conflicts_with(&compact_l0_l1));
    }

    #[tokio::test]
    async fn test_coordinator_limits() {
        let config = CompactionConfig {
            max_concurrent_per_collection: 1,
            global_max_concurrent: 2,
            ..Default::default()
        };
        let coordinator = Arc::new(CompactionCoordinator::new(config));

        // First operation should succeed
        let _lock1 = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "collection1",
            OperationType::Flush { level: 0 },
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("First operation should succeed");

        // Second operation on same collection should fail
        let result = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "collection1",
            OperationType::Flush { level: 1 },
            Some(Duration::from_secs(10)),
        )
        .await;
        assert!(result.is_err());

        // Operation on different collection should succeed
        let _lock2 = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "collection2",
            OperationType::Flush { level: 0 },
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("Different collection should succeed");

        // Third operation should fail due to global limit
        let result = CompactionCoordinator::request_operation(
            coordinator.clone(),
            "collection3",
            OperationType::Flush { level: 0 },
            Some(Duration::from_secs(10)),
        )
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_filename_codec() {
        let codec = FilenameCodec::new();

        // Test generation
        let filename = codec.generate(5, "sst");
        assert!(filename.starts_with("L5_"));
        assert!(filename.ends_with(".sstable"));

        // Test parsing
        let test_filename = "L3_20250814T143052_a7f3c2d1.parquet";
        assert_eq!(codec.parse_level(test_filename), 3);
        assert!(codec.is_tiered_filename(test_filename, "parquet"));

        // Test invalid filename
        let invalid = "invalid_filename.sstable";
        assert_eq!(codec.parse_level(invalid), 0);
        assert!(!codec.is_tiered_filename(invalid, "sst"));
    }

    #[test]
    fn test_staging_detector() {
        let detector = StagingDetector;

        assert!(detector.is_staging("__staging_file"));
        assert!(detector.is_staging("file.tmp"));
        assert!(detector.is_staging("file.staging"));
        assert!(!detector.is_staging("normal_file.sstable"));
    }
}
