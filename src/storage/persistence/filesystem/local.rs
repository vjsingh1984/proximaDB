/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Local filesystem implementation supporting Windows, Linux, and other OS platforms
//!
//! IMPORTANT: This implementation uses sync_all() for writes when sync_enabled is true
//! to ensure data durability and prevent file truncation issues that can occur when
//! files are moved/renamed before data is fully flushed to disk.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::{DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsResult};

/// Local filesystem configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// Root directory for relative paths
    pub root_dir: Option<PathBuf>,

    /// Enable symbolic link resolution
    pub follow_symlinks: bool,

    /// Default file permissions (Unix-style octal)
    pub default_permissions: Option<u32>,

    /// Enable filesystem-level sync operations
    pub sync_enabled: bool,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            root_dir: None,
            follow_symlinks: true,
            default_permissions: None,
            sync_enabled: true,
        }
    }
}

/// Local filesystem implementation
#[derive(Debug)]
pub struct LocalFileSystem {
    config: LocalConfig,
}

impl LocalFileSystem {
    /// Extract path from URL for stateless operations (uses centralized utility)
    fn extract_path_from_url(&self, url: &str) -> FsResult<String> {
        use crate::storage::persistence::filesystem::FilesystemFactory;
        
        let path = FilesystemFactory::extract_path_from_url_safe(url)?;
        
        // Validate that it's a file URL if it has a scheme
        if url.contains("://") {
            use url::Url;
            let parsed_url = Url::parse(url)?;
            if parsed_url.scheme() != "file" {
                return Err(FilesystemError::UnsupportedScheme(parsed_url.scheme().to_string()));
            }
        }
        
        Ok(path)
    }

    /// Create new local filesystem instance
    pub async fn new(config: LocalConfig) -> FsResult<Self> {
        // Create and validate root directory if specified
        if let Some(ref root_dir) = config.root_dir {
            if !root_dir.exists() {
                // Create the root directory recursively if it doesn't exist
                fs::create_dir_all(root_dir).await.map_err(|e| {
                    FilesystemError::Config(format!(
                        "Failed to create root directory {}: {}",
                        root_dir.display(),
                        e
                    ))
                })?;
                tracing::info!("📁 Created root directory: {}", root_dir.display());
            }
            if !root_dir.is_dir() {
                return Err(FilesystemError::Config(format!(
                    "Root path is not a directory: {}",
                    root_dir.display()
                )));
            }
        }

        Ok(Self { config })
    }

    /// Unified method to extract path from URL and resolve relative to root if configured
    /// This replaces the separate extract_path_from_url + resolve_path pattern
    fn extract_and_resolve_path(&self, url_or_path: &str) -> FsResult<PathBuf> {
        // Extract path from URL using centralized method
        let path_str = self.extract_path_from_url(url_or_path)?;
        let path_buf = PathBuf::from(path_str);

        // Apply root directory resolution if configured
        let resolved = if let Some(ref root_dir) = self.config.root_dir {
            if path_buf.is_absolute() {
                path_buf
            } else {
                root_dir.join(path_buf)
            }
        } else {
            path_buf
        };
        
        Ok(resolved)
    }

    /// Convert std::fs::Metadata to FileMetadata
    fn convert_metadata(&self, path: &Path, metadata: &std::fs::Metadata) -> FileMetadata {
        use std::time::UNIX_EPOCH;

        let to_datetime = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos()))
                .flatten()
        };

        FileMetadata {
            path: path.display().to_string(),
            size: metadata.len(),
            created: to_datetime(metadata.created()),
            modified: to_datetime(metadata.modified()),
            is_directory: metadata.is_dir(),
            permissions: self.get_permissions_string(metadata),
            etag: None,          // Not applicable for local filesystem
            storage_class: None, // Not applicable for local filesystem
        }
    }

    /// Get permissions string from metadata
    fn get_permissions_string(&self, metadata: &std::fs::Metadata) -> Option<String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            Some(format!("{:o}", mode & 0o777))
        }

        #[cfg(not(unix))]
        {
            if metadata.permissions().readonly() {
                Some("readonly".to_string())
            } else {
                Some("readwrite".to_string())
            }
        }
    }
}

#[async_trait]
impl FileSystem for LocalFileSystem {
    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        // Extract and resolve path in one step
        let resolved_path = self.extract_and_resolve_path(path)?;

        match fs::read(&resolved_path).await {
            Ok(data) => Ok(data),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(FilesystemError::NotFound(
                    format!("File not found: {}", path)
                )),
                std::io::ErrorKind::PermissionDenied => Err(FilesystemError::PermissionDenied(
                    resolved_path.display().to_string(),
                )),
                _ => Err(FilesystemError::Io(e)),
            },
        }
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        let resolved_path = self.extract_and_resolve_path(path)?;
        let options = options.unwrap_or_default();

        // Create parent directories if requested
        if options.create_dirs {
            if let Some(parent) = resolved_path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(FilesystemError::Io)?;
            }
        }

        // Check if file exists and handle overwrite option
        if !options.overwrite && resolved_path.exists() {
            return Err(FilesystemError::AlreadyExists(
                resolved_path.display().to_string(),
            ));
        }

        // Write file with proper sync to ensure data is flushed to disk
        // This is critical for atomic writes to prevent truncation
        if self.config.sync_enabled {
            // Open file for writing with sync
            let mut file = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&resolved_path)
                .await
                .map_err(FilesystemError::Io)?;
                
            // Write all data
            file.write_all(data).await.map_err(FilesystemError::Io)?;
            
            // Sync data to disk to prevent truncation issues
            file.sync_all().await.map_err(FilesystemError::Io)?;
        } else {
            // Fast write without sync (for development/testing only)
            fs::write(&resolved_path, data)
                .await
                .map_err(FilesystemError::Io)?;
        }

        // Set permissions if specified
        #[cfg(unix)]
        if let Some(permissions) = self.config.default_permissions {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(permissions);
            fs::set_permissions(&resolved_path, perms)
                .await
                .map_err(FilesystemError::Io)?;
        }

        Ok(())
    }

    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let resolved_path = self.extract_and_resolve_path(path)?;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved_path)
            .await
            .map_err(FilesystemError::Io)?;

        file.write_all(data).await.map_err(FilesystemError::Io)?;

        if self.config.sync_enabled {
            file.sync_all().await.map_err(FilesystemError::Io)?;
        }

        Ok(())
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.extract_and_resolve_path(path)?;

        if !resolved_path.exists() {
            return Err(FilesystemError::NotFound(
                resolved_path.display().to_string(),
            ));
        }

        if resolved_path.is_dir() {
            fs::remove_dir_all(&resolved_path)
                .await
                .map_err(FilesystemError::Io)
        } else {
            fs::remove_file(&resolved_path)
                .await
                .map_err(FilesystemError::Io)
        }
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        let resolved_path = self.extract_and_resolve_path(path)?;
        Ok(resolved_path.exists())
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        
        // Extract and resolve path in one step
        let resolved_path = self.extract_and_resolve_path(path)?;
        
        // Open file for reading
        let mut file = match tokio::fs::File::open(&resolved_path).await {
            Ok(f) => f,
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => {
                    return Err(FilesystemError::NotFound(
                        format!("File not found: {}", path)
                    ))
                }
                std::io::ErrorKind::PermissionDenied => {
                    return Err(FilesystemError::PermissionDenied(
                        resolved_path.display().to_string(),
                    ))
                }
                _ => return Err(FilesystemError::Io(e)),
            },
        };
        
        // Get file size to validate the range
        let metadata = file.metadata().await.map_err(FilesystemError::Io)?;
        let file_size = metadata.len();
        
        // Check if offset is beyond file size
        if offset >= file_size {
            return Ok(vec![]);
        }
        
        // Seek to the offset
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(FilesystemError::Io)?;
        
        // Calculate actual bytes to read (handle case where offset + length > file_size)
        let bytes_to_read = std::cmp::min(length, file_size - offset) as usize;
        
        // Debug log for SSTable bloom filter issue
        if path.contains(".sst") && offset < 1000 {
            tracing::debug!(
                "DEBUG LocalFS read_range: path={}, offset={}, length={}, file_size={}, bytes_to_read={}",
                path, offset, length, file_size, bytes_to_read
            );
        }
        
        // Read the exact amount of bytes
        let mut buffer = vec![0u8; bytes_to_read];
        match file.read_exact(&mut buffer).await {
            Ok(_) => {
                // Success
            }
            Err(e) => {
                // Get current position for debugging
                let current_pos = file.stream_position().await.unwrap_or(0);
                tracing::error!(
                    "LocalFS read_exact failed: path={}, offset={}, bytes_to_read={}, current_pos={}, file_size={}, error={:?}",
                    path, offset, bytes_to_read, current_pos, file_size, e
                );
                return Err(FilesystemError::Io(e));
            }
        }
        
        Ok(buffer)
    }

    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        let resolved_path = self.extract_and_resolve_path(path)?;

        let metadata = if self.config.follow_symlinks {
            fs::metadata(&resolved_path).await
        } else {
            fs::symlink_metadata(&resolved_path).await
        };

        match metadata {
            Ok(meta) => Ok(self.convert_metadata(&resolved_path, &meta)),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(FilesystemError::NotFound(
                    resolved_path.display().to_string(),
                )),
                _ => Err(FilesystemError::Io(e)),
            },
        }
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        info!("📁 DEBUG LocalFileSystem::list called with path: '{}'", path);
        // Extract and resolve path in one step
        let resolved_path = self.extract_and_resolve_path(path)?;
        info!("📁 DEBUG LocalFileSystem::list resolved_path: {:?}", resolved_path);

        let mut entries = Vec::new();
        let mut dir = fs::read_dir(&resolved_path)
            .await
            .map_err(FilesystemError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(FilesystemError::Io)? {
            let entry_path = entry.path();
            let name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();

            let metadata = entry.metadata().await.map_err(FilesystemError::Io)?;
            let file_metadata = self.convert_metadata(&entry_path, &metadata);

            // Create full URL for entry (stateless design)
            let entry_url = if self.config.root_dir.is_some() && path.starts_with("file://./") {
                // For relative URLs, preserve the relative nature
                let relative_from_root = entry_path.strip_prefix(&self.config.root_dir.as_ref().unwrap())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| entry_path.display().to_string());
                format!("file://./{}", relative_from_root)
            } else {
                format!("file://{}", entry_path.display())
            };
            info!("📁 DEBUG LocalFileSystem::list entry_url: '{}' for entry_path: {:?}", entry_url, entry_path);

            entries.push(DirEntry {
                name,
                url: entry_url,  // Use full URL instead of path
                metadata: file_metadata,
            });
        }

        // Sort entries by name for consistent ordering
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(entries)
    }

    async fn create_dir(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.extract_and_resolve_path(path)?;

        fs::create_dir(&resolved_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    FilesystemError::AlreadyExists(resolved_path.display().to_string())
                }
                _ => FilesystemError::Io(e),
            })
    }

    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        info!("📁 LocalFileSystem::create_dir_all called with path: '{}'", path);
        let resolved_path = self.extract_and_resolve_path(path)?;
        info!("📁 LocalFileSystem::create_dir_all resolved to: {:?}", resolved_path);
        info!("📁 LocalFileSystem config: root_dir={:?}", self.config.root_dir);

        fs::create_dir_all(&resolved_path)
            .await
            .map_err(FilesystemError::Io)
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let from_path = self.extract_and_resolve_path(from)?;
        let to_path = self.extract_and_resolve_path(to)?;

        // Create parent directory for destination if needed
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(FilesystemError::Io)?;
        }

        fs::copy(&from_path, &to_path)
            .await
            .map_err(FilesystemError::Io)?;

        Ok(())
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        // Extract and resolve paths in one step
        let from_path = self.extract_and_resolve_path(from)?;
        let to_path = self.extract_and_resolve_path(to)?;

        // Create parent directory for destination if needed
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(FilesystemError::Io)?;
        }

        fs::rename(&from_path, &to_path)
            .await
            .map_err(FilesystemError::Io)
    }

    fn filesystem_type(&self) -> &'static str {
        "local"
    }

    async fn sync(&self) -> FsResult<()> {
        // For local filesystem, sync is handled at the file level
        // This is a no-op for the filesystem as a whole
        Ok(())
    }
    
    async fn sync_file(&self, path: &str) -> FsResult<()> {
        // Extract and resolve path in one step
        let resolved_path = self.extract_and_resolve_path(path)?;
        
        // Only sync if enabled in config
        if !self.config.sync_enabled {
            return Ok(());
        }
        
        // Open the file for sync
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&resolved_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    FilesystemError::NotFound(resolved_path.display().to_string())
                }
                _ => FilesystemError::Io(e),
            })?;
        
        // Call sync_all() which performs fsync on the file
        file.sync_all().await
            .map_err(FilesystemError::Io)?;
        
        tracing::debug!("✅ File synced to disk: {}", resolved_path.display());
        Ok(())
    }

    async fn open_file(&self, path: &str, create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        let resolved_path = self.extract_and_resolve_path(path)?;
        
        let file = if create {
            tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&resolved_path)
                .await
        } else {
            tokio::fs::File::open(&resolved_path).await
        }.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FilesystemError::NotFound(
                format!("File not found: {}", path)
            ),
            std::io::ErrorKind::PermissionDenied => FilesystemError::PermissionDenied(
                resolved_path.display().to_string(),
            ),
            _ => FilesystemError::Io(e),
        })?;
        
        Ok(Box::new(LocalFile {
            file,
            path: resolved_path,
        }))
    }
}

/// Local file handle for streaming operations
#[derive(Debug)]
struct LocalFile {
    file: tokio::fs::File,
    path: PathBuf,
}

#[async_trait]
impl FilesystemFile for LocalFile {
    async fn read(&mut self, buf: &mut [u8]) -> FsResult<usize> {
        use tokio::io::AsyncReadExt;
        self.file.read(buf).await.map_err(FilesystemError::Io)
    }
    
    async fn write(&mut self, buf: &[u8]) -> FsResult<usize> {
        use tokio::io::AsyncWriteExt;
        self.file.write(buf).await.map_err(FilesystemError::Io)
    }
    
    async fn flush(&mut self) -> FsResult<()> {
        use tokio::io::AsyncWriteExt;
        self.file.flush().await.map_err(FilesystemError::Io)
    }
    
    async fn seek(&mut self, pos: u64) -> FsResult<u64> {
        use tokio::io::{AsyncSeekExt, SeekFrom};
        self.file.seek(SeekFrom::Start(pos)).await.map_err(FilesystemError::Io)
    }
    
    async fn position(&self) -> FsResult<u64> {
        // Note: Getting current position requires a mutable reference in tokio
        // This is a limitation of the tokio API
        Ok(0) // Would need to track position separately or use seek(SeekFrom::Current(0))
    }
    
    async fn file_size(&self) -> FsResult<u64> {
        let metadata = tokio::fs::metadata(&self.path).await.map_err(FilesystemError::Io)?;
        Ok(metadata.len())
    }
    
    async fn sync_all(&mut self) -> FsResult<()> {
        self.file.sync_all().await.map_err(FilesystemError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_relative_path_url_handling() {
        // Create a temporary directory
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path();
        
        // Create test structure
        std::fs::create_dir_all(temp_path.join("test_metadata/current")).unwrap();
        std::fs::write(temp_path.join("test_metadata/current/test.txt"), b"test content").unwrap();
        
        // Create filesystem with relative path configuration
        let config = LocalConfig {
            root_dir: Some(temp_path.to_path_buf()),
            follow_symlinks: true,
            default_permissions: None,
            sync_enabled: false,
        };
        
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Test listing with relative URL
        let relative_url = "file://./test_metadata/current";
        let entries = fs.list(relative_url).await.unwrap();
        
        // Verify entries
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "test.txt");
        
        // The key test: verify the URL doesn't have duplicated paths
        let entry_url = &entries[0].url;
        println!("Entry URL: {}", entry_url);
        
        // The URL should be file://./test_metadata/current/test.txt
        // NOT file://./test_metadata/./test_metadata/current/test.txt
        assert!(entry_url.contains("file://./test_metadata/current/test.txt"));
        assert!(!entry_url.contains("test_metadata/test_metadata"));
    }

    #[tokio::test]
    async fn test_absolute_path_url_handling() {
        // Create a temporary directory
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path();
        
        // Create test structure
        let test_dir = temp_path.join("test_metadata/current");
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("test.txt"), b"test content").unwrap();
        
        // Create filesystem without root_dir (absolute paths)
        let config = LocalConfig::default();
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Test listing with absolute URL
        let absolute_url = format!("file://{}", test_dir.display());
        let entries = fs.list(&absolute_url).await.unwrap();
        
        // Verify entries
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "test.txt");
        
        // Verify the URL is properly formatted
        let entry_url = &entries[0].url;
        println!("Entry URL: {}", entry_url);
        assert!(entry_url.starts_with("file://"));
        assert!(entry_url.ends_with("/test.txt"));
    }

    #[tokio::test]
    async fn test_extract_path_from_url() {
        let config = LocalConfig::default();
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Test various URL formats
        assert_eq!(fs.extract_path_from_url("file:///tmp/test").unwrap(), "/tmp/test");
        assert_eq!(fs.extract_path_from_url("file://./relative/path").unwrap(), "./relative/path");
        assert_eq!(fs.extract_path_from_url("/absolute/path").unwrap(), "/absolute/path");
        assert_eq!(fs.extract_path_from_url("relative/path").unwrap(), "relative/path");
    }

    #[tokio::test]
    async fn test_no_path_duplication_in_operations() {
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path();
        
        // Setup filesystem with root directory
        let config = LocalConfig {
            root_dir: Some(temp_path.to_path_buf()),
            follow_symlinks: true,
            default_permissions: None,
            sync_enabled: false,
        };
        
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Create a directory structure
        let test_url = "file://./metadata/current";
        fs.create_dir_all(test_url).await.unwrap();
        
        // Write a file
        let file_url = "file://./metadata/current/test.oplog";
        fs.write(file_url, b"test data", None).await.unwrap();
        
        // List the directory
        let entries = fs.list(test_url).await.unwrap();
        
        // Verify no path duplication
        assert_eq!(entries.len(), 1);
        let entry_url = &entries[0].url;
        
        // Count occurrences of "metadata" in the URL
        let metadata_count = entry_url.matches("metadata").count();
        assert_eq!(metadata_count, 1, "Path 'metadata' should appear only once in URL: {}", entry_url);
        
        // Verify we can read the file using the listed URL
        let content = fs.read(&entry_url).await.unwrap();
        assert_eq!(content, b"test data");
    }

    #[tokio::test]
    async fn test_filesystem_without_root_dir() {
        // Test that filesystem without root_dir handles URLs correctly
        let temp_dir = TempDir::new().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();
        
        // Create filesystem WITHOUT root_dir
        let config = LocalConfig::default();
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Test with relative URL
        let relative_url = "file://./test_metadata/current";
        fs.create_dir_all(relative_url).await.unwrap();
        
        // Verify the directory was created at the correct location
        assert!(std::path::Path::new("./test_metadata/current").exists());
        
        // Write a file
        fs.write("file://./test_metadata/current/test.txt", b"test", None).await.unwrap();
        
        // List directory
        let entries = fs.list(relative_url).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].url.contains("test.txt"));
        
        // Verify no path duplication in the URL
        assert!(!entries[0].url.contains("test_metadata/test_metadata"));
        
        // Restore original directory
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[tokio::test]
    async fn test_resolve_path_consistency() {
        // Test that resolve_path behaves consistently
        let temp_dir = TempDir::new().unwrap();
        
        // Test with root_dir
        let config_with_root = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let fs_with_root = LocalFileSystem::new(config_with_root).await.unwrap();
        
        // Test path resolution
        let resolved = fs_with_root.extract_and_resolve_path("file://./subdir/file.txt").unwrap();
        assert_eq!(resolved, temp_dir.path().join("./subdir/file.txt"));
        
        // Test without root_dir
        let config_no_root = LocalConfig::default();
        let fs_no_root = LocalFileSystem::new(config_no_root).await.unwrap();
        
        let resolved = fs_no_root.extract_and_resolve_path("file://./subdir/file.txt").unwrap();
        assert_eq!(resolved, std::path::PathBuf::from("./subdir/file.txt"));
    }

    #[tokio::test]
    #[ignore = "Environment-specific permission issues on some systems"]
    async fn test_prevent_double_relative_path_resolution() {
        // This test specifically checks the bug where paths like
        // "./demo_metadata" with root_dir "./demo_metadata" 
        // resulted in "./demo_metadata/./demo_metadata"
        
        let temp_dir = TempDir::new().unwrap();
        let metadata_dir = temp_dir.path().join("metadata");
        std::fs::create_dir_all(&metadata_dir).unwrap();
        
        // Create filesystem with root_dir pointing to metadata
        let config = LocalConfig {
            root_dir: Some(metadata_dir.clone()),
            sync_enabled: false, // Disable sync for testing to avoid permission issues
            ..Default::default()
        };
        let fs = LocalFileSystem::new(config).await.unwrap();
        
        // Create subdirectory using a path that's already relative to root
        let subdir_path = "current"; // NOT "./metadata/current"
        fs.create_dir_all(subdir_path).await.unwrap();
        
        // Verify it was created at the correct location
        assert!(metadata_dir.join("current").exists());
        assert!(!metadata_dir.join("metadata/current").exists());
        
        // Test with URL - use FileOptions to ensure proper creation
        let options = Some(crate::storage::persistence::filesystem::FileOptions {
            create_dirs: true,
            overwrite: true,
            buffer_size: None,
            encryption: None,
            storage_class: None,
            metadata: None,
            temp_path: None,
        });
        fs.write("file://current/test.txt", b"data", options).await.unwrap();
        assert!(metadata_dir.join("current/test.txt").exists());
    }
}

