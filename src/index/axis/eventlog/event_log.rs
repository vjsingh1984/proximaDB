/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Filesystem-based persistent metadata queue for AXIS
//! Uses atomic coordination to ensure cloud storage compatibility

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::transaction_coordinator::TransactionCoordinator;

/// Index event for async processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEvent {
    /// Unique event ID
    pub event_id: String,
    
    /// Collection ID
    pub collection_id: String,
    
    /// Flushed/compacted file paths
    pub file_paths: Vec<String>,
    
    /// Number of vectors
    pub vector_count: usize,
    
    /// Storage engine type
    pub storage_engine: StorageEngineType,
    
    /// Whether files contain quantized vectors
    pub has_quantized: bool,
    
    /// Whether files contain FP32 vectors
    pub has_fp32: bool,
    
    /// Event timestamp
    pub timestamp: u64,
    
    /// Operation type
    pub operation: OperationType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEngineType {
    SST,
    VIPER,
    NOVA,
    RAPTOR,
    SWIFT,
    PRISM,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    Flush,
    Compaction,
    Delete,
}

/// Alias for backward compatibility with EventLog consumer
pub type EventType = OperationType;

/// File indexing status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndexingStatus {
    /// File path
    pub file_path: String,
    
    /// Indexes that need to process this file
    pub pending_indexes: Vec<String>,
    
    /// Indexes that have processed this file
    pub completed_indexes: Vec<String>,
    
    /// Whether file can be compacted
    pub ready_for_compaction: bool,
    
    /// Creation timestamp
    pub timestamp: u64,
}

/// Queue state for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueState {
    /// All events in order
    events: Vec<IndexEvent>,
    
    /// File status tracking
    file_status: Vec<FileIndexingStatus>,
    
    /// Last processed offset per index
    processed_offsets: std::collections::HashMap<String, usize>,
    
    /// Queue version for compatibility
    version: u32,
}

/// Filesystem-based event log queue
pub struct EventLogQueue {
    /// Base URL for queue storage (e.g., s3://bucket/queue/ or file:///data/queue/)
    base_url: String,
    
    /// Collection ID
    collection_id: String,
    
    /// Filesystem for cloud-compatible storage
    filesystem: Arc<dyn FileSystem>,
    
    /// Transaction coordinator for safe writes
    transaction_coordinator: Arc<TransactionCoordinator>,
    
    /// In-memory active events
    active_events: Arc<RwLock<VecDeque<IndexEvent>>>,
    
    /// File indexing status
    file_status: Arc<DashMap<String, FileIndexingStatus>>,
    
    /// Processed offsets per index
    processed_offsets: Arc<DashMap<String, usize>>,
    
    /// Current queue version
    version: u32,
}

impl EventLogQueue {
    /// Create new filesystem-based queue
    pub async fn new(
        base_url: String,
        collection_id: String,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        // Create filesystem for the base URL
        let filesystem_ref = filesystem_factory
            .get_filesystem(&base_url)
            .context("Failed to create filesystem")?;
        // Note: This assumes the filesystem factory returns an owned filesystem
        // If it returns a reference, we need to handle this differently
        let filesystem = todo!("Handle filesystem conversion from &dyn to Arc<dyn>");
        
        // Create transaction coordinator for this collection
        let transaction_coordinator = Arc::new(TransactionCoordinator::new(
            filesystem_factory.clone(),
            Some(format!("{}/queue/{}/temp", base_url, collection_id)),
        ).await?);
        
        let queue = Self {
            base_url: base_url.clone(),
            collection_id: collection_id.clone(),
            filesystem,
            transaction_coordinator,
            active_events: Arc::new(RwLock::new(VecDeque::new())),
            file_status: Arc::new(DashMap::new()),
            processed_offsets: Arc::new(DashMap::new()),
            version: 1,
        };
        
        // Try to recover existing state
        queue.recover().await?;
        
        Ok(queue)
    }
    
    /// Get queue state file path
    fn queue_state_path(&self) -> String {
        format!("{}/queue/{}/state.json", self.base_url, self.collection_id)
    }
    
    /// Get event log directory path
    fn event_log_dir(&self) -> String {
        format!("{}/queue/{}/events/", self.base_url, self.collection_id)
    }
    
    /// Add event to queue (fire-and-forget for producers)
    pub fn add_event(&self, event: IndexEvent) {
        // Add to in-memory queue immediately (non-blocking)
        self.active_events.blocking_write().push_back(event.clone());
        
        // Track file status
        for file_path in &event.file_paths {
            self.file_status.insert(
                file_path.clone(),
                FileIndexingStatus {
                    file_path: file_path.clone(),
                    pending_indexes: self.get_active_indexes(),
                    completed_indexes: Vec::new(),
                    ready_for_compaction: false,
                    timestamp: current_timestamp(),
                },
            );
        }
        
        // Persist asynchronously (fire-and-forget)
        let queue = self.clone_refs();
        tokio::spawn(async move {
            if let Err(e) = queue.persist_state().await {
                warn!("Failed to persist queue state: {}", e);
                // Not critical - can be recovered from storage scan
            }
        });
    }
    
    /// Get pending events for processing
    pub async fn get_pending_events(&self) -> Vec<IndexEvent> {
        self.active_events.read().await.iter().cloned().collect()
    }
    
    /// Mark event as processed by an index
    pub fn mark_processed(&self, event_id: &str, index_name: &str) {
        // Update processed offset
        if let Some(offset) = self.find_event_offset(event_id) {
            self.processed_offsets.insert(index_name.to_string(), offset);
        }
        
        // Update file status for all files in the event
        if let Some(event) = self.find_event(event_id) {
            for file_path in &event.file_paths {
                if let Some(mut status) = self.file_status.get_mut(file_path) {
                    // Move from pending to completed
                    status.pending_indexes.retain(|i| i != index_name);
                    if !status.completed_indexes.contains(&index_name.to_string()) {
                        status.completed_indexes.push(index_name.to_string());
                    }
                    
                    // Check if all indexes are done
                    if status.pending_indexes.is_empty() {
                        status.ready_for_compaction = true;
                        info!("File {} ready for compaction_info", file_path);
                    }
                }
            }
        }
        
        // Persist state asynchronously
        let queue = self.clone_refs();
        tokio::spawn(async move {
            let _ = queue.persist_state().await;
        });
    }
    
    /// Get file status for a specific file
    pub fn get_file_status(&self, file_path: &str) -> Option<FileIndexingStatus> {
        self.file_status.get(file_path).map(|s| s.clone())
    }

    /// Check if file can be compacted
    pub fn can_compact(&self, file_path: &str) -> bool {
        self.file_status
            .get(&self.collection_id)
            .map(|s| s.ready_for_compaction)
            .unwrap_or(true) // If not tracked, allow compaction
    }
    
    /// Clean up after compaction
    pub fn cleanup_compacted_files(&self, deleted_files: Vec<String>) {
        for file in &deleted_files {
            self.file_status.remove(file);
        }
        
        // Remove events for deleted files
        self.active_events.blocking_write().retain(|e| {
            !e.file_paths.iter().any(|f| deleted_files.contains(f))
        });
        
        // Persist changes
        let queue = self.clone_refs();
        tokio::spawn(async move {
            let _ = queue.persist_state().await;
        });
    }
    
    /// Persist queue state to filesystem
    async fn persist_state(&self) -> Result<()> {
        let state = QueueState {
            events: self.active_events.read().await.iter().cloned().collect(),
            file_status: self.file_status.iter()
                .map(|e| e.value().clone())
                .collect(),
            processed_offsets: self.processed_offsets.iter()
                .map(|e| (e.key().clone(), *e.value()))
                .collect(),
            version: self.version,
        };
        
        let json = serde_json::to_vec(&state)?;
        let state_path = self.queue_state_path();
        
        // Use transaction coordinator for safe cloud writes
        let operation_id = uuid::Uuid::new_v4().to_string();
        self.transaction_coordinator
            .write_to_staging(&operation_id, "queue_state.json", &json)
            .await
            .context("Failed to write queue state to staging")?;
        
        self.transaction_coordinator
            .commit_transaction(&operation_id)
            .await
            .context("Failed to commit queue state")?;
        
        debug!("Persisted queue state to {}", state_path);
        Ok(())
    }
    
    /// Recover queue state from filesystem
    async fn recover(&self) -> Result<()> {
        let state_path = self.queue_state_path();
        
        // Check if state file exists
        if !self.filesystem.exists(&state_path).await? {
            info!("No existing queue state found, starting fresh");
            return Ok(());
        }
        
        // Read state file
        let data = self.filesystem.read(&state_path).await?;
        let state: QueueState = serde_json::from_slice(&data)?;
        
        // Restore in-memory structures
        *self.active_events.write().await = state.events.into_iter().collect();
        
        for status in state.file_status {
            self.file_status.insert(status.file_path.clone(), status);
        }
        
        for (index, offset) in state.processed_offsets {
            self.processed_offsets.insert(index, offset);
        }
        
        info!(
            "Recovered queue state: {} events, {} files tracked",
            self.active_events.read().await.len(),
            self.file_status.len()
        );
        
        Ok(())
    }
    
    /// Get list of active indexes from collection config
    fn get_active_indexes(&self) -> Vec<String> {
        // TODO: Get from collection config
        vec!["hnsw".to_string(), "ivf".to_string()]
    }
    
    /// Find event by ID
    fn find_event(&self, event_id: &str) -> Option<IndexEvent> {
        self.active_events
            .blocking_read()
            .iter()
            .find(|e| e.event_id == event_id)
            .cloned()
    }
    
    /// Find event offset by ID
    fn find_event_offset(&self, event_id: &str) -> Option<usize> {
        self.active_events
            .blocking_read()
            .iter()
            .position(|e| e.event_id == event_id)
    }
    
    /// Clone references for async tasks
    fn clone_refs(&self) -> EventLogQueue {
        EventLogQueue {
            base_url: self.base_url.clone(),
            collection_id: self.collection_id.clone(),
            filesystem: self.filesystem.clone(),
            transaction_coordinator: self.transaction_coordinator.clone(),
            active_events: self.active_events.clone(),
            file_status: self.file_status.clone(),
            processed_offsets: self.processed_offsets.clone(),
            version: self.version,
        }
    }
    
    /// Generate smart extraction hints for AXIS
    pub fn get_extraction_hints(&self, event: &IndexEvent, index_type: &str) -> ExtractionMode {
        match (index_type, event.has_fp32, event.has_quantized) {
            // HNSW prefers FP32 for accuracy
            ("hnsw", true, _) => ExtractionMode::Fp32Only,
            
            // IVF can work with quantized for efficiency
            ("ivf", _, true) => ExtractionMode::QuantizedOnly,
            
            // PQ benefits from both
            ("pq", true, true) => ExtractionMode::Both,
            
            // LSH is flexible
            ("lsh", true, false) => ExtractionMode::Fp32Only,
            ("lsh", false, true) => ExtractionMode::QuantizedOnly,
            
            // Default: use what's available
            (_, true, false) => ExtractionMode::Fp32Only,
            (_, false, true) => ExtractionMode::QuantizedOnly,
            (_, true, true) => ExtractionMode::Auto,
            _ => ExtractionMode::Auto,
        }
    }
}

/// Vector extraction mode for AXIS
#[derive(Debug, Clone)]
pub enum ExtractionMode {
    Fp32Only,
    QuantizedOnly,
    Both,
    Auto,
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Helper to create index events
pub struct IndexEventBuilder;

impl IndexEventBuilder {
    /// Create flush event
    pub fn flush_event(
        collection_id: String,
        file_paths: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
        has_quantized: bool,
        has_fp32: bool,
    ) -> IndexEvent {
        IndexEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            collection_id,
            file_paths,
            vector_count,
            storage_engine,
            has_quantized,
            has_fp32,
            timestamp: current_timestamp(),
            operation: OperationType::Flush,
        }
    }
    
    /// Create compaction event
    pub fn compaction_event(
        collection_id: String,
        output_files: Vec<String>,
        vector_count: usize,
        storage_engine: StorageEngineType,
    ) -> IndexEvent {
        IndexEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            collection_id,
            file_paths: output_files,
            vector_count,
            storage_engine,
            has_quantized: true, // Compacted files typically have both
            has_fp32: true,
            timestamp: current_timestamp(),
            operation: OperationType::Compaction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    async fn create_test_queue() -> (EventLogQueue, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let base_url = format!("file://{}", temp_dir.path().display());
        
        let mut config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        config.default_fs = Some(base_url.clone());
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(config).await.unwrap()
        );
        
        let queue = EventLogQueue::new(
            base_url,
            "test_collection".to_string(),
            filesystem_factory,
        ).await.unwrap();
        
        (queue, temp_dir)
    }
    
    #[tokio::test]
    async fn test_add_and_retrieve_events() {
        let (queue, _dir) = create_test_queue().await;
        
        // Add flush event
        let event = IndexEventBuilder::flush_event(
            "test_collection".to_string(),
            vec!["file1.sstable".to_string(), "file2.sstable".to_string()],
            1000,
            StorageEngineType::SST,
            false,
            true,
        );
        
        queue.add_event(event.clone());
        
        // Retrieve events
        let events = queue.get_pending_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, event.event_id);
        assert_eq!(events[0].vector_count, 1000);
    }
    
    #[tokio::test]
    async fn test_mark_processed_and_compaction_ready() {
        let (queue, _dir) = create_test_queue().await;
        
        // Add event
        let event = IndexEventBuilder::flush_event(
            "test_collection".to_string(),
            vec!["file1.sstable".to_string()],
            500,
            StorageEngineType::SST,
            true,
            true,
        );
        
        queue.add_event(event.clone());
        
        // Initially not ready for compaction
        assert!(!queue.can_compact("file1.sstable"));
        
        // Mark as processed by indexes
        queue.mark_processed(&event.event_id, "hnsw");
        assert!(!queue.can_compact("file1.sstable")); // Still one pending
        
        queue.mark_processed(&event.event_id, "ivf");
        assert!(queue.can_compact("file1.sstable")); // Now ready
    }
    
    #[tokio::test]
    async fn test_cleanup_after_compaction() {
        let (queue, _dir) = create_test_queue().await;
        
        // Add multiple events
        let event1 = IndexEventBuilder::flush_event(
            "test_collection".to_string(),
            vec!["file1.sstable".to_string()],
            100,
            StorageEngineType::SST,
            false,
            true,
        );
        
        let event2 = IndexEventBuilder::flush_event(
            "test_collection".to_string(),
            vec!["file2.sstable".to_string()],
            200,
            StorageEngineType::SST,
            false,
            true,
        );
        
        queue.add_event(event1);
        queue.add_event(event2);
        
        assert_eq!(queue.get_pending_events().await.len(), 2);
        
        // Clean up file1 after compaction
        queue.cleanup_compacted_files(vec!["file1.sstable".to_string()]);
        
        // Should have only event2 remaining
        let events = queue.get_pending_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].file_paths[0], "file2.sstable");
    }
    
    #[tokio::test]
    async fn test_persistence_and_recovery() {
        let temp_dir = TempDir::new().unwrap();
        let base_url = format!("file://{}", temp_dir.path().display());
        
        let mut config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        config.default_fs = Some(base_url.clone());
        
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(config.clone()).await.unwrap()
        );
        
        // Create queue and add events
        {
            let queue = EventLogQueue::new(
                base_url.clone(),
                "test_collection".to_string(),
                filesystem_factory.clone(),
            ).await.unwrap();
            
            let event = IndexEventBuilder::flush_event(
                "test_collection".to_string(),
                vec!["file1.sstable".to_string()],
                1000,
                StorageEngineType::SST,
                true,
                false,
            );
            
            queue.add_event(event.clone());
            queue.mark_processed(&event.event_id, "hnsw");
            
            // Force persist
            queue.persist_state().await.unwrap();
        }
        
        // Create new queue and verify recovery
        {
            let queue = EventLogQueue::new(
                base_url,
                "test_collection".to_string(),
                filesystem_factory,
            ).await.unwrap();
            
            let events = queue.get_pending_events().await;
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].vector_count, 1000);
            
            // Check processed offset was recovered
            assert!(queue.processed_offsets.contains_key("hnsw"));
        }
    }
    
    #[tokio::test]
    async fn test_extraction_hints() {
        let (queue, _dir) = create_test_queue().await;
        
        // Test different scenarios
        let event_fp32 = IndexEventBuilder::flush_event(
            "test".to_string(),
            vec!["file.sstable".to_string()],
            100,
            StorageEngineType::SST,
            false,
            true,
        );
        
        let event_quantized = IndexEventBuilder::flush_event(
            "test".to_string(),
            vec!["file.sstable".to_string()],
            100,
            StorageEngineType::SST,
            true,
            false,
        );
        
        let event_both = IndexEventBuilder::flush_event(
            "test".to_string(),
            vec!["file.sstable".to_string()],
            100,
            StorageEngineType::SST,
            true,
            true,
        );
        
        // HNSW prefers FP32
        assert!(matches!(
            queue.get_extraction_hints(&event_fp32, "hnsw"),
            ExtractionMode::Fp32Only
        ));
        
        // IVF prefers quantized when available
        assert!(matches!(
            queue.get_extraction_hints(&event_quantized, "ivf"),
            ExtractionMode::QuantizedOnly
        ));
        
        // PQ uses both when available
        assert!(matches!(
            queue.get_extraction_hints(&event_both, "pq"),
            ExtractionMode::Both
        ));
    }
}