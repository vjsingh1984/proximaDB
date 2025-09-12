//! Disk Manager for WAL operations
//!
//! This module centralizes all disk I/O operations, removing them from batch strategies.
//! It handles writing WAL data to disk, reading it back, and managing WAL files.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::BatchId;
use crate::storage::persistence::write_ahead_log::serialization::SerializationFormat;
use crate::storage::persistence::write_ahead_log::collection_path::slug_for;
use crate::utils::checksum::Crc32;

/// Centralized manager for all WAL disk operations
pub struct WriteBufferDiskManager {
    /// Filesystem factory for creating filesystem instances
    filesystem_factory: Arc<FilesystemFactory>,
    /// Base URL for WriteBuffer files (e.g., file:///path, s3://bucket/prefix)
    wal_base_url: String,
    /// Statistics
    stats: Arc<tokio::sync::RwLock<DiskStats>>,
}

/// Statistics for disk operations
#[derive(Debug, Clone, Default)]
pub struct DiskStats {
    pub total_bytes_written: u64,
    pub total_bytes_read: u64,
    pub total_files_written: u64,
    pub total_files_read: u64,
    pub write_errors: u64,
    pub read_errors: u64,
}

/// WAL file metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBufferFileInfo {
    pub collection_id: String,
    pub batch_id: BatchId,
    /// Full URL to the WAL file (scheme-preserving)
    pub file_url: String,
    pub size_bytes: u64,
    pub format: SerializationFormat,
}

impl WriteBufferDiskManager {
    /// Create a new disk manager
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        wal_base_url: impl AsRef<str>,
    ) -> Self {
        let wal_base_url = wal_base_url.as_ref().to_string();
        info!("🎯 Creating WriteBufferDiskManager with base URL: {}", wal_base_url);

        Self {
            filesystem_factory,
            wal_base_url,
            stats: Arc::new(tokio::sync::RwLock::new(DiskStats::default())),
        }
    }

    /// Helper: join URL segments preserving scheme and avoiding duplicate slashes
    fn join_url(base: &str, segments: &[&str], trailing_slash: bool) -> String {
        // Remove trailing slash from base (but preserve scheme authority like file:///)
        let mut url = if base == "file://" {
            // extremely unlikely, but keep as-is
            base.to_string()
        } else {
            // For file:// URIs, usually base already has file:///path or file://./path
            base.trim_end_matches('/')
                .to_string()
        };

        for seg in segments {
            if !seg.is_empty() {
                url.push('/');
                url.push_str(seg.trim_matches('/'));
            }
        }
        if trailing_slash {
            url.push('/');
        }
        url
    }

    /// Get the filesystem factory
    pub fn filesystem_factory(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem_factory
    }

    /// Get the base WAL URL
    pub fn get_base_wal_url(&self) -> &str {
        &self.wal_base_url
    }

    /// Build the WAL URL for a collection (slugged): {base}/{slug}/wal/
    pub fn collection_wal_url(&self, collection_id: &str) -> String {
        let slug = slug_for(collection_id);
        Self::join_url(&self.wal_base_url, &[&slug, "wal"], true)
    }

    /// Build WAL batch URL: .../{slug}/wal/<batch_id>.<ext>
    pub fn batch_url(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        format: SerializationFormat,
    ) -> String {
        let slug = slug_for(collection_id);
        let ext = match format {
            SerializationFormat::ProtocolBuffers => "pbwal",
            SerializationFormat::Bincode => "bcwal",
            SerializationFormat::Avro => "avwal",
        };
        let fname = format!("{}.{}", batch_id.to_base62(), ext);
        Self::join_url(&self.wal_base_url, &[&slug, "wal", &fname], false)
    }

    /// Build manifest URL: .../{slug}/wal/manifest.log
    pub fn manifest_url(&self, collection_id: &str) -> String {
        let slug = slug_for(collection_id);
        Self::join_url(&self.wal_base_url, &[&slug, "wal", "manifest.log"], false)
    }

    /// Write a serialized batch to disk
    pub async fn write_batch(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        data: &[u8],
        format: SerializationFormat,
    ) -> Result<WriteBufferFileInfo> {
        self.write_batch_with_sync(collection_id, batch_id, data, format, false)
            .await
    }

    /// Write a serialized batch to disk with optional sync
    pub async fn write_batch_with_sync(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        data: &[u8],
        format: SerializationFormat,
        sync_to_disk: bool,
    ) -> Result<WriteBufferFileInfo> {
        let file_url = self.batch_url(collection_id, batch_id, format);

        debug!(
            "📝 Writing WriteBuffer batch {} for collection {} to {} ({} bytes)",
            batch_id.to_base62(),
            collection_id,
            file_url,
            data.len()
        );

        // Ensure directory exists (create if missing). This improves robustness
        // in serverless/cloud cold starts where collection directories may not
        // be pre-created by control-plane paths.
        let dir_url = self.collection_wal_url(collection_id);
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;

        // Always attempt directory creation for uniform semantics (no-op on object stores)
        let _ = filesystem.create_dir_all(&dir_url).await;

        // Write data atomically; for object stores this will write to a temp and
        // then rename to the final path.
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*filesystem, None)?;
        let file_options = strategy.create_file_options(&*filesystem, &file_url)?;
        filesystem
            .write(&file_url, data, Some(file_options))
            .await
            .context("Failed to write WriteBuffer batch to disk")?;

        // Sync to disk if requested
        if sync_to_disk {
            filesystem
                .sync_file(&file_url)
                .await
                .context("Failed to sync WriteBuffer batch to disk")?;
            debug!("✅ WriteBuffer batch synced to disk for durability");
        }

        // Append manifest entry with checksum (atomic rewrite of manifest.log)
        let checksum = Crc32::checksum(data);
        // Extract filename from URL for manifest entry
        let file_name = file_url
            .split('/')
            .last()
            .unwrap_or("")
            .to_string();
        let entry = crate::storage::persistence::write_ahead_log::manifest::WalManifestEntry::from_batch(
            batch_id,
            file_name,
            data.len() as u64,
            checksum,
        );
        let manifest_url = self.manifest_url(collection_id);
        let mfs = self.filesystem_factory.get_filesystem(&manifest_url)?;
        let mut manifest_content = if mfs.exists(&manifest_url).await? {
            mfs.read(&manifest_url).await?
        } else {
            Vec::new()
        };
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        manifest_content.extend_from_slice(&line);
        let mstrategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*mfs, None)?;
        let mopts = mstrategy.create_file_options(&*mfs, &manifest_url)?;
        mfs.write(&manifest_url, &manifest_content, Some(mopts)).await?;
        let _ = mfs.sync_file(&manifest_url).await;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_written += data.len() as u64;
            stats.total_files_written += 1;
        }

        let file_info = WriteBufferFileInfo {
            collection_id: collection_id.to_string(),
            batch_id: batch_id.clone(),
            file_url: file_url.clone(),
            size_bytes: data.len() as u64,
            format,
        };

        debug!("✅ Successfully wrote WAL batch to disk: {:?}", file_info);
        Ok(file_info)
    }

    /// Read a serialized batch from disk
    pub async fn read_batch(&self, file_info: &WriteBufferFileInfo) -> Result<Vec<u8>> {
        let file_url = file_info.file_url.clone();

        debug!(
            "📖 Reading WAL batch {} for collection {} from {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id,
            file_url
        );

        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        let data = filesystem
            .read(&file_url)
            .await
            .context("Failed to read WAL batch from disk")?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_read += data.len() as u64;
            stats.total_files_read += 1;
        }

        debug!("✅ Successfully read {} bytes from disk", data.len());
        Ok(data)
    }

    /// List all WAL files for a collection
    pub async fn list_collection_files(
        &self,
        collection_id: &str,
    ) -> Result<Vec<WriteBufferFileInfo>> {
        let dir_url = self.collection_wal_url(collection_id);

        debug!(
            "📂 Listing WriteBuffer files for collection {} in {}",
            collection_id, dir_url
        );

        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;

        // Check if directory exists
        if !filesystem.exists(&dir_url).await? {
            debug!("WriteBuffer directory does not exist: {}", dir_url);
            return Ok(Vec::new());
        }

        let entries = filesystem.list(&dir_url).await?;
        let mut wal_files = Vec::new();

        debug!("Raw entries from filesystem: {} entries", entries.len());
        for entry in entries {
            debug!(
                "  Entry: path={}, is_dir={}",
                entry.url, entry.metadata.is_directory
            );
            if let Some(file_info) = self.parse_wal_filename(&entry.url, collection_id) {
                wal_files.push(file_info);
            } else {
                debug!("  Failed to parse WAL filename: {}", entry.url);
            }
        }

        // Backward-compat: if none found, fall back to legacy write_buffer path
        if wal_files.is_empty() {
            // Backward-compat path: {base}/{collection_id}/write_buffer/
            let legacy_url = Self::join_url(&self.wal_base_url, &[collection_id, "write_buffer"], true);
            if filesystem.exists(&legacy_url).await.unwrap_or(false) {
                if let Ok(legacy_fs) = self.filesystem_factory.get_filesystem(&legacy_url) {
                    if let Ok(entries) = legacy_fs.list(&legacy_url).await {
                        for entry in entries {
                            if let Some(file_info) = self.parse_wal_filename(&entry.url, collection_id) {
                                wal_files.push(file_info);
                            }
                        }
                    }
                }
            }
        }

        debug!("Found {} WAL files for collection {}", wal_files.len(), collection_id);
        Ok(wal_files)
    }

    /// Delete a WAL file
    pub async fn delete_file(&self, file_info: &WriteBufferFileInfo) -> Result<()> {
        let file_url = file_info.file_url.clone();

        debug!(
            "🗑️ Deleting WAL file {} for collection {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id
        );

        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem
            .delete(&file_url)
            .await
            .context("Failed to delete WAL file")?;

        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }

    /// Delete a WAL file by path (used by recovery manager)
    pub async fn delete_wal_file_url(&self, file_url: &str) -> Result<()> {
        debug!("🗑️ Deleting WAL file at URL: {}", file_url);

        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem
            .delete(&file_url)
            .await
            .context("Failed to delete WAL file")?;

        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }

    /// Delete all WAL files for a collection
    pub async fn delete_collection_files(&self, collection_id: &str) -> Result<u64> {
        let files = self.list_collection_files(collection_id).await?;
        let count = files.len() as u64;

        for file_info in files {
            if let Err(e) = self.delete_file(&file_info).await {
                warn!("Failed to delete WAL file {}: {}", file_info.file_url, e);
                continue;
            }
        }

        // Try to remove the collection WAL directory
        let dir_url = self.collection_wal_url(collection_id);
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;

        if let Err(e) = filesystem.delete(&dir_url).await {
            debug!(
                "Failed to delete collection directory (may have subdirs): {}",
                e
            );
        }

        Ok(count)
    }

    /// Get statistics
    pub async fn get_stats(&self) -> Result<DiskStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }

    /// Get the file path for a batch
    // get_batch_file_path removed in favor of URL builders

    /// Parse a WAL filename to extract metadata
    fn parse_wal_filename(&self, path: &str, collection_id: &str) -> Option<WriteBufferFileInfo> {
        // Use last path segment as filename regardless of scheme
        let file_name = path.split('/').last()?;

        // Expected format: <batch_id>.<format>
        let parts: Vec<&str> = file_name.split('.').collect();
        if parts.len() != 2 {
            return None;
        }

        let batch_id_str = parts[0];
        let extension = parts[1];

        // Parse format from extension
        let format = match extension {
            "pbwal" => SerializationFormat::ProtocolBuffers,
            "bcwal" => SerializationFormat::Bincode,
            "avwal" => SerializationFormat::Avro,
            // Alternative extensions for backward compatibility
            "proto" => SerializationFormat::ProtocolBuffers,
            "bincode" => SerializationFormat::Bincode,
            "avro" => SerializationFormat::Avro,
            _ => return None,
        };

        // Parse batch ID
        let batch_id = BatchId::from_base62(batch_id_str)?;

        Some(WriteBufferFileInfo {
            collection_id: collection_id.to_string(),
            batch_id,
            file_url: path.to_string(),
            size_bytes: 0, // Will be filled by caller if needed
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemConfig;
    use tempfile::TempDir;

    async fn create_test_manager() -> (WriteBufferDiskManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let manager = WriteBufferDiskManager::new(filesystem_factory, temp_dir.path());

        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_disk_manager_write_read() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "test_collection";
        let batch_id = BatchId::new();
        let data = b"test data";
        let format = SerializationFormat::ProtocolBuffers;

        // Create WAL directory for collection (simulating collection creation)
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        std::fs::create_dir_all(&write_buffer_dir).expect("Failed to create WriteBuffer directory");

        // Write batch
        let file_info = manager
            .write_batch(collection_id, &batch_id, data, format)
            .await
            .expect("Failed to write batch");
        assert_eq!(file_info.collection_id, collection_id);
        assert_eq!(file_info.size_bytes, data.len() as u64);

        // Read batch
        let read_data = manager
            .read_batch(&file_info)
            .await
            .expect("Failed to read batch");
        assert_eq!(read_data, data);

        // Check stats
        let stats = manager.stats().await.expect("Failed to get stats");
        assert_eq!(stats.total_bytes_written, data.len() as u64);
        assert_eq!(stats.total_bytes_read, data.len() as u64);
        assert_eq!(stats.total_files_written, 1);
        assert_eq!(stats.total_files_read, 1);
    }

    #[tokio::test]
    async fn test_disk_manager_list_delete() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "test_collection";

        // Create WAL directory for collection (simulating collection creation)
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        std::fs::create_dir_all(&write_buffer_dir).expect("Failed to create WriteBuffer directory");

        // Write multiple batches
        for i in 0..3 {
            let batch_id = BatchId::new();
            let data = format!("test data {}", i).into_bytes();
            manager
                .write_batch(
                    collection_id,
                    &batch_id,
                    &data,
                    SerializationFormat::Bincode,
                )
                .await
                .expect("Failed to write batch");
        }

        // List files
        let files = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(files.len(), 3);

        // Delete one file
        manager
            .delete_file(&files[0])
            .await
            .expect("Failed to delete file");

        // List again
        let remaining = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(remaining.len(), 2);

        // Delete all
        let deleted = manager
            .delete_collection_files(collection_id)
            .await
            .expect("Failed to delete collection files");
        assert_eq!(deleted, 2);

        // Verify empty
        let final_list = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(final_list.len(), 0);
    }
}
