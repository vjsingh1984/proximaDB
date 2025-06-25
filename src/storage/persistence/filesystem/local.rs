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

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError, FsResult};

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
    /// Create new local filesystem instance
    pub async fn new(config: LocalConfig) -> FsResult<Self> {
        // Validate root directory if specified
        if let Some(ref root_dir) = config.root_dir {
            if !root_dir.exists() {
                return Err(FilesystemError::NotFound(format!(
                    "Root directory does not exist: {}",
                    root_dir.display()
                )));
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

    /// Resolve path relative to root if configured
    fn resolve_path(&self, path: &str) -> PathBuf {
        let path_buf = PathBuf::from(path);

        if let Some(ref root_dir) = self.config.root_dir {
            if path_buf.is_absolute() {
                path_buf
            } else {
                root_dir.join(path_buf)
            }
        } else {
            path_buf
        }
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
        let resolved_path = self.resolve_path(path);

        match fs::read(&resolved_path).await {
            Ok(data) => Ok(data),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(FilesystemError::NotFound(
                    resolved_path.display().to_string(),
                )),
                std::io::ErrorKind::PermissionDenied => Err(FilesystemError::PermissionDenied(
                    resolved_path.display().to_string(),
                )),
                _ => Err(FilesystemError::Io(e)),
            },
        }
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);
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

        // Write file
        fs::write(&resolved_path, data)
            .await
            .map_err(FilesystemError::Io)?;

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
        let resolved_path = self.resolve_path(path);

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
        let resolved_path = self.resolve_path(path);

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
        let resolved_path = self.resolve_path(path);
        Ok(resolved_path.exists())
    }

    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        let resolved_path = self.resolve_path(path);

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
        let resolved_path = self.resolve_path(path);

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

            entries.push(DirEntry {
                name,
                path: entry_path.display().to_string(),
                metadata: file_metadata,
            });
        }

        // Sort entries by name for consistent ordering
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(entries)
    }

    async fn create_dir(&self, path: &str) -> FsResult<()> {
        let resolved_path = self.resolve_path(path);

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
        let resolved_path = self.resolve_path(path);

        fs::create_dir_all(&resolved_path)
            .await
            .map_err(FilesystemError::Io)
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let from_path = self.resolve_path(from);
        let to_path = self.resolve_path(to);

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
        let from_path = self.resolve_path(from);
        let to_path = self.resolve_path(to);

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_filesystem_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };

        let fs = LocalFileSystem::new(config).await.unwrap();

        // Test write and read
        let test_data = b"Hello, ProximaDB!";
        let test_path = "test_file.txt";

        fs.write(test_path, test_data, None).await.unwrap();
        assert!(fs.exists(test_path).await.unwrap());

        let read_data = fs.read(test_path).await.unwrap();
        assert_eq!(read_data, test_data);

        // Test metadata
        let metadata = fs.metadata(test_path).await.unwrap();
        assert_eq!(metadata.size, test_data.len() as u64);
        assert!(!metadata.is_directory);

        // Test append
        let append_data = b" Filesystem test";
        fs.append(test_path, append_data).await.unwrap();

        let full_data = fs.read(test_path).await.unwrap();
        assert_eq!(full_data.len(), test_data.len() + append_data.len());

        // Test directory operations
        let dir_path = "test_dir";
        fs.create_dir(dir_path).await.unwrap();
        assert!(fs.exists(dir_path).await.unwrap());

        let dir_metadata = fs.metadata(dir_path).await.unwrap();
        assert!(dir_metadata.is_directory);

        // Test list directory
        let entries = fs.list(".").await.unwrap();
        assert!(entries.len() >= 2); // At least test_file.txt and test_dir

        // Test copy
        let copy_path = "test_file_copy.txt";
        fs.copy(test_path, copy_path).await.unwrap();
        assert!(fs.exists(copy_path).await.unwrap());

        // Test delete
        fs.delete(test_path).await.unwrap();
        assert!(!fs.exists(test_path).await.unwrap());

        fs.delete(copy_path).await.unwrap();
        fs.delete(dir_path).await.unwrap();
    }
}
