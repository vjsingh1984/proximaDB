//! SQL Validator
//!
//! Validates and sanitizes SQL queries to prevent injection attacks
//! and ensure tenant isolation compliance.

use crate::ai::natural_language::translator::UserContext;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use anyhow::{Result, anyhow};
use tracing::{debug, error};
use sqlparser::ast::Query;

/// SQL Validator for security and safety
#[derive(Debug, Clone)]
pub struct SQLValidator {
    config: SQLValidatorConfig,
    allowed_functions: HashSet<String>,
    forbidden_patterns: Vec<String>,
}

/// Configuration for SQL validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SQLValidatorConfig {
    pub enable_strict_validation: bool,
    pub allow_joins: bool,
    pub allow_subqueries: bool,
    pub allow_aggregations: bool,
    pub max_result_limit: u32,
    pub require_tenant_filtering: bool,
    pub allowed_sql_operations: Vec<String>,
}

/// Result of SQL validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub sanitized_sql: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub security_issues: Vec<SecurityIssue>,
    pub tenant_isolation_verified: bool,
}

/// Security issues found in SQL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityIssue {
    pub issue_type: SecurityIssueType,
    pub description: String,
    pub severity: SecuritySeverity,
    pub location: Option<String>,
}

/// Types of security issues
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityIssueType {
    SQLInjection,
    UnauthorizedTableAccess,
    MissingTenantFilter,
    ForbiddenOperation,
    ExcessiveResultSize,
    UnsafeFunction,
}

/// Severity levels for security issues
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecuritySeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl SQLValidator {
    pub async fn new() -> Result<Self> {
        let config = SQLValidatorConfig::default();

        // Build list of allowed SQL functions
        let allowed_functions = [
            // String functions
            "UPPER", "LOWER", "SUBSTR", "LENGTH", "TRIM", "CONCAT",
            // Math functions
            "SUM", "COUNT", "AVG", "MIN", "MAX", "ROUND", "ABS",
            // Date functions
            "NOW", "DATE", "EXTRACT", "DATE_TRUNC",
            // Conditional functions
            "CASE", "COALESCE", "NULLIF",
            // Vector functions (ProximaDB specific)
            "VECTOR_SIMILARITY", "COSINE_DISTANCE", "DOT_PRODUCT",
        ].iter().map(|s| s.to_string()).collect();

        // Build list of forbidden patterns
        let forbidden_patterns = vec![
            // SQL injection patterns
            r"(?i)(\s|^)(DROP|DELETE|UPDATE|INSERT|ALTER|CREATE|TRUNCATE)\s".to_string(),
            r"(?i)--\s*.*".to_string(), // SQL comments
            r"(?i)/\*.*\*/".to_string(), // Block comments
            r"(?i)(\s|^)EXEC(\s|\(|$)".to_string(),
            r"(?i)(\s|^)EXECUTE(\s|\(|$)".to_string(),
            r"(?i)(\s|^)xp_".to_string(),
            r"(?i)(\s|^)sp_".to_string(),

            // System functions and procedures
            r"(?i)(\s|^)(LOAD_FILE|INTO\s+OUTFILE|INTO\s+DUMPFILE)".to_string(),
            r"(?i)(\s|^)(UNION\s+SELECT|UNION\s+ALL\s+SELECT)".to_string(),

            // Information schema access
            r"(?i)information_schema".to_string(),
            r"(?i)sys\.".to_string(),
            r"(?i)pg_".to_string(),
        ];

        Ok(Self {
            config,
            allowed_functions,
            forbidden_patterns,
        })
    }

    /// Validate and sanitize SQL query
    pub async fn validate_and_sanitize(
        &self,
        sql: &str,
        user_context: &UserContext,
    ) -> Result<String, anyhow::Error> {
        debug!("Validating SQL query for user {}", user_context.user_id);

        let validation_result = self.validate_sql(sql, user_context).await?;

        if !validation_result.is_valid {
            error!("SQL validation failed for user {}: {:?}", user_context.user_id, validation_result.errors);
            return Err(anyhow!("SQL validation failed: {}", validation_result.errors.join("; ")));
        }

        // Check for critical security issues
        let critical_issues: Vec<&SecurityIssue> = validation_result.security_issues.iter()
            .filter(|issue| matches!(issue.severity, SecuritySeverity::Critical))
            .collect();

        if !critical_issues.is_empty() {
            error!("Critical security issues found in SQL: {:?}", critical_issues);
            return Err(anyhow!("Critical security issues detected in query"));
        }

        if !validation_result.tenant_isolation_verified && self.config.require_tenant_filtering {
            return Err(anyhow!("Query lacks proper tenant isolation filtering"));
        }

        Ok(validation_result.sanitized_sql)
    }

    /// Comprehensive SQL validation
    async fn validate_sql(&self, sql: &str, user_context: &UserContext) -> Result<ValidationResult> {
        let warnings = Vec::new();
        let mut errors = Vec::new();
        let mut security_issues = Vec::new();

        // Step 1: Basic format validation
        if sql.trim().is_empty() {
            errors.push("Empty SQL query".to_string());
        }

        if sql.len() > 10000 {
            errors.push("SQL query too long (max 10,000 characters)".to_string());
        }

        // Step 2: Check for forbidden patterns
        for pattern in &self.forbidden_patterns {
            if let Ok(regex) = regex::Regex::new(pattern) {
                if regex.is_match(sql) {
                    security_issues.push(SecurityIssue {
                        issue_type: SecurityIssueType::SQLInjection,
                        description: format!("Forbidden pattern detected: {}", pattern),
                        severity: SecuritySeverity::Critical,
                        location: None,
                    });
                }
            }
        }

        // Step 3: Validate SQL structure using sqlparser
        let sanitized_sql = match self.parse_and_sanitize_sql(sql, user_context) {
            Ok(sanitized) => sanitized,
            Err(e) => {
                errors.push(format!("SQL parsing failed: {}", e));
                sql.to_string()
            }
        };

        // Step 4: Check tenant isolation
        let tenant_isolation_verified = self.verify_tenant_isolation(&sanitized_sql, user_context);

        if !tenant_isolation_verified && self.config.require_tenant_filtering {
            security_issues.push(SecurityIssue {
                issue_type: SecurityIssueType::MissingTenantFilter,
                description: "Query lacks proper tenant_id filtering".to_string(),
                severity: SecuritySeverity::High,
                location: None,
            });
        }

        // Step 5: Validate table access
        let table_access_issues = self.validate_table_access(&sanitized_sql, user_context);
        security_issues.extend(table_access_issues);

        // Step 6: Check for excessive result limits
        if let Some(limit_issue) = self.check_result_limits(&sanitized_sql) {
            security_issues.push(limit_issue);
        }

        let is_valid = errors.is_empty() && security_issues.iter().all(|issue| {
            !matches!(issue.severity, SecuritySeverity::Critical)
        });

        Ok(ValidationResult {
            is_valid,
            sanitized_sql,
            warnings,
            errors,
            security_issues,
            tenant_isolation_verified,
        })
    }

    /// Parse and sanitize SQL using sqlparser
    fn parse_and_sanitize_sql(&self, sql: &str, user_context: &UserContext) -> Result<String> {
        use sqlparser::parser::Parser;
        use sqlparser::dialect::PostgreSqlDialect;
        use sqlparser::ast::Statement;

        let dialect = PostgreSqlDialect {};
        let parser = Parser::new(&dialect);

        // Parse SQL
        let statements = parser.try_with_sql(sql)
            .map_err(|e| anyhow!("Failed to parse SQL: {}", e))?
            .parse_statements()
            .map_err(|e| anyhow!("Failed to parse statements: {}", e))?;

        if statements.len() != 1 {
            return Err(anyhow!("Only single statements are allowed"));
        }

        let statement = &statements[0];

        // Only allow SELECT statements
        match statement {
            Statement::Query(query) => {
                let sanitized_query = self.sanitize_select_query(query, user_context)?;
                Ok(format!("{}", sanitized_query))
            }
            _ => {
                Err(anyhow!("Only SELECT queries are allowed"))
            }
        }
    }

    /// Sanitize SELECT query
    fn sanitize_select_query(&self, query: &Query, user_context: &UserContext) -> Result<Query> {
        let mut sanitized_query = query.clone();

        // Add tenant filtering if required
        if self.config.require_tenant_filtering && user_context.tenant_id.is_some() {
            sanitized_query = self.add_tenant_filtering(sanitized_query, user_context)?;
        }

        // Add result limiting if not present
        if sanitized_query.limit.is_none() {
            use sqlparser::ast::Expr;
            sanitized_query.limit = Some(Expr::Value(sqlparser::ast::Value::Number(
                self.config.max_result_limit.to_string(),
                false,
            )));
        }

        Ok(sanitized_query)
    }

    /// Add tenant filtering to query
    fn add_tenant_filtering(&self, query: Query, user_context: &UserContext) -> Result<Query> {
        // This is a simplified implementation
        // In practice, would need to traverse the AST and add WHERE clauses

        if let Some(ref tenant_id) = user_context.tenant_id {
            debug!("Adding tenant filtering for tenant: {}", tenant_id);
            // Would implement AST modification to add tenant_id = '{}' filters
        }

        Ok(query)
    }

    /// Verify tenant isolation in SQL
    fn verify_tenant_isolation(&self, sql: &str, user_context: &UserContext) -> bool {
        if let Some(ref tenant_id) = user_context.tenant_id {
            // Check if query includes tenant_id filtering
            let sql_lower = sql.to_lowercase();
            sql_lower.contains("tenant_id") && sql_lower.contains(tenant_id)
        } else {
            true // No tenant filtering required if no tenant_id
        }
    }

    /// Validate table access permissions
    fn validate_table_access(&self, sql: &str, user_context: &UserContext) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();

        // Extract table names from SQL (simplified approach)
        let sql_upper = sql.to_uppercase();
        let accessible_tables_upper: HashSet<String> = user_context.accessible_tables.iter()
            .map(|t| t.to_uppercase())
            .collect();

        // Check for unauthorized table access
        for accessible_table in &user_context.accessible_tables {
            if sql_upper.contains(&accessible_table.to_uppercase()) &&
               !accessible_tables_upper.contains(&accessible_table.to_uppercase()) {
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::UnauthorizedTableAccess,
                    description: format!("Unauthorized access to table: {}", accessible_table),
                    severity: SecuritySeverity::Critical,
                    location: Some(accessible_table.clone()),
                });
            }
        }

        issues
    }

    /// Check for excessive result limits
    fn check_result_limits(&self, sql: &str) -> Option<SecurityIssue> {
        let sql_upper = sql.to_uppercase();

        // Look for LIMIT clause
        if let Some(limit_pos) = sql_upper.find("LIMIT") {
            // Extract the limit value (simplified parsing)
            let after_limit = &sql[limit_pos + 5..].trim();
            if let Some(space_pos) = after_limit.find(' ') {
                let limit_str = &after_limit[..space_pos];
                if let Ok(limit_value) = limit_str.parse::<u32>() {
                    if limit_value > self.config.max_result_limit {
                        return Some(SecurityIssue {
                            issue_type: SecurityIssueType::ExcessiveResultSize,
                            description: format!("LIMIT {} exceeds maximum allowed {}", limit_value, self.config.max_result_limit),
                            severity: SecuritySeverity::Medium,
                            location: Some(format!("LIMIT {}", limit_value)),
                        });
                    }
                }
            }
        }

        None
    }
}

impl Default for SQLValidatorConfig {
    fn default() -> Self {
        Self {
            enable_strict_validation: true,
            allow_joins: true,
            allow_subqueries: false, // Subqueries can be complex and risky
            allow_aggregations: true,
            max_result_limit: 10000,
            require_tenant_filtering: true,
            allowed_sql_operations: vec!["SELECT".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sql_validator_creation() {
        let validator = SQLValidator::new().await.unwrap();
        assert!(validator.config.enable_strict_validation);
        assert!(!validator.allowed_functions.is_empty());
    }

    #[tokio::test]
    async fn test_safe_sql_validation() {
        let validator = SQLValidator::new().await.unwrap();
        let user_context = UserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant_1".to_string()),
            accessible_tables: vec!["collections".to_string(), "vectors".to_string()],
            permissions: vec!["read_data".to_string()],
            roles: vec!["analyst".to_string()],
        };

        let safe_sql = "SELECT id, name FROM collections WHERE tenant_id = 'tenant_1' LIMIT 100";
        let result = validator.validate_and_sanitize(safe_sql, &user_context).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_malicious_sql_rejection() {
        let validator = SQLValidator::new().await.unwrap();
        let user_context = UserContext::default();

        let malicious_sqls = vec![
            "DROP TABLE users;",
            "SELECT * FROM users; DELETE FROM users;",
            "SELECT * FROM users WHERE id = 1 OR 1=1",
            "INSERT INTO users (name) VALUES ('hacker')",
            "UPDATE users SET password = 'hacked'",
        ];

        for malicious_sql in malicious_sqls {
            let result = validator.validate_and_sanitize(malicious_sql, &user_context).await;
            assert!(result.is_err(), "Malicious SQL should be rejected: {}", malicious_sql);
        }
    }

    #[test]
    fn test_tenant_isolation_verification() {
        let validator = SQLValidator {
            config: SQLValidatorConfig::default(),
            allowed_functions: HashSet::new(),
            forbidden_patterns: vec![],
        };

        let user_context = UserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant_1".to_string()),
            accessible_tables: vec!["collections".to_string()],
            permissions: vec!["read_data".to_string()],
            roles: vec!["analyst".to_string()],
        };

        // Query with tenant filtering
        let good_sql = "SELECT * FROM collections WHERE tenant_id = 'tenant_1'";
        assert!(validator.verify_tenant_isolation(good_sql, &user_context));

        // Query without tenant filtering
        let bad_sql = "SELECT * FROM collections";
        assert!(!validator.verify_tenant_isolation(bad_sql, &user_context));
    }

    #[test]
    fn test_result_limit_checking() {
        let validator = SQLValidator {
            config: SQLValidatorConfig {
                max_result_limit: 1000,
                ..Default::default()
            },
            allowed_functions: HashSet::new(),
            forbidden_patterns: vec![],
        };

        // Query with acceptable limit
        let good_sql = "SELECT * FROM collections LIMIT 500";
        assert!(validator.check_result_limits(good_sql).is_none());

        // Query with excessive limit
        let bad_sql = "SELECT * FROM collections LIMIT 50000";
        let issue = validator.check_result_limits(bad_sql);
        assert!(issue.is_some());
        assert!(matches!(issue.unwrap().issue_type, SecurityIssueType::ExcessiveResultSize));
    }
}