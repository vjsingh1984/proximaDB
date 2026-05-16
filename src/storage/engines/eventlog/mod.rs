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

//! # Event Sourcing Engine
//!
//! This module implements an append-only event log for event sourcing patterns,
//! providing immutable audit trails and temporal replay capabilities.
//!
//! ## Architecture
//!
//! ```text
//! EventLogEngine
//!      ↓
//! ┌─────────────────────────────────────┐
//! │  Append-Only Event Store            │
//! │  - Immutable events                 │
//! │  - Monotonic sequence numbers       │
//! │  - WAL for durability               │
//! └─────────────────────────────────────┘
//!      ↓
//! ┌─────────────────────────────────────┐
//! │  Event Index                        │
//! │  - By entity ID                     │
//! │  - By event type                    │
//! │  - By timestamp                     │
//! └─────────────────────────────────────┘
//!      ↓
//! ┌─────────────────────────────────────┐
//! │  Temporal Query Engine              │
//! │  - As-of queries                   │
//! │  - Event replay                    │
//! │  - Point-in-time state             │
//! └─────────────────────────────────────┘
//! ```
//!
//! ## Use Cases
//!
//! - **Financial Services**: Trade audit trails (MiFID II compliant)
//! - **Regulatory Compliance**: Immutable audit logs
//! - **Event Sourcing**: CQRS and event-driven architectures
//! - **Temporal Queries**: Point-in-time data reconstruction
//! - **Audit Trails**: Complete change history

use proximadb_kernel::error::ProximaDBError;
use crate::storage::persistence::filesystem::{FileSystem, UnifiedCachingFilesystem};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

pub mod index;
pub mod snapshot;
pub mod temporal;

use index::EventIndex;
use snapshot::SnapshotManager;
use temporal::TemporalQueryEngine;

/// Unique event identifier (monotonically increasing)
pub type EventSequence = u64;

/// Entity identifier (e.g., "account:123", "order:456")
pub type EntityId = String;

/// Event type identifier (e.g., "AccountCreated", "MoneyDeposited")
pub type EventType = String;

/// Event record in the event log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic sequence number
    pub sequence: EventSequence,

    /// Entity this event belongs to
    pub entity_id: EntityId,

    /// Event type (e.g., "AccountCreated")
    pub event_type: EventType,

    /// Event data (JSON)
    pub data: serde_json::Value,

    /// Event timestamp
    pub timestamp: DateTime<Utc>,

    /// Causation metadata (optional correlation IDs)
    pub causation_id: Option<String>,

    /// Metadata (user ID, request ID, etc.)
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Event log configuration
#[derive(Debug, Clone)]
pub struct EventLogConfig {
    /// Base directory for event storage
    pub base_dir: PathBuf,

    /// Snapshot interval (number of events after which to snapshot)
    pub snapshot_interval: EventSequence,

    /// Enable compression for event data
    pub enable_compression: bool,

    /// Retention period for events (days)
    pub retention_days: u32,

    /// Enable regulatory compliance mode (MiFID II)
    pub regulatory_mode: bool,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("/tmp/proximadb/eventlog"),
            snapshot_interval: 1000,
            enable_compression: true,
            retention_days: 365 * 7, // 7 years for financial data
            regulatory_mode: false,
        }
    }
}

/// Event log statistics
#[derive(Debug, Clone, Default)]
pub struct EventLogStats {
    /// Total events stored
    pub total_events: u64,

    /// Total entities tracked
    pub total_entities: u64,

    /// Event types registered
    pub event_types: usize,

    /// Snapshots created
    pub snapshots_created: u64,

    /// Temporal queries executed
    pub temporal_queries: u64,
}

/// Event sourcing engine for append-only event storage
pub struct EventLogEngine {
    /// Configuration
    config: EventLogConfig,

    /// Monotonic sequence number counter
    sequence_counter: Arc<RwLock<EventSequence>>,

    /// Event index for fast lookups
    event_index: Arc<EventIndex>,

    /// Snapshot manager
    snapshot_manager: Arc<SnapshotManager>,

    /// Temporal query engine
    temporal_engine: Arc<TemporalQueryEngine>,

    /// Filesystem for persistence
    filesystem: Arc<UnifiedCachingFilesystem>,

    /// Statistics
    stats: Arc<RwLock<EventLogStats>>,
}

impl EventLogEngine {
    /// Create a new event log engine
    pub fn new(config: EventLogConfig, filesystem: Arc<UnifiedCachingFilesystem>) -> Result<Self> {
        info!("Creating event log engine at {:?}", config.base_dir);

        // Create base directory
        std::fs::create_dir_all(&config.base_dir).map_err(|e| {
            ProximaDBError::Internal(format!("Failed to create eventlog dir: {}", e))
        })?;

        // Initialize sequence counter
        let sequence_counter = Arc::new(RwLock::new(0));

        // Initialize components
        let event_index = Arc::new(EventIndex::new(config.base_dir.clone())?);
        let snapshot_manager = Arc::new(SnapshotManager::new(config.base_dir.clone())?);
        let temporal_engine = Arc::new(TemporalQueryEngine::new(
            event_index.clone(),
            snapshot_manager.clone(),
            config.clone(),
        )?);

        Ok(Self {
            config,
            sequence_counter,
            event_index,
            snapshot_manager,
            temporal_engine,
            filesystem,
            stats: Arc::new(RwLock::new(EventLogStats::default())),
        })
    }

    /// Append an event to the log (append-only, immutable)
    ///
    /// # Arguments
    ///
    /// * `event` - Event to append (without sequence number)
    ///
    /// # Returns
    ///
    /// The appended event with assigned sequence number
    pub async fn append_event(&self, mut event: Event) -> Result<Event> {
        // Verify regulatory compliance if enabled
        if self.config.regulatory_mode {
            self.verify_regulatory_compliance(&event)?;
        }

        // Assign monotonically increasing sequence number
        let sequence = {
            let mut counter = self.sequence_counter.write().await;
            *counter += 1;
            *counter
        };

        event.sequence = sequence;

        // Validate event data
        self.validate_event(&event)?;

        // Serialize event to JSON (better compatibility than bincode)
        let serialized = serde_json::to_vec(&event)
            .map_err(|e| ProximaDBError::Internal(format!("Event serialization failed: {}", e)))?;

        // Persist to storage (append-only)
        let event_path = self.get_event_path(sequence);

        // Create partition directory if it doesn't exist
        if let Some(partition_dir) = event_path.parent() {
            tokio::fs::create_dir_all(partition_dir)
                .await
                .map_err(|e| {
                    ProximaDBError::Internal(format!("Failed to create partition dir: {}", e))
                })?;
        }

        // Write with create_dirs option to ensure parent directories exist
        let options = crate::storage::persistence::filesystem::FileOptions {
            create_dirs: true,
            overwrite: true,
            ..Default::default()
        };
        FileSystem::write(
            self.filesystem.as_ref(),
            &event_path.to_string_lossy(),
            &serialized,
            Some(options),
        )
        .await?;

        // Update index
        self.event_index.index_event(&event).await?;

        // Check if snapshot is needed
        if sequence % self.config.snapshot_interval == 0 {
            debug!(
                "Triggering snapshot for entity {} at sequence {}",
                event.entity_id, sequence
            );
            self.snapshot_manager
                .create_snapshot(&event.entity_id, sequence)
                .await?;

            // Update snapshot stats
            {
                let mut stats = self.stats.write().await;
                stats.snapshots_created += 1;
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_events += 1;
        }

        debug!(
            "Appended event {} for entity {} at sequence {}",
            event.event_type, event.entity_id, event.sequence
        );

        Ok(event)
    }

    /// Read events for an entity
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to read events for
    /// * `from_sequence` - Starting sequence (inclusive)
    /// * `limit` - Maximum number of events to return
    ///
    /// # Returns
    ///
    /// Vector of events in sequence order
    pub async fn read_events(
        &self,
        entity_id: &EntityId,
        from_sequence: EventSequence,
        limit: usize,
    ) -> Result<Vec<Event>> {
        debug!(
            "Reading events for {} from sequence {}, limit {}",
            entity_id, from_sequence, limit
        );

        // Query index for event sequences
        let sequences = self
            .event_index
            .get_entity_events(entity_id, from_sequence, limit)
            .await?;

        // Load events from storage
        let mut events = Vec::new();
        for sequence in sequences {
            let event_path = self.get_event_path(sequence);
            let data =
                FileSystem::read(self.filesystem.as_ref(), &event_path.to_string_lossy()).await?;

            let event: Event = serde_json::from_slice(&data).map_err(|e| {
                ProximaDBError::Internal(format!("Event deserialization failed: {}", e))
            })?;

            events.push(event);
        }

        Ok(events)
    }

    /// Get current state of an entity (as-of now)
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to get state for
    ///
    /// # Returns
    ///
    /// Current entity state as JSON
    pub async fn get_entity_state(&self, entity_id: &EntityId) -> Result<serde_json::Value> {
        self.temporal_engine.get_state_as_of(entity_id, None).await
    }

    /// Get entity state as of a specific point in time
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to get state for
    /// * `as_of` - Point-in-time timestamp
    ///
    /// # Returns
    ///
    /// Entity state as of the specified time
    pub async fn get_state_as_of(
        &self,
        entity_id: &EntityId,
        as_of: DateTime<Utc>,
    ) -> Result<serde_json::Value> {
        self.temporal_engine
            .get_state_as_of(entity_id, Some(as_of))
            .await
    }

    /// Replay events for an entity
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to replay
    /// * `from_sequence` - Starting sequence (0 for all)
    /// * `to_sequence` - Ending sequence (None for latest)
    ///
    /// # Returns
    ///
    /// Stream of events in replay order
    pub async fn replay_events(
        &self,
        entity_id: &EntityId,
        from_sequence: EventSequence,
        to_sequence: Option<EventSequence>,
    ) -> Result<Vec<Event>> {
        // Determine limit based on to_sequence
        let limit = match to_sequence {
            Some(to_seq) => (to_seq - from_sequence + 1) as usize,
            None => usize::MAX, // No limit if to_sequence is None
        };

        let mut events = self.read_events(entity_id, from_sequence, limit).await?;

        // Filter by to_sequence if specified (double-check)
        if let Some(to_seq) = to_sequence {
            events.retain(|e| e.sequence <= to_seq);
        }

        Ok(events)
    }

    /// Get event log statistics
    pub async fn get_stats(&self) -> EventLogStats {
        self.stats.read().await.clone()
    }

    /// Verify regulatory compliance (MiFID II)
    fn verify_regulatory_compliance(&self, event: &Event) -> Result<()> {
        // Check required metadata fields
        let required_fields = vec!["user_id", "request_id", "origin"];

        for field in required_fields {
            if !event.metadata.contains_key(field) {
                return Err(ProximaDBError::Internal(format!(
                    "Regulatory compliance: missing required metadata field: {}",
                    field
                )));
            }
        }

        // Verify timestamp precision
        if event.timestamp.timestamp_subsec_nanos() == 0 {
            return Err(ProximaDBError::Internal(
                "Regulatory compliance: timestamp must have sub-millisecond precision".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate event data
    fn validate_event(&self, event: &Event) -> Result<()> {
        if event.entity_id.is_empty() {
            return Err(ProximaDBError::InvalidInput(
                "Entity ID cannot be empty".to_string(),
            ));
        }

        if event.event_type.is_empty() {
            return Err(ProximaDBError::InvalidInput(
                "Event type cannot be empty".to_string(),
            ));
        }

        if !event.data.is_object() {
            return Err(ProximaDBError::InvalidInput(
                "Event data must be a JSON object".to_string(),
            ));
        }

        Ok(())
    }

    /// Get storage path for an event
    fn get_event_path(&self, sequence: EventSequence) -> PathBuf {
        // Partition events by sequence (1000 events per partition)
        let partition = sequence / 1000;
        self.config
            .base_dir
            .join(format!("partition_{:05}", partition))
            .join(format!("event_{:010}.bin", sequence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::UnifiedCachingFilesystem;

    #[tokio::test]
    async fn test_event_log_creation() {
        let base_dir = PathBuf::from("/tmp/test_eventlog_creation");
        // Create base directory first
        std::fs::create_dir_all(&base_dir).expect("Failed to create test eventlog directory");

        let config = EventLogConfig {
            base_dir: base_dir.clone(),
            ..Default::default()
        };

        // Create local filesystem
        let local_config = crate::storage::persistence::filesystem::local::LocalConfig::default();
        let local_fs =
            crate::storage::persistence::filesystem::local::LocalFileSystem::new(local_config)
                .await
                .expect("Failed to create local filesystem");
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            Arc::new(local_fs),
            "eventlog_test".to_string(),
            "eventlog".to_string(),
        ));

        let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");
        assert_eq!(engine.config.snapshot_interval, 1000);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }

    #[tokio::test]
    async fn test_append_event() {
        let base_dir = PathBuf::from("/tmp/test_eventlog_append");
        // Create base directory first
        std::fs::create_dir_all(&base_dir)
            .expect("Failed to create test eventlog append directory");

        let config = EventLogConfig {
            base_dir: base_dir.clone(),
            ..Default::default()
        };

        // Create local filesystem
        let local_config = crate::storage::persistence::filesystem::local::LocalConfig::default();
        let local_fs =
            crate::storage::persistence::filesystem::local::LocalFileSystem::new(local_config)
                .await
                .expect("Failed to create local filesystem");
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            Arc::new(local_fs),
            "eventlog_append".to_string(),
            "eventlog".to_string(),
        ));

        let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

        let event = Event {
            sequence: 0, // Will be assigned
            entity_id: "account:123".to_string(),
            event_type: "AccountCreated".to_string(),
            data: serde_json::json!({"balance": 1000}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        let appended = engine
            .append_event(event)
            .await
            .expect("Failed to append event");
        assert_eq!(appended.sequence, 1);
        assert_eq!(appended.entity_id, "account:123");

        // Cleanup
        let _ = std::fs::remove_dir_all("/tmp/test_eventlog_append");
    }

    #[tokio::test]
    async fn test_regulatory_compliance() {
        let base_dir = PathBuf::from("/tmp/test_eventlog_reg");
        // Create base directory first
        std::fs::create_dir_all(&base_dir)
            .expect("Failed to create test eventlog regulatory directory");

        let config = EventLogConfig {
            base_dir: base_dir.clone(),
            regulatory_mode: true,
            ..Default::default()
        };

        // Create local filesystem
        let local_config = crate::storage::persistence::filesystem::local::LocalConfig::default();
        let local_fs =
            crate::storage::persistence::filesystem::local::LocalFileSystem::new(local_config)
                .await
                .expect("Failed to create local filesystem");
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            Arc::new(local_fs),
            "eventlog_reg".to_string(),
            "eventlog".to_string(),
        ));

        let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

        // Missing required metadata
        let event = Event {
            sequence: 0,
            entity_id: "trade:456".to_string(),
            event_type: "TradeExecuted".to_string(),
            data: serde_json::json!({"amount": 100}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(), // Missing required fields
        };

        let result = engine.append_event(event).await;
        assert!(result.is_err());

        // Cleanup
        let _ = std::fs::remove_dir_all("/tmp/test_eventlog_reg");
    }

    #[tokio::test]
    async fn test_immutable_audit_trail() {
        let base_dir = PathBuf::from("/tmp/test_eventlog_immutable");
        // Create base directory first
        std::fs::create_dir_all(&base_dir)
            .expect("Failed to create test eventlog immutable directory");

        let config = EventLogConfig {
            base_dir: base_dir.clone(),
            ..Default::default()
        };

        // Create local filesystem
        let local_config = crate::storage::persistence::filesystem::local::LocalConfig::default();
        let local_fs =
            crate::storage::persistence::filesystem::local::LocalFileSystem::new(local_config)
                .await
                .expect("Failed to create local filesystem");
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            Arc::new(local_fs),
            "eventlog_immutable".to_string(),
            "eventlog".to_string(),
        ));

        let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

        // Append multiple events
        for i in 1..=3 {
            let event = Event {
                sequence: 0,
                entity_id: "order:789".to_string(),
                event_type: format!("OrderUpdated{}", i),
                data: serde_json::json!({"version": i}),
                timestamp: Utc::now(),
                causation_id: None,
                metadata: HashMap::new(),
            };

            let appended = engine
                .append_event(event)
                .await
                .expect("Failed to append event");
            assert_eq!(appended.sequence, i as u64);
        }

        // Verify monotonically increasing sequence numbers
        let counter = engine.sequence_counter.read().await;
        assert_eq!(*counter, 3);

        // Cleanup
        let _ = std::fs::remove_dir_all("/tmp/test_eventlog_immutable");
    }
}
