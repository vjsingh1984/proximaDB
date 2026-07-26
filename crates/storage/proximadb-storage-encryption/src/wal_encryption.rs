// WAL encryption layer
//
// Provides transparent encryption for WAL segments:
// - Automatic encryption on write
// - Automatic decryption on read
// - Per-segment key derivation
// - Key version tracking in segment metadata

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::key_manager::{KeyVersionId, KeyVersionManager};
use super::{
    decrypt_data, decrypt_data_with_aad, decrypt_data_with_nonce_and_aad, encrypt_data,
    encrypt_data_with_nonce_and_aad, generate_aes_gcm_nonce,
};

/// WAL segment metadata with encryption info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalSegmentMetadata {
    /// Segment name
    pub segment_name: String,
    /// Segment ID
    pub segment_id: u64,
    /// Key version used for encryption
    pub key_version: KeyVersionId,
    /// Whether the segment is encrypted
    pub encrypted: bool,
    /// Segment size in bytes
    pub size_bytes: u64,
    /// AES-GCM nonce for self-describing WAL envelopes. Legacy segments keep
    /// the nonce prefixed to the ciphertext and deserialize this as `None`.
    #[serde(default)]
    pub nonce: Option<Vec<u8>>,
}

/// WAL encryption layer
pub struct WALEncryptionLayer {
    /// Key version manager
    key_manager: Arc<KeyVersionManager>,
    /// Whether encryption is enabled
    encryption_enabled: bool,
}

impl WALEncryptionLayer {
    /// Create a new WAL encryption layer
    pub fn new(key_manager: Arc<KeyVersionManager>, encryption_enabled: bool) -> Self {
        Self {
            key_manager,
            encryption_enabled,
        }
    }

    /// Encrypt WAL segment before writing
    pub fn encrypt_segment(
        &self,
        segment_name: &str,
        segment_id: u64,
        plaintext: &[u8],
    ) -> Result<(Vec<u8>, WalSegmentMetadata)> {
        if !self.encryption_enabled {
            // Return plaintext unchanged
            let metadata = WalSegmentMetadata {
                segment_name: segment_name.to_string(),
                segment_id,
                key_version: self.key_manager.current_version(),
                encrypted: false,
                size_bytes: plaintext.len() as u64,
                nonce: None,
            };
            return Ok((plaintext.to_vec(), metadata));
        }

        let key_version = self.key_manager.current_version();
        let key = self.key_manager.derive_wal_key(segment_name, segment_id);

        let ciphertext = encrypt_data(&key, plaintext)?;

        let metadata = WalSegmentMetadata {
            segment_name: segment_name.to_string(),
            segment_id,
            key_version,
            encrypted: true,
            size_bytes: ciphertext.len() as u64,
            nonce: None,
        };

        debug!(
            "Encrypted WAL segment {}:{} ({} -> {} bytes)",
            segment_name,
            segment_id,
            plaintext.len(),
            ciphertext.len()
        );

        Ok((ciphertext, metadata))
    }

    pub fn prepare_segment_metadata(
        &self,
        segment_name: &str,
        segment_id: u64,
        plaintext_len: usize,
    ) -> WalSegmentMetadata {
        WalSegmentMetadata {
            segment_name: segment_name.to_string(),
            segment_id,
            key_version: self.key_manager.current_version(),
            encrypted: self.encryption_enabled,
            size_bytes: if self.encryption_enabled {
                plaintext_len.saturating_add(16) as u64
            } else {
                plaintext_len as u64
            },
            nonce: self
                .encryption_enabled
                .then(|| generate_aes_gcm_nonce().to_vec()),
        }
    }

    pub fn encrypt_segment_with_metadata_and_aad(
        &self,
        metadata: &WalSegmentMetadata,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>> {
        if !metadata.encrypted {
            return Ok(plaintext.to_vec());
        }
        let key = self
            .key_manager
            .derive_wal_key(&metadata.segment_name, metadata.segment_id);
        let nonce: [u8; 12] = metadata
            .nonce
            .as_deref()
            .context("WAL envelope encryption metadata has no nonce")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("WAL envelope nonce must be 12 bytes"))?;
        encrypt_data_with_nonce_and_aad(&key, plaintext, &nonce, aad)
    }

    pub fn decrypt_segment_with_aad(
        &self,
        metadata: &WalSegmentMetadata,
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>> {
        if !metadata.encrypted {
            return Ok(ciphertext.to_vec());
        }
        let key = self
            .key_manager
            .derive_wal_key(&metadata.segment_name, metadata.segment_id);
        if let Some(nonce) = metadata.nonce.as_deref() {
            let nonce: [u8; 12] = nonce
                .try_into()
                .map_err(|_| anyhow::anyhow!("WAL envelope nonce must be 12 bytes"))?;
            decrypt_data_with_nonce_and_aad(&key, ciphertext, &nonce, aad)
        } else {
            decrypt_data_with_aad(&key, ciphertext, aad)
        }
    }

    /// Decrypt WAL segment after reading
    pub fn decrypt_segment(
        &self,
        metadata: &WalSegmentMetadata,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        if !metadata.encrypted {
            // Return ciphertext unchanged (already plaintext)
            return Ok(ciphertext.to_vec());
        }

        let key = self
            .key_manager
            .derive_wal_key(&metadata.segment_name, metadata.segment_id);

        let plaintext = decrypt_data(&key, ciphertext)?;

        debug!(
            "Decrypted WAL segment {}:{} ({} -> {} bytes)",
            metadata.segment_name,
            metadata.segment_id,
            ciphertext.len(),
            plaintext.len()
        );

        Ok(plaintext)
    }

    /// Check if a segment needs re-encryption due to key rotation
    pub fn needs_reencryption(&self, metadata: &WalSegmentMetadata) -> bool {
        if !metadata.encrypted {
            return false;
        }

        // Check if the key version is still active
        let current_version = self.key_manager.current_version();
        metadata.key_version != current_version
    }

    /// Re-encrypt a WAL segment with the current key version
    pub fn reencrypt_segment(
        &self,
        old_metadata: &WalSegmentMetadata,
        ciphertext: &[u8],
    ) -> Result<(Vec<u8>, WalSegmentMetadata)> {
        // First decrypt with old key
        let plaintext = self.decrypt_segment(old_metadata, ciphertext)?;

        // Then encrypt with new key
        let (new_ciphertext, new_metadata) = self.encrypt_segment(
            &old_metadata.segment_name,
            old_metadata.segment_id,
            &plaintext,
        )?;

        debug!(
            "Re-encrypted WAL segment {}:{} from key version {} to {}",
            old_metadata.segment_name,
            old_metadata.segment_id,
            old_metadata.key_version,
            new_metadata.key_version
        );

        Ok((new_ciphertext, new_metadata))
    }

    /// Create encrypted WAL segment file path
    pub fn encrypted_segment_path(&self, base_dir: &Path, segment_name: &str) -> PathBuf {
        base_dir.join(format!("{}.encrypted", segment_name))
    }

    /// Check if encryption is enabled
    pub fn is_enabled(&self) -> bool {
        self.encryption_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_manager::KeyManager;

    #[test]
    fn test_wal_encryption_round_trip() {
        // Set test master key
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_CRYPTO_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager = std::sync::Arc::new(
            KeyManager::from_env("TEST_PROXIMADB_CRYPTO_MASTER_KEY")
                .expect("failed to create key manager from env"),
        );
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let layer = WALEncryptionLayer::new(key_version_manager.clone(), true);

        let segment_name = "wal_0001";
        let segment_id = 1;
        let plaintext = b"WAL segment data";

        // Encrypt
        let (ciphertext, metadata) = layer
            .encrypt_segment(segment_name, segment_id, plaintext)
            .expect("failed to encrypt segment");

        assert!(metadata.encrypted);
        assert_ne!(ciphertext, plaintext.to_vec());

        // Decrypt
        let decrypted = layer
            .decrypt_segment(&metadata, &ciphertext)
            .expect("failed to decrypt segment");
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn test_wal_encryption_disabled() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_CRYPTO_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager = std::sync::Arc::new(
            KeyManager::from_env("TEST_PROXIMADB_CRYPTO_MASTER_KEY")
                .expect("failed to create key manager from env"),
        );
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let layer = WALEncryptionLayer::new(key_version_manager.clone(), false);

        let segment_name = "wal_0001";
        let segment_id = 1;
        let plaintext = b"WAL segment data";

        // Encrypt (should be no-op when disabled)
        let (ciphertext, metadata) = layer
            .encrypt_segment(segment_name, segment_id, plaintext)
            .expect("failed to encrypt segment");

        assert!(!metadata.encrypted);
        assert_eq!(ciphertext, plaintext.to_vec());

        // Decrypt (should be no-op)
        let decrypted = layer
            .decrypt_segment(&metadata, &ciphertext)
            .expect("failed to decrypt segment");
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    fn envelope_nonce_and_aad_are_authenticated() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_CRYPTO_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }
        let key_manager = std::sync::Arc::new(
            KeyManager::from_env("TEST_PROXIMADB_CRYPTO_MASTER_KEY")
                .expect("failed to create key manager"),
        );
        let layer = WALEncryptionLayer::new(
            std::sync::Arc::new(KeyVersionManager::new(key_manager)),
            true,
        );
        let metadata = layer.prepare_segment_metadata("wal", 7, 7);
        assert_eq!(metadata.nonce.as_deref().map(<[u8]>::len), Some(12));
        let ciphertext = layer
            .encrypt_segment_with_metadata_and_aad(&metadata, b"payload", b"header")
            .unwrap();
        assert_eq!(
            layer
                .decrypt_segment_with_aad(&metadata, &ciphertext, b"header")
                .unwrap(),
            b"payload"
        );
        assert!(
            layer
                .decrypt_segment_with_aad(&metadata, &ciphertext, b"tampered")
                .is_err()
        );
    }

    #[test]
    fn test_reencryption_needed() {
        unsafe {
            std::env::set_var(
                "TEST_PROXIMADB_CRYPTO_MASTER_KEY",
                "test-master-key-32-bytes-long-here!!",
            );
        }

        let key_manager = std::sync::Arc::new(
            KeyManager::from_env("TEST_PROXIMADB_CRYPTO_MASTER_KEY")
                .expect("failed to create key manager from env"),
        );
        let key_version_manager = std::sync::Arc::new(KeyVersionManager::new(key_manager));

        let layer = WALEncryptionLayer::new(key_version_manager.clone(), true);

        let plaintext = b"WAL data";
        let (ciphertext, metadata) = layer
            .encrypt_segment("wal_0001", 1, plaintext)
            .expect("failed to encrypt segment");

        // Initially, no re-encryption needed
        assert!(!layer.needs_reencryption(&metadata));

        // Rotate key
        key_version_manager.rotate().expect("failed to rotate key");

        // Now re-encryption is needed
        assert!(layer.needs_reencryption(&metadata));

        // Re-encrypt
        let (new_ciphertext, new_metadata) = layer
            .reencrypt_segment(&metadata, &ciphertext)
            .expect("failed to re-encrypt segment");

        // Decrypt with new key
        let decrypted = layer
            .decrypt_segment(&new_metadata, &new_ciphertext)
            .expect("failed to decrypt segment");
        assert_eq!(decrypted, plaintext.to_vec());
    }
}
