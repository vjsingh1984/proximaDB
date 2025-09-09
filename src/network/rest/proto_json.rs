//! Proto-JSON serialization helper for seamless protobuf-JSON conversion in REST APIs
//! 
//! This module provides a ProtoJson wrapper that enables automatic conversion
//! between protobuf messages and JSON for REST API handlers, ensuring consistency
//! with gRPC while maintaining REST-friendly JSON formats.

use axum::{
    async_trait,
    body::HttpBody,
    extract::FromRequest,
    http::Request,
    response::{IntoResponse, Response},
    Json,
};
use std::error::Error;
use prost::Message;
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;

/// Wrapper for protobuf messages in REST APIs
/// 
/// This struct provides automatic JSON serialization/deserialization for protobuf
/// messages, allowing REST handlers to work directly with protobuf types while
/// maintaining JSON input/output.
#[derive(Debug, Clone)]
pub struct ProtoJson<T>(pub T);

impl<T> ProtoJson<T> {
    /// Create a new ProtoJson wrapper
    pub fn new(value: T) -> Self {
        ProtoJson(value)
    }
    
    /// Extract the inner value
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Implement FromRequest for ProtoJson to enable extraction from HTTP requests
#[async_trait]
impl<T, S, B> FromRequest<S, B> for ProtoJson<T>
where
    T: Message + Default + for<'de> Deserialize<'de>,
    S: Send + Sync,
    B: HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn Error + Send + Sync>>,
{
    type Rejection = ApiError;

    async fn from_request(req: Request<B>, state: &S) -> Result<Self, Self::Rejection> {
        // Extract JSON from request body
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(|e| ApiError::InvalidArgument(format!("Invalid JSON: {}", e)))?;
        
        Ok(ProtoJson(value))
    }
}

/// Implement IntoResponse for ProtoJson to enable returning as HTTP response
impl<T> IntoResponse for ProtoJson<T>
where
    T: Message + Serialize,
{
    fn into_response(self) -> Response {
        // Serialize the protobuf message as JSON
        match serde_json::to_value(&self.0) {
            Ok(json_value) => Json(json_value).into_response(),
            Err(e) => {
                tracing::error!("Failed to serialize protobuf to JSON: {}", e);
                ApiError::Internal(format!("Serialization error: {}", e)).into_response()
            }
        }
    }
}

/// Helper trait for protobuf message conversion
pub trait ProtoConvert: Sized {
    /// Convert from JSON value to protobuf message
    fn from_json(json: serde_json::Value) -> Result<Self, ApiError>;
    
    /// Convert protobuf message to JSON value
    fn to_json(&self) -> Result<serde_json::Value, ApiError>;
}

/// Implement ProtoConvert for messages that implement serde traits
impl<T> ProtoConvert for T
where
    T: Message + Serialize + for<'de> Deserialize<'de> + Default,
{
    fn from_json(json: serde_json::Value) -> Result<Self, ApiError> {
        serde_json::from_value(json)
            .map_err(|e| ApiError::InvalidArgument(format!("Invalid JSON for proto: {}", e)))
    }
    
    fn to_json(&self) -> Result<serde_json::Value, ApiError> {
        serde_json::to_value(self)
            .map_err(|e| ApiError::Internal(format!("Failed to convert proto to JSON: {}", e)))
    }
}

/// Batch conversion helper for collections of protobuf messages
pub struct ProtoJsonBatch<T> {
    items: Vec<T>,
}

impl<T> ProtoJsonBatch<T> {
    /// Create a new batch from a vector of items
    pub fn new(items: Vec<T>) -> Self {
        ProtoJsonBatch { items }
    }
    
    /// Convert all items to JSON array
    pub fn to_json_array(&self) -> Result<serde_json::Value, ApiError>
    where
        T: Serialize,
    {
        serde_json::to_value(&self.items)
            .map_err(|e| ApiError::Internal(format!("Failed to serialize batch: {}", e)))
    }
    
    /// Create from JSON array
    pub fn from_json_array(json: serde_json::Value) -> Result<Self, ApiError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let items: Vec<T> = serde_json::from_value(json)
            .map_err(|e| ApiError::InvalidArgument(format!("Invalid JSON array: {}", e)))?;
        Ok(ProtoJsonBatch { items })
    }
}

/// Response wrapper that ensures consistent JSON structure
#[derive(Debug, Serialize, Deserialize)]
pub struct ProtoApiResponse<T> {
    /// Indicates if the operation was successful
    pub success: bool,
    
    /// The actual data (protobuf message serialized as JSON)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    
    /// Error information if the operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    
    /// Additional metadata about the response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseMetadata {
    pub request_id: String,
    pub processing_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
}

impl<T> ProtoApiResponse<T>
where
    T: Serialize,
{
    /// Create a successful response
    pub fn success(data: T) -> Self {
        ProtoApiResponse {
            success: true,
            data: Some(data),
            error: None,
            metadata: None,
        }
    }
    
    /// Create an error response
    pub fn error(error: ApiError) -> Self {
        ProtoApiResponse {
            success: false,
            data: None,
            error: Some(ErrorInfo {
                code: format!("{:?}", error),
                message: error.to_string(),
                details: None,
            }),
            metadata: None,
        }
    }
    
    /// Add metadata to the response
    pub fn with_metadata(mut self, metadata: ResponseMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

impl<T> IntoResponse for ProtoApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

/// Utility functions for common proto-JSON conversions
pub mod utils {
    use crate::proto::proximadb;
    
    /// Convert a VectorRecord to JSON-friendly format
    pub fn vector_record_to_json(record: &proximadb::VectorRecord) -> serde_json::Value {
        serde_json::json!({
            "id": record.id,
            "vector": record.vector,
            "metadata": metadata_to_json(&record.metadata),
            "timestamp": record.timestamp,
            "version": record.version,
        })
    }
    
    /// Convert metadata items to JSON object
    pub fn metadata_to_json(items: &[proximadb::MetadataItem]) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        for item in items {
            if let Some(value) = &item.value {
                map.insert(item.key.clone(), metadata_value_to_json(value));
            }
        }
        map
    }
    
    /// Convert a single metadata item value to JSON
    pub fn metadata_value_to_json(value: &proximadb::metadata_item::Value) -> serde_json::Value {
        match value {
            proximadb::metadata_item::Value::StringValue(s) => serde_json::json!(s),
            proximadb::metadata_item::Value::NumberValue(f) => serde_json::json!(f),
            proximadb::metadata_item::Value::BoolValue(b) => serde_json::json!(b),
        }
    }
    
    /// Convert JSON to metadata items
    pub fn json_to_metadata(obj: serde_json::Map<String, serde_json::Value>) -> Vec<proximadb::MetadataItem> {
        obj.into_iter()
            .map(|(key, value)| proximadb::MetadataItem {
                key,
                value: Some(json_to_metadata_value(value)),
            })
            .collect()
    }
    
    /// Convert JSON value to metadata item value
    pub fn json_to_metadata_value(value: serde_json::Value) -> proximadb::metadata_item::Value {
        match value {
            serde_json::Value::String(s) => {
                proximadb::metadata_item::Value::StringValue(s)
            }
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    proximadb::metadata_item::Value::NumberValue(f)
                } else {
                    proximadb::metadata_item::Value::NumberValue(0.0)
                }
            }
            serde_json::Value::Bool(b) => {
                proximadb::metadata_item::Value::BoolValue(b)
            }
            // Arrays and other complex types fallback to string representation
            _ => proximadb::metadata_item::Value::StringValue(value.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_proto_json_conversion() {
        // Test that ProtoJson wrapper maintains data integrity
        let data = vec![1, 2, 3];
        let proto_json = ProtoJson::new(data.clone());
        assert_eq!(proto_json.into_inner(), data);
    }
    
    #[test]
    fn test_metadata_conversion() {
        use utils::*;
        
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), serde_json::json!("test"));
        map.insert("count".to_string(), serde_json::json!(42));
        map.insert("active".to_string(), serde_json::json!(true));
        
        let metadata = json_to_metadata(map.clone());
        assert_eq!(metadata.len(), 3);
        
        let converted_back = metadata_to_json(&metadata);
        assert_eq!(converted_back.get("name").unwrap(), &serde_json::json!("test"));
        assert_eq!(converted_back.get("count").unwrap(), &serde_json::json!(42));
        assert_eq!(converted_back.get("active").unwrap(), &serde_json::json!(true));
    }
}