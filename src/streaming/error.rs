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

//! Error types for streaming operations

use std::fmt;

/// Result type for streaming operations
pub type StreamResult<T> = Result<T, StreamError>;

/// Errors that can occur during streaming operations
#[derive(Debug, Clone)]
pub enum StreamError {
    /// Too many concurrent streaming sessions
    TooManySessions {
        /// Maximum allowed sessions
        max: usize,
        /// Current session count
        current: usize,
    },

    /// Session not found
    SessionNotFound {
        /// The session ID that was not found
        session_id: String,
    },

    /// Session is closed or inactive
    SessionClosed {
        /// The session ID
        session_id: String,
    },

    /// Rate limit exceeded
    RateLimited {
        /// Current rate (records/second)
        current_rate: u64,
        /// Maximum allowed rate
        max_rate: u64,
        /// Suggested retry delay in milliseconds
        retry_after_ms: u64,
    },

    /// Buffer is full (backpressure critical)
    BufferFull {
        /// Buffer capacity
        capacity: usize,
        /// Number of records that couldn't be buffered
        dropped: usize,
    },

    /// Invalid configuration
    InvalidConfig {
        /// Description of what's invalid
        message: String,
    },

    /// Collection not found
    CollectionNotFound {
        /// The collection name
        collection: String,
    },

    /// Storage engine error
    StorageError {
        /// Error message from storage
        message: String,
    },

    /// Serialization/deserialization error
    SerializationError {
        /// Error message
        message: String,
    },

    /// Connection error (WebSocket, Kafka, etc.)
    ConnectionError {
        /// Error message
        message: String,
    },

    /// Timeout waiting for operation
    Timeout {
        /// Operation that timed out
        operation: String,
        /// Timeout duration in milliseconds
        timeout_ms: u64,
    },

    /// Internal error
    Internal {
        /// Error message
        message: String,
    },
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::TooManySessions { max, current } => {
                write!(
                    f,
                    "Too many concurrent sessions: {} (max: {})",
                    current, max
                )
            }
            StreamError::SessionNotFound { session_id } => {
                write!(f, "Session not found: {}", session_id)
            }
            StreamError::SessionClosed { session_id } => {
                write!(f, "Session is closed: {}", session_id)
            }
            StreamError::RateLimited {
                current_rate,
                max_rate,
                retry_after_ms,
            } => {
                write!(
                    f,
                    "Rate limit exceeded: {}/s (max: {}/s), retry after {}ms",
                    current_rate, max_rate, retry_after_ms
                )
            }
            StreamError::BufferFull { capacity, dropped } => {
                write!(
                    f,
                    "Buffer full (capacity: {}), dropped {} records",
                    capacity, dropped
                )
            }
            StreamError::InvalidConfig { message } => {
                write!(f, "Invalid configuration: {}", message)
            }
            StreamError::CollectionNotFound { collection } => {
                write!(f, "Collection not found: {}", collection)
            }
            StreamError::StorageError { message } => {
                write!(f, "Storage error: {}", message)
            }
            StreamError::SerializationError { message } => {
                write!(f, "Serialization error: {}", message)
            }
            StreamError::ConnectionError { message } => {
                write!(f, "Connection error: {}", message)
            }
            StreamError::Timeout {
                operation,
                timeout_ms,
            } => {
                write!(f, "Timeout after {}ms: {}", timeout_ms, operation)
            }
            StreamError::Internal { message } => {
                write!(f, "Internal error: {}", message)
            }
        }
    }
}

impl std::error::Error for StreamError {}

impl From<String> for StreamError {
    fn from(message: String) -> Self {
        StreamError::Internal { message }
    }
}

impl From<&str> for StreamError {
    fn from(message: &str) -> Self {
        StreamError::Internal {
            message: message.to_string(),
        }
    }
}
