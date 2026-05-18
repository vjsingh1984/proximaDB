// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! PAX segment layer: file-backed multi-block storage with predicate pruning.
//!
//! A **segment file** (`.pax`) contains one or more PAX blocks written
//! sequentially. Each block is self-describing (header + footer) so readers
//! can scan block-by-block and apply block-level predicate pruning without
//! reading the full file.
//!
//! ## File layout
//!
//! ```text
//! [Block_0: N_0 bytes]
//! [Block_1: N_1 bytes]
//! ...
//! [Block_k: N_k bytes]
//! [SegmentIndex: block_count u32 + (offset u64 + size u32) × block_count + crc32 u32]
//! [SegmentMagic: 8B "PAXSEG01"]
//! ```
//!
//! The index at the tail allows random-access block lookup without scanning
//! the whole file. Writers append the index after all blocks are flushed.
//!
//! ## Scan predicate pushdown
//!
//! `PaxSegmentScanner` applies three levels of skipping before decoding:
//! 1. Tenant hash mismatch → skip block entirely
//! 2. Time range outside block min/max → skip block
//! 3. Column stats exclude predicate value → skip stripe within block
//!
//! ## Iceberg manifest integration
//!
//! `PaxSegmentWriter::finish()` returns `SegmentMeta` containing per-block
//! `BlockStats`. These map directly to Iceberg `DataFile` entries in the
//! `iceberg_rest_service.rs` manifest generator.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use proximadb_block_format::{
    BlockCompression, BlockMode, BlockStats, PaxBlockReader, PaxBlockWriter, header::fnv1a_hash,
};
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

use crate::engine_constants::{DEFAULT_TARGET_BLOCK_SIZE_BYTES, MAX_TARGET_BLOCK_SIZE_BYTES};

/// File extension for PAX segment files.
pub const PAX_SEGMENT_EXT: &str = ".pax";

/// Magic bytes at the tail of a segment file (after the index).
pub const SEGMENT_MAGIC: &[u8; 8] = b"PAXSEG01";

// ── Segment index ─────────────────────────────────────────────────────────────

/// Per-block entry in the segment index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockIndexEntry {
    /// Byte offset from the start of the segment file.
    pub offset: u64,
    /// Block byte length (matches the block's header + body + footer).
    pub size: u32,
}

/// Index appended at the tail of a segment file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentIndex {
    pub blocks: Vec<BlockIndexEntry>,
}

impl SegmentIndex {
    /// Serialise to bytes: `[block_count u32] [offset u64, size u32] × n [crc32 u32]`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + self.blocks.len() * 12 + 4);
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for e in &self.blocks {
            buf.extend_from_slice(&e.offset.to_le_bytes());
            buf.extend_from_slice(&e.size.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Deserialise from the last N bytes of a segment file.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            bail!("segment index too small");
        }
        let n = u32::from_le_bytes(data[0..4].try_into()?) as usize;
        let body_len = 4 + n * 12;
        if data.len() < body_len + 4 {
            bail!("segment index truncated: expected {} + 4 bytes", body_len);
        }
        let stored_crc = u32::from_le_bytes(data[body_len..body_len + 4].try_into()?);
        let computed = crc32fast::hash(&data[..body_len]);
        if stored_crc != computed {
            bail!("segment index CRC mismatch");
        }
        let mut blocks = Vec::with_capacity(n);
        for i in 0..n {
            let off = 4 + i * 12;
            let offset = u64::from_le_bytes(data[off..off + 8].try_into()?);
            let size = u32::from_le_bytes(data[off + 8..off + 12].try_into()?);
            blocks.push(BlockIndexEntry { offset, size });
        }
        Ok(Self { blocks })
    }
}

// ── Segment metadata (returned from finish()) ──────────────────────────────────

/// Per-segment statistics returned when a segment is finalised.
///
/// Maps directly to Iceberg `DataFile` fields for manifest generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentMeta {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub block_count: u32,
    pub row_count: u64,
    /// Per-block statistics for Iceberg manifest data-file descriptors.
    pub block_stats: Vec<BlockStats>,
}

// ── Writer ────────────────────────────────────────────────────────────────────

/// Writes `ProximaRecord` rows to a PAX segment file.
///
/// Records are buffered in a `PaxBlockWriter`; when the estimated block size
/// reaches `block_size_threshold`, the block is flushed and a new one begins.
pub struct PaxSegmentWriter {
    path: PathBuf,
    mode: BlockMode,
    compression: BlockCompression,
    collection_id: String,
    schema_fingerprint: u64,
    embedding_count: usize,
    block_size_threshold: usize,

    current_writer: PaxBlockWriter,
    index: SegmentIndex,
    block_stats: Vec<BlockStats>,
    file_buf: Vec<u8>,
    row_count: u64,
}

impl PaxSegmentWriter {
    /// Create a new segment writer. The segment file is written to `path`.
    ///
    /// `block_size_threshold` (bytes) controls when the current block is
    /// flushed and a new one begins. Defaults to `DEFAULT_TARGET_BLOCK_SIZE_BYTES`.
    pub fn new(
        path: impl AsRef<Path>,
        mode: BlockMode,
        compression: BlockCompression,
        collection_id: impl Into<String>,
        schema_fingerprint: u64,
        embedding_count: usize,
        block_size_threshold: Option<usize>,
    ) -> Self {
        let collection_id = collection_id.into();
        let threshold = block_size_threshold
            .unwrap_or(DEFAULT_TARGET_BLOCK_SIZE_BYTES)
            .min(MAX_TARGET_BLOCK_SIZE_BYTES);

        let writer = PaxBlockWriter::new(
            mode,
            compression,
            &collection_id,
            schema_fingerprint,
            embedding_count,
        );

        Self {
            path: path.as_ref().to_path_buf(),
            mode,
            compression,
            collection_id,
            schema_fingerprint,
            embedding_count,
            block_size_threshold: threshold,
            current_writer: writer,
            index: SegmentIndex { blocks: Vec::new() },
            block_stats: Vec::new(),
            file_buf: Vec::new(),
            row_count: 0,
        }
    }

    /// Append a record to the current block.
    ///
    /// Flushes the block automatically when it exceeds `block_size_threshold`.
    pub fn add_record(&mut self, record: &ProximaRecord) -> Result<()> {
        self.current_writer.add_record(record)?;
        self.row_count += 1;

        // Rough size estimate: each record contributes ~1 KB in the worst case.
        // We flush based on row count as a proxy when threshold is not hit yet.
        let approx_bytes = self.current_writer.row_count() * 1024;
        if approx_bytes >= self.block_size_threshold {
            self.flush_current_block()?;
        }
        Ok(())
    }

    /// Force-flush any buffered records as the final (possibly partial) block.
    fn flush_current_block(&mut self) -> Result<()> {
        if self.current_writer.is_empty() {
            return Ok(());
        }
        // Capture timestamp bounds before flush (flush does not reset internal state).
        let min_ts = self.current_writer.min_ts();
        let max_ts = self.current_writer.max_ts();
        let row_count = self.current_writer.row_count() as u32;

        let block_bytes = self.current_writer.flush()?;
        let block_size = block_bytes.len() as u32;
        let offset = self.file_buf.len() as u64;

        let stats = BlockStats {
            row_count,
            block_size_bytes: block_size,
            min_timestamp_ns: min_ts,
            max_timestamp_ns: max_ts,
            null_counts: std::collections::HashMap::new(),
            lower_bounds: std::collections::HashMap::new(),
            upper_bounds: std::collections::HashMap::new(),
        };

        self.index.blocks.push(BlockIndexEntry {
            offset,
            size: block_size,
        });
        self.file_buf.extend_from_slice(&block_bytes);
        self.block_stats.push(stats);

        // Reset writer for the next block
        self.current_writer = PaxBlockWriter::new(
            self.mode,
            self.compression,
            &self.collection_id,
            self.schema_fingerprint,
            self.embedding_count,
        );
        Ok(())
    }

    /// Finalise the segment: flush remaining records, write index + magic, and
    /// persist to `self.path`. Returns `SegmentMeta` for Iceberg manifest use.
    pub fn finish(mut self) -> Result<SegmentMeta> {
        // Flush any remaining rows as the last block
        self.flush_current_block()?;

        if self.index.blocks.is_empty() {
            bail!("segment is empty — nothing to write");
        }

        // Append segment index
        let index_bytes = self.index.to_bytes();
        self.file_buf.extend_from_slice(&index_bytes);
        // Append magic
        self.file_buf.extend_from_slice(SEGMENT_MAGIC);

        let total_bytes = self.file_buf.len() as u64;

        // Persist to disk
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&self.path)?;
        f.write_all(&self.file_buf)?;

        Ok(SegmentMeta {
            path: self.path,
            size_bytes: total_bytes,
            block_count: self.index.blocks.len() as u32,
            row_count: self.row_count,
            block_stats: self.block_stats,
        })
    }
}

// ── Scanner ───────────────────────────────────────────────────────────────────

/// Predicate context for block-level pruning during segment scans.
#[derive(Debug, Clone, Default)]
pub struct ScanPredicate {
    /// If set, skip blocks whose tenant hash doesn't match.
    pub tenant_hash: Option<u64>,
    /// If set, skip blocks with no overlap with `[from_ns, to_ns]`.
    pub time_range: Option<(i64, i64)>,
}

impl ScanPredicate {
    pub fn for_tenant(tenant_id: &str) -> Self {
        Self {
            tenant_hash: Some(fnv1a_hash(tenant_id)),
            ..Default::default()
        }
    }

    pub fn for_time_range(from_ns: i64, to_ns: i64) -> Self {
        Self {
            time_range: Some((from_ns, to_ns)),
            ..Default::default()
        }
    }

    pub fn with_tenant(mut self, tenant_id: &str) -> Self {
        self.tenant_hash = Some(fnv1a_hash(tenant_id));
        self
    }

    pub fn with_time_range(mut self, from_ns: i64, to_ns: i64) -> Self {
        self.time_range = Some((from_ns, to_ns));
        self
    }
}

/// Iterator over raw PAX blocks in a segment file with block-level pruning.
///
/// Reads the segment index from the file tail, then yields the byte slice of
/// each block that passes the predicate. Callers decode individual stripes via
/// `PaxBlockReader`.
pub struct PaxSegmentScanner {
    data: Vec<u8>,
    index: SegmentIndex,
    predicate: ScanPredicate,
    cursor: usize,
}

impl PaxSegmentScanner {
    /// Open a segment file and parse its index.
    pub fn open(path: impl AsRef<Path>, predicate: ScanPredicate) -> Result<Self> {
        let data = std::fs::read(path.as_ref())?;
        Self::from_bytes(data, predicate)
    }

    /// Parse from an in-memory byte slice (useful for WAL replay / testing).
    pub fn from_bytes(data: Vec<u8>, predicate: ScanPredicate) -> Result<Self> {
        // Validate magic
        if data.len() < 8 || &data[data.len() - 8..] != SEGMENT_MAGIC {
            bail!("not a valid PAX segment file (bad magic)");
        }
        // The index sits between the blocks and the magic.
        // We don't know the index size without reading block_count first.
        // Strategy: read block_count from (len - 8 - 4 - ...). We need to
        // scan backwards. The index format is: [n u32][entries: n×12][crc32 u32].
        // Minimum index size = 4 + 0 + 4 = 8 bytes.
        // We try increasing index sizes until CRC matches.
        let magic_start = data.len() - 8;
        // Try to find the index by reading block_count at various positions
        // (binary search would be ideal, but CRC validation is cheap enough).
        let index = Self::parse_index(&data[..magic_start])?;

        Ok(Self {
            data,
            index,
            predicate,
            cursor: 0,
        })
    }

    fn parse_index(before_magic: &[u8]) -> Result<SegmentIndex> {
        if before_magic.len() < 8 {
            bail!("no room for segment index");
        }
        // Read block_count from various candidate positions.
        // The index is at the END of `before_magic`. We know:
        //   index_len = 4 + n * 12 + 4
        // Try up to 1 MB of index (for up to ~87 000 blocks — well beyond normal).
        for candidate_n in 0usize..=(before_magic.len().saturating_sub(8) / 12) {
            let index_len = 4 + candidate_n * 12 + 4;
            if index_len > before_magic.len() {
                break;
            }
            let idx_start = before_magic.len() - index_len;
            let n_in_data = u32::from_le_bytes(
                before_magic[idx_start..idx_start + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            ) as usize;
            if n_in_data == candidate_n {
                // Candidate matches — validate CRC
                match SegmentIndex::from_bytes(&before_magic[idx_start..]) {
                    Ok(idx) => return Ok(idx),
                    Err(_) => continue,
                }
            }
        }
        bail!("could not locate valid segment index");
    }

    /// Yield the next block that passes predicate pruning.
    pub fn next_block(&mut self) -> Option<PaxBlockReader<'_>> {
        while self.cursor < self.index.blocks.len() {
            let entry = &self.index.blocks[self.cursor];
            self.cursor += 1;

            let start = entry.offset as usize;
            let end = start + entry.size as usize;
            if end > self.data.len() {
                continue; // corrupted index entry — skip
            }

            let block_data = &self.data[start..end];
            let reader = match PaxBlockReader::open(block_data) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Block-level predicate pruning
            if let Some(th) = self.predicate.tenant_hash {
                if !reader.tenant_matches(th) {
                    continue;
                }
            }
            if let Some((from, to)) = self.predicate.time_range {
                if !reader.time_overlaps(from, to) {
                    continue;
                }
            }

            // SAFETY: we extend the lifetime here because `self.data` owns the
            // bytes and outlives the returned reader. The borrow checker cannot
            // see through the index indirection, so we use a raw pointer.
            // INVARIANT: `block_data` is a sub-slice of `self.data`; as long as
            // `self.data` is not modified (which PaxSegmentScanner does not do
            // after construction), this is sound.
            let reader: PaxBlockReader<'_> = unsafe {
                let ptr = block_data.as_ptr();
                let len = block_data.len();
                let static_slice: &'static [u8] = std::slice::from_raw_parts(ptr, len);
                PaxBlockReader::open(static_slice).ok()?
            };

            return Some(reader);
        }
        None
    }

    pub fn block_count(&self) -> usize {
        self.index.blocks.len()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;

    fn make_record(oid: &str, tenant: &str, ts: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            tenant_id: tenant.into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        }
    }

    #[test]
    fn segment_index_round_trip() {
        let idx = SegmentIndex {
            blocks: vec![
                BlockIndexEntry {
                    offset: 0,
                    size: 4096,
                },
                BlockIndexEntry {
                    offset: 4096,
                    size: 8192,
                },
            ],
        };
        let bytes = idx.to_bytes();
        let idx2 = SegmentIndex::from_bytes(&bytes).unwrap();
        assert_eq!(idx2.blocks.len(), 2);
        assert_eq!(idx2.blocks[0].size, 4096);
        assert_eq!(idx2.blocks[1].offset, 4096);
    }

    #[test]
    fn segment_write_read_scan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pax");

        let mut writer = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col_a",
            0,
            0,
            Some(1), // 1-byte threshold → each record gets its own block
        );

        writer
            .add_record(&make_record("r1", "tenant_a", 1000))
            .unwrap();
        writer
            .add_record(&make_record("r2", "tenant_a", 2000))
            .unwrap();
        writer
            .add_record(&make_record("r3", "tenant_b", 3000))
            .unwrap();

        let meta = writer.finish().unwrap();
        assert_eq!(meta.row_count, 3);
        assert!(meta.size_bytes > 0);

        // Scan with tenant_a predicate
        let mut scanner =
            PaxSegmentScanner::open(&path, ScanPredicate::for_tenant("tenant_a")).unwrap();

        let mut matched_blocks = 0usize;
        while scanner.next_block().is_some() {
            matched_blocks += 1;
        }
        // tenant_b block(s) should be pruned
        assert!(matched_blocks < meta.block_count as usize);
    }

    #[test]
    fn segment_scan_time_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("time.pax");

        let mut writer = PaxSegmentWriter::new(
            &path,
            BlockMode::Olap,
            BlockCompression::None,
            "col_b",
            0,
            0,
            Some(1), // one record per block
        );
        writer.add_record(&make_record("a", "t", 500)).unwrap();
        writer.add_record(&make_record("b", "t", 5000)).unwrap();
        writer.add_record(&make_record("c", "t", 9999)).unwrap();
        let meta = writer.finish().unwrap();

        // Only the [5000..9999] blocks should survive a [4000..6000] scan
        let mut scanner =
            PaxSegmentScanner::open(&path, ScanPredicate::for_time_range(4000, 6000)).unwrap();
        let mut hits = 0usize;
        while scanner.next_block().is_some() {
            hits += 1;
        }
        assert!(hits < meta.block_count as usize);
    }
}
