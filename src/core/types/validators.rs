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

//! # Type Validators
//!
//! Per-type validation implementations following the Strategy pattern.
//! Each type has a dedicated validator with specific validation rules.

use anyhow::{Result, anyhow};
use regex::Regex;
use std::sync::Arc;

/// Trait for type validation (Strategy pattern)
pub trait TypeValidator: Send + Sync {
    /// Validate a value
    fn validate(&self, value: &[u8]) -> Result<()>;

    /// Get the type name
    fn type_name(&self) -> &'static str;
}

/// Text validator with configurable constraints
pub struct TextValidator {
    max_length: usize,
    min_length: usize,
    pattern: Option<Regex>,
    forbidden_patterns: Vec<Regex>,
}

impl TextValidator {
    /// Default max length: 64KB
    pub const DEFAULT_MAX_LENGTH: usize = 64 * 1024;

    pub fn new() -> Self {
        Self {
            max_length: Self::DEFAULT_MAX_LENGTH,
            min_length: 0,
            pattern: None,
            forbidden_patterns: Self::default_forbidden_patterns(),
        }
    }

    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    pub fn with_min_length(mut self, min: usize) -> Self {
        self.min_length = min;
        self
    }

    pub fn with_pattern(mut self, pattern: &str) -> Result<Self> {
        self.pattern = Some(Regex::new(pattern)?);
        Ok(self)
    }

    /// Default patterns to detect SQL injection attempts
    fn default_forbidden_patterns() -> Vec<Regex> {
        // Note: These are basic patterns. Production should use parameterized queries.
        vec![
            Regex::new(r"(?i);\s*(drop|delete|truncate|alter)\s+").unwrap(),
            Regex::new(r"(?i)union\s+select").unwrap(),
            Regex::new(r"(?i)--\s*$").unwrap(),
        ]
    }

    /// Validate UTF-8 text
    pub fn validate_text(&self, text: &str) -> Result<()> {
        // Length checks
        if text.len() > self.max_length {
            return Err(anyhow!(
                "Text length {} exceeds maximum {}",
                text.len(),
                self.max_length
            ));
        }

        if text.len() < self.min_length {
            return Err(anyhow!(
                "Text length {} is below minimum {}",
                text.len(),
                self.min_length
            ));
        }

        // Pattern check
        if let Some(ref pattern) = self.pattern {
            if !pattern.is_match(text) {
                return Err(anyhow!("Text does not match required pattern"));
            }
        }

        // Forbidden pattern check
        for forbidden in &self.forbidden_patterns {
            if forbidden.is_match(text) {
                return Err(anyhow!("Text contains forbidden pattern"));
            }
        }

        Ok(())
    }
}

impl Default for TextValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeValidator for TextValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(value).map_err(|_| anyhow!("Invalid UTF-8 sequence"))?;
        self.validate_text(text)
    }

    fn type_name(&self) -> &'static str {
        "TEXT"
    }
}

/// UUID validator (RFC 4122)
pub struct UuidValidator;

impl UuidValidator {
    /// Validate UUID bytes (16 bytes)
    pub fn validate_bytes(bytes: &[u8]) -> Result<()> {
        if bytes.len() != 16 {
            return Err(anyhow!(
                "UUID must be exactly 16 bytes, got {}",
                bytes.len()
            ));
        }
        Ok(())
    }

    /// Validate UUID string format
    pub fn validate_string(s: &str) -> Result<()> {
        // Try parsing as UUID
        uuid::Uuid::parse_str(s).map_err(|e| anyhow!("Invalid UUID format: {}", e))?;
        Ok(())
    }

    /// Parse UUID string to bytes
    pub fn parse_to_bytes(s: &str) -> Result<[u8; 16]> {
        let uuid = uuid::Uuid::parse_str(s).map_err(|e| anyhow!("Invalid UUID format: {}", e))?;
        Ok(*uuid.as_bytes())
    }
}

impl TypeValidator for UuidValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() == 16 {
            Self::validate_bytes(value)
        } else {
            let text =
                std::str::from_utf8(value).map_err(|_| anyhow!("Invalid UTF-8 in UUID string"))?;
            Self::validate_string(text)
        }
    }

    fn type_name(&self) -> &'static str {
        "UUID"
    }
}

/// Decimal validator with precision and scale
pub struct DecimalValidator {
    precision: u8,
    scale: u8,
}

impl DecimalValidator {
    /// Default precision (38 digits, like SQL Server/Oracle)
    pub const DEFAULT_PRECISION: u8 = 38;
    /// Default scale (18 decimal places)
    pub const DEFAULT_SCALE: u8 = 18;

    pub fn new(precision: u8, scale: u8) -> Result<Self> {
        if precision == 0 || precision > 38 {
            return Err(anyhow!("Precision must be between 1 and 38"));
        }
        if scale > precision {
            return Err(anyhow!("Scale cannot exceed precision"));
        }
        Ok(Self { precision, scale })
    }

    pub fn default() -> Self {
        Self {
            precision: Self::DEFAULT_PRECISION,
            scale: Self::DEFAULT_SCALE,
        }
    }

    /// Validate a decimal value (i128 representation)
    pub fn validate_i128(&self, value: i128) -> Result<()> {
        // Calculate max absolute value for given precision
        let max_value = 10i128.pow(self.precision as u32) - 1;
        if value.abs() > max_value {
            return Err(anyhow!(
                "Decimal value {} exceeds precision {}",
                value,
                self.precision
            ));
        }
        Ok(())
    }

    /// Parse decimal string to i128 with scale adjustment
    pub fn parse_string(&self, s: &str) -> Result<i128> {
        // Simple decimal parsing (could use rust_decimal crate for production)
        let parts: Vec<&str> = s.split('.').collect();

        let (integer_part, fractional_part) = match parts.len() {
            1 => (parts[0], ""),
            2 => (parts[0], parts[1]),
            _ => return Err(anyhow!("Invalid decimal format")),
        };

        // Parse integer part
        let int_val: i128 = if integer_part.is_empty() {
            0
        } else {
            integer_part
                .parse()
                .map_err(|_| anyhow!("Invalid integer part"))?
        };

        // Parse fractional part (pad/truncate to scale)
        let frac_val: i128 = if fractional_part.is_empty() {
            0
        } else {
            let adjusted = if fractional_part.len() > self.scale as usize {
                &fractional_part[..self.scale as usize]
            } else {
                fractional_part
            };
            let padded = format!("{:0<width$}", adjusted, width = self.scale as usize);
            padded
                .parse()
                .map_err(|_| anyhow!("Invalid fractional part"))?
        };

        // Combine: int_val * 10^scale + frac_val
        let scale_factor = 10i128.pow(self.scale as u32);
        let result = int_val
            .checked_mul(scale_factor)
            .and_then(|v| v.checked_add(frac_val))
            .ok_or_else(|| anyhow!("Decimal overflow"))?;

        self.validate_i128(result)?;
        Ok(result)
    }
}

impl TypeValidator for DecimalValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() == 16 {
            // Assume i128 bytes (little-endian)
            let arr: [u8; 16] = value
                .try_into()
                .map_err(|_| anyhow!("Invalid decimal byte length"))?;
            let i128_val = i128::from_le_bytes(arr);
            self.validate_i128(i128_val)
        } else {
            // Assume string representation
            let text = std::str::from_utf8(value)
                .map_err(|_| anyhow!("Invalid UTF-8 in decimal string"))?;
            self.parse_string(text)?;
            Ok(())
        }
    }

    fn type_name(&self) -> &'static str {
        "DECIMAL"
    }
}

/// Binary validator with size limits
pub struct BinaryValidator {
    max_size: usize,
}

impl BinaryValidator {
    /// Default max size: 1MB
    pub const DEFAULT_MAX_SIZE: usize = 1024 * 1024;

    pub fn new() -> Self {
        Self {
            max_size: Self::DEFAULT_MAX_SIZE,
        }
    }

    pub fn with_max_size(mut self, max: usize) -> Self {
        self.max_size = max;
        self
    }
}

impl Default for BinaryValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeValidator for BinaryValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() > self.max_size {
            return Err(anyhow!(
                "Binary size {} exceeds maximum {}",
                value.len(),
                self.max_size
            ));
        }
        Ok(())
    }

    fn type_name(&self) -> &'static str {
        "BINARY"
    }
}

/// JSON validator with depth limit
pub struct JsonValidator {
    max_depth: usize,
    max_size: usize,
}

impl JsonValidator {
    pub const DEFAULT_MAX_DEPTH: usize = 32;
    pub const DEFAULT_MAX_SIZE: usize = 16 * 1024 * 1024; // 16MB

    pub fn new() -> Self {
        Self {
            max_depth: Self::DEFAULT_MAX_DEPTH,
            max_size: Self::DEFAULT_MAX_SIZE,
        }
    }

    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    pub fn with_max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    /// Validate JSON string
    pub fn validate_json(&self, json_str: &str) -> Result<()> {
        if json_str.len() > self.max_size {
            return Err(anyhow!("JSON size exceeds maximum"));
        }

        // Parse to check validity
        let value: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| anyhow!("Invalid JSON: {}", e))?;

        // Check depth
        let depth = self.calculate_depth(&value);
        if depth > self.max_depth {
            return Err(anyhow!(
                "JSON depth {} exceeds maximum {}",
                depth,
                self.max_depth
            ));
        }

        Ok(())
    }

    fn calculate_depth(&self, value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                1 + map
                    .values()
                    .map(|v| self.calculate_depth(v))
                    .max()
                    .unwrap_or(0)
            }
            serde_json::Value::Array(arr) => {
                1 + arr
                    .iter()
                    .map(|v| self.calculate_depth(v))
                    .max()
                    .unwrap_or(0)
            }
            _ => 1,
        }
    }
}

impl Default for JsonValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeValidator for JsonValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(value).map_err(|_| anyhow!("Invalid UTF-8 in JSON"))?;
        self.validate_json(text)
    }

    fn type_name(&self) -> &'static str {
        "JSON"
    }
}

/// Timestamp validator
pub struct TimestampValidator {
    min_value: i64,
    max_value: i64,
}

impl TimestampValidator {
    /// Minimum timestamp: 1970-01-01 (0)
    pub const MIN_TIMESTAMP: i64 = 0;
    /// Maximum timestamp: ~292,277 AD in microseconds
    pub const MAX_TIMESTAMP: i64 = i64::MAX;

    pub fn new() -> Self {
        Self {
            min_value: Self::MIN_TIMESTAMP,
            max_value: Self::MAX_TIMESTAMP,
        }
    }

    pub fn with_range(mut self, min: i64, max: i64) -> Self {
        self.min_value = min;
        self.max_value = max;
        self
    }

    pub fn validate_microseconds(&self, ts: i64) -> Result<()> {
        if ts < self.min_value {
            return Err(anyhow!("Timestamp {ts} is before minimum"));
        }
        if ts > self.max_value {
            return Err(anyhow!("Timestamp {ts} exceeds maximum"));
        }
        Ok(())
    }
}

impl Default for TimestampValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeValidator for TimestampValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() != 8 {
            return Err(anyhow!("Timestamp must be 8 bytes"));
        }
        let arr: [u8; 8] = value
            .try_into()
            .map_err(|_| anyhow!("Failed to convert bytes to array"))?;
        let ts = i64::from_le_bytes(arr);
        self.validate_microseconds(ts)
    }

    fn type_name(&self) -> &'static str {
        "TIMESTAMP"
    }
}

/// GeoPoint validator
pub struct GeoPointValidator;

impl GeoPointValidator {
    /// Validate latitude (-90 to 90)
    pub fn validate_latitude(lat: f64) -> Result<()> {
        if !(-90.0..=90.0).contains(&lat) {
            return Err(anyhow!("Latitude {lat} must be between -90 and 90"));
        }
        if lat.is_nan() || lat.is_infinite() {
            return Err(anyhow!("Latitude must be a finite number"));
        }
        Ok(())
    }

    /// Validate longitude (-180 to 180)
    pub fn validate_longitude(lon: f64) -> Result<()> {
        if !(-180.0..=180.0).contains(&lon) {
            return Err(anyhow!("Longitude {lon} must be between -180 and 180"));
        }
        if lon.is_nan() || lon.is_infinite() {
            return Err(anyhow!("Longitude must be a finite number"));
        }
        Ok(())
    }

    /// Validate a geo point
    pub fn validate_point(lat: f64, lon: f64) -> Result<()> {
        Self::validate_latitude(lat)?;
        Self::validate_longitude(lon)?;
        Ok(())
    }
}

impl TypeValidator for GeoPointValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() != 16 && value.len() != 24 {
            return Err(anyhow!("GeoPoint must be 16 or 24 bytes"));
        }
        let lat = f64::from_le_bytes(
            value[0..8]
                .try_into()
                .map_err(|_| anyhow!("Failed to convert latitude bytes"))?,
        );
        let lon = f64::from_le_bytes(
            value[8..16]
                .try_into()
                .map_err(|_| anyhow!("Failed to convert longitude bytes"))?,
        );
        Self::validate_point(lat, lon)
    }

    fn type_name(&self) -> &'static str {
        "GEO_POINT"
    }
}

/// Vector dimension validator
pub struct VectorValidator {
    /// Expected vector dimension
    expected_dimension: u32,
}

impl VectorValidator {
    pub fn new(dimension: u32) -> Self {
        Self {
            expected_dimension: dimension,
        }
    }

    pub fn validate_dimension(&self, actual_dimension: usize) -> Result<()> {
        if actual_dimension != self.expected_dimension as usize {
            return Err(anyhow!(
                "Vector dimension {} does not match expected {}",
                actual_dimension,
                self.expected_dimension
            ));
        }
        Ok(())
    }

    pub fn validate_values(&self, values: &[f32]) -> Result<()> {
        for (i, &v) in values.iter().enumerate() {
            if v.is_nan() {
                return Err(anyhow!("Vector contains NaN at index {i}"));
            }
            if v.is_infinite() {
                return Err(anyhow!("Vector contains infinite value at index {i}"));
            }
        }
        Ok(())
    }
}

impl TypeValidator for VectorValidator {
    fn validate(&self, value: &[u8]) -> Result<()> {
        if value.len() % 4 != 0 {
            return Err(anyhow!("Vector byte length must be multiple of 4"));
        }
        let dimension = value.len() / 4;
        self.validate_dimension(dimension)?;

        // Validate individual values
        for i in 0..dimension {
            let bytes: [u8; 4] = value[i * 4..(i + 1) * 4]
                .try_into()
                .map_err(|_| anyhow!("Failed to convert vector bytes at index {i}"))?;
            let v = f32::from_le_bytes(bytes);
            if v.is_nan() {
                return Err(anyhow!("Vector contains NaN at index {i}"));
            }
            if v.is_infinite() {
                return Err(anyhow!("Vector contains infinite value at index {i}"));
            }
        }

        Ok(())
    }

    fn type_name(&self) -> &'static str {
        "VECTOR"
    }
}

/// Validator registry for efficient lookup
pub struct ValidatorRegistry {
    /// Map of validator name to validator instance
    validators: std::collections::HashMap<String, Arc<dyn TypeValidator>>,
}

impl ValidatorRegistry {
    pub fn new() -> Self {
        Self {
            validators: std::collections::HashMap::new(),
        }
    }

    pub fn register<V: TypeValidator + 'static>(&mut self, name: &str, validator: V) {
        self.validators
            .insert(name.to_string(), Arc::new(validator));
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn TypeValidator>> {
        self.validators.get(name)
    }

    /// Create a registry with default validators
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register("TEXT", TextValidator::new());
        registry.register("UUID", UuidValidator);
        registry.register("BINARY", BinaryValidator::new());
        registry.register("JSON", JsonValidator::new());
        registry.register("TIMESTAMP", TimestampValidator::new());
        registry.register("GEO_POINT", GeoPointValidator);
        registry.register("DECIMAL", DecimalValidator::default());
        registry
    }
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_validator() {
        let validator = TextValidator::new().with_max_length(10).with_min_length(2);

        assert!(validator.validate_text("hello").is_ok());
        assert!(validator.validate_text("a").is_err()); // too short
        assert!(validator.validate_text("hello world!").is_err()); // too long
    }

    #[test]
    fn test_uuid_validator() {
        // Valid UUID string
        assert!(UuidValidator::validate_string("550e8400-e29b-41d4-a716-446655440000").is_ok());

        // Invalid UUID string
        assert!(UuidValidator::validate_string("not-a-uuid").is_err());

        // Valid UUID bytes
        assert!(UuidValidator::validate_bytes(&[0u8; 16]).is_ok());

        // Invalid UUID bytes length
        assert!(UuidValidator::validate_bytes(&[0u8; 15]).is_err());
    }

    #[test]
    fn test_decimal_validator() {
        let validator = DecimalValidator::new(10, 2).unwrap();

        // Parse valid decimal
        assert!(validator.parse_string("123.45").is_ok());
        assert!(validator.parse_string("0.01").is_ok());

        // Precision overflow
        let high_precision = DecimalValidator::new(5, 2).unwrap();
        assert!(high_precision.parse_string("99999.99").is_err()); // Would overflow
    }

    #[test]
    fn test_json_validator() {
        let validator = JsonValidator::new().with_max_depth(3);

        // Valid JSON
        assert!(validator.validate_json(r#"{"key": "value"}"#).is_ok());

        // Invalid JSON
        assert!(validator.validate_json(r#"{"key": broken}"#).is_err());

        // Too deep
        assert!(
            validator
                .validate_json(r#"{"a": {"b": {"c": {"d": "too deep"}}}}"#)
                .is_err()
        );
    }

    #[test]
    fn test_geo_point_validator() {
        // Valid coordinates
        assert!(GeoPointValidator::validate_point(37.7749, -122.4194).is_ok()); // San Francisco

        // Invalid latitude
        assert!(GeoPointValidator::validate_latitude(91.0).is_err());
        assert!(GeoPointValidator::validate_latitude(-91.0).is_err());

        // Invalid longitude
        assert!(GeoPointValidator::validate_longitude(181.0).is_err());
        assert!(GeoPointValidator::validate_longitude(-181.0).is_err());
    }

    #[test]
    fn test_vector_validator() {
        let validator = VectorValidator::new(3);

        // Valid vector
        let valid: Vec<u8> = [1.0f32, 2.0, 3.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert!(validator.validate(&valid).is_ok());

        // Wrong dimension
        let wrong_dim: Vec<u8> = [1.0f32, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert!(validator.validate(&wrong_dim).is_err());

        // Contains NaN
        let with_nan: Vec<u8> = [1.0f32, f32::NAN, 3.0]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert!(validator.validate(&with_nan).is_err());
    }

    #[test]
    fn test_validator_registry() {
        let registry = ValidatorRegistry::with_defaults();

        assert!(registry.get("TEXT").is_some());
        assert!(registry.get("UUID").is_some());
        assert!(registry.get("NONEXISTENT").is_none());
    }
}
