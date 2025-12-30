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

//! Position tracking for outbound CDC subscriptions

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::cdc::error::{CdcError, CdcResult};

/// Position in the WAL stream
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    /// Log sequence number
    pub lsn: u64,
    /// Segment file (if applicable)
    pub segment: Option<String>,
    /// Offset within segment
    pub offset: Option<u64>,
    /// Timestamp when position was recorded
    pub timestamp: u64,
}

impl Position {
    /// Create a new position from LSN
    pub fn from_lsn(lsn: u64) -> Self {
        Self {
            lsn,
            segment: None,
            offset: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Create a position with segment info
    pub fn with_segment(lsn: u64, segment: impl Into<String>, offset: u64) -> Self {
        Self {
            lsn,
            segment: Some(segment.into()),
            offset: Some(offset),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Create the beginning position
    pub fn beginning() -> Self {
        Self::from_lsn(0)
    }

    /// Create the latest position (will be resolved at runtime)
    pub fn latest() -> Self {
        Self::from_lsn(u64::MAX)
    }

    /// Check if this is the beginning position
    pub fn is_beginning(&self) -> bool {
        self.lsn == 0
    }

    /// Check if this is the latest position
    pub fn is_latest(&self) -> bool {
        self.lsn == u64::MAX
    }
}

/// Tracker for subscription positions
pub struct PositionTracker {
    /// Current positions per subscription
    positions: RwLock<HashMap<String, Position>>,
    /// Pending (unacknowledged) positions
    pending: RwLock<HashMap<String, Vec<Position>>>,
    /// Position store for persistence
    store: Option<Box<dyn PositionStore>>,
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PositionTracker {
    /// Create a new position tracker
    pub fn new() -> Self {
        Self {
            positions: RwLock::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
            store: None,
        }
    }

    /// Create with a position store
    pub fn with_store(store: Box<dyn PositionStore>) -> Self {
        Self {
            positions: RwLock::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
            store: Some(store),
        }
    }

    /// Get the current position for a subscription
    pub fn get(&self, subscription_id: &str) -> Option<Position> {
        self.positions.read().unwrap().get(subscription_id).cloned()
    }

    /// Set the current position for a subscription
    pub fn set(&self, subscription_id: &str, position: Position) {
        self.positions
            .write()
            .unwrap()
            .insert(subscription_id.to_string(), position);
    }

    /// Mark a position as pending (dispatched but not acked)
    pub fn mark_pending(&self, subscription_id: &str, position: Position) {
        let mut pending = self.pending.write().unwrap();
        pending
            .entry(subscription_id.to_string())
            .or_default()
            .push(position);
    }

    /// Acknowledge a position
    pub fn acknowledge(&self, subscription_id: &str, lsn: u64) -> CdcResult<()> {
        let mut pending = self.pending.write().unwrap();

        if let Some(positions) = pending.get_mut(subscription_id) {
            // Remove all positions up to and including this LSN
            positions.retain(|p| p.lsn > lsn);

            // Update committed position
            self.positions.write().unwrap().insert(
                subscription_id.to_string(),
                Position::from_lsn(lsn),
            );
        }

        Ok(())
    }

    /// Get pending count for a subscription
    pub fn pending_count(&self, subscription_id: &str) -> usize {
        self.pending
            .read()
            .unwrap()
            .get(subscription_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Check if subscription has any pending positions
    pub fn has_pending(&self, subscription_id: &str) -> bool {
        self.pending_count(subscription_id) > 0
    }

    /// Load position from store
    pub async fn load(&self, subscription_id: &str) -> CdcResult<Option<Position>> {
        if let Some(ref store) = self.store {
            let position = store.load(subscription_id).await?;
            if let Some(ref pos) = position {
                self.set(subscription_id, pos.clone());
            }
            Ok(position)
        } else {
            Ok(self.get(subscription_id))
        }
    }

    /// Save position to store (checkpoint)
    pub async fn checkpoint(&self, subscription_id: &str) -> CdcResult<()> {
        if let Some(ref store) = self.store {
            if let Some(position) = self.get(subscription_id) {
                store.save(subscription_id, &position).await?;
            }
        }
        Ok(())
    }

    /// Get oldest pending position (for timeout detection)
    pub fn oldest_pending(&self, subscription_id: &str) -> Option<Position> {
        self.pending
            .read()
            .unwrap()
            .get(subscription_id)
            .and_then(|v| v.first().cloned())
    }

    /// Clear all state for a subscription
    pub fn clear(&self, subscription_id: &str) {
        self.positions.write().unwrap().remove(subscription_id);
        self.pending.write().unwrap().remove(subscription_id);
    }
}

/// Trait for persisting positions
#[async_trait::async_trait]
pub trait PositionStore: Send + Sync {
    /// Load a position
    async fn load(&self, subscription_id: &str) -> CdcResult<Option<Position>>;

    /// Save a position
    async fn save(&self, subscription_id: &str, position: &Position) -> CdcResult<()>;

    /// Delete a position
    async fn delete(&self, subscription_id: &str) -> CdcResult<()>;

    /// List all subscription IDs
    async fn list(&self) -> CdcResult<Vec<String>>;
}

/// File-based position store
pub struct FilePositionStore {
    path: PathBuf,
}

impl FilePositionStore {
    /// Create a new file position store
    pub fn new(path: impl Into<PathBuf>) -> CdcResult<Self> {
        let path = path.into();
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn position_file(&self, subscription_id: &str) -> PathBuf {
        self.path.join(format!("{}.pos", subscription_id))
    }
}

#[async_trait::async_trait]
impl PositionStore for FilePositionStore {
    async fn load(&self, subscription_id: &str) -> CdcResult<Option<Position>> {
        let path = self.position_file(subscription_id);
        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let position: Position = serde_json::from_str(&content)
            .map_err(|e| CdcError::Serialization(e.to_string()))?;
        Ok(Some(position))
    }

    async fn save(&self, subscription_id: &str, position: &Position) -> CdcResult<()> {
        let path = self.position_file(subscription_id);
        let content = serde_json::to_string_pretty(position)
            .map_err(|e| CdcError::Serialization(e.to_string()))?;

        // Atomic write
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, content).await?;
        tokio::fs::rename(&temp_path, &path).await?;

        Ok(())
    }

    async fn delete(&self, subscription_id: &str) -> CdcResult<()> {
        let path = self.position_file(subscription_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn list(&self) -> CdcResult<Vec<String>> {
        let mut ids = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map(|e| e == "pos").unwrap_or(false) {
                if let Some(stem) = path.file_stem() {
                    ids.push(stem.to_string_lossy().to_string());
                }
            }
        }

        Ok(ids)
    }
}

/// In-memory position store (for testing)
pub struct MemoryPositionStore {
    positions: RwLock<HashMap<String, Position>>,
}

impl Default for MemoryPositionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryPositionStore {
    /// Create a new memory position store
    pub fn new() -> Self {
        Self {
            positions: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl PositionStore for MemoryPositionStore {
    async fn load(&self, subscription_id: &str) -> CdcResult<Option<Position>> {
        Ok(self.positions.read().unwrap().get(subscription_id).cloned())
    }

    async fn save(&self, subscription_id: &str, position: &Position) -> CdcResult<()> {
        self.positions
            .write()
            .unwrap()
            .insert(subscription_id.to_string(), position.clone());
        Ok(())
    }

    async fn delete(&self, subscription_id: &str) -> CdcResult<()> {
        self.positions.write().unwrap().remove(subscription_id);
        Ok(())
    }

    async fn list(&self) -> CdcResult<Vec<String>> {
        Ok(self.positions.read().unwrap().keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_from_lsn() {
        let pos = Position::from_lsn(12345);
        assert_eq!(pos.lsn, 12345);
        assert!(pos.segment.is_none());
        assert!(pos.offset.is_none());
    }

    #[test]
    fn test_position_with_segment() {
        let pos = Position::with_segment(12345, "segment_001", 1000);
        assert_eq!(pos.lsn, 12345);
        assert_eq!(pos.segment, Some("segment_001".to_string()));
        assert_eq!(pos.offset, Some(1000));
    }

    #[test]
    fn test_position_special_values() {
        let begin = Position::beginning();
        assert!(begin.is_beginning());
        assert!(!begin.is_latest());

        let latest = Position::latest();
        assert!(latest.is_latest());
        assert!(!latest.is_beginning());
    }

    #[test]
    fn test_position_ordering() {
        let p1 = Position::from_lsn(100);
        let p2 = Position::from_lsn(200);
        let p3 = Position::from_lsn(100);

        assert!(p1 < p2);
        assert!(p2 > p1);
        assert_eq!(p1, p3);
    }

    #[test]
    fn test_position_tracker() {
        let tracker = PositionTracker::new();

        assert!(tracker.get("sub1").is_none());

        tracker.set("sub1", Position::from_lsn(100));
        assert_eq!(tracker.get("sub1").unwrap().lsn, 100);

        tracker.set("sub1", Position::from_lsn(200));
        assert_eq!(tracker.get("sub1").unwrap().lsn, 200);
    }

    #[test]
    fn test_pending_tracking() {
        let tracker = PositionTracker::new();

        assert_eq!(tracker.pending_count("sub1"), 0);
        assert!(!tracker.has_pending("sub1"));

        tracker.mark_pending("sub1", Position::from_lsn(100));
        tracker.mark_pending("sub1", Position::from_lsn(200));
        tracker.mark_pending("sub1", Position::from_lsn(300));

        assert_eq!(tracker.pending_count("sub1"), 3);
        assert!(tracker.has_pending("sub1"));

        // Acknowledge up to 200
        tracker.acknowledge("sub1", 200).unwrap();
        assert_eq!(tracker.pending_count("sub1"), 1);
        assert_eq!(tracker.get("sub1").unwrap().lsn, 200);
    }

    #[test]
    fn test_oldest_pending() {
        let tracker = PositionTracker::new();

        assert!(tracker.oldest_pending("sub1").is_none());

        tracker.mark_pending("sub1", Position::from_lsn(100));
        tracker.mark_pending("sub1", Position::from_lsn(200));

        let oldest = tracker.oldest_pending("sub1").unwrap();
        assert_eq!(oldest.lsn, 100);
    }

    #[test]
    fn test_clear() {
        let tracker = PositionTracker::new();

        tracker.set("sub1", Position::from_lsn(100));
        tracker.mark_pending("sub1", Position::from_lsn(200));

        tracker.clear("sub1");

        assert!(tracker.get("sub1").is_none());
        assert!(!tracker.has_pending("sub1"));
    }

    #[tokio::test]
    async fn test_memory_position_store() {
        let store = MemoryPositionStore::new();

        // Initially empty
        assert!(store.load("sub1").await.unwrap().is_none());

        // Save and load
        store.save("sub1", &Position::from_lsn(100)).await.unwrap();
        let loaded = store.load("sub1").await.unwrap().unwrap();
        assert_eq!(loaded.lsn, 100);

        // List
        store.save("sub2", &Position::from_lsn(200)).await.unwrap();
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 2);

        // Delete
        store.delete("sub1").await.unwrap();
        assert!(store.load("sub1").await.unwrap().is_none());
    }
}
