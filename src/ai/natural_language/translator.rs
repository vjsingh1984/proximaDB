//! Natural Language Query Translator
//!
//! Implements natural language to SQL translation using LLM providers
//! with security validation and tenant isolation.

use crate::ai::llm_integration::{LLMIntegrationEngine, LLMRequest, LLMResponse, LLMError};
use crate::ai::llm_integration::types::LLMRequestContext;
use super::schema_context::{SchemaContext, SchemaContextBuilder};
use super::sql_validator::{SQLValidator, ValidationResult};
use super::prompt_builder::{PromptBuilder, PromptTemplate};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use thiserror::Error;
use tracing::{debug, warn, error, info};

/// Natural Language Query Translator
#[derive(Clone)]
pub struct NLQueryTranslator {
    llm_engine: Arc<LLMIntegrationEngine>,
    schema_context: Arc<SchemaContext>,
    sql_validator: Arc<SQLValidator>,
    prompt_builder: Arc<PromptBuilder>,
    config: TranslatorConfig,
}

/// Configuration for the translator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatorConfig {
    pub enable_query_caching: bool,
    pub max_query_complexity: u32,
    pub enable_explanation_generation: bool,
    pub safety_validation_enabled: bool,
    pub tenant_isolation_required: bool,
    pub max_translation_attempts: u32,
}

/// Result of natural language translation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationResult {
    pub sql: String,
    pub confidence: f32,
    pub explanation: String,
    pub security_context: UserContext,
    pub accessible_tables: Vec<String>,
    pub translation_metadata: TranslationMetadata,
}

/// Metadata about the translation process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationMetadata {
    pub original_query: String,
    pub provider_used: String,
    pub translation_time_ms: u64,
    pub tokens_used: u32,
    pub validation_passed: bool,
    pub safety_checks_passed: bool,
    pub translated_at: DateTime<Utc>,
}

/// User context for translation (simplified version)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub tenant_id: Option<String>,
    pub accessible_tables: Vec<String>,
    pub permissions: Vec<String>,
    pub roles: Vec<String>,
}

/// Errors that can occur during translation
#[derive(Debug, Error, Clone)]
pub enum TranslationError {
    #[error("LLM provider error: {0}")]
    LLMProviderError(#[from] LLMError),

    #[error("SQL validation failed: {reason}")]
    SQLValidationFailed { reason: String },

    #[error("Security validation failed: {reason}")]
    SecurityValidationFailed { reason: String },

    #[error("Schema context error: {0}")]
    SchemaContextError(String),

    #[error("Translation timeout after {seconds}s")]
    TranslationTimeout { seconds: u64 },

    #[error("Query too complex: {reason}")]
    QueryTooComplex { reason: String },

    #[error("Unsafe query detected: {reason}")]
    UnsafeQueryDetected { reason: String },

    #[error("Permission denied for user {user_id}: {reason}")]
    PermissionDenied { user_id: String, reason: String },

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Internal translation error: {0}")]
    InternalError(String),
}

impl NLQueryTranslator {
    /// Create a new Natural Language Query Translator
    pub async fn new(
        llm_engine: Arc<LLMIntegrationEngine>,
        config: TranslatorConfig,
    ) -> Result<Self, TranslationError> {
        let schema_context = Arc::new(SchemaContext::new().await
            .map_err(|e| TranslationError::SchemaContextError(format!("Failed to initialize schema context: {}", e)))?);

        let sql_validator = Arc::new(SQLValidator::new().await
            .map_err(|e| TranslationError::ConfigurationError(format!("Failed to initialize SQL validator: {}", e)))?);

        let prompt_builder = Arc::new(PromptBuilder::new());

        Ok(Self {
            llm_engine,
            schema_context,
            sql_validator,
            prompt_builder,
            config,
        })
    }

    /// Translate natural language query to SQL with full security validation
    pub async fn translate_to_sql(
        &self,
        nl_query: &str,
        user_context: &UserContext,
    ) -> Result<TranslationResult, TranslationError> {
        let start_time = std::time::Instant::now();

        info!("Starting NL translation for user {}: {}", user_context.user_id, nl_query.chars().take(100).collect::<String>());

        // Step 1: Validate user permissions
        self.validate_user_permissions(user_context)?;

        // Step 2: Analyze query complexity
        self.validate_query_complexity(nl_query)?;

        // Step 3: Get accessible tables for the user
        let accessible_tables = self.get_user_accessible_tables(user_context).await?;

        if accessible_tables.is_empty() {
            return Err(TranslationError::PermissionDenied {
                user_id: user_context.user_id.clone(),
                reason: "No accessible tables found for user".to_string(),
            });
        }

        // Step 4: Build schema context for accessible tables only
        let schema_context = self.schema_context.build_context(&accessible_tables).await
            .map_err(|e| TranslationError::SchemaContextError(format!("Failed to build schema context: {}", e)))?;

        // Step 5: Build secure translation prompt
        let prompt = self.build_secure_translation_prompt(nl_query, &schema_context, user_context)?;

        // Step 6: Query LLM with fallback
        let llm_response = self.query_llm_with_retry(&prompt, user_context).await?;

        // Step 7: Extract SQL from LLM response
        let raw_sql = self.extract_sql_from_response(&llm_response)?;

        // Step 8: Validate and sanitize SQL
        let validated_sql = self.sql_validator.validate_and_sanitize(&raw_sql, user_context).await
            .map_err(|e| TranslationError::SQLValidationFailed { reason: e.to_string() })?;

        // Step 9: Generate explanation
        let explanation = self.generate_explanation(nl_query, &validated_sql, &llm_response).await?;

        let translation_time_ms = start_time.elapsed().as_millis() as u64;

        // Step 10: Build result
        let result = TranslationResult {
            sql: validated_sql,
            confidence: llm_response.confidence_score.unwrap_or(0.8),
            explanation,
            security_context: user_context.clone(),
            accessible_tables: accessible_tables.clone(),
            translation_metadata: TranslationMetadata {
                original_query: nl_query.to_string(),
                provider_used: llm_response.provider.to_string(),
                translation_time_ms,
                tokens_used: llm_response.tokens_used.total_tokens,
                validation_passed: true,
                safety_checks_passed: true,
                translated_at: Utc::now(),
            },
        };

        info!(
            "NL translation successful for user {}: {}ms, {} tokens, confidence: {:.2}",
            user_context.user_id,
            translation_time_ms,
            llm_response.tokens_used.total_tokens,
            result.confidence
        );

        Ok(result)
    }

    /// Build secure translation prompt with security constraints
    fn build_secure_translation_prompt(
        &self,
        query: &str,
        schema_context: &str,
        user_context: &UserContext,
    ) -> Result<String, TranslationError> {
        let template = PromptTemplate::SecureTranslation;

        let accessible_tables_str = user_context.accessible_tables.iter()
            .map(|s| s.as_str())
            .collect::<Vec<&str>>()
            .join(", ");
        let prompt = self.prompt_builder.build_prompt(template, &[
            ("user_accessible_tables", &accessible_tables_str),
            ("schema_context", schema_context),
            ("natural_language_query", query),
            ("tenant_id", &user_context.tenant_id.clone().unwrap_or_default()),
            ("user_id", &user_context.user_id),
        ]).map_err(|e| TranslationError::ConfigurationError(format!("Failed to build prompt: {}", e)))?;

        debug!("Built secure translation prompt: {} characters", prompt.len());
        Ok(prompt)
    }

    /// Query LLM with retry logic
    async fn query_llm_with_retry(
        &self,
        prompt: &str,
        user_context: &UserContext,
    ) -> Result<LLMResponse, TranslationError> {
        let request = LLMRequest::new(prompt.to_string())
            .with_max_tokens(1000)
            .with_temperature(0.1) // Low temperature for consistent SQL generation
            .with_system_prompt(self.get_system_prompt());

        let context = LLMRequestContext::new(uuid::Uuid::new_v4().to_string())
            .with_user(user_context.user_id.clone())
            .with_tenant(user_context.tenant_id.clone().unwrap_or_default());

        let mut attempts = 0;
        let max_attempts = self.config.max_translation_attempts;

        while attempts < max_attempts {
            attempts += 1;

            match self.llm_engine.query_with_fallback_and_context(&request, &context).await {
                Ok(response) => {
                    debug!("LLM translation successful on attempt {}", attempts);
                    return Ok(response);
                }
                Err(e) => {
                    warn!("LLM translation attempt {} failed: {}", attempts, e);

                    // For certain errors, don't retry
                    match &e {
                        LLMError::InvalidRequest(_) => return Err(TranslationError::LLMProviderError(e)),
                        LLMError::AuthenticationFailed { .. } => return Err(TranslationError::LLMProviderError(e)),
                        _ => {
                            if attempts >= max_attempts {
                                return Err(TranslationError::LLMProviderError(e));
                            }
                            // Wait before retry
                            tokio::time::sleep(std::time::Duration::from_millis(1000 * attempts as u64)).await;
                        }
                    }
                }
            }
        }

        Err(TranslationError::InternalError("Max translation attempts exceeded".to_string()))
    }

    /// Extract SQL from LLM response with multiple parsing strategies
    fn extract_sql_from_response(&self, response: &LLMResponse) -> Result<String, TranslationError> {
        let content = &response.content;

        // Strategy 1: Look for SQL code blocks
        if let Some(sql) = self.extract_from_code_block(content) {
            return Ok(sql);
        }

        // Strategy 2: Look for SQL keywords and extract
        if let Some(sql) = self.extract_by_sql_keywords(content) {
            return Ok(sql);
        }

        // Strategy 3: Use the entire content if it looks like SQL
        if self.looks_like_sql(content) {
            return Ok(content.trim().to_string());
        }

        Err(TranslationError::InternalError(
            format!("Could not extract SQL from LLM response: {}", content.chars().take(200).collect::<String>())
        ))
    }

    /// Extract SQL from markdown code blocks
    fn extract_from_code_block(&self, content: &str) -> Option<String> {
        // Look for ```sql or ```SQL code blocks
        let patterns = ["```sql", "```SQL", "```"];

        for pattern in &patterns {
            if let Some(start) = content.find(pattern) {
                let start_pos = start + pattern.len();
                if let Some(end) = content[start_pos..].find("```") {
                    let sql = content[start_pos..start_pos + end].trim();
                    if !sql.is_empty() && self.looks_like_sql(sql) {
                        return Some(sql.to_string());
                    }
                }
            }
        }

        None
    }

    /// Extract SQL by finding SQL keywords
    fn extract_by_sql_keywords(&self, content: &str) -> Option<String> {
        let sql_keywords = ["SELECT", "WITH", "FROM", "WHERE", "JOIN"];

        for line in content.lines() {
            let line = line.trim();
            if sql_keywords.iter().any(|&keyword| line.to_uppercase().starts_with(keyword)) {
                // Found a line that starts with SQL keyword
                // Try to extract the complete SQL statement
                let mut sql_lines = vec![line];

                // Continue collecting lines until we find a complete statement
                for next_line in content.lines().skip_while(|l| l.trim() != line).skip(1) {
                    let trimmed = next_line.trim();
                    if trimmed.is_empty() && sql_lines.len() > 1 {
                        break; // End of SQL statement
                    }
                    sql_lines.push(trimmed);

                    // Stop if we hit a semicolon
                    if trimmed.ends_with(';') {
                        break;
                    }
                }

                let sql = sql_lines.join(" ");
                if self.looks_like_sql(&sql) {
                    return Some(sql);
                }
            }
        }

        None
    }

    /// Check if text looks like SQL
    fn looks_like_sql(&self, text: &str) -> bool {
        let text_upper = text.to_uppercase();
        let sql_indicators = [
            "SELECT", "FROM", "WHERE", "JOIN", "GROUP BY", "ORDER BY",
            "HAVING", "UNION", "WITH", "INSERT", "UPDATE", "DELETE"
        ];

        sql_indicators.iter().any(|&indicator| text_upper.contains(indicator))
    }

    /// Get system prompt for SQL translation
    fn get_system_prompt(&self) -> String {
        "You are a secure SQL translator for ProximaDB. You MUST follow these rules:

1. Generate only SELECT statements - no INSERT, UPDATE, DELETE, DROP, ALTER, or CREATE
2. Always include proper WHERE clauses for tenant isolation when tenant_id is provided
3. Use only the tables and columns provided in the schema context
4. Generate safe, parameterized queries that prevent SQL injection
5. Include helpful comments explaining the query logic
6. Respond with clean SQL code, optionally in a ```sql code block
7. If the request is unclear or unsafe, explain why and suggest alternatives

Your response should be a valid SQL query that safely retrieves the requested data.".to_string()
    }

    /// Validate user permissions for translation
    fn validate_user_permissions(&self, user_context: &UserContext) -> Result<(), TranslationError> {
        if user_context.user_id.is_empty() {
            return Err(TranslationError::SecurityValidationFailed {
                reason: "User ID is required for query translation".to_string(),
            });
        }

        if user_context.accessible_tables.is_empty() {
            return Err(TranslationError::PermissionDenied {
                user_id: user_context.user_id.clone(),
                reason: "User has no accessible tables".to_string(),
            });
        }

        // Check if user has read permissions
        if !user_context.permissions.iter().any(|p| p.contains("read") || p.contains("query") || p.contains("select")) {
            return Err(TranslationError::PermissionDenied {
                user_id: user_context.user_id.clone(),
                reason: "User lacks query permissions".to_string(),
            });
        }

        Ok(())
    }

    /// Validate query complexity
    fn validate_query_complexity(&self, query: &str) -> Result<(), TranslationError> {
        if query.len() > 5000 {
            return Err(TranslationError::QueryTooComplex {
                reason: "Query exceeds maximum length of 5000 characters".to_string(),
            });
        }

        // Count complexity indicators
        let complexity_score = self.calculate_complexity_score(query);

        if complexity_score > self.config.max_query_complexity {
            return Err(TranslationError::QueryTooComplex {
                reason: format!("Query complexity score {} exceeds maximum {}", complexity_score, self.config.max_query_complexity),
            });
        }

        Ok(())
    }

    /// Calculate query complexity score
    fn calculate_complexity_score(&self, query: &str) -> u32 {
        let mut score = 0;

        // Base score from length
        score += (query.len() / 100) as u32;

        // Add points for complex operations
        let query_lower = query.to_lowercase();

        if query_lower.contains("join") { score += 10; }
        if query_lower.contains("subquery") || query_lower.contains("nested") { score += 15; }
        if query_lower.contains("aggregate") || query_lower.contains("group") { score += 10; }
        if query_lower.contains("order") { score += 5; }
        if query_lower.contains("union") { score += 15; }
        if query_lower.contains("window") || query_lower.contains("over") { score += 20; }

        // Add points for multiple tables
        let table_mentions = query_lower.matches("table").count() +
                            query_lower.matches("from").count() +
                            query_lower.matches("join").count();
        score += (table_mentions * 5) as u32;

        score
    }

    /// Get user accessible tables
    async fn get_user_accessible_tables(&self, user_context: &UserContext) -> Result<Vec<String>, TranslationError> {
        // In a real implementation, this would query the RBAC system
        // For now, return the accessible tables from user context
        Ok(user_context.accessible_tables.clone())
    }

    /// Generate explanation for the translation
    async fn generate_explanation(
        &self,
        original_query: &str,
        sql: &str,
        llm_response: &LLMResponse,
    ) -> Result<String, TranslationError> {
        if !self.config.enable_explanation_generation {
            return Ok("Explanation generation disabled".to_string());
        }

        let explanation_prompt = format!(
            "Explain this SQL query in simple business terms:

Original question: \"{}\"

Generated SQL:
```sql
{}
```

Provide a clear, non-technical explanation of what this query does and what results it will return.",
            original_query, sql
        );

        let explanation_request = LLMRequest::new(explanation_prompt)
            .with_max_tokens(300)
            .with_temperature(0.3);

        let context = LLMRequestContext::new(uuid::Uuid::new_v4().to_string());

        match self.llm_engine.query_with_fallback_and_context(&explanation_request, &context).await {
            Ok(response) => Ok(response.content),
            Err(e) => {
                warn!("Failed to generate explanation: {}", e);
                Ok(format!("This query retrieves data from the database based on: {}", original_query))
            }
        }
    }
}

impl Default for TranslatorConfig {
    fn default() -> Self {
        Self {
            enable_query_caching: true,
            max_query_complexity: 100,
            enable_explanation_generation: true,
            safety_validation_enabled: true,
            tenant_isolation_required: true,
            max_translation_attempts: 3,
        }
    }
}

impl Default for UserContext {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            tenant_id: None,
            accessible_tables: vec![],
            permissions: vec![],
            roles: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complexity_calculation() {
        let translator = create_test_translator();

        // Simple query
        let simple_query = "What are our top customers?";
        assert!(translator.calculate_complexity_score(simple_query) < 20);

        // Complex query
        let complex_query = "Show me the revenue by customer segment with year-over-year growth analysis including subquery joins and window functions";
        assert!(translator.calculate_complexity_score(complex_query) > 50);
    }

    #[test]
    fn test_sql_detection() {
        let translator = create_test_translator();

        assert!(translator.looks_like_sql("SELECT * FROM customers WHERE id = 1"));
        assert!(translator.looks_like_sql("WITH cte AS (SELECT id FROM orders) SELECT * FROM cte"));
        assert!(!translator.looks_like_sql("This is just regular text"));
        assert!(!translator.looks_like_sql("Hello world"));
    }

    #[test]
    fn test_sql_extraction_from_code_block() {
        let translator = create_test_translator();

        let response_with_code_block = r#"
Here's the SQL query you requested:

```sql
SELECT customer_id, COUNT(*) as order_count
FROM orders
WHERE tenant_id = 'tenant_123'
GROUP BY customer_id
ORDER BY order_count DESC
LIMIT 10;
```

This query will return the top 10 customers by order count.
        "#;

        let extracted = translator.extract_from_code_block(response_with_code_block);
        assert!(extracted.is_some());
        assert!(extracted.unwrap().contains("SELECT customer_id"));
    }

    #[test]
    fn test_user_permission_validation() {
        let translator = create_test_translator();

        // Valid user context
        let valid_user = UserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant_1".to_string()),
            accessible_tables: vec!["customers".to_string(), "orders".to_string()],
            permissions: vec!["read_data".to_string()],
            roles: vec!["analyst".to_string()],
        };

        assert!(translator.validate_user_permissions(&valid_user).is_ok());

        // Invalid user context - no permissions
        let invalid_user = UserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant_1".to_string()),
            accessible_tables: vec!["customers".to_string()],
            permissions: vec![], // No permissions
            roles: vec![],
        };

        assert!(translator.validate_user_permissions(&invalid_user).is_err());

        // Invalid user context - no accessible tables
        let no_tables_user = UserContext {
            user_id: "test_user".to_string(),
            tenant_id: Some("tenant_1".to_string()),
            accessible_tables: vec![], // No accessible tables
            permissions: vec!["read_data".to_string()],
            roles: vec!["analyst".to_string()],
        };

        assert!(translator.validate_user_permissions(&no_tables_user).is_err());
    }

    fn create_test_translator() -> NLQueryTranslator {
        // Create a mock translator for testing (without actual LLM dependencies)
        NLQueryTranslator {
            llm_engine: Arc::new(create_mock_llm_engine()),
            schema_context: Arc::new(create_mock_schema_context()),
            sql_validator: Arc::new(create_mock_sql_validator()),
            prompt_builder: Arc::new(PromptBuilder::new()),
            config: TranslatorConfig::default(),
        }
    }

    // Mock implementations for testing
    fn create_mock_llm_engine() -> LLMIntegrationEngine {
        // This would need to be a mock implementation for testing
        todo!("Implement mock LLM engine for testing")
    }

    fn create_mock_schema_context() -> SchemaContext {
        todo!("Implement mock schema context for testing")
    }

    fn create_mock_sql_validator() -> SQLValidator {
        todo!("Implement mock SQL validator for testing")
    }
}