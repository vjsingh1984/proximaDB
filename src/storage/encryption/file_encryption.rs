// File encryption layer for data files
//
// Provides transparent encryption for:
// - SST files
// - Parquet files
// - Vector storage files
//
// Features:
// - Chunked encryption (4KB blocks) for large files
// - Parallel encryption with Rayon
// - Memory-mapped file support
// - Per-file key derivation

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::key_manager::{KeyVersionId, KeyVersionManager};
use super::{decrypt_data, encrypt_data, nonce_size, tag_size};

/// Encrypted file metadata (stored in file header)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedFileMetadata {
    /// File magic number (for validation)
    pub magic: [u8; 4],
    /// Version (for format evolution)
    pub version: u8,
    /// Key version used for encryption
    pub key_version: KeyVersionId,
    /// Chunk size in bytes
    pub chunk_size: usize,
    /// Original file size (unencrypted)
    pub original_size: u64,
    /// Number of chunks
    pub num_chunks: u64,
}

impl EncryptedFileMetadata {
    /// Magic number for encrypted files
    pub const MAGIC: [u8; 4] = *b"PEDE"; // ProximaDB Encrypted Data

    /// Current version
    pub const VERSION: u8 = 1;

    /// Create new metadata
    pub fn new(
        key_version: KeyVersionId,
        chunk_size: usize,
        original_size: u64,
        num_chunks: u64,
    ) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            key_version,
            chunk_size,
            original_size,
            num_chunks,
        }
    }

    /// Serialize to bytes
    ///
    /// CRITICAL: Returns Result to properly handle serialization failures.
    /// Using unwrap_or_default() would cause silent data corruption by
    /// returning empty metadata, breaking all subsequent file operations.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize encrypted file metadata: {}", e))
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let metadata: EncryptedFileMetadata = bincode::deserialize(data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?;

        // Validate magic number
        if metadata.magic != Self::MAGIC {
            return Err(anyhow::anyhow!(
                "Invalid magic number: {:?}",
                metadata.magic
            ));
        }

        Ok(metadata)
    }

    /// Size of serialized metadata
    pub fn serialized_size(&self) -> usize {
        // MAGIC (4) + version (1) + key_version (8) + chunk_size (8) + original_size (8) + num_chunks (8)
        37
    }
}

/// File encryption layer
pub struct FileEncryptionLayer {
    /// Key version manager
    key_manager: Arc<KeyVersionManager>,
    /// Whether encryption is enabled
    encryption_enabled: bool,
    /// Default chunk size
    chunk_size: usize,
}

impl FileEncryptionLayer {
    /// Create a new file encryption layer
    pub fn new(
        key_manager: Arc<KeyVersionManager>,
        encryption_enabled: bool,
        chunk_size: usize,
    ) -> Self {
        Self {
            key_manager,
            encryption_enabled,
            chunk_size,
        }
    }

    /// Encrypt a file in chunks
    pub fn encrypt_file(&self, file_path: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        if !self.encryption_enabled {
            return Ok(plaintext.to_vec());
        }

        let key_version = self.key_manager.current_version();
        let chunk_size = self.chunk_size;
        let num_chunks = (plaintext.len() + chunk_size - 1) / chunk_size;

        // Derive file-specific key
        let key = self.key_manager.derive_sst_key(file_path);

        // Encrypt chunks in parallel
        //
        // CRITICAL: We must propagate encryption failures rather than using unwrap_or_default(),
        // which would silently insert empty chunks and corrupt the encrypted file.
        let encrypted_chunks: Result<Vec<Vec<u8>>> = plaintext
            .par_chunks(chunk_size)
            .enumerate()
            .map(|(chunk_id, chunk)| {
                let chunk_key = Self::derive_chunk_key(&key, chunk_id);
                encrypt_data(&chunk_key, chunk)
                    .map_err(|e| anyhow::anyhow!("Failed to encrypt chunk {}: {}", chunk_id, e))
            })
            .collect();
        let encrypted_chunks = encrypted_chunks?;

        // Calculate total size
        let metadata = EncryptedFileMetadata::new(
            key_version,
            chunk_size,
            plaintext.len() as u64,
            num_chunks as u64,
        );

        let metadata_bytes = metadata
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("Failed to serialize file metadata: {}", e))?;

        // Combine metadata + encrypted chunks
        let total_size =
            metadata_bytes.len() + encrypted_chunks.iter().map(|c| c.len()).sum::<usize>();
        let mut result = Vec::with_capacity(total_size);
        result.extend_from_slice(&metadata_bytes);

        for chunk in encrypted_chunks {
            result.extend_from_slice(&chunk);
        }

        debug!(
            "Encrypted file {} ({} bytes -> {} bytes, {} chunks)",
            file_path,
            plaintext.len(),
            result.len(),
            num_chunks
        );

        Ok(result)
    }

    /// Decrypt a file
    pub fn decrypt_file(&self, file_path: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if ciphertext.len() < 37 {
            return Err(anyhow::anyhow!("File too short to be encrypted"));
        }

        // Parse metadata
        let metadata = EncryptedFileMetadata::from_bytes(&ciphertext[..37])?;

        if !self.encryption_enabled || metadata.num_chunks == 0 {
            // File is not encrypted, return as-is (skipping metadata)
            return Ok(ciphertext.to_vec());
        }

        let key = self.key_manager.derive_sst_key(file_path);
        let chunk_size = metadata.chunk_size;

        // Skip metadata and decrypt chunks
        let encrypted_data = &ciphertext[37..];
        let mut decrypted_chunks = Vec::with_capacity(metadata.num_chunks as usize);

        let mut offset = 0;
        for chunk_id in 0..metadata.num_chunks {
            // Calculate actual plaintext chunk size (last chunk may be smaller)
            let chunk_start = (chunk_id as usize) * chunk_size;
            let chunk_end_plaintext =
                std::cmp::min(chunk_start + chunk_size, metadata.original_size as usize);
            let actual_plaintext_size = chunk_end_plaintext - chunk_start;

            // Encrypted chunk size = actual plaintext + nonce + tag
            let encrypted_chunk_size = actual_plaintext_size + nonce_size() + tag_size();
            let chunk_end = offset + encrypted_chunk_size;

            if chunk_end > encrypted_data.len() {
                return Err(anyhow::anyhow!(
                    "Unexpected end of file at chunk {} (expected {} bytes, got {})",
                    chunk_id,
                    chunk_end,
                    encrypted_data.len()
                ));
            }

            let chunk = &encrypted_data[offset..chunk_end];
            let chunk_key = Self::derive_chunk_key(&key, chunk_id as usize);

            let decrypted = decrypt_data(&chunk_key, chunk)?;
            decrypted_chunks.push(decrypted);

            offset = chunk_end;
        }

        // Combine chunks
        let mut result = Vec::with_capacity(metadata.original_size as usize);
        for chunk in decrypted_chunks {
            result.extend_from_slice(&chunk);
        }

        debug!(
            "Decrypted file {} ({} bytes -> {} bytes)",
            file_path,
            ciphertext.len(),
            result.len()
        );

        Ok(result)
    }

    /// Encrypt file chunk with chunk-specific key
    fn derive_chunk_key(base_key: &[u8; 32], chunk_id: usize) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(base_key);
        hasher.update(&chunk_id.to_be_bytes());
        let result = hasher.finalize();

        let mut key = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        key
    }

    /// Create encrypted file path
    pub fn encrypted_file_path(&self, base_dir: &Path, file_name: &str) -> PathBuf {
        base_dir.join(format!("{}.encrypted", file_name))
    }

    /// Check if encryption is enabled
    pub fn is_enabled(&self) -> bool {
        self.encryption_enabled
    }

    /// Get file metadata without decrypting
    pub fn get_metadata(&self, ciphertext: &[u8]) -> Result<EncryptedFileMetadata> {
        if ciphertext.len() < 37 {
            return Err(anyhow::anyhow!("File too short"));
        }
        EncryptedFileMetadata::from_bytes(&ciphertext[..37])
    }

    /// Re-encrypt file with new key version
    pub fn reencrypt_file(&self, file_path: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
        // Decrypt file
        let plaintext = self.decrypt_file(file_path, ciphertext)?;

        // Re-encrypt with new key
        self.encrypt_file(file_path, &plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::encryption::key_manager::KeyManager;

    #[test]
    fn test_file_encryption_round_trip() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager =
            std::sync::Arc::new(KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap());
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let layer = FileEncryptionLayer::new(key_version_manager.clone(), true, 4096);

        let file_path = "test.sst";
        let plaintext = b"File data for testing encryption";

        // Encrypt
        let ciphertext = layer.encrypt_file(file_path, plaintext).unwrap();

        assert_ne!(ciphertext, plaintext.to_vec());

        // Decrypt
        let decrypted = layer.decrypt_file(file_path, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn test_large_file_chunking() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager =
            std::sync::Arc::new(KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap());
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let chunk_size = 100;
        let layer = FileEncryptionLayer::new(key_version_manager.clone(), true, chunk_size);

        // Create data larger than chunk size
        let plaintext = vec![42u8; 1000]; // 10 chunks

        let ciphertext = layer.encrypt_file("large.sst", &plaintext).unwrap();
        let decrypted = layer.decrypt_file("large.sst", &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_metadata_serialization() {
        let metadata = EncryptedFileMetadata::new(0, 4096, 1024, 1);

        let bytes = metadata.to_bytes().expect("Failed to serialize metadata");
        let deserialized = EncryptedFileMetadata::from_bytes(&bytes).unwrap();

        assert_eq!(deserialized.magic, EncryptedFileMetadata::MAGIC);
        assert_eq!(deserialized.version, 1);
        assert_eq!(deserialized.key_version, 0);
        assert_eq!(deserialized.chunk_size, 4096);
        assert_eq!(deserialized.original_size, 1024);
        assert_eq!(deserialized.num_chunks, 1);
    }

    #[test]
    fn test_file_encryption_disabled() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager =
            std::sync::Arc::new(KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap());
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let layer = FileEncryptionLayer::new(key_version_manager.clone(), false, 4096);

        let plaintext = b"File data";
        let ciphertext = layer.encrypt_file("test.sst", plaintext).unwrap();

        // When encryption is disabled, ciphertext == plaintext
        assert_eq!(ciphertext, plaintext.to_vec());
    }
}
