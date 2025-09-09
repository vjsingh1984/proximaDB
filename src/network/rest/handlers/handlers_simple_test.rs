#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::proto_metadata_helper;
    use crate::proto::proximadb::{MetadataItem, metadata_item};
    use tracing::{debug, error, info};

    #[test]
    fn test_metadata_conversion_issue() {
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

        // Convert using the function used in REST handler
        let converted = proto_metadata_helper::proto_metadata_to_hashmap(&proto_metadata);

        debug!("Original metadata: {:?}", proto_metadata);
        debug!("Converted metadata: {:?}", converted);

        // Check conversion
        assert_eq!(converted.get("category"), Some(&"electronics".to_string()));
        assert_eq!(converted.get("price"), Some(&"99.99".to_string()));
        assert_eq!(converted.get("in_stock"), Some(&"true".to_string()));
    }

    #[test]
    fn test_empty_metadata_conversion() {
        let empty_metadata: Vec<MetadataItem> = vec![];
        let converted = proto_metadata_helper::proto_metadata_to_hashmap(&empty_metadata);

        debug!("Empty metadata conversion: {:?}", converted);
        assert!(converted.is_none());
    }
}
