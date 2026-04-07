//! Error types for the ProximaDB Rust SDK
//!
//! This module provides a comprehensive set of error types for handling
//! various failure modes in both client and embedded modes.

use thiserror::Error;

/// The main error type for the ProximaDB SDK
#[derive(Error, Debug)]
pub enum ProximaError {
    /// Collection-related errors
    #[error("Collection error: {0}")]
    Collection(#[from] CollectionError),

    /// Vector operation errors
    #[error("Vector error: {0}")]
    Vector(#[from] VectorError),

    /// Network/connection errors (client mode)
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),

    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Search operation errors
    #[error("Search error: {0}")]
    Search(#[from] SearchError),

    /// Embedded mode specific errors
    #[error("Embedded error: {0}")]
    Embedded(#[from] EmbeddedError),

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Errors related to collection operations
#[derive(Error, Debug)]
pub enum CollectionError {
    /// Collection not found
    #[error("Collection '{name}' not found")]
    NotFound { name: String },

    /// Collection already exists
    #[error("Collection '{name}' already exists")]
    AlreadyExists { name: String },

    /// Invalid collection configuration
    #[error("Invalid collection configuration: {reason}")]
    InvalidConfig { reason: String },

    /// Invalid collection name
    #[error("Invalid collection name: {reason}")]
    InvalidName { reason: String },

    /// Dimension mismatch
    #[error("Dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: u32, actual: u32 },

    /// Unknown storage engine
    #[error(
        "Unknown storage engine: {engine}. Supported: sst, helix, viper, nova, swift, raptor, tst"
    )]
    UnknownEngine { engine: String },
}

/// Errors related to vector operations
#[derive(Error, Debug)]
pub enum VectorError {
    /// Vector not found
    #[error("Vector '{id}' not found in collection '{collection}'")]
    NotFound { id: String, collection: String },

    /// Invalid vector dimension
    #[error("Invalid vector dimension: expected {expected}, got {actual}")]
    InvalidDimension { expected: u32, actual: u32 },

    /// Invalid vector format
    #[error("Invalid vector format: {reason}")]
    InvalidFormat { reason: String },

    /// Empty vector provided
    #[error("Empty vector provided")]
    EmptyVector,

    /// Batch size mismatch
    #[error("Batch size mismatch: {ids} IDs but {vectors} vectors")]
    BatchSizeMismatch { ids: usize, vectors: usize },

    /// ID already exists
    #[error("Vector ID '{id}' already exists in collection '{collection}'")]
    IdExists { id: String, collection: String },
}

/// Errors related to network operations (client mode)
#[derive(Error, Debug)]
pub enum NetworkError {
    /// Connection failed
    #[error("Failed to connect to {url}: {reason}")]
    ConnectionFailed { url: String, reason: String },

    /// Request timeout
    #[error("Request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// HTTP error response
    #[error("HTTP error {status}: {message}")]
    HttpError { status: u16, message: String },

    /// Invalid URL
    #[error("Invalid URL: {url}")]
    InvalidUrl { url: String },

    /// Serialization error
    #[error("Serialization error: {reason}")]
    Serialization { reason: String },

    /// Deserialization error
    #[error("Deserialization error: {reason}")]
    Deserialization { reason: String },

    /// Authentication failed
    #[error("Authentication failed: {reason}")]
    AuthenticationFailed { reason: String },

    /// Rate limited
    #[error("Rate limited: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
}

/// Configuration errors
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Missing required field
    #[error("Missing required configuration: {field}")]
    MissingRequired { field: String },

    /// Invalid value
    #[error("Invalid value for {field}: {reason}")]
    InvalidValue { field: String, reason: String },

    /// Invalid data directory
    #[error("Invalid data directory: {path}")]
    InvalidDataDir { path: String },

    /// IO error during config
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Search operation errors
#[derive(Error, Debug)]
pub enum SearchError {
    /// Invalid top_k value
    #[error("Invalid top_k value: {value}. Must be between 1 and {max}")]
    InvalidTopK { value: usize, max: usize },

    /// Invalid filter expression
    #[error("Invalid filter expression: {reason}")]
    InvalidFilter { reason: String },

    /// No results found
    #[error("No results found for query")]
    NoResults,

    /// Invalid search mode
    #[error("Invalid search mode: {mode}. Supported: exact, approximate, adaptive")]
    InvalidMode { mode: String },
}

/// Embedded mode specific errors
#[derive(Error, Debug)]
pub enum EmbeddedError {
    /// Failed to initialize embedded database
    #[error("Failed to initialize embedded database: {reason}")]
    InitializationFailed { reason: String },

    /// WAL error
    #[error("WAL error: {reason}")]
    WalError { reason: String },

    /// Flush error
    #[error("Flush error: {reason}")]
    FlushError { reason: String },

    /// Shutdown error
    #[error("Shutdown error: {reason}")]
    ShutdownError { reason: String },

    /// Feature not available
    #[error("Embedded mode not available: compile with --features embedded")]
    NotAvailable,
}

/// Result type alias for ProximaDB operations
pub type Result<T> = std::result::Result<T, ProximaError>;

// Convenience implementations for error conversion

impl From<String> for ProximaError {
    fn from(s: String) -> Self {
        ProximaError::Internal(s)
    }
}

impl From<&str> for ProximaError {
    fn from(s: &str) -> Self {
        ProximaError::Internal(s.to_string())
    }
}

#[cfg(feature = "client")]
impl From<reqwest::Error> for ProximaError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ProximaError::Network(NetworkError::Timeout { timeout_ms: 30000 })
        } else if err.is_connect() {
            ProximaError::Network(NetworkError::ConnectionFailed {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
                reason: err.to_string(),
            })
        } else {
            ProximaError::Network(NetworkError::ConnectionFailed {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
                reason: err.to_string(),
            })
        }
    }
}

impl From<serde_json::Error> for ProximaError {
    fn from(err: serde_json::Error) -> Self {
        ProximaError::Network(NetworkError::Serialization {
            reason: err.to_string(),
        })
    }
}

impl From<url::ParseError> for ProximaError {
    fn from(err: url::ParseError) -> Self {
        ProximaError::Network(NetworkError::InvalidUrl {
            url: err.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CollectionError::NotFound {
            name: "test".to_string(),
        };
        assert_eq!(err.to_string(), "Collection 'test' not found");

        let err = VectorError::InvalidDimension {
            expected: 768,
            actual: 512,
        };
        assert_eq!(
            err.to_string(),
            "Invalid vector dimension: expected 768, got 512"
        );
    }

    #[test]
    fn test_error_conversion() {
        let err: ProximaError = CollectionError::NotFound {
            name: "test".to_string(),
        }
        .into();
        assert!(matches!(err, ProximaError::Collection(_)));

        let err: ProximaError = "internal error".into();
        assert!(matches!(err, ProximaError::Internal(_)));
    }
}
