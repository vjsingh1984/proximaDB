//! Disk Manager for WAL operations
//!
//! This module centralizes all disk I/O operations, removing them from batch strategies.
//! It handles writing WAL data to disk, reading it back, and managing WAL files.
//!
//! TD-016: Integrated with WALEncryptionLayer for AES-256-GCM encryption at rest.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::storage::encryption::WALEncryptionLayer;
use crate::storage::encryption::wal_encryption::WalSegmentMetadata;
use crate::storage::persistence::filesystem::{
    DirEntry, FileSystem, FilesystemError, FilesystemFactory,
};
use crate::storage::persistence::write_ahead_log::BatchId;
use crate::storage::persistence::write_ahead_log::serialization::SerializationFormat;
use crate::storage::persistence::write_ahead_log::{RecoveryToken, RecoveryTokenProvider};
use proximadb_kernel::checksum::Crc32;

/// Centralized manager for all WAL disk operations
pub struct WriteAheadLogDiskManager {
    /// Filesystem factory for creating filesystem instances
    filesystem_factory: Arc<FilesystemFactory>,
    /// Base URL for WAL files (e.g., file:///path, s3://bucket/prefix)
    wal_base_url: String,
    /// Optional WAL encryption layer (TD-016)
    encryption_layer: Option<Arc<WALEncryptionLayer>>,
    /// Injected ordering-token provider. The default constructors use the
    /// process provider populated by the canonical partition-lease manager.
    recovery_token_provider: Arc<RecoveryTokenProvider>,
    /// Statistics
    stats: Arc<tokio::sync::RwLock<WalDiskManagerStats>>,
}

/// Backwards-compat alias for [`WalDiskManagerStats`].
pub type DiskStats = WalDiskManagerStats;

/// Statistics for disk operations
#[derive(Debug, Clone, Default)]
pub struct WalDiskManagerStats {
    pub total_bytes_written: u64,
    pub total_bytes_read: u64,
    pub total_files_written: u64,
    pub total_files_read: u64,
    pub write_errors: u64,
    pub read_errors: u64,
}

/// WAL file metadata
#[derive(Debug, Clone)]
pub struct WalFileInfo {
    pub collection_id: String,
    pub batch_id: BatchId,
    /// Full URL to the WAL file (scheme-preserving)
    pub file_url: String,
    pub size_bytes: u64,
    pub format: SerializationFormat,
    /// Encryption metadata (TD-016)
    pub encryption_metadata: Option<WalSegmentMetadata>,
    /// Ordered token parsed from the object name. The tenant is populated from
    /// the authenticated envelope when the object is read.
    pub recovery_token: Option<RecoveryToken>,
    /// The durable manifest `global_lsn` allocated for this batch (TD-DELVEC-1
    /// WI-5 P0). Set on the write path; `0` when parsed from a path (reads don't
    /// need it — only the write path returns it for DV-bit keying).
    pub global_lsn: u64,
}

#[derive(Debug, Clone)]
pub struct ReadWalBatch {
    pub data: Vec<u8>,
    pub recovery_token: Option<RecoveryToken>,
    pub record_ordinals: Vec<u32>,
    pub checksum_crc32: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalObjectEnvelope {
    version: u16,
    recovery_token: RecoveryToken,
    payload_checksum_crc32: u32,
    vector_count: u64,
    encryption_metadata: Option<WalSegmentMetadata>,
    record_ordinals: Vec<u32>,
}

impl WriteAheadLogDiskManager {
    const LIST_ATTEMPTS: usize = 3;
    const ENVELOPE_MAGIC: &'static [u8; 8] = b"PXWAL02\0";

    /// Create a new disk manager
    pub fn new(filesystem_factory: Arc<FilesystemFactory>, wal_base_url: impl AsRef<str>) -> Self {
        Self::with_encryption(filesystem_factory, wal_base_url, None)
    }

    /// Create a new disk manager with encryption support (TD-016)
    pub fn with_encryption(
        filesystem_factory: Arc<FilesystemFactory>,
        wal_base_url: impl AsRef<str>,
        encryption_layer: Option<Arc<WALEncryptionLayer>>,
    ) -> Self {
        Self::with_encryption_and_token_provider(
            filesystem_factory,
            wal_base_url,
            encryption_layer,
            RecoveryTokenProvider::global(),
        )
    }

    pub fn with_encryption_and_token_provider(
        filesystem_factory: Arc<FilesystemFactory>,
        wal_base_url: impl AsRef<str>,
        encryption_layer: Option<Arc<WALEncryptionLayer>>,
        recovery_token_provider: Arc<RecoveryTokenProvider>,
    ) -> Self {
        let wal_base_url = wal_base_url.as_ref().to_string();
        let encryption_enabled = encryption_layer.as_ref().is_some_and(|e| e.is_enabled());

        info!(
            "🎯 Creating WriteAheadLogDiskManager with base URL: {}, encryption: {}",
            wal_base_url,
            if encryption_enabled {
                "enabled (AES-256-GCM)"
            } else {
                "disabled"
            }
        );

        Self {
            filesystem_factory,
            wal_base_url,
            encryption_layer,
            recovery_token_provider,
            stats: Arc::new(tokio::sync::RwLock::new(WalDiskManagerStats::default())),
        }
    }

    /// Helper: join URL segments preserving scheme and avoiding duplicate slashes
    fn join_url(base: &str, segments: &[&str], trailing_slash: bool) -> String {
        // Remove trailing slash from base (but preserve scheme authority like file:///)
        let mut url = if base == "file://" {
            // extremely unlikely, but keep as-is
            base.to_string()
        } else {
            // For file:// URIs, usually base already has file:///path or file://./path
            base.trim_end_matches('/').to_string()
        };

        for seg in segments {
            if !seg.is_empty() {
                url.push('/');
                url.push_str(seg.trim_matches('/'));
            }
        }
        if trailing_slash {
            url.push('/');
        }
        url
    }

    /// Get the filesystem factory
    pub fn filesystem_factory(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem_factory
    }

    /// Get the base WAL URL
    pub fn get_base_wal_url(&self) -> &str {
        &self.wal_base_url
    }

    /// Build the WAL URL for a collection: {base}/{collection_id}/wal/
    pub fn collection_wal_url(&self, collection_id: &str) -> String {
        // Use collection_id directly - collection IDs are already short base62 UUIDs
        Self::join_url(&self.wal_base_url, &[collection_id, "wal"], true)
    }

    /// Build WAL batch URL: .../{collection_id}/wal/<batch_id>.<ext>
    pub fn batch_url(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        format: SerializationFormat,
    ) -> String {
        // Use collection_id directly - collection IDs are already short base62 UUIDs
        let ext = match format {
            SerializationFormat::ProtocolBuffers => "pbwal",
            SerializationFormat::Bincode => "bcwal",
            SerializationFormat::Avro => "avwal",
        };
        let fname = format!("{}.{}", batch_id.to_base62(), ext);
        Self::join_url(&self.wal_base_url, &[collection_id, "wal", &fname], false)
    }

    fn tokenized_batch_url(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        format: SerializationFormat,
        token: &RecoveryToken,
    ) -> String {
        let ext = match format {
            SerializationFormat::ProtocolBuffers => "pbwal",
            SerializationFormat::Bincode => "bcwal",
            SerializationFormat::Avro => "avwal",
        };
        let fname = format!(
            "{:020}-{:020}-{}.{}",
            token.epoch,
            token.sequence,
            batch_id.to_base62(),
            ext
        );
        Self::join_url(&self.wal_base_url, &[collection_id, "wal", &fname], false)
    }

    /// Build manifest URL: .../{collection_id}/wal/manifest.log
    pub fn manifest_url(&self, collection_id: &str) -> String {
        // Use collection_id directly - collection IDs are already short base62 UUIDs
        Self::join_url(
            &self.wal_base_url,
            &[collection_id, "wal", "manifest.log"],
            false,
        )
    }

    /// Write a serialized batch to disk. `vector_count` is the number of records
    /// in the batch, recorded on the global manifest entry (used by stats,
    /// checkpoints, and the per-collection drift signal).
    pub async fn write_batch(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        data: &[u8],
        format: SerializationFormat,
        vector_count: u64,
    ) -> Result<WalFileInfo> {
        self.write_batch_with_sync(collection_id, batch_id, data, format, vector_count, false)
            .await
    }

    /// Write a serialized batch to disk with optional sync. `vector_count` is the
    /// number of records in the batch (recorded on the manifest entry); pass the
    /// batch's `vector_records.len()`.
    pub async fn write_batch_with_sync(
        &self,
        collection_id: &str,
        batch_id: &BatchId,
        data: &[u8],
        format: SerializationFormat,
        vector_count: u64,
        sync_to_disk: bool,
    ) -> Result<WalFileInfo> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let recovery_token = self
            .recovery_token_provider
            .allocate(collection_id, now_ms)?;
        let file_url = self.tokenized_batch_url(collection_id, batch_id, format, &recovery_token);

        debug!(
            "📝 Writing WriteBuffer batch {} for collection {} to {} ({} bytes)",
            batch_id.to_base62(),
            collection_id,
            file_url,
            data.len()
        );

        // Ensure directory exists (create if missing). This improves robustness
        // in serverless/cloud cold starts where collection directories may not
        // be pre-created by control-plane paths.
        let dir_url = self.collection_wal_url(collection_id);
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;

        // Always attempt directory creation for uniform semantics (no-op on object stores)
        let _ = filesystem.create_dir_all(&dir_url).await;

        let checksum = Crc32::checksum(data);
        let encryption_metadata = self.encryption_layer.as_ref().map(|layer| {
            layer.prepare_segment_metadata(
                &format!("{}_{}", collection_id, batch_id.to_base62()),
                batch_id.timestamp_ms() ^ u64::from(batch_id.counter()),
                data.len(),
            )
        });
        let envelope = WalObjectEnvelope {
            version: 2,
            recovery_token: recovery_token.clone(),
            payload_checksum_crc32: checksum,
            vector_count,
            encryption_metadata: encryption_metadata.clone(),
            record_ordinals: (0..vector_count)
                .map(u32::try_from)
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("WAL batch has more records than the recovery ordinal can represent")?,
        };
        let header = serde_json::to_vec(&envelope).context("serializing WAL object envelope")?;
        let payload = match (&self.encryption_layer, &encryption_metadata) {
            (Some(layer), Some(metadata)) => layer
                .encrypt_segment_with_metadata_and_aad(metadata, data, &header)
                .context("encrypting WAL payload with authenticated envelope")?,
            _ => data.to_vec(),
        };
        let mut data_to_write = Vec::with_capacity(12 + header.len() + payload.len());
        data_to_write.extend_from_slice(Self::ENVELOPE_MAGIC);
        data_to_write.extend_from_slice(&(header.len() as u32).to_be_bytes());
        data_to_write.extend_from_slice(&header);
        data_to_write.extend_from_slice(&payload);

        // Write data atomically; for object stores this will write to a temp and
        // then rename to the final path.
        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        let strategy = crate::storage::persistence::filesystem::write_strategy::WriteStrategyFactory
            ::create_metadata_strategy(&*filesystem, None)?;
        let file_options = strategy.create_file_options(&*filesystem, &file_url)?;
        filesystem
            .write_if_absent(&file_url, &data_to_write, Some(file_options))
            .await
            .context("Failed to write WriteBuffer batch to disk")?;

        // Sync to disk if requested
        if sync_to_disk {
            filesystem
                .sync_file(&file_url)
                .await
                .context("Failed to sync WriteBuffer batch to disk")?;
            debug!("✅ WriteBuffer batch synced to disk for durability");
        }

        // Register in global manifest
        let file_name = file_url.split('/').next_back().unwrap_or("").to_string();

        use crate::storage::persistence::write_ahead_log::manifest;
        // TD-DELVEC-1 WI-5 P0: capture the durable `global_lsn` allocated for this
        // batch (synchronous inside `append_async`) so the write path can return it
        // for deletion-vector bit keying. Stays `0` if there is no manifest service
        // or the (non-fatal) append failed — callers fall back to the memtable seq.
        let mut global_lsn = 0u64;
        if let Some(manifest_service) = manifest::get_service() {
            let format_str = match format {
                SerializationFormat::ProtocolBuffers => "proto",
                SerializationFormat::Bincode => "bincode",
                SerializationFormat::Avro => "avro",
            };
            let entry = manifest::GlobalManifestEntry::new(
                0,                          // LSN will be auto-allocated
                collection_id.to_string(),  // collection_id
                batch_id,                   // batch_id (pass reference)
                file_name,                  // file_name
                data_to_write.len() as u64, // size_bytes (encrypted size)
                checksum,                   // checksum_crc32
                SerializationFormat::parse_format(format_str)
                    .unwrap_or(SerializationFormat::Bincode), // format enum
                vector_count,               // records in this batch (drift / stats / checkpoints)
                self.wal_base_url.clone(),  // storage_url
            );
            // Async append (non-blocking, high performance); non-fatal if manifest
            // channel is closed (e.g. singleton background worker from a prior run).
            match manifest_service.append_async(entry).await {
                Ok(lsn) => global_lsn = lsn,
                Err(e) => warn!("⚠️  Manifest append failed (non-fatal): {}", e),
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_written += data_to_write.len() as u64;
            stats.total_files_written += 1;
        }

        let file_info = WalFileInfo {
            collection_id: collection_id.to_string(),
            batch_id: *batch_id,
            file_url: file_url.clone(),
            size_bytes: data_to_write.len() as u64,
            format,
            encryption_metadata,
            recovery_token: Some(recovery_token),
            global_lsn,
        };

        debug!("✅ Successfully wrote WAL batch to disk: {:?}", file_info);
        Ok(file_info)
    }

    /// Read a serialized batch from disk
    pub async fn read_batch(&self, file_info: &WalFileInfo) -> Result<Vec<u8>> {
        Ok(self.read_batch_with_envelope(file_info).await?.data)
    }

    pub async fn read_batch_with_envelope(&self, file_info: &WalFileInfo) -> Result<ReadWalBatch> {
        let file_url = file_info.file_url.clone();

        debug!(
            "📖 Reading WAL batch {} for collection {} from {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id,
            file_url
        );

        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        let encrypted_data = filesystem
            .read(&file_url)
            .await
            .context("Failed to read WAL batch from disk")?;

        // Track the object length before parsing/decrypting.
        let encrypted_len = encrypted_data.len();

        if encrypted_data.starts_with(Self::ENVELOPE_MAGIC) {
            if encrypted_data.len() < 12 {
                anyhow::bail!("truncated WAL envelope at {file_url}");
            }
            let header_len = u32::from_be_bytes(encrypted_data[8..12].try_into()?) as usize;
            let header_end = 12usize
                .checked_add(header_len)
                .context("WAL envelope header length overflow")?;
            if header_end > encrypted_data.len() {
                anyhow::bail!("truncated WAL envelope header at {file_url}");
            }
            let header = &encrypted_data[12..header_end];
            let envelope: WalObjectEnvelope =
                serde_json::from_slice(header).context("decoding WAL object envelope")?;
            if envelope.version != 2 {
                anyhow::bail!("unsupported WAL envelope version {}", envelope.version);
            }
            if let Some(name_token) = &file_info.recovery_token
                && (name_token.epoch != envelope.recovery_token.epoch
                    || name_token.sequence != envelope.recovery_token.sequence)
            {
                anyhow::bail!("WAL filename token does not match authenticated envelope");
            }
            if envelope.record_ordinals.len() != envelope.vector_count as usize
                || envelope
                    .record_ordinals
                    .iter()
                    .enumerate()
                    .any(|(index, ordinal)| *ordinal as usize != index)
            {
                anyhow::bail!("invalid WAL record ordinal sequence");
            }
            let payload = &encrypted_data[header_end..];
            let data = match (&self.encryption_layer, &envelope.encryption_metadata) {
                (Some(layer), Some(metadata)) if metadata.encrypted => layer
                    .decrypt_segment_with_aad(metadata, payload, header)
                    .context("decrypting authenticated WAL envelope")?,
                (None, Some(metadata)) if metadata.encrypted => {
                    anyhow::bail!("encrypted WAL object has no configured encryption layer")
                }
                _ => payload.to_vec(),
            };
            if Crc32::checksum(&data) != envelope.payload_checksum_crc32 {
                anyhow::bail!("WAL payload checksum mismatch at {file_url}");
            }
            let mut stats = self.stats.write().await;
            stats.total_bytes_read += encrypted_len as u64;
            stats.total_files_read += 1;
            drop(stats);
            return Ok(ReadWalBatch {
                data,
                recovery_token: Some(envelope.recovery_token),
                record_ordinals: envelope.record_ordinals,
                checksum_crc32: envelope.payload_checksum_crc32,
            });
        }

        // TD-016: Decrypt data after reading if it was encrypted
        let data = if let Some(ref metadata) = file_info.encryption_metadata {
            if metadata.encrypted {
                if let Some(ref encryption_layer) = self.encryption_layer {
                    debug!("🔓 Decrypting WAL segment after read");
                    match encryption_layer.decrypt_segment(metadata, &encrypted_data) {
                        Ok(decrypted) => {
                            debug!(
                                "✅ WAL segment decrypted ({} -> {} bytes)",
                                encrypted_data.len(),
                                decrypted.len()
                            );
                            decrypted
                        }
                        Err(e) => {
                            warn!("⚠️  WAL decryption failed: {}", e);
                            return Err(e.context("Failed to decrypt WAL segment"));
                        }
                    }
                } else {
                    warn!("⚠️  WAL segment is encrypted but no encryption layer available");
                    return Err(anyhow::anyhow!(
                        "Cannot decrypt WAL segment: no encryption layer available"
                    ));
                }
            } else {
                encrypted_data
            }
        } else {
            encrypted_data
        };

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_bytes_read += encrypted_len as u64;
            stats.total_files_read += 1;
        }

        debug!("✅ Successfully read {} bytes from disk", data.len());
        Ok(ReadWalBatch {
            checksum_crc32: Crc32::checksum(&data),
            data,
            recovery_token: None,
            record_ordinals: Vec::new(),
        })
    }

    /// List all WAL files for a collection
    pub async fn list_collection_files(&self, collection_id: &str) -> Result<Vec<WalFileInfo>> {
        let dir_url = self.collection_wal_url(collection_id);

        debug!(
            "📂 Listing WriteBuffer files for collection {} in {}",
            collection_id, dir_url
        );

        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;
        let entries = Self::list_prefix_entries(&*filesystem, &dir_url).await?;
        let mut wal_files = Vec::new();

        debug!("Raw entries from filesystem: {} entries", entries.len());
        for entry in entries {
            debug!(
                "  Entry: path={}, is_dir={}",
                entry.url, entry.metadata.is_directory
            );
            if let Some(file_info) = self.parse_wal_filename(&entry.url, collection_id) {
                wal_files.push(file_info);
            } else {
                debug!("  Failed to parse WAL filename: {}", entry.url);
            }
        }

        // Backward-compat: if none found, fall back to legacy write_buffer path
        if wal_files.is_empty() {
            // Backward-compat path: {base}/{collection_id}/write_buffer/
            let legacy_url =
                Self::join_url(&self.wal_base_url, &[collection_id, "write_buffer"], true);
            let legacy_fs = self.filesystem_factory.get_filesystem(&legacy_url)?;
            for entry in Self::list_prefix_entries(&*legacy_fs, &legacy_url).await? {
                if let Some(file_info) = self.parse_wal_filename(&entry.url, collection_id) {
                    wal_files.push(file_info);
                }
            }
        }

        debug!(
            "Found {} WAL files for collection {}",
            wal_files.len(),
            collection_id
        );
        Ok(wal_files)
    }

    /// LIST is authoritative for a prefix. Object stores return an empty page for an
    /// absent prefix; the local backend reports a missing directory as NotFound.
    /// Normalize only those absent forms to empty. Every other failure is retried and
    /// then returned so recovery cannot silently reinterpret an auth/network/throttle
    /// failure as data absence (ADR-063 D3-D5 / TD-OBJSTORE-4 S1).
    pub(crate) async fn list_prefix_entries(
        filesystem: &dyn FileSystem,
        prefix: &str,
    ) -> Result<Vec<DirEntry>> {
        for attempt in 1..=Self::LIST_ATTEMPTS {
            match filesystem.list(prefix).await {
                Ok(entries) => return Ok(entries),
                Err(error) if Self::is_absent_list_error(&error) => return Ok(Vec::new()),
                Err(error) if attempt < Self::LIST_ATTEMPTS => {
                    warn!(
                        "WAL prefix LIST failed for {} (attempt {}/{}): {}",
                        prefix,
                        attempt,
                        Self::LIST_ATTEMPTS,
                        error
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50 * attempt as u64)).await;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to LIST WAL prefix {prefix}"));
                }
            }
        }

        unreachable!("LIST retry loop always returns")
    }

    fn is_absent_list_error(error: &FilesystemError) -> bool {
        match error {
            FilesystemError::NotFound(_) => true,
            FilesystemError::Io(error) => error.kind() == std::io::ErrorKind::NotFound,
            _ => false,
        }
    }

    /// Delete a WAL file
    pub async fn delete_file(&self, file_info: &WalFileInfo) -> Result<()> {
        let file_url = file_info.file_url.clone();

        debug!(
            "🗑️ Deleting WAL file {} for collection {}",
            file_info.batch_id.to_base62(),
            file_info.collection_id
        );

        let filesystem = self.filesystem_factory.get_filesystem(&file_url)?;
        filesystem
            .delete(&file_url)
            .await
            .context("Failed to delete WAL file")?;

        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }

    /// Delete a WAL file by path (used by recovery manager)
    pub async fn delete_wal_file_url(&self, file_url: &str) -> Result<()> {
        debug!("🗑️ Deleting WAL file at URL: {}", file_url);

        let filesystem = self.filesystem_factory.get_filesystem(file_url)?;
        filesystem
            .delete(file_url)
            .await
            .context("Failed to delete WAL file")?;

        debug!("✅ Successfully deleted WAL file");
        Ok(())
    }

    /// Delete all WAL files for a collection
    pub async fn delete_collection_files(&self, collection_id: &str) -> Result<u64> {
        let files = self.list_collection_files(collection_id).await?;
        let count = files.len() as u64;

        for file_info in files {
            if let Err(e) = self.delete_file(&file_info).await {
                warn!("Failed to delete WAL file {}: {}", file_info.file_url, e);
                continue;
            }
        }

        // Try to remove the collection WAL directory
        let dir_url = self.collection_wal_url(collection_id);
        let filesystem = self.filesystem_factory.get_filesystem(&dir_url)?;

        if let Err(e) = filesystem.delete(&dir_url).await {
            debug!(
                "Failed to delete collection directory (may have subdirs): {}",
                e
            );
        }

        Ok(count)
    }

    /// Get statistics
    pub async fn get_stats(&self) -> Result<WalDiskManagerStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }

    /// Get the file path for a batch
    // get_batch_file_path removed in favor of URL builders
    /// Parse a WAL filename to extract metadata
    fn parse_wal_filename(&self, path: &str, collection_id: &str) -> Option<WalFileInfo> {
        // Use last path segment as filename regardless of scheme
        let file_name = path.split('/').next_back()?;

        // Expected format: <batch_id>.<format>
        let parts: Vec<&str> = file_name.split('.').collect();
        if parts.len() != 2 {
            return None;
        }

        let batch_id_str = parts[0];
        let extension = parts[1];

        // Parse format from extension
        let format = match extension {
            "pbwal" => SerializationFormat::ProtocolBuffers,
            "bcwal" => SerializationFormat::Bincode,
            "avwal" => SerializationFormat::Avro,
            // Alternative extensions for backward compatibility
            "proto" => SerializationFormat::ProtocolBuffers,
            "bincode" => SerializationFormat::Bincode,
            "avro" => SerializationFormat::Avro,
            _ => return None,
        };

        // New names are epoch-first; legacy names contain only the batch id.
        let (batch_id_str, recovery_token) = match batch_id_str.split_once('-') {
            Some((epoch, rest)) => {
                let (sequence, encoded_batch_id) = rest.split_once('-')?;
                (
                    encoded_batch_id,
                    Some(RecoveryToken {
                        tenant_id: String::new(),
                        epoch: epoch.parse().ok()?,
                        sequence: sequence.parse().ok()?,
                    }),
                )
            }
            None => (batch_id_str, None),
        };
        let batch_id = BatchId::from_base62(batch_id_str)?;

        Some(WalFileInfo {
            collection_id: collection_id.to_string(),
            batch_id,
            file_url: path.to_string(),
            size_bytes: 0, // Will be filled by caller if needed
            format,
            encryption_metadata: None, // Path parsing doesn't have encryption metadata
            recovery_token,
            global_lsn: 0, // Not recoverable from the path; reads don't need it.
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemConfig;
    use tempfile::TempDir;

    async fn create_test_manager() -> (WriteAheadLogDiskManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        let manager =
            WriteAheadLogDiskManager::new(filesystem_factory, temp_dir.path().to_str().unwrap());

        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_disk_manager_write_read() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "test_collection";
        let batch_id = BatchId::new();
        let data = b"test data";
        let format = SerializationFormat::ProtocolBuffers;

        // Create WAL directory for collection (simulating collection creation)
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        std::fs::create_dir_all(&write_buffer_dir).expect("Failed to create WriteBuffer directory");

        // Write batch
        let file_info = manager
            .write_batch(collection_id, &batch_id, data, format, 1)
            .await
            .expect("Failed to write batch");
        assert_eq!(file_info.collection_id, collection_id);
        assert!(file_info.size_bytes > data.len() as u64);
        let token = file_info
            .recovery_token
            .as_ref()
            .expect("new WAL objects carry recovery tokens");
        let file_name = file_info.file_url.split('/').next_back().unwrap();
        assert!(file_name.starts_with(&format!("{:020}-{:020}-", token.epoch, token.sequence)));

        // Read batch
        let read_data = manager
            .read_batch(&file_info)
            .await
            .expect("Failed to read batch");
        assert_eq!(read_data, data);

        // Check stats
        let stats = manager.get_stats().await.expect("Failed to get stats");
        assert_eq!(stats.total_bytes_written, file_info.size_bytes);
        assert_eq!(stats.total_bytes_read, file_info.size_bytes);
        assert_eq!(stats.total_files_written, 1);
        assert_eq!(stats.total_files_read, 1);
    }

    #[tokio::test]
    async fn test_disk_manager_list_delete() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "test_collection";

        // Create WAL directory for collection (simulating collection creation)
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        std::fs::create_dir_all(&write_buffer_dir).expect("Failed to create WriteBuffer directory");

        // Write multiple batches
        for i in 0..3 {
            let batch_id = BatchId::new();
            let data = format!("test data {}", i).into_bytes();
            manager
                .write_batch(
                    collection_id,
                    &batch_id,
                    &data,
                    SerializationFormat::Bincode,
                    1,
                )
                .await
                .expect("Failed to write batch");
        }

        // List files
        let files = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(files.len(), 3);

        // Delete one file
        manager
            .delete_file(&files[0])
            .await
            .expect("Failed to delete file");

        // List again
        let remaining = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(remaining.len(), 2);

        // Delete all
        let deleted = manager
            .delete_collection_files(collection_id)
            .await
            .expect("Failed to delete collection files");
        assert_eq!(deleted, 2);

        // Verify empty
        let final_list = manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(final_list.len(), 0);
    }

    #[tokio::test]
    async fn list_missing_prefix_is_empty_and_legacy_prefix_is_discoverable() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "legacy_collection";

        assert!(
            manager
                .list_collection_files(collection_id)
                .await
                .expect("missing prefixes are empty")
                .is_empty()
        );

        let batch_id = BatchId::new();
        let legacy_dir = temp_dir.path().join(collection_id).join("write_buffer");
        std::fs::create_dir_all(&legacy_dir).expect("legacy directory");
        std::fs::write(
            legacy_dir.join(format!("{}.bcwal", batch_id.to_base62())),
            b"legacy WAL",
        )
        .expect("legacy WAL object");

        let files = manager
            .list_collection_files(collection_id)
            .await
            .expect("legacy LIST fallback");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].batch_id, batch_id);
        assert!(files[0].file_url.contains("/write_buffer/"));
    }

    #[tokio::test]
    async fn list_error_fails_closed_instead_of_looking_empty() {
        let (manager, temp_dir) = create_test_manager().await;
        let collection_id = "broken_collection";
        let collection_dir = temp_dir.path().join(collection_id);
        std::fs::create_dir_all(&collection_dir).expect("collection directory");
        std::fs::write(collection_dir.join("wal"), b"not a directory").expect("blocking file");

        let error = manager
            .list_collection_files(collection_id)
            .await
            .expect_err("a LIST failure must not be treated as an empty prefix");
        assert!(
            error.to_string().contains("Failed to LIST WAL prefix"),
            "unexpected error: {error:#}"
        );
    }
}
