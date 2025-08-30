//! Core ProximaDB error types

use serde::{Deserialize, Serialize};
use thiserror::Error;
use super::{ConfigError, MetadataError, ServiceError};
use std::io;
use std::sync::Arc;

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
    Storage(#[from] StorageError),
    
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
    
    #[error("Invalid cache key format: {0}")]
    InvalidCacheKey(String),
    
    #[error("Unknown engine type: {0}")]
    UnknownEngineType(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
    
    #[error("IO error: {0}")]
    Io(String),
    
    #[error("Timeout: operation exceeded {0} seconds")]
    Timeout(u64),
    
    #[error("Capacity exceeded: {message}")]
    CapacityExceeded { message: String },
    
    #[error("Compression error: {0}")]
    Compression(String),
    
    #[error("Quantization error: {0}")]
    Quantization(String),
}

/// Storage-specific error types
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum StorageError {
    #[error("Disk IO error: {0}")]
    DiskIO(String),
    
    #[error("WAL error: {0}")]
    WAL(String),
    
    #[error("Compaction error: {0}")]
    Compaction(String),
    
    #[error("Flush error: {0}")]
    Flush(String),
    
    #[error("Corruption detected: {0}")]
    Corruption(String),
    
    #[error("Engine not found: {0}")]
    EngineNotFound(String),
    
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    
    #[error("Lock acquisition failed: {0}")]
    LockFailed(String),
    
    #[error("Metadata error: {0}")]
    Metadata(String),
}

// Custom implementation for io::Error conversion since it's not Clone/Serialize
impl From<io::Error> for ProximaDBError {
    fn from(err: io::Error) -> Self {
        ProximaDBError::Io(err.to_string())
    }
}

impl From<io::Error> for StorageError {
    fn from(err: io::Error) -> Self {
        StorageError::DiskIO(err.to_string())
    }
}