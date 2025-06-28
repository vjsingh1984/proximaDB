// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Atomic Operations Package for ProximaDB Storage Components
//!
//! This module provides common atomic operations infrastructure that can be used
//! across all storage components: VIPER, WAL, and FilestoreMetadataBackend.
//!
//! ## Key Features:
//! - **Staging Pattern**: Write to `__flush/`, `__temp/`, or custom staging directories
//! - **Atomic Moves**: Use filesystem's atomic move operations
//! - **Strategy-Based**: Different strategies for local vs cloud storage
//! - **Cleanup**: Automatic cleanup of failed/orphaned operations
//! - **Unified Interface**: Single API for all storage components

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::core::CollectionId;
use crate::storage::persistence::filesystem::{
    FilesystemFactory, FileOptions, 
    atomic_strategy::{AtomicWriteExecutor, AtomicWriteExecutorFactory, AtomicWriteConfig},
    write_strategy::{WriteStrategyFactory, MetadataWriteStrategy}
};

/// Atomic operation ID for tracking operations
pub type OperationId = String;

/// Staging operation types
#[derive(Debug, Clone, PartialEq)]
pub enum StagingOperationType {
    /// Flush operation (e.g., memtable to storage)
    Flush,
    /// Compaction operation (e.g., merge files)
    Compaction,
    /// Metadata operation (e.g., collection updates)
    Metadata,
    /// WAL operation (e.g., segment rotation)
    Wal,
    /// Custom operation type
    Custom(String),
}

impl StagingOperationType {
    /// Get staging directory name for this operation type
    pub fn staging_dir_name(&self) -> &str {
        match self {
            StagingOperationType::Flush => "__flush",
            StagingOperationType::Compaction => "__compact", 
            StagingOperationType::Metadata => "__metadata",
            StagingOperationType::Wal => "__wal",
            StagingOperationType::Custom(name) => name,
        }
    }
}

/// Staging directory configuration
#[derive(Debug, Clone)]
pub struct StagingConfig {
    /// Base storage URL (e.g., "file:///data", "s3://bucket")
    pub base_url: String,
    
    /// Collection ID (optional, for collection-specific operations)
    pub collection_id: Option<CollectionId>,
    
    /// Operation type
    pub operation_type: StagingOperationType,
    
    /// Custom staging directory name (overrides default)
    pub custom_staging_dir: Option<String>,
    
    /// Enable automatic cleanup of successful operations
    pub auto_cleanup: bool,
    
    /// Maximum age for cleanup of orphaned operations (hours)
    pub max_orphaned_age_hours: u64,
}

impl Default for StagingConfig {
    fn default() -> Self {
        Self {
            base_url: "file://./data".to_string(),
            collection_id: None,
            operation_type: StagingOperationType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        }
    }
}

/// Atomic operation metadata
#[derive(Debug, Clone)]
pub struct AtomicOperationMetadata {
    pub operation_id: OperationId,
    pub operation_type: StagingOperationType,
    pub collection_id: Option<CollectionId>,
    pub started_at: DateTime<Utc>,
    pub staging_url: String,
    pub final_url: String,
    pub status: AtomicOperationStatus,
}

/// Atomic operation status
#[derive(Debug, Clone, PartialEq)]
pub enum AtomicOperationStatus {
    /// Operation is being prepared
    Preparing,
    /// Operation is writing to staging
    Staging,
    /// Operation is performing atomic move
    Finalizing,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed(String),
}

/// Unified atomic operations coordinator
pub struct UnifiedAtomicCoordinator {
    /// Filesystem factory for multi-cloud support
    filesystem: Arc<FilesystemFactory>,
    
    /// Atomic write executor for robust operations
    atomic_executor: Box<dyn AtomicWriteExecutor>,
    
    /// Write strategy for optimized operations
    write_strategy: MetadataWriteStrategy,
    
    /// Active operations tracker
    active_operations: Arc<RwLock<HashMap<OperationId, AtomicOperationMetadata>>>,
    
    /// Configuration
    config: AtomicWriteConfig,
}

impl std::fmt::Debug for UnifiedAtomicCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedAtomicCoordinator")
            .field("filesystem", &self.filesystem)
            .field("active_operations_count", &"<RwLock>")
            .field("config", &self.config)
            .finish()
    }
}

impl UnifiedAtomicCoordinator {
    /// Create new unified atomic coordinator
    pub async fn new(
        filesystem: Arc<FilesystemFactory>,
        temp_directory: Option<String>,
    ) -> Result<Self> {
        let config = AtomicWriteConfig::default();
        
        // Create appropriate atomic executor based on environment
        let atomic_executor = if cfg!(test) {
            AtomicWriteExecutorFactory::create_dev_executor()
        } else {
            AtomicWriteExecutorFactory::create_executor(&config)
        };
        
        // Create metadata-optimized write strategy
        let fs = filesystem.get_filesystem("file://").unwrap_or_else(|_| {
            // Fallback to any available filesystem
            panic!("No filesystem available")
        });
        let write_strategy = WriteStrategyFactory::create_metadata_strategy(
            fs, 
            temp_directory.as_deref()
        )?;
        
        Ok(Self {
            filesystem,
            atomic_executor,
            write_strategy,
            active_operations: Arc::new(RwLock::new(HashMap::new())),
            config,
        })
    }
    
    /// Begin atomic operation - creates staging directory and returns operation metadata
    pub async fn begin_atomic_operation(
        &self,
        staging_config: &StagingConfig,
    ) -> Result<AtomicOperationMetadata> {
        let operation_id = Uuid::new_v4().to_string();
        
        debug!("🚀 Beginning atomic operation: {}", operation_id);
        
        // Build staging and final URLs
        let (staging_url, final_url) = self.build_operation_urls(staging_config, &operation_id)?;
        
        debug!("📁 Staging URL: {}", staging_url);
        debug!("🎯 Final URL: {}", final_url);
        
        // Create staging directory
        self.filesystem.create_dir_all(&staging_url).await
            .context("Failed to create staging directory")?;
        
        // Create operation metadata
        let metadata = AtomicOperationMetadata {
            operation_id: operation_id.clone(),
            operation_type: staging_config.operation_type.clone(),
            collection_id: staging_config.collection_id.clone(),
            started_at: Utc::now(),
            staging_url,
            final_url,
            status: AtomicOperationStatus::Preparing,
        };
        
        // Track operation
        {
            let mut active_ops = self.active_operations.write().await;
            active_ops.insert(operation_id.clone(), metadata.clone());
        }
        
        info!("✅ Atomic operation prepared: {}", operation_id);
        Ok(metadata)
    }
    
    /// Write file to staging area
    pub async fn write_to_staging(
        &self,
        operation_id: &OperationId,
        relative_path: &str,
        data: &[u8],
    ) -> Result<()> {
        // Get operation metadata
        let metadata = {
            let active_ops = self.active_operations.read().await;
            active_ops.get(operation_id)
                .ok_or_else(|| anyhow::anyhow!("Operation not found: {}", operation_id))?
                .clone()
        };
        
        // Update status to staging
        self.update_operation_status(operation_id, AtomicOperationStatus::Staging).await?;
        
        // Build staging file URL
        let staging_file_url = format!("{}/{}", metadata.staging_url.trim_end_matches('/'), relative_path);
        
        debug!("📝 Writing to staging: {} ({} bytes)", staging_file_url, data.len());
        
        // Create file options using write strategy
        let fs = self.filesystem.get_filesystem(&staging_file_url)?;
        let file_options = self.write_strategy.create_file_options(fs, &staging_file_url)?;
        
        // Write to staging using atomic executor
        self.atomic_executor.write_atomic(
            fs,
            &self.filesystem.extract_path(&staging_file_url)?,
            data,
            Some(file_options),
        ).await.context("Failed to write to staging")?;
        
        debug!("✅ Written to staging: {}", staging_file_url);
        Ok(())
    }
    
    /// Finalize atomic operation - move from staging to final location
    pub async fn finalize_atomic_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<()> {
        debug!("🔄 Finalizing atomic operation: {}", operation_id);
        
        // Get operation metadata
        let metadata = {
            let active_ops = self.active_operations.read().await;
            active_ops.get(operation_id)
                .ok_or_else(|| anyhow::anyhow!("Operation not found: {}", operation_id))?
                .clone()
        };
        
        // Update status to finalizing
        self.update_operation_status(operation_id, AtomicOperationStatus::Finalizing).await?;
        
        // List all files in staging directory
        let staging_entries = self.filesystem.list(&metadata.staging_url).await
            .context("Failed to list staging directory")?;
        
        debug!("📂 Found {} files in staging", staging_entries.len());
        
        // Move each file atomically from staging to final location
        for entry in staging_entries {
            if !entry.metadata.is_directory && !entry.name.starts_with(".") {
                let staging_file_url = format!("{}/{}", metadata.staging_url.trim_end_matches('/'), entry.name);
                let final_file_url = format!("{}/{}", metadata.final_url.trim_end_matches('/'), entry.name);
                
                debug!("🔄 Moving: {} → {}", staging_file_url, final_file_url);
                
                // Use FilesystemFactory's atomic move which handles cross-storage scenarios
                self.filesystem.move_atomic(&staging_file_url, &final_file_url).await
                    .context(format!("Failed to move {} to {}", staging_file_url, final_file_url))?;
                
                debug!("✅ Moved: {}", entry.name);
            }
        }
        
        // Clean up staging directory
        if metadata.operation_type != StagingOperationType::Custom("preserve_staging".to_string()) {
            self.filesystem.delete(&metadata.staging_url).await
                .context("Failed to cleanup staging directory")?;
            debug!("🧹 Cleaned up staging directory");
        }
        
        // Update status to completed
        self.update_operation_status(operation_id, AtomicOperationStatus::Completed).await?;
        
        // Remove from active operations
        {
            let mut active_ops = self.active_operations.write().await;
            active_ops.remove(operation_id);
        }
        
        info!("🎉 Atomic operation completed: {}", operation_id);
        Ok(())
    }
    
    /// Abort atomic operation - cleanup staging area
    pub async fn abort_atomic_operation(
        &self,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<()> {
        warn!("❌ Aborting atomic operation: {} ({})", operation_id, reason);
        
        // Get operation metadata
        let metadata = {
            let active_ops = self.active_operations.read().await;
            active_ops.get(operation_id).cloned()
        };
        
        if let Some(metadata) = metadata {
            // Update status to failed
            self.update_operation_status(operation_id, AtomicOperationStatus::Failed(reason.to_string())).await?;
            
            // Cleanup staging directory
            self.filesystem.delete(&metadata.staging_url).await.ok();
            debug!("🧹 Cleaned up staging directory after abort");
            
            // Remove from active operations
            {
                let mut active_ops = self.active_operations.write().await;
                active_ops.remove(operation_id);
            }
        }
        
        Ok(())
    }
    
    /// Get operation status
    pub async fn get_operation_status(&self, operation_id: &OperationId) -> Option<AtomicOperationStatus> {
        let active_ops = self.active_operations.read().await;
        active_ops.get(operation_id).map(|meta| meta.status.clone())
    }
    
    /// List active operations
    pub async fn list_active_operations(&self) -> Vec<AtomicOperationMetadata> {
        let active_ops = self.active_operations.read().await;
        active_ops.values().cloned().collect()
    }
    
    /// Cleanup orphaned operations older than configured age
    pub async fn cleanup_orphaned_operations(&self, base_url: &str, max_age_hours: u64) -> Result<usize> {
        debug!("🧹 Cleaning up orphaned operations older than {} hours", max_age_hours);
        
        let cutoff_time = Utc::now() - chrono::Duration::hours(max_age_hours as i64);
        let mut cleaned_count = 0;
        
        // Find directories with staging patterns
        let staging_patterns = ["__flush", "__compact", "__metadata", "__wal"];
        
        for pattern in &staging_patterns {
            let pattern_url = format!("{}/{}", base_url.trim_end_matches('/'), pattern);
            
            if let Ok(entries) = self.filesystem.list(&pattern_url).await {
                for entry in entries {
                    if entry.metadata.is_directory {
                        // Check if directory is older than cutoff
                        if let Some(created) = entry.metadata.created {
                            if created < cutoff_time {
                                let old_dir_url = format!("{}/{}", pattern_url, entry.name);
                                if let Ok(()) = self.filesystem.delete(&old_dir_url).await {
                                    cleaned_count += 1;
                                    debug!("🧹 Cleaned orphaned staging dir: {}", old_dir_url);
                                }
                            }
                        }
                    }
                }
            }
        }
        
        info!("🧹 Cleaned {} orphaned operations", cleaned_count);
        Ok(cleaned_count)
    }
    
    // Private helper methods
    
    /// Build staging and final URLs for operation
    fn build_operation_urls(
        &self,
        config: &StagingConfig,
        operation_id: &OperationId,
    ) -> Result<(String, String)> {
        let base_url = config.base_url.trim_end_matches('/');
        
        // Build collection-specific path if provided
        let collection_path = if let Some(ref collection_id) = config.collection_id {
            format!("/collections/{}", collection_id)
        } else {
            String::new()
        };
        
        // Build staging directory name
        let staging_dir = config.custom_staging_dir
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or_else(|| config.operation_type.staging_dir_name());
        
        // Build URLs
        let staging_url = format!("{}{}/{}/{}", base_url, collection_path, staging_dir, operation_id);
        let final_url = format!("{}{}", base_url, collection_path);
        
        Ok((staging_url, final_url))
    }
    
    /// Update operation status
    async fn update_operation_status(
        &self,
        operation_id: &OperationId,
        status: AtomicOperationStatus,
    ) -> Result<()> {
        let mut active_ops = self.active_operations.write().await;
        if let Some(metadata) = active_ops.get_mut(operation_id) {
            metadata.status = status;
        }
        Ok(())
    }
}

/// Convenience wrapper for VIPER-specific atomic operations
pub struct ViperAtomicOperations {
    coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl ViperAtomicOperations {
    pub fn new(coordinator: Arc<UnifiedAtomicCoordinator>) -> Self {
        Self { coordinator }
    }
    
    /// Begin flush operation for a collection
    pub async fn begin_flush_operation(
        &self,
        collection_id: &CollectionId,
        storage_url: &str,
    ) -> Result<AtomicOperationMetadata> {
        let config = StagingConfig {
            base_url: storage_url.to_string(),
            collection_id: Some(collection_id.clone()),
            operation_type: StagingOperationType::Flush,
            auto_cleanup: true,
            ..Default::default()
        };
        
        self.coordinator.begin_atomic_operation(&config).await
    }
    
    /// Write Parquet file to flush staging
    pub async fn write_parquet_to_staging(
        &self,
        operation_id: &OperationId,
        parquet_filename: &str,
        parquet_data: &[u8],
    ) -> Result<()> {
        let relative_path = format!("vectors/{}", parquet_filename);
        self.coordinator.write_to_staging(operation_id, &relative_path, parquet_data).await
    }
    
    /// Finalize flush operation
    pub async fn finalize_flush(&self, operation_id: &OperationId) -> Result<()> {
        self.coordinator.finalize_atomic_operation(operation_id).await
    }
    
    /// Abort flush operation
    pub async fn abort_flush(&self, operation_id: &OperationId, reason: &str) -> Result<()> {
        self.coordinator.abort_atomic_operation(operation_id, reason).await
    }
}

/// Convenience wrapper for WAL-specific atomic operations
pub struct WalAtomicOperations {
    coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl WalAtomicOperations {
    pub fn new(coordinator: Arc<UnifiedAtomicCoordinator>) -> Self {
        Self { coordinator }
    }
    
    /// Begin WAL segment rotation operation
    pub async fn begin_segment_rotation(
        &self,
        wal_url: &str,
    ) -> Result<AtomicOperationMetadata> {
        let config = StagingConfig {
            base_url: wal_url.to_string(),
            collection_id: None,
            operation_type: StagingOperationType::Wal,
            auto_cleanup: true,
            ..Default::default()
        };
        
        self.coordinator.begin_atomic_operation(&config).await
    }
    
    /// Write WAL segment to staging
    pub async fn write_segment_to_staging(
        &self,
        operation_id: &OperationId,
        segment_filename: &str,
        segment_data: &[u8],
    ) -> Result<()> {
        self.coordinator.write_to_staging(operation_id, segment_filename, segment_data).await
    }
    
    /// Finalize segment rotation
    pub async fn finalize_rotation(&self, operation_id: &OperationId) -> Result<()> {
        self.coordinator.finalize_atomic_operation(operation_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemConfig;
    use tempfile::TempDir;
    
    async fn create_test_coordinator() -> (UnifiedAtomicCoordinator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().to_string_lossy().to_string();
        
        let fs_config = FilesystemConfig {
            default_fs: Some(format!("file://{}", temp_path)),
            ..Default::default()
        };
        
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
        let coordinator = UnifiedAtomicCoordinator::new(filesystem, None).await.unwrap();
        
        (coordinator, temp_dir)
    }
    
    #[tokio::test]
    async fn test_atomic_operation_lifecycle() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        
        let config = StagingConfig {
            base_url: "file://./test_data".to_string(),
            collection_id: Some("test_collection".to_string()),
            operation_type: StagingOperationType::Flush,
            ..Default::default()
        };
        
        // Begin operation
        let metadata = coordinator.begin_atomic_operation(&config).await.unwrap();
        assert_eq!(metadata.status, AtomicOperationStatus::Preparing);
        
        // Write to staging
        let test_data = b"test parquet data";
        coordinator.write_to_staging(&metadata.operation_id, "test.parquet", test_data).await.unwrap();
        
        // Check status
        let status = coordinator.get_operation_status(&metadata.operation_id).await.unwrap();
        assert_eq!(status, AtomicOperationStatus::Staging);
        
        // Finalize operation
        coordinator.finalize_atomic_operation(&metadata.operation_id).await.unwrap();
        
        // Operation should be removed from active operations
        assert!(coordinator.get_operation_status(&metadata.operation_id).await.is_none());
    }
    
    #[tokio::test]
    async fn test_viper_atomic_operations() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);
        
        let viper_ops = ViperAtomicOperations::new(coordinator);
        
        // Begin flush operation
        let metadata = viper_ops.begin_flush_operation(
            &"test_collection".to_string(),
            "file://./test_data"
        ).await.unwrap();
        
        // Write Parquet data
        let parquet_data = b"mock parquet data";
        viper_ops.write_parquet_to_staging(
            &metadata.operation_id,
            "batch_001.parquet",
            parquet_data
        ).await.unwrap();
        
        // Finalize flush
        viper_ops.finalize_flush(&metadata.operation_id).await.unwrap();
    }
    
    #[tokio::test]
    async fn test_operation_abort() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        
        let config = StagingConfig {
            base_url: "file://./test_data".to_string(),
            operation_type: StagingOperationType::Flush,
            ..Default::default()
        };
        
        // Begin operation
        let metadata = coordinator.begin_atomic_operation(&config).await.unwrap();
        
        // Write some data
        coordinator.write_to_staging(&metadata.operation_id, "test.dat", b"test").await.unwrap();
        
        // Abort operation
        coordinator.abort_atomic_operation(&metadata.operation_id, "test abort").await.unwrap();
        
        // Operation should be removed
        assert!(coordinator.get_operation_status(&metadata.operation_id).await.is_none());
    }
}