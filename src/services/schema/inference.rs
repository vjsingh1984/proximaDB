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

//! Schema inference from existing VectorRecord metadata
//!
//! This module provides intelligent schema inference capabilities that analyze
//! existing VectorRecord metadata patterns to determine optimal column types
//! for ProximaRecord storage.
//!
//! ## Inference Process
//!
//! 1. **Sampling**: Extract a representative sample of records
//! 2. **Column Discovery**: Identify all unique column names
//! 3. **Type Inference**: Analyze value patterns for each column
//! 4. **Pattern Detection**: Detect special patterns (UUID, timestamp, etc.)
//! 5. **TEXT Detection**: Identify columns suitable for TEXT storage
//! 6. **Confidence Scoring**: Calculate confidence for each inference
//!
//! ## Type Detection
//!
//! The service detects the following types:
//!
//! | Pattern | Detected Type |
//! |---------|---------------|
//! | Integer values | Integer |
//! | Decimal values | Float or Decimal |
//! | Boolean values | Boolean |
//! | UUID format | Uuid |
//! | ISO timestamp | Timestamp |
//! | Long strings | Text/TextLarge |
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::services::schema::SchemaInferenceService;
//!
//! let service = SchemaInferenceService::new(InferenceConfig::default());
//! let schema = service.infer_schema(&records);
//!
//! // Convert to proto config for collection
//! let proto_config = schema.to_proto_config();
//! ```

use std::collections::HashMap;

use crate::core::types::ColumnDataType;
use crate::proto::proximadb_v1::{
    FilterableDataType, RecordSchemaConfig, SchemaEnforcement, SqlValue, TextStorage,
    TypedColumnConfig, VectorRecord, sql_value::Value as SqlValueVariant,
};

/// Inferred column information
///
/// Contains the inferred type and statistics for a single column.
#[derive(Debug, Clone)]
pub struct InferredColumn {
    /// Column name
    pub name: String,

    /// Inferred data type
    pub data_type: ColumnDataType,

    /// Whether the column can contain null values
    pub nullable: bool,

    /// Number of samples that had this column
    pub sample_count: u64,

    /// Number of null values observed
    pub null_count: u64,

    /// Confidence score for the type inference (0.0 to 1.0)
    pub confidence: f64,
}

impl InferredColumn {
    /// Calculate the null ratio
    pub fn null_ratio(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.null_count as f64 / self.sample_count as f64
    }

    /// Check if this column should be marked as nullable
    pub fn should_be_nullable(&self) -> bool {
        self.nullable || self.null_ratio() > 0.0
    }
}

/// Configuration for schema inference
#[derive(Debug, Clone)]
pub struct InferenceConfig {
    /// Maximum number of records to sample for inference
    ///
    /// Larger samples provide more accurate inference but take longer.
    /// Default: 1000
    pub sample_size: usize,

    /// Minimum confidence threshold for type inference
    ///
    /// Types with confidence below this threshold will fall back to Text.
    /// Default: 0.8
    pub confidence_threshold: f64,

    /// Whether to detect TEXT columns based on content length
    ///
    /// Default: true
    pub detect_text_columns: bool,

    /// Minimum average length to consider a column as TEXT
    ///
    /// Columns with average string length above this are candidates for TEXT.
    /// Default: 256
    pub text_length_threshold: usize,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            sample_size: 1000,
            confidence_threshold: 0.8,
            detect_text_columns: true,
            text_length_threshold: 256,
        }
    }
}

impl InferenceConfig {
    /// Create a new configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the sample size
    pub fn with_sample_size(mut self, size: usize) -> Self {
        self.sample_size = size;
        self
    }

    /// Set the confidence threshold
    pub fn with_confidence_threshold(mut self, threshold: f64) -> Self {
        self.confidence_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Enable or disable TEXT column detection
    pub fn with_detect_text_columns(mut self, detect: bool) -> Self {
        self.detect_text_columns = detect;
        self
    }

    /// Set the TEXT length threshold
    pub fn with_text_length_threshold(mut self, threshold: usize) -> Self {
        self.text_length_threshold = threshold;
        self
    }
}

/// Result of schema inference
#[derive(Debug, Clone)]
pub struct InferredSchema {
    /// Inferred column definitions
    pub columns: Vec<InferredColumn>,

    /// Columns recommended for TEXT storage
    pub text_columns: Vec<String>,

    /// Overall confidence score for the schema
    pub confidence: f64,

    /// Number of records sampled
    pub sample_size: usize,
}

impl InferredSchema {
    /// Convert to RecordSchemaConfig proto
    ///
    /// Creates a proto configuration that can be used when creating
    /// or updating a collection.
    pub fn to_proto_config(&self) -> RecordSchemaConfig {
        let columns = self
            .columns
            .iter()
            .map(|col| TypedColumnConfig {
                name: col.name.clone(),
                data_type: column_data_type_to_proto(&col.data_type) as i32,
                nullable: col.nullable,
                indexed: false,                       // Default to not indexed
                filterable: !col.data_type.is_text(), // Text columns are not filterable by default
                max_length: None,
                min_value: None,
                max_value: None,
                regex_pattern: None,
                default_value: None,
                text_storage: if col.data_type.is_text() {
                    Some(TextStorage::Adaptive as i32)
                } else {
                    None
                },
                fulltext_indexed: if col.data_type.is_text() {
                    Some(false)
                } else {
                    None
                },
            })
            .collect();

        RecordSchemaConfig {
            schema_id: uuid::Uuid::new_v4().to_string(),
            schema_version: "1.0.0".to_string(),
            enforcement: SchemaEnforcement::SchemaFlexible as i32,
            auto_evolve: true,
            columns,
        }
    }

    /// Get columns recommended for TEXT storage
    pub fn recommended_text_columns(&self) -> Vec<&str> {
        self.text_columns.iter().map(|s| s.as_str()).collect()
    }

    /// Get column by name
    pub fn get_column(&self, name: &str) -> Option<&InferredColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Check if a column is inferred as TEXT
    pub fn is_text_column(&self, name: &str) -> bool {
        self.text_columns.contains(&name.to_string())
    }
}

/// Schema inference service
///
/// Analyzes VectorRecord metadata to infer optimal column types
/// for ProximaRecord storage.
pub struct SchemaInferenceService {
    config: InferenceConfig,
}

impl SchemaInferenceService {
    /// Create a new schema inference service
    pub fn new(config: InferenceConfig) -> Self {
        Self { config }
    }

    /// Get the current configuration
    pub fn config(&self) -> &InferenceConfig {
        &self.config
    }

    /// Infer schema from a sample of VectorRecords
    ///
    /// Analyzes the metadata fields across all records to determine
    /// the most appropriate column types.
    ///
    /// # Arguments
    ///
    /// * `records` - Slice of VectorRecords to analyze
    ///
    /// # Returns
    ///
    /// An `InferredSchema` containing column definitions and recommendations.
    pub fn infer_schema(&self, records: &[VectorRecord]) -> InferredSchema {
        // Limit to sample size
        let sample: Vec<_> = records.iter().take(self.config.sample_size).collect();

        if sample.is_empty() {
            return InferredSchema {
                columns: Vec::new(),
                text_columns: Vec::new(),
                confidence: 1.0,
                sample_size: 0,
            };
        }

        // Collect all values per column
        let mut column_values: HashMap<String, Vec<&SqlValue>> = HashMap::new();

        for record in &sample {
            for (key, value) in &record.metadata {
                column_values.entry(key.clone()).or_default().push(value);
            }
        }

        // Infer type for each column
        let mut columns = Vec::new();
        let mut text_columns = Vec::new();

        for (name, values) in column_values {
            let (data_type, confidence) = self.infer_column_type(&values);

            // Check if this should be a TEXT column
            let is_text = if self.config.detect_text_columns && data_type.is_text() {
                self.should_be_text_column(&values)
            } else {
                false
            };

            let final_type = if is_text {
                text_columns.push(name.clone());
                ColumnDataType::TextLarge
            } else {
                data_type
            };

            // Count nulls
            let null_count = values
                .iter()
                .filter(|v| matches!(v.value, Some(SqlValueVariant::NullValue(_)) | None))
                .count() as u64;

            columns.push(InferredColumn {
                name,
                data_type: final_type,
                nullable: null_count > 0,
                sample_count: values.len() as u64,
                null_count,
                confidence,
            });
        }

        // Sort columns by name for consistency
        columns.sort_by(|a, b| a.name.cmp(&b.name));

        // Calculate overall confidence
        let overall_confidence = if columns.is_empty() {
            1.0
        } else {
            columns.iter().map(|c| c.confidence).sum::<f64>() / columns.len() as f64
        };

        InferredSchema {
            columns,
            text_columns,
            confidence: overall_confidence,
            sample_size: sample.len(),
        }
    }

    /// Infer column type from SqlValue samples
    fn infer_column_type(&self, values: &[&SqlValue]) -> (ColumnDataType, f64) {
        if values.is_empty() {
            return (ColumnDataType::Text, 1.0);
        }

        // Count types
        let mut type_counts: HashMap<&str, usize> = HashMap::new();
        let mut string_values: Vec<&str> = Vec::new();

        for value in values {
            match &value.value {
                Some(SqlValueVariant::StringValue(s)) => {
                    *type_counts.entry("string").or_insert(0) += 1;
                    string_values.push(s);
                }
                Some(SqlValueVariant::Int64Value(_)) => {
                    *type_counts.entry("integer").or_insert(0) += 1;
                }
                Some(SqlValueVariant::NumberValue(_)) => {
                    *type_counts.entry("float").or_insert(0) += 1;
                }
                Some(SqlValueVariant::BoolValue(_)) => {
                    *type_counts.entry("boolean").or_insert(0) += 1;
                }
                Some(SqlValueVariant::ArrayValue(_)) => {
                    *type_counts.entry("array").or_insert(0) += 1;
                }
                Some(SqlValueVariant::ObjectValue(_)) => {
                    *type_counts.entry("object").or_insert(0) += 1;
                }
                Some(SqlValueVariant::BytesValue(_)) => {
                    *type_counts.entry("binary").or_insert(0) += 1;
                }
                Some(SqlValueVariant::NullValue(_)) | None => {
                    *type_counts.entry("null").or_insert(0) += 1;
                }
            }
        }

        // Find dominant type (excluding nulls)
        let non_null_count = values.len() - type_counts.get("null").unwrap_or(&0);

        if non_null_count == 0 {
            return (ColumnDataType::Text, 1.0); // All nulls, default to text
        }

        let (dominant_type, count) = type_counts
            .iter()
            .filter(|(k, _)| **k != "null")
            .max_by_key(|(_, v)| *v)
            .map(|(k, v)| (*k, *v))
            .unwrap_or(("string", 0));

        let confidence = count as f64 / non_null_count as f64;

        // For strings, try to detect more specific types
        if dominant_type == "string" && !string_values.is_empty() {
            // Check for UUID pattern
            let uuid_matches = string_values
                .iter()
                .filter(|s| self.is_uuid_pattern(s))
                .count();
            if uuid_matches as f64 / string_values.len() as f64 >= self.config.confidence_threshold
            {
                return (
                    ColumnDataType::Uuid,
                    uuid_matches as f64 / string_values.len() as f64,
                );
            }

            // Check for timestamp pattern (ISO8601 or Unix epoch)
            let timestamp_matches = string_values
                .iter()
                .filter(|s| self.is_timestamp_pattern(s) || self.is_unix_epoch_pattern(s))
                .count();
            if timestamp_matches as f64 / string_values.len() as f64
                >= self.config.confidence_threshold
            {
                return (
                    ColumnDataType::Timestamp,
                    timestamp_matches as f64 / string_values.len() as f64,
                );
            }

            // Check for boolean pattern (true/false, yes/no, 0/1)
            let boolean_matches = string_values
                .iter()
                .filter(|s| self.is_boolean_pattern(s))
                .count();
            if boolean_matches as f64 / string_values.len() as f64
                >= self.config.confidence_threshold
            {
                return (
                    ColumnDataType::Boolean,
                    boolean_matches as f64 / string_values.len() as f64,
                );
            }

            // Check for decimal pattern
            let decimal_matches = string_values
                .iter()
                .filter(|s| self.is_decimal_pattern(s))
                .count();
            if decimal_matches as f64 / string_values.len() as f64
                >= self.config.confidence_threshold
            {
                return (
                    ColumnDataType::Decimal {
                        precision: 38,
                        scale: 18,
                    },
                    decimal_matches as f64 / string_values.len() as f64,
                );
            }

            return (ColumnDataType::Text, confidence);
        }

        // Map dominant type to ColumnDataType
        let data_type = match dominant_type {
            "integer" => ColumnDataType::Integer,
            "float" => ColumnDataType::Float,
            "boolean" => ColumnDataType::Boolean,
            "array" => ColumnDataType::ArrayText, // Default to text array
            "object" => ColumnDataType::Json,
            "binary" => ColumnDataType::Binary,
            _ => ColumnDataType::Text,
        };

        (data_type, confidence)
    }

    /// Detect if a string column should be TEXT type
    fn should_be_text_column(&self, values: &[&SqlValue]) -> bool {
        let string_values: Vec<&str> = values
            .iter()
            .filter_map(|v| match &v.value {
                Some(SqlValueVariant::StringValue(s)) => Some(s.as_str()),
                _ => None,
            })
            .collect();

        if string_values.is_empty() {
            return false;
        }

        // Calculate average length
        let total_len: usize = string_values.iter().map(|s| s.len()).sum();
        let avg_len = total_len / string_values.len();

        avg_len >= self.config.text_length_threshold
    }

    /// Detect UUID pattern
    ///
    /// Matches RFC 4122 UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
    fn is_uuid_pattern(&self, value: &str) -> bool {
        // Simple UUID pattern check
        if value.len() != 36 {
            return false;
        }

        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 5 {
            return false;
        }

        let expected_lengths = [8, 4, 4, 4, 12];
        for (part, expected_len) in parts.iter().zip(expected_lengths.iter()) {
            if part.len() != *expected_len {
                return false;
            }
            if !part.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
        }

        true
    }

    /// Detect ISO timestamp pattern
    ///
    /// Matches common ISO 8601 formats:
    /// - 2024-01-15T10:30:00Z
    /// - 2024-01-15T10:30:00+00:00
    /// - 2024-01-15 10:30:00
    fn is_timestamp_pattern(&self, value: &str) -> bool {
        // Check for ISO 8601 patterns
        if value.len() < 10 {
            return false;
        }

        // Check for date prefix: YYYY-MM-DD
        let date_part = &value[..10];
        if !self.is_date_format(date_part) {
            return false;
        }

        // If just a date, it's a date not timestamp
        if value.len() == 10 {
            return false;
        }

        // Check for time separator
        let separator = value.chars().nth(10);
        if !matches!(separator, Some('T') | Some(' ')) {
            return false;
        }

        true
    }

    /// Check if a string matches date format YYYY-MM-DD
    fn is_date_format(&self, value: &str) -> bool {
        if value.len() != 10 {
            return false;
        }

        let chars: Vec<char> = value.chars().collect();

        // Check dashes
        if chars[4] != '-' || chars[7] != '-' {
            return false;
        }

        // Check digits
        for (i, c) in chars.iter().enumerate() {
            if i == 4 || i == 7 {
                continue;
            }
            if !c.is_ascii_digit() {
                return false;
            }
        }

        true
    }

    /// Detect decimal/financial pattern
    ///
    /// Matches patterns like: 1234.56, -1234.56, 0.00
    fn is_decimal_pattern(&self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }

        let trimmed = value.trim();

        // Handle negative sign
        let value_to_check = if trimmed.starts_with('-') || trimmed.starts_with('+') {
            &trimmed[1..]
        } else {
            trimmed
        };

        if value_to_check.is_empty() {
            return false;
        }

        // Check for decimal point
        let parts: Vec<&str> = value_to_check.split('.').collect();
        if parts.len() != 2 {
            return false;
        }

        // Both parts must be digits
        let integer_part = parts[0];
        let decimal_part = parts[1];

        if integer_part.is_empty() || decimal_part.is_empty() {
            return false;
        }

        integer_part.chars().all(|c| c.is_ascii_digit())
            && decimal_part.chars().all(|c| c.is_ascii_digit())
            && decimal_part.len() >= 2 // At least 2 decimal places suggests financial
    }

    /// Detect boolean pattern in string values
    ///
    /// Matches patterns: "true"/"false" (case-insensitive), "0"/"1", "yes"/"no"
    fn is_boolean_pattern(&self, value: &str) -> bool {
        let lower = value.trim().to_lowercase();
        matches!(
            lower.as_str(),
            "true" | "false" | "0" | "1" | "yes" | "no" | "t" | "f" | "y" | "n"
        )
    }

    /// Detect Unix epoch timestamp
    ///
    /// Matches patterns:
    /// - Seconds since epoch: 1704067200 (10 digits)
    /// - Milliseconds since epoch: 1704067200000 (13 digits)
    /// - Microseconds since epoch: 1704067200000000 (16 digits)
    fn is_unix_epoch_pattern(&self, value: &str) -> bool {
        let trimmed = value.trim();

        // Must be all digits
        if !trimmed.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }

        let len = trimmed.len();

        // Seconds (10 digits), milliseconds (13 digits), or microseconds (16 digits)
        if !matches!(len, 10 | 13 | 16) {
            return false;
        }

        // Parse and check reasonable range (1970 to 2100)
        if let Ok(value) = trimmed.parse::<i64>() {
            let seconds = match len {
                10 => value,
                13 => value / 1000,
                16 => value / 1_000_000,
                _ => return false,
            };

            // Reasonable range: 1970-01-01 to 2100-01-01
            const MIN_TIMESTAMP: i64 = 0;
            const MAX_TIMESTAMP: i64 = 4102444800; // 2100-01-01

            return seconds >= MIN_TIMESTAMP && seconds <= MAX_TIMESTAMP;
        }

        false
    }
}

// =============================================================================
// Standalone Helper Functions
// =============================================================================

/// Detect if a string value represents a timestamp
///
/// Checks for both ISO8601 format and Unix epoch timestamps.
///
/// # Returns
/// - `Some((ColumnDataType, confidence))` if timestamp detected
/// - `None` if not a timestamp
///
/// # Example
/// ```ignore
/// use proximadb::services::schema::detect_timestamp;
///
/// // ISO8601 format
/// let result = detect_timestamp("2024-01-15T10:30:00Z");
/// assert!(matches!(result, Some((ColumnDataType::Timestamp, c)) if c > 0.9));
///
/// // Unix epoch (milliseconds)
/// let result = detect_timestamp("1704067200000");
/// assert!(matches!(result, Some((ColumnDataType::Timestamp, c)) if c > 0.8));
/// ```
pub fn detect_timestamp(value: &str) -> Option<(ColumnDataType, f64)> {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    // Check ISO8601 format first (higher confidence)
    if service.is_timestamp_pattern(value) {
        return Some((ColumnDataType::Timestamp, 0.95));
    }

    // Check Unix epoch format
    if service.is_unix_epoch_pattern(value) {
        // Lower confidence since numbers could be other things
        return Some((ColumnDataType::Timestamp, 0.75));
    }

    None
}

/// Detect if a string value represents a UUID
///
/// Validates against RFC 4122 UUID format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
///
/// # Returns
/// - `Some(confidence)` if UUID detected (confidence 0.0-1.0)
/// - `None` if not a UUID
///
/// # Example
/// ```ignore
/// use proximadb::services::schema::detect_uuid;
///
/// let confidence = detect_uuid("550e8400-e29b-41d4-a716-446655440000");
/// assert!(matches!(confidence, Some(c) if c > 0.9));
///
/// let confidence = detect_uuid("not-a-uuid");
/// assert!(confidence.is_none());
/// ```
pub fn detect_uuid(value: &str) -> Option<f64> {
    let service = SchemaInferenceService::new(InferenceConfig::default());

    if service.is_uuid_pattern(value) {
        // High confidence for exact UUID format match
        Some(0.98)
    } else {
        None
    }
}

/// Detect the numeric type of a string value
///
/// Detects:
/// - Integer: whole numbers like "123", "-456"
/// - Float: decimal numbers like "3.14", "-2.718"
/// - Decimal: financial/precision decimals with 2+ decimal places like "99.99"
///
/// # Returns
/// - `Some((ColumnDataType, confidence))` if numeric type detected
/// - `None` if not a numeric value
///
/// # Example
/// ```ignore
/// use proximadb::services::schema::detect_numeric_type;
/// use proximadb::core::types::ColumnDataType;
///
/// // Integer
/// let result = detect_numeric_type("42");
/// assert!(matches!(result, Some((ColumnDataType::Integer, _))));
///
/// // Decimal (financial)
/// let result = detect_numeric_type("99.99");
/// assert!(matches!(result, Some((ColumnDataType::Decimal { .. }, _))));
///
/// // Float
/// let result = detect_numeric_type("3.14159");
/// assert!(matches!(result, Some((ColumnDataType::Float, _))));
/// ```
pub fn detect_numeric_type(value: &str) -> Option<(ColumnDataType, f64)> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return None;
    }

    let service = SchemaInferenceService::new(InferenceConfig::default());

    // Check for decimal pattern first (financial/precision)
    if service.is_decimal_pattern(trimmed) {
        // Count decimal places to determine precision
        if let Some(dot_pos) = trimmed.rfind('.') {
            let decimal_places = trimmed.len() - dot_pos - 1;
            return Some((
                ColumnDataType::Decimal {
                    precision: 38,
                    scale: decimal_places.min(18) as u8,
                },
                0.9,
            ));
        }
    }

    // Check for float (has decimal point but maybe only 1 decimal place)
    if trimmed.contains('.') {
        let value_to_check = if trimmed.starts_with('-') || trimmed.starts_with('+') {
            &trimmed[1..]
        } else {
            trimmed
        };

        let parts: Vec<&str> = value_to_check.split('.').collect();
        if parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && parts[1].chars().all(|c| c.is_ascii_digit())
        {
            return Some((ColumnDataType::Float, 0.85));
        }
    }

    // Check for integer
    let value_to_check = if trimmed.starts_with('-') || trimmed.starts_with('+') {
        &trimmed[1..]
    } else {
        trimmed
    };

    if !value_to_check.is_empty() && value_to_check.chars().all(|c| c.is_ascii_digit()) {
        return Some((ColumnDataType::Integer, 0.9));
    }

    None
}

/// Detect if a string represents a boolean value
///
/// Matches patterns: "true"/"false" (case-insensitive), "0"/"1", "yes"/"no", "t"/"f", "y"/"n"
///
/// # Returns
/// - `Some(confidence)` if boolean pattern detected
/// - `None` if not a boolean
///
/// # Example
/// ```ignore
/// use proximadb::services::schema::detect_boolean;
///
/// assert!(detect_boolean("true").is_some());
/// assert!(detect_boolean("FALSE").is_some());
/// assert!(detect_boolean("1").is_some());
/// assert!(detect_boolean("yes").is_some());
/// assert!(detect_boolean("maybe").is_none());
/// ```
pub fn detect_boolean(value: &str) -> Option<f64> {
    let lower = value.trim().to_lowercase();
    match lower.as_str() {
        "true" | "false" => Some(0.98),
        "yes" | "no" => Some(0.9),
        "t" | "f" | "y" | "n" => Some(0.85),
        "0" | "1" => Some(0.7), // Lower confidence since could be integer
        _ => None,
    }
}

/// Convert ColumnDataType to FilterableDataType proto enum
fn column_data_type_to_proto(data_type: &ColumnDataType) -> FilterableDataType {
    match data_type {
        ColumnDataType::Text => FilterableDataType::FilterableText,
        ColumnDataType::TextLarge => FilterableDataType::FilterableTextLarge,
        ColumnDataType::Integer => FilterableDataType::FilterableInteger,
        ColumnDataType::Float => FilterableDataType::FilterableFloat,
        ColumnDataType::Decimal { .. } => FilterableDataType::FilterableDecimal,
        ColumnDataType::Boolean => FilterableDataType::FilterableBoolean,
        ColumnDataType::Timestamp => FilterableDataType::FilterableDatetime,
        ColumnDataType::TimestampTz { .. } => FilterableDataType::FilterableTimestampTz,
        ColumnDataType::Date => FilterableDataType::FilterableDate,
        ColumnDataType::Time => FilterableDataType::FilterableTime,
        ColumnDataType::Uuid => FilterableDataType::FilterableUuid,
        ColumnDataType::Binary | ColumnDataType::BinaryLarge => {
            FilterableDataType::FilterableBinary
        }
        ColumnDataType::Json => FilterableDataType::FilterableJson,
        ColumnDataType::ArrayText => FilterableDataType::FilterableArrayString,
        ColumnDataType::ArrayInteger => FilterableDataType::FilterableArrayInteger,
        ColumnDataType::ArrayFloat => FilterableDataType::FilterableArrayFloat,
        _ => FilterableDataType::FilterableString,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_sql_value_string(s: &str) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::StringValue(s.to_string())),
        }
    }

    fn create_sql_value_int(i: i64) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(i)),
        }
    }

    fn create_sql_value_float(f: f64) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::NumberValue(f)),
        }
    }

    fn create_sql_value_bool(b: bool) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::BoolValue(b)),
        }
    }

    fn create_sql_value_null() -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        }
    }

    fn create_test_record(id: &str, metadata: HashMap<String, SqlValue>) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata,
            timestamp: Some(1704067200000),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    #[test]
    fn test_inference_config_default() {
        let config = InferenceConfig::default();
        assert_eq!(config.sample_size, 1000);
        assert_eq!(config.confidence_threshold, 0.8);
        assert!(config.detect_text_columns);
        assert_eq!(config.text_length_threshold, 256);
    }

    #[test]
    fn test_inference_config_builder() {
        let config = InferenceConfig::new()
            .with_sample_size(500)
            .with_confidence_threshold(0.9)
            .with_detect_text_columns(false)
            .with_text_length_threshold(512);

        assert_eq!(config.sample_size, 500);
        assert_eq!(config.confidence_threshold, 0.9);
        assert!(!config.detect_text_columns);
        assert_eq!(config.text_length_threshold, 512);
    }

    #[test]
    fn test_infer_schema_empty() {
        let service = SchemaInferenceService::new(InferenceConfig::default());
        let schema = service.infer_schema(&[]);

        assert!(schema.columns.is_empty());
        assert!(schema.text_columns.is_empty());
        assert_eq!(schema.confidence, 1.0);
        assert_eq!(schema.sample_size, 0);
    }

    #[test]
    fn test_infer_schema_basic_types() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), create_sql_value_string("Alice"));
        metadata.insert("age".to_string(), create_sql_value_int(30));
        metadata.insert("score".to_string(), create_sql_value_float(95.5));
        metadata.insert("active".to_string(), create_sql_value_bool(true));

        let records = vec![create_test_record("1", metadata)];
        let schema = service.infer_schema(&records);

        assert_eq!(schema.columns.len(), 4);

        let name_col = schema.get_column("name").unwrap();
        assert!(matches!(name_col.data_type, ColumnDataType::Text));

        let age_col = schema.get_column("age").unwrap();
        assert!(matches!(age_col.data_type, ColumnDataType::Integer));

        let score_col = schema.get_column("score").unwrap();
        assert!(matches!(score_col.data_type, ColumnDataType::Float));

        let active_col = schema.get_column("active").unwrap();
        assert!(matches!(active_col.data_type, ColumnDataType::Boolean));
    }

    #[test]
    fn test_infer_uuid_pattern() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        assert!(service.is_uuid_pattern("550e8400-e29b-41d4-a716-446655440000"));
        assert!(service.is_uuid_pattern("123e4567-e89b-12d3-a456-426614174000"));

        assert!(!service.is_uuid_pattern("not-a-uuid"));
        assert!(!service.is_uuid_pattern("550e8400-e29b-41d4-a716")); // Too short
        assert!(!service.is_uuid_pattern("550e8400-e29b-41d4-a716-44665544000Z")); // Invalid char
    }

    #[test]
    fn test_infer_timestamp_pattern() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        assert!(service.is_timestamp_pattern("2024-01-15T10:30:00Z"));
        assert!(service.is_timestamp_pattern("2024-01-15T10:30:00+00:00"));
        assert!(service.is_timestamp_pattern("2024-01-15 10:30:00"));

        assert!(!service.is_timestamp_pattern("2024-01-15")); // Date only
        assert!(!service.is_timestamp_pattern("not-a-timestamp"));
        assert!(!service.is_timestamp_pattern("10:30:00")); // Time only
    }

    #[test]
    fn test_infer_decimal_pattern() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        assert!(service.is_decimal_pattern("123.45"));
        assert!(service.is_decimal_pattern("-123.45"));
        assert!(service.is_decimal_pattern("0.00"));
        assert!(service.is_decimal_pattern("+999.99"));

        assert!(!service.is_decimal_pattern("123")); // No decimal
        assert!(!service.is_decimal_pattern("123.4")); // Only 1 decimal place
        assert!(!service.is_decimal_pattern(".45")); // No integer part
        assert!(!service.is_decimal_pattern("123.")); // No decimal part
    }

    #[test]
    fn test_infer_schema_with_nulls() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let mut metadata1 = HashMap::new();
        metadata1.insert("name".to_string(), create_sql_value_string("Alice"));

        let mut metadata2 = HashMap::new();
        metadata2.insert("name".to_string(), create_sql_value_null());

        let mut metadata3 = HashMap::new();
        metadata3.insert("name".to_string(), create_sql_value_string("Bob"));

        let records = vec![
            create_test_record("1", metadata1),
            create_test_record("2", metadata2),
            create_test_record("3", metadata3),
        ];

        let schema = service.infer_schema(&records);
        let name_col = schema.get_column("name").unwrap();

        assert!(name_col.nullable);
        assert_eq!(name_col.null_count, 1);
        assert_eq!(name_col.sample_count, 3);
        assert!((name_col.null_ratio() - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_infer_schema_text_detection() {
        let config = InferenceConfig::new()
            .with_detect_text_columns(true)
            .with_text_length_threshold(50);

        let service = SchemaInferenceService::new(config);

        let long_text = "a".repeat(100);
        let mut metadata = HashMap::new();
        metadata.insert("content".to_string(), create_sql_value_string(&long_text));
        metadata.insert("title".to_string(), create_sql_value_string("Short"));

        let records = vec![create_test_record("1", metadata)];
        let schema = service.infer_schema(&records);

        assert!(schema.is_text_column("content"));
        assert!(!schema.is_text_column("title"));
        assert_eq!(schema.text_columns.len(), 1);
    }

    #[test]
    fn test_infer_schema_uuid_column() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let records: Vec<VectorRecord> = (0..10)
            .map(|i| {
                let mut metadata = HashMap::new();
                let uuid = format!("550e8400-e29b-41d4-a716-44665544{:04}", i);
                metadata.insert("user_id".to_string(), create_sql_value_string(&uuid));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);
        let user_id_col = schema.get_column("user_id").unwrap();

        assert!(matches!(user_id_col.data_type, ColumnDataType::Uuid));
        assert!(user_id_col.confidence >= 0.8);
    }

    #[test]
    fn test_to_proto_config() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), create_sql_value_string("Test"));
        metadata.insert("count".to_string(), create_sql_value_int(42));

        let records = vec![create_test_record("1", metadata)];
        let schema = service.infer_schema(&records);

        let proto_config = schema.to_proto_config();

        assert!(!proto_config.schema_id.is_empty());
        assert_eq!(proto_config.schema_version, "1.0.0");
        assert_eq!(
            proto_config.enforcement,
            SchemaEnforcement::SchemaFlexible as i32
        );
        assert!(proto_config.auto_evolve);
        assert_eq!(proto_config.columns.len(), 2);
    }

    #[test]
    fn test_inferred_column_null_ratio() {
        let col = InferredColumn {
            name: "test".to_string(),
            data_type: ColumnDataType::Text,
            nullable: true,
            sample_count: 100,
            null_count: 25,
            confidence: 0.9,
        };

        assert_eq!(col.null_ratio(), 0.25);
        assert!(col.should_be_nullable());
    }

    #[test]
    fn test_inferred_column_null_ratio_zero_samples() {
        let col = InferredColumn {
            name: "test".to_string(),
            data_type: ColumnDataType::Text,
            nullable: false,
            sample_count: 0,
            null_count: 0,
            confidence: 1.0,
        };

        assert_eq!(col.null_ratio(), 0.0);
    }

    #[test]
    fn test_sample_size_limit() {
        let config = InferenceConfig::new().with_sample_size(5);
        let service = SchemaInferenceService::new(config);

        let records: Vec<VectorRecord> = (0..100)
            .map(|i| {
                let mut metadata = HashMap::new();
                metadata.insert("num".to_string(), create_sql_value_int(i));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);

        assert_eq!(schema.sample_size, 5);
    }

    // =========================================================================
    // Tests for standalone helper functions
    // =========================================================================

    #[test]
    fn test_detect_timestamp_iso8601() {
        // ISO8601 with Z suffix
        let result = super::detect_timestamp("2024-01-15T10:30:00Z");
        assert!(result.is_some());
        let (data_type, confidence) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Timestamp));
        assert!(confidence > 0.9);

        // ISO8601 with timezone offset
        let result = super::detect_timestamp("2024-01-15T10:30:00+00:00");
        assert!(result.is_some());

        // ISO8601 with space separator
        let result = super::detect_timestamp("2024-01-15 10:30:00");
        assert!(result.is_some());

        // Date only should not match timestamp
        let result = super::detect_timestamp("2024-01-15");
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_timestamp_unix_epoch() {
        // Seconds (10 digits)
        let result = super::detect_timestamp("1704067200");
        assert!(result.is_some());
        let (data_type, confidence) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Timestamp));
        assert!(confidence >= 0.7);

        // Milliseconds (13 digits)
        let result = super::detect_timestamp("1704067200000");
        assert!(result.is_some());

        // Microseconds (16 digits)
        let result = super::detect_timestamp("1704067200000000");
        assert!(result.is_some());

        // Invalid: 9 digits
        let result = super::detect_timestamp("123456789");
        assert!(result.is_none());

        // Invalid: timestamp before 1970
        let result = super::detect_timestamp("-1000000000");
        assert!(result.is_none());

        // Invalid: contains letters
        let result = super::detect_timestamp("170406720a");
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_uuid() {
        // Valid UUID
        let result = super::detect_uuid("550e8400-e29b-41d4-a716-446655440000");
        assert!(result.is_some());
        assert!(result.unwrap() > 0.9);

        // Another valid UUID
        let result = super::detect_uuid("123e4567-e89b-12d3-a456-426614174000");
        assert!(result.is_some());

        // Lowercase hex
        let result = super::detect_uuid("a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        assert!(result.is_some());

        // Invalid: too short
        let result = super::detect_uuid("550e8400-e29b-41d4-a716");
        assert!(result.is_none());

        // Invalid: wrong format
        let result = super::detect_uuid("not-a-uuid");
        assert!(result.is_none());

        // Invalid: contains non-hex character
        let result = super::detect_uuid("550e8400-e29b-41d4-a716-44665544000g");
        assert!(result.is_none());

        // Invalid: missing dashes
        let result = super::detect_uuid("550e8400e29b41d4a716446655440000");
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_numeric_type_integer() {
        // Positive integer
        let result = super::detect_numeric_type("42");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Integer));

        // Negative integer
        let result = super::detect_numeric_type("-123");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Integer));

        // Positive with plus sign
        let result = super::detect_numeric_type("+456");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Integer));

        // Zero
        let result = super::detect_numeric_type("0");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Integer));
    }

    #[test]
    fn test_detect_numeric_type_decimal() {
        // Financial decimal (2 places)
        let result = super::detect_numeric_type("99.99");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(
            data_type,
            ColumnDataType::Decimal {
                precision: 38,
                scale: 2
            }
        ));

        // More precision
        let result = super::detect_numeric_type("123.456");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(
            data_type,
            ColumnDataType::Decimal {
                precision: 38,
                scale: 3
            }
        ));

        // Negative decimal
        let result = super::detect_numeric_type("-50.00");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Decimal { .. }));

        // Zero decimal
        let result = super::detect_numeric_type("0.00");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Decimal { .. }));
    }

    #[test]
    fn test_detect_numeric_type_float() {
        // Single decimal place (detected as float, not decimal)
        let result = super::detect_numeric_type("3.1");
        assert!(result.is_some());
        let (data_type, _) = result.unwrap();
        assert!(matches!(data_type, ColumnDataType::Float));

        // Scientific notation not supported (returns None)
        let result = super::detect_numeric_type("1.5e10");
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_numeric_type_invalid() {
        // Empty string
        let result = super::detect_numeric_type("");
        assert!(result.is_none());

        // Text
        let result = super::detect_numeric_type("hello");
        assert!(result.is_none());

        // Mixed
        let result = super::detect_numeric_type("12abc");
        assert!(result.is_none());

        // Multiple decimals
        let result = super::detect_numeric_type("1.2.3");
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_boolean() {
        // True/false (case insensitive)
        assert!(super::detect_boolean("true").is_some());
        assert!(super::detect_boolean("TRUE").is_some());
        assert!(super::detect_boolean("True").is_some());
        assert!(super::detect_boolean("false").is_some());
        assert!(super::detect_boolean("FALSE").is_some());

        // Yes/no
        assert!(super::detect_boolean("yes").is_some());
        assert!(super::detect_boolean("YES").is_some());
        assert!(super::detect_boolean("no").is_some());
        assert!(super::detect_boolean("NO").is_some());

        // Single letter abbreviations
        assert!(super::detect_boolean("t").is_some());
        assert!(super::detect_boolean("f").is_some());
        assert!(super::detect_boolean("y").is_some());
        assert!(super::detect_boolean("n").is_some());

        // Numeric booleans
        assert!(super::detect_boolean("0").is_some());
        assert!(super::detect_boolean("1").is_some());

        // Invalid
        assert!(super::detect_boolean("maybe").is_none());
        assert!(super::detect_boolean("2").is_none());
        assert!(super::detect_boolean("").is_none());
    }

    #[test]
    fn test_detect_boolean_confidence_levels() {
        // Highest confidence for true/false
        let conf = super::detect_boolean("true").unwrap();
        assert!(conf >= 0.95);

        // Medium confidence for yes/no
        let conf = super::detect_boolean("yes").unwrap();
        assert!(conf >= 0.85 && conf < 0.95);

        // Lower confidence for 0/1 (could be integer)
        let conf = super::detect_boolean("1").unwrap();
        assert!(conf < 0.8);
    }

    #[test]
    fn test_infer_boolean_from_string_column() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let records: Vec<VectorRecord> = ["true", "false", "true", "false", "true"]
            .iter()
            .enumerate()
            .map(|(i, val)| {
                let mut metadata = HashMap::new();
                metadata.insert("is_active".to_string(), create_sql_value_string(val));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);
        let is_active_col = schema.get_column("is_active").unwrap();

        assert!(matches!(is_active_col.data_type, ColumnDataType::Boolean));
        assert!(is_active_col.confidence >= 0.8);
    }

    #[test]
    fn test_infer_unix_epoch_timestamp_column() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // Millisecond timestamps
        let timestamps = [
            "1704067200000",
            "1704153600000",
            "1704240000000",
            "1704326400000",
            "1704412800000",
        ];

        let records: Vec<VectorRecord> = timestamps
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let mut metadata = HashMap::new();
                metadata.insert("created_at".to_string(), create_sql_value_string(ts));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);
        let created_at_col = schema.get_column("created_at").unwrap();

        assert!(matches!(
            created_at_col.data_type,
            ColumnDataType::Timestamp
        ));
        assert!(created_at_col.confidence >= 0.8);
    }

    #[test]
    fn test_infer_timestamp_column_iso8601() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let timestamps = [
            "2024-01-15T10:30:00Z",
            "2024-01-16T11:30:00Z",
            "2024-01-17T12:30:00Z",
            "2024-01-18T13:30:00Z",
            "2024-01-19T14:30:00Z",
        ];

        let records: Vec<VectorRecord> = timestamps
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let mut metadata = HashMap::new();
                metadata.insert("event_time".to_string(), create_sql_value_string(ts));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);
        let event_time_col = schema.get_column("event_time").unwrap();

        assert!(matches!(
            event_time_col.data_type,
            ColumnDataType::Timestamp
        ));
        assert!(event_time_col.confidence >= 0.8);
    }

    #[test]
    fn test_infer_decimal_column() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        let prices = ["99.99", "149.50", "24.95", "199.00", "49.99"];

        let records: Vec<VectorRecord> = prices
            .iter()
            .enumerate()
            .map(|(i, price)| {
                let mut metadata = HashMap::new();
                metadata.insert("price".to_string(), create_sql_value_string(price));
                create_test_record(&format!("{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);
        let price_col = schema.get_column("price").unwrap();

        assert!(matches!(
            price_col.data_type,
            ColumnDataType::Decimal { .. }
        ));
        assert!(price_col.confidence >= 0.8);
    }

    #[test]
    fn test_infer_mixed_types_fallback_to_text() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // Mix of UUIDs and regular strings (below threshold)
        let records: Vec<VectorRecord> = [
            "550e8400-e29b-41d4-a716-446655440000",
            "not-a-uuid",
            "hello world",
            "123e4567-e89b-12d3-a456-426614174000",
            "random text",
        ]
        .iter()
        .enumerate()
        .map(|(i, val)| {
            let mut metadata = HashMap::new();
            metadata.insert("mixed_field".to_string(), create_sql_value_string(val));
            create_test_record(&format!("{}", i), metadata)
        })
        .collect();

        let schema = service.infer_schema(&records);
        let mixed_col = schema.get_column("mixed_field").unwrap();

        // Should fallback to Text since UUID ratio is below threshold
        assert!(matches!(mixed_col.data_type, ColumnDataType::Text));
    }

    #[test]
    fn test_real_world_metadata_sample() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // Simulate real-world metadata from a product catalog
        let records: Vec<VectorRecord> = (0..10)
            .map(|i| {
                let mut metadata = HashMap::new();
                metadata.insert(
                    "product_id".to_string(),
                    create_sql_value_string(&format!("550e8400-e29b-41d4-a716-44665544{:04}", i)),
                );
                metadata.insert(
                    "name".to_string(),
                    create_sql_value_string(&format!("Product {}", i)),
                );
                metadata.insert(
                    "price".to_string(),
                    create_sql_value_string(&format!("{}.99", 10 + i)),
                );
                metadata.insert("stock".to_string(), create_sql_value_int(100 + i));
                metadata.insert(
                    "rating".to_string(),
                    create_sql_value_float(4.0 + (i as f64 * 0.1)),
                );
                metadata.insert("in_stock".to_string(), create_sql_value_bool(i % 2 == 0));
                metadata.insert(
                    "created_at".to_string(),
                    create_sql_value_string(&format!("2024-01-{:02}T10:30:00Z", i + 1)),
                );
                metadata.insert(
                    "description".to_string(),
                    create_sql_value_string(&"A great product with amazing features. ".repeat(20)),
                );
                create_test_record(&format!("rec_{}", i), metadata)
            })
            .collect();

        let schema = service.infer_schema(&records);

        // Verify all columns are correctly inferred
        assert_eq!(schema.columns.len(), 8);

        // UUID column
        let product_id = schema.get_column("product_id").unwrap();
        assert!(matches!(product_id.data_type, ColumnDataType::Uuid));

        // Text column
        let name = schema.get_column("name").unwrap();
        assert!(matches!(name.data_type, ColumnDataType::Text));

        // Decimal column (price with 2 decimal places)
        let price = schema.get_column("price").unwrap();
        assert!(matches!(price.data_type, ColumnDataType::Decimal { .. }));

        // Integer column
        let stock = schema.get_column("stock").unwrap();
        assert!(matches!(stock.data_type, ColumnDataType::Integer));

        // Float column
        let rating = schema.get_column("rating").unwrap();
        assert!(matches!(rating.data_type, ColumnDataType::Float));

        // Boolean column
        let in_stock = schema.get_column("in_stock").unwrap();
        assert!(matches!(in_stock.data_type, ColumnDataType::Boolean));

        // Timestamp column
        let created_at = schema.get_column("created_at").unwrap();
        assert!(matches!(created_at.data_type, ColumnDataType::Timestamp));

        // TEXT_LARGE column (long description)
        let description = schema.get_column("description").unwrap();
        assert!(matches!(description.data_type, ColumnDataType::TextLarge));
        assert!(schema.is_text_column("description"));
    }

    #[test]
    fn test_unix_epoch_pattern_edge_cases() {
        let service = SchemaInferenceService::new(InferenceConfig::default());

        // Valid: epoch 0 (1970-01-01)
        assert!(service.is_unix_epoch_pattern("0000000000"));

        // Valid: current era timestamp
        assert!(service.is_unix_epoch_pattern("1704067200"));

        // Valid: far future but within 2100
        assert!(service.is_unix_epoch_pattern("4102444800"));

        // Invalid: too far in future (beyond 2100)
        assert!(!service.is_unix_epoch_pattern("5000000000"));

        // Invalid: wrong number of digits
        assert!(!service.is_unix_epoch_pattern("12345")); // 5 digits
        assert!(!service.is_unix_epoch_pattern("12345678901234567")); // 17 digits
    }

    #[test]
    fn test_recommended_text_columns() {
        let config = InferenceConfig::new()
            .with_detect_text_columns(true)
            .with_text_length_threshold(50);

        let service = SchemaInferenceService::new(config);

        let long_text = "a".repeat(100);
        let mut metadata = HashMap::new();
        metadata.insert("content".to_string(), create_sql_value_string(&long_text));
        metadata.insert("summary".to_string(), create_sql_value_string(&long_text));
        metadata.insert("title".to_string(), create_sql_value_string("Short Title"));

        let records = vec![create_test_record("1", metadata)];
        let schema = service.infer_schema(&records);

        let text_cols = schema.recommended_text_columns();
        assert_eq!(text_cols.len(), 2);
        assert!(text_cols.contains(&"content"));
        assert!(text_cols.contains(&"summary"));
        assert!(!text_cols.contains(&"title"));
    }
}
