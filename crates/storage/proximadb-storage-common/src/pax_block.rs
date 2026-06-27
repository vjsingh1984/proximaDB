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
use std::sync::LazyLock;

use anyhow::{Result, bail};
use proximadb_block_format::{
    BlockCompression, BlockMode, BlockStats, BlockZoneSource, ColumnMeta, FlatRow, PaxBlockReader,
    PaxBlockWriter, RowGroupBlock, VectorQuant, col_id, header::fnv1a_hash,
};
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

use crate::engine_constants::{DEFAULT_TARGET_BLOCK_SIZE_BYTES, MAX_TARGET_BLOCK_SIZE_BYTES};

/// File extension for PAX segment files.
pub const PAX_SEGMENT_EXT: &str = ".pax";

/// Magic bytes at the tail of a segment file (after the index).
pub const SEGMENT_MAGIC: &[u8; 8] = b"PAXSEG01";

// ── Segment index ─────────────────────────────────────────────────────────────

/// Trailer marker for a **v2** segment index carrying per-block zone-map
/// summaries (TD-167 follow-up / ADR-034 P1). v1 indexes lack it, so the reader
/// detects the format by whether the bytes before [`SEGMENT_MAGIC`] end with this
/// marker — keeping v1 byte-identical (old readers/segments unaffected).
pub const ZONE_INDEX_MARKER: &[u8; 4] = b"PAXZ";

// Bit positions in [`BlockZoneSummary::present`] — only the numeric canonical
// columns predicate pushdown prunes on without decoding rows.
const ZONE_CREATED_AT: u8 = 1 << 0;
const ZONE_UPDATED_AT: u8 = 1 << 1;
const ZONE_VALID_FROM: u8 = 1 << 2;
const ZONE_VALID_TO: u8 = 1 << 3;
const ZONE_EDGE_WEIGHT: u8 = 1 << 4;

/// Serialized fixed width of a [`BlockZoneSummary`]: row_count(4) + present(1) +
/// 4×(i64,i64)=64 + (f64,f64)=16.
const ZONE_SUMMARY_BYTES: usize = 4 + 1 + 64 + 16;
/// v2 per-block index entry width: offset(8) + size(4) + zone summary.
const V2_ENTRY_BYTES: usize = 8 + 4 + ZONE_SUMMARY_BYTES;

/// Fixed-width per-block zone-map summary embedded in a v2 segment index, so the
/// reader can prune a block from the *cached index* with **zero** per-block
/// footer/metadata GETs (cold-path depth 4→2). min/max are copied verbatim from
/// the block's `ColumnMeta` (the same bounds the full-layout prune uses), so v2
/// pruning is byte-for-byte identical to v1 for these predicates; columns absent
/// from the summary fall through to "may match" (conservative — never skips a
/// block wrongly).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockZoneSummary {
    pub row_count: u32,
    /// Bitmask of which canonical columns carry valid bounds.
    pub present: u8,
    pub created_at: (i64, i64),
    pub updated_at: (i64, i64),
    pub valid_from: (i64, i64),
    pub valid_to: (i64, i64),
    pub edge_weight: (f64, f64),
}

impl BlockZoneSummary {
    /// An empty summary (no bounds) — used for blocks whose zone is unknown when
    /// a v2 index is written.
    pub fn empty(row_count: u32) -> Self {
        Self {
            row_count,
            present: 0,
            created_at: (0, 0),
            updated_at: (0, 0),
            valid_from: (0, 0),
            valid_to: (0, 0),
            edge_weight: (0.0, 0.0),
        }
    }

    /// Extract the canonical-column bounds from a flushed block's column metadata.
    /// A column with `distinct_hint == 0` (no usable bounds) is left out of
    /// `present`.
    pub fn from_column_metas(row_count: u32, metas: &[ColumnMeta]) -> Self {
        let mut s = Self::empty(row_count);
        for m in metas {
            if m.distinct_hint == 0 {
                continue;
            }
            let i64_lo = i64::from_le_bytes(m.min_val[0..8].try_into().unwrap_or([0; 8]));
            let i64_hi = i64::from_le_bytes(m.max_val[0..8].try_into().unwrap_or([0; 8]));
            match m.column_id {
                col_id::CREATED_AT => {
                    s.created_at = (i64_lo, i64_hi);
                    s.present |= ZONE_CREATED_AT;
                }
                col_id::UPDATED_AT => {
                    s.updated_at = (i64_lo, i64_hi);
                    s.present |= ZONE_UPDATED_AT;
                }
                col_id::VALID_FROM => {
                    s.valid_from = (i64_lo, i64_hi);
                    s.present |= ZONE_VALID_FROM;
                }
                col_id::VALID_TO => {
                    s.valid_to = (i64_lo, i64_hi);
                    s.present |= ZONE_VALID_TO;
                }
                col_id::EDGE_WEIGHT => {
                    s.edge_weight = (
                        f64::from_le_bytes(m.min_val[0..8].try_into().unwrap_or([0; 8])),
                        f64::from_le_bytes(m.max_val[0..8].try_into().unwrap_or([0; 8])),
                    );
                    s.present |= ZONE_EDGE_WEIGHT;
                }
                _ => {}
            }
        }
        s
    }

    fn write_to(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.row_count.to_le_bytes());
        buf.push(self.present);
        for (lo, hi) in [
            self.created_at,
            self.updated_at,
            self.valid_from,
            self.valid_to,
        ] {
            buf.extend_from_slice(&lo.to_le_bytes());
            buf.extend_from_slice(&hi.to_le_bytes());
        }
        buf.extend_from_slice(&self.edge_weight.0.to_le_bytes());
        buf.extend_from_slice(&self.edge_weight.1.to_le_bytes());
    }

    fn read_from(data: &[u8]) -> Result<Self> {
        if data.len() < ZONE_SUMMARY_BYTES {
            bail!("zone summary truncated");
        }
        let row_count = u32::from_le_bytes(data[0..4].try_into()?);
        let present = data[4];
        let rd = |off: usize| -> Result<(i64, i64)> {
            Ok((
                i64::from_le_bytes(data[off..off + 8].try_into()?),
                i64::from_le_bytes(data[off + 8..off + 16].try_into()?),
            ))
        };
        let created_at = rd(5)?;
        let updated_at = rd(21)?;
        let valid_from = rd(37)?;
        let valid_to = rd(53)?;
        let edge_weight = (
            f64::from_le_bytes(data[69..77].try_into()?),
            f64::from_le_bytes(data[77..85].try_into()?),
        );
        Ok(Self {
            row_count,
            present,
            created_at,
            updated_at,
            valid_from,
            valid_to,
            edge_weight,
        })
    }

    fn i64_bounds(&self, column_id: i32) -> Option<(i64, i64)> {
        match column_id {
            col_id::CREATED_AT if self.present & ZONE_CREATED_AT != 0 => Some(self.created_at),
            col_id::UPDATED_AT if self.present & ZONE_UPDATED_AT != 0 => Some(self.updated_at),
            col_id::VALID_FROM if self.present & ZONE_VALID_FROM != 0 => Some(self.valid_from),
            col_id::VALID_TO if self.present & ZONE_VALID_TO != 0 => Some(self.valid_to),
            _ => None,
        }
    }

    fn f64_bounds(&self, column_id: i32) -> Option<(f64, f64)> {
        match column_id {
            col_id::EDGE_WEIGHT if self.present & ZONE_EDGE_WEIGHT != 0 => Some(self.edge_weight),
            _ => None,
        }
    }
}

/// Shared empty row-group sub-index for summary-based pruning (the summary is
/// block-level only — no per-row-group bounds).
static EMPTY_ROW_GROUP: LazyLock<RowGroupBlock> = LazyLock::new(RowGroupBlock::default);

// block-format wire `data_type_id`s (private in prune.rs) — stable on-disk codec
// markers, mirrored here so the summary can advertise the right predicate path.
const DT_I64: u8 = 0x03;
const DT_F64: u8 = 0x07;

/// Prune a block from its compact zone-map summary alone — the reader path that
/// needs NO per-block footer/metadata GET. Soundness matches the full-layout
/// prune: bounds are copied verbatim from the block's `ColumnMeta`, and any column
/// not in the summary advertises `None` (→ `evaluate_leaf` returns `MayMatch`), so
/// a block is skipped only when provably empty for the summarized predicate.
impl BlockZoneSource for BlockZoneSummary {
    fn column_meta_type(&self, column_id: i32) -> Option<u8> {
        if self.i64_bounds(column_id).is_some() {
            Some(DT_I64)
        } else if self.f64_bounds(column_id).is_some() {
            Some(DT_F64)
        } else {
            None
        }
    }

    fn may_contain_i64(&self, column_id: i32, value: i64) -> bool {
        self.i64_bounds(column_id)
            .is_none_or(|(lo, hi)| value >= lo && value <= hi)
    }

    fn range_overlaps_i64(&self, column_id: i32, lo: i64, hi: i64) -> bool {
        self.i64_bounds(column_id)
            .is_none_or(|(min, max)| lo <= max && hi >= min)
    }

    fn range_overlaps_f64(&self, column_id: i32, lo: f64, hi: f64) -> bool {
        self.f64_bounds(column_id).is_none_or(|(min, max)| {
            if min.is_nan() || max.is_nan() {
                return true;
            }
            lo <= max && hi >= min
        })
    }

    fn may_contain_str(&self, _column_id: i32, _value: &str) -> bool {
        true // the summary carries no string bounds; unreachable (column_meta_type → None)
    }

    fn row_group_index(&self) -> &RowGroupBlock {
        &EMPTY_ROW_GROUP
    }

    fn row_count_hint(&self) -> u32 {
        self.row_count
    }
}

/// Per-block entry in the segment index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockIndexEntry {
    /// Byte offset from the start of the segment file.
    pub offset: u64,
    /// Block byte length (matches the block's header + body + footer).
    pub size: u32,
    /// v2 zone-map summary (`None` in a legacy v1 index — prune falls back to a
    /// per-block metadata read).
    #[serde(default)]
    pub zone: Option<BlockZoneSummary>,
}

/// Index appended at the tail of a segment file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentIndex {
    pub blocks: Vec<BlockIndexEntry>,
}

impl SegmentIndex {
    /// Serialise to bytes. v1 (no zone summaries): `[n u32] [offset u64, size u32]
    /// × n [crc32 u32]`. When any block carries a zone summary, emit v2 instead:
    /// `[n u32] [offset, size, zone(85B)] × n [crc32 u32] [body_len u32] [PAXZ]`.
    /// The trailing `PAXZ` marker lets the reader detect v2 without disturbing v1.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.blocks.iter().any(|b| b.zone.is_some()) {
            self.to_bytes_v2()
        } else {
            self.to_bytes_v1()
        }
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
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

    fn to_bytes_v2(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + self.blocks.len() * V2_ENTRY_BYTES + 12);
        body.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for e in &self.blocks {
            body.extend_from_slice(&e.offset.to_le_bytes());
            body.extend_from_slice(&e.size.to_le_bytes());
            e.zone
                .clone()
                .unwrap_or_else(|| BlockZoneSummary::empty(0))
                .write_to(&mut body);
        }
        let crc = crc32fast::hash(&body);
        body.extend_from_slice(&crc.to_le_bytes());
        // Self-describing trailer so the reader needs no brute-force for v2.
        let body_len = body.len() as u32;
        body.extend_from_slice(&body_len.to_le_bytes());
        body.extend_from_slice(ZONE_INDEX_MARKER);
        body
    }

    /// Deserialise a **v1** index from the last N bytes of a segment file.
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
            blocks.push(BlockIndexEntry {
                offset,
                size,
                zone: None,
            });
        }
        Ok(Self { blocks })
    }

    /// Deserialise a **v2** index body (`[n][offset,size,zone]×n[crc]`, without the
    /// `[body_len][PAXZ]` trailer).
    fn from_bytes_v2(body: &[u8]) -> Result<Self> {
        if body.len() < 8 {
            bail!("v2 segment index too small");
        }
        let n = u32::from_le_bytes(body[0..4].try_into()?) as usize;
        let crc_pos = 4 + n * V2_ENTRY_BYTES;
        if body.len() < crc_pos + 4 {
            bail!("v2 segment index truncated");
        }
        let stored_crc = u32::from_le_bytes(body[crc_pos..crc_pos + 4].try_into()?);
        if crc32fast::hash(&body[..crc_pos]) != stored_crc {
            bail!("v2 segment index CRC mismatch");
        }
        let mut blocks = Vec::with_capacity(n);
        for i in 0..n {
            let off = 4 + i * V2_ENTRY_BYTES;
            let offset = u64::from_le_bytes(body[off..off + 8].try_into()?);
            let size = u32::from_le_bytes(body[off + 8..off + 12].try_into()?);
            let zone = BlockZoneSummary::read_from(&body[off + 12..off + 12 + ZONE_SUMMARY_BYTES])?;
            blocks.push(BlockIndexEntry {
                offset,
                size,
                zone: Some(zone),
            });
        }
        Ok(Self { blocks })
    }

    /// Locate and parse the index at the tail of `before_magic` (the segment bytes
    /// with the trailing [`SEGMENT_MAGIC`] removed). v2 is found directly via the
    /// self-describing `[body_len][PAXZ]` trailer; v1's length is not stored, so
    /// candidate counts are tried until the embedded count + CRC validate. Shared
    /// by the whole-file scanner and the ranged reader.
    pub fn locate(before_magic: &[u8]) -> Result<Self> {
        if before_magic.len() < 8 {
            bail!("no room for segment index");
        }
        // v2: trailing [body_len u32][PAXZ].
        if &before_magic[before_magic.len() - 4..] == ZONE_INDEX_MARKER {
            let blen_pos = before_magic.len() - 8;
            let body_len =
                u32::from_le_bytes(before_magic[blen_pos..blen_pos + 4].try_into()?) as usize;
            if before_magic.len() < 8 + body_len {
                bail!("v2 index marker present but body does not fit");
            }
            let body = &before_magic[before_magic.len() - 8 - body_len..before_magic.len() - 8];
            return Self::from_bytes_v2(body);
        }
        // v1 brute-force.
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
            if n_in_data == candidate_n
                && let Ok(idx) = SegmentIndex::from_bytes(&before_magic[idx_start..])
            {
                return Ok(idx);
            }
        }
        bail!("could not locate valid segment index");
    }

    /// Locate and parse the index from a file **suffix** that must contain the
    /// trailing [`SEGMENT_MAGIC`] and the full index. Returns `Ok(None)` when the
    /// suffix is too small to hold the whole index (the caller should re-read a
    /// larger suffix), and `Err` only on a corrupt/invalid tail.
    pub fn locate_in_suffix(suffix: &[u8]) -> Result<Option<Self>> {
        if suffix.len() < 8 || &suffix[suffix.len() - 8..] != SEGMENT_MAGIC {
            bail!("segment suffix missing magic (not a PAX segment tail)");
        }
        let before_magic = &suffix[..suffix.len() - 8];
        // A v2 index needs its whole body in the suffix; signal a re-read otherwise.
        if before_magic.len() >= 8 && &before_magic[before_magic.len() - 4..] == ZONE_INDEX_MARKER {
            let blen_pos = before_magic.len() - 8;
            let body_len =
                u32::from_le_bytes(before_magic[blen_pos..blen_pos + 4].try_into()?) as usize;
            if before_magic.len() < 8 + body_len {
                return Ok(None);
            }
        }
        match Self::locate(before_magic) {
            Ok(idx) => Ok(Some(idx)),
            Err(_) => Ok(None),
        }
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
    /// Vector quantization strategy for every block in this segment (P3 Phase D).
    quant: VectorQuant,

    current_writer: PaxBlockWriter,
    index: SegmentIndex,
    block_stats: Vec<BlockStats>,
    file_buf: Vec<u8>,
    row_count: u64,
    /// Emit a v2 zone-map segment index (per-block bounds for index-only pruning).
    /// Gated default-OFF by `PROXIMADB_PAX_INDEX_ZONEMAP`; mixed-read-safe.
    zonemap: bool,
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
            quant: VectorQuant::Auto,
            current_writer: writer,
            index: SegmentIndex { blocks: Vec::new() },
            block_stats: Vec::new(),
            file_buf: Vec::new(),
            row_count: 0,
            zonemap: std::env::var("PROXIMADB_PAX_INDEX_ZONEMAP")
                .map(|v| matches!(v.as_str(), "1" | "true" | "on" | "yes"))
                .unwrap_or(false),
        }
    }

    /// Force the v2 zone-map segment index on/off for this writer, overriding the
    /// `PROXIMADB_PAX_INDEX_ZONEMAP` env default. Builder form (deterministic for
    /// tests, which must not depend on a process-global env var).
    pub fn with_zonemap(mut self, on: bool) -> Self {
        self.zonemap = on;
        self
    }

    /// Set the vector quantization strategy for this segment (P3 Phase D). Builder form
    /// so existing `new(..)` callers are unchanged; rebuilds the (still-empty) current
    /// block writer so the strategy applies from the first record. `Auto` = env default.
    pub fn with_quant(mut self, quant: VectorQuant) -> Self {
        self.quant = quant;
        self.current_writer = PaxBlockWriter::new(
            self.mode,
            self.compression,
            &self.collection_id,
            self.schema_fingerprint,
            self.embedding_count,
        )
        .with_quant(quant);
        self
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

        let reader = PaxBlockReader::open(&block_bytes)?;
        let stats =
            BlockStats::from_metas(row_count, block_size, min_ts, max_ts, reader.column_metas());

        // TD-167 follow-up: when the v2 zone-map index is enabled, capture each
        // block's canonical-column bounds (already in hand via the just-opened
        // reader) into the index, so the reader can prune from the cached index
        // with no per-block metadata GET. Gated default-OFF (mixed-read-safe).
        let zone = self
            .zonemap
            .then(|| BlockZoneSummary::from_column_metas(row_count, reader.column_metas()));
        self.index.blocks.push(BlockIndexEntry {
            offset,
            size: block_size,
            zone,
        });
        self.file_buf.extend_from_slice(&block_bytes);
        self.block_stats.push(stats);

        // Reset writer for the next block (preserving the segment's quant strategy).
        self.current_writer = PaxBlockWriter::new(
            self.mode,
            self.compression,
            &self.collection_id,
            self.schema_fingerprint,
            self.embedding_count,
        )
        .with_quant(self.quant);
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
        SegmentIndex::locate(before_magic)
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
            if let Some(th) = self.predicate.tenant_hash
                && !reader.tenant_matches(th)
            {
                continue;
            }
            if let Some((from, to)) = self.predicate.time_range
                && !reader.time_overlaps(from, to)
            {
                continue;
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

    /// Reconstruct every row of the segment into full [`ProximaRecord`]s — the
    /// canonical read-side inverse of `PaxSegmentWriter::add_record`. Iterates
    /// the (predicate-pruned) blocks, rebuilds each row via
    /// [`FlatRow::from_block_reader`], and materializes records through
    /// [`FlatRow::into_record`].
    ///
    /// `embedding_model_ids` / `user_column_keys` come from the collection schema
    /// (the segment stores embeddings positionally and does not persist model ids
    /// or promoted-column names). Pass empty slices for best-effort defaults
    /// (`model_0`, `model_1`, …).
    /// `tenant_ctx` is the segment's owning tenant (from the catalog/path); it is
    /// stamped onto rows whose tenant column was dropped (catalog-resolution) and
    /// ignored when the column is still present. Pass `None` to keep stored values.
    pub fn read_records(
        &mut self,
        embedding_model_ids: &[String],
        user_column_keys: &[String],
        tenant_ctx: Option<&str>,
    ) -> Result<Vec<ProximaRecord>> {
        let mut records = Vec::new();
        while let Some(block) = self.next_block() {
            for flat in FlatRow::from_block_reader(&block)? {
                records.push(flat.into_record(
                    embedding_model_ids,
                    user_column_keys,
                    tenant_ctx,
                )?);
            }
        }
        Ok(records)
    }
}

// ── Compaction (TD-114) ─────────────────────────────────────────────────────────

/// Statistics from a PAX segment compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionStats {
    /// Number of input segment files merged.
    pub inputs: usize,
    /// Total records read across all inputs.
    pub records_in: u64,
    /// Records written to the output (survivors, after dropping tombstones).
    pub records_out: u64,
    /// Records dropped because they were tombstones at `now_ns` (merge-on-read deletes).
    pub tombstones_dropped: u64,
}

/// Merge several L0 PAX segments into one L1 segment, dropping records that are
/// tombstones as of `now_ns` (merge-on-read deletes; TD-114).
///
/// `embedding_model_ids` and `user_column_keys` are the collection's schema keys
/// used to reconstruct records (see [`PaxSegmentScanner::read_records`]). Inputs are
/// read in order; surviving records are written to `output` via a fresh
/// [`PaxSegmentWriter`]. This is a pure, engine-agnostic primitive: the caller owns
/// L0 discovery (the input paths) and manifest registration of the output.
#[allow(clippy::too_many_arguments)]
pub fn compact_pax_segments(
    inputs: &[PathBuf],
    output: &Path,
    mode: BlockMode,
    compression: BlockCompression,
    collection_id: &str,
    schema_fingerprint: u64,
    embedding_count: usize,
    embedding_model_ids: &[String],
    user_column_keys: &[String],
    tenant_ctx: Option<&str>,
    now_ns: i64,
) -> Result<CompactionStats> {
    let mut writer = PaxSegmentWriter::new(
        output,
        mode,
        compression,
        collection_id,
        schema_fingerprint,
        embedding_count,
        None,
    );
    let mut records_in = 0u64;
    let mut records_out = 0u64;
    let mut tombstones_dropped = 0u64;

    for input in inputs {
        let mut scanner = PaxSegmentScanner::open(input, ScanPredicate::default())?;
        for record in scanner.read_records(embedding_model_ids, user_column_keys, tenant_ctx)? {
            records_in += 1;
            if record.is_tombstone_at(now_ns) {
                tombstones_dropped += 1;
                continue;
            }
            writer.add_record(&record)?;
            records_out += 1;
        }
    }
    writer.finish()?;

    Ok(CompactionStats {
        inputs: inputs.len(),
        records_in,
        records_out,
        tombstones_dropped,
    })
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
                    zone: None,
                },
                BlockIndexEntry {
                    offset: 4096,
                    size: 8192,
                    zone: None,
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
    fn segment_index_v2_zonemap_round_trips_and_detects_via_locate() {
        let mut zone = BlockZoneSummary::empty(10);
        zone.created_at = (1000, 2000);
        zone.valid_to = (0, 5000);
        zone.present = ZONE_CREATED_AT | ZONE_VALID_TO;
        let idx = SegmentIndex {
            blocks: vec![
                BlockIndexEntry {
                    offset: 0,
                    size: 4096,
                    zone: Some(zone.clone()),
                },
                BlockIndexEntry {
                    offset: 4096,
                    size: 8192,
                    zone: Some(BlockZoneSummary::empty(3)),
                },
            ],
        };
        // to_bytes picks v2 (a zone is present) and appends the PAXZ trailer.
        let mut bytes = idx.to_bytes();
        assert_eq!(
            &bytes[bytes.len() - 4..],
            ZONE_INDEX_MARKER,
            "v2 trailer marker"
        );
        // The reader path strips SEGMENT_MAGIC, then locate() detects v2 by PAXZ.
        bytes.extend_from_slice(SEGMENT_MAGIC);
        let located = SegmentIndex::locate(&bytes[..bytes.len() - 8]).unwrap();
        assert_eq!(located.blocks.len(), 2);
        let z0 = located.blocks[0].zone.as_ref().expect("v2 zone preserved");
        assert_eq!(z0.created_at, (1000, 2000));
        assert_eq!(z0.valid_to, (0, 5000));
        assert_eq!(z0.present, ZONE_CREATED_AT | ZONE_VALID_TO);
        assert_eq!(located.blocks[1].size, 8192);
    }

    #[test]
    fn compact_drops_tombstones_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let seg0 = dir.path().join("L0_0.pax");
        let seg1 = dir.path().join("L0_1.pax");
        let out = dir.path().join("L1.pax");

        // L0 segment 0: two live records.
        let mut w0 = PaxSegmentWriter::new(
            &seg0,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            0,
            Some(1),
        );
        w0.add_record(&make_record("a", "t", 100)).unwrap();
        w0.add_record(&make_record("b", "t", 200)).unwrap();
        w0.finish().unwrap();

        // L0 segment 1: one live record + a tombstone (deleted at ts=500).
        let mut w1 = PaxSegmentWriter::new(
            &seg1,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            0,
            Some(1),
        );
        w1.add_record(&make_record("c", "t", 300)).unwrap();
        let mut tombstone = make_record("a", "t", 500);
        tombstone.valid_to_ns = Some(500);
        tombstone.origin = Some("delete".to_string());
        w1.add_record(&tombstone).unwrap();
        w1.finish().unwrap();

        // Compact as of now=1000 (after the delete) → the tombstone is dropped.
        let stats = compact_pax_segments(
            &[seg0, seg1],
            &out,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            0,
            &[],
            &[],
            None,
            1000,
        )
        .unwrap();

        assert_eq!(stats.inputs, 2);
        assert_eq!(stats.records_in, 4);
        assert_eq!(stats.tombstones_dropped, 1);
        assert_eq!(stats.records_out, 3);

        // The merged L1 segment holds exactly the 3 survivors; no tombstone remains.
        let mut scanner = PaxSegmentScanner::open(&out, ScanPredicate::default()).unwrap();
        let survivors = scanner.read_records(&[], &[], None).unwrap();
        assert_eq!(survivors.len(), 3);
        assert!(
            survivors
                .iter()
                .all(|r| r.origin.as_deref() != Some("delete")),
            "the delete tombstone must not survive compaction"
        );
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
        assert_eq!(meta.block_stats.len(), meta.block_count as usize);
        assert!(
            meta.block_stats[0]
                .distinct_counts
                .contains_key(&proximadb_block_format::col_id::TENANT_ID)
        );
        assert!(
            meta.block_stats[0]
                .bloom_filter_bytes
                .contains_key(&proximadb_block_format::col_id::TENANT_ID)
        );
        assert_eq!(
            meta.block_stats[0]
                .lower_bounds
                .get(&proximadb_block_format::col_id::CREATED_AT),
            Some(&1000)
        );

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

    /// `read_records` is the canonical inverse of `add_record`: props, labels,
    /// timestamps, and the dense embedding all round-trip (not just oid+vector).
    #[test]
    fn segment_read_records_round_trips_full_fidelity() {
        use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaTreeNode, ProximaValue};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.pax");

        let mut rich = make_record("r1", "tenant_a", 1_700_000_000_000_000_000);
        rich.props.insert(
            "category".into(),
            ProximaTreeNode::Value(ProximaValue::String("books".into())),
        );
        rich.props
            .insert("qty".into(), ProximaTreeNode::Value(ProximaValue::Int64(7)));
        rich.labels = vec!["a".to_string(), "b".to_string()].into();
        rich.embeddings.push(EmbeddingCell {
            modality: "dense".into(),
            dim: 3,
            values: EmbeddingValues::Fp32(vec![1.0, 2.0, 3.0]),
            ..Default::default()
        });

        let mut writer = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col_full",
            0,
            1, // embedding_count
            None,
        );
        writer.add_record(&rich).unwrap();
        writer
            .add_record(&make_record("r2", "tenant_a", 1_700_000_000_000_000_001))
            .unwrap();
        writer.finish().unwrap();

        let mut scanner = PaxSegmentScanner::open(&path, ScanPredicate::default()).unwrap();
        let records = scanner.read_records(&[], &[], None).unwrap();

        assert_eq!(records.len(), 2);
        let r1 = records.iter().find(|r| r.oid == "r1").expect("r1 present");
        assert_eq!(r1.tenant_id, "tenant_a");
        assert_eq!(r1.created_at_ns, 1_700_000_000_000_000_000);
        assert_eq!(
            r1.props.get("category"),
            Some(&ProximaTreeNode::Value(ProximaValue::String(
                "books".into()
            )))
        );
        assert_eq!(
            r1.props.get("qty"),
            Some(&ProximaTreeNode::Value(ProximaValue::Int64(7)))
        );
        let mut labels: Vec<String> = r1.labels.iter().map(|s| s.to_string()).collect();
        labels.sort();
        assert_eq!(labels, vec!["a".to_string(), "b".to_string()]);
        // Vectors are SQ8-quantized in PAX v2 (lossy, 4× smaller): assert the
        // embedding reconstructs within the per-column quantization error rather
        // than bit-exactly. For [1,2,3] the step is (3-1)/255 ≈ 0.0078, so the
        // bound is ~0.004 — well under 0.01.
        let recon = r1
            .embeddings
            .first()
            .map(|e| e.values.to_fp32_owned())
            .expect("embedding present");
        let expected = [1.0f32, 2.0, 3.0];
        assert_eq!(recon.len(), expected.len());
        for (got, exp) in recon.iter().zip(expected.iter()) {
            assert!(
                (got - exp).abs() <= 0.01,
                "SQ8 embedding {got} not within 0.01 of {exp}"
            );
        }
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
