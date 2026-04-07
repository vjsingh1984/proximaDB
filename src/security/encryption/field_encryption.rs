//! Field-Level Encryption for ProximaDB
//!
//! Provides encryption for sensitive metadata fields at rest while supporting
//! search operations through blind indexes. Supports both deterministic
//! encryption (for searchable fields) and randomized encryption (for maximum security).

use super::key_store::{EncryptionKey, KeyStore, KeyStoreError};
use anyhow::Result;
use ring::aead::{AES_256_GCM, Aad, NONCE_LEN, Nonce};
use ring::digest::{self, SHA256};
use ring::hkdf::{HKDF_SHA256, Salt};
use ring::hmac::{self, HMAC_SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// Errors from field encryption operations
#[derive(Debug, Error)]
pub enum FieldEncryptionError {
    #[error("Key store error: {0}")]
    KeyStore(#[from] KeyStoreError),

    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("Field not encrypted: {0}")]
    FieldNotEncrypted(String),

    #[error("Invalid encrypted data format")]
    InvalidFormat,

    #[error("Key not found for field: {0}")]
    KeyNotFound(String),

    #[error("Unsupported value type for encryption")]
    UnsupportedValueType,
}

/// Type of encryption for a field
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionType {
    /// Deterministic encryption - same plaintext always produces same ciphertext
    /// Enables equality searches but reveals patterns
    Deterministic,

    /// Randomized encryption - same plaintext produces different ciphertext each time
    /// Maximum security but no search capability
    Randomized,
}

/// Configuration for field encryption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// Enable field encryption
    pub enabled: bool,

    /// Default encryption type for new fields
    pub default_type: EncryptionType,

    /// Field-specific encryption settings
    pub field_settings: HashMap<String, FieldEncryptionSettings>,

    /// Generate blind indexes for searchable encrypted fields
    pub enable_blind_indexes: bool,

    /// Blind index key derivation salt (should be unique per deployment)
    pub blind_index_salt: Option<String>,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_type: EncryptionType::Randomized,
            field_settings: HashMap::new(),
            enable_blind_indexes: true,
            blind_index_salt: None,
        }
    }
}

/// Settings for a specific field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldEncryptionSettings {
    /// Encryption type for this field
    pub encryption_type: EncryptionType,

    /// Key ID to use for this field
    pub key_id: String,

    /// Generate blind index for this field
    pub blind_index: bool,

    /// Truncate blind index to N bytes (for space efficiency, trades off uniqueness)
    pub blind_index_bytes: Option<usize>,
}

/// An encrypted field value with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedField {
    /// The encrypted ciphertext (base64 encoded)
    pub ciphertext: String,

    /// Key ID used for encryption
    pub key_id: String,

    /// Key version used for encryption
    pub key_version: u32,

    /// Encryption type used
    pub encryption_type: EncryptionType,

    /// Nonce used (base64 encoded, for randomized encryption)
    pub nonce: Option<String>,

    /// Blind index for searchable encryption (hex encoded)
    pub blind_index: Option<String>,
}

/// Field-level encryption service
pub struct FieldEncryption {
    /// Key store for encryption keys
    key_store: Arc<KeyStore>,

    /// Configuration
    config: EncryptionConfig,

    /// Random number generator
    rng: SystemRandom,

    /// Blind index HMAC key (derived from salt)
    blind_index_key: Option<hmac::Key>,
}

impl FieldEncryption {
    /// Create a new field encryption service
    pub fn new(
        key_store: Arc<KeyStore>,
        config: EncryptionConfig,
    ) -> Result<Self, FieldEncryptionError> {
        let blind_index_key = if config.enable_blind_indexes {
            let salt = config
                .blind_index_salt
                .as_deref()
                .unwrap_or("proximadb-blind-index-default-salt");

            // Derive a key from the salt using HKDF
            let hkdf_salt = Salt::new(HKDF_SHA256, salt.as_bytes());
            let prk = hkdf_salt.extract(b"blind-index-key-material");

            let mut key_bytes = [0u8; 32];
            let info: &[&[u8]] = &[b"blind-index"];
            let hkdf = prk.expand(info, HKDF_SHA256).map_err(|_| {
                FieldEncryptionError::EncryptionFailed("HKDF expansion failed".into())
            })?;
            hkdf.fill(&mut key_bytes)
                .map_err(|_| FieldEncryptionError::EncryptionFailed("HKDF fill failed".into()))?;

            Some(hmac::Key::new(HMAC_SHA256, &key_bytes))
        } else {
            None
        };

        Ok(Self {
            key_store,
            config,
            rng: SystemRandom::new(),
            blind_index_key,
        })
    }

    /// Encrypt a field value
    pub fn encrypt_field(
        &self,
        field_name: &str,
        value: &serde_json::Value,
    ) -> Result<EncryptedField, FieldEncryptionError> {
        // Get field settings or use defaults
        let settings = self.config.field_settings.get(field_name);
        let encryption_type =
            settings.map_or_else(|| self.config.default_type, |s| s.encryption_type);

        let key_id = settings.map_or("default", |s| s.key_id.as_str());

        // Serialize value to bytes
        let plaintext = serde_json::to_vec(value)
            .map_err(|e| FieldEncryptionError::EncryptionFailed(e.to_string()))?;

        // Get encryption key
        let key = self.key_store.get_key(key_id)?;

        // Encrypt based on type
        let (ciphertext, nonce) = match encryption_type {
            EncryptionType::Deterministic => {
                self.encrypt_deterministic(&plaintext, &key, field_name)?
            }
            EncryptionType::Randomized => self.encrypt_randomized(&plaintext, &key)?,
        };

        // Generate blind index if enabled
        let blind_index =
            if settings.map_or_else(|| self.config.enable_blind_indexes, |s| s.blind_index) {
                let truncate = settings.and_then(|s| s.blind_index_bytes);
                self.generate_blind_index(value, truncate)?
            } else {
                None
            };

        Ok(EncryptedField {
            ciphertext: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &ciphertext,
            ),
            key_id: key_id.to_string(),
            key_version: key.version,
            encryption_type,
            nonce: nonce
                .map(|n| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &n)),
            blind_index,
        })
    }

    /// Decrypt an encrypted field
    pub fn decrypt_field(
        &self,
        encrypted: &EncryptedField,
    ) -> Result<serde_json::Value, FieldEncryptionError> {
        // Get the key version used for encryption
        let key = self
            .key_store
            .get_key_version(&encrypted.key_id, encrypted.key_version)?;

        // Decode ciphertext
        let ciphertext = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &encrypted.ciphertext,
        )
        .map_err(|_| FieldEncryptionError::InvalidFormat)?;

        // Decrypt based on type
        let plaintext = match encrypted.encryption_type {
            EncryptionType::Deterministic => self.decrypt_deterministic(&ciphertext, &key)?,
            EncryptionType::Randomized => {
                let nonce = encrypted
                    .nonce
                    .as_ref()
                    .ok_or(FieldEncryptionError::InvalidFormat)?;
                let nonce_bytes =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, nonce)
                        .map_err(|_| FieldEncryptionError::InvalidFormat)?;
                self.decrypt_randomized(&ciphertext, &key, &nonce_bytes)?
            }
        };

        // Deserialize value
        serde_json::from_slice(&plaintext)
            .map_err(|e| FieldEncryptionError::DecryptionFailed(e.to_string()))
    }

    /// Encrypt using deterministic encryption (same input = same output)
    fn encrypt_deterministic(
        &self,
        plaintext: &[u8],
        key: &EncryptionKey,
        field_name: &str,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), FieldEncryptionError> {
        // For deterministic encryption, derive nonce from content hash
        let hash = digest::digest(&SHA256, plaintext);
        let field_hash = digest::digest(&SHA256, field_name.as_bytes());

        // XOR content hash with field hash to create nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        for (i, byte) in nonce_bytes.iter_mut().enumerate() {
            *byte = hash.as_ref()[i] ^ field_hash.as_ref()[i];
        }

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aead_key = key.to_aead_key()?;

        // Encrypt
        let mut in_out = plaintext.to_vec();
        in_out.resize(plaintext.len() + AES_256_GCM.tag_len(), 0);

        aead_key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut in_out[..plaintext.len()])
            .map(|tag| {
                in_out[plaintext.len()..].copy_from_slice(tag.as_ref());
            })
            .map_err(|_| FieldEncryptionError::EncryptionFailed("AEAD seal failed".into()))?;

        // Prepend nonce to ciphertext (for verification)
        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&in_out);

        Ok((result, None)) // No separate nonce for deterministic
    }

    /// Encrypt using randomized encryption (different output each time)
    fn encrypt_randomized(
        &self,
        plaintext: &[u8],
        key: &EncryptionKey,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>), FieldEncryptionError> {
        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce_bytes).map_err(|_| {
            FieldEncryptionError::EncryptionFailed("Failed to generate nonce".into())
        })?;

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aead_key = key.to_aead_key()?;

        // Encrypt
        let mut in_out = plaintext.to_vec();
        in_out.resize(plaintext.len() + AES_256_GCM.tag_len(), 0);

        aead_key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut in_out[..plaintext.len()])
            .map(|tag| {
                in_out[plaintext.len()..].copy_from_slice(tag.as_ref());
            })
            .map_err(|_| FieldEncryptionError::EncryptionFailed("AEAD seal failed".into()))?;

        Ok((in_out, Some(nonce_bytes.to_vec())))
    }

    /// Decrypt deterministically encrypted data
    fn decrypt_deterministic(
        &self,
        ciphertext: &[u8],
        key: &EncryptionKey,
    ) -> Result<Vec<u8>, FieldEncryptionError> {
        if ciphertext.len() < NONCE_LEN + AES_256_GCM.tag_len() {
            return Err(FieldEncryptionError::InvalidFormat);
        }

        // Extract nonce and encrypted data
        let nonce_bytes: [u8; NONCE_LEN] = ciphertext[..NONCE_LEN]
            .try_into()
            .map_err(|_| FieldEncryptionError::InvalidFormat)?;
        let encrypted = &ciphertext[NONCE_LEN..];

        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let aead_key = key.to_aead_key()?;

        // Decrypt
        let mut in_out = encrypted.to_vec();
        let plaintext = aead_key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| FieldEncryptionError::DecryptionFailed("AEAD open failed".into()))?;

        Ok(plaintext.to_vec())
    }

    /// Decrypt randomized encrypted data
    fn decrypt_randomized(
        &self,
        ciphertext: &[u8],
        key: &EncryptionKey,
        nonce_bytes: &[u8],
    ) -> Result<Vec<u8>, FieldEncryptionError> {
        if nonce_bytes.len() != NONCE_LEN {
            return Err(FieldEncryptionError::InvalidFormat);
        }
        if ciphertext.len() < AES_256_GCM.tag_len() {
            return Err(FieldEncryptionError::InvalidFormat);
        }

        let nonce_arr: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| FieldEncryptionError::InvalidFormat)?;
        let nonce = Nonce::assume_unique_for_key(nonce_arr);
        let aead_key = key.to_aead_key()?;

        // Decrypt
        let mut in_out = ciphertext.to_vec();
        let plaintext = aead_key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| FieldEncryptionError::DecryptionFailed("AEAD open failed".into()))?;

        Ok(plaintext.to_vec())
    }

    /// Generate a blind index for a value (for searchable encryption)
    fn generate_blind_index(
        &self,
        value: &serde_json::Value,
        truncate_bytes: Option<usize>,
    ) -> Result<Option<String>, FieldEncryptionError> {
        let key = self.blind_index_key.as_ref().ok_or_else(|| {
            FieldEncryptionError::EncryptionFailed("Blind index key not configured".into())
        })?;

        // Serialize value consistently
        let value_bytes = serde_json::to_vec(value)
            .map_err(|e| FieldEncryptionError::EncryptionFailed(e.to_string()))?;

        // HMAC the value
        let tag = hmac::sign(key, &value_bytes);
        let hash = tag.as_ref();

        // Optionally truncate for space efficiency
        let index_bytes = match truncate_bytes {
            Some(n) if n < hash.len() => &hash[..n],
            _ => hash,
        };

        Ok(Some(hex::encode(index_bytes)))
    }

    /// Generate a blind index for a search query (to match against stored indexes)
    pub fn generate_search_index(
        &self,
        value: &serde_json::Value,
        truncate_bytes: Option<usize>,
    ) -> Result<String, FieldEncryptionError> {
        self.generate_blind_index(value, truncate_bytes)?
            .ok_or_else(|| {
                FieldEncryptionError::EncryptionFailed("Blind indexes not enabled".into())
            })
    }

    /// Encrypt a record's metadata fields based on configuration
    pub fn encrypt_record_metadata(
        &self,
        metadata: &mut HashMap<String, serde_json::Value>,
    ) -> Result<HashMap<String, EncryptedField>, FieldEncryptionError> {
        let mut encrypted_fields = HashMap::new();

        for field_name in self.config.field_settings.keys() {
            if let Some(value) = metadata.remove(field_name) {
                let encrypted = self.encrypt_field(field_name, &value)?;
                encrypted_fields.insert(field_name.clone(), encrypted);
            }
        }

        Ok(encrypted_fields)
    }

    /// Decrypt a record's encrypted metadata fields
    pub fn decrypt_record_metadata(
        &self,
        encrypted_fields: &HashMap<String, EncryptedField>,
    ) -> Result<HashMap<String, serde_json::Value>, FieldEncryptionError> {
        let mut decrypted = HashMap::new();

        for (field_name, encrypted) in encrypted_fields {
            let value = self.decrypt_field(encrypted)?;
            decrypted.insert(field_name.clone(), value);
        }

        Ok(decrypted)
    }

    /// Re-encrypt a field with a new key version (after key rotation)
    pub fn reencrypt_field(
        &self,
        field_name: &str,
        encrypted: &EncryptedField,
    ) -> Result<EncryptedField, FieldEncryptionError> {
        // Decrypt with old key
        let value = self.decrypt_field(encrypted)?;

        // Re-encrypt with current key
        self.encrypt_field(field_name, &value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::encryption::key_store::KeyStoreConfig;

    fn setup() -> (Arc<KeyStore>, FieldEncryption) {
        let key_store = Arc::new(
            KeyStore::new(KeyStoreConfig::default())
                .expect("KeyStore initialization should succeed in tests"),
        );
        key_store
            .create_key("default", "Default encryption key")
            .expect("Key creation should succeed in tests");
        key_store
            .create_key("ssn-key", "SSN encryption key")
            .expect("Key creation should succeed in tests");

        let config = EncryptionConfig {
            enabled: true,
            default_type: EncryptionType::Randomized,
            field_settings: HashMap::from([(
                "ssn".to_string(),
                FieldEncryptionSettings {
                    encryption_type: EncryptionType::Deterministic,
                    key_id: "ssn-key".to_string(),
                    blind_index: true,
                    blind_index_bytes: Some(8),
                },
            )]),
            enable_blind_indexes: true,
            blind_index_salt: Some("test-salt".to_string()),
        };

        let encryption = FieldEncryption::new(Arc::clone(&key_store), config)
            .expect("FieldEncryption initialization should succeed in tests");
        (key_store, encryption)
    }

    #[test]
    fn test_encrypt_decrypt_randomized() {
        let (_, encryption) = setup();

        let value = serde_json::json!("secret-value");
        let encrypted = encryption
            .encrypt_field("random_field", &value)
            .expect("Encryption should succeed");

        assert_eq!(encrypted.encryption_type, EncryptionType::Randomized);
        assert!(encrypted.nonce.is_some());

        let decrypted = encryption
            .decrypt_field(&encrypted)
            .expect("Decryption should succeed");
        assert_eq!(decrypted, value);
    }

    #[test]
    fn test_encrypt_decrypt_deterministic() {
        let (_, encryption) = setup();

        let value = serde_json::json!("123-45-6789");
        let encrypted1 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");
        let encrypted2 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");

        assert_eq!(encrypted1.encryption_type, EncryptionType::Deterministic);
        assert!(encrypted1.nonce.is_none());

        // Deterministic encryption should produce same ciphertext
        assert_eq!(encrypted1.ciphertext, encrypted2.ciphertext);

        let decrypted = encryption
            .decrypt_field(&encrypted1)
            .expect("Decryption should succeed");
        assert_eq!(decrypted, value);
    }

    #[test]
    fn test_randomized_different_each_time() {
        let (_, encryption) = setup();

        let value = serde_json::json!("secret");
        let encrypted1 = encryption
            .encrypt_field("random_field", &value)
            .expect("Encryption should succeed");
        let encrypted2 = encryption
            .encrypt_field("random_field", &value)
            .expect("Encryption should succeed");

        // Randomized encryption should produce different ciphertext
        assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
        assert_ne!(encrypted1.nonce, encrypted2.nonce);
    }

    #[test]
    fn test_blind_index_generation() {
        let (_, encryption) = setup();

        let value = serde_json::json!("123-45-6789");
        let encrypted = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");

        assert!(encrypted.blind_index.is_some());
        let index = encrypted
            .blind_index
            .as_ref()
            .expect("Blind index should be present");

        // Truncated to 8 bytes = 16 hex chars
        assert_eq!(index.len(), 16);

        // Same value should produce same blind index
        let encrypted2 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");
        assert_eq!(encrypted.blind_index, encrypted2.blind_index);
    }

    #[test]
    fn test_search_index() {
        let (_, encryption) = setup();

        let value = serde_json::json!("123-45-6789");

        // Encrypt the field
        let encrypted = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");

        // Generate search index
        let search_index = encryption
            .generate_search_index(&value, Some(8))
            .expect("Search index generation should succeed");

        // Should match the stored blind index
        assert_eq!(encrypted.blind_index, Some(search_index));
    }

    #[test]
    fn test_key_version_tracking() {
        let (key_store, encryption) = setup();

        let value = serde_json::json!("test");
        let encrypted1 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");

        assert_eq!(encrypted1.key_version, 1);

        // Rotate key
        key_store
            .rotate_key("ssn-key")
            .expect("Key rotation should succeed");

        let encrypted2 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");
        assert_eq!(encrypted2.key_version, 2);

        // Both should still decrypt correctly
        let decrypted1 = encryption
            .decrypt_field(&encrypted1)
            .expect("Decryption should succeed");
        let decrypted2 = encryption
            .decrypt_field(&encrypted2)
            .expect("Decryption should succeed");

        assert_eq!(decrypted1, value);
        assert_eq!(decrypted2, value);
    }

    #[test]
    fn test_reencrypt_after_rotation() {
        let (key_store, encryption) = setup();

        let value = serde_json::json!("sensitive-data");
        let encrypted_v1 = encryption
            .encrypt_field("ssn", &value)
            .expect("Encryption should succeed");
        assert_eq!(encrypted_v1.key_version, 1);

        // Rotate key
        key_store
            .rotate_key("ssn-key")
            .expect("Key rotation should succeed");

        // Re-encrypt with new key
        let encrypted_v2 = encryption
            .reencrypt_field("ssn", &encrypted_v1)
            .expect("Re-encryption should succeed");
        assert_eq!(encrypted_v2.key_version, 2);

        // Verify decryption still works
        let decrypted = encryption
            .decrypt_field(&encrypted_v2)
            .expect("Decryption should succeed");
        assert_eq!(decrypted, value);
    }

    #[test]
    fn test_encrypt_various_types() {
        let (_, encryption) = setup();

        // String
        let str_val = serde_json::json!("hello");
        let encrypted = encryption
            .encrypt_field("field", &str_val)
            .expect("Encryption should succeed");
        assert_eq!(
            encryption
                .decrypt_field(&encrypted)
                .expect("Decryption should succeed"),
            str_val
        );

        // Number
        let num_val = serde_json::json!(42);
        let encrypted = encryption
            .encrypt_field("field", &num_val)
            .expect("Encryption should succeed");
        assert_eq!(
            encryption
                .decrypt_field(&encrypted)
                .expect("Decryption should succeed"),
            num_val
        );

        // Boolean
        let bool_val = serde_json::json!(true);
        let encrypted = encryption
            .encrypt_field("field", &bool_val)
            .expect("Encryption should succeed");
        assert_eq!(
            encryption
                .decrypt_field(&encrypted)
                .expect("Decryption should succeed"),
            bool_val
        );

        // Object
        let obj_val = serde_json::json!({"nested": "value", "count": 123});
        let encrypted = encryption
            .encrypt_field("field", &obj_val)
            .expect("Encryption should succeed");
        assert_eq!(
            encryption
                .decrypt_field(&encrypted)
                .expect("Decryption should succeed"),
            obj_val
        );

        // Array
        let arr_val = serde_json::json!([1, 2, 3, "four"]);
        let encrypted = encryption
            .encrypt_field("field", &arr_val)
            .expect("Encryption should succeed");
        assert_eq!(
            encryption
                .decrypt_field(&encrypted)
                .expect("Decryption should succeed"),
            arr_val
        );
    }

    #[test]
    fn test_encrypt_record_metadata() {
        let (_, encryption) = setup();

        let mut metadata: HashMap<String, serde_json::Value> = HashMap::from([
            ("ssn".to_string(), serde_json::json!("123-45-6789")),
            ("name".to_string(), serde_json::json!("John Doe")),
            ("age".to_string(), serde_json::json!(30)),
        ]);

        let encrypted = encryption
            .encrypt_record_metadata(&mut metadata)
            .expect("Encryption should succeed");

        // Only ssn should be encrypted (it's in field_settings)
        assert!(encrypted.contains_key("ssn"));
        assert!(!encrypted.contains_key("name"));
        assert!(!encrypted.contains_key("age"));

        // ssn should be removed from metadata
        assert!(!metadata.contains_key("ssn"));
        assert!(metadata.contains_key("name"));
        assert!(metadata.contains_key("age"));
    }

    #[test]
    fn test_decrypt_record_metadata() {
        let (_, encryption) = setup();

        let mut metadata: HashMap<String, serde_json::Value> =
            HashMap::from([("ssn".to_string(), serde_json::json!("123-45-6789"))]);

        let encrypted = encryption
            .encrypt_record_metadata(&mut metadata)
            .expect("Encryption should succeed");
        let decrypted = encryption
            .decrypt_record_metadata(&encrypted)
            .expect("Decryption should succeed");

        assert_eq!(
            decrypted
                .get("ssn")
                .expect("SSN should be present in decrypted metadata"),
            &serde_json::json!("123-45-6789")
        );
    }
}
