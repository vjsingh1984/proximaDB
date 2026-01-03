//! # Core Error Handling Module
//!
//! This module defines ProximaDB's comprehensive error handling system with
//! structured error types, automatic conversions, and serialization support
//! for network transmission.
//!
//! ## Design Philosophy
//!
//! 1. **Structured Errors**: Each error category has its own enum variant
//! 2. **Contextual Information**: Errors include relevant context (IDs, types)
//! 3. **Network-Ready**: All errors are serializable for gRPC/REST responses
//! 4. **Automatic Conversion**: `From` traits for seamless error propagation
//! 5. **User-Friendly**: Clear error messages for debugging and logging
//!
//! ## Error Categories
//!
//! ### Configuration Errors
//! - Invalid configuration values
//! - Missing required settings
//! - Environment variable issues
//!
//! ### Storage Errors
//! - Disk I/O failures
//! - WAL corruption
//! - Compaction failures
//! - Lock contention
//!
//! ### Service Errors
//! - RPC failures
//! - Timeout issues
//! - Resource exhaustion
//!
//! ### Index Errors
//! - Index build failures
//! - Search errors
//! - Quantization issues
//!
//! ## Error Propagation
//!
//! ```rust,no_run
//! # use proximadb::core::ProximaDBError;
//! // Automatic conversion with ? operator
//! fn process_vector() -> Result<(), ProximaDBError> {
//! #   fn load_config() -> Result<(), ProximaDBError> { Ok(()) }
//! #   fn open_storage() -> Result<(), ProximaDBError> { Ok(()) }
//!     let config = load_config()?;  // ConfigError -> ProximaDBError
//!     let storage = open_storage()?; // StorageError -> ProximaDBError
//!     Ok(())
//! }
//! ```
//!
//! ## Network Serialization
//!
//! All errors implement Serialize/Deserialize for network transmission:
//!
//! ```rust,ignore
//! // Convert to gRPC status
//! let status = match error {
//!     ProximaDBError::NotFound { .. } => Status::not_found(error.to_string()),
//!     ProximaDBError::PermissionDenied(_) => Status::permission_denied(error.to_string()),
//!     _ => Status::internal(error.to_string()),
//! };
//! ```

use super::{ConfigError, MetadataError, ServiceError};
use serde::{Deserialize, Serialize};
use std::io;
use thiserror::Error;

/// Main ProximaDB error type
///
/// ## Usage Guidelines
///
/// 1. **Choose Specific Variants**: Use the most specific error variant available
/// 2. **Include Context**: Always provide meaningful error messages with context
/// 3. **Avoid Internal**: Reserve `Internal` for truly unexpected failures
/// 4. **Resource Errors**: Use `NotFound`/`AlreadyExists` for resource operations
/// 5. **Validation**: Use `InvalidInput` for user input validation failures
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

    // Transaction errors
    #[error("Transaction not found: {id}")]
    TransactionNotFound { id: String },

    #[error("Transaction not active: {id}")]
    TransactionNotActive { id: String },

    #[error("Transaction timed out: {id}")]
    TransactionTimedOut { id: String },

    #[error("Transaction conflict: {transaction} conflicts with {conflicting_with}")]
    TransactionConflict {
        transaction: String,
        conflicting_with: String,
    },

    #[error("Too many transactions: maximum {max} concurrent transactions allowed")]
    TooManyTransactions { max: usize },

    #[error("Lock timeout for resource: {resource}")]
    LockTimeout { resource: String },

    #[error("Deadlock detected for transaction: {transaction}")]
    DeadlockDetected { transaction: String },

    #[error("Savepoint not found: {name}")]
    SavepointNotFound { name: String },
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
