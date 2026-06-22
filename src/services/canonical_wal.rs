//! File-backed canonical WAL appenders for service-layer table writes.
//!
//! This module provides a production-oriented `TableWalAppender`
//! implementation for the canonical DML path. It writes shared
//! `CanonicalWalEntry` records instead of vector-specific operation envelopes,
//! matching the Layer 0 spine described by ADR-010.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use crc32fast::Hasher as Crc32Hasher;
use proximadb_storage_common::{CanonicalOperation, CanonicalWalEntry};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::services::record_store::TableWalAppender;

const FRAME_MAGIC: &[u8; 8] = b"PXWAL001";
const FRAME_VERSION: u16 = 1;
const CODEC_MSGPACK_NAMED: u8 = 1;
const FRAME_HEADER_LEN: usize = 8 + 2 + 1 + 1 + 8 + 4 + 4;

/// Framed binary canonical WAL appender.
///
/// Frame layout:
/// `[magic:8][version:u16][codec:u8][flags:u8][sequence:u64][payload_len:u32][payload_crc32:u32][payload]`.
///
/// Payloads currently use MessagePack named-field encoding for compact,
/// language-readable, schema-tolerant storage. The frame header keeps codec and
/// version explicit so later protobuf/Avro encodings can be introduced without
/// changing the `TableWalAppender` contract.
pub struct FramedTableWalAppender {
    path: PathBuf,
    next_sequence: AtomicU64,
    append_lock: Mutex<()>,
}

impl FramedTableWalAppender {
    /// Open or create a framed binary canonical WAL file.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("creating canonical WAL directory {}", parent.display())
            })?;
        }

        let recovery = recover_wal_state(&path).await?;
        if recovery.valid_len < recovery.file_len {
            truncate_wal(&path, recovery.valid_len).await?;
        }

        Ok(Self {
            path,
            next_sequence: AtomicU64::new(recovery.last_sequence),
            append_lock: Mutex::new(()),
        })
    }

    /// Path backing this WAL appender.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read all complete entries currently present in the WAL.
    pub async fn read_entries(&self) -> Result<Vec<CanonicalWalEntry>> {
        read_entries_from_path(&self.path).await
    }

    /// Read all complete entries from a framed WAL path.
    pub async fn read_entries_from_path(path: impl AsRef<Path>) -> Result<Vec<CanonicalWalEntry>> {
        read_entries_from_path(path.as_ref()).await
    }
}

#[async_trait]
impl TableWalAppender for FramedTableWalAppender {
    async fn read_all_entries(&self) -> Result<Vec<CanonicalWalEntry>> {
        self.read_entries().await
    }

    async fn append_operations(
        &self,
        operations: Vec<CanonicalOperation>,
        tenant_id: Option<String>,
    ) -> Result<Vec<CanonicalWalEntry>> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }

        let _guard = self.append_lock.lock().await;
        let base_sequence = self.next_sequence.load(Ordering::SeqCst);

        let entries: Vec<CanonicalWalEntry> = operations
            .into_iter()
            .enumerate()
            .map(|(offset, operation)| {
                CanonicalWalEntry::new(
                    base_sequence + offset as u64 + 1,
                    operation,
                    tenant_id.clone(),
                )
            })
            .collect();

        let mut frames = Vec::new();
        for entry in &entries {
            encode_frame(entry, &mut frames).context("encoding canonical WAL frame")?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("opening canonical WAL {}", self.path.display()))?;

        file.write_all(&frames)
            .await
            .with_context(|| format!("writing canonical WAL {}", self.path.display()))?;
        file.flush()
            .await
            .with_context(|| format!("flushing canonical WAL {}", self.path.display()))?;
        file.sync_data()
            .await
            .with_context(|| format!("syncing canonical WAL {}", self.path.display()))?;

        self.next_sequence
            .store(base_sequence + entries.len() as u64, Ordering::SeqCst);

        Ok(entries)
    }
}

/// In-memory `TableWalAppender` used when no on-disk canonical WAL is
/// configured. Operations are stored in a `Mutex<Vec<CanonicalWalEntry>>`
/// keyed by a single monotonic counter so consumers that need to replay or
/// inspect appended entries can do so.
///
/// Intended for test paths and the `opt_config = None` boot path where the
/// shared canonical WAL is not opened. Not durable — entries are lost when
/// the process exits.
pub struct MemoryTableWalAppender {
    next_sequence: AtomicU64,
    entries: Mutex<Vec<CanonicalWalEntry>>,
}

impl MemoryTableWalAppender {
    /// Build an empty in-memory appender.
    pub fn new() -> Self {
        Self {
            next_sequence: AtomicU64::new(0),
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot the currently appended entries. Cheap clone; intended for
    /// tests + the rank-profile store's in-memory recovery path.
    pub async fn entries(&self) -> Vec<CanonicalWalEntry> {
        self.entries.lock().await.clone()
    }
}

impl Default for MemoryTableWalAppender {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TableWalAppender for MemoryTableWalAppender {
    async fn append_operations(
        &self,
        operations: Vec<CanonicalOperation>,
        tenant_id: Option<String>,
    ) -> Result<Vec<CanonicalWalEntry>> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }
        let base = self.next_sequence.load(Ordering::SeqCst);
        let entries: Vec<CanonicalWalEntry> = operations
            .into_iter()
            .enumerate()
            .map(|(offset, op)| {
                CanonicalWalEntry::new(base + offset as u64 + 1, op, tenant_id.clone())
            })
            .collect();
        self.next_sequence
            .store(base + entries.len() as u64, Ordering::SeqCst);
        self.entries.lock().await.extend(entries.clone());
        Ok(entries)
    }
}

#[derive(Debug)]
struct WalRecoveryState {
    last_sequence: u64,
    valid_len: u64,
    file_len: u64,
}

async fn recover_wal_state(path: &Path) -> Result<WalRecoveryState> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WalRecoveryState {
                last_sequence: 0,
                valid_len: 0,
                file_len: 0,
            });
        }
        Err(err) => {
            return Err(err).with_context(|| format!("opening canonical WAL {}", path.display()));
        }
    };

    let scan = scan_wal_bytes(&bytes, path)?;
    let last_sequence = scan
        .entries
        .iter()
        .map(|entry| entry.sequence_number)
        .max()
        .unwrap_or(0);

    Ok(WalRecoveryState {
        last_sequence,
        valid_len: scan.valid_len as u64,
        file_len: bytes.len() as u64,
    })
}

async fn read_entries_from_path(path: &Path) -> Result<Vec<CanonicalWalEntry>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("opening canonical WAL {}", path.display()));
        }
    };
    Ok(scan_wal_bytes(&bytes, path)?.entries)
}

async fn truncate_wal(path: &Path, valid_len: u64) -> Result<()> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("opening canonical WAL {} for truncation", path.display()))?;
    file.set_len(valid_len)
        .await
        .with_context(|| format!("truncating canonical WAL {}", path.display()))
}

#[derive(Debug)]
struct WalScan {
    entries: Vec<CanonicalWalEntry>,
    valid_len: usize,
}

fn scan_wal_bytes(bytes: &[u8], path: &Path) -> Result<WalScan> {
    let mut entries = Vec::new();
    let mut offset = 0;

    while offset < bytes.len() {
        match decode_frame(&bytes[offset..]).with_context(|| {
            format!(
                "decoding canonical WAL frame at byte {} in {}",
                offset,
                path.display()
            )
        })? {
            Some((entry, consumed)) => {
                entries.push(entry);
                offset += consumed;
            }
            None => break,
        }
    }

    Ok(WalScan {
        entries,
        valid_len: offset,
    })
}

fn encode_frame(entry: &CanonicalWalEntry, out: &mut Vec<u8>) -> Result<()> {
    let payload = rmp_serde::to_vec_named(entry).context("serializing canonical WAL payload")?;
    let payload_len: u32 = payload
        .len()
        .try_into()
        .context("canonical WAL payload exceeds u32 frame size")?;
    let payload_crc = checksum(&payload);

    out.extend_from_slice(FRAME_MAGIC);
    out.extend_from_slice(&FRAME_VERSION.to_le_bytes());
    out.push(CODEC_MSGPACK_NAMED);
    out.push(0);
    out.extend_from_slice(&entry.sequence_number.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&payload_crc.to_le_bytes());
    out.extend_from_slice(&payload);

    Ok(())
}

/// Atomically rewrite the canonical WAL at `path` so it contains exactly
/// `entries`, **preserving their original `sequence_number`s**. Writes a temp
/// file, fsyncs it, atomically renames it over `path`, then best-effort fsyncs
/// the parent directory so the rename is durable.
///
/// Used by the system catalog to compact its WAL after a durable snapshot: the
/// snapshot covers every mutation up to its watermark LSN, so the WAL is
/// rewritten to keep only the entries after it. Because the sequence numbers are
/// preserved verbatim (not reassigned), the live appender's monotonic counter
/// and the snapshot watermark stay consistent across the rewrite.
pub async fn rewrite_canonical_wal(path: &Path, entries: &[CanonicalWalEntry]) -> Result<()> {
    let mut frames = Vec::new();
    for entry in entries {
        encode_frame(entry, &mut frames)?;
    }
    let tmp = path.with_extension("wal-compact-tmp");
    {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .await
            .with_context(|| format!("opening compaction temp {}", tmp.display()))?;
        file.write_all(&frames)
            .await
            .with_context(|| format!("writing compaction temp {}", tmp.display()))?;
        file.flush().await?;
        file.sync_data()
            .await
            .with_context(|| format!("fsync compaction temp {}", tmp.display()))?;
    }
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("atomically replacing canonical WAL {}", path.display()))?;
    if let Some(dir) = path.parent()
        && let Ok(handle) = std::fs::File::open(dir)
    {
        let _ = handle.sync_all();
    }
    Ok(())
}

fn decode_frame(bytes: &[u8]) -> Result<Option<(CanonicalWalEntry, usize)>> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Ok(None);
    }

    let mut offset = 0;

    let magic = &bytes[offset..offset + FRAME_MAGIC.len()];
    offset += FRAME_MAGIC.len();
    if magic != FRAME_MAGIC {
        bail!("invalid canonical WAL frame magic");
    }

    let version = read_u16(bytes, &mut offset)?;
    if version != FRAME_VERSION {
        bail!("unsupported canonical WAL frame version {version}");
    }

    let codec = bytes[offset];
    offset += 1;
    if codec != CODEC_MSGPACK_NAMED {
        bail!("unsupported canonical WAL codec {codec}");
    }

    let _flags = bytes[offset];
    offset += 1;

    let header_sequence = read_u64(bytes, &mut offset)?;
    let payload_len = read_u32(bytes, &mut offset)? as usize;
    let expected_crc = read_u32(bytes, &mut offset)?;

    let frame_len = FRAME_HEADER_LEN
        .checked_add(payload_len)
        .ok_or_else(|| anyhow!("canonical WAL frame length overflow"))?;
    if bytes.len() < frame_len {
        return Ok(None);
    }

    let payload = &bytes[offset..frame_len];
    let actual_crc = checksum(payload);
    if actual_crc != expected_crc {
        bail!("canonical WAL payload checksum mismatch");
    }

    let entry: CanonicalWalEntry =
        rmp_serde::from_slice(payload).context("deserializing canonical WAL payload")?;
    if entry.sequence_number != header_sequence {
        bail!(
            "canonical WAL header sequence {} does not match payload sequence {}",
            header_sequence,
            entry.sequence_number
        );
    }

    Ok(Some((entry, frame_len)))
}

fn checksum(payload: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(payload);
    hasher.finalize()
}

// Each `read_uN` helper takes an `&[u8]` and an offset, slices N bytes,
// and converts to a fixed-size array via `try_into`. The `.get(...)?`
// returns `None` when the slice would overrun, so by the time we hit
// `try_into`, the source slice is exactly N bytes long and the conversion
// is infallible. The `expect` is a documented "slice length matches array
// size" invariant — keep it inline so future readers see why panic is
// unreachable.
#[allow(clippy::expect_used)]
fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16> {
    let end = *offset + 2;
    let value = bytes
        .get(*offset..end)
        .ok_or_else(|| anyhow!("canonical WAL frame missing u16"))?;
    *offset = end;
    Ok(u16::from_le_bytes(
        value.try_into().expect("u16 slice length"),
    ))
}

#[allow(clippy::expect_used)]
fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let end = *offset + 4;
    let value = bytes
        .get(*offset..end)
        .ok_or_else(|| anyhow!("canonical WAL frame missing u32"))?;
    *offset = end;
    Ok(u32::from_le_bytes(
        value.try_into().expect("u32 slice length"),
    ))
}

#[allow(clippy::expect_used)]
fn read_u64(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let end = *offset + 8;
    let value = bytes
        .get(*offset..end)
        .ok_or_else(|| anyhow!("canonical WAL frame missing u64"))?;
    *offset = end;
    Ok(u64::from_le_bytes(
        value.try_into().expect("u64 slice length"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;
    use proximadb_storage_common::ProjectionDirective;

    fn upsert_operation(collection_id: &str, oid: &str) -> CanonicalOperation {
        CanonicalOperation::RecordUpsert {
            collection_id: collection_id.to_string(),
            record: Box::new(ProximaRecord {
                oid: oid.to_string(),
                ..ProximaRecord::default()
            }),
            projections: vec![ProjectionDirective::ColumnarVariation {
                collection_id: collection_id.to_string(),
                fields: vec!["id".to_string()],
            }],
        }
    }

    #[tokio::test]
    async fn framed_table_wal_appender_persists_and_recovers_sequence() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("tables").join("catalog.wal");

        let appender = FramedTableWalAppender::open(&wal_path).await?;
        let first_entries = appender
            .append_operations(
                vec![
                    upsert_operation("orders", "order-1"),
                    upsert_operation("orders", "order-2"),
                ],
                Some("tenant-a".to_string()),
            )
            .await?;

        assert_eq!(
            first_entries
                .iter()
                .map(|entry| entry.sequence_number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let reopened = FramedTableWalAppender::open(&wal_path).await?;
        let next_entries = reopened
            .append_operations(vec![upsert_operation("orders", "order-3")], None)
            .await?;
        assert_eq!(next_entries[0].sequence_number, 3);

        let entries = reopened.read_entries().await?;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(entries[2].sequence_number, 3);

        let entries_from_path = FramedTableWalAppender::read_entries_from_path(&wal_path).await?;
        assert_eq!(entries_from_path.len(), 3);

        let bytes = tokio::fs::read(&wal_path).await?;
        assert!(String::from_utf8(bytes.clone()).is_err());

        let mut offset = 0;
        let mut decoded_entries = Vec::new();
        while let Some((entry, consumed)) = decode_frame(&bytes[offset..])? {
            decoded_entries.push(entry);
            offset += consumed;
        }
        assert_eq!(decoded_entries.len(), 3);

        Ok(())
    }

    #[tokio::test]
    async fn framed_table_wal_appender_truncates_trailing_partial_frame() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("partial.wal");

        let appender = FramedTableWalAppender::open(&wal_path).await?;
        appender
            .append_operations(vec![upsert_operation("orders", "order-1")], None)
            .await?;

        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_path)
            .await?;
        file.write_all(&FRAME_MAGIC[..4]).await?;
        file.flush().await?;

        let reopened = FramedTableWalAppender::open(&wal_path).await?;
        let next_entries = reopened
            .append_operations(vec![upsert_operation("orders", "order-2")], None)
            .await?;
        assert_eq!(next_entries[0].sequence_number, 2);

        let final_reopen = FramedTableWalAppender::open(&wal_path).await?;
        let entries = final_reopen.read_entries().await?;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.sequence_number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        Ok(())
    }

    #[tokio::test]
    async fn framed_table_wal_appender_rejects_corrupt_complete_frame() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let wal_path = dir.path().join("corrupt.wal");

        let appender = FramedTableWalAppender::open(&wal_path).await?;
        appender
            .append_operations(vec![upsert_operation("orders", "order-1")], None)
            .await?;

        let mut bytes = tokio::fs::read(&wal_path).await?;
        let last_byte = bytes
            .last_mut()
            .expect("test WAL should contain one complete frame");
        *last_byte ^= 0x01;
        tokio::fs::write(&wal_path, bytes).await?;

        let err = match FramedTableWalAppender::open(&wal_path).await {
            Ok(_) => bail!("corrupt complete frame must not be silently truncated"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("decoding canonical WAL frame")
                || err
                    .chain()
                    .any(|cause| cause.to_string().contains("checksum mismatch"))
        );

        Ok(())
    }
}
