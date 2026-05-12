//! Collection Service Security Extension
//!
//! Provides secure wrappers for collection operations including:
//! - Row-Level Security (RLS) filtering
//! - Field-Level Encryption
//! - Audit Logging
//!
//! This module acts as a security layer that can be integrated with
//! the existing CollectionService to provide transparent security enforcement.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::{debug, info};

use crate::audit::logger::AuditLogger;
use crate::core::search::FilterExpression;
use crate::core::service_types::{AuditLevel, CollectionSecurityConfig};
use crate::proto::proximadb_v1::VectorRecord;
use crate::security::encryption::{EncryptionConfig, FieldEncryption, KeyStore, KeyStoreConfig};
use crate::security::rls::{CollectionRLS, RLSConfig};
use crate::security::unified_rbac::UnifiedUserContext;
use crate::services::operations::{SecureVectorOperations, combine_filters};
use proximadb_security::{AuditConfig, AuditEvent, AuditEventType, AuditResource, AuditResult};

/// Security-enabled collection service extension
///
/// This struct provides a high-level interface for secure collection operations,
/// integrating RLS, encryption, and audit logging.
pub struct SecureCollectionService {
    /// RLS service for row-level security
    rls_service: Arc<CollectionRLS>,
    /// Key store for encryption keys
    key_store: Arc<KeyStore>,
    /// Field encryption service
    encryption_service: Option<Arc<FieldEncryption>>,
    /// Audit logger
    audit_logger: Arc<AuditLogger>,
    /// Per-collection security configurations
    collection_configs: parking_lot::RwLock<HashMap<String, CollectionSecurityConfig>>,
    /// Secure operations wrapper
    secure_ops: Arc<SecureVectorOperations>,
}

impl SecureCollectionService {
    /// Create a new secure collection service
    pub async fn new(
        rls_config: RLSConfig,
        key_store_config: KeyStoreConfig,
        encryption_config: Option<EncryptionConfig>,
        audit_config: AuditConfig,
    ) -> Result<Self> {
        let rls_service = Arc::new(CollectionRLS::new(rls_config));
        let key_store = Arc::new(KeyStore::new(key_store_config)?);

        let encryption_service = if let Some(enc_config) = encryption_config {
            Some(Arc::new(FieldEncryption::new(
                key_store.clone(),
                enc_config,
            )?))
        } else {
            None
        };

        let audit_logger = Arc::new(AuditLogger::new(audit_config).await?);

        let secure_ops = Arc::new(SecureVectorOperations::new(
            rls_service.clone(),
            encryption_service.clone(),
            audit_logger.clone(),
        ));

        Ok(Self {
            rls_service,
            key_store,
            encryption_service,
            audit_logger,
            collection_configs: parking_lot::RwLock::new(HashMap::new()),
            secure_ops,
        })
    }

    /// Register security configuration for a collection
    pub fn register_collection_security(
        &self,
        collection_name: &str,
        config: CollectionSecurityConfig,
    ) {
        info!(
            "Registering security config for collection '{}': RLS={}, encryption={}, audit={}",
            collection_name,
            config.rls_enabled,
            config.field_encryption_enabled,
            config.audit_enabled
        );

        let mut configs = self.collection_configs.write();
        configs.insert(collection_name.to_string(), config);
    }

    /// Get security configuration for a collection
    pub fn get_collection_security(
        &self,
        collection_name: &str,
    ) -> Option<CollectionSecurityConfig> {
        let configs = self.collection_configs.read();
        configs.get(collection_name).cloned()
    }

    /// Remove security configuration for a collection
    pub fn unregister_collection_security(&self, collection_name: &str) {
        let mut configs = self.collection_configs.write();
        configs.remove(collection_name);
    }

    /// Apply security for search operation
    ///
    /// Returns an optional FilterExpression that should be combined with
    /// the user's filter using AND logic.
    pub async fn secure_search(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        user_filter: Option<FilterExpression>,
    ) -> Result<Option<FilterExpression>> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        let rls_filter = self
            .secure_ops
            .apply_search_security(collection, user_context, &security_config)
            .await?;

        Ok(combine_filters(user_filter, rls_filter))
    }

    /// Apply security transformations before insert
    pub async fn secure_insert(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        records: &mut Vec<VectorRecord>,
    ) -> Result<()> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        // Check write permission
        self.secure_ops
            .check_write_permission(collection, user_context, &security_config)
            .await?;

        // Apply insert security (ownership metadata, encryption)
        self.secure_ops
            .apply_insert_security(collection, records, user_context, &security_config)
            .await?;

        Ok(())
    }

    /// Decrypt fields in search results
    pub async fn decrypt_results(
        &self,
        collection: &str,
        records: &mut [VectorRecord],
    ) -> Result<()> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        self.secure_ops
            .decrypt_search_results(records, &security_config)
            .await
    }

    /// Check if user can perform update on collection
    pub async fn check_update_permission(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
    ) -> Result<()> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        self.secure_ops
            .check_write_permission(collection, user_context, &security_config)
            .await
    }

    /// Check if user can perform delete on collection
    pub async fn check_delete_permission(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
    ) -> Result<()> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        self.secure_ops
            .check_delete_permission(collection, user_context, &security_config)
            .await
    }

    /// Log a collection access event
    pub async fn log_collection_access(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        operation: &str,
        success: bool,
        details: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<()> {
        let security_config = self.get_collection_security(collection).ok_or_else(|| {
            anyhow!(
                "Collection '{}' security configuration not found and no default available",
                collection
            )
        })?;

        if !security_config.audit_enabled {
            return Ok(());
        }

        let should_log = match (&security_config.audit_level, operation) {
            (AuditLevel::All, _) => true,
            (AuditLevel::Reads, "search" | "get" | "list") => true,
            (AuditLevel::Writes, "insert" | "update" | "delete") => true,
            (AuditLevel::None, _) => false,
            _ => false,
        };

        if !should_log {
            return Ok(());
        }

        let mut event = AuditEvent::new(
            AuditEventType::DataAccess,
            AuditResource::new("collection".to_string(), collection.to_string()),
            operation.to_string(),
            if success {
                AuditResult::Success
            } else {
                AuditResult::Failure {
                    error_code: "ACCESS_FAILED".to_string(),
                    error_message: "Operation failed".to_string(),
                }
            },
        )
        .with_user(user_context.user_id.clone());

        if let Some(ref tenant) = user_context.tenant_id {
            event = event.with_tenant(tenant.clone());
        }

        if let Some(details) = details {
            for (key, value) in details {
                event = event.with_detail(key, value);
            }
        }

        self.audit_logger.log_event(event).await
    }

    /// Rotate encryption keys for a collection
    ///
    /// Note: This rotates the key identified by the collection name.
    /// Ensure you have created a key for this collection first.
    pub async fn rotate_collection_keys(&self, collection: &str) -> Result<()> {
        let security_config = self.get_collection_security(collection);

        match security_config {
            Some(config) if config.field_encryption_enabled => {
                info!("Rotating encryption keys for collection '{}'", collection);
                // Use the collection name as key_id
                self.key_store.rotate_key(collection)?;
                Ok(())
            }
            Some(_) => {
                debug!("Encryption not enabled for collection '{}'", collection);
                Ok(())
            }
            None => {
                tracing::warn!("No security config found for collection '{}'", collection);
                Err(anyhow!(
                    "Collection '{}' not found in security registry",
                    collection
                ))
            }
        }
    }

    /// Get the RLS service for advanced configuration
    pub fn rls_service(&self) -> &Arc<CollectionRLS> {
        &self.rls_service
    }

    /// Get the key store for key management
    pub fn key_store(&self) -> &Arc<KeyStore> {
        &self.key_store
    }

    /// Get the audit logger
    pub fn audit_logger(&self) -> &Arc<AuditLogger> {
        &self.audit_logger
    }

    /// Check if encryption is enabled globally
    pub fn encryption_enabled(&self) -> bool {
        self.encryption_service.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::unified_rbac::UnifiedAuthMethod;
    use chrono::Utc;
    use std::collections::HashSet;

    fn create_test_user_context() -> UnifiedUserContext {
        UnifiedUserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant1".to_string()),
            roles: vec!["user".to_string()],
            effective_permissions: HashSet::new(),
            auth_method: UnifiedAuthMethod::Internal,
            session_id: "test_session".to_string(),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_secure_collection_service_creation() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        assert!(!service.encryption_enabled());
    }

    #[tokio::test]
    async fn test_register_collection_security() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        let config = CollectionSecurityConfig {
            rls_enabled: true,
            field_encryption_enabled: false,
            audit_enabled: true,
            audit_level: AuditLevel::All,
            ..Default::default()
        };

        service.register_collection_security("test_collection", config.clone());

        let retrieved = service.get_collection_security("test_collection").unwrap();
        assert!(retrieved.rls_enabled);
        assert!(!retrieved.field_encryption_enabled);
        assert!(retrieved.audit_enabled);
    }

    #[tokio::test]
    async fn test_unregister_collection_security() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        let config = CollectionSecurityConfig::default();
        service.register_collection_security("test_collection", config);

        assert!(service.get_collection_security("test_collection").is_some());

        service.unregister_collection_security("test_collection");

        assert!(service.get_collection_security("test_collection").is_none());
    }

    #[tokio::test]
    async fn test_secure_search_no_config() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        let user_context = create_test_user_context();

        // No security config registered - should return error
        let result = service
            .secure_search("unregistered", &user_context, None)
            .await;

        // Should fail with appropriate error message
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("security configuration not found"));
    }

    #[tokio::test]
    async fn test_secure_insert() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        // Register with RLS enabled
        service.register_collection_security(
            "test_collection",
            CollectionSecurityConfig {
                rls_enabled: true,
                ..Default::default()
            },
        );

        let user_context = create_test_user_context();
        let mut records = vec![VectorRecord {
            id: "rec1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
            ..Default::default()
        }];

        service
            .secure_insert("test_collection", &user_context, &mut records)
            .await
            .unwrap();

        // Should have added owner_id metadata
        assert!(records[0].metadata.contains_key("owner_id"));
    }

    #[tokio::test]
    async fn test_log_collection_access() {
        let service = SecureCollectionService::new(
            RLSConfig::default(),
            KeyStoreConfig::default(),
            None,
            AuditConfig::default(),
        )
        .await
        .unwrap();

        // Register with audit enabled
        service.register_collection_security(
            "test_collection",
            CollectionSecurityConfig {
                audit_enabled: true,
                audit_level: AuditLevel::All,
                ..Default::default()
            },
        );

        let user_context = create_test_user_context();

        // Should not error
        service
            .log_collection_access(
                "test_collection",
                &user_context,
                "search",
                true,
                Some(HashMap::from([(
                    "query_count".to_string(),
                    serde_json::json!(10),
                )])),
            )
            .await
            .unwrap();
    }
}
