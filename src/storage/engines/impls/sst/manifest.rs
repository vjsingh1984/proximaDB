/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SST Manifest for SSTable Tracking
//!
//! The manifest provides a centralized record of all SSTable files in the SST storage,
//! tracking their levels, sizes, key ranges, and metadata for efficient query planning
//! and compaction scheduling.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::transaction_coordinator::TransactionCoordinator;

/// SSTable file metadata in the manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableFileInfo {
    /// Unique identifier for this SSTable
    pub file_id: String,
    /// File path relative to collection directory
    pub file_path: String,
    /// Level in the SST storage (0 = newest)
    pub level: u8,
    /// File size in bytes
    pub size_bytes: u64,
    /// Number of records in the file
    pub record_count: u64,
    /// Minimum key in the file
    pub min_key: String,
    /// Maximum key in the file
    pub max_key: String,
    /// Creation timestamp
    pub timestamp: i64,
    /// Last compaction timestamp (if any)
    pub last_compacted_at: Option<i64>,
    /// Bloom filter false positive rate (actual)
    pub bloom_fpr: f64,
    /// Metadata column statistics
    pub metadata_columns: HashMap<String, ColumnStats>,
    /// Whether this file is marked for deletion
    pub marked_for_deletion: bool,
    /// Sequence number range
    pub min_sequence: u64,
    pub max_sequence: u64,
}

/// Column statistics for metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStats {
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
    pub null_count: u64,
    pub distinct_count_estimate: u64,
}

/// Manifest version information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestVersion {
    pub version: u64,
    pub timestamp: i64,
    pub files: BTreeMap<String, SstableFileInfo>,
}

/// SST Manifest for tracking SSTable files
pub struct SstManifest {
    /// Collection ID this manifest belongs to
    /// Current manifest version
    current_version: Arc<RwLock<ManifestVersion>>,
    /// Manifest file URL (includes scheme: file://, s3://, etc.)
    manifest_url: String,
    /// Filesystem for I/O
    filesystem: Arc<FilesystemFactory>,
    /// Atomic coordinator for safe updates
    #[allow(dead_code)]
    atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    /// Version history (limited to last N versions)
    version_history: Arc<RwLock<Vec<ManifestVersion>>>,
    /// Maximum versions to keep in history
    max_history_versions: usize,
}

impl SstManifest {
    /// Create a new manifest
    pub fn new(
        collection_id: String,
        storage_url: String,
        filesystem: Arc<FilesystemFactory>,
        atomic_coordinator: Option<Arc<TransactionCoordinator>>,
    ) -> Self {
        // Construct manifest URL by appending filename to storage URL
        let manifest_url = format!(
            "{}/{}_manifest.json",
            storage_url.trim_end_matches('/'),
            collection_id
        );
        tracing::info!("📋 Creating manifest at: {}", manifest_url);

        Self {
            current_version: Arc::new(RwLock::new(ManifestVersion {
                version: 0,
                timestamp: chrono::Utc::now().timestamp(),
                files: BTreeMap::new(),
            })),
            manifest_url,
            filesystem,
            atomic_coordinator,
            version_history: Arc::new(RwLock::new(Vec::new())),
            max_history_versions: 10,
        }
    }

    /// Load manifest from disk
    pub async fn load(&self) -> Result<()> {
        info!("🔍 Loading manifest from {}", self.manifest_url);

        // Get filesystem based on the URL scheme
        let fs = self.filesystem.get_filesystem(&self.manifest_url)?;

        match fs.read(&self.manifest_url).await {
            Ok(data) => {
                let manifest: ManifestVersion = serde_json::from_slice(&data)?;
                info!(
                    "✅ Loaded SST manifest version {} with {} files",
                    manifest.version,
                    manifest.files.len()
                );

                // Debug: print loaded files
                for (file_id, info) in &manifest.files {
                    debug!(
                        "  Loaded file: {} (level={}, records={})",
                        file_id, info.level, info.record_count
                    );
                }

                let mut current = self.current_version.write().await;
                *current = manifest;
                Ok(())
            }
            Err(e) => {
                info!(
                    "📋 No existing manifest found at {}, starting fresh: {}",
                    self.manifest_url, e
                );
                Ok(())
            }
        }
    }

    /// Save manifest to disk atomically
    pub async fn save(&self) -> Result<()> {
        let current = self.current_version.read().await;
        info!(
            "💾 Saving manifest v{} with {} files to {}",
            current.version,
            current.files.len(),
            self.manifest_url
        );

        // Debug: print all files in manifest
        for (file_id, info) in &current.files {
            debug!(
                "  - {}: level={}, records={}",
                file_id, info.level, info.record_count
            );
        }

        let data = serde_json::to_vec_pretty(&*current)?;

        // Get filesystem based on the URL scheme
        let fs = self.filesystem.get_filesystem(&self.manifest_url)?;

        // Always allow overwrite for manifest updates
        let write_options = crate::storage::persistence::filesystem::FileOptions {
            overwrite: true,
            ..Default::default()
        };

        // Write to manifest URL with overwrite enabled
        fs.write(&self.manifest_url, &data, Some(write_options))
            .await?;

        debug!(
            "Saved manifest version {} to {} (overwrite mode)",
            current.version, self.manifest_url
        );
        Ok(())
    }

    /// Add a new SSTable file to the manifest
    pub async fn add_sstable(&self, file_info: SstableFileInfo) -> Result<()> {
        tracing::info!(
            "📋 Adding SSTable to manifest: file_id={}, path={}, records={}",
            file_info.file_id,
            file_info.file_path,
            file_info.record_count
        );
        let mut current = self.current_version.write().await;
        let mut history = self.version_history.write().await;

        // Save current version to history
        history.push(current.clone());
        if history.len() > self.max_history_versions {
            history.remove(0);
        }

        // Update version
        current.version += 1;
        current.timestamp = chrono::Utc::now().timestamp();
        current
            .files
            .insert(file_info.file_id.clone(), file_info.clone());

        info!(
            "Added SSTable {} to manifest (version {})",
            file_info.file_id, current.version
        );

        drop(current);
        drop(history);

        // Save to disk
        self.save().await?;
        Ok(())
    }

    /// Remove SSTable files from the manifest (after compaction)
    pub async fn remove_sstables(&self, file_ids: &[String]) -> Result<()> {
        let mut current = self.current_version.write().await;
        let mut history = self.version_history.write().await;

        // Save current version to history
        history.push(current.clone());
        if history.len() > self.max_history_versions {
            history.remove(0);
        }

        // Update version
        current.version += 1;
        current.timestamp = chrono::Utc::now().timestamp();

        for file_id in file_ids {
            if let Some(removed) = current.files.remove(file_id) {
                debug!("Removed SSTable {} from manifest", removed.file_id);
            }
        }

        info!(
            "Removed {} SSTables from manifest (version {})",
            file_ids.len(),
            current.version
        );

        drop(current);
        drop(history);

        // Save to disk
        self.save().await?;
        Ok(())
    }

    /// Mark files for deletion (soft delete)
    pub async fn mark_for_deletion(&self, file_ids: &[String]) -> Result<()> {
        let mut current = self.current_version.write().await;

        for file_id in file_ids {
            if let Some(file_info) = current.files.get_mut(file_id) {
                file_info.marked_for_deletion = true;
                debug!("Marked SSTable {} for deletion_info", file_id);
            }
        }

        drop(current);
        self.save().await?;
        Ok(())
    }

    /// Get all SSTable files at a specific level
    pub async fn files_at_level(&self, level: u8) -> Vec<SstableFileInfo> {
        let current = self.current_version.read().await;
        current
            .files
            .values()
            .filter(|f| f.level == level && !f.marked_for_deletion)
            .cloned()
            .collect()
    }

    /// Get SSTable files that overlap with a key range
    pub async fn overlapping_files(&self, min_key: &str, max_key: &str) -> Vec<SstableFileInfo> {
        let current = self.current_version.read().await;
        current
            .files
            .values()
            .filter(|f| {
                !f.marked_for_deletion
                    && !(f.max_key < min_key.to_string() || f.min_key > max_key.to_string())
            })
            .cloned()
            .collect()
    }

    /// Get files that might contain specific metadata values
    pub async fn files_with_metadata(
        &self,
        column: &str,
        value: &serde_json::Value,
    ) -> Vec<SstableFileInfo> {
        let current = self.current_version.read().await;
        current
            .files
            .values()
            .filter(|f| {
                if f.marked_for_deletion {
                    return false;
                }

                if let Some(stats) = f.metadata_columns.get(column) {
                    // Check if value falls within the range
                    Self::value_in_range(value, &stats.min_value, &stats.max_value)
                } else {
                    // Column not present in this file
                    false
                }
            })
            .cloned()
            .collect()
    }

    /// Get compaction candidates based on level and size
    pub async fn compaction_candidates(
        &self,
        level: u8,
        threshold: usize,
    ) -> Vec<Vec<SstableFileInfo>> {
        let files = self.files_at_level(level).await;

        if files.len() < threshold {
            return vec![];
        }

        // Group files that can be compacted together
        // Simple // strategy removed -  group by overlapping key ranges
        let mut groups = Vec::new();
        let mut current_group = Vec::new();
        let mut current_max_key = String::new();

        for file in files {
            if current_group.is_empty() || file.min_key <= current_max_key {
                // File overlaps with current group
                if file.max_key > current_max_key {
                    current_max_key = file.max_key.clone();
                }
                current_group.push(file);

                if current_group.len() >= threshold {
                    groups.push(current_group);
                    current_group = Vec::new();
                    current_max_key = String::new();
                }
            } else {
                // Start new group
                if current_group.len() >= 2 {
                    groups.push(current_group);
                }
                current_group = vec![file.clone()];
                current_max_key = file.max_key.clone();
            }
        }

        // Don't forget the last group
        if current_group.len() >= 2 {
            groups.push(current_group);
        }

        groups
    }

    /// Get manifest statistics
    pub async fn stats(&self) -> ManifestStats {
        let current = self.current_version.read().await;
        let mut stats = ManifestStats::default();

        for file in current.files.values() {
            if file.marked_for_deletion {
                stats.files_marked_for_deletion += 1;
                continue;
            }

            stats.total_files += 1;
            stats.total_size_bytes += file.size_bytes;
            stats.total_records += file.record_count;

            stats
                .files_per_level
                .entry(file.level)
                .and_modify(|e| *e += 1)
                .or_insert(1);

            stats
                .size_per_level
                .entry(file.level)
                .and_modify(|e| *e += file.size_bytes)
                .or_insert(file.size_bytes);
        }

        stats.manifest_version = current.version;
        stats
    }

    /// Check if a value falls within a range
    fn value_in_range(
        value: &serde_json::Value,
        min: &serde_json::Value,
        max: &serde_json::Value,
    ) -> bool {
        use serde_json::Value;
        match (value, min, max) {
            (Value::Number(v), Value::Number(min_n), Value::Number(max_n)) => {
                let v_f = v.as_f64();
                let min_f = min_n.as_f64();
                let max_f = max_n.as_f64();
                v_f >= min_f && v_f <= max_f
            }
            (Value::String(v), Value::String(min_s), Value::String(max_s)) => {
                v >= min_s && v <= max_s
            }
            _ => true, // Can't determine, assume it might be in range
        }
    }
}

/// Manifest statistics
#[derive(Debug, Default)]
pub struct ManifestStats {
    pub manifest_version: u64,
    pub total_files: usize,
    pub total_size_bytes: u64,
    pub total_records: u64,
    pub files_per_level: HashMap<u8, usize>,
    pub size_per_level: HashMap<u8, u64>,
    pub files_marked_for_deletion: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_manifest() -> (SstManifest, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let filesystem = Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());

        let storage_url = format!("file://{}", temp_dir.path().display());
        let manifest =
            SstManifest::new("test_collection".to_string(), storage_url, filesystem, None);

        (manifest, temp_dir)
    }

    #[tokio::test]
    async fn test_manifest_basic_operations() {
        let (manifest, _temp_dir) = create_test_manifest().await;

        // Add an SSTable
        let file_info = SstableFileInfo {
            file_id: "sst_001".to_string(),
            file_path: "level0/sst_001.sstable".to_string(),
            level: 0,
            size_bytes: 1024 * 1024,
            record_count: 1000,
            min_key: "a".to_string(),
            max_key: "z".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            last_compacted_at: None,
            bloom_fpr: 0.01,
            metadata_columns: HashMap::new(),
            marked_for_deletion: false,
            min_sequence: 1,
            max_sequence: 1000,
        };

        manifest.add_sstable(file_info).await.unwrap();

        // Check stats
        let stats = manifest.stats().await;
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.total_records, 1000);

        // Get files at level 0
        let level0_files = manifest.files_at_level(0).await;
        assert_eq!(level0_files.len(), 1);

        // Remove the file
        manifest
            .remove_sstables(&["sst_001".to_string()])
            .await
            .unwrap();

        let stats_after = manifest.stats().await;
        assert_eq!(stats_after.total_files, 0);
    }
}

impl fmt::Debug for SstManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SstManifest")
            .field("manifest_url", &self.manifest_url)
            .field("max_history_versions", &self.max_history_versions)
            .finish()
    }
}
