//! Disk Manager for WAL operations
//! 
//! This module centralizes all disk I/O operations, removing them from batch strategies.
//! It handles writing WAL data to disk, reading it back, and managing WAL files.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::serialization::SerializationFormat;
use crate::storage::persistence::write_ahead_log::BatchId;

/// Centralized manager for all WAL disk operations
pub struct WriteBufferDiskManager {
    /// Filesystem factory for creating filesystem instances
    filesystem_factory: Arc<FilesystemFactory>,
    /// Base directory for WriteBuffer files
    wal_base_dir: PathBuf,
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
#[derive(Debug, Clone)]
pub struct WriteBufferFileInfo {
    pub collection_id: String,
    pub batch_id: BatchId,
    pub file_path: PathBuf,
    pub size_bytes: u64,
    pub format: SerializationFormat,
}

impl WriteBufferDiskManager {
    /// Create a new disk manager
    pub fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        wal_base_dir: impl AsRef<Path>,
    ) -> Self {
        info!("🎯 Creating WriteBufferDiskManager with base dir: {:?}", wal_base_dir.as_ref());
        
        Self {
            filesystem_factory,
            wal_base_dir: wal_base_dir.as_ref().to_path_buf(),
            stats: Arc::new(tokio::sync::RwLock::new(DiskStats::default())),
        }
    }
    
    /// Get the filesystem factory
    pub fn filesystem_factory(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem_factory
    }
    
    /// Write a serialized batch to disk
    pub async fn write_batch(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        data: &[u8],
        format: SerializationFormat,
    ) -> Result<WriteBufferFileInfo> {
        self.write_batch_with_sync(collection_id, batch_id, data, format, false).await
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
        let file_path = self.get_batch_file_path(collection_id, batch_id, format);
        let file_url = format!("file://{}", file_path.display());
        
        debug!(
            "📝 Writing WriteBuffer batch {} for collection {} to {} ({} bytes)",
            batch_id.to_base62(),
            collection_id,
            file_url,
            data.len()
        );
        
        // Directory should already exist from collection creation
        // If it doesn't exist, it's an error condition
        let dir_path = file_path.parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid file path"))?;
        let dir_url = format!("file://{}/", dir_path.display());
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;
        
        if !filesystem.exists(&dir_url).await? {
            return Err(anyhow::anyhow!(
                "WriteBuffer directory does not exist for collection {}. Was the collection created properly?",
                collection_id
            ));
        }
        
        // Write data
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem.write(&file_url, data, None).await
            .context("Failed to write WriteBuffer batch to disk")?;
        
        // Sync to disk if requested
        if sync_to_disk {
            filesystem.sync_file(&file_url).await
                .context("Failed to sync WriteBuffer batch to disk")?;
            debug!("✅ WriteBuffer batch synced to disk for durability");
        }
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_written += data.len() as u64;
            stats.total_files_written += 1;
        }
        
        let file_info = WriteBufferFileInfo {
            collection_id: collection_id.to_string(),
            batch_id: batch_id.clone(),
            file_path,
            size_bytes: data.len() as u64,
            format,
        };
        
        debug!("✅ Successfully wrote WAL batch to disk: {:?}", file_info);
        Ok(file_info)
    }
    
    /// Read a serialized batch from disk
    pub async fn read_batch(
        &self,
        file_info: &WriteBufferFileInfo,
    ) -> Result<Vec<u8>> {
        let file_url = format!("file://{}", file_info.file_path.display());
        
        debug!(
            "📖 Reading WAL batch {} for collection {} from {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id,
            file_url
        );
        
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        let data = filesystem.read(&file_url).await
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
        let write_buffer_dir = self.wal_base_dir.join(collection_id).join("write_buffer");
        let dir_url = format!("file://{}/", write_buffer_dir.display());
        
        debug!("📂 Listing WriteBuffer files for collection {} in {}", collection_id, dir_url);
        
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
            debug!("  Entry: path={}, is_dir={}", entry.url, entry.metadata.is_directory);
            if let Some(file_info) = self.parse_wal_filename(&entry.url, collection_id) {
                wal_files.push(file_info);
            } else {
                debug!("  Failed to parse WAL filename: {}", entry.url);
            }
        }
        
        debug!("Found {} WAL files for collection {}", wal_files.len(), collection_id);
        Ok(wal_files)
    }
    
    /// Delete a WAL file
    pub async fn delete_file(
        &self,
        file_info: &WriteBufferFileInfo,
    ) -> Result<()> {
        let file_url = format!("file://{}", file_info.file_path.display());
        
        debug!(
            "🗑️ Deleting WAL file {} for collection {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id
        );
        
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem.delete(&file_url).await
            .context("Failed to delete WAL file")?;
        
        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }
    
    /// Delete a WAL file by path (used by recovery manager)
    pub async fn delete_wal_file(
        &self,
        file_path: &Path,
    ) -> Result<()> {
        let file_url = format!("file://{}", file_path.display());
        
        debug!(
            "🗑️ Deleting WAL file at path: {}",
            file_path.display()
        );
        
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem.delete(&file_url).await
            .context("Failed to delete WAL file")?;
        
        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }
    
    /// Delete all WAL files for a collection
    pub async fn delete_collection_files(
        &self,
        collection_id: &str,
    ) -> Result<u64> {
        let files = self.list_collection_files(collection_id).await?;
        let count = files.len() as u64;
        
        for file_info in files {
            if let Err(e) = self.delete_file(&file_info).await {
                warn!("Failed to delete WAL file {:?}: {}", file_info.file_path, e);
                continue;
            }
        }
        
        // Try to remove the WriteBuffer directory
        let write_buffer_dir = self.wal_base_dir.join(collection_id).join("write_buffer");
        let dir_url = format!("file://{}/", write_buffer_dir.display());
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;
        
        if let Err(e) = filesystem.delete(&dir_url).await {
            debug!("Failed to delete collection directory (may have subdirs): {}", e);
        }
        
        Ok(count)
    }
    
    /// Get statistics
    pub async fn get_stats(&self) -> Result<DiskStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }
    
    /// Get the file path for a batch
    pub fn get_batch_file_path(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        format: SerializationFormat,
    ) -> PathBuf {
        let extension = match format {
            SerializationFormat::ProtocolBuffers => "pbwal",
            SerializationFormat::Bincode => "bcwal",
            SerializationFormat::Avro => "avwal",
        };
        
        self.wal_base_dir
            .join(collection_id)
            .join("write_buffer")
            .join(format!("{}.{}", batch_id.to_base62(), extension))
    }
    
    /// Parse a WAL filename to extract metadata
    fn parse_wal_filename(
        &self,
        path: &str,
        collection_id: &str,
    ) -> Option<WriteBufferFileInfo> {
        // Strip file:// prefix if present
        let clean_path = if path.starts_with("file://") {
            path.strip_prefix("file://")
        } else {
            path
        };
        
        let path_buf = PathBuf::from(clean_path);
        let file_name = path_buf.file_name()?.to_str()?;
        
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
            file_path: path_buf,
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
            FilesystemFactory::new(filesystem_config).await
                .expect("Failed to create filesystem factory")
        );
        
        let manager = WriteBufferDiskManager::new(
            filesystem_factory,
            temp_dir.path(),
        );
        
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
        let file_info = manager.write_batch(collection_id, &batch_id, data, format).await
            .expect("Failed to write batch");
        assert_eq!(file_info.collection_id, collection_id);
        assert_eq!(file_info.size_bytes, data.len() as u64);
        
        // Read batch
        let read_data = manager.read_batch(&file_info).await
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
            manager.write_batch(collection_id, &batch_id, &data, SerializationFormat::Bincode).await
                .expect("Failed to write batch");
        }
        
        // List files
        let files = manager.list_collection_files(collection_id).await
            .expect("Failed to list files");
        assert_eq!(files.len(), 3);
        
        // Delete one file
        manager.delete_file(&files[0]).await
            .expect("Failed to delete file");
        
        // List again
        let remaining = manager.list_collection_files(collection_id).await
            .expect("Failed to list files");
        assert_eq!(remaining.len(), 2);
        
        // Delete all
        let deleted = manager.delete_collection_files(collection_id).await
            .expect("Failed to delete collection files");
        assert_eq!(deleted, 2);
        
        // Verify empty
        let final_list = manager.list_collection_files(collection_id).await
            .expect("Failed to list files");
        assert_eq!(final_list.len(), 0);
    }
}