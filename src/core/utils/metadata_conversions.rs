//! # Metadata Conversion Utilities
//! 
//! This module provides centralized conversion functions between different metadata representations
//! used throughout ProximaDB. It consolidates previously duplicated conversion logic from various
//! storage engines and API handlers.
//!
//! ## Supported Conversions
//! 
//! - **Proto ↔ JSON**: Convert between Protocol Buffer MetadataItem and JSON representations
//! - **JSON ↔ HashMap**: Convert between serde_json::Value and HashMap<String, Value>
//! - **String ↔ Value**: Parse and serialize metadata values of different types
//!
//! ## Usage Examples
//!
//! ```rust
//! use proximadb::core::utils::metadata_conversions::*;
//! 
//! // Convert protobuf metadata to JSON
//! let proto_metadata = vec![MetadataItem { key: "age".into(), value: Some(NumberValue(25.0)) }];
//! let json_map = proto_metadata_to_json(&proto_metadata);
//! 
//! // Convert JSON to protobuf metadata
//! let mut json_obj = serde_json::Map::new();
//! json_obj.insert("name".into(), json!("Alice"));
//! let proto_items = json_to_proto_metadata(json_obj);
//! ```

use crate::proto::proximadb_v1::{MetadataItem, metadata_item};
use serde_json::{Map, Value as JsonValue};
use std::collections::HashMap;
use anyhow::{Result, anyhow};

/// ## Proto to JSON Conversion
/// 
/// Converts a slice of Protocol Buffer MetadataItems to a JSON object representation.
/// This function handles all metadata value types including strings, numbers, booleans,
/// arrays, and nested objects.
///
/// ### Arguments
/// * `metadata` - Slice of MetadataItem from protobuf definition
///
/// ### Returns
/// * HashMap where keys are metadata field names and values are JSON representations
///
/// ### Performance Note
/// This function performs allocation for each metadata item. For hot paths,
/// consider caching the converted result.
pub fn proto_metadata_to_json(metadata: &[MetadataItem]) -> HashMap<String, JsonValue> {
    let mut map = HashMap::with_capacity(metadata.len());
    
    for (key, value) in metadata {
        if let Some(ref value) = value {
            let json_value = match value {
                metadata_item::Value::StringValue(s) => JsonValue::String(s.clone()),
                metadata_item::Value::NumberValue(n) => JsonValue::Number(
                    serde_json::Number::from_f64(*n).unwrap_or_else(|| serde_json::Number::from(0))
                ),
                metadata_item::Value::BoolValue(b) => JsonValue::Bool(*b),
                // Note: Arrays and objects are serialized as JSON strings for now
                // since the proto doesn't have native array/object types yet
            };
            map.insert(item.0.clone(), json_value);
        }
    }
    
    map
}

/// ## JSON to Proto Conversion
/// 
/// Converts a JSON object to a vector of Protocol Buffer MetadataItems.
/// Handles nested objects and arrays appropriately.
///
/// ### Arguments
/// * `json_map` - serde_json Map containing the metadata
///
/// ### Returns
/// * Vector of MetadataItem for protobuf serialization
///
/// ### Type Mapping
/// - JSON String → StringValue
/// - JSON Number → NumberValue  
/// - JSON Boolean → BoolValue
/// - JSON Array of Strings → StringArrayValue
/// - JSON Array of Numbers → NumberArrayValue
/// - JSON Object → ObjectValue (recursive)
pub fn json_to_proto_metadata(json_map: Map<String, JsonValue>) -> Vec<MetadataItem> {
    let mut items = Vec::with_capacity(json_map.len());
    
    for (key, value) in json_map {
        let metadata_value = json_value_to_metadata_value(&value);
        if let Some(val) = metadata_value {
            items.push(MetadataItem {
                key,
                value: Some(val),
            });
        }
    }
    
    items
}

/// ## JSON Value to Metadata Value Conversion
///
/// Converts a single JSON value to the appropriate metadata_item::Value variant.
/// This is a helper function used internally by json_to_proto_metadata.
///
/// ### Arguments  
/// * `value` - The JSON value to convert
///
/// ### Returns
/// * Option containing the converted metadata value, None if value is null
///
/// ### Note
/// Arrays and objects are serialized as JSON strings since the proto
/// doesn't have native support for these types yet.
fn json_value_to_metadata_value(value: &JsonValue) -> Option<metadata_item::Value> {
    match value {
        JsonValue::String(s) => Some(metadata_item::Value::StringValue(s.clone())),
        JsonValue::Number(n) => Some(metadata_item::Value::NumberValue(n.as_f64().unwrap_or(0.0))),
        JsonValue::Bool(b) => Some(metadata_item::Value::BoolValue(*b)),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            // Serialize complex types as JSON strings for now
            Some(metadata_item::Value::StringValue(value.to_string()))
        }
        JsonValue::Null => None,
    }
}

/// ## Bloom Filter Helper
///
/// Converts a JSON value and field name to a MetadataItem suitable for bloom filter operations.
/// This is commonly used in SST and other storage engines for metadata filtering.
///
/// ### Arguments
/// * `field` - The field name for the metadata
/// * `value` - The JSON value to convert
///
/// ### Returns
/// * MetadataItem configured for bloom filter usage
pub fn json_to_metadata_item(field: &str, value: &JsonValue) -> MetadataItem {
    MetadataItem {
        key: field.to_string(),
        value: json_value_to_metadata_value(value),
    }
}

/// ## Metadata Merge Utility
///
/// Merges two sets of metadata, with values from the second set overwriting the first.
/// Useful for applying updates or patches to existing metadata.
///
/// ### Arguments
/// * `base` - The base metadata to merge into
/// * `updates` - The metadata updates to apply
///
/// ### Returns
/// * Merged metadata vector with updates applied
pub fn merge_metadata(base: &[MetadataItem], updates: &[MetadataItem]) -> Vec<MetadataItem> {
    let mut merged: HashMap<String, MetadataItem> = HashMap::new();
    
    // Add base metadata
    for item in base {
        merged.insert(item.0.clone(), item.clone());
    }
    
    // Apply updates (overwrites existing keys)
    for item in updates {
        merged.insert(item.0.clone(), item.clone());
    }
    
    merged.into_values().collect()
}

/// ## Metadata Filtering
///
/// Filters metadata items based on a list of allowed keys.
/// Useful for projection and privacy operations.
///
/// ### Arguments
/// * `metadata` - The metadata to filter
/// * `allowed_keys` - Set of keys that should be retained
///
/// ### Returns
/// * Filtered metadata containing only allowed keys
pub fn filter_metadata(metadata: &[MetadataItem], allowed_keys: &[&str]) -> Vec<MetadataItem> {
    metadata.iter()
        .filter(|(key, value)| allowed_keys.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// ## Type Validation
///
/// Validates that metadata values match expected types.
/// Returns an error if any type mismatches are found.
///
/// ### Arguments
/// * `metadata` - The metadata to validate
/// * `schema` - HashMap mapping field names to expected value types
///
/// ### Returns
/// * Ok(()) if all types match, Err with details of mismatches
pub fn validate_metadata_types(
    metadata: &[MetadataItem], 
    schema: &HashMap<String, MetadataValueType>
) -> Result<()> {
    for (key, value) in metadata {
        if let Some(expected_type) = schema.get(&key) {
            if !matches_type(&value, expected_type) {
                return Err(anyhow!(
                    "Type mismatch for field '{}': expected {:?}",
                    key, expected_type
                ));
            }
        }
    }
    Ok(())
}

/// ## Metadata Value Type Enumeration
///
/// Represents the possible types for metadata values.
/// Used for schema validation and type checking.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValueType {
    String,
    Number,
    Boolean,
    // Complex types (arrays, objects) are stored as JSON strings for now
}

/// Helper function to check if a metadata value matches an expected type
fn matches_type(value: &Option<metadata_item::Value>, expected: &MetadataValueType) -> bool {
    match (value, expected) {
        (Some(metadata_item::Value::StringValue(_)), MetadataValueType::String) => true,
        (Some(metadata_item::Value::NumberValue(_)), MetadataValueType::Number) => true,
        (Some(metadata_item::Value::BoolValue(_)), MetadataValueType::Boolean) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_proto_to_json_conversion() {
        let proto_metadata = vec![
            MetadataItem {
                key: "name".to_string(),
                value: Some(metadata_item::Value::StringValue("Alice".to_string())),
            },
            MetadataItem {
                key: "age".to_string(),  
                value: Some(metadata_item::Value::NumberValue(25.0)),
            },
        ];
        
        let json_map = proto_metadata_to_json(&proto_metadata);
        
        assert_eq!(json_map.get("name").unwrap(), &JsonValue::String("Alice".to_string()));
        assert_eq!(json_map.get("age").unwrap().as_f64().unwrap(), 25.0);
    }
    
    #[test]
    fn test_json_to_proto_conversion() {
        let mut json_map = Map::new();
        json_map.insert("active".to_string(), JsonValue::Bool(true));
        json_map.insert("score".to_string(), JsonValue::Number(serde_json::Number::from(99)));
        
        let proto_metadata = json_to_proto_metadata(json_map);
        
        assert_eq!(proto_metadata.len(), 2);
        assert!(proto_metadata.iter().any(|item| key == "active"));
        assert!(proto_metadata.iter().any(|item| key == "score"));
    }
}