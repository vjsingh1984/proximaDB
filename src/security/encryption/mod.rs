//! Field-Level Encryption for ProximaDB
//!
//! Provides encryption for sensitive metadata fields at rest while supporting
//! search operations through blind indexes. Integrates with the security
//! coordinator and storage engines.
//!
//! ## Features
//! - Deterministic encryption for searchable fields
//! - Randomized encryption for maximum security
//! - Blind indexes for encrypted field search
//! - Key rotation support
//! - Integration with UnifiedCachingFilesystem

pub mod field_encryption;
pub mod key_store;

pub use field_encryption::{
    EncryptedField, EncryptionConfig, EncryptionType, FieldEncryption, FieldEncryptionError,
    FieldEncryptionSettings,
};
pub use key_store::{KeyInfo, KeyStore, KeyStoreConfig, KeyStoreError};
