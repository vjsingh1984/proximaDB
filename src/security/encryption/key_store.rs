//! Key Store for Field-Level Encryption
//!
//! Manages encryption keys with support for key rotation, versioning,
//! and secure storage. Keys are stored encrypted at rest using a master key.

use anyhow::Result;
use chrono::{DateTime, Utc};
use ring::aead::{AES_256_GCM, LessSafeKey, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

/// Errors from the key store
#[derive(Debug, Error)]
pub enum KeyStoreError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Key version not found: {key_id} v{version}")]
    KeyVersionNotFound { key_id: String, version: u32 },

    #[error("Key already exists: {0}")]
    KeyAlreadyExists(String),

    #[error("Key rotation failed: {0}")]
    RotationFailed(String),

    #[error("Master key not configured")]
    MasterKeyNotConfigured,

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Decryption error: {0}")]
    DecryptionError(String),

    #[error("Invalid key material: {0}")]
    InvalidKeyMaterial(String),
}

/// Key store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyStoreConfig {
    /// Enable key store
    pub enabled: bool,
    /// Master key for encrypting stored keys (base64 encoded)
    /// In production, this should come from a KMS
    pub master_key: Option<String>,
    /// Default key rotation period in days
    pub rotation_period_days: u32,
    /// Maximum number of key versions to retain
    pub max_versions: u32,
    /// Enable automatic key rotation
    pub auto_rotation: bool,
}

impl Default for KeyStoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            master_key: None,
            rotation_period_days: 90,
            max_versions: 5,
            auto_rotation: false,
        }
    }
}

/// A versioned encryption key
#[derive(Debug, Clone)]
pub struct EncryptionKey {
    /// Unique key identifier
    pub key_id: String,
    /// Current version number
    pub version: u32,
    /// Raw key material (256-bit for AES-256-GCM)
    key_material: Vec<u8>,
    /// When this key version was created
    pub created_at: DateTime<Utc>,
    /// When this key version expires (for rotation)
    pub expires_at: Option<DateTime<Utc>>,
    /// Purpose/description of this key
    pub purpose: String,
    /// Whether this key version is active for encryption
    pub active: bool,
}

impl EncryptionKey {
    /// Create a new encryption key with random material
    pub fn generate(
        key_id: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Result<Self, KeyStoreError> {
        let rng = SystemRandom::new();
        let mut key_material = vec![0u8; 32]; // 256 bits
        rng.fill(&mut key_material)
            .map_err(|_| KeyStoreError::EncryptionError("Failed to generate random key".into()))?;

        Ok(Self {
            key_id: key_id.into(),
            version: 1,
            key_material,
            created_at: Utc::now(),
            expires_at: None,
            purpose: purpose.into(),
            active: true,
        })
    }

    /// Create a key from existing material (for testing or import)
    pub fn from_material(
        key_id: impl Into<String>,
        material: Vec<u8>,
        purpose: impl Into<String>,
    ) -> Result<Self, KeyStoreError> {
        if material.len() != 32 {
            return Err(KeyStoreError::InvalidKeyMaterial(format!(
                "Key must be 256 bits (32 bytes), got {} bytes",
                material.len()
            )));
        }

        Ok(Self {
            key_id: key_id.into(),
            version: 1,
            key_material: material,
            created_at: Utc::now(),
            expires_at: None,
            purpose: purpose.into(),
            active: true,
        })
    }

    /// Get the key material (for internal use only)
    pub(crate) fn material(&self) -> &[u8] {
        &self.key_material
    }

    /// Create an AEAD key for encryption/decryption
    pub(crate) fn to_aead_key(&self) -> Result<LessSafeKey, KeyStoreError> {
        let unbound = UnboundKey::new(&AES_256_GCM, &self.key_material)
            .map_err(|_| KeyStoreError::InvalidKeyMaterial("Failed to create AEAD key".into()))?;
        Ok(LessSafeKey::new(unbound))
    }
}

/// Stored key with all versions
struct StoredKey {
    /// All versions of this key, indexed by version number
    versions: HashMap<u32, EncryptionKey>,
    /// Current active version
    current_version: u32,
}

/// Key store for managing encryption keys
pub struct KeyStore {
    /// Configuration
    config: KeyStoreConfig,
    /// Master key for encrypting stored keys
    master_key: Option<LessSafeKey>,
    /// Stored keys
    keys: RwLock<HashMap<String, StoredKey>>,
    /// Random number generator
    rng: SystemRandom,
}

impl KeyStore {
    /// Create a new key store
    pub fn new(config: KeyStoreConfig) -> Result<Self, KeyStoreError> {
        let master_key = if let Some(ref master_key_b64) = config.master_key {
            let master_bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, master_key_b64)
                    .map_err(|e| {
                        KeyStoreError::InvalidKeyMaterial(format!("Invalid master key: {}", e))
                    })?;

            if master_bytes.len() != 32 {
                return Err(KeyStoreError::InvalidKeyMaterial(format!(
                    "Master key must be 256 bits, got {} bits",
                    master_bytes.len() * 8
                )));
            }

            let unbound = UnboundKey::new(&AES_256_GCM, &master_bytes).map_err(|_| {
                KeyStoreError::InvalidKeyMaterial("Invalid master key material".into())
            })?;
            Some(LessSafeKey::new(unbound))
        } else {
            None
        };

        Ok(Self {
            config,
            master_key,
            keys: RwLock::new(HashMap::new()),
            rng: SystemRandom::new(),
        })
    }

    /// Generate and store a new encryption key
    pub fn create_key(
        &self,
        key_id: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Result<EncryptionKey, KeyStoreError> {
        let key_id = key_id.into();
        let purpose = purpose.into();

        let mut keys = self.keys.write().unwrap();
        if keys.contains_key(&key_id) {
            return Err(KeyStoreError::KeyAlreadyExists(key_id));
        }

        let key = EncryptionKey::generate(&key_id, &purpose)?;
        let key_clone = key.clone();

        let stored = StoredKey {
            versions: HashMap::from([(1, key)]),
            current_version: 1,
        };

        keys.insert(key_id, stored);
        Ok(key_clone)
    }

    /// Import an existing key
    pub fn import_key(&self, key: EncryptionKey) -> Result<(), KeyStoreError> {
        let mut keys = self.keys.write().unwrap();
        if keys.contains_key(&key.key_id) {
            return Err(KeyStoreError::KeyAlreadyExists(key.key_id.clone()));
        }

        let key_id = key.key_id.clone();
        let version = key.version;

        let stored = StoredKey {
            versions: HashMap::from([(version, key)]),
            current_version: version,
        };

        keys.insert(key_id, stored);
        Ok(())
    }

    /// Get the current (active) version of a key
    pub fn get_key(&self, key_id: &str) -> Result<EncryptionKey, KeyStoreError> {
        let keys = self.keys.read().unwrap();
        let stored = keys
            .get(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        stored
            .versions
            .get(&stored.current_version)
            .cloned()
            .ok_or_else(|| KeyStoreError::KeyVersionNotFound {
                key_id: key_id.to_string(),
                version: stored.current_version,
            })
    }

    /// Get a specific version of a key (for decryption of old data)
    pub fn get_key_version(
        &self,
        key_id: &str,
        version: u32,
    ) -> Result<EncryptionKey, KeyStoreError> {
        let keys = self.keys.read().unwrap();
        let stored = keys
            .get(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        stored
            .versions
            .get(&version)
            .cloned()
            .ok_or_else(|| KeyStoreError::KeyVersionNotFound {
                key_id: key_id.to_string(),
                version,
            })
    }

    /// Rotate a key, creating a new version
    pub fn rotate_key(&self, key_id: &str) -> Result<EncryptionKey, KeyStoreError> {
        let mut keys = self.keys.write().unwrap();
        let stored = keys
            .get_mut(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        // Get the old key for reference
        let old_key = stored
            .versions
            .get(&stored.current_version)
            .ok_or_else(|| KeyStoreError::RotationFailed("Current version not found".into()))?;

        // Generate new key material
        let mut new_material = vec![0u8; 32];
        self.rng.fill(&mut new_material).map_err(|_| {
            KeyStoreError::RotationFailed("Failed to generate new key material".into())
        })?;

        let new_version = stored.current_version + 1;
        let new_key = EncryptionKey {
            key_id: key_id.to_string(),
            version: new_version,
            key_material: new_material,
            created_at: Utc::now(),
            expires_at: None,
            purpose: old_key.purpose.clone(),
            active: true,
        };

        // Mark old version as inactive
        if let Some(old) = stored.versions.get_mut(&stored.current_version) {
            old.active = false;
        }

        // Store new version
        let new_key_clone = new_key.clone();
        stored.versions.insert(new_version, new_key);
        stored.current_version = new_version;

        // Prune old versions if exceeding max
        self.prune_old_versions(stored);

        Ok(new_key_clone)
    }

    /// Remove old versions beyond max_versions
    fn prune_old_versions(&self, stored: &mut StoredKey) {
        if stored.versions.len() > self.config.max_versions as usize {
            let mut versions: Vec<u32> = stored.versions.keys().cloned().collect();
            versions.sort();

            // Remove oldest versions, keeping max_versions
            let to_remove = versions.len() - self.config.max_versions as usize;
            for version in versions.into_iter().take(to_remove) {
                stored.versions.remove(&version);
            }
        }
    }

    /// Delete a key and all its versions
    pub fn delete_key(&self, key_id: &str) -> Result<(), KeyStoreError> {
        let mut keys = self.keys.write().unwrap();
        keys.remove(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;
        Ok(())
    }

    /// List all key IDs
    pub fn list_keys(&self) -> Vec<String> {
        let keys = self.keys.read().unwrap();
        keys.keys().cloned().collect()
    }

    /// Check if a key exists
    pub fn key_exists(&self, key_id: &str) -> bool {
        let keys = self.keys.read().unwrap();
        keys.contains_key(key_id)
    }

    /// Get key metadata (without material)
    pub fn get_key_info(&self, key_id: &str) -> Result<KeyInfo, KeyStoreError> {
        let keys = self.keys.read().unwrap();
        let stored = keys
            .get(key_id)
            .ok_or_else(|| KeyStoreError::KeyNotFound(key_id.to_string()))?;

        let current = stored
            .versions
            .get(&stored.current_version)
            .ok_or_else(|| KeyStoreError::KeyVersionNotFound {
                key_id: key_id.to_string(),
                version: stored.current_version,
            })?;

        Ok(KeyInfo {
            key_id: key_id.to_string(),
            current_version: stored.current_version,
            versions: stored.versions.keys().cloned().collect(),
            purpose: current.purpose.clone(),
            created_at: current.created_at,
            active: current.active,
        })
    }
}

/// Key information without sensitive material
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub key_id: String,
    pub current_version: u32,
    pub versions: Vec<u32>,
    pub purpose: String,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> KeyStoreConfig {
        KeyStoreConfig {
            enabled: true,
            master_key: None,
            rotation_period_days: 90,
            max_versions: 3,
            auto_rotation: false,
        }
    }

    #[test]
    fn test_create_key() {
        let store = KeyStore::new(test_config()).unwrap();
        let key = store.create_key("test-key", "Test encryption key").unwrap();

        assert_eq!(key.key_id, "test-key");
        assert_eq!(key.version, 1);
        assert_eq!(key.key_material.len(), 32);
        assert!(key.active);
    }

    #[test]
    fn test_get_key() {
        let store = KeyStore::new(test_config()).unwrap();
        store.create_key("test-key", "Test").unwrap();

        let key = store.get_key("test-key").unwrap();
        assert_eq!(key.key_id, "test-key");
    }

    #[test]
    fn test_key_not_found() {
        let store = KeyStore::new(test_config()).unwrap();
        let result = store.get_key("nonexistent");
        assert!(matches!(result, Err(KeyStoreError::KeyNotFound(_))));
    }

    #[test]
    fn test_duplicate_key() {
        let store = KeyStore::new(test_config()).unwrap();
        store.create_key("test-key", "Test").unwrap();

        let result = store.create_key("test-key", "Another");
        assert!(matches!(result, Err(KeyStoreError::KeyAlreadyExists(_))));
    }

    #[test]
    fn test_key_rotation() {
        let store = KeyStore::new(test_config()).unwrap();
        let key1 = store.create_key("test-key", "Test").unwrap();

        let key2 = store.rotate_key("test-key").unwrap();

        assert_eq!(key2.version, 2);
        assert_ne!(key1.key_material, key2.key_material);

        // Current key should be version 2
        let current = store.get_key("test-key").unwrap();
        assert_eq!(current.version, 2);

        // Old version should still be retrievable
        let old = store.get_key_version("test-key", 1).unwrap();
        assert_eq!(old.version, 1);
        assert!(!old.active);
    }

    #[test]
    fn test_version_pruning() {
        let store = KeyStore::new(test_config()).unwrap();
        store.create_key("test-key", "Test").unwrap();

        // Rotate 4 times (config has max_versions = 3)
        for _ in 0..4 {
            store.rotate_key("test-key").unwrap();
        }

        let info = store.get_key_info("test-key").unwrap();
        assert_eq!(info.versions.len(), 3);
        assert_eq!(info.current_version, 5);

        // Version 1 and 2 should be pruned
        assert!(store.get_key_version("test-key", 1).is_err());
        assert!(store.get_key_version("test-key", 2).is_err());

        // Versions 3, 4, 5 should exist
        assert!(store.get_key_version("test-key", 3).is_ok());
        assert!(store.get_key_version("test-key", 4).is_ok());
        assert!(store.get_key_version("test-key", 5).is_ok());
    }

    #[test]
    fn test_delete_key() {
        let store = KeyStore::new(test_config()).unwrap();
        store.create_key("test-key", "Test").unwrap();

        assert!(store.key_exists("test-key"));
        store.delete_key("test-key").unwrap();
        assert!(!store.key_exists("test-key"));
    }

    #[test]
    fn test_list_keys() {
        let store = KeyStore::new(test_config()).unwrap();
        store.create_key("key1", "Test 1").unwrap();
        store.create_key("key2", "Test 2").unwrap();
        store.create_key("key3", "Test 3").unwrap();

        let keys = store.list_keys();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"key1".to_string()));
        assert!(keys.contains(&"key2".to_string()));
        assert!(keys.contains(&"key3".to_string()));
    }

    #[test]
    fn test_import_key() {
        let store = KeyStore::new(test_config()).unwrap();

        let material = vec![0u8; 32];
        let key =
            EncryptionKey::from_material("imported", material.clone(), "Imported key").unwrap();

        store.import_key(key).unwrap();

        let retrieved = store.get_key("imported").unwrap();
        assert_eq!(retrieved.key_id, "imported");
        assert_eq!(retrieved.key_material, material);
    }

    #[test]
    fn test_invalid_key_material() {
        let result = EncryptionKey::from_material("test", vec![0u8; 16], "Too short");
        assert!(matches!(result, Err(KeyStoreError::InvalidKeyMaterial(_))));
    }

    #[test]
    fn test_aead_key_creation() {
        let key = EncryptionKey::generate("test", "Test").unwrap();
        let aead = key.to_aead_key();
        assert!(aead.is_ok());
    }
}
