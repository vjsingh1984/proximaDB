//! RLS Policy definitions and security predicates

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Operations that RLS policies can apply to
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Read operations (search, get)
    Read,
    /// Write operations (insert, update)
    Write,
    /// Delete operations
    Delete,
    /// All operations
    All,
}

impl Operation {
    /// Check if this operation matches the requested operation
    pub fn matches(&self, requested: &Operation) -> bool {
        matches!(self, Operation::All) || self == requested
    }
}

/// Row-Level Security policy
///
/// Note: Policies are built programmatically using the builder pattern.
#[derive(Debug, Clone)]
pub struct RLSPolicy {
    /// Policy name for identification
    pub name: String,
    /// Collection this policy applies to
    pub collection: String,
    /// Operations this policy applies to
    pub operations: HashSet<Operation>,
    /// Security predicate that defines the filter
    pub predicate: SecurityPredicate,
    /// Whether the policy is enabled
    pub enabled: bool,
    /// Priority (lower = higher priority, evaluated first)
    pub priority: i32,
    /// Optional description
    pub description: Option<String>,
}

impl RLSPolicy {
    /// Create a new RLS policy builder
    pub fn builder(name: impl Into<String>, collection: impl Into<String>) -> RLSPolicyBuilder {
        RLSPolicyBuilder::new(name, collection)
    }

    /// Check if this policy applies to the given operation
    pub fn applies_to_operation(&self, operation: &Operation) -> bool {
        self.enabled && self.operations.iter().any(|op| op.matches(operation))
    }
}

/// Builder for RLS policies
pub struct RLSPolicyBuilder {
    /// Policy name for identification
    name: String,
    /// Target collection this policy applies to
    collection: String,
    /// Set of operations (read, write, delete) this policy covers
    operations: HashSet<Operation>,
    /// Security predicate defining the row filter logic
    predicate: Option<SecurityPredicate>,
    /// Whether the policy is active
    enabled: bool,
    /// Evaluation priority (lower values are evaluated first)
    priority: i32,
    /// Optional human-readable description of the policy
    description: Option<String>,
}

impl RLSPolicyBuilder {
    pub fn new(name: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collection: collection.into(),
            operations: HashSet::new(),
            predicate: None,
            enabled: true,
            priority: 100,
            description: None,
        }
    }

    pub fn for_read(mut self) -> Self {
        self.operations.insert(Operation::Read);
        self
    }

    pub fn for_write(mut self) -> Self {
        self.operations.insert(Operation::Write);
        self
    }

    pub fn for_delete(mut self) -> Self {
        self.operations.insert(Operation::Delete);
        self
    }

    pub fn for_all_operations(mut self) -> Self {
        self.operations.insert(Operation::All);
        self
    }

    pub fn with_predicate(mut self, predicate: SecurityPredicate) -> Self {
        self.predicate = Some(predicate);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn build(self) -> Result<RLSPolicy, &'static str> {
        if self.operations.is_empty() {
            return Err("At least one operation must be specified");
        }
        let predicate = self.predicate.ok_or("Predicate is required")?;

        Ok(RLSPolicy {
            name: self.name,
            collection: self.collection,
            operations: self.operations,
            predicate,
            enabled: self.enabled,
            priority: self.priority,
            description: self.description,
        })
    }
}

/// Security predicate that defines how records are filtered
///
/// Note: Serde derives removed due to recursive type compilation performance.
/// Predicates are typically built programmatically using the builder pattern.
#[derive(Debug, Clone)]
pub enum SecurityPredicate {
    /// User can only see records they own (based on metadata field)
    OwnerOnly {
        /// Metadata field that contains the owner ID
        metadata_field: String,
    },

    /// Role-based access: user must have one of the allowed roles
    RoleBased {
        /// Metadata field to check
        metadata_field: String,
        /// Allowed values that grant access
        allowed_values: Vec<String>,
    },

    /// Department isolation: user's department must match record's department
    DepartmentIsolation {
        /// User context field containing user's department
        user_dept_field: String,
        /// Record metadata field containing record's department
        record_dept_field: String,
    },

    /// Tenant isolation: user's tenant must match record's tenant
    TenantIsolation {
        /// Record metadata field containing tenant ID
        record_tenant_field: String,
    },

    /// Time-based access: record must not be expired
    TimeBasedAccess {
        /// Metadata field containing expiry timestamp (unix seconds)
        expiry_field: String,
    },

    /// Classification-based: user must have clearance for classification level
    ClassificationBased {
        /// Record metadata field containing classification level
        record_field: String,
        /// User attribute containing clearance level
        user_clearance_field: String,
        /// Ordered list of classification levels (lowest to highest)
        classification_hierarchy: Vec<String>,
    },

    /// Custom metadata filter expression
    CustomFilter {
        /// Field to filter on
        field: String,
        /// Comparison operator
        operator: FilterOperator,
        /// Value source
        value_source: ValueSource,
    },

    /// Combination of predicates (all must pass)
    And(Vec<Box<SecurityPredicate>>),

    /// Combination of predicates (at least one must pass)
    Or(Vec<Box<SecurityPredicate>>),

    /// Negate a predicate
    Not(Box<SecurityPredicate>),

    /// Always allow (useful for admin bypass)
    AlwaysAllow,

    /// Always deny (useful for disabled collections)
    AlwaysDeny,
}

impl SecurityPredicate {
    /// Create a builder for security predicates
    pub fn builder() -> SecurityPredicateBuilder {
        SecurityPredicateBuilder::new()
    }

    /// Create an owner-only predicate
    pub fn owner_only(metadata_field: impl Into<String>) -> Self {
        SecurityPredicate::OwnerOnly {
            metadata_field: metadata_field.into(),
        }
    }

    /// Create a tenant isolation predicate
    pub fn tenant_isolation(record_tenant_field: impl Into<String>) -> Self {
        SecurityPredicate::TenantIsolation {
            record_tenant_field: record_tenant_field.into(),
        }
    }

    /// Create a time-based access predicate
    pub fn time_based(expiry_field: impl Into<String>) -> Self {
        SecurityPredicate::TimeBasedAccess {
            expiry_field: expiry_field.into(),
        }
    }

    /// Combine with another predicate using AND
    pub fn and(self, other: SecurityPredicate) -> Self {
        match self {
            SecurityPredicate::And(mut predicates) => {
                predicates.push(Box::new(other));
                SecurityPredicate::And(predicates)
            }
            _ => SecurityPredicate::And(vec![Box::new(self), Box::new(other)]),
        }
    }

    /// Combine with another predicate using OR
    pub fn or(self, other: SecurityPredicate) -> Self {
        match self {
            SecurityPredicate::Or(mut predicates) => {
                predicates.push(Box::new(other));
                SecurityPredicate::Or(predicates)
            }
            _ => SecurityPredicate::Or(vec![Box::new(self), Box::new(other)]),
        }
    }

    /// Negate this predicate
    pub fn not(self) -> Self {
        SecurityPredicate::Not(Box::new(self))
    }
}

/// Filter operators for custom filters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEquals,
    LessThan,
    LessThanOrEquals,
    Contains,
    StartsWith,
    EndsWith,
    In,
    NotIn,
}

/// Source of filter value
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ValueSource {
    /// Value from user context attribute
    UserAttribute { attribute: String },
    /// Literal value
    Literal { value: serde_json::Value },
    /// Current timestamp
    CurrentTimestamp,
    /// User's tenant ID
    UserTenant,
    /// User's ID
    UserId,
}

/// Builder for security predicates
pub struct SecurityPredicateBuilder {
    /// Accumulated predicates to be combined via AND or OR
    predicates: Vec<SecurityPredicate>,
}

impl SecurityPredicateBuilder {
    pub fn new() -> Self {
        Self {
            predicates: Vec::new(),
        }
    }

    pub fn owner_only(mut self, metadata_field: impl Into<String>) -> Self {
        self.predicates
            .push(SecurityPredicate::owner_only(metadata_field));
        self
    }

    pub fn tenant_isolation(mut self, record_tenant_field: impl Into<String>) -> Self {
        self.predicates
            .push(SecurityPredicate::tenant_isolation(record_tenant_field));
        self
    }

    pub fn role_based(
        mut self,
        metadata_field: impl Into<String>,
        allowed_values: Vec<String>,
    ) -> Self {
        self.predicates.push(SecurityPredicate::RoleBased {
            metadata_field: metadata_field.into(),
            allowed_values,
        });
        self
    }

    pub fn department_isolation(
        mut self,
        user_dept_field: impl Into<String>,
        record_dept_field: impl Into<String>,
    ) -> Self {
        self.predicates
            .push(SecurityPredicate::DepartmentIsolation {
                user_dept_field: user_dept_field.into(),
                record_dept_field: record_dept_field.into(),
            });
        self
    }

    pub fn time_based(mut self, expiry_field: impl Into<String>) -> Self {
        self.predicates
            .push(SecurityPredicate::time_based(expiry_field));
        self
    }

    /// Build as AND combination (all predicates must pass)
    pub fn build_and(self) -> SecurityPredicate {
        match self.predicates.len() {
            0 => SecurityPredicate::AlwaysAllow,
            1 => {
                if let Some(predicate) = self.predicates.into_iter().next() {
                    predicate
                } else {
                    SecurityPredicate::AlwaysAllow
                }
            }
            _ => SecurityPredicate::And(self.predicates.into_iter().map(Box::new).collect()),
        }
    }

    /// Build as OR combination (any predicate must pass)
    pub fn build_or(self) -> SecurityPredicate {
        match self.predicates.len() {
            0 => SecurityPredicate::AlwaysDeny,
            1 => {
                if let Some(predicate) = self.predicates.into_iter().next() {
                    predicate
                } else {
                    SecurityPredicate::AlwaysDeny
                }
            }
            _ => SecurityPredicate::Or(self.predicates.into_iter().map(Box::new).collect()),
        }
    }
}

impl Default for SecurityPredicateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_matching() {
        assert!(Operation::All.matches(&Operation::Read));
        assert!(Operation::All.matches(&Operation::Write));
        assert!(Operation::Read.matches(&Operation::Read));
        assert!(!Operation::Read.matches(&Operation::Write));
    }

    #[test]
    fn test_policy_builder() {
        let policy = RLSPolicy::builder("owner_policy", "documents")
            .for_read()
            .for_write()
            .with_predicate(SecurityPredicate::owner_only("owner_id"))
            .priority(10)
            .description("Users can only access their own documents")
            .build()
            .unwrap();

        assert_eq!(policy.name, "owner_policy");
        assert_eq!(policy.collection, "documents");
        assert!(policy.applies_to_operation(&Operation::Read));
        assert!(policy.applies_to_operation(&Operation::Write));
        assert!(!policy.applies_to_operation(&Operation::Delete));
    }

    #[test]
    fn test_predicate_builder() {
        let predicate = SecurityPredicate::builder()
            .owner_only("owner_id")
            .tenant_isolation("tenant_id")
            .build_and();

        match predicate {
            SecurityPredicate::And(predicates) => {
                assert_eq!(predicates.len(), 2);
            }
            _ => panic!("Expected And predicate"),
        }
    }

    #[test]
    fn test_predicate_composition() {
        let owner = SecurityPredicate::owner_only("owner_id");
        let tenant = SecurityPredicate::tenant_isolation("tenant_id");

        let combined = owner.and(tenant);
        match combined {
            SecurityPredicate::And(predicates) => {
                assert_eq!(predicates.len(), 2);
                // Verify the predicates are boxed correctly
                assert!(matches!(
                    predicates[0].as_ref(),
                    SecurityPredicate::OwnerOnly { .. }
                ));
                assert!(matches!(
                    predicates[1].as_ref(),
                    SecurityPredicate::TenantIsolation { .. }
                ));
            }
            _ => panic!("Expected And predicate"),
        }
    }

    // Note: Serialization test removed since SecurityPredicate no longer has serde derives
    // due to recursive type compilation performance issues. Predicates are built
    // programmatically using the builder pattern.
}
