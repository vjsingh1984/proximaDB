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

//! Stream session management
//!
//! This module provides types for managing individual streaming sessions,
//! including session state, identification, and lifecycle management.

use crossbeam::atomic::AtomicCell;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{RingBuffer, SessionConfig};
use crate::proto::proximadb_v1::VectorRecord;

/// Unique identifier for a streaming session
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamId(String);

impl StreamId {
    /// Create a new unique stream ID
    pub fn new() -> Self {
        Self(format!("stream_{}", Uuid::new_v4().simple()))
    }

    /// Create a stream ID from an existing string
    pub fn from_string(id: String) -> Self {
        Self(id)
    }

    /// Get the inner string value
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for StreamId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for StreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for StreamId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for StreamId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// State of a streaming session
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Session is active and accepting records
    Active,

    /// Session is paused (backpressure)
    Paused,

    /// Session is draining (no new records, processing remaining)
    Draining,

    /// Session is closed
    Closed,

    /// Session encountered an error
    Error,
}

impl SessionState {
    /// Check if the session is accepting new records
    pub fn accepts_records(&self) -> bool {
        matches!(self, SessionState::Active)
    }

    /// Check if the session is still processing
    pub fn is_processing(&self) -> bool {
        matches!(self, SessionState::Active | SessionState::Paused | SessionState::Draining)
    }

    /// Check if the session is terminated
    pub fn is_terminated(&self) -> bool {
        matches!(self, SessionState::Closed | SessionState::Error)
    }
}

/// Acknowledgment message sent back to the client
#[derive(Debug, Clone)]
pub struct AckMessage {
    /// Acknowledged sequence numbers
    pub acked_sequences: Vec<u64>,
    /// Server timestamp
    pub server_timestamp: u64,
    /// Current backpressure level
    pub backpressure_level: i32,
    /// Suggested delay in milliseconds
    pub suggested_delay_ms: u32,
    /// Buffer utilization percentage
    pub buffer_percent: u32,
}

/// A streaming session for a collection
pub struct StreamSession {
    /// Unique session identifier
    pub id: StreamId,

    /// Target collection for this stream
    pub collection: String,

    /// Ring buffer for incoming records
    pub buffer: Arc<RingBuffer<VectorRecord>>,

    /// Current session state
    pub state: AtomicCell<SessionState>,

    /// Session configuration
    pub config: SessionConfig,

    /// Timestamp when session was created
    pub created_at: Instant,

    /// Timestamp of last activity
    pub last_activity: AtomicCell<Instant>,

    /// Channel for sending acknowledgments
    pub ack_sender: mpsc::Sender<AckMessage>,

    /// Last acknowledged sequence number
    pub last_acked_sequence: AtomicCell<u64>,

    /// Total records received in this session
    pub records_received: AtomicCell<u64>,

    /// Total records processed (flushed to storage)
    pub records_processed: AtomicCell<u64>,
}

impl StreamSession {
    /// Create a new streaming session
    pub fn new(
        collection: String,
        config: SessionConfig,
        buffer_size: usize,
        ack_sender: mpsc::Sender<AckMessage>,
    ) -> Self {
        let now = Instant::now();

        Self {
            id: StreamId::new(),
            collection,
            buffer: Arc::new(RingBuffer::new(buffer_size.next_power_of_two())),
            state: AtomicCell::new(SessionState::Active),
            config,
            created_at: now,
            last_activity: AtomicCell::new(now),
            ack_sender,
            last_acked_sequence: AtomicCell::new(0),
            records_received: AtomicCell::new(0),
            records_processed: AtomicCell::new(0),
        }
    }

    /// Update last activity timestamp
    pub fn touch(&self) {
        self.last_activity.store(Instant::now());
    }

    /// Get session age in seconds
    pub fn age_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }

    /// Get time since last activity in seconds
    pub fn idle_secs(&self) -> u64 {
        self.last_activity.load().elapsed().as_secs()
    }

    /// Check if session has timed out
    pub fn is_timed_out(&self, timeout_secs: u64) -> bool {
        self.idle_secs() > timeout_secs
    }

    /// Increment records received counter
    pub fn increment_received(&self, count: u64) {
        let current = self.records_received.load();
        self.records_received.store(current + count);
    }

    /// Increment records processed counter
    pub fn increment_processed(&self, count: u64) {
        let current = self.records_processed.load();
        self.records_processed.store(current + count);
    }

    /// Get session statistics
    pub fn stats(&self) -> SessionStats {
        SessionStats {
            id: self.id.clone(),
            collection: self.collection.clone(),
            state: self.state.load(),
            buffer_len: self.buffer.len(),
            buffer_capacity: self.buffer.capacity(),
            records_received: self.records_received.load(),
            records_processed: self.records_processed.load(),
            age_secs: self.age_secs(),
            idle_secs: self.idle_secs(),
        }
    }

    /// Transition to a new state
    pub fn transition_to(&self, new_state: SessionState) -> bool {
        let current = self.state.load();

        // Validate state transitions
        let valid = match (current, new_state) {
            // Active can go to any state
            (SessionState::Active, _) => true,
            // Paused can go to Active, Draining, Closed, or Error
            (SessionState::Paused, SessionState::Active) => true,
            (SessionState::Paused, SessionState::Draining) => true,
            (SessionState::Paused, SessionState::Closed) => true,
            (SessionState::Paused, SessionState::Error) => true,
            // Draining can only go to Closed or Error
            (SessionState::Draining, SessionState::Closed) => true,
            (SessionState::Draining, SessionState::Error) => true,
            // Terminal states cannot transition
            (SessionState::Closed, _) => false,
            (SessionState::Error, _) => false,
            _ => false,
        };

        if valid {
            self.state.store(new_state);
        }
        valid
    }
}

/// Statistics for a streaming session
#[derive(Debug, Clone)]
pub struct SessionStats {
    /// Session ID
    pub id: StreamId,
    /// Target collection
    pub collection: String,
    /// Current state
    pub state: SessionState,
    /// Current buffer length
    pub buffer_len: usize,
    /// Buffer capacity
    pub buffer_capacity: usize,
    /// Total records received
    pub records_received: u64,
    /// Total records processed
    pub records_processed: u64,
    /// Session age in seconds
    pub age_secs: u64,
    /// Time since last activity in seconds
    pub idle_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_id_generation() {
        let id1 = StreamId::new();
        let id2 = StreamId::new();
        assert_ne!(id1, id2);
        assert!(id1.as_str().starts_with("stream_"));
    }

    #[test]
    fn test_session_state_transitions() {
        let (tx, _rx) = mpsc::channel(1);
        let session = StreamSession::new(
            "test".to_string(),
            SessionConfig::default(),
            1024,
            tx,
        );

        assert_eq!(session.state.load(), SessionState::Active);
        assert!(session.transition_to(SessionState::Paused));
        assert_eq!(session.state.load(), SessionState::Paused);
        assert!(session.transition_to(SessionState::Draining));
        assert_eq!(session.state.load(), SessionState::Draining);
        assert!(session.transition_to(SessionState::Closed));
        assert_eq!(session.state.load(), SessionState::Closed);

        // Cannot transition from Closed
        assert!(!session.transition_to(SessionState::Active));
    }

    #[test]
    fn test_session_stats() {
        let (tx, _rx) = mpsc::channel(1);
        let session = StreamSession::new(
            "test_collection".to_string(),
            SessionConfig::default(),
            1024,
            tx,
        );

        session.increment_received(100);
        session.increment_processed(50);

        let stats = session.stats();
        assert_eq!(stats.collection, "test_collection");
        assert_eq!(stats.records_received, 100);
        assert_eq!(stats.records_processed, 50);
        assert_eq!(stats.buffer_capacity, 1024);
    }
}
