use thiserror::Error;

/// Standard result type for ProximaDB kernel contracts.
pub type Result<T, E = ProximaDBError> = std::result::Result<T, E>;

/// Top-level error type for all ProximaDB operations.
#[derive(Error, Debug)]
pub enum VectorDBError {
    /// Error originating from a storage engine.
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Error from the Raft consensus layer.
    #[error("Consensus error: {0}")]
    Consensus(#[from] ConsensusError),

    /// Network or transport-level error.
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),

    /// Error during query parsing or execution.
    #[error("Query error: {0}")]
    Query(#[from] QueryError),

    /// Schema validation or compatibility error.
    #[error("Schema error: {0}")]
    Schema(#[from] SchemaError),

    /// Invalid or missing configuration parameter.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Unexpected internal error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// Error during vector quantization.
    #[error("Quantization error: {0}")]
    Quantization(String),

    /// Client-supplied input failed validation.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Filesystem I/O or path resolution error.
    #[error("Filesystem error: {0}")]
    Filesystem(String),

    /// Requested feature is not yet implemented.
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Transaction with the given ID was not found.
    #[error("Transaction not found: {id}")]
    TransactionNotFound { id: String },

    /// Transaction exists but is not in an active state.
    #[error("Transaction not active: {id}")]
    TransactionNotActive { id: String },

    /// Transaction exceeded its deadline.
    #[error("Transaction timed out: {id}")]
    TransactionTimedOut { id: String },

    /// Write-write conflict between concurrent transactions.
    #[error("Transaction conflict: {transaction} conflicts with {conflicting_with}")]
    TransactionConflict {
        transaction: String,
        conflicting_with: String,
    },

    /// Concurrency limit for active transactions exceeded.
    #[error("Too many transactions: maximum {max} concurrent transactions allowed")]
    TooManyTransactions { max: usize },

    /// Transaction was explicitly aborted.
    #[error("Transaction aborted: {0}")]
    TransactionAborted(String),

    /// Timed out waiting to acquire a resource lock.
    #[error("Lock timeout for resource: {resource}")]
    LockTimeout { resource: String },

    /// Circular dependency detected in lock acquisition.
    #[error("Deadlock detected for transaction: {transaction}")]
    DeadlockDetected { transaction: String },

    /// Savepoint with the given name does not exist.
    #[error("Savepoint not found: {name}")]
    SavepointNotFound { name: String },
}

/// Backward-compatible alias used broadly across the current tree.
pub type ProximaDBError = VectorDBError;

/// Errors originating from storage engines and I/O operations.
#[derive(Error, Debug)]
pub enum StorageError {
    /// Error from the SST (Sorted String Table) engine.
    #[error("SST engine error: {0}")]
    SstEngine(String),

    /// Memory-mapped I/O failure.
    #[error("MMAP error: {0}")]
    Mmap(String),

    /// Low-level disk I/O error.
    #[error("Disk I/O error: {0}")]
    DiskIO(#[from] std::io::Error),

    /// Data serialization or encoding failure.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Data integrity check failed.
    #[error("Corruption detected: {0}")]
    Corruption(String),

    /// Attempted to create a resource that already exists.
    #[error("Resource already exists: {0}")]
    AlreadyExists(String),

    /// Requested resource was not found.
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Specific key lookup failed.
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    /// Index-level error during build or lookup.
    #[error("Index error: {0}")]
    IndexError(String),

    /// Write-ahead log operation failed.
    #[error("WAL error: {0}")]
    WalError(String),

    /// Serialization format error (alternative variant).
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Metadata catalog error.
    #[error("Metadata error: {0}")]
    MetadataError(#[from] anyhow::Error),

    /// Collection with the given name does not exist.
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// Vector dimension mismatch between expected and actual.
    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidDimension { expected: usize, actual: usize },

    /// A durable write during transaction commit failed.
    #[error("Transaction commit failed: {0}")]
    TransactionCommitFailed(String),
}

/// Errors from the Raft consensus layer.
#[derive(Error, Debug)]
pub enum ConsensusError {
    /// Raft protocol-level failure.
    #[error("Raft error: {0}")]
    Raft(String),

    /// Leadership election or forwarding failure.
    #[error("Leadership error: {0}")]
    Leadership(String),

    /// Log replication failure.
    #[error("Replication error: {0}")]
    Replication(String),
}

/// Errors from network transport (gRPC, HTTP).
#[derive(Error, Debug)]
pub enum NetworkError {
    /// gRPC status error.
    #[error("gRPC error: {0}")]
    Grpc(String),

    /// HTTP request or response error.
    #[error("HTTP error: {0}")]
    Http(String),

    /// TCP/TLS connection failure.
    #[error("Connection error: {0}")]
    Connection(String),
}

/// Errors during query parsing, planning, or execution.
#[derive(Error, Debug)]
pub enum QueryError {
    /// Vector similarity search failure.
    #[error("Vector search error: {0}")]
    VectorSearch(String),

    /// SQL statement parsing failure.
    #[error("SQL parse error: {0}")]
    SqlParse(String),

    /// Query is semantically invalid.
    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    /// Queried collection does not exist.
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// Metadata filter expression is invalid.
    #[error("Invalid filter: {0}")]
    InvalidFilter(String),
}

/// Errors related to schema definitions and validation.
#[derive(Error, Debug)]
pub enum SchemaError {
    /// Schema definition is invalid.
    #[error("Invalid schema: {0}")]
    InvalidSchema(String),

    /// Schema incompatibility between expected and actual.
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),

    /// Data failed schema validation constraints.
    #[error("Schema validation error: {0}")]
    Validation(String),
}

/// Unified protocol error type for consistent error handling across REST, gRPC, and Arrow IPC.
#[derive(Error, Debug, Clone)]
pub enum ProtocolError {
    /// Invalid argument provided by the client.
    #[error("Invalid argument: {msg}")]
    InvalidArgument { msg: String, field: Option<String> },

    /// Requested resource was not found.
    #[error("{resource} not found: {id}")]
    NotFound { resource: String, id: String },

    /// Resource already exists.
    #[error("{resource} already exists: {id}")]
    AlreadyExists { resource: String, id: String },

    /// Internal server error.
    #[error("Internal error: {details}")]
    Internal { details: String },

    /// Permission denied for the requested action.
    #[error("Permission denied: {action}")]
    PermissionDenied { action: String },

    /// Operation timed out.
    #[error("Operation '{operation}' timed out after {duration_ms}ms")]
    Timeout { operation: String, duration_ms: u64 },

    /// Resource exhausted (rate limiting, quota exceeded).
    #[error("Resource exhausted: {details}")]
    ResourceExhausted { details: String },

    /// Precondition failed (e.g. version mismatch).
    #[error("Precondition failed: {details}")]
    PreconditionFailed { details: String },
}

impl ProtocolError {
    /// Create an invalid-argument error.
    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Self::InvalidArgument {
            msg: msg.into(),
            field: None,
        }
    }

    /// Create an invalid-argument error with field name.
    pub fn invalid_argument_field(msg: impl Into<String>, field: impl Into<String>) -> Self {
        Self::InvalidArgument {
            msg: msg.into(),
            field: Some(field.into()),
        }
    }

    /// Create a not-found error.
    pub fn not_found(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            resource: resource.into(),
            id: id.into(),
        }
    }

    /// Create an already-exists error.
    pub fn already_exists(resource: impl Into<String>, id: impl Into<String>) -> Self {
        Self::AlreadyExists {
            resource: resource.into(),
            id: id.into(),
        }
    }

    /// Create an internal error.
    pub fn internal(details: impl Into<String>) -> Self {
        Self::Internal {
            details: details.into(),
        }
    }

    /// Create a permission-denied error.
    pub fn permission_denied(action: impl Into<String>) -> Self {
        Self::PermissionDenied {
            action: action.into(),
        }
    }

    /// Create a timeout error.
    pub fn timeout(operation: impl Into<String>, duration_ms: u64) -> Self {
        Self::Timeout {
            operation: operation.into(),
            duration_ms,
        }
    }

    /// Create a resource-exhausted error.
    pub fn resource_exhausted(details: impl Into<String>) -> Self {
        Self::ResourceExhausted {
            details: details.into(),
        }
    }

    /// Create a precondition-failed error.
    pub fn precondition_failed(details: impl Into<String>) -> Self {
        Self::PreconditionFailed {
            details: details.into(),
        }
    }

    /// Convert to anyhow::Error for non-transport callers.
    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!("{}", self)
    }
}

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
