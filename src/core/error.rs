use thiserror::Error;

#[derive(Error, Debug)]
pub enum VectorDBError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("Consensus error: {0}")]
    Consensus(#[from] ConsensusError),

    #[error("Network error: {0}")]
    Network(#[from] NetworkError),

    #[error("Query error: {0}")]
    Query(#[from] QueryError),

    #[error("Schema error: {0}")]
    Schema(#[from] SchemaError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Quantization error: {0}")]
    Quantization(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Filesystem error: {0}")]
    Filesystem(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

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

// Type alias for backward compatibility
pub type ProximaDBError = VectorDBError;

// Add conversion from FilesystemError to VectorDBError
impl From<crate::storage::persistence::filesystem::FilesystemError> for VectorDBError {
    fn from(err: crate::storage::persistence::filesystem::FilesystemError) -> Self {
        VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(
            std::io::ErrorKind::Other,
            err.to_string(),
        )))
    }
}

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("SST engine error: {0}")]
    SstEngine(String),

    #[error("MMAP error: {0}")]
    Mmap(String),

    #[error("Disk I/O error: {0}")]
    DiskIO(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Corruption detected: {0}")]
    Corruption(String),

    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Index error: {0}")]
    IndexError(String),

    #[error("WAL error: {0}")]
    WalError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Metadata error: {0}")]
    MetadataError(#[from] anyhow::Error),

    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidDimension { expected: usize, actual: usize },
}

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Raft error: {0}")]
    Raft(String),

    #[error("Leadership error: {0}")]
    Leadership(String),

    #[error("Replication error: {0}")]
    Replication(String),
}

#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Connection error: {0}")]
    Connection(String),
}

#[derive(Error, Debug)]
pub enum QueryError {
    #[error("Vector search error: {0}")]
    VectorSearch(String),

    #[error("SQL parse error: {0}")]
    SqlParse(String),

    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Invalid filter: {0}")]
    InvalidFilter(String),
}

#[derive(Error, Debug)]
pub enum SchemaError {
    #[error("Invalid schema: {0}")]
    InvalidSchema(String),

    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),

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
    InvalidArgument { msg: String, field: Option<String> },

    /// Requested resource was not found
    #[error("{resource} not found: {id}")]
    NotFound { resource: String, id: String },

    /// Resource already exists (conflict)
    #[error("{resource} already exists: {id}")]
    AlreadyExists { resource: String, id: String },

    /// Internal server error
    #[error("Internal error: {details}")]
    Internal { details: String },

    /// Permission denied for the requested action
    #[error("Permission denied: {action}")]
    PermissionDenied { action: String },

    /// Operation timed out
    #[error("Operation '{operation}' timed out after {duration_ms}ms")]
    Timeout { operation: String, duration_ms: u64 },

    /// Resource exhausted (rate limiting, quota exceeded)
    #[error("Resource exhausted: {details}")]
    ResourceExhausted { details: String },

    /// Precondition failed (e.g., version mismatch)
    #[error("Precondition failed: {details}")]
    PreconditionFailed { details: String },
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

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
