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

//! Per-type validators for ProximaRecord fields
//!
//! Each type has specific validation rules to ensure data integrity and security.
//! This module provides validators for all supported ProximaDB column data types.
//!
//! # Type-Specific Validation
//!
//! - **UUID**: RFC 4122 format validation
//! - **Decimal**: Precision and scale overflow checks
//! - **Binary**: Size limit enforcement
//! - **JSON**: Depth limit and structure validation
//! - **Timestamp**: Range and format validation
//!
//! # Security Features
//!
//! When security mode is enabled, additional checks are performed:
//! - SQL injection pattern detection in text fields
//! - Forbidden pattern matching
//! - Enhanced format validation

use crate::core::types::{ColumnDataType, TypedValue};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

/// Validation result type alias
pub type ValidationResult = Result<(), ValidationError>;

/// Validation error with details
#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    /// Value exceeds maximum length
    #[error("Value exceeds maximum length: {actual} > {max}")]
    LengthExceeded {
        /// Actual length
        actual: usize,
        /// Maximum allowed length
        max: usize,
    },

    /// Value below minimum
    #[error("Value below minimum: {value} < {min}")]
    BelowMinimum {
        /// Actual value
        value: String,
        /// Minimum allowed value
        min: String,
    },

    /// Value above maximum
    #[error("Value above maximum: {value} > {max}")]
    AboveMaximum {
        /// Actual value
        value: String,
        /// Maximum allowed value
        max: String,
    },

    /// Invalid format for type
    #[error("Invalid format for {type_name}: {value}")]
    InvalidFormat {
        /// Type name
        type_name: String,
        /// Offending value
        value: String,
    },

    /// Null value not allowed
    #[error("Null value not allowed for field {field}")]
    NullNotAllowed {
        /// Field name
        field: String,
    },

    /// Pattern validation failed
    #[error("Pattern validation failed: {pattern}")]
    PatternMismatch {
        /// Expected pattern
        pattern: String,
    },

    /// Security violation detected
    #[error("Security violation: {reason}")]
    SecurityViolation {
        /// Violation reason
        reason: String,
    },

    /// Decimal overflow
    #[error("Decimal overflow: precision {precision}, scale {scale}")]
    DecimalOverflow {
        /// Precision
        precision: u8,
        /// Scale
        scale: u8,
    },

    /// JSON depth exceeded
    #[error("JSON depth exceeds limit: {depth} > {max}")]
    JsonDepthExceeded {
        /// Actual depth
        depth: usize,
        /// Maximum allowed depth
        max: usize,
    },

    /// Type mismatch
    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// Expected type
        expected: String,
        /// Actual type
        actual: String,
    },

    /// Invalid UTF-8 sequence
    #[error("Invalid UTF-8 sequence")]
    InvalidUtf8,

    /// Binary size exceeded
    #[error("Binary size exceeds limit: {actual} > {max}")]
    BinarySizeExceeded {
        /// Actual size
        actual: usize,
        /// Maximum allowed size
        max: usize,
    },

    /// Timestamp out of range
    #[error("Timestamp out of range: {timestamp}")]
    TimestampOutOfRange {
        /// Offending timestamp
        timestamp: i64,
    },
}

/// Configuration for field validation
#[derive(Debug, Clone)]
pub struct FieldValidationConfig {
    /// Expected data type
    pub data_type: ColumnDataType,
    /// Whether null values are allowed
    pub nullable: bool,
    /// Maximum length for string/binary types
    pub max_length: Option<usize>,
    /// Minimum value for numeric types
    pub min_value: Option<i128>,
    /// Maximum value for numeric types
    pub max_value: Option<i128>,
    /// Regex pattern for text validation
    pub regex_pattern: Option<String>,
    /// Name of custom validator function
    pub custom_validator: Option<String>,
}

impl Default for FieldValidationConfig {
    fn default() -> Self {
        Self {
            data_type: ColumnDataType::Text,
            nullable: true,
            max_length: None,
            min_value: None,
            max_value: None,
            regex_pattern: None,
            custom_validator: None,
        }
    }
}

impl FieldValidationConfig {
    /// Create a new field validation config for the given data type
    pub fn new(data_type: ColumnDataType) -> Self {
        Self {
            data_type,
            ..Default::default()
        }
    }

    /// Set nullable
    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    /// Set max length
    pub fn with_max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    /// Set min value
    pub fn with_min_value(mut self, min_value: i128) -> Self {
        self.min_value = Some(min_value);
        self
    }

    /// Set max value
    pub fn with_max_value(mut self, max_value: i128) -> Self {
        self.max_value = Some(max_value);
        self
    }

    /// Set regex pattern
    pub fn with_regex_pattern(mut self, pattern: String) -> Self {
        self.regex_pattern = Some(pattern);
        self
    }
}

/// Validates a TypedValue against its expected configuration
pub struct TypedValueValidator {
    /// Field configurations
    configs: HashMap<String, FieldValidationConfig>,
    /// Whether security validation is enabled
    security_enabled: bool,
    /// Compiled regex patterns cache
    regex_cache: HashMap<String, Regex>,
}

impl Default for TypedValueValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypedValueValidator {
    /// Create a new validator
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            security_enabled: true,
            regex_cache: HashMap::new(),
        }
    }

    /// Enable or disable security validation
    pub fn with_security(mut self, enabled: bool) -> Self {
        self.security_enabled = enabled;
        self
    }

    /// Add a field configuration
    pub fn add_field_config(&mut self, field_name: String, config: FieldValidationConfig) {
        // Pre-compile regex if present
        if let Some(ref pattern) = config.regex_pattern
            && let Ok(re) = Regex::new(pattern) {
                self.regex_cache.insert(field_name.clone(), re);
            }
        self.configs.insert(field_name, config);
    }

    /// Validate a single field value
    pub fn validate_field(&self, field_name: &str, value: &TypedValue) -> ValidationResult {
        // Get config if exists, otherwise use default validation
        let config = self.configs.get(field_name);

        // Check null
        if value.is_null() {
            if let Some(cfg) = config
                && !cfg.nullable {
                    return Err(ValidationError::NullNotAllowed {
                        field: field_name.to_string(),
                    });
                }
            return Ok(());
        }

        // Type-specific validation
        match value {
            TypedValue::Null => Ok(()),
            TypedValue::Text(s) => self.validate_text(field_name, s, config),
            TypedValue::Integer(v) => self.validate_integer(field_name, *v, config),
            TypedValue::Float(v) => self.validate_float(field_name, *v, config),
            TypedValue::Decimal {
                value,
                precision,
                scale,
            } => {
                let validator = DecimalValidator::new(*precision, *scale);
                validator.validate(*value)
            }
            TypedValue::Boolean(_) => Ok(()),
            TypedValue::Timestamp(ts) => {
                let validator = TimestampValidator::new();
                validator.validate(*ts)
            }
            TypedValue::TimestampTz { timestamp, .. } => {
                let validator = TimestampValidator::new();
                validator.validate(*timestamp)
            }
            TypedValue::Date(_) => Ok(()),
            TypedValue::Time(_) => Ok(()),
            TypedValue::Duration(_) => Ok(()),
            TypedValue::Interval { .. } => Ok(()),
            TypedValue::Uuid(bytes) => UuidValidator::validate_bytes(bytes),
            TypedValue::Binary(data) => {
                let max_size = config
                    .and_then(|c| c.max_length)
                    .unwrap_or(BinaryValidator::DEFAULT_MAX_SIZE);
                let validator = BinaryValidator::new(max_size);
                validator.validate(data)
            }
            TypedValue::Json(json_str) => {
                let validator = JsonValidator::new();
                validator.validate_string(json_str)
            }
            TypedValue::ArrayText(arr) => self.validate_array_text(field_name, arr, config),
            TypedValue::ArrayInteger(_) => Ok(()),
            TypedValue::ArrayFloat(_) => Ok(()),
            TypedValue::ArrayBoolean(_) => Ok(()),
            TypedValue::ArrayUuid(arr) => {
                for uuid_bytes in arr {
                    UuidValidator::validate_bytes(uuid_bytes)?;
                }
                Ok(())
            }
            TypedValue::MapStringString(map) => self.validate_map_string_string(field_name, map),
            TypedValue::MapStringInteger(_) => Ok(()),
            TypedValue::MapStringFloat(_) => Ok(()),
            TypedValue::GeoPoint {
                latitude,
                longitude,
                ..
            } => self.validate_geo_point(*latitude, *longitude),
            TypedValue::GeoPolygon(points) => {
                for (lat, lon) in points {
                    self.validate_geo_point(*lat, *lon)?;
                }
                Ok(())
            }
            TypedValue::Vector(values) => self.validate_vector(values),
            TypedValue::SparseVector {
                indices, values, ..
            } => self.validate_sparse_vector(indices, values),
        }
    }

    /// Validate all fields in a record
    pub fn validate_record(
        &self,
        fields: &HashMap<String, TypedValue>,
    ) -> Vec<(String, ValidationError)> {
        let mut errors = Vec::new();

        for (field_name, value) in fields {
            if let Err(e) = self.validate_field(field_name, value) {
                errors.push((field_name.clone(), e));
            }
        }

        // Check for required fields that are missing
        for (field_name, config) in &self.configs {
            if !config.nullable && !fields.contains_key(field_name) {
                errors.push((
                    field_name.clone(),
                    ValidationError::NullNotAllowed {
                        field: field_name.clone(),
                    },
                ));
            }
        }

        errors
    }

    /// Validate text field
    fn validate_text(
        &self,
        field_name: &str,
        text: &str,
        config: Option<&FieldValidationConfig>,
    ) -> ValidationResult {
        // Length check
        if let Some(cfg) = config
            && let Some(max_len) = cfg.max_length
                && text.len() > max_len {
                    return Err(ValidationError::LengthExceeded {
                        actual: text.len(),
                        max: max_len,
                    });
                }

        // Regex pattern check
        if let Some(re) = self.regex_cache.get(field_name)
            && !re.is_match(text) {
                return Err(ValidationError::PatternMismatch {
                    pattern: re.to_string(),
                });
            }

        // Security check for SQL injection
        if self.security_enabled && contains_sql_injection(text) {
            return Err(ValidationError::SecurityViolation {
                reason: "Potential SQL injection detected".to_string(),
            });
        }

        Ok(())
    }

    /// Validate integer field
    fn validate_integer(
        &self,
        _field_name: &str,
        value: i64,
        config: Option<&FieldValidationConfig>,
    ) -> ValidationResult {
        if let Some(cfg) = config {
            if let Some(min) = cfg.min_value
                && (value as i128) < min {
                    return Err(ValidationError::BelowMinimum {
                        value: value.to_string(),
                        min: min.to_string(),
                    });
                }
            if let Some(max) = cfg.max_value
                && (value as i128) > max {
                    return Err(ValidationError::AboveMaximum {
                        value: value.to_string(),
                        max: max.to_string(),
                    });
                }
        }
        Ok(())
    }

    /// Validate float field
    fn validate_float(
        &self,
        _field_name: &str,
        value: f64,
        _config: Option<&FieldValidationConfig>,
    ) -> ValidationResult {
        if value.is_nan() {
            return Err(ValidationError::InvalidFormat {
                type_name: "float".to_string(),
                value: "NaN".to_string(),
            });
        }
        if value.is_infinite() {
            return Err(ValidationError::InvalidFormat {
                type_name: "float".to_string(),
                value: "Infinity".to_string(),
            });
        }
        Ok(())
    }

    /// Validate text array
    fn validate_array_text(
        &self,
        field_name: &str,
        arr: &[String],
        config: Option<&FieldValidationConfig>,
    ) -> ValidationResult {
        for text in arr {
            self.validate_text(field_name, text, config)?;
        }
        Ok(())
    }

    /// Validate string-string map
    fn validate_map_string_string(
        &self,
        _field_name: &str,
        map: &HashMap<String, String>,
    ) -> ValidationResult {
        if self.security_enabled {
            for (key, value) in map {
                if contains_sql_injection(key) || contains_sql_injection(value) {
                    return Err(ValidationError::SecurityViolation {
                        reason: "Potential SQL injection detected in map".to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Validate geo point
    fn validate_geo_point(&self, latitude: f64, longitude: f64) -> ValidationResult {
        if latitude < -90.0 || latitude > 90.0 {
            return Err(ValidationError::InvalidFormat {
                type_name: "geo_point".to_string(),
                value: format!("latitude {} out of range [-90, 90]", latitude),
            });
        }
        if longitude < -180.0 || longitude > 180.0 {
            return Err(ValidationError::InvalidFormat {
                type_name: "geo_point".to_string(),
                value: format!("longitude {} out of range [-180, 180]", longitude),
            });
        }
        if latitude.is_nan() || longitude.is_nan() {
            return Err(ValidationError::InvalidFormat {
                type_name: "geo_point".to_string(),
                value: "NaN coordinates not allowed".to_string(),
            });
        }
        Ok(())
    }

    /// Validate vector
    fn validate_vector(&self, values: &[f32]) -> ValidationResult {
        for (i, &v) in values.iter().enumerate() {
            if v.is_nan() {
                return Err(ValidationError::InvalidFormat {
                    type_name: "vector".to_string(),
                    value: format!("NaN at index {}", i),
                });
            }
            if v.is_infinite() {
                return Err(ValidationError::InvalidFormat {
                    type_name: "vector".to_string(),
                    value: format!("Infinity at index {}", i),
                });
            }
        }
        Ok(())
    }

    /// Validate sparse vector
    fn validate_sparse_vector(&self, indices: &[u32], values: &[f32]) -> ValidationResult {
        // Check indices are sorted and unique
        for i in 1..indices.len() {
            if indices[i] <= indices[i - 1] {
                return Err(ValidationError::InvalidFormat {
                    type_name: "sparse_vector".to_string(),
                    value: "Indices must be sorted and unique".to_string(),
                });
            }
        }

        // Validate values
        for (i, &v) in values.iter().enumerate() {
            if v.is_nan() {
                return Err(ValidationError::InvalidFormat {
                    type_name: "sparse_vector".to_string(),
                    value: format!("NaN at index {}", i),
                });
            }
            if v.is_infinite() {
                return Err(ValidationError::InvalidFormat {
                    type_name: "sparse_vector".to_string(),
                    value: format!("Infinity at index {}", i),
                });
            }
        }

        Ok(())
    }
}

/// UUID validator (RFC 4122)
pub struct UuidValidator;

/// Compiled UUID regex pattern
static UUID_REGEX: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$",
    )
    .ok()
});

impl UuidValidator {
    /// Validate UUID string format (RFC 4122)
    pub fn validate(value: &str) -> ValidationResult {
        if let Some(regex) = &*UUID_REGEX
            && regex.is_match(value) {
                return Ok(());
            }

        Err(ValidationError::InvalidFormat {
            type_name: "UUID".to_string(),
            value: value.to_string(),
        })
    }

    /// Validate UUID bytes (must be exactly 16 bytes)
    pub fn validate_bytes(bytes: &[u8]) -> ValidationResult {
        if bytes.len() != 16 {
            return Err(ValidationError::InvalidFormat {
                type_name: "UUID".to_string(),
                value: format!("Expected 16 bytes, got {}", bytes.len()),
            });
        }
        Ok(())
    }
}

/// Decimal validator with precision/scale checks
pub struct DecimalValidator {
    /// Precision (total digits)
    pub precision: u8,
    /// Scale (decimal places)
    pub scale: u8,
}

impl DecimalValidator {
    /// Create a new decimal validator
    pub fn new(precision: u8, scale: u8) -> Self {
        Self { precision, scale }
    }

    /// Validate a decimal value (i128 representation)
    pub fn validate(&self, value: i128) -> ValidationResult {
        // Calculate max absolute value for given precision
        // precision 38 is the max for i128
        let effective_precision = self.precision.min(38);
        let max_value = 10i128
            .checked_pow(effective_precision as u32)
            .map(|v| v - 1)
            .unwrap_or(i128::MAX);

        if value.abs() > max_value {
            return Err(ValidationError::DecimalOverflow {
                precision: self.precision,
                scale: self.scale,
            });
        }
        Ok(())
    }

    /// Validate a decimal string
    pub fn validate_string(&self, value: &str) -> ValidationResult {
        // Parse the string to check format
        let parts: Vec<&str> = value.split('.').collect();
        if parts.len() > 2 {
            return Err(ValidationError::InvalidFormat {
                type_name: "Decimal".to_string(),
                value: value.to_string(),
            });
        }

        // Count total digits
        let integer_digits = parts[0].trim_start_matches('-').len();
        let fractional_digits = parts.get(1).map(|s| s.len()).unwrap_or(0);
        let total_digits = integer_digits + fractional_digits;

        if total_digits > self.precision as usize {
            return Err(ValidationError::DecimalOverflow {
                precision: self.precision,
                scale: self.scale,
            });
        }

        if fractional_digits > self.scale as usize {
            return Err(ValidationError::DecimalOverflow {
                precision: self.precision,
                scale: self.scale,
            });
        }

        Ok(())
    }
}

/// Binary validator with size limits
#[derive(Debug, Clone)]
pub struct BinaryValidator {
    /// Maximum allowed size in bytes
    pub max_size: usize,
}

impl BinaryValidator {
    /// Default maximum size: 1MB
    pub const DEFAULT_MAX_SIZE: usize = 1024 * 1024;

    /// Create a new binary validator with the given max size
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }

    /// Validate binary data
    pub fn validate(&self, data: &[u8]) -> ValidationResult {
        if data.len() > self.max_size {
            return Err(ValidationError::BinarySizeExceeded {
                actual: data.len(),
                max: self.max_size,
            });
        }
        Ok(())
    }
}

impl Default for BinaryValidator {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_SIZE)
    }
}

/// JSON validator with depth limit
#[derive(Debug, Clone)]
pub struct JsonValidator {
    /// Maximum nesting depth
    pub max_depth: usize,
}

impl JsonValidator {
    /// Default maximum depth
    pub const DEFAULT_MAX_DEPTH: usize = 32;

    /// Create a new JSON validator
    pub fn new() -> Self {
        Self {
            max_depth: Self::DEFAULT_MAX_DEPTH,
        }
    }

    /// Create with custom max depth
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self { max_depth }
    }

    /// Validate JSON value
    pub fn validate(&self, json: &serde_json::Value) -> ValidationResult {
        self.check_depth(json, 0)
    }

    /// Validate JSON string
    pub fn validate_string(&self, json_str: &str) -> ValidationResult {
        let value: serde_json::Value =
            serde_json::from_str(json_str).map_err(|_| ValidationError::InvalidFormat {
                type_name: "JSON".to_string(),
                value: "Invalid JSON syntax".to_string(),
            })?;
        self.validate(&value)
    }

    /// Recursively check JSON depth
    fn check_depth(&self, value: &serde_json::Value, current_depth: usize) -> ValidationResult {
        if current_depth > self.max_depth {
            return Err(ValidationError::JsonDepthExceeded {
                depth: current_depth,
                max: self.max_depth,
            });
        }

        match value {
            serde_json::Value::Object(map) => {
                for v in map.values() {
                    self.check_depth(v, current_depth + 1)?;
                }
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    self.check_depth(v, current_depth + 1)?;
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl Default for JsonValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Timestamp validator with range checks
pub struct TimestampValidator {
    /// Minimum allowed timestamp (milliseconds since epoch)
    pub min_timestamp: Option<i64>,
    /// Maximum allowed timestamp (milliseconds since epoch)
    pub max_timestamp: Option<i64>,
    /// Whether future timestamps are allowed
    pub allow_future: bool,
}

impl TimestampValidator {
    /// Create a new timestamp validator with default settings
    pub fn new() -> Self {
        Self {
            min_timestamp: None,
            max_timestamp: None,
            allow_future: true,
        }
    }

    /// Set minimum timestamp
    pub fn with_min_timestamp(mut self, min: i64) -> Self {
        self.min_timestamp = Some(min);
        self
    }

    /// Set maximum timestamp
    pub fn with_max_timestamp(mut self, max: i64) -> Self {
        self.max_timestamp = Some(max);
        self
    }

    /// Set whether future timestamps are allowed
    pub fn with_allow_future(mut self, allow: bool) -> Self {
        self.allow_future = allow;
        self
    }

    /// Validate a timestamp value (milliseconds since epoch)
    pub fn validate(&self, timestamp_ms: i64) -> ValidationResult {
        // Check minimum
        if let Some(min) = self.min_timestamp
            && timestamp_ms < min {
                return Err(ValidationError::TimestampOutOfRange {
                    timestamp: timestamp_ms,
                });
            }

        // Check maximum
        if let Some(max) = self.max_timestamp
            && timestamp_ms > max {
                return Err(ValidationError::TimestampOutOfRange {
                    timestamp: timestamp_ms,
                });
            }

        // Check future timestamps
        if !self.allow_future {
            let now = chrono::Utc::now().timestamp_millis();
            if timestamp_ms > now {
                return Err(ValidationError::TimestampOutOfRange {
                    timestamp: timestamp_ms,
                });
            }
        }

        Ok(())
    }
}

impl Default for TimestampValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// SQL injection patterns for security checking
static SQL_INJECTION_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"(?i)(\b(SELECT|INSERT|UPDATE|DELETE|DROP|UNION|ALTER)\b.*\b(FROM|INTO|SET|TABLE)\b)",
        r"(?i)(\b(OR|AND)\s+['\x220-9]+=\s*['\x220-9]+)",
        r"(?i)(--\s*$|/\*.*\*/)",
        r"(?i)(\bEXEC\s*\(|\bEXECUTE\s*\()",
        r"(?i)(;\s*(DROP|DELETE|UPDATE|INSERT))",
        r"['\x22]\s*;\s*--",
    ]
    .into_iter()
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

/// Check if text contains SQL injection patterns
///
/// This function uses pre-compiled regex patterns to detect common SQL injection
/// attack patterns in text. It checks for:
/// - SQL keywords (SELECT, INSERT, UPDATE, DELETE, DROP, etc.)
/// - Comment sequences (-- or /* */)
/// - OR/AND injection patterns
/// - EXEC/EXECUTE statements
/// - Command chaining with semicolons
///
/// # Arguments
/// * `text` - The text to check for SQL injection patterns
///
/// # Returns
/// * `true` if any SQL injection pattern is detected
/// * `false` if the text appears safe
pub fn contains_sql_injection_pattern(text: &str) -> bool {
    for pattern in SQL_INJECTION_PATTERNS.iter() {
        if pattern.is_match(text) {
            return true;
        }
    }
    false
}

/// Alias for backwards compatibility
fn contains_sql_injection(text: &str) -> bool {
    contains_sql_injection_pattern(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_validator() {
        // Valid UUID
        assert!(UuidValidator::validate("550e8400-e29b-41d4-a716-446655440000").is_ok());

        // Invalid UUIDs
        assert!(UuidValidator::validate("not-a-uuid").is_err());
        assert!(UuidValidator::validate("550e8400-e29b-41d4-a716").is_err());
        assert!(UuidValidator::validate("").is_err());

        // Valid bytes
        assert!(UuidValidator::validate_bytes(&[0u8; 16]).is_ok());

        // Invalid bytes length
        assert!(UuidValidator::validate_bytes(&[0u8; 15]).is_err());
        assert!(UuidValidator::validate_bytes(&[0u8; 17]).is_err());
    }

    #[test]
    fn test_decimal_validator() {
        let validator = DecimalValidator::new(10, 2);

        // Valid values
        assert!(validator.validate(12345678).is_ok());
        assert!(validator.validate_string("123.45").is_ok());
        assert!(validator.validate_string("0.99").is_ok());

        // Invalid: too many decimal places
        assert!(validator.validate_string("1.234").is_err());

        // Invalid: precision overflow
        let small_validator = DecimalValidator::new(3, 1);
        assert!(small_validator.validate_string("1234.5").is_err());
    }

    #[test]
    fn test_binary_validator() {
        let validator = BinaryValidator::new(100);

        // Valid
        assert!(validator.validate(&[0u8; 100]).is_ok());
        assert!(validator.validate(&[0u8; 50]).is_ok());

        // Invalid: too large
        assert!(validator.validate(&[0u8; 101]).is_err());
    }

    #[test]
    fn test_json_validator() {
        let validator = JsonValidator::with_max_depth(3);

        // Valid JSON
        let valid_json: serde_json::Value = serde_json::json!({"a": {"b": "c"}});
        assert!(validator.validate(&valid_json).is_ok());

        // Too deep
        let deep_json: serde_json::Value =
            serde_json::json!({"a": {"b": {"c": {"d": {"e": "f"}}}}});
        assert!(validator.validate(&deep_json).is_err());

        // String validation
        assert!(validator.validate_string(r#"{"key": "value"}"#).is_ok());
        assert!(validator.validate_string("invalid json").is_err());
    }

    #[test]
    fn test_timestamp_validator() {
        let validator = TimestampValidator::new()
            .with_min_timestamp(0)
            .with_max_timestamp(i64::MAX);

        // Valid
        assert!(validator.validate(1000000).is_ok());

        // Invalid: negative
        let min_validator = TimestampValidator::new().with_min_timestamp(0);
        assert!(min_validator.validate(-1).is_err());
    }

    #[test]
    fn test_sql_injection_detection() {
        assert!(contains_sql_injection("SELECT * FROM users"));
        assert!(contains_sql_injection("EXEC("));

        // Safe strings
        assert!(!contains_sql_injection("Hello, World!"));
        assert!(!contains_sql_injection("user@example.com"));
        assert!(!contains_sql_injection("normal text with numbers 123"));
    }

    #[test]
    fn test_typed_value_validator() {
        let mut validator = TypedValueValidator::new().with_security(true);

        // Add config for a field
        validator.add_field_config(
            "name".to_string(),
            FieldValidationConfig::new(ColumnDataType::Text).with_max_length(100),
        );

        // Valid text
        let valid_text = TypedValue::Text("John Doe".to_string());
        assert!(validator.validate_field("name", &valid_text).is_ok());

        // Null handling
        let null_value = TypedValue::Null;
        assert!(validator.validate_field("name", &null_value).is_ok());

        // Required field
        validator.add_field_config(
            "required_field".to_string(),
            FieldValidationConfig::new(ColumnDataType::Text).with_nullable(false),
        );
        assert!(
            validator
                .validate_field("required_field", &TypedValue::Null)
                .is_err()
        );
    }

    #[test]
    fn test_vector_validation() {
        let validator = TypedValueValidator::new();

        // Valid vector
        let valid_vector = TypedValue::Vector(vec![1.0, 2.0, 3.0]);
        assert!(validator.validate_field("embedding", &valid_vector).is_ok());

        // Vector with NaN
        let nan_vector = TypedValue::Vector(vec![1.0, f32::NAN, 3.0]);
        assert!(validator.validate_field("embedding", &nan_vector).is_err());

        // Vector with infinity
        let inf_vector = TypedValue::Vector(vec![1.0, f32::INFINITY, 3.0]);
        assert!(validator.validate_field("embedding", &inf_vector).is_err());
    }

    #[test]
    fn test_geo_point_validation() {
        let validator = TypedValueValidator::new();

        // Valid geo point
        let valid_point = TypedValue::GeoPoint {
            latitude: 37.7749,
            longitude: -122.4194,
            altitude: None,
        };
        assert!(validator.validate_field("location", &valid_point).is_ok());

        // Invalid latitude
        let invalid_lat = TypedValue::GeoPoint {
            latitude: 91.0,
            longitude: 0.0,
            altitude: None,
        };
        assert!(validator.validate_field("location", &invalid_lat).is_err());

        // Invalid longitude
        let invalid_lon = TypedValue::GeoPoint {
            latitude: 0.0,
            longitude: 181.0,
            altitude: None,
        };
        assert!(validator.validate_field("location", &invalid_lon).is_err());
    }

    #[test]
    fn test_record_validation() {
        let mut validator = TypedValueValidator::new();
        validator.add_field_config(
            "name".to_string(),
            FieldValidationConfig::new(ColumnDataType::Text).with_nullable(false),
        );
        validator.add_field_config(
            "age".to_string(),
            FieldValidationConfig::new(ColumnDataType::Integer)
                .with_min_value(0)
                .with_max_value(150),
        );

        let mut record = HashMap::new();
        record.insert("name".to_string(), TypedValue::Text("Alice".to_string()));
        record.insert("age".to_string(), TypedValue::Integer(30));

        let errors = validator.validate_record(&record);
        assert!(errors.is_empty());

        // Missing required field
        let mut incomplete_record = HashMap::new();
        incomplete_record.insert("age".to_string(), TypedValue::Integer(25));

        let errors = validator.validate_record(&incomplete_record);
        assert!(!errors.is_empty());
    }
}
