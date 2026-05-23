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

//! # Event Index for Fast Lookups
//!
//! This module provides indexing for events by entity, type, and timestamp
//! to enable efficient temporal queries and event replay.

use crate::storage::engines::eventlog::{EntityId, Event, EventSequence, EventType};
use chrono::{DateTime, Utc};
use proximadb_kernel::error::ProximaDBError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Event index for fast lookups
#[allow(dead_code)]
pub struct EventIndex {
    /// Base directory for index storage
    base_dir: PathBuf,

    /// Entity index: entity_id -> Vec<sequence>
    entity_index: Arc<RwLock<HashMap<EntityId, Vec<EventSequence>>>>,

    /// Event type index: event_type -> Vec<sequence>
    type_index: Arc<RwLock<HashMap<EventType, Vec<EventSequence>>>>,

    /// Timestamp index: timestamp -> Vec<sequence> (truncated to second)
    timestamp_index: Arc<RwLock<HashMap<i64, Vec<EventSequence>>>>,

    /// Reverse index: sequence -> (entity_id, event_type, timestamp)
    reverse_index: Arc<RwLock<HashMap<EventSequence, (EntityId, EventType, DateTime<Utc>)>>>,
}

impl EventIndex {
    /// Create a new event index
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        debug!("Creating event index at {:?}", base_dir);

        // Create index directory
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| ProximaDBError::Internal(format!("Failed to create index dir: {}", e)))?;

        Ok(Self {
            base_dir,
            entity_index: Arc::new(RwLock::new(HashMap::new())),
            type_index: Arc::new(RwLock::new(HashMap::new())),
            timestamp_index: Arc::new(RwLock::new(HashMap::new())),
            reverse_index: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Index an event for fast lookups
    pub async fn index_event(&self, event: &Event) -> Result<()> {
        let seq = event.sequence;
        let entity_id = event.entity_id.clone();
        let event_type = event.event_type.clone();
        let timestamp = event.timestamp;

        // Update entity index
        {
            let mut index = self.entity_index.write().await;
            index
                .entry(entity_id.clone())
                .or_insert_with(Vec::new)
                .push(seq);
        }

        // Update type index
        {
            let mut index = self.type_index.write().await;
            index
                .entry(event_type.clone())
                .or_insert_with(Vec::new)
                .push(seq);
        }

        // Update timestamp index (truncated to second)
        {
            let mut index = self.timestamp_index.write().await;
            let timestamp_key = timestamp.timestamp();
            index
                .entry(timestamp_key)
                .or_insert_with(Vec::new)
                .push(seq);
        }

        // Update reverse index
        {
            let mut index = self.reverse_index.write().await;
            index.insert(seq, (entity_id.clone(), event_type, timestamp));
        }

        debug!("Indexed event {} for entity {}", seq, entity_id);
        Ok(())
    }

    /// Get event sequences for an entity (sorted)
    pub async fn get_entity_events(
        &self,
        entity_id: &EntityId,
        from_sequence: EventSequence,
        limit: usize,
    ) -> Result<Vec<EventSequence>> {
        let index = self.entity_index.read().await;

        let sequences = match index.get(entity_id) {
            Some(seqs) => seqs
                .iter()
                .filter(|&&s| s >= from_sequence)
                .take(limit)
                .copied()
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };

        Ok(sequences)
    }

    /// Get event sequences by type
    pub async fn get_events_by_type(
        &self,
        event_type: &EventType,
        from_sequence: EventSequence,
        limit: usize,
    ) -> Result<Vec<EventSequence>> {
        let index = self.type_index.read().await;

        let sequences = match index.get(event_type) {
            Some(seqs) => seqs
                .iter()
                .filter(|&&s| s >= from_sequence)
                .take(limit)
                .copied()
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };

        Ok(sequences)
    }

    /// Get event sequences in a time range
    pub async fn get_events_by_timerange(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<EventSequence>> {
        let index = self.timestamp_index.read().await;

        let mut sequences = Vec::new();

        let from_ts = from.timestamp();
        let to_ts = to.timestamp();

        for (&ts, seqs) in index.iter() {
            if ts >= from_ts && ts <= to_ts {
                sequences.extend(seqs);
            }
        }

        sequences.sort(); // Ensure order
        Ok(sequences)
    }

    /// Get events for multiple entities (batch lookup)
    pub async fn get_entities_events(
        &self,
        entity_ids: &[EntityId],
    ) -> Result<HashMap<EntityId, Vec<EventSequence>>> {
        let index = self.entity_index.read().await;

        let mut result = HashMap::new();
        for entity_id in entity_ids {
            if let Some(sequences) = index.get(entity_id) {
                result.insert(entity_id.clone(), sequences.clone());
            }
        }

        Ok(result)
    }

    /// Get entity info at a specific sequence
    pub async fn get_event_info(
        &self,
        sequence: EventSequence,
    ) -> Result<Option<(EntityId, EventType, DateTime<Utc>)>> {
        let index = self.reverse_index.read().await;
        Ok(index.get(&sequence).cloned())
    }

    /// Get all entities tracked
    pub async fn get_all_entities(&self) -> Result<Vec<EntityId>> {
        let index = self.entity_index.read().await;
        Ok(index.keys().cloned().collect())
    }

    /// Get all event types
    pub async fn get_all_event_types(&self) -> Result<Vec<EventType>> {
        let index = self.type_index.read().await;
        Ok(index.keys().cloned().collect())
    }

    /// Purge events for an entity (admin operation)
    pub async fn purge_entity(&self, entity_id: &EntityId) -> Result<Vec<EventSequence>> {
        let mut entity_index = self.entity_index.write().await;

        let sequences = entity_index.remove(entity_id).unwrap_or_default();

        // Remove from other indices
        for &seq in &sequences {
            // Remove from reverse index
            let mut reverse_index = self.reverse_index.write().await;
            reverse_index.remove(&seq);

            // Note: We don't remove from type/timestamp indexes for simplicity
            // In production, we'd maintain reverse mappings
        }

        debug!("Purged {} events for entity {}", sequences.len(), entity_id);
        Ok(sequences)
    }

    /// Get index statistics
    pub async fn get_stats(&self) -> EventLogIndexStats {
        let entity_index = self.entity_index.read().await;
        let type_index = self.type_index.read().await;
        let reverse_index = self.reverse_index.read().await;

        EventLogIndexStats {
            total_entities: entity_index.len(),
            total_event_types: type_index.len(),
            total_indexed_events: reverse_index.len(),
        }
    }
}

/// Index statistics
#[derive(Debug, Clone, Default)]
pub struct EventLogIndexStats {
    pub total_entities: usize,
    pub total_event_types: usize,
    pub total_indexed_events: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::eventlog::Event;

    #[test]
    fn test_index_creation() {
        let base_dir = PathBuf::from("/tmp/test_event_index");
        let index = EventIndex::new(base_dir.clone()).unwrap();
        assert_eq!(index.base_dir, base_dir);
    }

    #[tokio::test]
    async fn test_index_event() {
        let base_dir = PathBuf::from("/tmp/test_index_event");
        let index = EventIndex::new(base_dir.clone()).unwrap();

        let event = Event {
            sequence: 1,
            entity_id: "account:123".to_string(),
            event_type: "AccountCreated".to_string(),
            data: serde_json::json!({"balance": 100}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        let result = index.index_event(&event).await;
        assert!(result.is_ok());

        // Verify entity index
        let entity_events = index
            .get_entity_events(&"account:123".to_string(), 0, 10)
            .await
            .unwrap();
        assert_eq!(entity_events, vec![1]);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }

    #[tokio::test]
    async fn test_get_entity_events() {
        let base_dir = PathBuf::from("/tmp/test_get_entity");
        let index = EventIndex::new(base_dir.clone()).unwrap();

        // Index multiple events
        for i in 1..=3 {
            let event = Event {
                sequence: i,
                entity_id: "order:456".to_string(),
                event_type: "OrderUpdated".to_string(),
                data: serde_json::json!({"version": i}),
                timestamp: Utc::now(),
                causation_id: None,
                metadata: HashMap::new(),
            };

            index.index_event(&event).await.unwrap();
        }

        // Get all events
        let events = index
            .get_entity_events(&"order:456".to_string(), 0, 10)
            .await
            .unwrap();
        assert_eq!(events, vec![1, 2, 3]);

        // Get from sequence 2
        let events = index
            .get_entity_events(&"order:456".to_string(), 2, 10)
            .await
            .unwrap();
        assert_eq!(events, vec![2, 3]);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }

    #[tokio::test]
    async fn test_get_stats() {
        let base_dir = PathBuf::from("/tmp/test_index_stats");
        let index = EventIndex::new(base_dir.clone()).unwrap();

        let event = Event {
            sequence: 1,
            entity_id: "entity:test".to_string(),
            event_type: "TestEvent".to_string(),
            data: serde_json::json!({}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        index.index_event(&event).await.unwrap();

        let stats = index.get_stats().await;
        assert_eq!(stats.total_entities, 1);
        assert_eq!(stats.total_event_types, 1);
        assert_eq!(stats.total_indexed_events, 1);

        // Cleanup
        let _ = std::fs::remove_dir_all(base_dir);
    }
}
