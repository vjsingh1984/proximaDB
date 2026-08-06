/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! CDC error types and result aliases

use std::fmt;

/// Result type for CDC operations
pub type CdcResult<T> = Result<T, CdcError>;

/// Error types for CDC operations
#[derive(Debug)]
pub enum CdcError {
    /// Configuration error
    Configuration(String),
    /// Connection error to source or sink
    Connection(String),
    /// Serialization/deserialization error
    Serialization(String),
    /// Offset storage error
    OffsetStorage(String),
    /// Source-specific error
    Source(String),
    /// Sink-specific error
    Sink(String),
    /// Transform pipeline error
    Transform(String),
    /// Embedding generation error
    Embedding(String),
    /// Coordinator error
    Coordinator(String),
    /// Timeout error
    Timeout(String),
    /// Schema error
    Schema(String),
    /// Authentication error
    Authentication(String),
    /// Resource not found
    NotFound(String),
    /// Already exists
    AlreadyExists(String),
    /// Invalid state
    InvalidState(String),
    /// Duplicate event
    Duplicate(String),
    /// Capacity exceeded
    Capacity(String),
    /// Channel error
    Channel(String),
    /// IO error
    Io(std::io::Error),
    /// Other error
    Other(String),
}

impl fmt::Display for CdcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(msg) => write!(f, "CDC configuration error: {}", msg),
            Self::Connection(msg) => write!(f, "CDC connection error: {}", msg),
            Self::Serialization(msg) => write!(f, "CDC serialization error: {}", msg),
            Self::OffsetStorage(msg) => write!(f, "CDC offset storage error: {}", msg),
            Self::Source(msg) => write!(f, "CDC source error: {}", msg),
            Self::Sink(msg) => write!(f, "CDC sink error: {}", msg),
            Self::Transform(msg) => write!(f, "CDC transform error: {}", msg),
            Self::Embedding(msg) => write!(f, "CDC embedding error: {}", msg),
            Self::Coordinator(msg) => write!(f, "CDC coordinator error: {}", msg),
            Self::Timeout(msg) => write!(f, "CDC timeout: {}", msg),
            Self::Schema(msg) => write!(f, "CDC schema error: {}", msg),
            Self::Authentication(msg) => write!(f, "CDC authentication error: {}", msg),
            Self::NotFound(msg) => write!(f, "CDC resource not found: {}", msg),
            Self::AlreadyExists(msg) => write!(f, "CDC resource already exists: {}", msg),
            Self::InvalidState(msg) => write!(f, "CDC invalid state: {}", msg),
            Self::Duplicate(msg) => write!(f, "CDC duplicate event: {}", msg),
            Self::Capacity(msg) => write!(f, "CDC capacity exceeded: {}", msg),
            Self::Channel(msg) => write!(f, "CDC channel error: {}", msg),
            Self::Io(err) => write!(f, "CDC IO error: {}", err),
            Self::Other(msg) => write!(f, "CDC error: {}", msg),
        }
    }
}

impl std::error::Error for CdcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CdcError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for CdcError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CdcError::Configuration("invalid port".to_string());
        assert!(err.to_string().contains("configuration error"));
        assert!(err.to_string().contains("invalid port"));
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let cdc_err: CdcError = io_err.into();
        assert!(matches!(cdc_err, CdcError::Io(_)));
    }

    #[test]
    fn test_error_variants() {
        let errors = vec![
            CdcError::Connection("timeout".to_string()),
            CdcError::Serialization("invalid json".to_string()),
            CdcError::OffsetStorage("corrupt".to_string()),
            CdcError::Source("pg error".to_string()),
            CdcError::Sink("kafka error".to_string()),
            CdcError::Transform("mapping failed".to_string()),
            CdcError::Embedding("model not found".to_string()),
            CdcError::Coordinator("not running".to_string()),
            CdcError::Timeout("5s exceeded".to_string()),
            CdcError::Schema("incompatible".to_string()),
            CdcError::Authentication("invalid token".to_string()),
            CdcError::NotFound("connector xyz".to_string()),
            CdcError::AlreadyExists("source abc".to_string()),
            CdcError::InvalidState("already stopped".to_string()),
            CdcError::Duplicate("event already processed".to_string()),
            CdcError::Capacity("queue full".to_string()),
            CdcError::Channel("channel closed".to_string()),
            CdcError::Other("unknown".to_string()),
        ];

        for err in errors {
            // All errors should have non-empty display strings
            assert!(!err.to_string().is_empty());
        }
    }
}
