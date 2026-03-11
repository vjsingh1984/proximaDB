// Encrypted filesystem wrapper
//
// Provides transparent encryption for whole-file filesystem operations.
// This keeps storage engines unchanged while moving encryption concerns into
// a single adapter layer.

use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use crate::storage::encryption::{FileEncryptionLayer, KeyVersionManager};
use crate::storage::persistence::filesystem::{
    DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError, FilesystemFile, FsResult,
};

/// Encrypted filesystem wrapper that transparently encrypts data at rest.
///
/// This adapter is intentionally whole-file oriented today. Standard read/write
/// calls work transparently, but streaming handles are rejected until a correct
/// truncate/rewrite story exists for encrypted files.
pub struct EncryptedFilesystem {
    /// Underlying filesystem (local, S3, Azure, GCS, etc.).
    underlying: Arc<dyn FileSystem>,
    /// File encryption layer.
    encryption: Arc<FileEncryptionLayer>,
    /// Optional extension used to distinguish encrypted files on disk.
    encrypted_extension: Option<String>,
}

impl std::fmt::Debug for EncryptedFilesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFilesystem")
            .field("underlying", &"<dyn FileSystem>")
            .field("encryption_enabled", &self.encryption.is_enabled())
            .field("encrypted_extension", &self.encrypted_extension)
            .finish()
    }
}

impl EncryptedFilesystem {
    /// Create a new encrypted filesystem wrapper.
    pub fn new(
        underlying: Arc<dyn FileSystem>,
        key_manager: Arc<KeyVersionManager>,
        encryption_enabled: bool,
    ) -> Self {
        let encryption = Arc::new(FileEncryptionLayer::new(
            key_manager,
            encryption_enabled,
            4096,
        ));

        Self {
            underlying,
            encryption,
            encrypted_extension: Some(".enc".to_string()),
        }
    }

    /// Create with a custom encrypted file extension.
    pub fn with_extension(mut self, extension: String) -> Self {
        self.encrypted_extension = Some(extension);
        self
    }

    /// Use the same filename for encrypted files.
    pub fn without_extension(mut self) -> Self {
        self.encrypted_extension = None;
        self
    }

    fn actual_path(&self, path: &str) -> String {
        if let Some(ref ext) = self.encrypted_extension {
            if !path.ends_with(ext) {
                return format!("{}{}", path, ext);
            }
        }
        path.to_string()
    }

    fn crypto_error(action: &str, path: &str, err: impl std::fmt::Display) -> FilesystemError {
        FilesystemError::InvalidOperation(format!("{} failed for {}: {}", action, path, err))
    }

    fn slice_range(data: &[u8], range: Range<u64>) -> Vec<u8> {
        let start = range.start as usize;
        if start >= data.len() {
            return vec![];
        }

        let end = (range.end as usize).min(data.len());
        if end <= start {
            return vec![];
        }

        data[start..end].to_vec()
    }

    async fn encrypted_exists(&self, path: &str) -> FsResult<bool> {
        let actual_path = self.actual_path(path);
        if actual_path == path {
            return self.underlying.exists(path).await;
        }
        self.underlying.exists(&actual_path).await
    }

    async fn read_encrypted(&self, path: &str) -> FsResult<Option<Vec<u8>>> {
        let actual_path = self.actual_path(path);
        if !self.encrypted_exists(path).await? {
            return Ok(None);
        }

        if !self.encryption.is_enabled() {
            return self.underlying.read(&actual_path).await.map(Some);
        }

        let encrypted = self.underlying.read(&actual_path).await?;
        let decrypted = self
            .encryption
            .decrypt_file(path, &encrypted)
            .map_err(|e| Self::crypto_error("decryption", path, e))?;
        Ok(Some(decrypted))
    }

    async fn write_encrypted(
        &self,
        path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let actual_path = self.actual_path(path);
        let encrypted = self
            .encryption
            .encrypt_file(path, data)
            .map_err(|e| Self::crypto_error("encryption", path, e))?;
        self.underlying
            .write(&actual_path, &encrypted, options)
            .await
    }
}

#[async_trait]
impl FileSystem for EncryptedFilesystem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn read(&self, path: &str) -> FsResult<Vec<u8>> {
        if let Some(decrypted) = self.read_encrypted(path).await? {
            return Ok(decrypted);
        }

        debug!(
            "Reading plaintext file for backward compatibility: {}",
            path
        );
        self.underlying.read(path).await
    }

    async fn get_mmap(&self, _path: &str) -> FsResult<Option<memmap2::Mmap>> {
        // Encrypted data cannot be exposed as a stable mmap without a decrypted
        // materialization layer, so fall back to regular reads.
        Ok(None)
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> FsResult<Vec<u8>> {
        if let Some(decrypted) = self.read_encrypted(path).await? {
            return Ok(Self::slice_range(
                &decrypted,
                offset..offset.saturating_add(length),
            ));
        }

        self.underlying.read_range(path, offset, length).await
    }

    async fn read_ranges(
        &self,
        path: &str,
        ranges: Vec<std::ops::Range<u64>>,
    ) -> FsResult<Vec<Vec<u8>>> {
        if let Some(decrypted) = self.read_encrypted(path).await? {
            return Ok(ranges
                .into_iter()
                .map(|range| Self::slice_range(&decrypted, range))
                .collect());
        }

        self.underlying.read_ranges(path, ranges).await
    }

    async fn write(&self, path: &str, data: &[u8], options: Option<FileOptions>) -> FsResult<()> {
        self.write_encrypted(path, data, options).await
    }

    async fn sync_file(&self, path: &str) -> FsResult<()> {
        if self.encrypted_exists(path).await? {
            let actual_path = self.actual_path(path);
            return self.underlying.sync_file(&actual_path).await;
        }

        self.underlying.sync_file(path).await
    }

    async fn append(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let mut existing = if self.exists(path).await? {
            self.read(path).await?
        } else {
            Vec::new()
        };
        existing.extend_from_slice(data);
        self.write(path, &existing, None).await
    }

    async fn delete(&self, path: &str) -> FsResult<()> {
        let actual_path = self.actual_path(path);
        let mut deleted = false;

        if self.encrypted_exists(path).await? {
            self.underlying.delete(&actual_path).await?;
            deleted = true;
        }

        if actual_path != path && self.underlying.exists(path).await? {
            self.underlying.delete(path).await?;
            deleted = true;
        }

        if deleted {
            Ok(())
        } else {
            Err(FilesystemError::NotFound(format!(
                "File not found: {}",
                path
            )))
        }
    }

    async fn exists(&self, path: &str) -> FsResult<bool> {
        if self.encrypted_exists(path).await? {
            return Ok(true);
        }
        self.underlying.exists(path).await
    }

    async fn metadata(&self, path: &str) -> FsResult<FileMetadata> {
        if self.encrypted_exists(path).await? {
            let actual_path = self.actual_path(path);
            let mut metadata = self.underlying.metadata(&actual_path).await?;
            metadata.path = path.to_string();

            if let Ok(header) = self.underlying.read_range(&actual_path, 0, 37).await {
                if let Ok(encrypted_metadata) = self.encryption.get_metadata(&header) {
                    metadata.size = encrypted_metadata.original_size;
                }
            }

            return Ok(metadata);
        }

        self.underlying.metadata(path).await
    }

    async fn list(&self, path: &str) -> FsResult<Vec<DirEntry>> {
        self.underlying.list(path).await
    }

    async fn create_dir(&self, path: &str) -> FsResult<()> {
        self.underlying.create_dir(path).await
    }

    async fn create_dir_all(&self, path: &str) -> FsResult<()> {
        self.underlying.create_dir_all(path).await
    }

    async fn copy(&self, from: &str, to: &str) -> FsResult<()> {
        let data = self.read(from).await?;
        self.write(
            to,
            &data,
            Some(FileOptions {
                create_dirs: true,
                overwrite: true,
                ..Default::default()
            }),
        )
        .await
    }

    async fn move_file(&self, from: &str, to: &str) -> FsResult<()> {
        self.copy(from, to).await?;
        self.delete(from).await
    }

    fn filesystem_type(&self) -> &'static str {
        "encrypted"
    }

    async fn write_atomic(
        &self,
        path: &str,
        data: &[u8],
        options: Option<FileOptions>,
    ) -> FsResult<()> {
        let actual_path = self.actual_path(path);
        let encrypted = self
            .encryption
            .encrypt_file(path, data)
            .map_err(|e| Self::crypto_error("encryption", path, e))?;
        self.underlying
            .write_atomic(&actual_path, &encrypted, options)
            .await
    }

    async fn sync(&self) -> FsResult<()> {
        self.underlying.sync().await
    }

    async fn open_file(&self, path: &str, _create: bool) -> FsResult<Box<dyn FilesystemFile>> {
        Err(FilesystemError::InvalidOperation(format!(
            "Streaming open_file is not wired for encrypted filesystem: {}",
            path
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::storage::encryption::KeyManager;
    use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

    fn test_key_manager(seed: u8) -> Arc<KeyVersionManager> {
        Arc::new(KeyVersionManager::new(Arc::new(KeyManager::new(
            [seed; 32],
        ))))
    }

    #[tokio::test]
    async fn test_encrypted_filesystem_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let underlying: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config).await.unwrap());
        let fs = EncryptedFilesystem::new(underlying.clone(), test_key_manager(7), true);
        let logical_path = temp_dir.path().join("roundtrip.bin");
        let logical_path = logical_path.to_string_lossy().to_string();
        let encrypted_path = format!("{}.enc", logical_path);

        fs.write(&logical_path, b"secret-data", None).await.unwrap();

        assert!(underlying.exists(&encrypted_path).await.unwrap());
        assert_eq!(fs.read(&logical_path).await.unwrap(), b"secret-data");
        assert_ne!(
            underlying.read(&encrypted_path).await.unwrap(),
            b"secret-data".to_vec()
        );

        let metadata = fs.metadata(&logical_path).await.unwrap();
        assert_eq!(metadata.path, logical_path);
        assert_eq!(metadata.size, b"secret-data".len() as u64);

        assert_eq!(fs.read_range(&logical_path, 1, 6).await.unwrap(), b"ecret-");
    }

    #[tokio::test]
    async fn test_encrypted_filesystem_reads_plaintext_files() {
        let temp_dir = TempDir::new().unwrap();
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let underlying: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config).await.unwrap());
        let fs = EncryptedFilesystem::new(underlying.clone(), test_key_manager(9), true);
        let logical_path = temp_dir.path().join("plain.txt");
        let logical_path = logical_path.to_string_lossy().to_string();

        underlying
            .write(&logical_path, b"plain-data", None)
            .await
            .unwrap();

        assert!(fs.exists(&logical_path).await.unwrap());
        assert_eq!(fs.read(&logical_path).await.unwrap(), b"plain-data");
        assert_eq!(fs.read_range(&logical_path, 6, 4).await.unwrap(), b"data");
    }

    #[tokio::test]
    async fn test_encrypted_filesystem_disabled_mode_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let underlying: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config).await.unwrap());
        let fs = EncryptedFilesystem::new(underlying.clone(), test_key_manager(10), false);
        let logical_path = temp_dir.path().join("disabled.txt");
        let logical_path = logical_path.to_string_lossy().to_string();
        let encrypted_path = format!("{}.enc", logical_path);

        fs.write(&logical_path, b"plain-disabled", None)
            .await
            .unwrap();

        assert!(underlying.exists(&encrypted_path).await.unwrap());
        assert_eq!(
            underlying.read(&encrypted_path).await.unwrap(),
            b"plain-disabled"
        );
        assert_eq!(fs.read(&logical_path).await.unwrap(), b"plain-disabled");
    }

    #[tokio::test]
    async fn test_open_file_returns_explicit_error() {
        let temp_dir = TempDir::new().unwrap();
        let config = LocalConfig {
            root_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let underlying: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config).await.unwrap());
        let fs = EncryptedFilesystem::new(underlying, test_key_manager(11), true);
        let logical_path = temp_dir.path().join("stream.bin");
        let logical_path = logical_path.to_string_lossy().to_string();

        let err = fs.open_file(&logical_path, true).await.unwrap_err();
        assert!(matches!(err, FilesystemError::InvalidOperation(_)));
    }
}
