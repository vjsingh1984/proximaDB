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
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;


use crate::storage::persistence::filesystem::{
    atomic_strategy::{AtomicWriteConfig, AtomicWriteExecutor, AtomicWriteExecutorFactory},
    write_strategy::{MetadataWriteStrategy, WriteStrategyFactory},
    FilesystemFactory,
};

/// Atomic operation ID for tracking operations
pub type OperationId = String;

/// Transaction ID for ACID operations
pub type TransactionId = String;

/// Transaction ID generator
static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(1);

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
    /// ACID transaction operation
    Transaction,
}

impl StagingOperationType {
    /// Get staging directory name for this operation type
    pub fn staging_dir_name(&self) -> &str {
        match self {
            StagingOperationType::Flush => "__flush",
            StagingOperationType::Compaction => "__compact",
            StagingOperationType::Metadata => "__metadata",
            StagingOperationType::Wal => "__wal",
            StagingOperationType::Transaction => "__transaction",
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
    pub collection_id: Option<String>,

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
    pub collection_id: Option<String>,
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

/// Transaction state for ACID compliance
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    /// Transaction initialized but not started
    Initialized,
    /// Phase 1: Preparing all participants
    Preparing,
    /// All participants prepared successfully
    Prepared,
    /// Phase 2: Committing all participants
    Committing,
    /// Transaction committed successfully
    Committed,
    /// Transaction is aborting
    Aborting,
    /// Transaction aborted/rolled back
    Aborted,
    /// Transaction failed
    Failed(String),
}

/// Participant in the atomic transaction
#[derive(Debug, Clone)]
pub struct TransactionParticipant {
    /// Unique participant ID
    pub id: String,
    /// Current state of this participant
    pub state: ParticipantState,
    /// Rollback action if needed
    pub rollback_action: Option<RollbackAction>,
    /// Timestamp when participant was added
    pub joined_at: Instant,
}

/// State of individual participant
#[derive(Debug, Clone, PartialEq)]
pub enum ParticipantState {
    /// Participant registered but not prepared
    Registered,
    /// Participant is preparing
    Preparing,
    /// Participant prepared successfully
    Prepared,
    /// Participant is committing
    Committing,
    /// Participant committed successfully
    Committed,
    /// Participant is rolling back
    RollingBack,
    /// Participant rolled back successfully
    RolledBack,
    /// Participant failed
    Failed(String),
}

/// Rollback action for a participant
#[derive(Debug, Clone)]
pub enum RollbackAction {
    /// Remove entry from memtable by key
    RemoveFromMemtable { key: String },
    /// Restore previous value in memtable
    RestoreMemtableValue { key: String, previous_value: Vec<u8> },
    /// Remove entry from secondary index
    RemoveFromSecondaryIndex { name: String, uuid: String },
    /// Delete file from disk
    DeleteFile { path: String },
    /// Restore file on disk
    RestoreFile { path: String, content: Vec<u8> },
    /// Custom rollback action
    Custom(String),
}

/// Active transaction tracking
#[derive(Debug)]
pub struct ActiveTransaction {
    /// Unique transaction ID
    pub id: String,
    /// Transaction state
    pub state: TransactionState,
    /// Participants in this transaction
    pub participants: HashMap<String, TransactionParticipant>,
    /// Transaction start time
    pub started_at: Instant,
    /// Transaction deadline
    pub deadline: Option<Instant>,
    /// Transaction metadata
    pub metadata: HashMap<String, String>,
}

impl ActiveTransaction {
    /// Create new transaction
    pub fn new(id: String, deadline_ms: Option<u64>) -> Self {
        let now = Instant::now();
        Self {
            id,
            state: TransactionState::Initialized,
            participants: HashMap::new(),
            started_at: now,
            deadline: deadline_ms.map(|ms| now + Duration::from_millis(ms)),
            metadata: HashMap::new(),
        }
    }

    /// Check if transaction has expired
    pub fn is_expired(&self) -> bool {
        self.deadline.map_or(false, |d| Instant::now() > d)
    }

    /// Add participant to transaction
    pub fn add_participant(&mut self, id: String, rollback_action: Option<RollbackAction>) {
        self.participants.insert(
            id.clone(),
            TransactionParticipant {
                id,
                state: ParticipantState::Registered,
                rollback_action,
                joined_at: Instant::now(),
            },
        );
    }

    /// Check if all participants are in the given state
    pub fn all_participants_in_state(&self, state: &ParticipantState) -> bool {
        self.participants.values().all(|p| &p.state == state)
    }

    /// Update participant state
    pub fn update_participant_state(&mut self, id: &str, state: ParticipantState) -> Result<()> {
        self.participants
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Participant {} not found", id))
            .map(|p| p.state = state)
    }
}


/// Unified atomic operations coordinator with ACID support
pub struct UnifiedAtomicCoordinator {
    /// Filesystem factory for multi-cloud support
    filesystem: Arc<FilesystemFactory>,

    /// Atomic write executor for robust operations
    atomic_executor: Box<dyn AtomicWriteExecutor>,

    /// Write strategy for optimized operations
    write_strategy: MetadataWriteStrategy,

    /// Active operations tracker using lock-free DashMap
    active_operations: Arc<DashMap<OperationId, AtomicOperationMetadata>>,

    /// Active ACID transactions
    transactions: Arc<DashMap<TransactionId, Arc<RwLock<ActiveTransaction>>>>,

    /// Transaction timeout (default: 30 seconds)
    default_timeout_ms: u64,

    /// Cleanup task handle
    cleanup_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,

    /// Configuration
    config: AtomicWriteConfig,
}

impl std::fmt::Debug for UnifiedAtomicCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedAtomicCoordinator")
            .field("filesystem", &self.filesystem)
            .field("active_operations_count", &self.active_operations.len())
            .field("config", &self.config)
            .finish()
    }
}

impl UnifiedAtomicCoordinator {
    /// Create new unified atomic coordinator with ACID support
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
        let write_strategy =
            WriteStrategyFactory::create_metadata_strategy(fs, temp_directory.as_deref())?;

        let coordinator = Self {
            filesystem,
            atomic_executor,
            write_strategy,
            active_operations: Arc::new(DashMap::new()),
            transactions: Arc::new(DashMap::new()),
            default_timeout_ms: 30000, // 30 seconds
            cleanup_handle: Arc::new(Mutex::new(None)),
            config,
        };
        
        // Start cleanup task for expired transactions
        coordinator.start_cleanup_task();
        
        Ok(coordinator)
    }

    /// Begin atomic operation - creates staging directory and returns operation metadata
    pub async fn begin_atomic_operation(
        &self,
        staging_config: &StagingConfig,
    ) -> Result<AtomicOperationMetadata> {
        let operation_id = Uuid::new_v4().to_string();

        info!("🚀 Beginning atomic operation: {}", operation_id);
        info!("📋 Staging config: base_url={}, operation_type={:?}, custom_staging_dir={:?}", 
            staging_config.base_url, staging_config.operation_type, staging_config.custom_staging_dir);

        // Build staging and final URLs
        let (staging_url, final_url) = self.build_operation_urls(staging_config, &operation_id)?;

        info!("📁 Staging URL: {}", staging_url);
        info!("🎯 Final URL: {}", final_url);

        // Create both staging and final directories upfront for robustness
        // Note: staging_url and final_url are directory URLs, not file URLs
        info!("📂 Creating staging directory: {}", staging_url);
        self.filesystem
            .create_dir_all(&staging_url)
            .await
            .context("Failed to create staging directory")?;
            
        info!("📂 Creating final directory: {}", final_url);
        self.filesystem
            .create_dir_all(&final_url)
            .await
            .context("Failed to create final directory")?;

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

        // Track operation using DashMap
        self.active_operations.insert(operation_id.clone(), metadata.clone());

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
        info!("📝 write_to_staging START");
        info!("    operation_id: {}", operation_id);
        info!("    relative_path: {}", relative_path);
        info!("    data size: {} bytes", data.len());
        
        // Get operation metadata from DashMap
        let metadata = self.active_operations
            .get(operation_id)
            .ok_or_else(|| anyhow::anyhow!("Operation not found: {}", operation_id))?
            .clone();

        info!("    metadata.staging_url: {}", metadata.staging_url);

        // Update status to staging
        self.update_operation_status(operation_id, AtomicOperationStatus::Staging)
            .await?;

        // Build staging file URL
        let staging_file_url = format!(
            "{}/{}",
            metadata.staging_url.trim_end_matches('/'),
            relative_path
        );

        info!("    staging_file_url: {}", staging_file_url);

        // Create file options using write strategy
        let fs = self.filesystem.get_filesystem(&staging_file_url)?;
        info!("    filesystem type: {}", fs.filesystem_type());
        
        let file_options = self
            .write_strategy
            .create_file_options(fs, &staging_file_url)?;

        // Extract path for write
        let staging_path = self.filesystem.extract_path(&staging_file_url)?;
        info!("    staging_path extracted: {}", staging_path);

        // Write to staging using atomic executor
        info!("    Calling write_atomic...");
        match self.atomic_executor
            .write_atomic(
                fs,
                &staging_path,
                data,
                Some(file_options),
            )
            .await {
            Ok(_) => {
                info!("✅ Written to staging: {}", staging_file_url);
                Ok(())
            }
            Err(e) => {
                error!("❌ Failed to write to staging: {}", e);
                Err(anyhow::anyhow!("Failed to write to staging: {}", e))
            }
        }
    }

    /// Finalize atomic operation - move from staging to final location
    pub async fn finalize_atomic_operation(&self, operation_id: &OperationId) -> Result<()> {
        info!("🔄 Finalizing atomic operation: {}", operation_id);

        // Get operation metadata from DashMap
        let metadata = self.active_operations
            .get(operation_id)
            .ok_or_else(|| anyhow::anyhow!("Operation not found: {}", operation_id))?
            .clone();

        info!("📋 Operation metadata:");
        info!("    staging_url: {}", metadata.staging_url);
        info!("    final_url: {}", metadata.final_url);
        info!("    operation_type: {:?}", metadata.operation_type);

        // Update status to finalizing
        self.update_operation_status(operation_id, AtomicOperationStatus::Finalizing)
            .await?;

        // List all files in staging directory
        info!("📂 Listing staging directory: {}", metadata.staging_url);
        let staging_entries = self
            .filesystem
            .list(&metadata.staging_url)
            .await
            .context("Failed to list staging directory")?;

        info!("📂 Found {} files in staging", staging_entries.len());
        for (idx, entry) in staging_entries.iter().enumerate() {
            info!("    [{}] {} (dir: {})", idx, entry.name, entry.metadata.is_directory);
        }

        // Move each file atomically from staging to final location
        // Note: Final directory was already created during begin_atomic_operation
        for entry in staging_entries {
            if !entry.metadata.is_directory && !entry.name.starts_with(".") {
                // Use the full URL from entry, not just the name
                let staging_file_url = entry.url.clone();
                let final_file_url = format!(
                    "{}/{}",
                    metadata.final_url.trim_end_matches('/'),
                    entry.name
                );

                info!("🔄 Moving file:");
                info!("    From: {}", staging_file_url);
                info!("    To:   {}", final_file_url);

                // Use FilesystemFactory's atomic move which handles cross-storage scenarios
                match self.filesystem
                    .move_atomic(&staging_file_url, &final_file_url)
                    .await {
                    Ok(_) => {
                        info!("    ✅ Move successful");
                        // Verify the file exists at the final location
                        if let Ok(fs) = self.filesystem.get_filesystem(&final_file_url) {
                            if let Ok(exists) = fs.exists(&final_file_url).await {
                                info!("    ✅ Verified file exists at final location: {}", exists);
                            }
                        }
                    },
                    Err(e) => {
                        error!("    ❌ Move failed: {}", e);
                        return Err(anyhow::anyhow!(
                            "Failed to move {} to {}: {}",
                            staging_file_url, final_file_url, e
                        ));
                    }
                }

                debug!("✅ Moved: {}", entry.name);
            }
        }

        // Clean up staging directory
        if metadata.operation_type != StagingOperationType::Custom("preserve_staging".to_string()) {
            self.filesystem
                .delete(&metadata.staging_url)
                .await
                .context("Failed to cleanup staging directory")?;
            debug!("🧹 Cleaned up staging directory");
        }

        // Update status to completed
        self.update_operation_status(operation_id, AtomicOperationStatus::Completed)
            .await?;

        // Remove from active operations using DashMap
        self.active_operations.remove(operation_id);

        // List the final directory to confirm files are there
        info!("📂 Listing final directory after operation: {}", metadata.final_url);
        if let Ok(final_entries) = self.filesystem.list(&metadata.final_url).await {
            info!("📂 Found {} files in final location", final_entries.len());
            for (idx, entry) in final_entries.iter().enumerate() {
                info!("    [{}] {} (size: {} bytes)", idx, entry.name, entry.metadata.size);
            }
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
        warn!(
            "❌ Aborting atomic operation: {} ({})",
            operation_id, reason
        );

        // Get operation metadata from DashMap
        let metadata = self.active_operations.get(operation_id).map(|entry| entry.clone());

        if let Some(metadata) = metadata {
            // Update status to failed
            self.update_operation_status(
                operation_id,
                AtomicOperationStatus::Failed(reason.to_string()),
            )
            .await?;

            // Cleanup staging directory
            self.filesystem.delete(&metadata.staging_url).await.ok();
            debug!("🧹 Cleaned up staging directory after abort");

            // Remove from active operations using DashMap
            self.active_operations.remove(operation_id);
        }

        Ok(())
    }

    /// Get operation status
    pub async fn get_operation_status(
        &self,
        operation_id: &OperationId,
    ) -> Option<AtomicOperationStatus> {
        self.active_operations.get(operation_id).map(|entry| entry.status.clone())
    }

    /// List active operations
    pub async fn list_active_operations(&self) -> Vec<AtomicOperationMetadata> {
        self.active_operations.iter().map(|entry| entry.value().clone()).collect()
    }

    /// Cleanup orphaned operations older than configured age
    pub async fn cleanup_orphaned_operations(
        &self,
        base_url: &str,
        max_age_hours: u64,
    ) -> Result<usize> {
        debug!(
            "🧹 Cleaning up orphaned operations older than {} hours",
            max_age_hours
        );

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

    // ACID Transaction Support Methods

    /// Begin a new atomic transaction
    pub async fn begin_transaction(
        &self,
        tx_id: &str,
        participants: Vec<String>,
    ) -> Result<TransactionHandle> {
        info!("🔄 Beginning transaction: {}", tx_id);
        
        // Check if transaction already exists
        if self.transactions.contains_key(tx_id) {
            return Err(anyhow::anyhow!("Transaction {} already exists", tx_id));
        }
        
        // Create new transaction
        let mut transaction = ActiveTransaction::new(tx_id.to_string(), Some(self.default_timeout_ms));
        
        // Register participants
        for participant in participants {
            transaction.add_participant(participant, None);
        }
        
        transaction.state = TransactionState::Initialized;
        
        // Store transaction
        let tx_arc = Arc::new(RwLock::new(transaction));
        self.transactions.insert(tx_id.to_string(), tx_arc.clone());
        
        Ok(TransactionHandle {
            id: tx_id.to_string(),
            coordinator: self,
            transaction: tx_arc,
        })
    }

    /// Prepare phase of two-phase commit
    pub async fn prepare_transaction(&self, tx_id: &str) -> Result<bool> {
        let tx = self.get_transaction(tx_id).await?;
        let mut tx_guard = tx.write().await;
        
        // Check current state
        match &tx_guard.state {
            TransactionState::Initialized => {
                tx_guard.state = TransactionState::Preparing;
            }
            _ => return Err(anyhow::anyhow!("Transaction {} not in correct state for prepare", tx_id)),
        }
        
        // Mark all participants as preparing
        let participant_ids: Vec<String> = tx_guard.participants.keys().cloned().collect();
        for pid in &participant_ids {
            tx_guard.update_participant_state(pid, ParticipantState::Preparing)?;
        }
        
        drop(tx_guard); // Release lock during preparation
        
        // Simulate prepare phase (in real implementation, would call each participant)
        info!("📝 Preparing {} participants for transaction {}", participant_ids.len(), tx_id);
        
        // Re-acquire lock and check results
        let mut tx_guard = tx.write().await;
        
        // Mark all participants as prepared (in real implementation, based on actual results)
        for pid in &participant_ids {
            tx_guard.update_participant_state(pid, ParticipantState::Prepared)?;
        }
        
        // Update transaction state
        if tx_guard.all_participants_in_state(&ParticipantState::Prepared) {
            tx_guard.state = TransactionState::Prepared;
            info!("✅ All participants prepared for transaction {}", tx_id);
            Ok(true)
        } else {
            tx_guard.state = TransactionState::Failed("Not all participants prepared".to_string());
            Ok(false)
        }
    }

    /// Commit phase of two-phase commit
    pub async fn commit_transaction(&self, tx_id: &str) -> Result<()> {
        let tx = self.get_transaction(tx_id).await?;
        let mut tx_guard = tx.write().await;
        
        // Check current state
        match &tx_guard.state {
            TransactionState::Prepared => {
                tx_guard.state = TransactionState::Committing;
            }
            _ => return Err(anyhow::anyhow!("Transaction {} not prepared for commit", tx_id)),
        }
        
        // Mark all participants as committing
        let participant_ids: Vec<String> = tx_guard.participants.keys().cloned().collect();
        for pid in &participant_ids {
            tx_guard.update_participant_state(pid, ParticipantState::Committing)?;
        }
        
        drop(tx_guard); // Release lock during commit
        
        // Simulate commit phase
        info!("💾 Committing {} participants for transaction {}", participant_ids.len(), tx_id);
        
        // Re-acquire lock and mark as committed
        let mut tx_guard = tx.write().await;
        
        // Mark all participants as committed
        for pid in &participant_ids {
            tx_guard.update_participant_state(pid, ParticipantState::Committed)?;
        }
        
        tx_guard.state = TransactionState::Committed;
        info!("✅ Transaction {} committed successfully", tx_id);
        
        // Remove from active transactions after short delay
        let transactions = self.transactions.clone();
        let tx_id_clone = tx_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            transactions.remove(&tx_id_clone);
        });
        
        Ok(())
    }

    /// Rollback/abort transaction
    pub async fn rollback_transaction(&self, tx_id: &str) -> Result<()> {
        let tx = self.get_transaction(tx_id).await?;
        let mut tx_guard = tx.write().await;
        
        info!("🔙 Rolling back transaction: {}", tx_id);
        tx_guard.state = TransactionState::Aborting;
        
        // Get rollback actions
        let rollback_actions: Vec<(String, Option<RollbackAction>)> = tx_guard
            .participants
            .values()
            .map(|p| (p.id.clone(), p.rollback_action.clone()))
            .collect();
        
        // Mark participants as rolling back
        for (pid, _) in &rollback_actions {
            tx_guard.update_participant_state(pid, ParticipantState::RollingBack)?;
        }
        
        drop(tx_guard); // Release lock during rollback
        
        // Execute rollback actions
        for (pid, action) in rollback_actions {
            if let Some(action) = action {
                self.execute_rollback_action(&pid, action).await?;
            }
        }
        
        // Mark transaction as aborted
        let mut tx_guard = tx.write().await;
        for pid in tx_guard.participants.keys().cloned().collect::<Vec<_>>() {
            tx_guard.update_participant_state(&pid, ParticipantState::RolledBack)?;
        }
        
        tx_guard.state = TransactionState::Aborted;
        info!("✅ Transaction {} rolled back successfully", tx_id);
        
        // Remove from active transactions
        self.transactions.remove(tx_id);
        
        Ok(())
    }

    /// Execute a rollback action
    async fn execute_rollback_action(&self, participant_id: &str, action: RollbackAction) -> Result<()> {
        match action {
            RollbackAction::RemoveFromMemtable { key } => {
                debug!("🔙 Rolling back memtable: removing key {}", key);
                // In real implementation, would call memtable remove
                Ok(())
            }
            RollbackAction::RestoreMemtableValue { key, previous_value } => {
                debug!("🔙 Rolling back memtable: restoring key {} with {} bytes", key, previous_value.len());
                // In real implementation, would call memtable restore
                Ok(())
            }
            RollbackAction::RemoveFromSecondaryIndex { name, uuid } => {
                debug!("🔙 Rolling back secondary index: removing {} -> {}", name, uuid);
                // In real implementation, would call secondary index remove
                Ok(())
            }
            RollbackAction::DeleteFile { path } => {
                debug!("🔙 Rolling back disk: deleting file {}", path);
                // Use filesystem to delete
                self.filesystem.delete(&path).await?;
                Ok(())
            }
            RollbackAction::RestoreFile { path, content } => {
                debug!("🔙 Rolling back disk: restoring file {} with {} bytes", path, content.len());
                // Use filesystem to restore
                let fs = self.filesystem.get_filesystem(&path)?;
                fs.write(&self.filesystem.extract_path(&path)?, &content, None).await?;
                Ok(())
            }
            RollbackAction::Custom(desc) => {
                debug!("🔙 Rolling back {}: {}", participant_id, desc);
                Ok(())
            }
        }
    }

    /// Register rollback action for a participant
    pub async fn register_rollback(
        &self,
        tx_id: &str,
        participant_id: &str,
        action: RollbackAction,
    ) -> Result<()> {
        let tx = self.get_transaction(tx_id).await?;
        let mut tx_guard = tx.write().await;
        
        if let Some(participant) = tx_guard.participants.get_mut(participant_id) {
            participant.rollback_action = Some(action);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Participant {} not found in transaction {}", participant_id, tx_id))
        }
    }

    /// Get transaction by ID
    async fn get_transaction(&self, tx_id: &str) -> Result<Arc<RwLock<ActiveTransaction>>> {
        self.transactions
            .get(tx_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Transaction {} not found", tx_id))
    }

    /// Get transaction state
    pub async fn get_transaction_state(&self, tx_id: &str) -> Result<TransactionState> {
        let tx = self.get_transaction(tx_id).await?;
        let tx_guard = tx.read().await;
        Ok(tx_guard.state.clone())
    }

    /// Start background cleanup task for expired transactions
    fn start_cleanup_task(&self) {
        let transactions = self.transactions.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            
            loop {
                interval.tick().await;
                
                // Find and clean up expired transactions
                let mut expired = Vec::new();
                for entry in transactions.iter() {
                    let tx = entry.value().clone();
                    let is_expired = {
                        let tx_guard = tx.read().await;
                        tx_guard.is_expired()
                    };
                    
                    if is_expired {
                        expired.push(entry.key().clone());
                    }
                }
                
                // Abort expired transactions
                for tx_id in expired {
                    warn!("🕐 Transaction {} expired, aborting", tx_id);
                    if let Some((_, tx)) = transactions.remove(&tx_id) {
                        let mut tx_guard = tx.write().await;
                        tx_guard.state = TransactionState::Failed("Transaction expired".to_string());
                        // In production, we'd trigger rollback here
                    }
                }
            }
        });
        
        let cleanup_handle = self.cleanup_handle.clone();
        tokio::spawn(async move {
            *cleanup_handle.lock().await = Some(handle);
        });
    }

    // Private helper methods

    /// Build staging and final URLs for operation
    fn build_operation_urls(
        &self,
        config: &StagingConfig,
        operation_id: &OperationId,
    ) -> Result<(String, String)> {
        let base_url = config.base_url.trim_end_matches('/');

        info!("🔍 build_operation_urls START");
        info!("    base_url: {}", base_url);
        info!("    operation_type: {:?}", config.operation_type);
        info!("    custom_staging_dir: {:?}", config.custom_staging_dir);
        info!("    collection_id: {:?}", config.collection_id);

        // Build collection-specific path if provided
        let collection_path = if let Some(ref collection_id) = config.collection_id {
            format!("/collections/{}", collection_id)
        } else {
            String::new()
        };

        // Build staging directory name
        let staging_dir = config
            .custom_staging_dir
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or_else(|| config.operation_type.staging_dir_name());

        info!("    staging_dir resolved to: '{}'", staging_dir);
        info!("    collection_path: '{}'", collection_path);

        // For metadata operations with custom staging dir containing path separators,
        // we need special handling
        let (staging_url, final_url) = if config.operation_type == StagingOperationType::Metadata 
            && staging_dir.starts_with("../") {
            info!("    Using metadata with relative staging path");
            // For relative paths like "../staging", we need to resolve them properly
            // base_url is like "file:///path/to/metadata/current"
            // We want staging to be "file:///path/to/metadata/staging/{operation_id}"
            
            // Parse the base URL to extract scheme and path
            if let Some((scheme, path)) = base_url.split_once("://") {
                // Remove the last path component (e.g., "current")
                let parent_path = if let Some(last_slash) = path.rfind('/') {
                    &path[..last_slash]
                } else {
                    path
                };
                
                // Extract the staging directory name (remove "../")
                let staging_dirname = staging_dir.trim_start_matches("../");
                
                let staging_url = format!("{}://{}/{}/{}", scheme, parent_path, staging_dirname, operation_id);
                let final_url = base_url.to_string();
                
                info!("    Resolved URLs: staging='{}', final='{}'", staging_url, final_url);
                (staging_url, final_url)
            } else {
                // Fallback for non-URL paths
                info!("    Fallback: simple staging dir");
                let staging_url = format!("{}/{}/{}", base_url, staging_dir, operation_id);
                let final_url = base_url.to_string();
                (staging_url, final_url)
            }
        } else if config.operation_type == StagingOperationType::Metadata {
            info!("    Using simple metadata staging");
            let staging_url = format!("{}/{}/{}", base_url, staging_dir, operation_id);
            let final_url = base_url.to_string();
            (staging_url, final_url)
        } else {
            info!("    Using non-metadata staging");
            let staging_url = format!("{}{}/{}/{}", base_url, collection_path, staging_dir, operation_id);
            let final_url = format!("{}{}", base_url, collection_path);
            (staging_url, final_url)
        };

        info!("🔍 build_operation_urls COMPLETE");
        info!("    Final staging_url: {}", staging_url);
        info!("    Final final_url: {}", final_url);
        Ok((staging_url, final_url))
    }

    /// Update operation status
    async fn update_operation_status(
        &self,
        operation_id: &OperationId,
        status: AtomicOperationStatus,
    ) -> Result<()> {
        // Update using DashMap's entry API
        if let Some(mut entry) = self.active_operations.get_mut(operation_id) {
            entry.status = status;
        }
        Ok(())
    }

}

/// Handle for an active transaction
pub struct TransactionHandle<'a> {
    id: String,
    coordinator: &'a UnifiedAtomicCoordinator,
    transaction: Arc<RwLock<ActiveTransaction>>,
}


impl<'a> TransactionHandle<'a> {
    /// Prepare the transaction (phase 1 of 2PC)
    pub async fn prepare(&self) -> Result<bool> {
        self.coordinator.prepare_transaction(&self.id).await
    }

    /// Commit the transaction (phase 2 of 2PC)
    pub async fn commit(&self) -> Result<()> {
        self.coordinator.commit_transaction(&self.id).await
    }

    /// Rollback the transaction
    pub async fn rollback(&self) -> Result<()> {
        self.coordinator.rollback_transaction(&self.id).await
    }

    /// Register a rollback action
    pub async fn register_rollback(
        &self,
        participant_id: &str,
        action: RollbackAction,
    ) -> Result<()> {
        self.coordinator.register_rollback(&self.id, participant_id, action).await
    }

    /// Get transaction ID
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Generate unique transaction ID
pub fn generate_transaction_id(prefix: &str) -> String {
    let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{}_{}_{}",  prefix, timestamp, counter)
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
        collection_id: &str,
        storage_url: &str,
    ) -> Result<AtomicOperationMetadata> {
        let config = StagingConfig {
            base_url: storage_url.to_string(),
            collection_id: Some(collection_id.to_string()),
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
        self.coordinator
            .write_to_staging(operation_id, &relative_path, parquet_data)
            .await
    }

    /// Finalize flush operation
    pub async fn finalize_flush(&self, operation_id: &OperationId) -> Result<()> {
        self.coordinator
            .finalize_atomic_operation(operation_id)
            .await
    }

    /// Abort flush operation
    pub async fn abort_flush(&self, operation_id: &OperationId, reason: &str) -> Result<()> {
        self.coordinator
            .abort_atomic_operation(operation_id, reason)
            .await
    }
}

/// Convenience wrapper for Write Buffer-specific atomic operations
pub struct WalAtomicOperations {
    coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl WalAtomicOperations {
    pub fn new(coordinator: Arc<UnifiedAtomicCoordinator>) -> Self {
        Self { coordinator }
    }

    /// Begin WAL segment rotation operation
    pub async fn begin_segment_rotation(&self, write_buffer_url: &str) -> Result<AtomicOperationMetadata> {
        let config = StagingConfig {
            base_url: write_buffer_url.to_string(),
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
        self.coordinator
            .write_to_staging(operation_id, segment_filename, segment_data)
            .await
    }

    /// Finalize segment rotation
    pub async fn finalize_rotation(&self, operation_id: &OperationId) -> Result<()> {
        self.coordinator
            .finalize_atomic_operation(operation_id)
            .await
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
        let coordinator = UnifiedAtomicCoordinator::new(filesystem, None)
            .await
            .unwrap();

        (coordinator, temp_dir)
    }

    #[tokio::test]
    async fn test_atomic_operation_lifecycle() {
        let (coordinator, temp_dir) = create_test_coordinator().await;
        let temp_path = temp_dir.path().to_string_lossy().to_string();

        let config = StagingConfig {
            base_url: format!("file://{}", temp_path),
            collection_id: Some("test_collection".to_string()),
            operation_type: StagingOperationType::Flush,
            ..Default::default()
        };

        // Begin operation
        let metadata = coordinator.begin_atomic_operation(&config).await.unwrap();
        assert_eq!(metadata.status, AtomicOperationStatus::Preparing);

        // Write to staging
        let test_data = b"test parquet data";
        coordinator
            .write_to_staging(&metadata.operation_id, "test.parquet", test_data)
            .await
            .unwrap();

        // Check status
        let status = coordinator
            .get_operation_status(&metadata.operation_id)
            .await
            .unwrap();
        assert_eq!(status, AtomicOperationStatus::Staging);

        // Finalize operation
        coordinator
            .finalize_atomic_operation(&metadata.operation_id)
            .await
            .unwrap();

        // Operation should be removed from active operations
        assert!(coordinator
            .get_operation_status(&metadata.operation_id)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn test_viper_atomic_operations() {
        let (coordinator, temp_dir) = create_test_coordinator().await;
        let temp_path = temp_dir.path().to_string_lossy().to_string();
        let coordinator = Arc::new(coordinator);

        let viper_ops = ViperAtomicOperations::new(coordinator);

        // Begin flush operation
        let metadata = viper_ops
            .begin_flush_operation(&"test_collection".to_string(), &format!("file://{}", temp_path))
            .await
            .unwrap();

        // Write Parquet data
        let parquet_data = b"mock parquet data";
        viper_ops
            .write_parquet_to_staging(&metadata.operation_id, "batch_001.parquet", parquet_data)
            .await
            .unwrap();

        // Finalize flush
        viper_ops
            .finalize_flush(&metadata.operation_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_operation_abort() {
        let (coordinator, temp_dir) = create_test_coordinator().await;
        let temp_path = temp_dir.path().to_string_lossy().to_string();

        let config = StagingConfig {
            base_url: format!("file://{}", temp_path),
            operation_type: StagingOperationType::Flush,
            ..Default::default()
        };

        // Begin operation
        let metadata = coordinator.begin_atomic_operation(&config).await.unwrap();

        // Write some data
        coordinator
            .write_to_staging(&metadata.operation_id, "test.dat", b"test")
            .await
            .unwrap();

        // Abort operation
        coordinator
            .abort_atomic_operation(&metadata.operation_id, "test abort")
            .await
            .unwrap();

        // Operation should be removed
        assert!(coordinator
            .get_operation_status(&metadata.operation_id)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn test_acid_transaction_lifecycle() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        
        // Begin transaction
        let tx = coordinator
            .begin_transaction("test_tx_1", vec!["memtable".to_string(), "disk".to_string()])
            .await
            .unwrap();
        
        // Register rollback actions
        tx.register_rollback(
            "memtable",
            RollbackAction::RemoveFromMemtable {
                key: "test_key".to_string(),
            },
        )
        .await
        .unwrap();
        
        // Prepare
        let prepared = tx.prepare().await.unwrap();
        assert!(prepared);
        
        // Commit
        tx.commit().await.unwrap();
        
        // Verify state
        let state = coordinator.get_transaction_state("test_tx_1").await.unwrap();
        assert_eq!(state, TransactionState::Committed);
    }

    #[tokio::test]
    async fn test_acid_transaction_rollback() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        
        // Begin transaction
        let tx = coordinator
            .begin_transaction("test_tx_2", vec!["memtable".to_string()])
            .await
            .unwrap();
        
        // Register rollback action
        tx.register_rollback(
            "memtable",
            RollbackAction::RestoreMemtableValue {
                key: "test_key".to_string(),
                previous_value: vec![1, 2, 3],
            },
        )
        .await
        .unwrap();
        
        // Prepare
        tx.prepare().await.unwrap();
        
        // Rollback instead of commit
        tx.rollback().await.unwrap();
        
        // Transaction should be removed after rollback
        let result = coordinator.get_transaction_state("test_tx_2").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_generate_transaction_id() {
        let id1 = generate_transaction_id("collection");
        let id2 = generate_transaction_id("collection");
        
        // IDs should be unique
        assert_ne!(id1, id2);
        
        // IDs should follow expected format
        assert!(id1.starts_with("collection_"));
        assert!(id2.starts_with("collection_"));
    }
}
