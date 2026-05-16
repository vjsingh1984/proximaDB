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

//! # Temporal Query Engine
//!
//! This module provides time travel queries for event-sourced entities,
//! enabling point-in-time state reconstruction and event replay.

use proximadb_kernel::error::ProximaDBError;
use crate::storage::engines::eventlog::index::EventIndex;
use crate::storage::engines::eventlog::snapshot::SnapshotManager;
use crate::storage::engines::eventlog::{EntityId, Event, EventLogConfig, EventSequence};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::debug;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Temporal query engine for time travel
#[allow(dead_code)]
pub struct TemporalQueryEngine {
    /// Event index for looking up events
    event_index: Arc<EventIndex>,

    /// Snapshot manager for efficient state reconstruction
    snapshot_manager: Arc<SnapshotManager>,

    /// Configuration
    config: EventLogConfig,
}

impl TemporalQueryEngine {
    /// Create a new temporal query engine
    pub fn new(
        event_index: Arc<EventIndex>,
        snapshot_manager: Arc<SnapshotManager>,
        config: EventLogConfig,
    ) -> Result<Self> {
        Ok(Self {
            event_index,
            snapshot_manager,
            config,
        })
    }

    /// Get entity state as of a specific point in time
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to query
    /// * `as_of` - Point in time (None for current state)
    ///
    /// # Returns
    ///
    /// Entity state as JSON at the specified time
    pub async fn get_state_as_of(
        &self,
        entity_id: &EntityId,
        as_of: Option<DateTime<Utc>>,
    ) -> Result<serde_json::Value> {
        debug!("Getting state for {} as of {:?}", entity_id, as_of);

        // Try to find a snapshot before the as_of time
        let snapshot_sequence = if let Some(time) = as_of {
            self.snapshot_manager
                .find_snapshot_before(entity_id, time)
                .await?
        } else {
            None
        };

        // Determine starting sequence
        let from_sequence = snapshot_sequence.unwrap_or(0) + 1;

        // Get event sequences from index
        let sequences = if let Some(time) = as_of {
            self.get_sequences_before(entity_id, time).await?
        } else {
            self.event_index
                .get_entity_events(entity_id, from_sequence, usize::MAX)
                .await?
        };

        // Load events and replay
        let mut state = if let Some(seq) = snapshot_sequence {
            // Start from snapshot
            debug!("Using snapshot at sequence {} as base", seq);
            self.snapshot_manager.load_snapshot(entity_id, seq).await?
        } else {
            // Start from empty state
            serde_json::json!({})
        };

        // Apply events
        for sequence in sequences {
            // Skip if before snapshot
            if let Some(snapshot_seq) = snapshot_sequence
                && sequence <= snapshot_seq
            {
                continue;
            }

            // Load event
            let event_info = self.event_index.get_event_info(sequence).await?;
            let (event_entity_id, _event_type, _timestamp) = event_info.ok_or_else(|| {
                ProximaDBError::Internal(format!("Event {} not indexed", sequence))
            })?;

            // In production, we'd load the full event from storage here
            // For now, apply a placeholder update
            if &event_entity_id == entity_id {
                state = self.apply_event_to_state(state, sequence);
            }
        }

        Ok(state)
    }

    /// Get sequences before a specific timestamp
    async fn get_sequences_before(
        &self,
        entity_id: &EntityId,
        before: DateTime<Utc>,
    ) -> Result<Vec<EventSequence>> {
        // Get all entity events
        let all_sequences = self
            .event_index
            .get_entity_events(entity_id, 0, usize::MAX)
            .await?;

        // Filter by timestamp
        let mut filtered = Vec::new();
        for sequence in all_sequences {
            if let Some((_entity_id, _event_type, timestamp)) =
                self.event_index.get_event_info(sequence).await?
            {
                if timestamp <= before {
                    filtered.push(sequence);
                } else {
                    break; // Sequences are ordered
                }
            }
        }

        Ok(filtered)
    }

    /// Replay events to reconstruct state
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to replay
    /// * `from_sequence` - Starting sequence
    /// * `to_sequence` - Ending sequence (None for latest)
    ///
    /// # Returns
    ///
    /// Vector of events in replay order
    pub async fn replay(
        &self,
        entity_id: &EntityId,
        from_sequence: EventSequence,
        to_sequence: Option<EventSequence>,
    ) -> Result<Vec<Event>> {
        debug!(
            "Replaying events for {} from {} to {:?}",
            entity_id, from_sequence, to_sequence
        );

        let limit = to_sequence.map_or(usize::MAX, |s| (s - from_sequence + 1) as usize);
        let sequences = self
            .event_index
            .get_entity_events(entity_id, from_sequence, limit)
            .await?;

        // Filter by to_sequence if specified
        let sequences: Vec<EventSequence> = if let Some(to_seq) = to_sequence {
            sequences.into_iter().filter(|&s| s <= to_seq).collect()
        } else {
            sequences
        };

        // In production, we'd load the full events from storage
        // For now, return placeholder events with sequence numbers
        let mut events = Vec::new();
        for sequence in sequences {
            if let Some((entity_id, event_type, timestamp)) =
                self.event_index.get_event_info(sequence).await?
            {
                events.push(Event {
                    sequence,
                    entity_id,
                    event_type,
                    data: serde_json::json!({"reconstructed": true}),
                    timestamp,
                    causation_id: None,
                    metadata: Default::default(),
                });
            }
        }

        Ok(events)
    }

    /// Get entity version history
    ///
    /// # Arguments
    ///
    /// * `entity_id` - Entity to get history for
    /// * `limit` - Maximum versions to return
    ///
    /// # Returns
    ///
    /// Vector of (sequence, timestamp, event_type) tuples
    pub async fn get_version_history(
        &self,
        entity_id: &EntityId,
        limit: usize,
    ) -> Result<Vec<(EventSequence, DateTime<Utc>, String)>> {
        debug!(
            "Getting version history for {} (limit {})",
            entity_id, limit
        );

        let sequences = self
            .event_index
            .get_entity_events(entity_id, 0, limit)
            .await?;

        let mut history = Vec::new();
        for sequence in sequences {
            if let Some((_entity_id, event_type, timestamp)) =
                self.event_index.get_event_info(sequence).await?
            {
                history.push((sequence, timestamp, event_type));
            }
        }

        Ok(history)
    }

    /// Apply event to state (simplified - in production would use event handlers)
    fn apply_event_to_state(
        &self,
        mut state: serde_json::Value,
        _sequence: EventSequence,
    ) -> serde_json::Value {
        // In production, this would call event-specific handlers
        // For now, just update a placeholder
        let current_version = state.get("version").and_then(|v| v.as_i64()).unwrap_or(0);

        if let Some(obj) = state.as_object_mut() {
            obj.insert(
                "version".to_string(),
                serde_json::json!(current_version + 1),
            );
        }
        state
    }

    /// Check if entity exists at a point in time
    pub async fn entity_exists_at(&self, entity_id: &EntityId, at: DateTime<Utc>) -> Result<bool> {
        let sequences = self.get_sequences_before(entity_id, at).await?;
        Ok(!sequences.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::eventlog::{Event, EventLogConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_temporal_engine_creation() {
        let base_dir = PathBuf::from("/tmp/test_temporal_engine");
        let event_index = Arc::new(
            EventIndex::new(base_dir.clone()).expect("Failed to create event index for test"),
        );
        let snapshot_manager = Arc::new(
            SnapshotManager::new(base_dir.clone())
                .expect("Failed to create snapshot manager for test"),
        );
        let config = EventLogConfig::default();

        let engine = TemporalQueryEngine::new(event_index, snapshot_manager, config)
            .expect("Failed to create temporal query engine for test");
        assert_eq!(engine.config.snapshot_interval, 1000);
    }

    #[tokio::test]
    async fn test_entity_exists_at() {
        let base_dir = PathBuf::from("/tmp/test_entity_exists");
        let event_index = Arc::new(
            EventIndex::new(base_dir.clone()).expect("Failed to create event index for test"),
        );
        let snapshot_manager = Arc::new(
            SnapshotManager::new(base_dir.clone())
                .expect("Failed to create snapshot manager for test"),
        );
        let config = EventLogConfig::default();

        let engine = TemporalQueryEngine::new(event_index, snapshot_manager, config)
            .expect("Failed to create temporal query engine for test");

        // Index an event
        let event = Event {
            sequence: 1,
            entity_id: "account:test".to_string(),
            event_type: "AccountCreated".to_string(),
            data: serde_json::json!({}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        engine
            .event_index
            .index_event(&event)
            .await
            .expect("Failed to index event in test");

        // Check existence
        let exists = engine
            .entity_exists_at(&"account:test".to_string(), Utc::now())
            .await
            .expect("Failed to check entity existence in test");
        assert!(exists);

        // Check before event (should not exist)
        let before = Utc::now() - chrono::Duration::hours(1);
        let exists_before = engine
            .entity_exists_at(&"account:test".to_string(), before)
            .await
            .expect("Failed to check entity existence in test");
        assert!(!exists_before);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }
}
