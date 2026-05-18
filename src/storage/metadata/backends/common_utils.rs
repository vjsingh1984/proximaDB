// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Common Utilities for Metadata Backends
//!
//! Provides shared functionality used across different metadata backend implementations
//! to reduce code duplication and ensure consistency.

use anyhow::{Context, Result};
use proximadb_kernel::uuid::Uuid;
use std::path::{Path, PathBuf};

use crate::proto::proximadb_v1::Collection;
use crate::storage::persistence::filesystem::FileSystem;

/// Common serialization utilities
pub mod serialization {
    use super::*;
    use prost::Message;

    /// Serialize a Collection to protobuf bytes
    pub fn serialize_collection(collection: &Collection) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        collection
            .encode(&mut buf)
            .context("Failed to encode collection to protobuf")?;
        Ok(buf)
    }

    /// Deserialize a Collection from protobuf bytes
    pub fn deserialize_collection(data: &[u8]) -> Result<Collection> {
        Collection::decode(data).context("Failed to decode collection from protobuf")
    }
}

/// Common path utilities
pub mod paths {
    use super::*;

    /// Generate a collection file path
    pub fn collection_path(base: &Path, collection_id: &str) -> PathBuf {
        base.join("collections")
            .join(format!("{}.pb", collection_id))
    }

    /// Generate a UUID index path
    pub fn uuid_index_path(base: &Path) -> PathBuf {
        base.join("uuid_index.pb")
    }

    /// Generate a snapshot path
    pub fn snapshot_path(base: &Path, timestamp: u64) -> PathBuf {
        base.join("snapshots")
            .join(format!("snapshot_{}.pb", timestamp))
    }

    /// Generate a backup path
    pub fn backup_path(base: &Path, collection_id: &str, timestamp: u64) -> PathBuf {
        base.join("backups")
            .join(collection_id)
            .join(format!("backup_{}.pb", timestamp))
    }
}

/// Common validation utilities
pub mod validation {
    use super::*;

    /// Validate a collection ID
    pub fn validate_collection_id(id: &str) -> Result<()> {
        if id.is_empty() {
            anyhow::bail!("Collection ID cannot be empty");
        }

        if id.len() > 256 {
            anyhow::bail!("Collection ID too long (max 256 characters)");
        }

        // Check for invalid characters
        if !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            anyhow::bail!("Collection ID contains invalid characters");
        }

        Ok(())
    }

    /// Validate a Collection object
    pub fn validate_collection(collection: &Collection) -> Result<()> {
        validate_collection_id(&collection.id)?;

        if collection.config.is_none() {
            anyhow::bail!("Collection config is required");
        }

        let Some(config) = collection.config.as_ref() else {
            anyhow::bail!("Collection config is required");
        };
        if config.name.is_empty() {
            anyhow::bail!("Collection name cannot be empty");
        }

        Ok(())
    }
}

/// Common UUID management
pub mod uuid_management {
    use super::*;
    use std::collections::HashMap;

    /// Generate a new UUID for a collection
    pub fn generate_uuid() -> String {
        Uuid::new_v4().to_string()
    }

    /// UUID index for managing collection ID to UUID mappings
    #[derive(Clone, Default)]
    pub struct UuidIndex {
        mappings: HashMap<String, String>,
    }

    impl UuidIndex {
        /// Create a new empty index
        pub fn new() -> Self {
            Self {
                mappings: HashMap::new(),
            }
        }

        /// Add a mapping
        pub fn insert(&mut self, collection_id: String, uuid: String) {
            self.mappings.insert(collection_id, uuid);
        }

        /// Get a UUID for a collection ID
        pub fn get(&self, collection_id: &str) -> Option<String> {
            self.mappings.get(collection_id).cloned()
        }

        /// Remove a mapping
        pub fn remove(&mut self, collection_id: &str) -> Option<String> {
            self.mappings.remove(collection_id)
        }

        /// Get all mappings
        pub fn all(&self) -> &HashMap<String, String> {
            &self.mappings
        }
    }
}

/// Common retry utilities
pub mod retry {
    use super::*;
    use tokio::time::{Duration, sleep};

    /// Retry configuration
    pub struct RetryConfig {
        pub max_attempts: u32,
        pub initial_delay_ms: u64,
        pub max_delay_ms: u64,
        pub exponential_base: f64,
    }

    impl Default for RetryConfig {
        fn default() -> Self {
            Self {
                max_attempts: 3,
                initial_delay_ms: 100,
                max_delay_ms: 5000,
                exponential_base: 2.0,
            }
        }
    }

    /// Execute an operation with retry logic
    pub async fn retry_operation<F, T, Fut>(config: &RetryConfig, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut delay = config.initial_delay_ms;

        for attempt in 1..=config.max_attempts {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) if attempt < config.max_attempts => {
                    tracing::warn!(
                        "Operation failed (attempt {}/{}): {}",
                        attempt,
                        config.max_attempts,
                        e
                    );

                    sleep(Duration::from_millis(delay)).await;

                    // Exponential backoff
                    delay = ((delay as f64) * config.exponential_base) as u64;
                    delay = delay.min(config.max_delay_ms);
                }
                Err(e) => return Err(e),
            }
        }

        unreachable!("Retry loop should have returned")
    }
}

/// Common batch operation utilities
pub mod batch {

    /// Batch size configuration
    pub struct BatchConfig {
        pub max_batch_size: usize,
        pub max_batch_bytes: usize,
    }

    impl Default for BatchConfig {
        fn default() -> Self {
            Self {
                max_batch_size: 100,
                max_batch_bytes: 10 * 1024 * 1024, // 10MB
            }
        }
    }

    /// Create batches from a list of items
    pub fn create_batches<T>(
        items: Vec<T>,
        config: &BatchConfig,
        size_fn: impl Fn(&T) -> usize,
    ) -> Vec<Vec<T>> {
        let mut batches = Vec::new();
        let mut current_batch = Vec::new();
        let mut current_size = 0;

        for item in items {
            let item_size = size_fn(&item);

            if !current_batch.is_empty()
                && (current_batch.len() >= config.max_batch_size
                    || current_size + item_size > config.max_batch_bytes)
            {
                batches.push(current_batch);
                current_batch = Vec::new();
                current_size = 0;
            }

            current_size += item_size;
            current_batch.push(item);
        }

        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        batches
    }
}

/// Common filesystem utilities
pub mod filesystem {
    use super::*;

    /// Ensure a directory exists
    pub async fn ensure_directory<F: FileSystem>(fs: &F, path: &str) -> Result<()> {
        if !fs.exists(path).await? {
            fs.create_dir_all(path)
                .await
                .with_context(|| format!("Failed to create directory: {}", path))?;
        }
        Ok(())
    }

    /// List all files with a specific extension in a directory
    pub async fn list_files_with_extension<F: FileSystem>(
        fs: &F,
        dir: &str,
        extension: &str,
    ) -> Result<Vec<String>> {
        let entries = fs.list(dir).await?;

        let mut files = Vec::new();
        for entry in entries {
            if entry.name.ends_with(&format!(".{}", extension)) {
                files.push(entry.url);
            }
        }

        files.sort();
        Ok(files)
    }

    /// Atomic file write (write to temp file, then rename)
    pub async fn atomic_write<F: FileSystem>(fs: &F, path: &str, data: Vec<u8>) -> Result<()> {
        let temp_path = format!("{}.tmp", path);

        // Write to temp file
        fs.write(&temp_path, &data, None)
            .await
            .with_context(|| format!("Failed to write temp file: {}", temp_path))?;

        // Atomic rename
        fs.move_file(&temp_path, path)
            .await
            .with_context(|| format!("Failed to rename temp file to: {}", path))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_collection_id() {
        use validation::validate_collection_id;

        assert!(validate_collection_id("valid_collection-123").is_ok());
        assert!(validate_collection_id("").is_err());
        assert!(validate_collection_id(&"a".repeat(257)).is_err());
        assert!(validate_collection_id("invalid/collection").is_err());
        assert!(validate_collection_id("invalid collection").is_err());
    }

    #[test]
    fn test_uuid_index() {
        use uuid_management::UuidIndex;

        let mut index = UuidIndex::new();
        index.insert("collection1".to_string(), "uuid1".to_string());

        assert_eq!(index.get("collection1"), Some("uuid1".to_string()));
        assert_eq!(index.get("collection2"), None);

        assert_eq!(index.remove("collection1"), Some("uuid1".to_string()));
        assert_eq!(index.get("collection1"), None);
    }

    #[test]
    fn test_create_batches() {
        use batch::{BatchConfig, create_batches};

        let items = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let config = BatchConfig {
            max_batch_size: 3,
            max_batch_bytes: 1000,
        };

        let batches = create_batches(items, &config, |_| 1);

        assert_eq!(batches.len(), 4);
        assert_eq!(batches[0].len(), 3);
        assert_eq!(batches[1].len(), 3);
        assert_eq!(batches[2].len(), 3);
        assert_eq!(batches[3].len(), 1);
    }
}
