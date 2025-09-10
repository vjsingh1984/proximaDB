#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::proto::proximadb_v1::{MetadataItem, VectorRecord, metadata_item};
    use axum::Json;
    use axum::extract::{Path, Query, State};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_metadata_conversion_helpers() {
        use crate::core::proto_metadata_helper;

        // Create test metadata with different value types
        let proto_metadata = vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(metadata_item::Value::StringValue("electronics".to_string())),
            },
            MetadataItem {
                key: "price".to_string(),
                value: Some(metadata_item::Value::NumberValue(99.99)),
            },
            MetadataItem {
                key: "in_stock".to_string(),
                value: Some(metadata_item::Value::BoolValue(true)),
            },
        ];

        // Test proto_metadata_to_json conversion
        let json_metadata = proto_metadata_helper::proto_metadata_to_json(&proto_metadata);

        // Verify JSON metadata preserves types
        assert_eq!(
            json_metadata.get("category"),
            Some(&serde_json::Value::String("electronics".to_string()))
        );
        assert_eq!(
            json_metadata.get("price"),
            Some(&serde_json::Value::Number(
                serde_json::Number::from_f64(99.99).unwrap()
            ))
        );
        assert_eq!(json_metadata.get("in_stock"), Some(&serde_json::Value::Bool(true)));

        // Test proto_metadata_to_hashmap (converts to strings)
        let hashmap_metadata = proto_metadata_helper::proto_metadata_to_hashmap(&proto_metadata);

        // Verify hashmap metadata is all strings
        assert_eq!(hashmap_metadata.get("category"), Some(&"electronics".to_string()));
        assert_eq!(hashmap_metadata.get("price"), Some(&"99.99".to_string()));
        assert_eq!(hashmap_metadata.get("in_stock"), Some(&"true".to_string()));
    }
}
