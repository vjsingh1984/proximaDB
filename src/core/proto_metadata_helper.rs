// Helper functions for proto metadata conversion with repeated MetadataItem

use crate::proto::proximadb_v1::{MetadataItem, metadata_item};
use std::collections::HashMap;

/// Convert repeated MetadataItem to serde_json Value map for REST API compatibility
///
/// This properly handles the typed metadata values from the proto definition,
/// preserving type information for accurate comparisons and filtering.
pub fn proto_metadata_to_json(metadata: &[MetadataItem]) -> HashMap<String, serde_json::Value> {
    metadata
        .iter()
        .map(|(key, value)| {
            let value = match &value {
                Some(metadata_item::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                Some(metadata_item::Value::NumberValue(n)) => serde_json::Number::from_f64(n)
                    .map(serde_json::Value::Number)
                    .unwrap_or_else(|| serde_json::Value::String(n.to_string())),
                Some(metadata_item::Value::BoolValue(b)) => serde_json::Value::Bool(b),
                None => serde_json::Value::Null,
            };
            (key.clone(), value)
        })
        .collect()
}

/// Convert serde_json Value map to repeated MetadataItem
pub fn json_metadata_to_proto(metadata: &HashMap<String, serde_json::Value>) -> Vec<MetadataItem> {
    metadata
        .iter()
        .map(|(k, v)| {
            let value = match v {
                serde_json::Value::String(s) => Some(metadata_item::Value::StringValue(s.clone())),
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        Some(metadata_item::Value::NumberValue(f))
                    } else {
                        // Fallback for very large integers
                        Some(metadata_item::Value::StringValue(n.to_string()))
                    }
                }
                serde_json::Value::Bool(b) => Some(metadata_item::Value::BoolValue(*b)),
                serde_json::Value::Null => None,
                other => Some(metadata_item::Value::StringValue(other.to_string())), // Arrays and objects become JSON strings
            };
            MetadataItem {
                key: k.clone(),
                value,
            }
        })
        .collect()
}

/// Convert repeated MetadataItem to HashMap<String, String> for internal use
/// This converts all values to strings for backward compatibility
pub fn proto_metadata_to_hashmap(metadata: &[MetadataItem]) -> HashMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| {
            let value_str = match &value {
                Some(metadata_item::Value::StringValue(s)) => s.clone(),
                Some(metadata_item::Value::NumberValue(n)) => n.to_string(),
                Some(metadata_item::Value::BoolValue(b)) => b.to_string(),
                None => String::new(),
            };
            (key.clone(), value_str)
        })
        .collect()
}

/// Convert HashMap<String, String> to repeated MetadataItem
/// All values are stored as strings since the source is HashMap<String, String>
pub fn hashmap_to_proto_metadata(metadata: &HashMap<String, String>) -> Vec<MetadataItem> {
    metadata
        .iter()
        .map(|(k, v)| {
            // Try to parse the string value to determine its type
            let value = if let Ok(n) = v.parse::<f64>() {
                Some(metadata_item::Value::NumberValue(n))
            } else if let Ok(b) = v.parse::<bool>() {
                Some(metadata_item::Value::BoolValue(b))
            } else {
                // Default to string for all other cases (including empty strings)
                Some(metadata_item::Value::StringValue(v.clone()))
            };

            MetadataItem {
                key: k.clone(),
                value,
            }
        })
        .collect()
}
