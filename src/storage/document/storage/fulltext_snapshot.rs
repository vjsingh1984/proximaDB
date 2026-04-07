use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::storage::document::indexes::fulltext::FullTextIndexSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredFullTextSnapshot {
    format_version: u32,
    snapshot: FullTextIndexSnapshot,
}

/// Filesystem-backed store for persisted document full-text index snapshots.
#[derive(Debug, Clone)]
pub struct FullTextSnapshotStore {
    base_path: PathBuf,
}

impl FullTextSnapshotStore {
    const FORMAT_VERSION: u32 = 1;

    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
        }
    }

    fn snapshot_path(&self, collection: &str) -> PathBuf {
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

    pub async fn persist_snapshot(
        &self,
        collection: &str,
        snapshot: &FullTextIndexSnapshot,
    ) -> Result<usize> {
        fs::create_dir_all(&self.base_path).await.with_context(|| {
            format!(
                "Failed to create full-text snapshot directory {}",
                self.base_path.display()
            )
        })?;

        let payload = serde_json::to_vec(&StoredFullTextSnapshot {
            format_version: Self::FORMAT_VERSION,
            snapshot: snapshot.clone(),
        })
        .with_context(|| {
            format!(
                "Failed to serialize full-text snapshot for collection '{}'",
                collection
            )
        })?;

        let path = self.snapshot_path(collection);
        fs::write(&path, &payload).await.with_context(|| {
            format!(
                "Failed to persist full-text snapshot for collection '{}' at {}",
                collection,
                path.display()
            )
        })?;

        Ok(payload.len())
    }

    pub async fn load_snapshot(&self, collection: &str) -> Result<Option<FullTextIndexSnapshot>> {
        let path = self.snapshot_path(collection);
        match fs::read(&path).await {
            Ok(bytes) => {
                let stored = serde_json::from_slice::<StoredFullTextSnapshot>(&bytes)
                    .with_context(|| {
                        format!(
                            "Failed to deserialize full-text snapshot for collection '{}' from {}",
                            collection,
                            path.display()
                        )
                    })?;
                Ok(Some(stored.snapshot))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Failed to read full-text snapshot for collection '{}' from {}",
                    collection,
                    path.display()
                )
            }),
        }
    }

    pub async fn delete_snapshot(&self, collection: &str) -> Result<()> {
        let path = self.snapshot_path(collection);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Failed to delete full-text snapshot for collection '{}' at {}",
                    collection,
                    path.display()
                )
            }),
        }
    }
}
