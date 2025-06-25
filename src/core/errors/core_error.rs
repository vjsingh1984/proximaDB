//! Core ProximaDB error types

use serde::{Deserialize, Serialize};
use thiserror::Error;
use super::{ConfigError, MetadataError, ServiceError};

/// Main ProximaDB error type
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ProximaDBError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    
    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),
    
    #[error("Service error: {0}")]
    Service(#[from] ServiceError),
    
    #[error("Storage error: {0}")]
    Storage(String),
    
    #[error("Index error: {0}")]
    Index(String),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Authentication error: {0}")]
    Authentication(String),
    
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    
    #[error("Resource not found: {resource_type} '{id}'")]
    NotFound { resource_type: String, id: String },
    
    #[error("Resource already exists: {resource_type} '{id}'")]
    AlreadyExists { resource_type: String, id: String },
    
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
}