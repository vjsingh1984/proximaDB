//! Unified Storage Error Types
//!
//! Provides a comprehensive error type hierarchy for all storage-related operations
//! in ProximaDB. This unifies error handling across:
//! - WAL operations (write, flush, recovery)
//! - Storage engines (SST, VIPER, HELIX, NOVA, SWIFT, RAPTOR)
//! - Filesystem operations (local, S3, Azure, GCS)
//! - Compaction and optimization
//! - Backup and recovery
//!
//! # Error Categories
//!
//! Errors are organized into logical categories:
//! - **IO Errors**: Filesystem and network I/O failures
//! - **Corruption Errors**: Data integrity issues
//! - **Capacity Errors**: Resource limits exceeded
//! - **Configuration Errors**: Invalid configuration
//! - **Operation Errors**: Failed operations (compaction, flush, etc.)
//! - **Concurrency Errors**: Lock conflicts and race conditions
//!
//! # Usage
//!
//! ```ignore
//! use proximadb::storage::error::{StorageError, StorageErrorKind};
//!
//! fn read_data() -> Result<Vec<u8>, StorageError> {
//!     // ... operation that might fail
//!     Err(StorageError::io("Failed to read file", Some(io_error)))
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;

/// The main storage error type that encompasses all storage-related errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageError {
    /// The kind of error that occurred
    pub kind: StorageErrorKind,
    /// Human-readable error message
    pub message: String,
    /// Optional source error message (extracted from source error)
    pub source: Option<String>,
    /// Error context (file path, collection ID, etc.)
    pub context: ErrorContext,
}

/// Error context providing additional information about where the error occurred
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorContext {
    /// File path involved (if any)
    pub file_path: Option<String>,
    /// Collection ID involved (if any)
    pub collection_id: Option<String>,
    /// LSN at the time of error (if applicable)
    pub lsn: Option<u64>,
    /// Operation that was being performed
    pub operation: Option<String>,
    /// Storage engine type
    pub engine: Option<String>,
    /// Additional key-value context
    pub extra: std::collections::HashMap<String, String>,
}

impl ErrorContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file path to the context
    pub fn with_file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    /// Add a collection ID to the context
    pub fn with_collection(mut self, collection_id: impl Into<String>) -> Self {
        self.collection_id = Some(collection_id.into());
        self
    }

    /// Add an LSN to the context
    pub fn with_lsn(mut self, lsn: u64) -> Self {
        self.lsn = Some(lsn);
        self
    }

    /// Add an operation name to the context
    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    /// Add an engine type to the context
    pub fn with_engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = Some(engine.into());
        self
    }

    /// Add extra context
    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// Categories of storage errors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StorageErrorKind {
    // === I/O Errors ===
    /// Generic I/O error
    Io,
    /// File not found
    NotFound,
    /// Permission denied
    PermissionDenied,
    /// Disk full or quota exceeded
    DiskFull,
    /// Network error (for cloud storage)
    Network,
    /// Timeout during I/O operation
    Timeout,

    // === Corruption Errors ===
    /// Data corruption detected (checksum mismatch, etc.)
    Corruption,
    /// WAL corruption
    WalCorruption,
    /// Index corruption
    IndexCorruption,
    /// Manifest corruption
    ManifestCorruption,

    // === Capacity Errors ===
    /// Capacity exceeded (memory, disk, etc.)
    CapacityExceeded,
    /// Too many open files
    TooManyOpenFiles,
    /// Memory limit exceeded
    MemoryLimitExceeded,
    /// Queue full
    QueueFull,

    // === Configuration Errors ===
    /// Invalid configuration
    InvalidConfiguration,
    /// Missing required configuration
    MissingConfiguration,
    /// Incompatible configuration
    IncompatibleConfiguration,

    // === Operation Errors ===
    /// Compaction failed
    CompactionFailed,
    /// Flush failed
    FlushFailed,
    /// Recovery failed
    RecoveryFailed,
    /// Backup failed
    BackupFailed,
    /// Restore failed
    RestoreFailed,
    /// Operation not supported
    NotSupported,
    /// Operation was canceled
    Canceled,

    // === Concurrency Errors ===
    /// Lock acquisition failed
    LockFailed,
    /// Deadlock detected
    Deadlock,
    /// Conflict with concurrent operation
    Conflict,
    /// Stale read (MVCC violation)
    StaleRead,

    // === Engine-Specific Errors ===
    /// SST engine error
    SstEngine,
    /// VIPER engine error
    ViperEngine,
    /// HELIX engine error
    HelixEngine,
    /// NOVA engine error
    NovaEngine,
    /// SWIFT engine error
    SwiftEngine,
    /// RAPTOR engine error
    RaptorEngine,

    // === Other ===
    /// Internal error (bug)
    Internal,
    /// Unknown error
    Unknown,
}

impl StorageErrorKind {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            StorageErrorKind::Io
                | StorageErrorKind::Network
                | StorageErrorKind::Timeout
                | StorageErrorKind::LockFailed
                | StorageErrorKind::Conflict
                | StorageErrorKind::QueueFull
        )
    }

    /// Check if this error indicates data corruption
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            StorageErrorKind::Corruption
                | StorageErrorKind::WalCorruption
                | StorageErrorKind::IndexCorruption
                | StorageErrorKind::ManifestCorruption
        )
    }

    /// Check if this error indicates a resource limit
    pub fn is_resource_limit(&self) -> bool {
        matches!(
            self,
            StorageErrorKind::CapacityExceeded
                | StorageErrorKind::DiskFull
                | StorageErrorKind::TooManyOpenFiles
                | StorageErrorKind::MemoryLimitExceeded
                | StorageErrorKind::QueueFull
        )
    }

    /// Get the HTTP status code equivalent
    pub fn http_status_code(&self) -> u16 {
        match self {
            StorageErrorKind::NotFound => 404,
            StorageErrorKind::PermissionDenied => 403,
            StorageErrorKind::InvalidConfiguration | StorageErrorKind::MissingConfiguration => 400,
            StorageErrorKind::Conflict | StorageErrorKind::LockFailed => 409,
            StorageErrorKind::CapacityExceeded
            | StorageErrorKind::DiskFull
            | StorageErrorKind::MemoryLimitExceeded
            | StorageErrorKind::QueueFull => 507,
            StorageErrorKind::NotSupported => 501,
            StorageErrorKind::Timeout => 504,
            _ => 500,
        }
    }
}

impl fmt::Display for StorageErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageErrorKind::Io => write!(f, "I/O error"),
            StorageErrorKind::NotFound => write!(f, "not found"),
            StorageErrorKind::PermissionDenied => write!(f, "permission denied"),
            StorageErrorKind::DiskFull => write!(f, "disk full"),
            StorageErrorKind::Network => write!(f, "network error"),
            StorageErrorKind::Timeout => write!(f, "timeout"),
            StorageErrorKind::Corruption => write!(f, "data corruption"),
            StorageErrorKind::WalCorruption => write!(f, "WAL corruption"),
            StorageErrorKind::IndexCorruption => write!(f, "index corruption"),
            StorageErrorKind::ManifestCorruption => write!(f, "manifest corruption"),
            StorageErrorKind::CapacityExceeded => write!(f, "capacity exceeded"),
            StorageErrorKind::TooManyOpenFiles => write!(f, "too many open files"),
            StorageErrorKind::MemoryLimitExceeded => write!(f, "memory limit exceeded"),
            StorageErrorKind::QueueFull => write!(f, "queue full"),
            StorageErrorKind::InvalidConfiguration => write!(f, "invalid configuration"),
            StorageErrorKind::MissingConfiguration => write!(f, "missing configuration"),
            StorageErrorKind::IncompatibleConfiguration => write!(f, "incompatible configuration"),
            StorageErrorKind::CompactionFailed => write!(f, "compaction failed"),
            StorageErrorKind::FlushFailed => write!(f, "flush failed"),
            StorageErrorKind::RecoveryFailed => write!(f, "recovery failed"),
            StorageErrorKind::BackupFailed => write!(f, "backup failed"),
            StorageErrorKind::RestoreFailed => write!(f, "restore failed"),
            StorageErrorKind::NotSupported => write!(f, "not supported"),
            StorageErrorKind::Canceled => write!(f, "canceled"),
            StorageErrorKind::LockFailed => write!(f, "lock acquisition failed"),
            StorageErrorKind::Deadlock => write!(f, "deadlock detected"),
            StorageErrorKind::Conflict => write!(f, "conflict"),
            StorageErrorKind::StaleRead => write!(f, "stale read"),
            StorageErrorKind::SstEngine => write!(f, "SST engine error"),
            StorageErrorKind::ViperEngine => write!(f, "VIPER engine error"),
            StorageErrorKind::HelixEngine => write!(f, "HELIX engine error"),
            StorageErrorKind::NovaEngine => write!(f, "NOVA engine error"),
            StorageErrorKind::SwiftEngine => write!(f, "SWIFT engine error"),
            StorageErrorKind::RaptorEngine => write!(f, "RAPTOR engine error"),
            StorageErrorKind::Internal => write!(f, "internal error"),
            StorageErrorKind::Unknown => write!(f, "unknown error"),
        }
    }
}

impl StorageError {
    /// Create a new storage error
    pub fn new(kind: StorageErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
            context: ErrorContext::default(),
        }
    }

    /// Create a new storage error with a source
    pub fn with_source(
        kind: StorageErrorKind,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(source.to_string()),
            context: ErrorContext::default(),
        }
    }

    /// Add context to the error
    pub fn with_context(mut self, context: ErrorContext) -> Self {
        self.context = context;
        self
    }

    // === Convenience constructors ===

    /// Create an I/O error
    pub fn io(message: impl Into<String>, source: Option<io::Error>) -> Self {
        let mut err = Self::new(StorageErrorKind::Io, message);
        if let Some(s) = source {
            err.source = Some(s.to_string());
        }
        err
    }

    /// Create a not found error
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::NotFound, message)
    }

    /// Create a corruption error
    pub fn corruption(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Corruption, message)
    }

    /// Create a WAL corruption error
    pub fn wal_corruption(message: impl Into<String>, lsn: Option<u64>) -> Self {
        let mut err = Self::new(StorageErrorKind::WalCorruption, message);
        if let Some(l) = lsn {
            err.context.lsn = Some(l);
        }
        err
    }

    /// Create a capacity exceeded error
    pub fn capacity_exceeded(current: u64, limit: u64) -> Self {
        Self::new(
            StorageErrorKind::CapacityExceeded,
            format!("Capacity exceeded: {}/{}", current, limit),
        )
    }

    /// Create a flush failed error
    pub fn flush_failed(collection_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let msg = reason.into();
        Self::new(StorageErrorKind::FlushFailed, msg.clone()).with_context(
            ErrorContext::new()
                .with_collection(collection_id)
                .with_operation("flush"),
        )
    }

    /// Create a compaction failed error
    pub fn compaction_failed(collection_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let msg = reason.into();
        Self::new(StorageErrorKind::CompactionFailed, msg.clone()).with_context(
            ErrorContext::new()
                .with_collection(collection_id)
                .with_operation("compaction"),
        )
    }

    /// Create a recovery failed error
    pub fn recovery_failed(reason: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::RecoveryFailed, reason)
            .with_context(ErrorContext::new().with_operation("recovery"))
    }

    /// Create a backup failed error
    pub fn backup_failed(reason: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::BackupFailed, reason)
            .with_context(ErrorContext::new().with_operation("backup"))
    }

    /// Create a lock failed error
    pub fn lock_failed(resource: impl Into<String>) -> Self {
        Self::new(
            StorageErrorKind::LockFailed,
            format!("Failed to acquire lock on: {}", resource.into()),
        )
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Internal, message)
    }

    /// Create an engine-specific error
    pub fn engine_error(engine: &str, message: impl Into<String>) -> Self {
        let kind = match engine.to_uppercase().as_str() {
            "SST" => StorageErrorKind::SstEngine,
            "VIPER" => StorageErrorKind::ViperEngine,
            "HELIX" => StorageErrorKind::HelixEngine,
            "NOVA" => StorageErrorKind::NovaEngine,
            "SWIFT" => StorageErrorKind::SwiftEngine,
            "RAPTOR" => StorageErrorKind::RaptorEngine,
            _ => StorageErrorKind::Unknown,
        };
        Self::new(kind, message).with_context(ErrorContext::new().with_engine(engine))
    }

    // === Accessors ===

    /// Get the error kind
    pub fn kind(&self) -> StorageErrorKind {
        self.kind
    }

    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Check if the error indicates corruption
    pub fn is_corruption(&self) -> bool {
        self.kind.is_corruption()
    }

    /// Get HTTP status code
    pub fn http_status_code(&self) -> u16 {
        self.kind.http_status_code()
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)?;

        // Add source if available
        if let Some(ref source) = self.source {
            write!(f, " caused by: {}", source)?;
        }

        // Add context if available
        if let Some(ref path) = self.context.file_path {
            write!(f, " (file: {})", path)?;
        }
        if let Some(ref collection) = self.context.collection_id {
            write!(f, " (collection: {})", collection)?;
        }
        if let Some(lsn) = self.context.lsn {
            write!(f, " (LSN: {})", lsn)?;
        }

        Ok(())
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Since source is now a String, we can't return it as an Error
        // The source information is preserved in the source field as a String
        None
    }
}

// === Conversions from other error types ===

impl From<io::Error> for StorageError {
    fn from(err: io::Error) -> Self {
        let kind = match err.kind() {
            io::ErrorKind::NotFound => StorageErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => StorageErrorKind::PermissionDenied,
            io::ErrorKind::TimedOut => StorageErrorKind::Timeout,
            io::ErrorKind::WouldBlock => StorageErrorKind::LockFailed,
            _ => StorageErrorKind::Io,
        };
        Self::with_source(kind, err.to_string(), err)
    }
}

impl From<anyhow::Error> for StorageError {
    fn from(err: anyhow::Error) -> Self {
        // Try to extract the original error type
        if let Some(storage_err) = err.downcast_ref::<StorageError>() {
            return StorageError::new(storage_err.kind, storage_err.message.clone())
                .with_context(storage_err.context.clone());
        }

        Self::new(StorageErrorKind::Unknown, err.to_string())
    }
}

/// Result type alias for storage operations
pub type StorageResult<T> = Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_error_creation() {
        let err = StorageError::new(StorageErrorKind::NotFound, "File not found");
        assert_eq!(err.kind, StorageErrorKind::NotFound);
        assert_eq!(err.message, "File not found");
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_storage_error_with_context() {
        let err = StorageError::corruption("Checksum mismatch").with_context(
            ErrorContext::new()
                .with_file_path("/tmp/data/sst/001.sst")
                .with_collection("test_collection")
                .with_lsn(12345),
        );

        assert_eq!(err.kind, StorageErrorKind::Corruption);
        assert!(err.context.file_path.is_some());
        assert_eq!(
            err.context.file_path.as_deref(),
            Some("/tmp/data/sst/001.sst")
        );
        assert_eq!(err.context.lsn, Some(12345));
    }

    #[test]
    fn test_retryable_errors() {
        assert!(StorageErrorKind::Io.is_retryable());
        assert!(StorageErrorKind::Network.is_retryable());
        assert!(StorageErrorKind::Timeout.is_retryable());
        assert!(StorageErrorKind::LockFailed.is_retryable());

        assert!(!StorageErrorKind::Corruption.is_retryable());
        assert!(!StorageErrorKind::NotFound.is_retryable());
        assert!(!StorageErrorKind::InvalidConfiguration.is_retryable());
    }

    #[test]
    fn test_corruption_errors() {
        assert!(StorageErrorKind::Corruption.is_corruption());
        assert!(StorageErrorKind::WalCorruption.is_corruption());
        assert!(StorageErrorKind::IndexCorruption.is_corruption());
        assert!(StorageErrorKind::ManifestCorruption.is_corruption());

        assert!(!StorageErrorKind::Io.is_corruption());
        assert!(!StorageErrorKind::NotFound.is_corruption());
    }

    #[test]
    fn test_resource_limit_errors() {
        assert!(StorageErrorKind::CapacityExceeded.is_resource_limit());
        assert!(StorageErrorKind::DiskFull.is_resource_limit());
        assert!(StorageErrorKind::MemoryLimitExceeded.is_resource_limit());

        assert!(!StorageErrorKind::Io.is_resource_limit());
    }

    #[test]
    fn test_http_status_codes() {
        assert_eq!(StorageErrorKind::NotFound.http_status_code(), 404);
        assert_eq!(StorageErrorKind::PermissionDenied.http_status_code(), 403);
        assert_eq!(StorageErrorKind::CapacityExceeded.http_status_code(), 507);
        assert_eq!(StorageErrorKind::Conflict.http_status_code(), 409);
        assert_eq!(StorageErrorKind::NotSupported.http_status_code(), 501);
        assert_eq!(StorageErrorKind::Internal.http_status_code(), 500);
    }

    #[test]
    fn test_convenience_constructors() {
        let io_err = StorageError::io("Failed to read", None);
        assert_eq!(io_err.kind, StorageErrorKind::Io);

        let not_found = StorageError::not_found("Collection not found");
        assert_eq!(not_found.kind, StorageErrorKind::NotFound);

        let corruption = StorageError::corruption("Data corrupted");
        assert_eq!(corruption.kind, StorageErrorKind::Corruption);

        let wal_corruption = StorageError::wal_corruption("WAL entry corrupted", Some(12345));
        assert_eq!(wal_corruption.kind, StorageErrorKind::WalCorruption);
        assert_eq!(wal_corruption.context.lsn, Some(12345));

        let capacity = StorageError::capacity_exceeded(100, 50);
        assert_eq!(capacity.kind, StorageErrorKind::CapacityExceeded);
    }

    #[test]
    fn test_engine_error() {
        let sst_err = StorageError::engine_error("SST", "Block read failed");
        assert_eq!(sst_err.kind, StorageErrorKind::SstEngine);
        assert_eq!(sst_err.context.engine.as_deref(), Some("SST"));

        let viper_err = StorageError::engine_error("VIPER", "Parquet decode failed");
        assert_eq!(viper_err.kind, StorageErrorKind::ViperEngine);
    }

    #[test]
    fn test_error_display() {
        let err = StorageError::flush_failed("my_collection", "Disk full");
        let display = format!("{}", err);
        assert!(display.contains("flush failed"));
        assert!(display.contains("Disk full"));
        assert!(display.contains("my_collection"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "File missing");
        let storage_err: StorageError = io_err.into();
        assert_eq!(storage_err.kind, StorageErrorKind::NotFound);
    }

    #[test]
    fn test_error_context_builder() {
        let ctx = ErrorContext::new()
            .with_file_path("/data/sst/001.sst")
            .with_collection("test_col")
            .with_lsn(1000)
            .with_operation("compaction")
            .with_engine("SST")
            .with_extra("level", "2");

        assert_eq!(ctx.file_path.as_deref(), Some("/data/sst/001.sst"));
        assert_eq!(ctx.collection_id.as_deref(), Some("test_col"));
        assert_eq!(ctx.lsn, Some(1000));
        assert_eq!(ctx.operation.as_deref(), Some("compaction"));
        assert_eq!(ctx.engine.as_deref(), Some("SST"));
        assert_eq!(ctx.extra.get("level").map(|s| s.as_str()), Some("2"));
    }
}
