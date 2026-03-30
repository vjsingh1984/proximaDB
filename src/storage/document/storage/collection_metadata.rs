use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::proto::proximadb_v1::DocumentCollectionConfig;
use crate::storage::document::DocumentCollection;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCollectionMetadata {
    format_version: u32,
    name: String,
    config_json: String,
    document_count: u64,
    storage_size_bytes: u64,
    created_at_ns: i64,
    updated_at_ns: i64,
}

/// Filesystem-backed store for persisted document collection metadata.
#[derive(Debug, Clone)]
pub struct CollectionMetadataStore {
    base_path: PathBuf,
}

impl CollectionMetadataStore {
    const FORMAT_VERSION: u32 = 1;

    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    fn collection_path(&self, collection: &str) -> PathBuf {
        self.base_path
            .join(format!("{}.json", Self::encode_component(collection)))
    }

    fn encode_component(value: &str) -> String {
        let mut encoded = String::with_capacity(value.len() * 2);
        for byte in value.as_bytes() {
            encoded.push_str(&format!("{:02x}", byte));
        }
        encoded
    }

    fn build_collection(
        &self,
        stored: StoredCollectionMetadata,
        path: &Path,
    ) -> Result<DocumentCollection> {
        let mut config = serde_json::from_str::<DocumentCollectionConfig>(&stored.config_json)
            .with_context(|| {
                format!(
                    "Failed to deserialize collection config for '{}' from {}",
                    stored.name,
                    path.display()
                )
            })?;
        if config.name.is_empty() {
            config.name = stored.name.clone();
        }

        Ok(DocumentCollection {
            name: stored.name,
            indexes: config.indexes.clone(),
            config,
            document_count: stored.document_count,
            storage_size_bytes: stored.storage_size_bytes,
            created_at_ns: stored.created_at_ns,
            updated_at_ns: stored.updated_at_ns,
        })
    }

    pub async fn persist_collection(&self, collection: &DocumentCollection) -> Result<usize> {
        fs::create_dir_all(&self.base_path).await.with_context(|| {
            format!(
                "Failed to create collection metadata directory {}",
                self.base_path.display()
            )
        })?;

        let config_json = serde_json::to_string(&collection.config).with_context(|| {
            format!(
                "Failed to serialize collection config for '{}'",
                collection.name
            )
        })?;
        let payload = serde_json::to_vec(&StoredCollectionMetadata {
            format_version: Self::FORMAT_VERSION,
            name: collection.name.clone(),
            config_json,
            document_count: collection.document_count,
            storage_size_bytes: collection.storage_size_bytes,
            created_at_ns: collection.created_at_ns,
            updated_at_ns: collection.updated_at_ns,
        })
        .with_context(|| {
            format!(
                "Failed to serialize collection metadata for '{}'",
                collection.name
            )
        })?;

        let path = self.collection_path(&collection.name);
        fs::write(&path, &payload).await.with_context(|| {
            format!(
                "Failed to persist collection metadata for '{}' at {}",
                collection.name,
                path.display()
            )
        })?;

        Ok(payload.len())
    }

    pub async fn load_collection(&self, collection: &str) -> Result<Option<DocumentCollection>> {
        let path = self.collection_path(collection);
        match fs::read(&path).await {
            Ok(bytes) => {
                let stored = serde_json::from_slice::<StoredCollectionMetadata>(&bytes)
                    .with_context(|| {
                        format!(
                            "Failed to deserialize collection metadata for '{}' from {}",
                            collection,
                            path.display()
                        )
                    })?;
                Ok(Some(self.build_collection(stored, &path)?))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Failed to read collection metadata for '{}' from {}",
                    collection,
                    path.display()
                )
            }),
        }
    }

    pub async fn load_all_collections(&self) -> Result<Vec<DocumentCollection>> {
        let mut entries = match fs::read_dir(&self.base_path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to enumerate collection metadata directory {}",
                        self.base_path.display()
                    )
                });
            }
        };

        let mut collections = Vec::new();
        while let Some(entry) = entries.next_entry().await.with_context(|| {
            format!(
                "Failed to iterate collection metadata directory {}",
                self.base_path.display()
            )
        })? {
            let path = entry.path();
            let bytes = fs::read(&path).await.with_context(|| {
                format!("Failed to read collection metadata file {}", path.display())
            })?;
            let stored =
                serde_json::from_slice::<StoredCollectionMetadata>(&bytes).with_context(|| {
                    format!(
                        "Failed to deserialize collection metadata from {}",
                        path.display()
                    )
                })?;
            collections.push(self.build_collection(stored, &path)?);
        }

        collections.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(collections)
    }

    pub async fn delete_collection(&self, collection: &str) -> Result<()> {
        let path = self.collection_path(collection);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Failed to delete collection metadata for '{}' at {}",
                    collection,
                    path.display()
                )
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_component_basic() {
        let encoded = CollectionMetadataStore::encode_component("test");
        assert_eq!(encoded, "74657374"); // hex of "test"
    }

    #[test]
    fn test_encode_component_special_chars() {
        let encoded = CollectionMetadataStore::encode_component("my/collection.name");
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('.'));
    }

    #[test]
    fn test_collection_path() {
        let store = CollectionMetadataStore::new("/tmp/test_meta");
        let path = store.collection_path("users");
        assert!(path.to_str().unwrap().ends_with(".json"));
        assert!(path.to_str().unwrap().contains("test_meta"));
    }

    #[test]
    fn test_store_creation() {
        let store = CollectionMetadataStore::new("/tmp/test_store");
        assert_eq!(store.base_path, PathBuf::from("/tmp/test_store"));
    }

    #[tokio::test]
    async fn test_delete_nonexistent_collection() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CollectionMetadataStore::new(temp_dir.path());
        // Deleting a non-existent collection should succeed (idempotent)
        let result = store.delete_collection("nonexistent").await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_stored_metadata_serialization() {
        let metadata = StoredCollectionMetadata {
            format_version: 1,
            name: "test_col".to_string(),
            config_json: "{}".to_string(),
            document_count: 100,
            storage_size_bytes: 4096,
            created_at_ns: 1000000,
            updated_at_ns: 2000000,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: StoredCollectionMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test_col");
        assert_eq!(deserialized.document_count, 100);
        assert_eq!(deserialized.format_version, 1);
    }
}
