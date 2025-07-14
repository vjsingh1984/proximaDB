// Helper functions for proto metadata conversion with repeated MetadataItem

use std::collections::HashMap;
use crate::proto::proximadb::MetadataItem;

/// Convert repeated MetadataItem to serde_json Value map for REST API compatibility
pub fn proto_metadata_to_json(metadata: &[MetadataItem]) -> HashMap<String, serde_json::Value> {
    metadata.iter()
        .map(|item| {
            let value = if let Ok(num) = item.value.parse::<f64>() {
                // Try to parse as number first
                if let Some(json_num) = serde_json::Number::from_f64(num) {
                    serde_json::Value::Number(json_num)
                } else {
                    // Fallback to string if number is invalid (NaN, Inf)
                    serde_json::Value::String(item.value.clone())
                }
            } else if let Ok(bool_val) = item.value.parse::<bool>() {
                // Try to parse as boolean
                serde_json::Value::Bool(bool_val)
            } else {
                // Default to string
                serde_json::Value::String(item.value.clone())
            };
            (item.key.clone(), value)
        })
        .collect()
}

/// Convert serde_json Value map to repeated MetadataItem
pub fn json_metadata_to_proto(metadata: &HashMap<String, serde_json::Value>) -> Vec<MetadataItem> {
    metadata.iter()
        .map(|(k, v)| {
            let value_str = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            MetadataItem {
                key: k.clone(),
                value: value_str,
            }
        })
        .collect()
}

/// Convert repeated MetadataItem to HashMap<String, String> for internal use
pub fn proto_metadata_to_hashmap(metadata: &[MetadataItem]) -> HashMap<String, String> {
    metadata.iter()
        .map(|item| (item.key.clone(), item.value.clone()))
        .collect()
}

/// Convert HashMap<String, String> to repeated MetadataItem
pub fn hashmap_to_proto_metadata(metadata: &HashMap<String, String>) -> Vec<MetadataItem> {
    metadata.iter()
        .map(|(k, v)| MetadataItem {
            key: k.clone(),
            value: v.clone(),
        })
        .collect()
}