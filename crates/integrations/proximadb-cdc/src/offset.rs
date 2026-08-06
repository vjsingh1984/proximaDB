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

//! Offset storage for CDC position tracking
//!
//! This module provides durable offset storage for CDC connectors,
//! enabling resume capability after restarts or failures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::error::CdcResult;

/// Unique identifier for an offset
pub type OffsetKey = String;

/// Offset data representing position in a source
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Offset {
    /// Source identifier (e.g., "postgres://host/db")
    pub source_id: String,
    /// Partition or shard identifier
    pub partition: Option<String>,
    /// Log sequence number or position
    pub lsn: u64,
    /// Transaction ID if applicable
    pub txn_id: Option<String>,
    /// Timestamp of the offset
    pub timestamp: u64,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl Offset {
    /// Create a new offset
    pub fn new(source_id: impl Into<String>, lsn: u64) -> Self {
        Self {
            source_id: source_id.into(),
            partition: None,
            lsn,
            txn_id: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            metadata: HashMap::new(),
        }
    }

    /// Set partition
    pub fn with_partition(mut self, partition: impl Into<String>) -> Self {
        self.partition = Some(partition.into());
        self
    }

    /// Set transaction ID
    pub fn with_txn_id(mut self, txn_id: impl Into<String>) -> Self {
        self.txn_id = Some(txn_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Generate a unique key for this offset
    pub fn key(&self) -> OffsetKey {
        match &self.partition {
            Some(p) => format!("{}:{}", self.source_id, p),
            None => self.source_id.clone(),
        }
    }
}

/// Trait for offset storage implementations
#[async_trait::async_trait]
pub trait OffsetStore: Send + Sync {
    /// Store an offset
    async fn store(&self, offset: &Offset) -> CdcResult<()>;

    /// Retrieve an offset by key
    async fn get(&self, key: &str) -> CdcResult<Option<Offset>>;

    /// Get all offsets for a source
    async fn get_all(&self, source_id: &str) -> CdcResult<Vec<Offset>>;

    /// Delete an offset
    async fn delete(&self, key: &str) -> CdcResult<()>;

    /// Flush any buffered writes
    async fn flush(&self) -> CdcResult<()>;
}

/// File-based offset store using JSON
pub struct FileOffsetStore {
    /// Base directory for offset files
    base_path: PathBuf,
    /// In-memory cache
    cache: Arc<RwLock<HashMap<OffsetKey, Offset>>>,
    /// Dirty flag for deferred writes
    dirty: Arc<RwLock<bool>>,
}

impl FileOffsetStore {
    /// Create a new file-based offset store
    pub async fn new(base_path: impl AsRef<Path>) -> CdcResult<Self> {
        let base_path = base_path.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        tokio::fs::create_dir_all(&base_path).await?;

        let store = Self {
            base_path,
            cache: Arc::new(RwLock::new(HashMap::new())),
            dirty: Arc::new(RwLock::new(false)),
        };

        // Load existing offsets
        store.load().await?;

        Ok(store)
    }

    /// Load offsets from disk
    async fn load(&self) -> CdcResult<()> {
        let offsets_file = self.base_path.join("offsets.json");

        if !offsets_file.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&offsets_file).await?;
        let offsets: HashMap<OffsetKey, Offset> = serde_json::from_str(&content)?;

        let mut cache = self.cache.write().await;
        *cache = offsets;

        Ok(())
    }

    /// Save offsets to disk
    async fn save(&self) -> CdcResult<()> {
        let cache = self.cache.read().await;
        let content = serde_json::to_string_pretty(&*cache)?;

        let offsets_file = self.base_path.join("offsets.json");
        let temp_file = self.base_path.join("offsets.json.tmp");

        // Write to temp file first for atomicity
        tokio::fs::write(&temp_file, &content).await?;
        tokio::fs::rename(&temp_file, &offsets_file).await?;

        let mut dirty = self.dirty.write().await;
        *dirty = false;

        Ok(())
    }

    /// Get the base path
    pub fn base_path(&self) -> &Path {
        &self.base_path
    }
}

#[async_trait::async_trait]
impl OffsetStore for FileOffsetStore {
    async fn store(&self, offset: &Offset) -> CdcResult<()> {
        let key = offset.key();

        {
            let mut cache = self.cache.write().await;
            cache.insert(key, offset.clone());
        }

        {
            let mut dirty = self.dirty.write().await;
            *dirty = true;
        }

        // Auto-flush for durability
        self.save().await?;

        Ok(())
    }

    async fn get(&self, key: &str) -> CdcResult<Option<Offset>> {
        let cache = self.cache.read().await;
        Ok(cache.get(key).cloned())
    }

    async fn get_all(&self, source_id: &str) -> CdcResult<Vec<Offset>> {
        let cache = self.cache.read().await;
        let offsets: Vec<Offset> = cache
            .values()
            .filter(|o| o.source_id == source_id)
            .cloned()
            .collect();
        Ok(offsets)
    }

    async fn delete(&self, key: &str) -> CdcResult<()> {
        {
            let mut cache = self.cache.write().await;
            cache.remove(key);
        }

        {
            let mut dirty = self.dirty.write().await;
            *dirty = true;
        }

        self.save().await?;

        Ok(())
    }

    async fn flush(&self) -> CdcResult<()> {
        let dirty = *self.dirty.read().await;
        if dirty {
            self.save().await?;
        }
        Ok(())
    }
}

/// In-memory offset store for testing
pub struct MemoryOffsetStore {
    offsets: Arc<RwLock<HashMap<OffsetKey, Offset>>>,
}

impl MemoryOffsetStore {
    /// Create a new in-memory offset store
    pub fn new() -> Self {
        Self {
            offsets: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemoryOffsetStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl OffsetStore for MemoryOffsetStore {
    async fn store(&self, offset: &Offset) -> CdcResult<()> {
        let key = offset.key();
        let mut offsets = self.offsets.write().await;
        offsets.insert(key, offset.clone());
        Ok(())
    }

    async fn get(&self, key: &str) -> CdcResult<Option<Offset>> {
        let offsets = self.offsets.read().await;
        Ok(offsets.get(key).cloned())
    }

    async fn get_all(&self, source_id: &str) -> CdcResult<Vec<Offset>> {
        let offsets = self.offsets.read().await;
        let results: Vec<Offset> = offsets
            .values()
            .filter(|o| o.source_id == source_id)
            .cloned()
            .collect();
        Ok(results)
    }

    async fn delete(&self, key: &str) -> CdcResult<()> {
        let mut offsets = self.offsets.write().await;
        offsets.remove(key);
        Ok(())
    }

    async fn flush(&self) -> CdcResult<()> {
        // No-op for memory store
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_offset_creation() {
        let offset = Offset::new("pg://localhost/mydb", 12345);
        assert_eq!(offset.source_id, "pg://localhost/mydb");
        assert_eq!(offset.lsn, 12345);
        assert!(offset.partition.is_none());
    }

    #[test]
    fn test_offset_with_partition() {
        let offset = Offset::new("pg://localhost/mydb", 12345).with_partition("table_0");
        assert_eq!(offset.key(), "pg://localhost/mydb:table_0");
    }

    #[test]
    fn test_offset_with_metadata() {
        let offset = Offset::new("pg://localhost/mydb", 12345)
            .with_metadata("slot", "cdc_slot_1")
            .with_txn_id("txn_abc");

        assert_eq!(offset.metadata.get("slot"), Some(&"cdc_slot_1".to_string()));
        assert_eq!(offset.txn_id, Some("txn_abc".to_string()));
    }

    #[tokio::test]
    async fn test_memory_store_basic() {
        let store = MemoryOffsetStore::new();

        let offset = Offset::new("source1", 100);
        store.store(&offset).await.expect("Failed to store offset");

        let retrieved = store
            .get(&offset.key())
            .await
            .expect("Failed to get offset");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("Offset should be present").lsn, 100);
    }

    #[tokio::test]
    async fn test_memory_store_get_all() {
        let store = MemoryOffsetStore::new();

        let offset1 = Offset::new("source1", 100).with_partition("p0");
        let offset2 = Offset::new("source1", 200).with_partition("p1");
        let offset3 = Offset::new("source2", 300);

        store
            .store(&offset1)
            .await
            .expect("Failed to store offset1");
        store
            .store(&offset2)
            .await
            .expect("Failed to store offset2");
        store
            .store(&offset3)
            .await
            .expect("Failed to store offset3");

        let source1_offsets = store
            .get_all("source1")
            .await
            .expect("Failed to get all offsets");
        assert_eq!(source1_offsets.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_store_delete() {
        let store = MemoryOffsetStore::new();

        let offset = Offset::new("source1", 100);
        store.store(&offset).await.expect("Failed to store offset");

        assert!(
            store
                .get(&offset.key())
                .await
                .expect("Failed to get offset")
                .is_some()
        );

        store
            .delete(&offset.key())
            .await
            .expect("Failed to delete offset");
        assert!(
            store
                .get(&offset.key())
                .await
                .expect("Failed to get offset")
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_file_store_persistence() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        // Create and populate store
        {
            let store = FileOffsetStore::new(temp_dir.path())
                .await
                .expect("Failed to create file store");

            let offset = Offset::new("pg://localhost/testdb", 12345)
                .with_partition("users")
                .with_metadata("slot", "cdc_slot");

            store.store(&offset).await.expect("Failed to store offset");
        }

        // Reload and verify
        {
            let store = FileOffsetStore::new(temp_dir.path())
                .await
                .expect("Failed to create file store");

            let offset = store
                .get("pg://localhost/testdb:users")
                .await
                .expect("Failed to get offset");
            assert!(offset.is_some());

            let offset = offset.expect("Offset should be present");
            assert_eq!(offset.lsn, 12345);
            assert_eq!(offset.metadata.get("slot"), Some(&"cdc_slot".to_string()));
        }
    }

    #[tokio::test]
    async fn test_file_store_atomic_write() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let store = FileOffsetStore::new(temp_dir.path())
            .await
            .expect("Failed to create file store");

        // Store multiple offsets
        for i in 0..10 {
            let offset = Offset::new("source", i * 100);
            store.store(&offset).await.expect("Failed to store offset");
        }

        // Verify final state
        let offset = store
            .get("source")
            .await
            .expect("Failed to get offset")
            .expect("Offset should be present");
        assert_eq!(offset.lsn, 900);
    }

    #[test]
    fn test_offset_serialization() {
        let offset = Offset::new("source1", 12345)
            .with_partition("p0")
            .with_txn_id("txn_1")
            .with_metadata("key", "value");

        let json = serde_json::to_string(&offset).expect("Failed to serialize offset");
        let deserialized: Offset =
            serde_json::from_str(&json).expect("Failed to deserialize offset");

        assert_eq!(offset, deserialized);
    }
}
