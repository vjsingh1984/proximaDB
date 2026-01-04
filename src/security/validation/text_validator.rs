/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! TEXT-specific validation with SQL injection prevention
//!
//! Provides comprehensive text field validation including:
//! - Length validation (min/max)
//! - UTF-8 validation
//! - SQL injection pattern detection
//! - Forbidden pattern matching
//! - Allowed pattern enforcement
//!
//! # Security Features
//!
//! The SQL injection detection uses a multi-pattern approach to detect common
//! attack vectors:
//! - Statement injection (SELECT, INSERT, UPDATE, DELETE, DROP, UNION, ALTER)
//! - Boolean-based injection (OR '1'='1')
//! - Comment injection (-- or /* */)
//! - Command execution (EXEC, EXECUTE)
//! - Chained commands (; DROP TABLE)
//!
//! # Usage
//!
//! ```rust,ignore
//! use proximadb::security::validation::TextValidator;
//!
//! // Default validator with SQL injection protection
//! let validator = TextValidator::new();
//! assert!(validator.validate("Hello, World!").is_ok());
//! assert!(validator.validate("'; DROP TABLE users; --").is_err());
//!
//! // Custom configuration
//! let strict_validator = TextValidator::new()
//!     .with_max_length(1000)
//!     .with_min_length(1)
//!     .with_sql_injection_check(true)
//!     .with_forbidden_pattern(r"<script>".to_string());
//!
//! // Builder pattern for common presets
//! let strict = TextValidatorBuilder::strict().build();
//! let permissive = TextValidatorBuilder::permissive().build();
//! ```

use crate::core::types::TextStorageStrategy;
use once_cell::sync::Lazy;
use regex::Regex;

/// Common SQL injection patterns to detect
static SQL_INJECTION_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Statement injection: SELECT/INSERT/UPDATE/DELETE followed by FROM/INTO/SET/TABLE
        Regex::new(
            r"(?i)(\b(SELECT|INSERT|UPDATE|DELETE|DROP|UNION|ALTER)\b.*\b(FROM|INTO|SET|TABLE)\b)",
        )
        .expect("Invalid regex: statement injection"),
        // Boolean-based injection: OR/AND followed by comparison
        Regex::new(r#"(?i)(\b(OR|AND)\s+['"0-9]+=\s*['"0-9]+)"#)
            .expect("Invalid regex: boolean injection"),
        // Comment injection: -- or /* */
        Regex::new(r"(?i)(--\s*$|/\*.*\*/)").expect("Invalid regex: comment injection"),
        // Command execution: EXEC() or EXECUTE()
        Regex::new(r"(?i)(\bEXEC\s*\(|\bEXECUTE\s*\()").expect("Invalid regex: exec injection"),
        // Chained commands: ; followed by dangerous statement
        Regex::new(r"(?i)(;\s*(DROP|DELETE|UPDATE|INSERT))")
            .expect("Invalid regex: chained commands"),
        // Quote-semicolon-comment: '; -- pattern
        Regex::new(r#"['"];\s*--"#).expect("Invalid regex: quote-semicolon-comment"),
        // CHAR encoding bypass
        Regex::new(r"(?i)CHAR\s*\(\s*\d+").expect("Invalid regex: char encoding"),
        // Hex encoding bypass
        Regex::new(r"0x[0-9a-fA-F]+").expect("Invalid regex: hex encoding"),
        // Batch separator
        Regex::new(r"(?i);\s*GO\b").expect("Invalid regex: batch separator"),
        // Information schema access
        Regex::new(r"(?i)INFORMATION_SCHEMA").expect("Invalid regex: information schema"),
        // System table access
        Regex::new(r"(?i)(sysobjects|syscolumns|sysusers)").expect("Invalid regex: system tables"),
        // xp_ stored procedures
        Regex::new(r"(?i)xp_\w+").expect("Invalid regex: xp_ procedures"),
    ]
});

/// TEXT field validator with security checks
#[derive(Debug, Clone)]
pub struct TextValidator {
    /// Maximum allowed length
    pub max_length: usize,
    /// Minimum required length
    pub min_length: usize,
    /// Whether empty strings are allowed
    pub allow_empty: bool,
    /// Whether to validate UTF-8
    pub check_utf8: bool,
    /// Whether to check for SQL injection patterns
    pub check_sql_injection: bool,
    /// Custom forbidden patterns
    pub forbidden_patterns: Vec<String>,
    /// Allowed pattern (if set, text must match)
    pub allowed_pattern: Option<String>,
    /// Compiled forbidden patterns
    #[allow(clippy::type_complexity)]
    compiled_forbidden: Vec<Regex>,
    /// Compiled allowed pattern
    compiled_allowed: Option<Regex>,
}

impl Default for TextValidator {
    fn default() -> Self {
        Self {
            max_length: 64 * 1024, // 64KB default
            min_length: 0,
            allow_empty: true,
            check_utf8: true,
            check_sql_injection: true,
            forbidden_patterns: vec![],
            allowed_pattern: None,
            compiled_forbidden: vec![],
            compiled_allowed: None,
        }
    }
}

impl TextValidator {
    /// Create a new text validator with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum length
    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    /// Set minimum length
    pub fn with_min_length(mut self, min: usize) -> Self {
        self.min_length = min;
        self
    }

    /// Set whether empty strings are allowed
    pub fn with_allow_empty(mut self, allow: bool) -> Self {
        self.allow_empty = allow;
        self
    }

    /// Enable or disable UTF-8 checking
    pub fn with_utf8_check(mut self, enabled: bool) -> Self {
        self.check_utf8 = enabled;
        self
    }

    /// Enable or disable SQL injection checking
    pub fn with_sql_injection_check(mut self, enabled: bool) -> Self {
        self.check_sql_injection = enabled;
        self
    }

    /// Add a forbidden pattern
    pub fn with_forbidden_pattern(mut self, pattern: String) -> Self {
        if let Ok(re) = Regex::new(&pattern) {
            self.compiled_forbidden.push(re);
        }
        self.forbidden_patterns.push(pattern);
        self
    }

    /// Set allowed pattern (text must match this pattern)
    pub fn with_allowed_pattern(mut self, pattern: String) -> Self {
        if let Ok(re) = Regex::new(&pattern) {
            self.compiled_allowed = Some(re);
        }
        self.allowed_pattern = Some(pattern);
        self
    }

    /// Validate text content
    pub fn validate(&self, text: &str) -> Result<(), TextValidationError> {
        // Empty check
        if text.is_empty() {
            if !self.allow_empty {
                return Err(TextValidationError::EmptyNotAllowed);
            }
            return Ok(());
        }

        // Length checks
        if text.len() > self.max_length {
            return Err(TextValidationError::TooLong {
                actual: text.len(),
                max: self.max_length,
            });
        }

        if text.len() < self.min_length {
            return Err(TextValidationError::TooShort {
                actual: text.len(),
                min: self.min_length,
            });
        }

        // UTF-8 check (text in Rust is always valid UTF-8, but we check for special chars)
        if self.check_utf8 && !text.is_ascii() {
            // Additional check for control characters
            for ch in text.chars() {
                if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
                    // Allow common whitespace control chars
                    return Err(TextValidationError::InvalidUtf8);
                }
            }
        }

        // SQL injection check
        if self.check_sql_injection && self.check_sql_injection_patterns(text) {
            return Err(TextValidationError::SqlInjectionDetected);
        }

        // Forbidden patterns check
        for (i, re) in self.compiled_forbidden.iter().enumerate() {
            if re.is_match(text) {
                let pattern = self
                    .forbidden_patterns
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                return Err(TextValidationError::ForbiddenPattern { pattern });
            }
        }

        // Allowed pattern check
        if let Some(ref re) = self.compiled_allowed {
            if !re.is_match(text) {
                return Err(TextValidationError::PatternMismatch);
            }
        }

        Ok(())
    }

    /// Check for SQL injection patterns
    fn check_sql_injection_patterns(&self, text: &str) -> bool {
        for pattern in SQL_INJECTION_PATTERNS.iter() {
            if pattern.is_match(text) {
                return true;
            }
        }
        false
    }

    /// Sanitize text by removing dangerous patterns
    ///
    /// This is a best-effort sanitization and should not be relied upon
    /// as the sole defense against injection attacks. Use parameterized
    /// queries and proper escaping instead.
    pub fn sanitize(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Remove SQL comment patterns
        let comment_pattern = Regex::new(r"--.*$|/\*.*?\*/").expect("Invalid comment regex");
        result = comment_pattern.replace_all(&result, "").to_string();

        // Escape single quotes
        result = result.replace('\'', "''");

        // Remove null bytes
        result = result.replace('\0', "");

        // Remove backslash-escaped sequences that could be problematic
        let escape_pattern = Regex::new(r"\\[nrtbfv0]").expect("Invalid escape regex");
        result = escape_pattern.replace_all(&result, " ").to_string();

        result
    }

    /// Check if text is safe (no security issues detected)
    pub fn is_safe(&self, text: &str) -> bool {
        self.validate(text).is_ok()
    }
}

/// Text validation error types
#[derive(Debug, Clone, thiserror::Error)]
pub enum TextValidationError {
    /// Text exceeds maximum length
    #[error("Text exceeds maximum length: {actual} > {max}")]
    TooLong {
        /// Actual text length
        actual: usize,
        /// Maximum allowed length
        max: usize,
    },

    /// Text below minimum length
    #[error("Text below minimum length: {actual} < {min}")]
    TooShort {
        /// Actual text length
        actual: usize,
        /// Minimum required length
        min: usize,
    },

    /// Empty text not allowed
    #[error("Empty text not allowed")]
    EmptyNotAllowed,

    /// Invalid UTF-8 sequence or control characters
    #[error("Invalid UTF-8 sequence or control characters")]
    InvalidUtf8,

    /// Potential SQL injection detected
    #[error("Potential SQL injection detected")]
    SqlInjectionDetected,

    /// Forbidden pattern matched
    #[error("Forbidden pattern matched: {pattern}")]
    ForbiddenPattern {
        /// The matched pattern
        pattern: String,
    },

    /// Pattern validation failed
    #[error("Pattern validation failed")]
    PatternMismatch,
}

/// Builder for TextValidator with common presets
pub struct TextValidatorBuilder {
    /// The validator being built
    validator: TextValidator,
}

impl Default for TextValidatorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextValidatorBuilder {
    /// Create a new builder with default settings
    pub fn new() -> Self {
        Self {
            validator: TextValidator::new(),
        }
    }

    /// Create a strict mode builder with all security checks enabled
    pub fn strict() -> Self {
        Self {
            validator: TextValidator {
                max_length: 4 * 1024, // 4KB for strict mode
                min_length: 1,
                allow_empty: false,
                check_utf8: true,
                check_sql_injection: true,
                forbidden_patterns: vec![
                    r"<script".to_string(),
                    r"javascript:".to_string(),
                    r"data:".to_string(),
                    r"vbscript:".to_string(),
                    r"on\w+=".to_string(),
                ],
                allowed_pattern: None,
                compiled_forbidden: vec![
                    Regex::new(r"<script").expect("Invalid regex"),
                    Regex::new(r"javascript:").expect("Invalid regex"),
                    Regex::new(r"data:").expect("Invalid regex"),
                    Regex::new(r"vbscript:").expect("Invalid regex"),
                    Regex::new(r"on\w+=").expect("Invalid regex"),
                ],
                compiled_allowed: None,
            },
        }
    }

    /// Create a permissive mode builder with minimal validation
    pub fn permissive() -> Self {
        Self {
            validator: TextValidator {
                max_length: 1024 * 1024, // 1MB
                min_length: 0,
                allow_empty: true,
                check_utf8: false,
                check_sql_injection: false,
                forbidden_patterns: vec![],
                allowed_pattern: None,
                compiled_forbidden: vec![],
                compiled_allowed: None,
            },
        }
    }

    /// Set maximum length
    pub fn max_length(mut self, max: usize) -> Self {
        self.validator.max_length = max;
        self
    }

    /// Set minimum length
    pub fn min_length(mut self, min: usize) -> Self {
        self.validator.min_length = min;
        self
    }

    /// Set allow empty
    pub fn allow_empty(mut self, allow: bool) -> Self {
        self.validator.allow_empty = allow;
        self
    }

    /// Set check UTF-8
    pub fn check_utf8(mut self, check: bool) -> Self {
        self.validator.check_utf8 = check;
        self
    }

    /// Set SQL injection check
    pub fn check_sql_injection(mut self, check: bool) -> Self {
        self.validator.check_sql_injection = check;
        self
    }

    /// Add forbidden pattern
    pub fn forbidden_pattern(mut self, pattern: &str) -> Self {
        if let Ok(re) = Regex::new(pattern) {
            self.validator.compiled_forbidden.push(re);
        }
        self.validator.forbidden_patterns.push(pattern.to_string());
        self
    }

    /// Set allowed pattern
    pub fn allowed_pattern(mut self, pattern: &str) -> Self {
        if let Ok(re) = Regex::new(pattern) {
            self.validator.compiled_allowed = Some(re);
        }
        self.validator.allowed_pattern = Some(pattern.to_string());
        self
    }

    /// Build the validator
    pub fn build(self) -> TextValidator {
        self.validator
    }
}

/// Configuration for validating TEXT fields with storage strategy awareness
///
/// This struct allows configuring validation rules based on the intended
/// storage strategy for TEXT data.
#[derive(Debug, Clone)]
pub struct TextStorageValidationConfig {
    /// The storage strategy to use
    pub strategy: TextStorageStrategy,
    /// Base text validator
    pub text_validator: TextValidator,
    /// Maximum inline size (for Inline strategy)
    pub max_inline_size: usize,
    /// Maximum chunk size (for Chunked strategy)
    pub max_chunk_size: usize,
    /// Maximum sidecar size (for Sidecar strategy)
    pub max_sidecar_size: usize,
    /// Whether to validate chunk boundaries for Chunked strategy
    pub validate_chunk_boundaries: bool,
}

impl Default for TextStorageValidationConfig {
    fn default() -> Self {
        Self {
            strategy: TextStorageStrategy::Adaptive,
            text_validator: TextValidator::new(),
            max_inline_size: TextStorageStrategy::INLINE_MAX_SIZE,
            max_chunk_size: 64 * 1024,           // 64KB chunks
            max_sidecar_size: 100 * 1024 * 1024, // 100MB sidecar limit
            validate_chunk_boundaries: true,
        }
    }
}

impl TextStorageValidationConfig {
    /// Create a new configuration with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create configuration for Inline storage strategy
    pub fn inline() -> Self {
        Self {
            strategy: TextStorageStrategy::Inline,
            text_validator: TextValidator::new()
                .with_max_length(TextStorageStrategy::INLINE_MAX_SIZE),
            max_inline_size: TextStorageStrategy::INLINE_MAX_SIZE,
            ..Default::default()
        }
    }

    /// Create configuration for Chunked storage strategy
    pub fn chunked() -> Self {
        Self {
            strategy: TextStorageStrategy::Chunked,
            text_validator: TextValidator::new()
                .with_max_length(TextStorageStrategy::CHUNKED_MAX_SIZE),
            max_chunk_size: 64 * 1024,
            ..Default::default()
        }
    }

    /// Create configuration for Sidecar storage strategy
    pub fn sidecar() -> Self {
        Self {
            strategy: TextStorageStrategy::Sidecar,
            text_validator: TextValidator::new().with_max_length(100 * 1024 * 1024), // 100MB
            max_sidecar_size: 100 * 1024 * 1024,
            ..Default::default()
        }
    }

    /// Set the storage strategy
    pub fn with_strategy(mut self, strategy: TextStorageStrategy) -> Self {
        self.strategy = strategy;
        // Update max_length based on strategy
        match strategy {
            TextStorageStrategy::Inline => {
                self.text_validator.max_length = self.max_inline_size;
            }
            TextStorageStrategy::Chunked => {
                self.text_validator.max_length = TextStorageStrategy::CHUNKED_MAX_SIZE;
            }
            TextStorageStrategy::Sidecar => {
                self.text_validator.max_length = self.max_sidecar_size;
            }
            TextStorageStrategy::Adaptive => {
                // Keep default large limit for adaptive
                self.text_validator.max_length = self.max_sidecar_size;
            }
        }
        self
    }

    /// Set the base text validator
    pub fn with_text_validator(mut self, validator: TextValidator) -> Self {
        self.text_validator = validator;
        self
    }

    /// Set the maximum inline size
    pub fn with_max_inline_size(mut self, size: usize) -> Self {
        self.max_inline_size = size;
        self
    }

    /// Set the maximum chunk size
    pub fn with_max_chunk_size(mut self, size: usize) -> Self {
        self.max_chunk_size = size;
        self
    }

    /// Set the maximum sidecar size
    pub fn with_max_sidecar_size(mut self, size: usize) -> Self {
        self.max_sidecar_size = size;
        self
    }

    /// Enable or disable chunk boundary validation
    pub fn with_chunk_boundary_validation(mut self, enabled: bool) -> Self {
        self.validate_chunk_boundaries = enabled;
        self
    }

    /// Validate text content according to the configured storage strategy
    pub fn validate(&self, text: &str) -> Result<TextStorageValidationResult, TextValidationError> {
        // First, run base text validation
        self.text_validator.validate(text)?;

        let text_len = text.len();

        // Determine effective strategy based on configuration and text size
        let effective_strategy = match self.strategy {
            TextStorageStrategy::Adaptive => TextStorageStrategy::for_size(text_len),
            other => other,
        };

        // Validate against strategy-specific constraints
        match effective_strategy {
            TextStorageStrategy::Inline => {
                if text_len > self.max_inline_size {
                    return Err(TextValidationError::TooLong {
                        actual: text_len,
                        max: self.max_inline_size,
                    });
                }
            }
            TextStorageStrategy::Chunked => {
                if text_len > TextStorageStrategy::CHUNKED_MAX_SIZE {
                    return Err(TextValidationError::TooLong {
                        actual: text_len,
                        max: TextStorageStrategy::CHUNKED_MAX_SIZE,
                    });
                }
            }
            TextStorageStrategy::Sidecar => {
                if text_len > self.max_sidecar_size {
                    return Err(TextValidationError::TooLong {
                        actual: text_len,
                        max: self.max_sidecar_size,
                    });
                }
            }
            TextStorageStrategy::Adaptive => {
                // Already handled above
            }
        }

        // Calculate chunk information for Chunked strategy
        let chunk_count = if effective_strategy == TextStorageStrategy::Chunked {
            (text_len + self.max_chunk_size - 1) / self.max_chunk_size
        } else {
            0
        };

        Ok(TextStorageValidationResult {
            recommended_strategy: effective_strategy,
            text_length: text_len,
            chunk_count,
            is_valid: true,
        })
    }

    /// Validate text and return the recommended storage strategy
    pub fn validate_and_recommend(
        &self,
        text: &str,
    ) -> Result<TextStorageStrategy, TextValidationError> {
        let result = self.validate(text)?;
        Ok(result.recommended_strategy)
    }

    /// Check if text is valid for the configured strategy
    pub fn is_valid(&self, text: &str) -> bool {
        self.validate(text).is_ok()
    }
}

/// Result of text storage validation
#[derive(Debug, Clone)]
pub struct TextStorageValidationResult {
    /// The recommended storage strategy based on validation
    pub recommended_strategy: TextStorageStrategy,
    /// Length of the validated text
    pub text_length: usize,
    /// Number of chunks (if Chunked strategy)
    pub chunk_count: usize,
    /// Whether the text passed validation
    pub is_valid: bool,
}

impl TextStorageValidationResult {
    /// Check if the text should use inline storage
    pub fn should_use_inline(&self) -> bool {
        self.recommended_strategy == TextStorageStrategy::Inline
    }

    /// Check if the text should use chunked storage
    pub fn should_use_chunked(&self) -> bool {
        self.recommended_strategy == TextStorageStrategy::Chunked
    }

    /// Check if the text should use sidecar storage
    pub fn should_use_sidecar(&self) -> bool {
        self.recommended_strategy == TextStorageStrategy::Sidecar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_validator() {
        let validator = TextValidator::new();

        // Normal text should pass
        assert!(validator.validate("Hello, World!").is_ok());
        assert!(validator.validate("user@example.com").is_ok());
        assert!(validator.validate("Some text with numbers 123").is_ok());
    }

    #[test]
    fn test_length_validation() {
        let validator = TextValidator::new().with_max_length(10).with_min_length(2);

        // Valid length
        assert!(validator.validate("hello").is_ok());

        // Too short
        assert!(matches!(
            validator.validate("a"),
            Err(TextValidationError::TooShort { .. })
        ));

        // Too long
        assert!(matches!(
            validator.validate("hello world!"),
            Err(TextValidationError::TooLong { .. })
        ));
    }

    #[test]
    fn test_empty_validation() {
        let allow_empty = TextValidator::new().with_allow_empty(true);
        assert!(allow_empty.validate("").is_ok());

        let disallow_empty = TextValidator::new().with_allow_empty(false);
        assert!(matches!(
            disallow_empty.validate(""),
            Err(TextValidationError::EmptyNotAllowed)
        ));
    }

    #[test]
    fn test_sql_injection_detection() {
        let validator = TextValidator::new().with_sql_injection_check(true);

        // Common SQL injection patterns should fail
        assert!(matches!(
            validator.validate("'; DROP TABLE users; --"),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        assert!(matches!(
            validator.validate("SELECT * FROM users"),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        assert!(matches!(
            validator.validate("1' OR '1'='1"),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        assert!(matches!(
            validator.validate("admin'--"),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        assert!(matches!(
            validator.validate("EXEC("),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        assert!(matches!(
            validator.validate("'; DELETE FROM users"),
            Err(TextValidationError::SqlInjectionDetected)
        ));

        // Safe strings should pass
        assert!(validator.validate("John Doe").is_ok());
        assert!(validator.validate("user@example.com").is_ok());
    }

    #[test]
    fn test_sql_injection_disabled() {
        let validator = TextValidator::new().with_sql_injection_check(false);

        // SQL injection patterns should pass when check is disabled
        assert!(validator.validate("SELECT * FROM users").is_ok());
    }

    #[test]
    fn test_forbidden_patterns() {
        let validator = TextValidator::new()
            .with_forbidden_pattern(r"<script>".to_string())
            .with_forbidden_pattern(r"javascript:".to_string());

        assert!(matches!(
            validator.validate("<script>alert('xss')</script>"),
            Err(TextValidationError::ForbiddenPattern { .. })
        ));

        assert!(matches!(
            validator.validate("javascript:alert(1)"),
            Err(TextValidationError::ForbiddenPattern { .. })
        ));

        assert!(validator.validate("Normal text").is_ok());
    }

    #[test]
    fn test_allowed_pattern() {
        // Email pattern
        let validator = TextValidator::new()
            .with_allowed_pattern(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string());

        assert!(validator.validate("user@example.com").is_ok());
        assert!(matches!(
            validator.validate("not an email"),
            Err(TextValidationError::PatternMismatch)
        ));
    }

    #[test]
    fn test_sanitize() {
        let validator = TextValidator::new();

        // SQL comment removal
        let sanitized = validator.sanitize("value -- comment");
        assert!(!sanitized.contains("--"));

        // Quote escaping
        let sanitized = validator.sanitize("O'Brien");
        assert!(sanitized.contains("''"));

        // Null byte removal
        let sanitized = validator.sanitize("hello\0world");
        assert!(!sanitized.contains('\0'));
    }

    #[test]
    fn test_strict_builder() {
        let validator = TextValidatorBuilder::strict().build();

        // Empty should fail
        assert!(matches!(
            validator.validate(""),
            Err(TextValidationError::EmptyNotAllowed)
        ));

        // XSS patterns should fail
        assert!(matches!(
            validator.validate("<script>alert(1)</script>"),
            Err(TextValidationError::ForbiddenPattern { .. })
        ));

        // SQL injection should fail
        assert!(matches!(
            validator.validate("'; DROP TABLE users;"),
            Err(TextValidationError::SqlInjectionDetected)
        ));
    }

    #[test]
    fn test_permissive_builder() {
        let validator = TextValidatorBuilder::permissive().build();

        // Everything should pass
        assert!(validator.validate("").is_ok());
        assert!(validator.validate("SELECT * FROM users").is_ok());
        assert!(validator.validate("<script>alert(1)</script>").is_ok());
    }

    #[test]
    fn test_builder_chaining() {
        let validator = TextValidatorBuilder::new()
            .max_length(100)
            .min_length(1)
            .allow_empty(false)
            .check_sql_injection(true)
            .forbidden_pattern(r"badword")
            .build();

        assert!(validator.validate("good text").is_ok());
        assert!(matches!(
            validator.validate(""),
            Err(TextValidationError::EmptyNotAllowed)
        ));
        assert!(matches!(
            validator.validate("has badword in it"),
            Err(TextValidationError::ForbiddenPattern { .. })
        ));
    }

    #[test]
    fn test_is_safe() {
        let validator = TextValidator::new();

        assert!(validator.is_safe("Hello, World!"));
        assert!(!validator.is_safe("'; DROP TABLE users; --"));
    }

    #[test]
    fn test_advanced_injection_patterns() {
        let validator = TextValidator::new();

        // CHAR encoding bypass
        assert!(validator.validate("CHAR(65)").is_err());

        // Hex encoding
        assert!(validator.validate("0x48454C4C4F").is_err());

        // Information schema access
        assert!(
            validator
                .validate("SELECT * FROM INFORMATION_SCHEMA.TABLES")
                .is_err()
        );

        // xp_ procedures
        assert!(validator.validate("xp_cmdshell").is_err());
    }

    // Tests for TextStorageValidationConfig

    #[test]
    fn test_storage_validation_config_default() {
        let config = TextStorageValidationConfig::new();
        assert_eq!(config.strategy, TextStorageStrategy::Adaptive);
        assert_eq!(config.max_inline_size, TextStorageStrategy::INLINE_MAX_SIZE);
    }

    #[test]
    fn test_storage_validation_inline_strategy() {
        let config = TextStorageValidationConfig::inline();

        // Small text should be valid
        let small_text = "Hello, World!";
        let result = config.validate(small_text).expect("Should be valid");
        assert!(result.should_use_inline());
        assert_eq!(result.text_length, small_text.len());

        // Text exactly at inline limit should be valid
        let inline_limit_text = "x".repeat(TextStorageStrategy::INLINE_MAX_SIZE);
        assert!(config.validate(&inline_limit_text).is_ok());

        // Text exceeding inline limit should fail
        let too_large_text = "x".repeat(TextStorageStrategy::INLINE_MAX_SIZE + 1);
        assert!(matches!(
            config.validate(&too_large_text),
            Err(TextValidationError::TooLong { .. })
        ));
    }

    #[test]
    fn test_storage_validation_chunked_strategy() {
        let config = TextStorageValidationConfig::chunked();

        // Medium text should be valid
        let medium_text = "x".repeat(10_000); // 10KB
        let result = config.validate(&medium_text).expect("Should be valid");
        assert!(result.should_use_chunked() || result.should_use_inline());
        assert_eq!(result.text_length, 10_000);

        // Check chunk calculation
        let config_with_small_chunks = TextStorageValidationConfig::new()
            .with_strategy(TextStorageStrategy::Chunked)
            .with_max_chunk_size(1000);
        let result = config_with_small_chunks
            .validate(&medium_text)
            .expect("Should be valid");
        assert_eq!(result.chunk_count, 10); // 10000 / 1000 = 10 chunks
    }

    #[test]
    fn test_storage_validation_sidecar_strategy() {
        let config = TextStorageValidationConfig::sidecar();

        // Large text should be valid
        let large_text = "x".repeat(2_000_000); // 2MB
        let result = config.validate(&large_text).expect("Should be valid");
        assert!(result.should_use_sidecar());
        assert_eq!(result.text_length, 2_000_000);
    }

    #[test]
    fn test_storage_validation_adaptive_strategy() {
        let config =
            TextStorageValidationConfig::new().with_strategy(TextStorageStrategy::Adaptive);

        // Small text should recommend Inline
        let small_text = "Hello!";
        let result = config.validate(small_text).expect("Should be valid");
        assert_eq!(result.recommended_strategy, TextStorageStrategy::Inline);

        // Medium text should recommend Chunked
        let medium_text = "x".repeat(10_000); // 10KB (above 4KB inline limit)
        let result = config.validate(&medium_text).expect("Should be valid");
        assert_eq!(result.recommended_strategy, TextStorageStrategy::Chunked);

        // Large text should recommend Sidecar
        let large_text = "x".repeat(2_000_000); // 2MB (above 1MB chunked limit)
        let result = config.validate(&large_text).expect("Should be valid");
        assert_eq!(result.recommended_strategy, TextStorageStrategy::Sidecar);
    }

    #[test]
    fn test_storage_validation_with_sql_injection() {
        let config = TextStorageValidationConfig::new();

        // SQL injection should still be detected
        assert!(matches!(
            config.validate("'; DROP TABLE users; --"),
            Err(TextValidationError::SqlInjectionDetected)
        ));
    }

    #[test]
    fn test_storage_validation_validate_and_recommend() {
        let config = TextStorageValidationConfig::new();

        // Should return Inline for small text
        let strategy = config
            .validate_and_recommend("Hello!")
            .expect("Should be valid");
        assert_eq!(strategy, TextStorageStrategy::Inline);

        // Should return Chunked for medium text
        let medium_text = "x".repeat(10_000);
        let strategy = config
            .validate_and_recommend(&medium_text)
            .expect("Should be valid");
        assert_eq!(strategy, TextStorageStrategy::Chunked);
    }

    #[test]
    fn test_storage_validation_result_helpers() {
        let inline_result = TextStorageValidationResult {
            recommended_strategy: TextStorageStrategy::Inline,
            text_length: 100,
            chunk_count: 0,
            is_valid: true,
        };
        assert!(inline_result.should_use_inline());
        assert!(!inline_result.should_use_chunked());
        assert!(!inline_result.should_use_sidecar());

        let chunked_result = TextStorageValidationResult {
            recommended_strategy: TextStorageStrategy::Chunked,
            text_length: 10_000,
            chunk_count: 10,
            is_valid: true,
        };
        assert!(!chunked_result.should_use_inline());
        assert!(chunked_result.should_use_chunked());
        assert!(!chunked_result.should_use_sidecar());

        let sidecar_result = TextStorageValidationResult {
            recommended_strategy: TextStorageStrategy::Sidecar,
            text_length: 2_000_000,
            chunk_count: 0,
            is_valid: true,
        };
        assert!(!sidecar_result.should_use_inline());
        assert!(!sidecar_result.should_use_chunked());
        assert!(sidecar_result.should_use_sidecar());
    }

    #[test]
    fn test_storage_validation_builder_methods() {
        let config = TextStorageValidationConfig::new()
            .with_strategy(TextStorageStrategy::Inline)
            .with_max_inline_size(2048)
            .with_max_chunk_size(32_000)
            .with_max_sidecar_size(50 * 1024 * 1024)
            .with_chunk_boundary_validation(false);

        assert_eq!(config.strategy, TextStorageStrategy::Inline);
        assert_eq!(config.max_inline_size, 2048);
        assert_eq!(config.max_chunk_size, 32_000);
        assert_eq!(config.max_sidecar_size, 50 * 1024 * 1024);
        assert!(!config.validate_chunk_boundaries);
    }

    #[test]
    fn test_storage_validation_is_valid() {
        let config = TextStorageValidationConfig::inline();

        // Valid text
        assert!(config.is_valid("Hello, World!"));

        // SQL injection should fail
        assert!(!config.is_valid("'; DROP TABLE users; --"));

        // Too long text should fail
        let too_long = "x".repeat(TextStorageStrategy::INLINE_MAX_SIZE + 1);
        assert!(!config.is_valid(&too_long));
    }

    #[test]
    fn test_storage_validation_with_custom_text_validator() {
        let text_validator = TextValidator::new()
            .with_max_length(100)
            .with_sql_injection_check(false);

        let config = TextStorageValidationConfig::new().with_text_validator(text_validator);

        // SQL injection should pass (disabled in text validator)
        assert!(config.validate("SELECT * FROM users").is_ok());

        // Text too long for the custom validator should fail
        let long_text = "x".repeat(150);
        assert!(matches!(
            config.validate(&long_text),
            Err(TextValidationError::TooLong { .. })
        ));
    }

    #[test]
    fn test_storage_validation_empty_text() {
        let config = TextStorageValidationConfig::new();

        // Empty text should be allowed with default settings
        let result = config.validate("").expect("Should be valid");
        assert_eq!(result.text_length, 0);
        assert!(result.should_use_inline());

        // Empty text should fail when not allowed
        let strict_config = TextStorageValidationConfig::new()
            .with_text_validator(TextValidator::new().with_allow_empty(false));
        assert!(matches!(
            strict_config.validate(""),
            Err(TextValidationError::EmptyNotAllowed)
        ));
    }
}
