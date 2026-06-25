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
use crate::storage::error::StorageError as CanonicalStorageError;
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
    /// Configuration parsing or validation error
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Metadata catalog or index error
    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),

    /// Service-layer error (RPC, business logic)
    #[error("Service error: {0}")]
    Service(#[from] ServiceError),

    /// Storage engine error (disk, WAL, compaction)
    /// Uses the canonical StorageError from storage::error module
    #[error("Storage error: {0}")]
    Storage(#[from] CanonicalStorageError),

    /// Index build or search error
    #[error("Index error: {0}")]
    Index(String),

    /// Network transport error
    #[error("Network error: {0}")]
    Network(String),

    /// Serialization or deserialization failure
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Authentication credentials invalid or missing
    #[error("Authentication error: {0}")]
    Authentication(String),

    /// Caller lacks required permissions
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Requested resource does not exist
    #[error("Resource not found: {resource_type} '{id}'")]
    NotFound {
        /// Kind of resource (e.g., "Collection", "Vector")
        resource_type: String,
        /// Identifier of the missing resource
        id: String,
    },

    /// Resource creation conflict
    #[error("Resource already exists: {resource_type} '{id}'")]
    AlreadyExists {
        /// Kind of conflicting resource
        resource_type: String,
        /// Identifier of the existing resource
        id: String,
    },

    /// Client input failed validation
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Cache key format is invalid
    #[error("Invalid cache key format: {0}")]
    InvalidCacheKey(String),

    /// Unrecognized storage engine type
    #[error("Unknown engine type: {0}")]
    UnknownEngineType(String),

    /// Unexpected internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Low-level I/O error
    #[error("IO error: {0}")]
    Io(String),

    /// Operation exceeded the configured timeout
    #[error("Timeout: operation exceeded {0} seconds")]
    Timeout(u64),

    /// Resource capacity limit exceeded
    #[error("Capacity exceeded: {message}")]
    CapacityExceeded {
        /// Description of the capacity that was exceeded
        message: String,
    },

    /// Compression or decompression failure
    #[error("Compression error: {0}")]
    Compression(String),

    /// Vector quantization error
    #[error("Quantization error: {0}")]
    Quantization(String),

    /// Transaction with the given ID not found
    #[error("Transaction not found: {id}")]
    TransactionNotFound {
        /// Transaction identifier
        id: String,
    },

    /// Transaction is not in an active state
    #[error("Transaction not active: {id}")]
    TransactionNotActive {
        /// Transaction identifier
        id: String,
    },

    /// Transaction exceeded its deadline
    #[error("Transaction timed out: {id}")]
    TransactionTimedOut {
        /// Transaction identifier
        id: String,
    },

    /// Write-write conflict between concurrent transactions
    #[error("Transaction conflict: {transaction} conflicts with {conflicting_with}")]
    TransactionConflict {
        /// Transaction that detected the conflict
        transaction: String,
        /// Transaction holding the conflicting lock
        conflicting_with: String,
    },

    /// Maximum concurrent transaction limit exceeded
    #[error("Too many transactions: maximum {max} concurrent transactions allowed")]
    TooManyTransactions {
        /// Maximum allowed concurrent transactions
        max: usize,
    },

    /// Timed out acquiring a resource lock
    #[error("Lock timeout for resource: {resource}")]
    LockTimeout {
        /// Name of the resource that could not be locked
        resource: String,
    },

    /// Circular lock dependency detected
    #[error("Deadlock detected for transaction: {transaction}")]
    DeadlockDetected {
        /// Transaction involved in the deadlock
        transaction: String,
    },

    /// Named savepoint does not exist
    #[error("Savepoint not found: {name}")]
    SavepointNotFound {
        /// Savepoint name
        name: String,
    },

    /// DML write-lock conflict — another writer holds the table/schema lease.
    /// Surfaces as pgwire SQLSTATE 55P03 (lock_not_available) / gRPC ABORTED /
    /// REST 409.
    #[error("DML lock conflict on {resource}")]
    DmlLockConflict {
        /// Locked resource description (e.g. "schema.table").
        resource: String,
        /// Pod holding the conflicting lease, if known.
        holder: Option<String>,
    },
}

// Custom implementation for io::Error conversion since it's not Clone/Serialize
// Note: The canonical StorageError from storage::error has its own From<io::Error> impl
impl From<io::Error> for ProximaDBError {
    fn from(err: io::Error) -> Self {
        ProximaDBError::Io(err.to_string())
    }
}
