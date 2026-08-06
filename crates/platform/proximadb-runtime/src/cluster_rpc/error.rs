/*
 * Copyright 2025 ProximaDB
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

//! RPC Error Types for Inter-Node Communication
//!
//! Provides strongly-typed error handling for cluster RPC operations.
//! These errors are designed to be:
//! - Serializable across node boundaries
//! - Retryable where appropriate
//! - Informative for debugging and monitoring

use std::fmt;
use std::time::Duration;

/// Error type for RPC operations in the cluster
#[derive(Debug, Clone)]
pub struct RpcError {
    /// The kind of error
    kind: RpcErrorKind,
    /// Human-readable error message
    message: String,
    /// Optional source node that generated the error
    source_node: Option<String>,
    /// Whether this error is retryable
    retryable: bool,
}

impl RpcError {
    /// Create a new RPC error
    pub fn new(kind: RpcErrorKind, message: impl Into<String>) -> Self {
        let retryable = kind.is_retryable();
        Self {
            kind,
            message: message.into(),
            source_node: None,
            retryable,
        }
    }

    /// Set the source node for this error
    pub fn with_source_node(mut self, node_id: impl Into<String>) -> Self {
        self.source_node = Some(node_id.into());
        self
    }

    /// Override the default retryable status
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Get the error kind
    pub fn kind(&self) -> &RpcErrorKind {
        &self.kind
    }

    /// Get the error message
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Get the source node (if known)
    pub fn source_node(&self) -> Option<&str> {
        self.source_node.as_deref()
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    // Convenience constructors for common errors

    /// Create a connection error
    pub fn connection(message: impl Into<String>) -> Self {
        Self::new(RpcErrorKind::Connection, message)
    }

    /// Create a timeout error
    pub fn timeout(duration: Duration) -> Self {
        Self::new(
            RpcErrorKind::Timeout,
            format!("RPC timed out after {:?}", duration),
        )
    }

    /// Create a node not found error
    pub fn node_not_found(node_id: impl Into<String>) -> Self {
        let node_id = node_id.into();
        Self::new(
            RpcErrorKind::NodeNotFound,
            format!("Node '{}' not found in cluster", node_id),
        )
    }

    /// Create a not leader error (for consensus operations)
    pub fn not_leader(leader_hint: Option<String>) -> Self {
        let message = match &leader_hint {
            Some(leader) => format!("Not the leader. Current leader: {}", leader),
            None => "Not the leader. Leader unknown.".to_string(),
        };
        Self::new(RpcErrorKind::NotLeader { leader_hint }, message)
    }

    /// Create a term mismatch error (for Raft operations)
    pub fn term_mismatch(our_term: u64, their_term: u64) -> Self {
        Self::new(
            RpcErrorKind::TermMismatch {
                our_term,
                their_term,
            },
            format!(
                "Term mismatch: our term={}, received term={}",
                our_term, their_term
            ),
        )
    }

    /// Create a consistency error
    pub fn consistency(required: u32, achieved: u32) -> Self {
        Self::new(
            RpcErrorKind::ConsistencyNotMet { required, achieved },
            format!(
                "Consistency requirement not met: required {} acks, got {}",
                required, achieved
            ),
        )
    }

    /// Create a shard not found error
    pub fn shard_not_found(shard_id: impl Into<String>) -> Self {
        let shard_id = shard_id.into();
        Self::new(
            RpcErrorKind::ShardNotFound,
            format!("Shard '{}' not found", shard_id),
        )
    }

    /// Create an invalid request error
    pub fn invalid_request(reason: impl Into<String>) -> Self {
        Self::new(RpcErrorKind::InvalidRequest, reason)
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(RpcErrorKind::Internal, message)
    }

    /// Create a serialization error
    pub fn serialization(message: impl Into<String>) -> Self {
        Self::new(RpcErrorKind::Serialization, message)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)?;
        if let Some(ref node) = self.source_node {
            write!(f, " (from node: {})", node)?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

/// Categories of RPC errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcErrorKind {
    /// Network connection failure
    Connection,

    /// Request timed out
    Timeout,

    /// Target node not found in cluster
    NodeNotFound,

    /// Target shard not found
    ShardNotFound,

    /// This node is not the leader (for consensus operations)
    NotLeader {
        /// Hint about who the current leader is
        leader_hint: Option<String>,
    },

    /// Raft term mismatch
    TermMismatch {
        /// Our current term
        our_term: u64,
        /// Term received from remote
        their_term: u64,
    },

    /// Consistency level could not be achieved
    ConsistencyNotMet {
        /// Required number of acknowledgments
        required: u32,
        /// Actual number of acknowledgments
        achieved: u32,
    },

    /// Log replication failed
    ReplicationFailed,

    /// Invalid request parameters
    InvalidRequest,

    /// Serialization/deserialization error
    Serialization,

    /// Internal server error
    Internal,

    /// Node is shutting down
    ShuttingDown,

    /// Rate limited
    RateLimited,
}

impl RpcErrorKind {
    /// Check if this error kind is typically retryable
    pub fn is_retryable(&self) -> bool {
        match self {
            RpcErrorKind::Connection => true,
            RpcErrorKind::Timeout => true,
            RpcErrorKind::NotLeader { .. } => true,
            RpcErrorKind::RateLimited => true,
            RpcErrorKind::ConsistencyNotMet { .. } => true,
            RpcErrorKind::ReplicationFailed => true,
            // Not retryable
            RpcErrorKind::NodeNotFound => false,
            RpcErrorKind::ShardNotFound => false,
            RpcErrorKind::TermMismatch { .. } => false,
            RpcErrorKind::InvalidRequest => false,
            RpcErrorKind::Serialization => false,
            RpcErrorKind::Internal => false,
            RpcErrorKind::ShuttingDown => false,
        }
    }
}

impl fmt::Display for RpcErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpcErrorKind::Connection => write!(f, "ConnectionError"),
            RpcErrorKind::Timeout => write!(f, "TimeoutError"),
            RpcErrorKind::NodeNotFound => write!(f, "NodeNotFoundError"),
            RpcErrorKind::ShardNotFound => write!(f, "ShardNotFoundError"),
            RpcErrorKind::NotLeader { .. } => write!(f, "NotLeaderError"),
            RpcErrorKind::TermMismatch { .. } => write!(f, "TermMismatchError"),
            RpcErrorKind::ConsistencyNotMet { .. } => write!(f, "ConsistencyError"),
            RpcErrorKind::ReplicationFailed => write!(f, "ReplicationError"),
            RpcErrorKind::InvalidRequest => write!(f, "InvalidRequestError"),
            RpcErrorKind::Serialization => write!(f, "SerializationError"),
            RpcErrorKind::Internal => write!(f, "InternalError"),
            RpcErrorKind::ShuttingDown => write!(f, "ShuttingDownError"),
            RpcErrorKind::RateLimited => write!(f, "RateLimitedError"),
        }
    }
}

/// Result type alias for RPC operations
pub type RpcResult<T> = std::result::Result<T, RpcError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let err = RpcError::connection("failed to connect");
        assert!(matches!(err.kind(), RpcErrorKind::Connection));
        assert!(err.is_retryable());
        assert!(err.message().contains("connect"));
    }

    #[test]
    fn test_timeout_error() {
        let err = RpcError::timeout(Duration::from_secs(5));
        assert!(matches!(err.kind(), RpcErrorKind::Timeout));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_not_leader_error() {
        let err = RpcError::not_leader(Some("node-2".to_string()));
        assert!(matches!(
            err.kind(),
            RpcErrorKind::NotLeader {
                leader_hint: Some(_)
            }
        ));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_term_mismatch_error() {
        let err = RpcError::term_mismatch(5, 10);
        assert!(matches!(
            err.kind(),
            RpcErrorKind::TermMismatch {
                our_term: 5,
                their_term: 10
            }
        ));
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_consistency_error() {
        let err = RpcError::consistency(3, 1);
        assert!(matches!(
            err.kind(),
            RpcErrorKind::ConsistencyNotMet {
                required: 3,
                achieved: 1
            }
        ));
        assert!(err.is_retryable());
    }

    #[test]
    fn test_error_with_source_node() {
        let err = RpcError::internal("something went wrong").with_source_node("node-1");

        assert_eq!(err.source_node(), Some("node-1"));
        let display = format!("{}", err);
        assert!(display.contains("node-1"));
    }

    #[test]
    fn test_retryable_override() {
        let err = RpcError::internal("retry this").with_retryable(true);

        assert!(err.is_retryable());
    }

    #[test]
    fn test_display() {
        let err = RpcError::node_not_found("node-42");
        let display = format!("{}", err);
        assert!(display.contains("NodeNotFoundError"));
        assert!(display.contains("node-42"));
    }
}
