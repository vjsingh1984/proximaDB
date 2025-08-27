//! Batch Sync Coordinator for WAL Durability
//!
//! This module implements batch synchronization logic based on the DurabilityLevel
//! configuration. It provides configurable durability guarantees while optimizing
//! for performance through batching.

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::storage::persistence::write_ahead_log::config::DurabilityLevel;
use crate::storage::persistence::write_ahead_log::WriteBufferDiskManager;

/// Batch sync coordinator for managing WAL durability
pub struct BatchSyncCoordinator {
    /// Durability level configuration
    durability_level: DurabilityLevel,
    
    /// Disk manager for sync operations
    disk_manager: Arc<WriteBufferDiskManager>,
    
    /// Pending sync requests
    pending_syncs: Arc<Mutex<Vec<PendingSyncRequest>>>,
    
    /// Statistics
    stats: Arc<RwLock<BatchSyncStats>>,
    
    /// Shutdown signal
    shutdown: Arc<tokio::sync::Notify>,
}

/// Pending sync request
#[derive(Debug, Clone)]
struct PendingSyncRequest {
    /// Collection ID
    collection_id: String,
    
    /// File path to sync
    file_path: String,
    
    /// Request timestamp
    requested_at: Instant,
}

/// Batch sync statistics
#[derive(Debug, Default, Clone)]
pub struct BatchSyncStats {
    /// Total sync operations performed
    pub total_syncs: u64,
    
    /// Total batch syncs performed
    pub batch_syncs: u64,
    
    /// Average batch size
    pub avg_batch_size: f64,
    
    /// Total sync duration (microseconds)
    pub total_sync_duration_us: u64,
    
    /// Failed sync attempts
    pub failed_syncs: u64,
}

impl BatchSyncCoordinator {
    /// Create new batch sync coordinator
    pub fn new(
        durability_level: DurabilityLevel,
        disk_manager: Arc<WriteBufferDiskManager>,
    ) -> Self {
        Self {
            durability_level,
            disk_manager,
            pending_syncs: Arc::new(Mutex::new(Vec::new())),
            stats: Arc::new(RwLock::new(BatchSyncStats::default())),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }
    
    /// Start the batch sync coordinator
    pub async fn start(&self) -> Result<()> {
        match &self.durability_level {
            DurabilityLevel::BatchSync { batch_size, interval_secs } => {
                info!(
                    "🔄 Starting batch sync coordinator: batch_size={}, interval={}s",
                    batch_size, interval_secs
                );
                
                let batch_size = *batch_size;
                let interval_duration = Duration::from_secs(*interval_secs);
                
                // Start background sync task
                let pending_syncs = self.pending_syncs.clone();
                let disk_manager = self.disk_manager.clone();
                let stats = self.stats.clone();
                let shutdown = self.shutdown.clone();
                
                tokio::spawn(async move {
                    let mut ticker = interval(interval_duration);
                    
                    loop {
                        tokio::select! {
                            _ = ticker.tick() => {
                                // Time-based sync
                                Self::process_pending_syncs(
                                    &pending_syncs,
                                    &disk_manager,
                                    &stats,
                                    None,
                                ).await;
                            }
                            _ = shutdown.notified() => {
                                debug!("Batch sync coordinator shutting down");
                                // Final sync before shutdown
                                Self::process_pending_syncs(
                                    &pending_syncs,
                                    &disk_manager,
                                    &stats,
                                    None,
                                ).await;
                                break;
                            }
                        }
                        
                        // Check if we have enough pending syncs for batch
                        let pending_count = {
                            let pending = pending_syncs.lock().await;
                            pending.len()
                        };
                        
                        if pending_count >= batch_size {
                            // Batch size reached - sync immediately
                            Self::process_pending_syncs(
                                &pending_syncs,
                                &disk_manager,
                                &stats,
                                Some(batch_size),
                            ).await;
                        }
                    }
                });
                
                Ok(())
            }
            _ => {
                // Other durability levels don't need a coordinator
                Ok(())
            }
        }
    }
    
    /// Request a sync for a file
    pub async fn request_sync(&self, collection_id: String, file_path: String) -> Result<()> {
        match &self.durability_level {
            DurabilityLevel::NoSync => {
                // No sync needed
                Ok(())
            }
            DurabilityLevel::SyncData | DurabilityLevel::SyncFull => {
                // Immediate sync
                self.sync_file(&file_path).await
            }
            DurabilityLevel::BatchSync { batch_size, .. } => {
                // Add to pending queue
                let mut pending = self.pending_syncs.lock().await;
                pending.push(PendingSyncRequest {
                    collection_id,
                    file_path,
                    requested_at: Instant::now(),
                });
                
                // Check if batch is full
                if pending.len() >= *batch_size {
                    drop(pending); // Release lock before processing
                    Self::process_pending_syncs(
                        &self.pending_syncs,
                        &self.disk_manager,
                        &self.stats,
                        Some(*batch_size),
                    ).await;
                }
                
                Ok(())
            }
        }
    }
    
    /// Process pending sync requests
    async fn process_pending_syncs(
        pending_syncs: &Arc<Mutex<Vec<PendingSyncRequest>>>,
        disk_manager: &Arc<WriteBufferDiskManager>,
        stats: &Arc<RwLock<BatchSyncStats>>,
        batch_limit: Option<usize>,
    ) {
        let start_time = Instant::now();
        
        // Extract pending requests
        let requests = {
            let mut pending = pending_syncs.lock().await;
            if pending.is_empty() {
                return;
            }
            
            match batch_limit {
                Some(limit) => {
                    let drain_count = pending.len().min(limit);
                    pending.drain(..drain_count).collect::<Vec<_>>()
                }
                None => {
                    // Process all pending
                    pending.drain(..).collect::<Vec<_>>()
                }
            }
        };
        
        let batch_size = requests.len();
        debug!("Processing batch sync for {} files", batch_size);
        
        // Sync each file
        let mut success_count = 0;
        let mut failed_count = 0;
        
        for request in requests {
            if let Ok(filesystem) = disk_manager.filesystem_factory().get_filesystem(&request.file_path) {
                match filesystem.sync_file(&request.file_path).await {
                    Ok(_) => success_count += 1,
                    Err(e) => {
                        warn!("Failed to sync file {}: {}", request.file_path, e);
                        failed_count += 1;
                    }
                }
            }
        }
        
        // Update statistics
        let duration_us = start_time.elapsed().as_micros() as u64;
        {
            let mut stats = stats.write().await;
            stats.total_syncs += success_count;
            stats.failed_syncs += failed_count;
            stats.batch_syncs += 1;
            stats.total_sync_duration_us += duration_us;
            
            // Update average batch size
            let total_batches = stats.batch_syncs as f64;
            stats.avg_batch_size = (stats.avg_batch_size * (total_batches - 1.0) + batch_size as f64) / total_batches;
        }
        
        if success_count > 0 {
            debug!(
                "Batch sync completed: {} succeeded, {} failed in {}μs",
                success_count, failed_count, duration_us
            );
        }
    }
    
    /// Sync a single file immediately
    async fn sync_file(&self, file_path: &str) -> Result<()> {
        let start_time = Instant::now();
        
        let filesystem = self.disk_manager.filesystem_factory().get_filesystem(file_path)?;
        
        // Perform sync based on durability level
        match &self.durability_level {
            DurabilityLevel::SyncData => {
                // For now, we use sync_file which does fsync
                // TODO: Implement fdatasync for metadata-only sync
                filesystem.sync_file(file_path).await?;
            }
            DurabilityLevel::SyncFull => {
                // Full fsync
                filesystem.sync_file(file_path).await?;
            }
            _ => {
                // Should not reach here
                return Ok(());
            }
        }
        
        // Update statistics
        let duration_us = start_time.elapsed().as_micros() as u64;
        {
            let mut stats = self.stats.write().await;
            stats.total_syncs += 1;
            stats.total_sync_duration_us += duration_us;
        }
        
        debug!("File sync completed in {}μs: {}", duration_us, file_path);
        Ok(())
    }
    
    /// Shutdown the coordinator
    pub async fn shutdown(&self) {
        info!("Shutting down batch sync coordinator");
        self.shutdown.notify_one();
    }
    
    /// Get statistics
    pub async fn get_stats(&self) -> BatchSyncStats {
        let stats = self.stats.read().await;
        BatchSyncStats {
            total_syncs: stats.total_syncs,
            batch_syncs: stats.batch_syncs,
            avg_batch_size: stats.avg_batch_size,
            total_sync_duration_us: stats.total_sync_duration_us,
            failed_syncs: stats.failed_syncs,
        }
    }
    
    /// Get durability level
    pub fn durability_level(&self) -> &DurabilityLevel {
        &self.durability_level
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
    
    async fn create_test_coordinator(durability_level: DurabilityLevel) -> (BatchSyncCoordinator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );
        let disk_manager = Arc::new(WriteBufferDiskManager::new(
            filesystem_factory,
            temp_dir.path(),
        ));
        
        let coordinator = BatchSyncCoordinator::new(durability_level, disk_manager);
        (coordinator, temp_dir)
    }
    
    #[tokio::test]
    async fn test_no_sync_durability() {
        let (coordinator, _temp_dir) = create_test_coordinator(DurabilityLevel::NoSync).await;
        
        // NoSync should not perform any syncs
        coordinator.request_sync("test_collection".to_string(), "file:///test.wal".to_string()).await.unwrap();
        
        let stats = coordinator.stats().await;
        assert_eq!(stats.total_syncs, 0);
    }
    
    #[tokio::test]
    async fn test_immediate_sync_durability() {
        let (coordinator, temp_dir) = create_test_coordinator(DurabilityLevel::SyncFull).await;
        
        // Create a test file
        let test_file = temp_dir.path().join("test.wal");
        std::fs::write(&test_file, b"test data").unwrap();
        
        let file_url = format!("file://{}", test_file.display());
        
        // SyncFull should sync immediately
        coordinator.request_sync("test_collection".to_string(), file_url).await.unwrap();
        
        let stats = coordinator.stats().await;
        assert_eq!(stats.total_syncs, 1);
        assert_eq!(stats.batch_syncs, 0); // No batching for immediate sync
    }
    
    #[tokio::test]
    async fn test_batch_sync_by_count() {
        let durability = DurabilityLevel::BatchSync {
            batch_size: 2,
            interval_secs: 60, // Long interval to test count-based trigger
        };
        let (coordinator, temp_dir) = create_test_coordinator(durability).await;
        coordinator.start().await.unwrap();
        
        // Create test files
        let file1 = temp_dir.path().join("test1.wal");
        let file2 = temp_dir.path().join("test2.wal");
        std::fs::write(&file1, b"test data 1").unwrap();
        std::fs::write(&file2, b"test data 2").unwrap();
        
        // Request sync for first file - should not trigger batch
        coordinator.request_sync(
            "test_collection".to_string(),
            format!("file://{}", file1.display())
        ).await.unwrap();
        
        // Stats should show no syncs yet
        let stats = coordinator.stats().await;
        assert_eq!(stats.total_syncs, 0);
        
        // Request sync for second file - should trigger batch
        coordinator.request_sync(
            "test_collection".to_string(),
            format!("file://{}", file2.display())
        ).await.unwrap();
        
        // Give some time for batch processing
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Stats should show batch sync
        let stats = coordinator.stats().await;
        assert_eq!(stats.total_syncs, 2);
        assert_eq!(stats.batch_syncs, 1);
        assert_eq!(stats.avg_batch_size, 2.0);
        
        coordinator.shutdown().await;
    }
}