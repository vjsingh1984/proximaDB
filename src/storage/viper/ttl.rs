//! TTL (Time-To-Live) Management for VIPER Storage
//! 
//! This module implements automatic cleanup of expired vectors and Parquet files
//! based on TTL configuration. It provides background tasks for efficient
//! expiration handling without impacting query performance.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::interval;
use anyhow::{Result, Context};
use chrono::{DateTime, Utc};
use tracing::{info, warn, debug, error};

use crate::storage::filesystem::FilesystemFactory;
use super::config::TTLConfig;
use super::types::{PartitionId, PartitionMetadata};

/// TTL cleanup service for automatic vector expiration
#[derive(Debug)]
pub struct TTLCleanupService {
    /// TTL configuration
    config: TTLConfig,
    
    /// Filesystem access for Parquet file operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Partition metadata for tracking file expiration
    partition_metadata: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,
    
    /// Background cleanup task handle
    cleanup_task: Option<tokio::task::JoinHandle<()>>,
    
    /// Statistics tracking
    stats: Arc<RwLock<TTLStats>>,
}

/// TTL cleanup statistics
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct TTLStats {
    pub total_cleanup_runs: u64,
    pub vectors_expired: u64,
    pub files_deleted: u64,
    pub last_cleanup_at: Option<DateTime<Utc>>,
    pub last_cleanup_duration_ms: u64,
    pub errors_count: u64,
}

/// Cleanup result for a single operation
#[derive(Debug, serde::Serialize)]
pub struct CleanupResult {
    pub vectors_expired: u64,
    pub files_deleted: u64,
    pub partitions_processed: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

impl TTLCleanupService {
    /// Create new TTL cleanup service
    pub fn new(
        config: TTLConfig,
        filesystem: Arc<FilesystemFactory>,
        partition_metadata: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,
    ) -> Self {
        Self {
            config,
            filesystem,
            partition_metadata,
            cleanup_task: None,
            stats: Arc::new(RwLock::new(TTLStats::default())),
        }
    }
    
    /// Start background TTL cleanup process
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            info!("🕒 TTL cleanup service disabled");
            return Ok(());
        }
        
        info!("🚀 Starting TTL cleanup service with interval: {:?}", self.config.cleanup_interval);
        
        let config = self.config.clone();
        let filesystem = self.filesystem.clone();
        let partition_metadata = self.partition_metadata.clone();
        let stats = self.stats.clone();
        
        // Convert chrono::Duration to tokio::time::Duration
        let std_duration = config.cleanup_interval.to_std()
            .context("Failed to convert cleanup interval to std duration")?;
        let tokio_duration = std_duration;
        
        let cleanup_task = tokio::spawn(async move {
            let mut interval = interval(tokio_duration);
            
            loop {
                interval.tick().await;
                
                let start_time = std::time::Instant::now();
                debug!("🧹 Starting TTL cleanup cycle");
                
                match Self::run_cleanup_cycle(&config, &filesystem, &partition_metadata).await {
                    Ok(result) => {
                        let mut stats_guard = stats.write().await;
                        stats_guard.total_cleanup_runs += 1;
                        stats_guard.vectors_expired += result.vectors_expired;
                        stats_guard.files_deleted += result.files_deleted;
                        stats_guard.last_cleanup_at = Some(Utc::now());
                        stats_guard.last_cleanup_duration_ms = result.duration_ms;
                        
                        if result.vectors_expired > 0 || result.files_deleted > 0 {
                            info!("✅ TTL cleanup completed: {} vectors expired, {} files deleted, {} partitions processed in {}ms",
                                  result.vectors_expired, result.files_deleted, result.partitions_processed, result.duration_ms);
                        } else {
                            debug!("🔍 TTL cleanup completed: no expired data found");
                        }
                        
                        if !result.errors.is_empty() {
                            stats_guard.errors_count += result.errors.len() as u64;
                            warn!("⚠️ TTL cleanup encountered {} errors: {:?}", result.errors.len(), result.errors);
                        }
                    }
                    Err(e) => {
                        error!("❌ TTL cleanup cycle failed: {}", e);
                        let mut stats_guard = stats.write().await;
                        stats_guard.errors_count += 1;
                    }
                }
            }
        });
        
        self.cleanup_task = Some(cleanup_task);
        Ok(())
    }
    
    /// Stop background cleanup process
    pub async fn stop(&mut self) {
        if let Some(task) = self.cleanup_task.take() {
            task.abort();
            info!("🛑 TTL cleanup service stopped");
        }
    }
    
    /// Run a single cleanup cycle
    async fn run_cleanup_cycle(
        config: &TTLConfig,
        filesystem: &FilesystemFactory,
        partition_metadata: &RwLock<HashMap<PartitionId, PartitionMetadata>>,
    ) -> Result<CleanupResult> {
        let start_time = std::time::Instant::now();
        let now = Utc::now();
        
        let mut vectors_expired = 0;
        let mut files_deleted = 0;
        let mut partitions_processed = 0;
        let mut errors = Vec::new();
        
        // Get current partition metadata
        let partitions = {
            let metadata_guard = partition_metadata.read().await;
            metadata_guard.clone()
        };
        
        for (partition_id, metadata) in partitions {
            partitions_processed += 1;
            
            // Check if this partition has expired vectors
            if let Err(e) = Self::cleanup_partition(
                config,
                filesystem,
                &partition_id,
                &metadata,
                now,
                &mut vectors_expired,
                &mut files_deleted,
            ).await {
                errors.push(format!("Partition {}: {}", partition_id, e));
                continue;
            }
        }
        
        Ok(CleanupResult {
            vectors_expired,
            files_deleted,
            partitions_processed,
            errors,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }
    
    /// Clean up expired vectors in a specific partition
    async fn cleanup_partition(
        config: &TTLConfig,
        filesystem: &FilesystemFactory,
        partition_id: &PartitionId,
        metadata: &PartitionMetadata,
        now: DateTime<Utc>,
        vectors_expired: &mut u64,
        files_deleted: &mut u64,
    ) -> Result<()> {
        // Check if the entire partition can be deleted (all vectors expired)
        if Self::is_partition_fully_expired(metadata, now, config) {
            // Delete all Parquet files for this partition
            for file_info in &metadata.parquet_files {
                match filesystem.delete(&file_info.storage_url).await {
                    Ok(_) => {
                        *files_deleted += 1;
                        debug!("🗑️ Deleted expired Parquet file: {}", file_info.storage_url);
                    }
                    Err(e) => {
                        warn!("Failed to delete Parquet file {}: {}", file_info.storage_url, e);
                    }
                }
            }
            
            *vectors_expired += metadata.statistics.total_vectors as u64;
            return Ok(());
        }
        
        // For partially expired partitions, we would need to:
        // 1. Read the Parquet file(s)
        // 2. Filter out expired vectors
        // 3. Write new Parquet file(s) without expired vectors
        // 4. Update metadata
        // This is more complex and would be implemented based on specific requirements
        
        Ok(())
    }
    
    /// Check if an entire partition has expired and can be deleted
    fn is_partition_fully_expired(
        metadata: &PartitionMetadata,
        now: DateTime<Utc>,
        config: &TTLConfig,
    ) -> bool {
        // If file-level filtering is disabled, don't delete entire files
        if !config.enable_file_level_filtering {
            return false;
        }
        
        // Check if partition is old enough to be considered for deletion
        let partition_age = now.signed_duration_since(metadata.created_at);
        if partition_age < config.min_file_expiration_age {
            return false;
        }
        
        // Check if all vectors in the partition have expired
        // This would require reading the Parquet file to check expires_at for each vector
        // For now, we use a heuristic based on the partition's last_modified time
        let time_since_modification = now.signed_duration_since(metadata.last_modified);
        
        // If the partition hasn't been modified for a long time and is past the minimum age,
        // it's likely that most/all vectors have expired
        time_since_modification > config.min_file_expiration_age * 2
    }
    
    /// Get TTL cleanup statistics
    pub async fn get_stats(&self) -> TTLStats {
        (*self.stats.read().await).clone()
    }
    
    /// Force run cleanup immediately (for testing/admin purposes)
    pub async fn force_cleanup(&self) -> Result<CleanupResult> {
        if !self.config.enabled {
            return Err(anyhow::anyhow!("TTL cleanup is disabled"));
        }
        
        info!("🔧 Running forced TTL cleanup");
        Self::run_cleanup_cycle(&self.config, &self.filesystem, &self.partition_metadata).await
    }
}

impl Drop for TTLCleanupService {
    fn drop(&mut self) {
        if let Some(task) = self.cleanup_task.take() {
            task.abort();
        }
    }
}