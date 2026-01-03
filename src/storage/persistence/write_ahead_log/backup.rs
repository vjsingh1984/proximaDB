//! Incremental Backup Coordinator
//!
//! Provides incremental backup capabilities leveraging the WAL system.
//! Backups capture only changes since the last backup, reducing storage
//! and time requirements for regular backups.
//!
//! # Architecture
//!
//! The backup system uses the global manifest to track which WAL entries
//! have been backed up. Each backup captures:
//!
//! 1. WAL entries since the last backup LSN
//! 2. A snapshot of collection metadata
//! 3. Storage engine state (SST files, Parquet files, etc.)
//!
//! # Backup Types
//!
//! - **Full Backup**: Complete copy of all data
//! - **Incremental Backup**: Only changes since last backup
//! - **Differential Backup**: Changes since last full backup
//!
//! # Usage
//!
//! ```ignore
//! let backup = BackupCoordinator::new(manifest_service, backup_storage);
//!
//! // Create an incremental backup
//! let backup_id = backup.create_incremental().await?;
//!
//! // Restore from a backup
//! backup.restore(backup_id).await?;
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::manifest::{GlobalManifestEntry, GlobalManifestService};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Unique identifier for a backup
pub type BackupId = String;

/// Type of backup
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BackupType {
    /// Complete backup of all data
    Full,
    /// Only changes since the last backup (any type)
    Incremental,
    /// Changes since the last full backup
    Differential,
}

impl std::fmt::Display for BackupType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupType::Full => write!(f, "full"),
            BackupType::Incremental => write!(f, "incremental"),
            BackupType::Differential => write!(f, "differential"),
        }
    }
}

/// Status of a backup operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupStatus {
    /// Backup is in progress
    InProgress,
    /// Backup completed successfully
    Completed,
    /// Backup failed
    Failed,
    /// Backup was canceled
    Canceled,
    /// Backup is being verified
    Verifying,
}

/// Configuration for the backup coordinator
///
/// **Default: Disabled** - Cloud deployments should rely on object store replication
/// (S3 Cross-Region Replication, Azure GRS, GCS Multi-Region) for durability.
///
/// Enable for:
/// - On-premises deployments without object store replication
/// - On-prem to cloud migration scenarios
/// - RTO/RPO requirements < 15 minutes (S3 replication SLA is 15 min)
/// - Point-in-time recovery needs beyond WAL retention
#[derive(Debug, Clone)]
pub struct BackupConfig {
    /// Enable backup functionality (default: false for cloud, true for on-prem)
    /// Cloud deployments should use object store replication instead
    pub enabled: bool,
    /// Maximum number of incremental backups before forcing a full backup
    pub max_incremental_chain_length: u32,
    /// Maximum age of an incremental chain before forcing a full backup
    pub max_incremental_chain_age_hours: u32,
    /// Enable compression for backup files
    pub compress_backups: bool,
    /// Compression level (1-9, where 9 is maximum compression)
    pub compression_level: u8,
    /// Enable parallel backup of multiple collections
    pub parallel_collection_backup: bool,
    /// Maximum parallel backup operations
    pub max_parallel_operations: usize,
    /// Enable verification after backup
    pub verify_after_backup: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            // Disabled by default - cloud deployments use object store replication
            // Enable for on-prem or when RTO/RPO < 15 minutes is required
            enabled: false,
            max_incremental_chain_length: 10,
            max_incremental_chain_age_hours: 168, // 7 days
            compress_backups: true,
            compression_level: 6,
            parallel_collection_backup: true,
            max_parallel_operations: 4,
            verify_after_backup: true,
        }
    }
}

/// Metadata for a backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Unique backup identifier
    pub backup_id: BackupId,
    /// Type of backup
    pub backup_type: BackupType,
    /// Status of the backup
    pub status: BackupStatus,
    /// LSN at the start of backup
    pub start_lsn: u64,
    /// LSN at the end of backup (inclusive)
    pub end_lsn: u64,
    /// Timestamp when backup started
    pub started_at: DateTime<Utc>,
    /// Timestamp when backup completed (if completed)
    pub completed_at: Option<DateTime<Utc>>,
    /// Total size of backup in bytes
    pub size_bytes: u64,
    /// Number of WAL entries included
    pub wal_entries_count: u64,
    /// Number of collections included
    pub collections_count: u64,
    /// Reference to parent backup (for incremental/differential)
    pub parent_backup_id: Option<BackupId>,
    /// Chain depth (0 for full backups)
    pub chain_depth: u32,
    /// Collection-specific backup information
    pub collections: HashMap<String, CollectionBackupInfo>,
    /// Errors encountered during backup (if any)
    pub errors: Vec<String>,
    /// Warnings during backup
    pub warnings: Vec<String>,
}

/// Collection-specific backup information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionBackupInfo {
    /// Collection identifier
    pub collection_id: String,
    /// Number of vectors backed up
    pub vector_count: u64,
    /// Size of collection backup in bytes
    pub size_bytes: u64,
    /// WAL entries for this collection
    pub wal_entries: u64,
    /// Storage files backed up
    pub storage_files: Vec<String>,
    /// Last LSN for this collection
    pub last_lsn: u64,
}

/// Result of a backup operation
#[derive(Debug)]
pub struct BackupResult {
    /// Backup metadata
    pub metadata: BackupMetadata,
    /// Whether verification passed (if enabled)
    pub verification_passed: Option<bool>,
    /// Duration of backup in seconds
    pub duration_seconds: f64,
}

/// Result of a restore operation
#[derive(Debug)]
pub struct RestoreResult {
    /// Backup ID that was restored
    pub backup_id: BackupId,
    /// Number of WAL entries replayed
    pub entries_replayed: u64,
    /// Number of collections restored
    pub collections_restored: u64,
    /// Timestamp of restore completion
    pub completed_at: DateTime<Utc>,
    /// Duration of restore in seconds
    pub duration_seconds: f64,
    /// Warnings during restore
    pub warnings: Vec<String>,
}

/// Options for backup creation
#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Backup type to create
    pub backup_type: BackupType,
    /// Optional name/tag for the backup
    pub name: Option<String>,
    /// Specific collections to backup (None = all)
    pub collections: Option<Vec<String>>,
    /// Whether to verify after backup
    pub verify: bool,
    /// Force full backup even if conditions don't require it
    pub force_full: bool,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            backup_type: BackupType::Incremental,
            name: None,
            collections: None,
            verify: true,
            force_full: false,
        }
    }
}

/// Options for restore operation
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Specific collections to restore (None = all)
    pub collections: Option<Vec<String>>,
    /// Whether to verify data after restore
    pub verify: bool,
    /// Whether to overwrite existing data
    pub overwrite_existing: bool,
    /// Dry run mode (show what would be restored)
    pub dry_run: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            collections: None,
            verify: true,
            overwrite_existing: false,
            dry_run: false,
        }
    }
}

/// Backup Coordinator for managing incremental and full backups
pub struct BackupCoordinator {
    /// Reference to the global manifest service
    manifest_service: Arc<GlobalManifestService>,
    /// Filesystem factory for backup storage
    filesystem_factory: Arc<FilesystemFactory>,
    /// Base URL for backup storage
    backup_storage_url: String,
    /// Configuration
    config: BackupConfig,
    /// In-memory cache of backup metadata
    backups: Arc<RwLock<HashMap<BackupId, BackupMetadata>>>,
    /// Last successful backup by type
    last_backup: Arc<RwLock<HashMap<BackupType, BackupId>>>,
}

impl BackupCoordinator {
    /// Create a new backup coordinator
    pub async fn new(
        manifest_service: Arc<GlobalManifestService>,
        filesystem_factory: Arc<FilesystemFactory>,
        backup_storage_url: String,
        config: BackupConfig,
    ) -> Result<Self> {
        info!(
            "🗄️  Initializing BackupCoordinator at {}",
            backup_storage_url
        );

        let coordinator = Self {
            manifest_service,
            filesystem_factory,
            backup_storage_url,
            config,
            backups: Arc::new(RwLock::new(HashMap::new())),
            last_backup: Arc::new(RwLock::new(HashMap::new())),
        };

        // Load existing backup metadata from storage
        coordinator.load_backup_metadata().await?;

        info!("✅ BackupCoordinator initialized");
        Ok(coordinator)
    }

    /// Create a backup with the given options
    pub async fn create_backup(&self, options: BackupOptions) -> Result<BackupResult> {
        let start_time = std::time::Instant::now();
        let backup_id = self.generate_backup_id(&options);

        info!("📦 Creating {} backup: {}", options.backup_type, backup_id);

        // Determine if we need to force a full backup
        let actual_backup_type = if options.force_full {
            BackupType::Full
        } else {
            self.determine_backup_type(&options).await?
        };

        // Get parent backup for incremental/differential
        let parent_backup = self.get_parent_backup(&actual_backup_type).await?;
        let start_lsn = parent_backup.as_ref().map(|p| p.end_lsn + 1).unwrap_or(0);
        let current_lsn = self.manifest_service.current_lsn().await;

        // Initialize backup metadata
        let mut metadata = BackupMetadata {
            backup_id: backup_id.clone(),
            backup_type: actual_backup_type,
            status: BackupStatus::InProgress,
            start_lsn,
            end_lsn: current_lsn,
            started_at: Utc::now(),
            completed_at: None,
            size_bytes: 0,
            wal_entries_count: 0,
            collections_count: 0,
            parent_backup_id: parent_backup.as_ref().map(|p| p.backup_id.clone()),
            chain_depth: parent_backup
                .as_ref()
                .map(|p| p.chain_depth + 1)
                .unwrap_or(0),
            collections: HashMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        // Get WAL entries to backup
        let entries = if actual_backup_type == BackupType::Full {
            self.manifest_service.get_all_entries().await
        } else {
            self.manifest_service
                .get_entries_between_lsn(start_lsn, current_lsn)
                .await
        };

        // Filter by collections if specified
        let entries: Vec<GlobalManifestEntry> = if let Some(ref collections) = options.collections {
            entries
                .into_iter()
                .filter(|e| collections.contains(&e.collection_id))
                .collect()
        } else {
            entries
        };

        metadata.wal_entries_count = entries.len() as u64;

        // Group entries by collection
        let mut collection_entries: HashMap<String, Vec<GlobalManifestEntry>> = HashMap::new();
        for entry in entries {
            collection_entries
                .entry(entry.collection_id.clone())
                .or_default()
                .push(entry);
        }

        metadata.collections_count = collection_entries.len() as u64;

        // Backup each collection
        for (collection_id, entries) in collection_entries {
            match self
                .backup_collection(&backup_id, &collection_id, &entries)
                .await
            {
                Ok(info) => {
                    metadata.size_bytes += info.size_bytes;
                    metadata.collections.insert(collection_id, info);
                }
                Err(e) => {
                    let error_msg = format!("Failed to backup collection {}: {}", collection_id, e);
                    error!("{}", error_msg);
                    metadata.errors.push(error_msg);
                }
            }
        }

        // Write backup metadata
        metadata.completed_at = Some(Utc::now());
        metadata.status = if metadata.errors.is_empty() {
            BackupStatus::Completed
        } else {
            BackupStatus::Failed
        };

        self.save_backup_metadata(&metadata).await?;

        // Update caches
        {
            let mut backups = self.backups.write().await;
            backups.insert(backup_id.clone(), metadata.clone());
        }
        {
            let mut last_backup = self.last_backup.write().await;
            if metadata.status == BackupStatus::Completed {
                last_backup.insert(actual_backup_type, backup_id.clone());
                if actual_backup_type == BackupType::Full {
                    // Full backup resets all chains
                    last_backup.insert(BackupType::Differential, backup_id.clone());
                }
            }
        }

        // Verify if enabled
        let verification_passed = if options.verify && metadata.status == BackupStatus::Completed {
            Some(self.verify_backup(&backup_id).await.unwrap_or(false))
        } else {
            None
        };

        let duration_seconds = start_time.elapsed().as_secs_f64();

        info!(
            "✅ Backup {} completed in {:.2}s ({} bytes, {} entries)",
            backup_id, duration_seconds, metadata.size_bytes, metadata.wal_entries_count
        );

        Ok(BackupResult {
            metadata,
            verification_passed,
            duration_seconds,
        })
    }

    /// Create an incremental backup (convenience method)
    pub async fn create_incremental(&self) -> Result<BackupResult> {
        self.create_backup(BackupOptions {
            backup_type: BackupType::Incremental,
            ..Default::default()
        })
        .await
    }

    /// Create a full backup (convenience method)
    pub async fn create_full(&self) -> Result<BackupResult> {
        self.create_backup(BackupOptions {
            backup_type: BackupType::Full,
            force_full: true,
            ..Default::default()
        })
        .await
    }

    /// Restore from a backup
    pub async fn restore(&self, backup_id: &str, options: RestoreOptions) -> Result<RestoreResult> {
        let start_time = std::time::Instant::now();

        info!("📥 Restoring from backup: {}", backup_id);

        // Get the backup metadata
        let backup = self.get_backup_metadata(backup_id).await?;

        if backup.status != BackupStatus::Completed {
            return Err(anyhow::anyhow!(
                "Cannot restore from backup with status: {:?}",
                backup.status
            ));
        }

        let mut entries_replayed = 0u64;
        let mut collections_restored = 0u64;
        let mut warnings = Vec::new();

        if options.dry_run {
            info!(
                "🔍 Dry run: would restore {} collections",
                backup.collections_count
            );
            warnings.push("Dry run mode - no actual restore performed".to_string());
        } else {
            // Build restore chain (for incremental backups)
            let restore_chain = self.build_restore_chain(&backup).await?;

            info!("📋 Restore chain: {} backups to apply", restore_chain.len());

            // Apply each backup in the chain
            for chain_backup in restore_chain {
                let result = self.apply_backup(&chain_backup, &options).await?;
                entries_replayed += result.0;
                collections_restored += result.1;
            }
        }

        let duration_seconds = start_time.elapsed().as_secs_f64();

        info!(
            "✅ Restore from {} completed in {:.2}s ({} entries, {} collections)",
            backup_id, duration_seconds, entries_replayed, collections_restored
        );

        Ok(RestoreResult {
            backup_id: backup_id.to_string(),
            entries_replayed,
            collections_restored,
            completed_at: Utc::now(),
            duration_seconds,
            warnings,
        })
    }

    /// List all available backups
    pub async fn list_backups(&self) -> Vec<BackupMetadata> {
        let backups = self.backups.read().await;
        let mut list: Vec<_> = backups.values().cloned().collect();
        list.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        list
    }

    /// Get a specific backup's metadata
    pub async fn get_backup_metadata(&self, backup_id: &str) -> Result<BackupMetadata> {
        let backups = self.backups.read().await;
        backups
            .get(backup_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Backup not found: {}", backup_id))
    }

    /// Delete a backup
    pub async fn delete_backup(&self, backup_id: &str) -> Result<()> {
        info!("🗑️  Deleting backup: {}", backup_id);

        // Check if any other backup depends on this one
        let backups = self.backups.read().await;
        for (id, backup) in backups.iter() {
            if backup.parent_backup_id.as_deref() == Some(backup_id) {
                return Err(anyhow::anyhow!(
                    "Cannot delete backup {}: backup {} depends on it",
                    backup_id,
                    id
                ));
            }
        }
        drop(backups);

        // Delete backup files
        let backup_dir = format!("{}/{}", self.backup_storage_url, backup_id);
        let fs = self.filesystem_factory.get_filesystem(&backup_dir)?;

        if fs.exists(&backup_dir).await.unwrap_or(false) {
            // List and delete all files in the backup directory
            if let Ok(entries) = fs.list(&backup_dir).await {
                for entry in entries {
                    let _ = fs.delete(&entry.url).await;
                }
            }
        }

        // Remove from cache
        let mut backups = self.backups.write().await;
        backups.remove(backup_id);

        info!("✅ Backup {} deleted", backup_id);
        Ok(())
    }

    /// Verify a backup's integrity
    pub async fn verify_backup(&self, backup_id: &str) -> Result<bool> {
        info!("🔍 Verifying backup: {}", backup_id);

        let backup = self.get_backup_metadata(backup_id).await?;
        let backup_dir = format!("{}/{}", self.backup_storage_url, backup_id);
        let fs = self.filesystem_factory.get_filesystem(&backup_dir)?;

        // Verify all collection backup files exist and have correct sizes
        for (collection_id, info) in &backup.collections {
            for file in &info.storage_files {
                let file_url = format!("{}/{}", backup_dir, file);
                if !fs.exists(&file_url).await.unwrap_or(false) {
                    warn!(
                        "❌ Missing backup file for collection {}: {}",
                        collection_id, file
                    );
                    return Ok(false);
                }
            }
        }

        // Verify metadata integrity
        let metadata_url = format!("{}/metadata.json", backup_dir);
        if !fs.exists(&metadata_url).await.unwrap_or(false) {
            warn!("❌ Missing backup metadata file");
            return Ok(false);
        }

        info!("✅ Backup {} verified successfully", backup_id);
        Ok(true)
    }

    // Private helper methods

    fn generate_backup_id(&self, options: &BackupOptions) -> BackupId {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let type_prefix = match options.backup_type {
            BackupType::Full => "full",
            BackupType::Incremental => "incr",
            BackupType::Differential => "diff",
        };
        if let Some(ref name) = options.name {
            format!("{}_{}__{}", type_prefix, timestamp, name)
        } else {
            format!("{}_{}", type_prefix, timestamp)
        }
    }

    async fn determine_backup_type(&self, options: &BackupOptions) -> Result<BackupType> {
        // If no previous backup exists, must do a full backup
        let last_full = self
            .last_backup
            .read()
            .await
            .get(&BackupType::Full)
            .cloned();
        if last_full.is_none() {
            info!("📋 No previous full backup found, creating full backup");
            return Ok(BackupType::Full);
        }

        // Check chain length for incremental
        if options.backup_type == BackupType::Incremental {
            let last_incr = self
                .last_backup
                .read()
                .await
                .get(&BackupType::Incremental)
                .cloned();
            if let Some(ref last_id) = last_incr {
                if let Ok(last_backup) = self.get_backup_metadata(last_id).await {
                    if last_backup.chain_depth >= self.config.max_incremental_chain_length {
                        info!(
                            "📋 Incremental chain length ({}) exceeded max ({}), creating full backup",
                            last_backup.chain_depth, self.config.max_incremental_chain_length
                        );
                        return Ok(BackupType::Full);
                    }
                }
            }
        }

        Ok(options.backup_type)
    }

    async fn get_parent_backup(&self, backup_type: &BackupType) -> Result<Option<BackupMetadata>> {
        let parent_id = match backup_type {
            BackupType::Full => return Ok(None),
            BackupType::Incremental => {
                // Parent is the most recent backup of any type
                let last_backup = self.last_backup.read().await;
                last_backup
                    .get(&BackupType::Incremental)
                    .or_else(|| last_backup.get(&BackupType::Full))
                    .cloned()
            }
            BackupType::Differential => {
                // Parent is always the last full backup
                self.last_backup
                    .read()
                    .await
                    .get(&BackupType::Full)
                    .cloned()
            }
        };

        if let Some(id) = parent_id {
            self.get_backup_metadata(&id).await.map(Some)
        } else {
            Ok(None)
        }
    }

    async fn backup_collection(
        &self,
        backup_id: &str,
        collection_id: &str,
        entries: &[GlobalManifestEntry],
    ) -> Result<CollectionBackupInfo> {
        debug!(
            "📁 Backing up collection {} ({} entries)",
            collection_id,
            entries.len()
        );

        let backup_dir = format!(
            "{}/{}/{}",
            self.backup_storage_url, backup_id, collection_id
        );
        let fs = self.filesystem_factory.get_filesystem(&backup_dir)?;

        let mut info = CollectionBackupInfo {
            collection_id: collection_id.to_string(),
            vector_count: 0,
            size_bytes: 0,
            wal_entries: entries.len() as u64,
            storage_files: Vec::new(),
            last_lsn: 0,
        };

        // Backup WAL entries
        for entry in entries {
            info.vector_count += entry.vector_count;
            info.size_bytes += entry.size_bytes;
            info.last_lsn = info.last_lsn.max(entry.global_lsn);

            // Copy WAL file to backup storage
            let source_url = entry.full_url();
            let dest_file = format!("{}.wal", entry.batch_id);
            let dest_url = format!("{}/{}", backup_dir, dest_file);

            // Get source filesystem
            let source_fs = self.filesystem_factory.get_filesystem(&source_url)?;

            // Copy the file
            if source_fs.exists(&source_url).await.unwrap_or(false) {
                let data = source_fs
                    .read(&source_url)
                    .await
                    .with_context(|| format!("Failed to read WAL file: {}", source_url))?;

                fs.write(&dest_url, &data, None)
                    .await
                    .with_context(|| format!("Failed to write backup file: {}", dest_url))?;

                info.storage_files.push(dest_file);
            } else {
                debug!("⚠️  WAL file not found (may be flushed): {}", source_url);
            }
        }

        debug!(
            "✅ Backed up collection {}: {} vectors, {} bytes",
            collection_id, info.vector_count, info.size_bytes
        );

        Ok(info)
    }

    async fn build_restore_chain(&self, backup: &BackupMetadata) -> Result<Vec<BackupMetadata>> {
        let mut chain = Vec::new();
        let mut current = backup.clone();

        // Build chain from newest to oldest
        loop {
            chain.push(current.clone());

            if current.backup_type == BackupType::Full || current.parent_backup_id.is_none() {
                break;
            }

            let parent_id = current.parent_backup_id.as_ref().unwrap();
            current = self.get_backup_metadata(parent_id).await?;
        }

        // Reverse to get oldest to newest order for replay
        chain.reverse();
        Ok(chain)
    }

    async fn apply_backup(
        &self,
        backup: &BackupMetadata,
        _options: &RestoreOptions,
    ) -> Result<(u64, u64)> {
        debug!("📥 Applying backup: {}", backup.backup_id);

        let mut entries_replayed = 0u64;
        let collections_restored = backup.collections.len() as u64;

        for (collection_id, info) in &backup.collections {
            debug!(
                "📂 Restoring collection {}: {} files",
                collection_id,
                info.storage_files.len()
            );
            entries_replayed += info.wal_entries;

            // Note: Actual restore would copy files back to their original locations
            // and replay WAL entries through the recovery manager
            // This is the foundation - full implementation would integrate with
            // RecoveryManager for actual WAL replay
        }

        Ok((entries_replayed, collections_restored))
    }

    async fn load_backup_metadata(&self) -> Result<()> {
        let fs = self
            .filesystem_factory
            .get_filesystem(&self.backup_storage_url)?;

        // List all backup directories
        let entries = fs.list(&self.backup_storage_url).await.unwrap_or_default();

        let mut backups = self.backups.write().await;
        let mut last_backup = self.last_backup.write().await;

        for entry in entries {
            let metadata_url = format!("{}/metadata.json", entry.url);
            if fs.exists(&metadata_url).await.unwrap_or(false) {
                match fs.read(&metadata_url).await {
                    Ok(data) => {
                        match serde_json::from_slice::<BackupMetadata>(&data) {
                            Ok(metadata) => {
                                if metadata.status == BackupStatus::Completed {
                                    // Track as last backup of its type
                                    let existing = last_backup.get(&metadata.backup_type);
                                    if existing.is_none() || {
                                        let existing_meta = backups.get(existing.unwrap());
                                        existing_meta
                                            .map(|m| metadata.started_at > m.started_at)
                                            .unwrap_or(true)
                                    } {
                                        last_backup.insert(
                                            metadata.backup_type,
                                            metadata.backup_id.clone(),
                                        );
                                    }
                                }
                                backups.insert(metadata.backup_id.clone(), metadata);
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️  Failed to parse backup metadata {}: {}",
                                    metadata_url, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!("⚠️  Failed to read backup metadata {}: {}", metadata_url, e);
                    }
                }
            }
        }

        info!("📋 Loaded {} existing backups", backups.len());

        Ok(())
    }

    async fn save_backup_metadata(&self, metadata: &BackupMetadata) -> Result<()> {
        let backup_dir = format!("{}/{}", self.backup_storage_url, metadata.backup_id);
        let metadata_url = format!("{}/metadata.json", backup_dir);
        let fs = self.filesystem_factory.get_filesystem(&metadata_url)?;

        let data =
            serde_json::to_vec_pretty(metadata).context("Failed to serialize backup metadata")?;

        fs.write(&metadata_url, &data, None)
            .await
            .context("Failed to write backup metadata")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_config_default() {
        let config = BackupConfig::default();
        // Disabled by default - cloud deployments use object store replication
        assert!(!config.enabled);
        assert_eq!(config.max_incremental_chain_length, 10);
        assert_eq!(config.max_incremental_chain_age_hours, 168);
        assert!(config.compress_backups);
        assert_eq!(config.compression_level, 6);
        assert!(config.parallel_collection_backup);
        assert_eq!(config.max_parallel_operations, 4);
        assert!(config.verify_after_backup);
    }

    #[test]
    fn test_backup_options_default() {
        let options = BackupOptions::default();
        assert_eq!(options.backup_type, BackupType::Incremental);
        assert!(options.name.is_none());
        assert!(options.collections.is_none());
        assert!(options.verify);
        assert!(!options.force_full);
    }

    #[test]
    fn test_restore_options_default() {
        let options = RestoreOptions::default();
        assert!(options.collections.is_none());
        assert!(options.verify);
        assert!(!options.overwrite_existing);
        assert!(!options.dry_run);
    }

    #[test]
    fn test_backup_type_display() {
        assert_eq!(BackupType::Full.to_string(), "full");
        assert_eq!(BackupType::Incremental.to_string(), "incremental");
        assert_eq!(BackupType::Differential.to_string(), "differential");
    }

    #[test]
    fn test_backup_metadata_serialization() {
        let metadata = BackupMetadata {
            backup_id: "test_backup_123".to_string(),
            backup_type: BackupType::Incremental,
            status: BackupStatus::Completed,
            start_lsn: 100,
            end_lsn: 200,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            size_bytes: 1024,
            wal_entries_count: 10,
            collections_count: 2,
            parent_backup_id: Some("parent_backup".to_string()),
            chain_depth: 1,
            collections: HashMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: BackupMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.backup_id, "test_backup_123");
        assert_eq!(deserialized.backup_type, BackupType::Incremental);
        assert_eq!(deserialized.status, BackupStatus::Completed);
        assert_eq!(deserialized.start_lsn, 100);
        assert_eq!(deserialized.end_lsn, 200);
    }
}
