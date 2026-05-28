//! Secure Vector Operations - RLS and Encryption Integration
//!
//! This module wraps vector operations with security features:
//! - Row-Level Security (RLS) filtering
//! - Field-Level Encryption/Decryption
//! - Audit logging
//!
//! All security operations are applied transparently at the service layer,
//! ensuring consistent enforcement regardless of the API entry point.

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::audit::logger::AuditLogger;
use crate::core::search::FilterExpression;
use crate::core::service_types::{AuditLevel, CollectionSecurityConfig};
use crate::security::encryption::{EncryptedField, FieldEncryption};
use crate::security::rbac_service::UnifiedUserContext;
use crate::security::rls::{CollectionRLS, Operation as RLSOperation, RLSFilterResult};
use proximadb_data_model::ProximaValue;
use proximadb_records::{ProximaRecord, ProximaTreeNode};
use proximadb_security::{AuditEvent, AuditEventType, AuditResource, AuditResult};

/// Helper to extract string from a ProximaTreeNode value
fn proxima_node_to_string(node: &ProximaTreeNode) -> Option<String> {
    match node {
        ProximaTreeNode::Value(ProximaValue::String(s)) => Some(s.clone()),
        ProximaTreeNode::Value(ProximaValue::Float64(n)) => Some(n.to_string()),
        ProximaTreeNode::Value(ProximaValue::Int64(i)) => Some(i.to_string()),
        ProximaTreeNode::Value(ProximaValue::Boolean(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// Secure operations wrapper for vector operations
pub struct SecureVectorOperations {
    /// RLS service for row-level security
    rls_service: Arc<CollectionRLS>,
    /// Field encryption service (optional, depends on configuration)
    encryption_service: Option<Arc<FieldEncryption>>,
    /// Audit logger for security events
    audit_logger: Arc<AuditLogger>,
}

impl SecureVectorOperations {
    /// Create a new secure operations wrapper
    pub fn new(
        rls_service: Arc<CollectionRLS>,
        encryption_service: Option<Arc<FieldEncryption>>,
        audit_logger: Arc<AuditLogger>,
    ) -> Self {
        Self {
            rls_service,
            encryption_service,
            audit_logger,
        }
    }

    /// Apply security filters for search operations
    ///
    /// Returns an optional FilterExpression that should be combined with the
    /// existing search filters to enforce RLS policies.
    pub async fn apply_search_security(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        security_config: &CollectionSecurityConfig,
    ) -> Result<Option<FilterExpression>> {
        if !security_config.rls_enabled {
            debug!("RLS disabled for collection {}", collection);
            return Ok(None);
        }

        let rls_result = self
            .rls_service
            .apply_security_filter(collection, &RLSOperation::Read, user_context)
            .await?;

        // Log the RLS application
        if security_config.audit_enabled
            && (security_config.audit_level == AuditLevel::Reads
                || security_config.audit_level == AuditLevel::All)
        {
            self.log_rls_application(collection, user_context, &rls_result)
                .await?;
        }

        if rls_result.access_denied {
            return Err(anyhow!(
                "Access denied: {}",
                rls_result
                    .denial_reason
                    .as_deref()
                    .unwrap_or("RLS policy violation")
            ));
        }

        Ok(rls_result.filter.clone())
    }

    /// Apply security transformations before insert
    ///
    /// This includes:
    /// - Adding ownership metadata for RLS
    /// - Encrypting configured fields
    #[expect(clippy::ptr_arg)] // Accepting &mut Vec for API compatibility
    pub async fn apply_insert_security(
        &self,
        collection: &str,
        records: &mut Vec<ProximaRecord>,
        user_context: &UnifiedUserContext,
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        // Add ownership metadata for RLS
        if security_config.rls_enabled {
            for record in records.iter_mut() {
                record.props.insert(
                    "owner_id".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(user_context.user_id.clone())),
                );
                record.props.insert(
                    "created_by".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(user_context.user_id.clone())),
                );
                if let Some(ref tenant) = user_context.tenant_id {
                    record.tenant_id = tenant.clone();
                }
            }
        }

        // Apply field encryption if enabled
        if security_config.field_encryption_enabled
            && let Some(ref encryption) = self.encryption_service
        {
            for record in records.iter_mut() {
                self.encrypt_record_fields(record, encryption, security_config)
                    .await?;
            }
        }

        // Log audit event
        if security_config.audit_enabled
            && (security_config.audit_level == AuditLevel::Writes
                || security_config.audit_level == AuditLevel::All)
        {
            self.log_insert_event(collection, user_context, records.len())
                .await?;
        }

        Ok(())
    }

    /// Decrypt fields in search results
    pub async fn decrypt_search_results(
        &self,
        records: &mut [ProximaRecord],
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        if !security_config.field_encryption_enabled {
            return Ok(());
        }

        let encryption = match &self.encryption_service {
            Some(e) => e,
            None => return Ok(()),
        };

        for record in records.iter_mut() {
            self.decrypt_record_fields(record, encryption, security_config)
                .await?;
        }

        Ok(())
    }

    /// Check if user has write permission for the operation
    pub async fn check_write_permission(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        if !security_config.rls_enabled {
            return Ok(());
        }

        let rls_result = self
            .rls_service
            .apply_security_filter(collection, &RLSOperation::Write, user_context)
            .await?;

        if rls_result.access_denied {
            return Err(anyhow!(
                "Write access denied: {}",
                rls_result
                    .denial_reason
                    .as_deref()
                    .unwrap_or("RLS policy violation")
            ));
        }

        Ok(())
    }

    /// Check if user has delete permission for the operation
    pub async fn check_delete_permission(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        if !security_config.rls_enabled {
            return Ok(());
        }

        let rls_result = self
            .rls_service
            .apply_security_filter(collection, &RLSOperation::Delete, user_context)
            .await?;

        if rls_result.access_denied {
            return Err(anyhow!(
                "Delete access denied: {}",
                rls_result
                    .denial_reason
                    .as_deref()
                    .unwrap_or("RLS policy violation")
            ));
        }

        Ok(())
    }

    /// Encrypt configured fields in a record
    async fn encrypt_record_fields(
        &self,
        record: &mut ProximaRecord,
        encryption: &FieldEncryption,
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        let fields_to_encrypt: Vec<String> = security_config
            .encryption_config
            .field_settings
            .keys()
            .cloned()
            .collect();

        let mut encrypted_fields: HashMap<String, EncryptedField> = HashMap::new();

        for field_name in &fields_to_encrypt {
            if let Some(node) = record.props.get(field_name)
                && let Some(value_str) = proxima_node_to_string(node)
            {
                let value: serde_json::Value = serde_json::from_str(&value_str)
                    .unwrap_or_else(|_| serde_json::json!(value_str));

                match encryption.encrypt_field(field_name, &value) {
                    Ok(encrypted) => {
                        encrypted_fields.insert(field_name.clone(), encrypted);
                    }
                    Err(e) => {
                        warn!("Failed to encrypt field {}: {}", field_name, e);
                    }
                }
            }
        }

        for (field_name, encrypted) in encrypted_fields {
            let encrypted_json = serde_json::to_string(&encrypted)?;
            record.props.insert(
                field_name,
                ProximaTreeNode::Value(ProximaValue::String(encrypted_json)),
            );
        }

        record.props.insert(
            "__encrypted".to_string(),
            ProximaTreeNode::Value(ProximaValue::Boolean(true)),
        );

        Ok(())
    }

    /// Decrypt configured fields in a record
    async fn decrypt_record_fields(
        &self,
        record: &mut ProximaRecord,
        encryption: &FieldEncryption,
        security_config: &CollectionSecurityConfig,
    ) -> Result<()> {
        let is_encrypted = matches!(
            record.props.get("__encrypted"),
            Some(ProximaTreeNode::Value(ProximaValue::Boolean(true)))
        );

        if !is_encrypted {
            return Ok(());
        }

        let fields_to_decrypt: Vec<String> = security_config
            .encryption_config
            .field_settings
            .keys()
            .cloned()
            .collect();

        for field_name in &fields_to_decrypt {
            if let Some(node) = record.props.get(field_name)
                && let Some(encrypted_json) = proxima_node_to_string(node)
            {
                let encrypted: EncryptedField = match serde_json::from_str(&encrypted_json) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                match encryption.decrypt_field(&encrypted) {
                    Ok(decrypted) => {
                        let decrypted_str = serde_json::to_string(&decrypted)?;
                        record.props.insert(
                            field_name.clone(),
                            ProximaTreeNode::Value(ProximaValue::String(decrypted_str)),
                        );
                    }
                    Err(e) => {
                        warn!("Failed to decrypt field {}: {}", field_name, e);
                    }
                }
            }
        }

        record.props.remove("__encrypted");

        Ok(())
    }

    /// Log RLS filter application
    async fn log_rls_application(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        rls_result: &Arc<RLSFilterResult>,
    ) -> Result<()> {
        let event = AuditEvent::new(
            AuditEventType::DataAccess,
            AuditResource::new("collection".to_string(), collection.to_string()),
            "search".to_string(),
            if rls_result.access_denied {
                AuditResult::Failure {
                    error_code: "RLS_DENIED".to_string(),
                    error_message: rls_result.denial_reason.clone().unwrap_or_default(),
                }
            } else {
                AuditResult::Success
            },
        )
        .with_user(user_context.user_id.clone())
        .with_detail(
            "rls_policies_applied".to_string(),
            serde_json::json!(rls_result.applied_policies),
        )
        .with_detail(
            "filters_applied".to_string(),
            serde_json::json!(rls_result.filters_applied),
        );

        let event = if let Some(ref tenant) = user_context.tenant_id {
            event.with_tenant(tenant.clone())
        } else {
            event
        };

        let event = if rls_result.access_denied {
            event.with_risk_score(0.5)
        } else {
            event
        };

        self.audit_logger.log_event(event).await?;
        Ok(())
    }

    /// Log insert operation
    async fn log_insert_event(
        &self,
        collection: &str,
        user_context: &UnifiedUserContext,
        record_count: usize,
    ) -> Result<()> {
        let event = AuditEvent::new(
            AuditEventType::DataModification,
            AuditResource::new("collection".to_string(), collection.to_string()),
            "insert".to_string(),
            AuditResult::Success,
        )
        .with_user(user_context.user_id.clone())
        .with_detail("record_count".to_string(), serde_json::json!(record_count));

        let event = if let Some(ref tenant) = user_context.tenant_id {
            event.with_tenant(tenant.clone())
        } else {
            event
        };

        self.audit_logger.log_event(event).await?;
        Ok(())
    }
}

/// Combine user's filter with RLS filter
pub fn combine_filters(
    user_filter: Option<FilterExpression>,
    rls_filter: Option<FilterExpression>,
) -> Option<FilterExpression> {
    match (user_filter, rls_filter) {
        (None, None) => None,
        (Some(f), None) | (None, Some(f)) => Some(f),
        (Some(user), Some(rls)) => Some(FilterExpression::And(vec![user, rls])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;
    use crate::security::rbac_service::UnifiedAuthMethod;
    use crate::security::rls::RLSConfig;
    use chrono::Utc;
    use proximadb_security::AuditConfig;
    use std::collections::HashSet;

    fn create_test_user_context() -> UnifiedUserContext {
        UnifiedUserContext {
            user_id: "test_user".to_string(),
            tenant_id: None,
            roles: vec!["user".to_string()],
            effective_permissions: HashSet::new(),
            auth_method: UnifiedAuthMethod::Internal,
            session_id: "test_session".to_string(),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Create a simple filter that always matches (for testing)
    fn always_true_filter() -> FilterExpression {
        // Use an empty And() which is logically true
        FilterExpression::And(vec![])
    }

    async fn create_test_service() -> SecureVectorOperations {
        let rls_service = Arc::new(CollectionRLS::new(RLSConfig::default()));
        let audit_logger = Arc::new(AuditLogger::new(AuditConfig::default()).await.unwrap());

        SecureVectorOperations::new(rls_service, None, audit_logger)
    }

    #[tokio::test]
    async fn test_secure_ops_creation() {
        let service = create_test_service().await;
        assert!(service.encryption_service.is_none());
    }

    #[tokio::test]
    async fn test_rls_disabled_returns_no_filter() {
        let service = create_test_service().await;
        let user_context = create_test_user_context();
        let security_config = CollectionSecurityConfig::default();

        let result = service
            .apply_search_security("test_collection", &user_context, &security_config)
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_insert_security_adds_ownership() {
        let service = create_test_service().await;
        let mut user_context = create_test_user_context();
        user_context.user_id = "user123".to_string();

        let mut security_config = CollectionSecurityConfig::default();
        security_config.rls_enabled = true;

        let mut records = vec![ProximaRecord {
            oid: "rec1".to_string(),
            ..Default::default()
        }];

        service
            .apply_insert_security(
                "test_collection",
                &mut records,
                &user_context,
                &security_config,
            )
            .await
            .unwrap();

        assert!(records[0].props.contains_key("owner_id"));
        assert!(records[0].props.contains_key("created_by"));
    }

    #[test]
    fn test_combine_filters_both_none() {
        let result = combine_filters(None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_combine_filters_one_present() {
        // Create a simple filter expression - use an always-true equivalent
        let user_filter = always_true_filter();

        let result = combine_filters(Some(user_filter.clone()), None);
        assert!(result.is_some());

        let result = combine_filters(None, Some(always_true_filter()));
        assert!(result.is_some());
    }

    #[test]
    fn test_combine_filters_both_present() {
        // Create simple filter expressions
        let user_filter = FilterExpression::Comparison {
            field: "status".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("active"),
        };
        let rls_filter = FilterExpression::Comparison {
            field: "owner".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("user123"),
        };

        let result = combine_filters(Some(user_filter), Some(rls_filter));
        assert!(matches!(result, Some(FilterExpression::And(_))));
    }
}
