//! Row-Level Security service implementation
//!
//! Converts security predicates to metadata filters and applies them to search requests.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::policy::{FilterOperator, Operation, RLSPolicy, SecurityPredicate, ValueSource};
use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::security::unified_rbac::UnifiedUserContext;

/// Configuration for Row-Level Security
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLSConfig {
    /// Whether RLS is enabled globally
    pub enabled: bool,
    /// Cache policy evaluation results (TTL in seconds)
    pub cache_ttl_seconds: u64,
    /// Maximum number of cached policy results
    pub max_cache_entries: usize,
    /// Log policy evaluations for debugging
    pub debug_logging: bool,
}

impl Default for RLSConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_ttl_seconds: 300,
            max_cache_entries: 10000,
            debug_logging: false,
        }
    }
}

/// Result of applying RLS filters
#[derive(Debug)]
pub struct RLSFilterResult {
    /// Whether any filters were applied
    pub filters_applied: bool,
    /// The combined filter expression (if any)
    pub filter: Option<FilterExpression>,
    /// Names of policies that were applied
    pub applied_policies: Vec<String>,
    /// Whether access was completely denied
    pub access_denied: bool,
    /// Reason for denial (if denied)
    pub denial_reason: Option<String>,
}

impl RLSFilterResult {
    /// Create a result indicating no filters needed
    pub fn no_filters() -> Self {
        Self {
            filters_applied: false,
            filter: None,
            applied_policies: Vec::new(),
            access_denied: false,
            denial_reason: None,
        }
    }

    /// Create a result indicating access is denied
    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            filters_applied: false,
            filter: None,
            applied_policies: Vec::new(),
            access_denied: true,
            denial_reason: Some(reason.into()),
        }
    }

    /// Create a result with filters applied
    pub fn with_filter(filter: FilterExpression, policies: Vec<String>) -> Self {
        Self {
            filters_applied: true,
            filter: Some(filter),
            applied_policies: policies,
            access_denied: false,
            denial_reason: None,
        }
    }
}

/// Collection Row-Level Security service
pub struct CollectionRLS {
    /// RLS configuration
    config: RLSConfig,
    /// Policies by collection name
    policies: Arc<RwLock<HashMap<String, Vec<RLSPolicy>>>>,
    /// Cache for evaluated filters (key: user_id:collection:operation)
    filter_cache: Arc<RwLock<HashMap<String, CachedFilter>>>,
}

/// Cached filter result (uses Arc to avoid expensive cloning of FilterExpression)
struct CachedFilter {
    filter: Arc<RLSFilterResult>,
    cached_at: i64,
}

impl CollectionRLS {
    /// Create a new CollectionRLS service
    pub fn new(config: RLSConfig) -> Self {
        Self {
            config,
            policies: Arc::new(RwLock::new(HashMap::new())),
            filter_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a policy for a collection
    pub async fn register_policy(&self, policy: RLSPolicy) -> Result<()> {
        let collection = policy.collection.clone();
        let policy_name = policy.name.clone();

        let mut policies = self.policies.write().await;
        let collection_policies = policies.entry(collection.clone()).or_default();

        // Check for duplicate policy name
        if collection_policies.iter().any(|p| p.name == policy_name) {
            return Err(anyhow!(
                "Policy '{}' already exists for collection '{}'",
                policy_name,
                collection
            ));
        }

        collection_policies.push(policy);
        // Sort by priority
        collection_policies.sort_by_key(|p| p.priority);

        info!(
            "Registered RLS policy '{}' for collection '{}'",
            policy_name, collection
        );

        // Invalidate cache for this collection
        self.invalidate_collection_cache(&collection).await;

        Ok(())
    }

    /// Remove a policy from a collection
    pub async fn remove_policy(&self, collection: &str, policy_name: &str) -> Result<()> {
        let mut policies = self.policies.write().await;

        if let Some(collection_policies) = policies.get_mut(collection) {
            let original_len = collection_policies.len();
            collection_policies.retain(|p| p.name != policy_name);

            if collection_policies.len() == original_len {
                return Err(anyhow!(
                    "Policy '{}' not found for collection '{}'",
                    policy_name,
                    collection
                ));
            }

            info!(
                "Removed RLS policy '{}' from collection '{}'",
                policy_name, collection
            );

            // Invalidate cache
            self.invalidate_collection_cache(collection).await;
        }

        Ok(())
    }

    /// Get all policies for a collection
    pub async fn get_policies(&self, collection: &str) -> Vec<RLSPolicy> {
        let policies = self.policies.read().await;
        policies.get(collection).cloned().unwrap_or_default()
    }

    /// Apply security filters for a search operation
    pub async fn apply_security_filter(
        &self,
        collection: &str,
        operation: &Operation,
        user_context: &UnifiedUserContext,
    ) -> Result<Arc<RLSFilterResult>> {
        if !self.config.enabled {
            return Ok(Arc::new(RLSFilterResult::no_filters()));
        }

        // Check cache first
        let cache_key = format!("{}:{}:{:?}", user_context.user_id, collection, operation);
        if let Some(cached) = self.get_cached_filter(&cache_key).await {
            if self.config.debug_logging {
                debug!("RLS cache hit for {}", cache_key);
            }
            return Ok(cached);
        }

        // Get policies for this collection
        let policies = self.get_policies(collection).await;
        if policies.is_empty() {
            return Ok(Arc::new(RLSFilterResult::no_filters()));
        }

        // Filter to applicable policies
        let applicable_policies: Vec<&RLSPolicy> = policies
            .iter()
            .filter(|p| p.applies_to_operation(operation))
            .collect();

        if applicable_policies.is_empty() {
            return Ok(Arc::new(RLSFilterResult::no_filters()));
        }

        // Build combined filter from all applicable policies
        let mut filters = Vec::new();
        let mut applied_policy_names = Vec::new();

        for policy in applicable_policies {
            match self.build_filter(&policy.predicate, user_context) {
                Ok(Some(filter)) => {
                    filters.push(filter);
                    applied_policy_names.push(policy.name.clone());
                }
                Ok(None) => {
                    // AlwaysAllow - no filter needed
                    continue;
                }
                Err(e) => {
                    if self.config.debug_logging {
                        warn!("Failed to build filter for policy '{}': {}", policy.name, e);
                    }
                    // Deny access if we can't build the filter (fail-secure)
                    return Ok(Arc::new(RLSFilterResult::denied(format!(
                        "Security policy evaluation failed: {}",
                        e
                    ))));
                }
            }
        }

        let result = if filters.is_empty() {
            RLSFilterResult::no_filters()
        } else {
            // Combine all filters with AND (all policies must be satisfied)
            let combined_filter = if filters.len() == 1 {
                filters.into_iter().next().ok_or_else(|| {
                    anyhow!("Failed to extract filter from non-empty filters vector")
                })?
            } else {
                FilterExpression::And(filters)
            };

            RLSFilterResult::with_filter(combined_filter, applied_policy_names)
        };

        let result = Arc::new(result);

        // Cache the result
        self.cache_filter(&cache_key, Arc::clone(&result)).await;

        if self.config.debug_logging {
            debug!(
                "RLS applied {} policies to collection '{}' for user '{}'",
                result.applied_policies.len(),
                collection,
                user_context.user_id
            );
        }

        Ok(result)
    }

    /// Build a filter expression from a security predicate
    fn build_filter(
        &self,
        predicate: &SecurityPredicate,
        user_context: &UnifiedUserContext,
    ) -> Result<Option<FilterExpression>> {
        match predicate {
            SecurityPredicate::OwnerOnly { metadata_field } => {
                Ok(Some(FilterExpression::Comparison {
                    field: metadata_field.clone(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::Value::String(user_context.user_id.clone()),
                }))
            }

            SecurityPredicate::TenantIsolation {
                record_tenant_field,
            } => {
                match &user_context.tenant_id {
                    Some(tenant_id) => Ok(Some(FilterExpression::Comparison {
                        field: record_tenant_field.clone(),
                        operator: ComparisonOperator::Equals,
                        value: serde_json::Value::String(tenant_id.clone()),
                    })),
                    None => {
                        // No tenant context - deny access to tenant-isolated collections
                        Err(anyhow!("Tenant context required for tenant-isolated data"))
                    }
                }
            }

            SecurityPredicate::RoleBased {
                metadata_field,
                allowed_values,
            } => {
                // Check if user has any of the allowed roles
                let has_access = allowed_values
                    .iter()
                    .any(|v| user_context.roles.contains(v));

                if has_access {
                    // User has required role - no additional filter needed
                    Ok(None)
                } else {
                    // User lacks required role - filter to empty set
                    // Use an impossible filter (field equals impossible value)
                    Ok(Some(FilterExpression::Comparison {
                        field: metadata_field.clone(),
                        operator: ComparisonOperator::Equals,
                        value: serde_json::Value::String("__rls_access_denied__".to_string()),
                    }))
                }
            }

            SecurityPredicate::DepartmentIsolation {
                user_dept_field,
                record_dept_field,
            } => {
                // Get user's department from their metadata
                let user_dept = user_context.metadata.get(user_dept_field).ok_or_else(|| {
                    anyhow!("User department attribute '{}' not found", user_dept_field)
                })?;

                Ok(Some(FilterExpression::Comparison {
                    field: record_dept_field.clone(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::Value::String(user_dept.clone()),
                }))
            }

            SecurityPredicate::TimeBasedAccess { expiry_field } => {
                let now = Utc::now().timestamp();
                Ok(Some(FilterExpression::Comparison {
                    field: expiry_field.clone(),
                    operator: ComparisonOperator::GreaterThan,
                    value: serde_json::Value::Number(now.into()),
                }))
            }

            SecurityPredicate::ClassificationBased {
                record_field,
                user_clearance_field,
                classification_hierarchy,
            } => {
                // Get user's clearance level
                let user_clearance = user_context
                    .metadata
                    .get(user_clearance_field)
                    .ok_or_else(|| anyhow!("User clearance attribute not found"))?;

                // Find user's clearance index in hierarchy
                let user_clearance_idx = classification_hierarchy
                    .iter()
                    .position(|c| c == user_clearance)
                    .ok_or_else(|| anyhow!("Invalid clearance level"))?;

                // User can access records at or below their clearance level
                let accessible_levels: Vec<serde_json::Value> = classification_hierarchy
                    [..=user_clearance_idx]
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect();

                Ok(Some(FilterExpression::Comparison {
                    field: record_field.clone(),
                    operator: ComparisonOperator::In,
                    value: serde_json::Value::Array(accessible_levels),
                }))
            }

            SecurityPredicate::CustomFilter {
                field,
                operator,
                value_source,
            } => {
                let value = self.resolve_value_source(value_source, user_context)?;
                let comp_operator = self.convert_operator(operator);

                Ok(Some(FilterExpression::Comparison {
                    field: field.clone(),
                    operator: comp_operator,
                    value,
                }))
            }

            SecurityPredicate::And(predicates) => {
                let mut filters = Vec::new();
                for predicate in predicates {
                    if let Some(filter) = self.build_filter(predicate.as_ref(), user_context)? {
                        filters.push(filter);
                    }
                }

                match filters.len() {
                    0 => Ok(None),
                    1 => Ok(Some(filters.into_iter().next().ok_or_else(|| {
                        anyhow!("Failed to extract filter from non-empty filters vector")
                    })?)),
                    _ => Ok(Some(FilterExpression::And(filters))),
                }
            }

            SecurityPredicate::Or(predicates) => {
                let mut filters = Vec::new();
                for predicate in predicates {
                    if let Some(filter) = self.build_filter(predicate.as_ref(), user_context)? {
                        filters.push(filter);
                    }
                }

                match filters.len() {
                    0 => Ok(None),
                    1 => Ok(Some(filters.into_iter().next().ok_or_else(|| {
                        anyhow!("Failed to extract filter from non-empty filters vector")
                    })?)),
                    _ => Ok(Some(FilterExpression::Or(filters))),
                }
            }

            SecurityPredicate::Not(inner) => {
                if let Some(inner_filter) = self.build_filter(inner, user_context)? {
                    Ok(Some(FilterExpression::Not(Box::new(inner_filter))))
                } else {
                    Ok(None)
                }
            }

            SecurityPredicate::AlwaysAllow => Ok(None),

            SecurityPredicate::AlwaysDeny => Err(anyhow!("Access denied by security policy")),
        }
    }

    /// Resolve a value source to an actual value
    fn resolve_value_source(
        &self,
        source: &ValueSource,
        user_context: &UnifiedUserContext,
    ) -> Result<serde_json::Value> {
        match source {
            ValueSource::UserAttribute { attribute } => {
                let value = user_context
                    .metadata
                    .get(attribute)
                    .ok_or_else(|| anyhow!("User attribute '{}' not found", attribute))?;
                Ok(serde_json::Value::String(value.clone()))
            }
            ValueSource::Literal { value } => Ok(value.clone()),
            ValueSource::CurrentTimestamp => {
                Ok(serde_json::Value::Number(Utc::now().timestamp().into()))
            }
            ValueSource::UserTenant => {
                let tenant = user_context
                    .tenant_id
                    .clone()
                    .ok_or_else(|| anyhow!("User tenant not available"))?;
                Ok(serde_json::Value::String(tenant))
            }
            ValueSource::UserId => Ok(serde_json::Value::String(user_context.user_id.clone())),
        }
    }

    /// Convert RLS filter operator to search comparison operator
    fn convert_operator(&self, op: &FilterOperator) -> ComparisonOperator {
        match op {
            FilterOperator::Equals => ComparisonOperator::Equals,
            FilterOperator::NotEquals => ComparisonOperator::NotEquals,
            FilterOperator::GreaterThan => ComparisonOperator::GreaterThan,
            FilterOperator::GreaterThanOrEquals => ComparisonOperator::GreaterThanOrEqual,
            FilterOperator::LessThan => ComparisonOperator::LessThan,
            FilterOperator::LessThanOrEquals => ComparisonOperator::LessThanOrEqual,
            FilterOperator::Contains => ComparisonOperator::Contains,
            FilterOperator::StartsWith => ComparisonOperator::StartsWith,
            FilterOperator::EndsWith => ComparisonOperator::EndsWith,
            FilterOperator::In => ComparisonOperator::In,
            FilterOperator::NotIn => ComparisonOperator::NotIn,
        }
    }

    /// Get cached filter result
    async fn get_cached_filter(&self, key: &str) -> Option<Arc<RLSFilterResult>> {
        let cache = self.filter_cache.read().await;
        if let Some(cached) = cache.get(key) {
            let now = Utc::now().timestamp();
            if now - cached.cached_at < self.config.cache_ttl_seconds as i64 {
                return Some(Arc::clone(&cached.filter));
            }
        }
        None
    }

    /// Cache a filter result
    async fn cache_filter(&self, key: &str, result: Arc<RLSFilterResult>) {
        let mut cache = self.filter_cache.write().await;

        // Evict old entries if cache is full
        if cache.len() >= self.config.max_cache_entries {
            let now = Utc::now().timestamp();
            cache.retain(|_, v| now - v.cached_at < self.config.cache_ttl_seconds as i64);
        }

        cache.insert(
            key.to_string(),
            CachedFilter {
                filter: result,
                cached_at: Utc::now().timestamp(),
            },
        );
    }

    /// Invalidate cache for a collection
    async fn invalidate_collection_cache(&self, collection: &str) {
        let mut cache = self.filter_cache.write().await;
        cache.retain(|k, _| !k.contains(&format!(":{}", collection)));
    }

    /// Clear all cached filters
    pub async fn clear_cache(&self) {
        let mut cache = self.filter_cache.write().await;
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::rls::policy::RLSPolicy;
    use std::collections::HashSet;

    fn create_test_user(
        user_id: &str,
        tenant_id: Option<&str>,
        roles: Vec<&str>,
    ) -> UnifiedUserContext {
        UnifiedUserContext {
            user_id: user_id.to_string(),
            tenant_id: tenant_id.map(|s| s.to_string()),
            roles: roles.into_iter().map(|s| s.to_string()).collect(),
            effective_permissions: HashSet::new(),
            auth_method: crate::security::unified_rbac::AuthMethod::Internal,
            session_id: "test_session".to_string(),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    fn create_user_with_dept(user_id: &str, dept: &str) -> UnifiedUserContext {
        let mut user = create_test_user(user_id, None, vec![]);
        user.metadata
            .insert("department".to_string(), dept.to_string());
        user
    }

    #[tokio::test]
    async fn test_owner_only_filter() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("owner_only", "documents")
            .for_read()
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        let user = create_test_user("user123", None, vec![]);
        let result = rls
            .apply_security_filter("documents", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        assert!(result.filters_applied);
        assert!(!result.access_denied);
        assert_eq!(result.applied_policies, vec!["owner_only"]);

        // Verify the filter structure
        match result
            .filter
            .as_ref()
            .expect("Expected filter to be present")
        {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                assert_eq!(field, "owner_id");
                assert_eq!(*operator, ComparisonOperator::Equals);
                assert_eq!(*value, serde_json::Value::String("user123".to_string()));
            }
            _ => panic!("Expected Comparison filter"),
        }
    }

    #[tokio::test]
    async fn test_tenant_isolation_filter() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("tenant_isolation", "data")
            .for_all_operations()
            .with_predicate(SecurityPredicate::tenant_isolation("tenant_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        // User with tenant
        let user = create_test_user("user1", Some("tenant_abc"), vec![]);
        let result = rls
            .apply_security_filter("data", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        assert!(result.filters_applied);
        match result
            .filter
            .as_ref()
            .expect("Expected filter to be present")
        {
            FilterExpression::Comparison { value, .. } => {
                assert_eq!(*value, serde_json::Value::String("tenant_abc".to_string()));
            }
            _ => panic!("Expected Comparison filter"),
        }

        // User without tenant - should be denied (fail-secure)
        let user_no_tenant = create_test_user("user2", None, vec![]);
        let result = rls
            .apply_security_filter("data", &Operation::Read, &user_no_tenant)
            .await
            .expect("Failed to apply security filter");

        assert!(result.access_denied);
        assert!(result.denial_reason.is_some());
    }

    #[tokio::test]
    async fn test_role_based_access() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("admin_only", "config")
            .for_read()
            .with_predicate(SecurityPredicate::RoleBased {
                metadata_field: "access_level".to_string(),
                allowed_values: vec!["admin".to_string(), "superuser".to_string()],
            })
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        // Admin user - should have access without filter
        let admin = create_test_user("admin1", None, vec!["admin"]);
        let result = rls
            .apply_security_filter("config", &Operation::Read, &admin)
            .await
            .expect("Failed to apply security filter");

        // Role-based with matching role returns no filter (full access)
        assert!(!result.filters_applied);

        // Regular user - should get access denied filter
        let regular = create_test_user("user1", None, vec!["viewer"]);
        let result = rls
            .apply_security_filter("config", &Operation::Read, &regular)
            .await
            .expect("Failed to apply security filter");

        assert!(result.filters_applied);
    }

    #[tokio::test]
    async fn test_department_isolation() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("dept_isolation", "projects")
            .for_read()
            .with_predicate(SecurityPredicate::DepartmentIsolation {
                user_dept_field: "department".to_string(),
                record_dept_field: "project_dept".to_string(),
            })
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        let user = create_user_with_dept("user1", "engineering");
        let result = rls
            .apply_security_filter("projects", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        assert!(result.filters_applied);
        match result
            .filter
            .as_ref()
            .expect("Expected filter to be present")
        {
            FilterExpression::Comparison { field, value, .. } => {
                assert_eq!(field, "project_dept");
                assert_eq!(*value, serde_json::Value::String("engineering".to_string()));
            }
            _ => panic!("Expected Comparison filter"),
        }
    }

    #[tokio::test]
    async fn test_combined_policies() {
        let rls = CollectionRLS::new(RLSConfig::default());

        // Register multiple policies
        let owner_policy = RLSPolicy::builder("owner", "docs")
            .for_read()
            .priority(10)
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .build()
            .expect("Failed to build test policy");

        let tenant_policy = RLSPolicy::builder("tenant", "docs")
            .for_read()
            .priority(20)
            .with_predicate(SecurityPredicate::tenant_isolation("tenant_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(owner_policy)
            .await
            .expect("Failed to register test policy");
        rls.register_policy(tenant_policy)
            .await
            .expect("Failed to register test policy");

        let user = create_test_user("user1", Some("tenant_x"), vec![]);
        let result = rls
            .apply_security_filter("docs", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        assert!(result.filters_applied);
        assert_eq!(result.applied_policies.len(), 2);

        // Should be AND of both filters
        match result
            .filter
            .as_ref()
            .expect("Expected filter to be present")
        {
            FilterExpression::And(filters) => {
                assert_eq!(filters.len(), 2);
            }
            _ => panic!("Expected And filter"),
        }
    }

    #[tokio::test]
    async fn test_operation_filtering() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("read_only", "data")
            .for_read()
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        let user = create_test_user("user1", None, vec![]);

        // Read should apply policy
        let read_result = rls
            .apply_security_filter("data", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");
        assert!(read_result.filters_applied);

        // Write should not apply this policy
        let write_result = rls
            .apply_security_filter("data", &Operation::Write, &user)
            .await
            .expect("Failed to apply security filter");
        assert!(!write_result.filters_applied);
    }

    #[tokio::test]
    async fn test_cache_functionality() {
        let config = RLSConfig {
            cache_ttl_seconds: 60,
            ..Default::default()
        };
        let rls = CollectionRLS::new(config);

        let policy = RLSPolicy::builder("test", "data")
            .for_read()
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");

        let user = create_test_user("user1", None, vec![]);

        // First call populates cache
        let result1 = rls
            .apply_security_filter("data", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        // Second call should use cache
        let result2 = rls
            .apply_security_filter("data", &Operation::Read, &user)
            .await
            .expect("Failed to apply security filter");

        assert_eq!(result1.applied_policies, result2.applied_policies);
    }

    #[tokio::test]
    async fn test_policy_removal() {
        let rls = CollectionRLS::new(RLSConfig::default());

        let policy = RLSPolicy::builder("test_policy", "data")
            .for_read()
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .build()
            .expect("Failed to build test policy");

        rls.register_policy(policy)
            .await
            .expect("Failed to register test policy");
        assert_eq!(rls.get_policies("data").await.len(), 1);

        rls.remove_policy("data", "test_policy")
            .await
            .expect("Failed to remove test policy");
        assert_eq!(rls.get_policies("data").await.len(), 0);
    }
}
