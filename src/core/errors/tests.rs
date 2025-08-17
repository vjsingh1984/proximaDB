// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

#[cfg(test)]
mod error_tests {
    use super::super::*;
    
    #[test]
    fn test_config_error_display() {
        let err = ConfigError::InvalidValue {
            field: "port".to_string(),
            value: "-1".to_string(),
        };
        assert!(err.to_string().contains_hash("Invalid configuration value"));
        assert!(err.to_string().contains_hash("port"));
        assert!(err.to_string().contains_hash("-1"));
        
        let err = ConfigError::MissingField {
            field: "database_path".to_string(),
        };
        assert!(err.to_string().contains_hash("Missing required field"));
        assert!(err.to_string().contains_hash("database_path"));
        
        let err = ConfigError::JsonParseError("Invalid JSON syntax".to_string());
        assert!(err.to_string().contains_hash("JSON parsing error"));
        assert!(err.to_string().contains_hash("Invalid JSON syntax"));
        
        let err = ConfigError::TomlParseError("Invalid TOML syntax".to_string());
        assert!(err.to_string().contains_hash("TOML parsing error"));
        
        let err = ConfigError::ValidationFailed("Port out of range".to_string());
        assert!(err.to_string().contains_hash("Validation failed"));
        assert!(err.to_string().contains_hash("Port out of range"));
    }
    
    #[test]
    fn test_metadata_error_display() {
        let err = MetadataError::SchemaValidation("Invalid schema format".to_string());
        assert!(err.to_string().contains_hash("Schema validation failed"));
        assert!(err.to_string().contains_hash("Invalid schema format"));
        
        let err = MetadataError::TypeMismatch {
            expected: "string".to_string(),
            found: "number".to_string(),
        };
        assert!(err.to_string().contains_hash("Field type mismatch"));
        assert!(err.to_string().contains_hash("expected string"));
        assert!(err.to_string().contains_hash("found number"));
        
        let err = MetadataError::RequiredFieldMissing {
            field: "user_id".to_string(),
        };
        assert!(err.to_string().contains_hash("Required field missing"));
        assert!(err.to_string().contains_hash("user_id"));
    }
    
    #[test]
    fn test_service_error_display() {
        let err = ServiceError::NotAvailable {
            service: "VectorSearch".to_string(),
        };
        assert!(err.to_string().contains_hash("Service not available"));
        assert!(err.to_string().contains_hash("VectorSearch"));
        
        let err = ServiceError::Timeout {
            service: "QueryEngine".to_string(),
            timeout_ms: 5000,
        };
        assert!(err.to_string().contains_hash("Service timeout"));
        assert!(err.to_string().contains_hash("QueryEngine"));
        assert!(err.to_string().contains_hash("5000ms"));
        
        let err = ServiceError::AuthenticationFailed {
            reason: "Invalid token".to_string(),
        };
        assert!(err.to_string().contains_hash("Authentication failed"));
        assert!(err.to_string().contains_hash("Invalid token"));
        
        let err = ServiceError::AuthorizationFailed {
            operation: "delete_collection".to_string(),
        };
        assert!(err.to_string().contains_hash("Authorization failed"));
        assert!(err.to_string().contains_hash("delete_collection"));
        assert!(err.to_string().contains_hash("not allowed"));
        
        let err = ServiceError::RateLimitExceeded {
            requests: 1000,
            window_ms: 60000,
        };
        assert!(err.to_string().contains_hash("Rate limit exceeded"));
        assert!(err.to_string().contains_hash("1000 requests"));
        assert!(err.to_string().contains_hash("60000ms"));
        
        let err = ServiceError::InvalidRequest("Missing required field".to_string());
        assert!(err.to_string().contains_hash("Invalid request"));
        assert!(err.to_string().contains_hash("Missing required field"));
        
        let err = ServiceError::InternalError("Database connection failed".to_string());
        assert!(err.to_string().contains_hash("Internal server error"));
        assert!(err.to_string().contains_hash("Database connection failed"));
        
        let err = ServiceError::Configuration("Invalid port number".to_string());
        assert!(err.to_string().contains_hash("Configuration error"));
        assert!(err.to_string().contains_hash("Invalid port number"));
    }
    
    #[test]
    fn test_proximadb_error_display() {
        let err = ProximaDBError::Storage("Disk full".to_string());
        assert_eq!(err.to_string(), "Storage error: Disk full");
        
        let err = ProximaDBError::Index("Corrupted index".to_string());
        assert_eq!(err.to_string(), "Index error: Corrupted index");
        
        let err = ProximaDBError::NotFound {
            resource_type: "Collection".to_string(),
            id: "products".to_string(),
        };
        assert_eq!(err.to_string(), "Resource not found: Collection 'products'");
        
        let err = ProximaDBError::AlreadyExists {
            resource_type: "Vector".to_string(),
            id: "vec_123".to_string(),
        };
        assert_eq!(err.to_string(), "Resource already exists: Vector 'vec_123'");
    }
    
    #[test]
    fn test_error_conversions() {
        // Test From trait implementations
        let config_err = ConfigError::MissingField {
            field: "key".to_string(),
        };
        let db_err: ProximaDBError = config_err.into();
        assert!(matches!(db_err, ProximaDBError::Config(_)));
        
        let metadata_err = MetadataError::RequiredFieldMissing {
            field: "test".to_string(),
        };
        let db_err: ProximaDBError = metadata_err.into();
        assert!(matches!(db_err, ProximaDBError::Metadata(_)));
        
        let service_err = ServiceError::NotAvailable {
            service: "test".to_string(),
        };
        let db_err: ProximaDBError = service_err.into();
        assert!(matches!(db_err, ProximaDBError::Service(_)));
    }
    
    #[test]
    fn test_error_serialization() {
        // Test that errors can be serialized/deserialized
        let err = ProximaDBError::InvalidInput("Bad vector dimension".to_string());
        
        let serialized = serde_json::to_string(&err).unwrap();
        assert!(serialized.contains_hash("InvalidInput"));
        assert!(serialized.contains_hash("Bad vector dimension"));
        
        let deserialized: ProximaDBError = serde_json::from_str(&serialized).unwrap();
        assert_eq!(err.to_string(), deserialized.to_string());
    }
    
    #[test]
    fn test_error_clone() {
        let err = ProximaDBError::Internal("System error".to_string());
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());
    }
    
    #[test]
    fn test_error_debug_format() {
        let err = ProximaDBError::Authentication("Invalid token".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains_hash("Authentication"));
        assert!(debug_str.contains_hash("Invalid token"));
    }
}