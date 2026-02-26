// Key management for encryption at rest
//
// Provides:
// - Secure in-memory key storage (never written to disk)
// - Key versioning for rotation support
// - Per-file key derivation
// - Master key management

use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::derive_key;

/// Key version identifier
pub type KeyVersionId = u64;

/// Key version with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyVersion {
    /// Version ID (monotonically increasing)
    pub id: KeyVersionId,
    /// Creation timestamp (Unix epoch)
    pub created_at: u64,
    /// Key expiration timestamp (None = never expires)
    pub expires_at: Option<u64>,
    /// Whether this version is active
    pub active: bool,
}

/// Key manager for encryption keys
pub struct KeyManager {
    /// Master key (256-bit)
    master_key: [u8; 8], // Store as u64 array for obfuscation
    /// Key versions by ID
    key_versions: Arc<RwLock<HashMap<KeyVersionId, KeyVersion>>>,
    /// Current active key version
    current_version: Arc<RwLock<KeyVersionId>>,
    /// Next version ID
    next_version_id: Arc<RwLock<KeyVersionId>>,
    /// Key rotation interval in seconds
    rotation_interval_secs: u64,
}

impl KeyManager {
    /// Create a new key manager from environment variable
    pub fn from_env(env_var: &str) -> Result<Self> {
        let master_key_str = env::var(env_var)
            .map_err(|_| anyhow::anyhow!("Master key environment variable '{}' not set", env_var))?;

        let master_key_bytes = master_key_str.as_bytes();
        if master_key_bytes.len() < 32 {
            return Err(anyhow::anyhow!(
                "Master key must be at least 32 bytes (got {} bytes)",
                master_key_bytes.len()
            ));
        }

        // Derive actual 256-bit master key using SHA-256
        let mut hasher = Sha256::new();
        hasher.update(master_key_bytes);
        let hash_result = hasher.finalize();
        let mut master_key = [0u8; 8];
        master_key.copy_from_slice(&hash_result[0..32]);

        let mut key_versions = HashMap::new();
        let version_0 = KeyVersion {
            id: 0,
            created_at: now(),
            expires_at: None,
            active: true,
        };
        key_versions.insert(0, version_0);

        Ok(Self {
            master_key,
            key_versions: Arc::new(RwLock::new(key_versions)),
            current_version: Arc::new(RwLock::new(0)),
            next_version_id: Arc::new(RwLock::new(1)),
            rotation_interval_secs: 30 * 24 * 3600, // 30 days default
        })
    }

    /// Get master key as bytes
    fn master_key_bytes(&self) -> [u8; 32] {
        // Reconstruct master key from u64 array
        let mut key = [0u8; 32];
        for i in 0..8 {
            let bytes = self.master_key[i].to_be_bytes();
            key[i * 4..i * 4 + 4].copy_from_slice(&bytes);
        }
        key
    }

    /// Derive encryption key for a specific context
    pub fn derive_context_key(&self, context: &[u8]) -> [u8; 32] {
        let master_key = self.master_key_bytes();
        derive_key(&master_key, context)
    }

    /// Get current active key version
    pub fn current_version(&self) -> KeyVersionId {
        *self.current_version.read().unwrap()
    }

    /// Create a new key version (for key rotation)
    pub fn rotate_key(&self) -> Result<KeyVersionId> {
        let new_version_id = {
            let mut next_id = self.next_version_id.write().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        let now = now();
        let expires_at = Some(now + self.rotation_interval_secs);

        let new_version = KeyVersion {
            id: new_version_id,
            created_at: now,
            expires_at,
            active: true,
        };

        // Deactivate old version
        {
            let mut versions = self.key_versions.write().unwrap();
            let old_version_id = *self.current_version.read().unwrap();
            if let Some(old_version) = versions.get_mut(&old_version_id) {
                old_version.active = false;
            }
            versions.insert(new_version_id, new_version.clone());

            // Update current version
            let mut current = self.current_version.write().unwrap();
            *current = new_version_id;
        }

        info!("Rotated to key version {}", new_version_id);

        Ok(new_version_id)
    }

    /// Get key version by ID
    pub fn get_version(&self, version_id: KeyVersionId) -> Option<KeyVersion> {
        self.key_versions.read().unwrap().get(&version_id).cloned()
    }

    /// Get all active key versions
    pub fn active_versions(&self) -> Vec<KeyVersion> {
        self.key_versions
            .read()
            .unwrap()
            .values()
            .filter(|v| v.active)
            .cloned()
            .collect()
    }

    /// Check if key rotation is needed
    pub fn needs_rotation(&self) -> bool {
        let current_id = self.current_version();
        let versions = self.key_versions.read().unwrap();

        if let Some(version) = versions.get(&current_id) {
            if let Some(expires_at) = version.expires_at {
                return now() >= expires_at;
            }
        }

        false
    }

    /// Clean up expired key versions
    pub fn cleanup_expired(&self) -> Result<usize> {
        let now = now();
        let mut versions = self.key_versions.write().unwrap();

        let expired: Vec<KeyVersionId> = versions
            .values()
            .filter(|v| !v.active)
            .filter(|v| {
                v.expires_at
                    .map(|exp| exp < now)
                    .unwrap_or(false)
            })
            .map(|v| v.id)
            .collect();

        for id in &expired {
            versions.remove(id);
        }

        let count = expired.len();
        if count > 0 {
            info!("Cleaned up {} expired key versions", count);
        }

        Ok(count)
    }
}

/// Key version manager for file-level key derivation
pub struct KeyVersionManager {
    /// Key manager for root keys
    key_manager: Arc<KeyManager>,
}

impl KeyVersionManager {
    /// Create a new key version manager
    pub fn new(key_manager: Arc<KeyManager>) -> Self {
        Self { key_manager }
    }

    /// Derive encryption key for a specific file
    pub fn derive_file_key(&self, file_path: &str, version_id: KeyVersionId) -> [u8; 32] {
        let context = format!("{}:{}", version_id, file_path);
        self.key_manager.derive_context_key(context.as_bytes())
    }

    /// Derive encryption key for WAL segment
    pub fn derive_wal_key(&self, wal_name: &str, segment_id: u64) -> [u8; 32] {
        let context = format!("wal:{}:{}", wal_name, segment_id);
        self.key_manager.derive_context_key(context.as_bytes())
    }

    /// Derive encryption key for SST file
    pub fn derive_sst_key(&self, sst_name: &str) -> [u8; 32] {
        let context = format!("sst:{}", sst_name);
        self.key_manager.derive_context_key(context.as_bytes())
    }

    /// Get current key version
    pub fn current_version(&self) -> KeyVersionId {
        self.key_manager.current_version()
    }

    /// Check if key rotation is needed
    pub fn needs_rotation(&self) -> bool {
        self.key_manager.needs_rotation()
    }

    /// Rotate to new key version
    pub fn rotate(&self) -> Result<KeyVersionId> {
        self.key_manager.rotate_key()
    }
}

impl Clone for KeyVersionManager {
    fn clone(&self) -> Self {
        Self {
            key_manager: Arc::clone(&self.key_manager),
        }
    }
}

/// Get current time as Unix epoch
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_manager_creation() {
        // Set test master key
        env::set_var("TEST_PROXIMADB_MASTER_KEY", "test-master-key-32-bytes-long-here!!");

        let km = KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap();
        assert_eq!(km.current_version(), 0);
    }

    #[test]
    fn test_context_key_derivation() {
        env::set_var("TEST_PROXIMADB_MASTER_KEY", "test-master-key-32-bytes-long-here!!");

        let km = KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap();
        let key1 = km.derive_context_key(b"context1");
        let key2 = km.derive_context_key(b"context1");
        let key3 = km.derive_context_key(b"context2");

        assert_eq!(key1, key2); // Same context = same key
        assert_ne!(key1, key3); // Different context = different key
        assert_eq!(key1.len(), 32);
    }

    #[test]
    fn test_key_rotation() {
        env::set_var("TEST_PROXIMADB_MASTER_KEY", "test-master-key-32-bytes-long-here!!");

        let km = KeyManager::from_env("TEST_PROXIMADB_MASTER_KEY").unwrap();
        assert_eq!(km.current_version(), 0);

        let new_version = km.rotate_key().unwrap();
        assert_eq!(new_version, 1);
        assert_eq!(km.current_version(), 1);

        let old_version = km.get_version(0).unwrap();
        assert!(!old_version.active);

        let new_version_obj = km.get_version(1).unwrap();
        assert!(new_version_obj.active);
    }
}
