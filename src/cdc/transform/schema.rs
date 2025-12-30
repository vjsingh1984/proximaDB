/*
 * Copyright 2025 Vijaykumar Singh
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

//! Schema mapping and field transformations for CDC events

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, RecordState};

/// Schema mapper for transforming CDC event fields
#[derive(Debug, Clone, Default)]
pub struct SchemaMapper {
    /// Field mappings (old_name -> new_name)
    field_mappings: HashMap<String, FieldMapping>,
    /// Fields to drop
    drop_fields: Vec<String>,
    /// Fields to keep (if set, only these fields are kept)
    keep_fields: Option<Vec<String>>,
    /// Collection name mapping
    collection_mapping: Option<String>,
    /// Default values for missing fields
    defaults: HashMap<String, serde_json::Value>,
}

/// Mapping configuration for a single field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMapping {
    /// Target field name
    pub target: String,
    /// Optional transformation to apply
    pub transform: Option<FieldTransform>,
}

/// Field transformation types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldTransform {
    /// Convert to string
    ToString,
    /// Convert to number
    ToNumber,
    /// Convert to boolean
    ToBool,
    /// Parse as JSON
    ParseJson,
    /// Apply regex replacement
    Regex { pattern: String, replacement: String },
    /// Extract substring
    Substring { start: usize, length: Option<usize> },
    /// Convert case
    Case(CaseTransform),
    /// Hash the value
    Hash(HashAlgorithm),
    /// Concatenate multiple fields
    Concat { fields: Vec<String>, separator: String },
    /// Extract from JSON path
    JsonPath(String),
    /// Custom expression (for future use)
    Expression(String),
}

/// Case transformation options
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseTransform {
    /// Convert to uppercase
    Upper,
    /// Convert to lowercase
    Lower,
    /// Convert to title case
    Title,
    /// Convert to snake_case
    Snake,
    /// Convert to camelCase
    Camel,
}

/// Hash algorithm options
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    /// MD5 hash
    Md5,
    /// SHA-256 hash
    Sha256,
    /// SHA-512 hash
    Sha512,
    /// xxHash (fast non-cryptographic)
    XxHash,
}

impl SchemaMapper {
    /// Create a new schema mapper
    pub fn new() -> Self {
        Self::default()
    }

    /// Rename a field
    pub fn rename_field(mut self, old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        self.field_mappings.insert(
            old_name.into(),
            FieldMapping {
                target: new_name.into(),
                transform: None,
            },
        );
        self
    }

    /// Add a field mapping with transformation
    pub fn map_field(
        mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        transform: FieldTransform,
    ) -> Self {
        self.field_mappings.insert(
            source.into(),
            FieldMapping {
                target: target.into(),
                transform: Some(transform),
            },
        );
        self
    }

    /// Drop a field
    pub fn drop_field(mut self, field: impl Into<String>) -> Self {
        self.drop_fields.push(field.into());
        self
    }

    /// Drop multiple fields
    pub fn drop_fields(mut self, fields: Vec<impl Into<String>>) -> Self {
        for field in fields {
            self.drop_fields.push(field.into());
        }
        self
    }

    /// Keep only specified fields
    pub fn keep_only(mut self, fields: Vec<impl Into<String>>) -> Self {
        self.keep_fields = Some(fields.into_iter().map(|f| f.into()).collect());
        self
    }

    /// Map collection name
    pub fn map_collection(mut self, new_name: impl Into<String>) -> Self {
        self.collection_mapping = Some(new_name.into());
        self
    }

    /// Add a default value for a field
    pub fn with_default(mut self, field: impl Into<String>, value: serde_json::Value) -> Self {
        self.defaults.insert(field.into(), value);
        self
    }

    /// Transform a change event
    pub fn transform(&self, mut event: ChangeEvent) -> CdcResult<ChangeEvent> {
        // Map collection name
        if let Some(ref new_name) = self.collection_mapping {
            event.collection = new_name.clone();
        }

        // Transform before state
        if let Some(before) = event.before.take() {
            event.before = Some(self.transform_record_state(before)?);
        }

        // Transform after state
        if let Some(after) = event.after.take() {
            event.after = Some(self.transform_record_state(after)?);
        }

        Ok(event)
    }

    /// Transform a record state
    fn transform_record_state(&self, state: RecordState) -> CdcResult<RecordState> {
        let mut metadata = state.metadata;
        let mut new_metadata = HashMap::new();

        // Apply keep filter if set
        if let Some(ref keep_fields) = self.keep_fields {
            metadata.retain(|k, _| keep_fields.contains(k));
        }

        // Drop fields
        for field in &self.drop_fields {
            metadata.remove(field);
        }

        // Apply mappings
        for (key, value) in metadata {
            if let Some(mapping) = self.field_mappings.get(&key) {
                let new_value = if let Some(ref transform) = mapping.transform {
                    self.apply_transform(&value, transform)?
                } else {
                    value
                };
                new_metadata.insert(mapping.target.clone(), new_value);
            } else {
                new_metadata.insert(key, value);
            }
        }

        // Apply defaults
        for (key, default_value) in &self.defaults {
            if !new_metadata.contains_key(key) {
                new_metadata.insert(key.clone(), default_value.clone());
            }
        }

        Ok(RecordState {
            vector: state.vector,
            metadata: new_metadata,
            raw: state.raw,
        })
    }

    /// Apply a field transformation
    fn apply_transform(
        &self,
        value: &serde_json::Value,
        transform: &FieldTransform,
    ) -> CdcResult<serde_json::Value> {
        match transform {
            FieldTransform::ToString => Ok(serde_json::Value::String(value.to_string())),

            FieldTransform::ToNumber => {
                let num = match value {
                    serde_json::Value::Number(n) => n.clone(),
                    serde_json::Value::String(s) => s
                        .parse::<f64>()
                        .ok()
                        .and_then(serde_json::Number::from_f64)
                        .ok_or_else(|| {
                            CdcError::Transform(format!("Cannot convert '{}' to number", s))
                        })?,
                    serde_json::Value::Bool(b) => {
                        serde_json::Number::from_f64(if *b { 1.0 } else { 0.0 }).unwrap()
                    }
                    _ => {
                        return Err(CdcError::Transform(format!(
                            "Cannot convert {:?} to number",
                            value
                        )))
                    }
                };
                Ok(serde_json::Value::Number(num))
            }

            FieldTransform::ToBool => {
                let b = match value {
                    serde_json::Value::Bool(b) => *b,
                    serde_json::Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                    serde_json::Value::String(s) => {
                        matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")
                    }
                    serde_json::Value::Null => false,
                    _ => true,
                };
                Ok(serde_json::Value::Bool(b))
            }

            FieldTransform::ParseJson => {
                if let serde_json::Value::String(s) = value {
                    serde_json::from_str(s)
                        .map_err(|e| CdcError::Transform(format!("JSON parse error: {}", e)))
                } else {
                    Ok(value.clone())
                }
            }

            FieldTransform::Case(case) => {
                if let serde_json::Value::String(s) = value {
                    let transformed = match case {
                        CaseTransform::Upper => s.to_uppercase(),
                        CaseTransform::Lower => s.to_lowercase(),
                        CaseTransform::Title => s
                            .split_whitespace()
                            .map(|word| {
                                let mut chars = word.chars();
                                match chars.next() {
                                    None => String::new(),
                                    Some(c) => {
                                        c.to_uppercase().chain(chars.flat_map(|c| c.to_lowercase())).collect()
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" "),
                        CaseTransform::Snake => {
                            let mut result = String::new();
                            for (i, c) in s.chars().enumerate() {
                                if c.is_uppercase() && i > 0 {
                                    result.push('_');
                                }
                                result.push(c.to_ascii_lowercase());
                            }
                            result
                        }
                        CaseTransform::Camel => {
                            let mut result = String::new();
                            let mut capitalize_next = false;
                            for c in s.chars() {
                                if c == '_' || c == '-' || c == ' ' {
                                    capitalize_next = true;
                                } else if capitalize_next {
                                    result.push(c.to_ascii_uppercase());
                                    capitalize_next = false;
                                } else {
                                    result.push(c.to_ascii_lowercase());
                                }
                            }
                            result
                        }
                    };
                    Ok(serde_json::Value::String(transformed))
                } else {
                    Ok(value.clone())
                }
            }

            FieldTransform::Substring { start, length } => {
                if let serde_json::Value::String(s) = value {
                    let chars: Vec<char> = s.chars().collect();
                    let end = length.map(|l| (*start + l).min(chars.len())).unwrap_or(chars.len());
                    let substring: String = chars[(*start).min(chars.len())..end].iter().collect();
                    Ok(serde_json::Value::String(substring))
                } else {
                    Ok(value.clone())
                }
            }

            FieldTransform::Hash(algorithm) => {
                let string_value = match value {
                    serde_json::Value::String(s) => s.clone(),
                    _ => value.to_string(),
                };

                let hash = match algorithm {
                    HashAlgorithm::Md5 => format!("{:x}", md5_hash(&string_value)),
                    HashAlgorithm::Sha256 => sha256_hash(&string_value),
                    HashAlgorithm::Sha512 => sha512_hash(&string_value),
                    HashAlgorithm::XxHash => format!("{:x}", xxhash(&string_value)),
                };

                Ok(serde_json::Value::String(hash))
            }

            FieldTransform::JsonPath(path) => {
                // Simple JSON path extraction
                let parts: Vec<&str> = path.split('.').collect();
                let mut current = value.clone();

                for part in parts {
                    match current {
                        serde_json::Value::Object(obj) => {
                            current = obj.get(part).cloned().unwrap_or(serde_json::Value::Null);
                        }
                        serde_json::Value::Array(arr) => {
                            if let Ok(idx) = part.parse::<usize>() {
                                current = arr.get(idx).cloned().unwrap_or(serde_json::Value::Null);
                            } else {
                                current = serde_json::Value::Null;
                            }
                        }
                        _ => {
                            current = serde_json::Value::Null;
                            break;
                        }
                    }
                }

                Ok(current)
            }

            FieldTransform::Regex { pattern, replacement } => {
                if let serde_json::Value::String(s) = value {
                    // Simple regex replacement (basic implementation)
                    // In production, use the regex crate
                    let result = s.replace(pattern.as_str(), replacement.as_str());
                    Ok(serde_json::Value::String(result))
                } else {
                    Ok(value.clone())
                }
            }

            FieldTransform::Concat { fields: _, separator: _ } => {
                // This would need access to all fields, not just the current value
                // Return value unchanged for now
                Ok(value.clone())
            }

            FieldTransform::Expression(_) => {
                // Expression evaluation not implemented yet
                Ok(value.clone())
            }
        }
    }

    /// Check if mapper has any transformations
    pub fn is_empty(&self) -> bool {
        self.field_mappings.is_empty()
            && self.drop_fields.is_empty()
            && self.keep_fields.is_none()
            && self.collection_mapping.is_none()
            && self.defaults.is_empty()
    }
}

// Simple hash implementations (non-cryptographic for testing)
fn md5_hash(input: &str) -> u128 {
    // Simple FNV-like hash for testing
    let mut hash: u128 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn sha256_hash(input: &str) -> String {
    // Placeholder - in production use sha2 crate
    let h = md5_hash(input);
    format!("{:032x}{:032x}", h, h.wrapping_mul(0x12345))
}

fn sha512_hash(input: &str) -> String {
    // Placeholder - in production use sha2 crate
    let h = md5_hash(input);
    format!("{:032x}{:032x}{:032x}{:032x}", h, h.wrapping_mul(0x12345), h.wrapping_mul(0x67890), h.wrapping_mul(0xabcde))
}

fn xxhash(input: &str) -> u64 {
    // Simple xxhash-like for testing
    let mut hash: u64 = 0x27d4eb2f165667c5;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x9e3779b97f4a7c15);
        hash = hash.rotate_left(31);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_state() -> RecordState {
        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), serde_json::json!("John Doe"));
        metadata.insert("age".to_string(), serde_json::json!(30));
        metadata.insert("email".to_string(), serde_json::json!("john@example.com"));
        metadata.insert("active".to_string(), serde_json::json!(true));

        RecordState {
            vector: None,
            metadata,
            raw: None,
        }
    }

    #[test]
    fn test_schema_mapper_new() {
        let mapper = SchemaMapper::new();
        assert!(mapper.is_empty());
    }

    #[test]
    fn test_rename_field() {
        let mapper = SchemaMapper::new().rename_field("name", "full_name");
        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.contains_key("full_name"));
        assert!(!result.metadata.contains_key("name"));
        assert_eq!(
            result.metadata.get("full_name"),
            Some(&serde_json::json!("John Doe"))
        );
    }

    #[test]
    fn test_drop_field() {
        let mapper = SchemaMapper::new().drop_field("email");
        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(!result.metadata.contains_key("email"));
        assert!(result.metadata.contains_key("name"));
    }

    #[test]
    fn test_drop_multiple_fields() {
        let mapper = SchemaMapper::new().drop_fields(vec!["email", "age"]);
        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(!result.metadata.contains_key("email"));
        assert!(!result.metadata.contains_key("age"));
        assert!(result.metadata.contains_key("name"));
    }

    #[test]
    fn test_keep_only() {
        let mapper = SchemaMapper::new().keep_only(vec!["name", "age"]);
        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.contains_key("name"));
        assert!(result.metadata.contains_key("age"));
        assert!(!result.metadata.contains_key("email"));
        assert!(!result.metadata.contains_key("active"));
    }

    #[test]
    fn test_with_default() {
        let mapper = SchemaMapper::new()
            .with_default("country", serde_json::json!("USA"));
        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(
            result.metadata.get("country"),
            Some(&serde_json::json!("USA"))
        );
    }

    #[test]
    fn test_transform_to_string() {
        let mapper = SchemaMapper::new().map_field("age", "age_str", FieldTransform::ToString);

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.get("age_str").unwrap().is_string());
    }

    #[test]
    fn test_transform_to_number() {
        let mut metadata = HashMap::new();
        metadata.insert("count".to_string(), serde_json::json!("42"));

        let state = RecordState {
            vector: None,
            metadata,
            raw: None,
        };

        let mapper = SchemaMapper::new().map_field("count", "count", FieldTransform::ToNumber);
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.get("count").unwrap().is_number());
    }

    #[test]
    fn test_transform_to_bool() {
        let mut metadata = HashMap::new();
        metadata.insert("flag".to_string(), serde_json::json!("true"));

        let state = RecordState {
            vector: None,
            metadata,
            raw: None,
        };

        let mapper = SchemaMapper::new().map_field("flag", "flag", FieldTransform::ToBool);
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(result.metadata.get("flag"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn test_case_transform_upper() {
        let mapper = SchemaMapper::new().map_field(
            "name",
            "name",
            FieldTransform::Case(CaseTransform::Upper),
        );

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(
            result.metadata.get("name"),
            Some(&serde_json::json!("JOHN DOE"))
        );
    }

    #[test]
    fn test_case_transform_lower() {
        let mapper = SchemaMapper::new().map_field(
            "name",
            "name",
            FieldTransform::Case(CaseTransform::Lower),
        );

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(
            result.metadata.get("name"),
            Some(&serde_json::json!("john doe"))
        );
    }

    #[test]
    fn test_substring_transform() {
        let mapper = SchemaMapper::new().map_field(
            "name",
            "initial",
            FieldTransform::Substring {
                start: 0,
                length: Some(1),
            },
        );

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(
            result.metadata.get("initial"),
            Some(&serde_json::json!("J"))
        );
    }

    #[test]
    fn test_hash_transform() {
        let mapper = SchemaMapper::new().map_field(
            "email",
            "email_hash",
            FieldTransform::Hash(HashAlgorithm::XxHash),
        );

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.get("email_hash").unwrap().is_string());
    }

    #[test]
    fn test_json_path_transform() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "data".to_string(),
            serde_json::json!({"nested": {"value": 42}}),
        );

        let state = RecordState {
            vector: None,
            metadata,
            raw: None,
        };

        let mapper = SchemaMapper::new().map_field(
            "data",
            "extracted",
            FieldTransform::JsonPath("nested.value".to_string()),
        );
        let result = mapper.transform_record_state(state).unwrap();

        assert_eq!(
            result.metadata.get("extracted"),
            Some(&serde_json::json!(42))
        );
    }

    #[test]
    fn test_collection_mapping() {
        use crate::cdc::event::{Operation, SourceInfo};

        let mapper = SchemaMapper::new().map_collection("new_collection");

        let event = ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "old_collection",
            "key_1",
        );

        let result = mapper.transform(event).unwrap();
        assert_eq!(result.collection, "new_collection");
    }

    #[test]
    fn test_chained_transforms() {
        let mapper = SchemaMapper::new()
            .rename_field("name", "full_name")
            .drop_field("email")
            .with_default("status", serde_json::json!("active"));

        let state = create_test_state();
        let result = mapper.transform_record_state(state).unwrap();

        assert!(result.metadata.contains_key("full_name"));
        assert!(!result.metadata.contains_key("name"));
        assert!(!result.metadata.contains_key("email"));
        assert!(result.metadata.contains_key("status"));
    }
}
