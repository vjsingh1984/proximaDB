// EncryptedFilesystem decorator integration tests.
//
// Relocated from `src/storage/encryption/filesystem.rs` when the encryption module was
// extracted into the `proximadb-storage-encryption` crate. These tests exercise the
// decorator over the real root-resident `LocalFileSystem` backend — which depends back on
// encryption (at-rest wrapping), so it cannot be reached from the leaf crate without a
// dependency cycle. Keeping the tests in root reuses the existing `LocalFileSystem` rather
// than introducing a second in-memory `FileSystem` implementation. Behaviour is unchanged.

use std::sync::Arc;

use tempfile::TempDir;

use crate::storage::encryption::{EncryptedFilesystem, KeyManager, KeyVersionManager};
use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};
use crate::storage::persistence::filesystem::{FileSystem, FilesystemError};

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

/// TD-OBJSTORE-4 S2/S3: the encryption decorator MUST delegate `write_if_absent`
/// to the backend's atomic conditional-create — not emulate it with an overwriting
/// `write`. The recovery commit primitive rests on this: a wrapper that clobbered
/// would let a crash-window re-replay silently overwrite a committed segment.
#[tokio::test]
async fn write_if_absent_delegates_through_encryption_wrapper() {
    let temp_dir = TempDir::new().unwrap();
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let underlying: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config).await.unwrap());
    let fs = EncryptedFilesystem::new(underlying, test_key_manager(13), true);

    let logical_path = temp_dir.path().join("commit.pax");
    let logical_path = logical_path.to_string_lossy().to_string();

    fs.write_if_absent(&logical_path, b"first", None)
        .await
        .expect("first conditional create succeeds");

    let err = fs
        .write_if_absent(&logical_path, b"second", None)
        .await
        .expect_err("second conditional create on the same key must fail");
    assert!(
        matches!(err, FilesystemError::AlreadyExists(_)),
        "encryption wrapper must surface the backend's AlreadyExists; got {err:?}"
    );

    assert_eq!(
        fs.read(&logical_path).await.expect("read back decrypts"),
        b"first",
        "first value must survive the rejected second create through encryption"
    );
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
