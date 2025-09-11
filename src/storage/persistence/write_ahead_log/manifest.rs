//! WAL Manifest support: per-collection manifest with LSN + checksums

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{BatchId, disk_manager::WriteBufferDiskManager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalManifestEntry {
    pub lsn: u64,
    pub batch_id: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub checksum_crc32: u32,
    pub timestamp_ms: u64,
}

impl WalManifestEntry {
    pub fn from_batch(batch_id: &BatchId, file_name: String, size_bytes: u64, checksum_crc32: u32) -> Self {
        let ts = batch_id.timestamp_ms();
        let ctr = batch_id.counter() as u64;
        let lsn = (ts << 16) | ctr;
        Self { lsn, batch_id: batch_id.to_base62(), file_name, size_bytes, checksum_crc32, timestamp_ms: ts }
    }
}

pub struct WalManifest {
    disk: Arc<WriteBufferDiskManager>,
}

impl WalManifest {
    pub fn new(disk: Arc<WriteBufferDiskManager>) -> Self { Self { disk } }

    pub async fn append_entry(&self, collection_id: &str, entry: &WalManifestEntry) -> Result<()> {
        let url = self.disk.manifest_url(collection_id);
        let fs = self.disk.filesystem_factory().get_filesystem(&url)?;

        // Read current content (if any), append new JSON line, write atomically
        let mut content = if fs.exists(&url).await? { fs.read(&url).await? } else { Vec::new() };
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        content.extend_from_slice(&line);

        // Use write strategy for atomic replace
        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*fs, None)?;
        let opts = strategy.create_file_options(&*fs, &url)?;
        fs.write(&url, &content, Some(opts)).await?;
        // Best-effort sync
        let _ = fs.sync_file(&url).await;
        Ok(())
    }

    pub async fn read_entries(&self, collection_id: &str) -> Result<Vec<WalManifestEntry>> {
        let url = self.disk.manifest_url(collection_id);
        let fs = self.disk.filesystem_factory().get_filesystem(&url)?;
        if !fs.exists(&url).await? { return Ok(Vec::new()); }
        let data = fs.read(&url).await?;
        let mut out = Vec::new();
        for line in data.split(|b| *b == b'\n') {
            if line.is_empty() { continue; }
            if let Ok(e) = serde_json::from_slice::<WalManifestEntry>(line) {
                out.push(e);
            }
        }
        out.sort_by_key(|e| e.lsn);
        Ok(out)
    }

    /// Rewrite manifest with provided entries (atomic replace)
    pub async fn rewrite_entries(
        &self,
        collection_id: &str,
        entries: &[WalManifestEntry],
    ) -> Result<()> {
        let url = self.disk.manifest_url(collection_id);
        let fs = self.disk.filesystem_factory().get_filesystem(&url)?;
        let mut buf = Vec::new();
        for e in entries {
            let mut line = serde_json::to_vec(e)?;
            line.push(b'\n');
            buf.extend_from_slice(&line);
        }
        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*fs, None)?;
        let opts = strategy.create_file_options(&*fs, &url)?;
        fs.write(&url, &buf, Some(opts)).await?;
        let _ = fs.sync_file(&url).await;
        Ok(())
    }

    /// Remove manifest entries by batch ids
    pub async fn remove_by_batch_ids(
        &self,
        collection_id: &str,
        batch_ids: &std::collections::HashSet<String>,
    ) -> Result<()> {
        let mut entries = self.read_entries(collection_id).await?;
        entries.retain(|e| !batch_ids.contains(&e.batch_id));
        self.rewrite_entries(collection_id, &entries).await
    }
}
