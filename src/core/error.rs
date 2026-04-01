use thiserror::Error;

/// Top-level error type for all ProximaDB operations
#[derive(Error, Debug)]
pub enum VectorDBError {
    /// Error originating from a storage engine
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Error from the Raft consensus layer
    #[error("Consensus error: {0}")]
    Consensus(#[from] ConsensusError),

    /// Network or transport-level error
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),

    /// Error during query parsing or execution
    #[error("Query error: {0}")]
    Query(#[from] QueryError),

    /// Schema validation or compatibility error
    #[error("Schema error: {0}")]
    Schema(#[from] SchemaError),

    /// Invalid or missing configuration parameter
    #[error("Configuration error: {0}")]
    Config(String),

    /// Unexpected internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Error during vector quantization
    #[error("Quantization error: {0}")]
    Quantization(String),

    /// Client-supplied input failed validation
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Filesystem I/O or path resolution error
    #[error("Filesystem error: {0}")]
    Filesystem(String),

    /// Requested feature is not yet implemented
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Transaction with the given ID was not found
    #[error("Transaction not found: {id}")]
    TransactionNotFound {
        /// Transaction identifier
        id: String,
    },

    /// Transaction exists but is not in an active state
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
        /// Transaction that holds the conflicting lock
        conflicting_with: String,
    },

    /// Concurrency limit for active transactions exceeded
    #[error("Too many transactions: maximum {max} concurrent transactions allowed")]
    TooManyTransactions {
        /// Maximum allowed concurrent transactions
        max: usize,
    },

    /// Transaction was explicitly aborted
    #[error("Transaction aborted: {0}")]
    TransactionAborted(String),

    /// Timed out waiting to acquire a resource lock
    #[error("Lock timeout for resource: {resource}")]
    LockTimeout {
        /// Name of the resource that could not be locked
        resource: String,
    },

    /// Circular dependency detected in lock acquisition
    #[error("Deadlock detected for transaction: {transaction}")]
    DeadlockDetected {
        /// Transaction involved in the deadlock cycle
        transaction: String,
    },

    /// Savepoint with the given name does not exist
    #[error("Savepoint not found: {name}")]
    SavepointNotFound {
        /// Savepoint name
        name: String,
    },
}

/// Type alias for backward compatibility with older code using ProximaDBError
pub type ProximaDBError = VectorDBError;

// Add conversion from FilesystemError to VectorDBError
impl From<crate::storage::persistence::filesystem::FilesystemError> for VectorDBError {
    fn from(err: crate::storage::persistence::filesystem::FilesystemError) -> Self {
        VectorDBError::Storage(StorageError::DiskIO(std::io::Error::other(
            err.to_string(),
        )))
    }
}

/// Errors originating from storage engines and I/O operations
#[derive(Error, Debug)]
pub enum StorageError {
    /// Error from the SST (Sorted String Table) engine
    #[error("SST engine error: {0}")]
    SstEngine(String),

    /// Memory-mapped I/O failure
    #[error("MMAP error: {0}")]
    Mmap(String),

    /// Low-level disk I/O error
    #[error("Disk I/O error: {0}")]
    DiskIO(#[from] std::io::Error),

    /// Data serialization or encoding failure
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Data integrity check failed (checksum mismatch, etc.)
    #[error("Corruption detected: {0}")]
    Corruption(String),

    /// Attempted to create a resource that already exists
    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    /// Requested resource was not found
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Specific key lookup failed
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    /// Index-level error during build or lookup
    #[error("Index error: {0}")]
    IndexError(String),

    /// Write-ahead log operation failed
    #[error("WAL error: {0}")]
    WalError(String),

    /// Serialization format error (alternative variant)
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Metadata catalog error
    #[error("Metadata error: {0}")]
    MetadataError(#[from] anyhow::Error),

    /// Collection with the given name does not exist
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// Vector dimension mismatch between expected and actual
    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidDimension {
        /// Expected vector dimension for the collection
        expected: usize,
        /// Actual dimension of the provided vector
        actual: usize,
    },

    /// A durable write during transaction commit failed
    #[error("Transaction commit failed: {0}")]
    TransactionCommitFailed(String),
}

/// Errors from the Raft consensus layer
#[derive(Error, Debug)]
pub enum ConsensusError {
    /// Raft protocol-level failure
    #[error("Raft error: {0}")]
    Raft(String),

    /// Leadership election or forwarding failure
    #[error("Leadership error: {0}")]
    Leadership(String),

    /// Log replication failure
    #[error("Replication error: {0}")]
    Replication(String),
}

/// Errors from network transport (gRPC, HTTP)
#[derive(Error, Debug)]
pub enum NetworkError {
    /// gRPC status error
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    /// HTTP request or response error
    #[error("HTTP error: {0}")]
    Http(String),

    /// TCP/TLS connection failure
    #[error("Connection error: {0}")]
    Connection(String),
}

/// Errors during query parsing, planning, or execution
#[derive(Error, Debug)]
pub enum QueryError {
    /// Vector similarity search failure
    #[error("Vector search error: {0}")]
    VectorSearch(String),

    /// SQL statement parsing failure
    #[error("SQL parse error: {0}")]
    SqlParse(String),

    /// Query is semantically invalid
    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    /// Queried collection does not exist
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// Metadata filter expression is invalid
    #[error("Invalid filter: {0}")]
    InvalidFilter(String),
}

/// Errors related to schema definitions and validation
#[derive(Error, Debug)]
pub enum SchemaError {
    /// Schema definition is syntactically or structurally invalid
    #[error("Invalid schema: {0}")]
    InvalidSchema(String),

    /// Schema incompatibility between expected and actual
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),

    /// Data failed schema validation constraints
    #[error("Schema validation error: {0}")]
    Validation(String),
}

/// Unified protocol error type for consistent error handling across REST, gRPC, and Arrow IPC
///
/// This enum provides a single error representation that can be converted to protocol-specific
/// error types. Use this for errors that originate in business logic and need to be communicated
/// to clients through any protocol.
#[derive(Error, Debug, Clone)]
pub enum ProtocolError {
    /// Invalid argument provided by the client
    #[error("Invalid argument: {msg}")]
    InvalidArgument {
        /// Human-readable error description
        msg: String,
        /// Name of the invalid field, if applicable
        field: Option<String>,
    },

    /// Requested resource was not found
    #[error("{resource} not found: {id}")]
    NotFound {
        /// Type of resource (e.g., "Collection", "Vector")
        resource: String,
        /// Identifier of the missing resource
        id: String,
    },

    /// Resource already exists (conflict)
    #[error("{resource} already exists: {id}")]
    AlreadyExists {
        /// Type of conflicting resource
        resource: String,
        /// Identifier of the existing resource
        id: String,
    },

    /// Internal server error
    #[error("Internal error: {details}")]
    Internal {
        /// Diagnostic details about the internal failure
        details: String,
    },

    /// Permission denied for the requested action
    #[error("Permission denied: {action}")]
    PermissionDenied {
        /// Description of the denied action
        action: String,
    },

    /// Operation timed out
    #[error("Operation '{operation}' timed out after {duration_ms}ms")]
    Timeout {
        /// Name of the operation that timed out
        operation: String,
        /// Elapsed time in milliseconds before timeout
        duration_ms: u64,
    },

    /// Resource exhausted (rate limiting, quota exceeded)
    #[error("Resource exhausted: {details}")]
    ResourceExhausted {
        /// Description of which resource was exhausted
        details: String,
    },

    /// Precondition failed (e.g., version mismatch)
    #[error("Precondition failed: {details}")]
    PreconditionFailed {
        /// Description of the failed precondition
        details: String,
    },
}

impl ProtocolError {
    /// Create an InvalidArgument error
    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Self::InvalidArgument {
            msg: msg.into(),
            field: None,
        }
    }

    /// Create an InvalidArgument error with field name
    pub fn invalid_argument_field(msg: impl Into<String>, field: impl Into<String>) -> Self {
        Self::InvalidArgument {
            msg: msg.into(),
            field: Some(field.into()),
        }
    }

    /// Create a NotFound error
    pub fn not_found(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
            id: id.into(),
        }
    }

    /// Create an AlreadyExists error
    pub fn already_exists(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::AlreadyExists {
            resource: resource.into(),
            id: id.into(),
        }
    }

    /// Create an Internal error
    pub fn internal(details: impl Into<String>) -> Self {
        Self::Internal {
            details: details.into(),
        }
    }

    /// Create a PermissionDenied error
    pub fn permission_denied(action: impl Into<String>) -> Self {
        Self::PermissionDenied {
            action: action.into(),
        }
    }

    /// Create a Timeout error
    pub fn timeout(operation: impl Into<String>, duration_ms: u64) -> Self {
        Self::Timeout {
            operation: operation.into(),
            duration_ms,
        }
    }

    /// Create a ResourceExhausted error
    pub fn resource_exhausted(details: impl Into<String>) -> Self {
        Self::ResourceExhausted {
            details: details.into(),
        }
    }

    /// Create a PreconditionFailed error
    pub fn precondition_failed(details: impl Into<String>) -> Self {
        Self::PreconditionFailed {
            details: details.into(),
        }
    }
}

/// Convert ProtocolError to gRPC tonic::Status
impl From<ProtocolError> for tonic::Status {
    fn from(err: ProtocolError) -> Self {
        match err {
            ProtocolError::InvalidArgument { msg, field } => {
                let message = if let Some(f) = field {
                    format!("{} (field: {})", msg, f)
                } else {
                    msg
                };
                tonic::Status::invalid_argument(message)
            }
            ProtocolError::NotFound { resource, id } => {
                tonic::Status::not_found(format!("{} not found: {}", resource, id))
            }
            ProtocolError::AlreadyExists { resource, id } => {
                tonic::Status::already_exists(format!("{} already exists: {}", resource, id))
            }
            ProtocolError::Internal { details } => tonic::Status::internal(details),
            ProtocolError::PermissionDenied { action } => {
                tonic::Status::permission_denied(format!("Permission denied: {}", action))
            }
            ProtocolError::Timeout {
                operation,
                duration_ms,
            } => tonic::Status::deadline_exceeded(format!(
                "Operation '{}' timed out after {}ms",
                operation, duration_ms
            )),
            ProtocolError::ResourceExhausted { details } => {
                tonic::Status::resource_exhausted(details)
            }
            ProtocolError::PreconditionFailed { details } => {
                tonic::Status::failed_precondition(details)
            }
        }
    }
}

impl ProtocolError {
    /// Convert to anyhow::Error (for Arrow IPC and other contexts)
    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!("{}", self)
    }
}

/// Convert from common error types to ProtocolError
impl From<StorageError> for ProtocolError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound(msg) => ProtocolError::not_found("Resource", msg),
            StorageError::CollectionNotFound(id) => ProtocolError::not_found("Collection", id),
            StorageError::AlreadyExists(msg) => ProtocolError::already_exists("Resource", msg),
            StorageError::InvalidDimension { expected, actual } => {
                ProtocolError::invalid_argument(format!(
                    "Invalid vector dimension: expected {}, got {}",
                    expected, actual
                ))
            }
            _ => ProtocolError::internal(err.to_string()),
        }
    }
}

impl From<QueryError> for ProtocolError {
    fn from(err: QueryError) -> Self {
        match err {
            QueryError::CollectionNotFound(id) => ProtocolError::not_found("Collection", id),
            QueryError::InvalidQuery(msg) => ProtocolError::invalid_argument(msg),
            QueryError::InvalidFilter(msg) => ProtocolError::invalid_argument_field(msg, "filter"),
            _ => ProtocolError::internal(err.to_string()),
        }
    }
}

// ── Bridge conversions: legacy error types → canonical core::errors types ──
// These allow code using the legacy VectorDBError/StorageError to convert
// to the canonical ProximaDBError without losing context.

impl From<StorageError> for crate::core::errors::StorageError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::SstEngine(msg) => crate::core::errors::StorageError::DiskIO(format!("SST engine: {}", msg)),
            StorageError::Mmap(msg) => crate::core::errors::StorageError::DiskIO(format!("MMAP: {}", msg)),
            StorageError::DiskIO(io_err) => crate::core::errors::StorageError::DiskIO(io_err.to_string()),
            StorageError::Serialization(msg) | StorageError::SerializationError(msg) => {
                crate::core::errors::StorageError::DiskIO(format!("Serialization: {}", msg))
            }
            StorageError::Corruption(msg) => crate::core::errors::StorageError::Corruption(msg),
            StorageError::AlreadyExists(msg) => crate::core::errors::StorageError::InvalidOperation(format!("Already exists: {}", msg)),
            StorageError::NotFound(msg) | StorageError::KeyNotFound(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!("Not found: {}", msg))
            }
            StorageError::IndexError(msg) => crate::core::errors::StorageError::InvalidOperation(format!("Index: {}", msg)),
            StorageError::WalError(msg) => crate::core::errors::StorageError::WAL(msg),
            StorageError::MetadataError(err) => crate::core::errors::StorageError::Metadata(err.to_string()),
            StorageError::CollectionNotFound(id) => crate::core::errors::StorageError::InvalidOperation(format!("Collection not found: {}", id)),
            StorageError::InvalidDimension { expected, actual } => {
                crate::core::errors::StorageError::InvalidOperation(format!("Dimension mismatch: expected {}, got {}", expected, actual))
            }
            StorageError::TransactionCommitFailed(msg) => {
                crate::core::errors::StorageError::InvalidOperation(format!("Transaction commit failed: {}", msg))
            }
        }
    }
}

impl From<ProtocolError> for crate::core::errors::ProximaDBError {
    fn from(err: ProtocolError) -> Self {
        match err {
            ProtocolError::InvalidArgument { msg, .. } => crate::core::errors::ProximaDBError::InvalidInput(msg),
            ProtocolError::NotFound { resource, id } => crate::core::errors::ProximaDBError::NotFound { resource_type: resource, id },
            ProtocolError::AlreadyExists { resource, id } => crate::core::errors::ProximaDBError::AlreadyExists { resource_type: resource, id },
            ProtocolError::Internal { details } => crate::core::errors::ProximaDBError::Internal(details),
            ProtocolError::PermissionDenied { action } => crate::core::errors::ProximaDBError::PermissionDenied(action),
            ProtocolError::Timeout { duration_ms, .. } => crate::core::errors::ProximaDBError::Timeout(duration_ms / 1000),
            ProtocolError::ResourceExhausted { details } => crate::core::errors::ProximaDBError::CapacityExceeded { message: details },
            ProtocolError::PreconditionFailed { details } => crate::core::errors::ProximaDBError::InvalidInput(details),
        }
    }
}

impl From<VectorDBError> for crate::core::errors::ProximaDBError {
    fn from(err: VectorDBError) -> Self {
        match err {
            VectorDBError::Storage(e) => crate::core::errors::ProximaDBError::Storage(e.into()),
            VectorDBError::Config(msg) => crate::core::errors::ProximaDBError::Internal(format!("Config: {}", msg)),
            VectorDBError::Internal(msg) => crate::core::errors::ProximaDBError::Internal(msg),
            VectorDBError::Quantization(msg) => crate::core::errors::ProximaDBError::Quantization(msg),
            VectorDBError::InvalidInput(msg) => crate::core::errors::ProximaDBError::InvalidInput(msg),
            VectorDBError::Filesystem(msg) => crate::core::errors::ProximaDBError::Io(msg),
            VectorDBError::NotImplemented(msg) => crate::core::errors::ProximaDBError::Internal(format!("Not implemented: {}", msg)),
            VectorDBError::Consensus(e) => crate::core::errors::ProximaDBError::Network(e.to_string()),
            VectorDBError::Network(e) => crate::core::errors::ProximaDBError::Network(e.to_string()),
            VectorDBError::Query(e) => crate::core::errors::ProximaDBError::Internal(e.to_string()),
            VectorDBError::Schema(e) => crate::core::errors::ProximaDBError::Internal(e.to_string()),
            VectorDBError::TransactionNotFound { id } => crate::core::errors::ProximaDBError::TransactionNotFound { id },
            VectorDBError::TransactionNotActive { id } => crate::core::errors::ProximaDBError::TransactionNotActive { id },
            VectorDBError::TransactionTimedOut { id } => crate::core::errors::ProximaDBError::TransactionTimedOut { id },
            VectorDBError::TransactionConflict { transaction, conflicting_with } => {
                crate::core::errors::ProximaDBError::TransactionConflict { transaction, conflicting_with }
            }
            VectorDBError::TooManyTransactions { max } => crate::core::errors::ProximaDBError::TooManyTransactions { max },
            VectorDBError::TransactionAborted(msg) => crate::core::errors::ProximaDBError::Internal(format!("Transaction aborted: {}", msg)),
            VectorDBError::LockTimeout { resource } => crate::core::errors::ProximaDBError::LockTimeout { resource },
            VectorDBError::DeadlockDetected { transaction } => crate::core::errors::ProximaDBError::DeadlockDetected { transaction },
            VectorDBError::SavepointNotFound { name } => crate::core::errors::ProximaDBError::SavepointNotFound { name },
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
