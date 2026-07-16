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
use proximadb_block_format::coalesced_rabitq::{RABITQ_SEED_BASE, encode_region};
use proximadb_block_format::{
    BlockCompression, BlockMode, BlockStats, BlockZoneSource, ColumnMeta, FlatRow, PaxBlockReader,
    PaxBlockWriter, RowGroupBlock, VectorQuant, col_id, header::fnv1a_hash,
};
use proximadb_records::{EmbeddingValues, ProximaRecord};
use serde::{Deserialize, Serialize};

use crate::engine_constants::{DEFAULT_TARGET_BLOCK_SIZE_BYTES, MAX_TARGET_BLOCK_SIZE_BYTES};
use crate::segment_layout::{
    FooterBlockEntry, SEG_HEADER_PREFIX_LEN, SEG_LAYOUT_VERSION, SegmentFooterIndex,
    SegmentHeaderPrefix, StatsKind, is_coalesced_segment, segment_tail,
};

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
    /// Serialise to bytes. **Writes are always v2** (TD-167 / ADR-034 P1):
    /// `[n u32] [offset, size, zone(85B)] × n [crc32 u32] [body_len u32] [PAXZ]`.
    /// A block with no computable bounds gets an **empty** zone summary, so EVERY
    /// new segment prunes from the cached index with **zero per-block metadata
    /// GETs** — the legacy v1 layout (`[n][offset,size]×n[crc]`) forced a per-block
    /// footer read for pruning (the depth the co-design exists to collapse).
    ///
    /// The v1 layout is **deprecated for writes** and retained for READS only
    /// (`from_bytes` detects it via the absent `PAXZ` trailer), so existing v1
    /// segments stay readable — mixed-read-safe, no flag-day migration. Per-block
    /// empty-zone overhead (73 B/block) is read once and cached: a worthwhile trade
    /// for unconditional depth-collapse (`DEPTH ≫ BYTES`).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_v2()
    }

    /// Legacy v1 segment-index encoding — **write-deprecated** (kept only to build
    /// v1 fixtures for read mixed-read-safety tests; production writes v2 via
    /// [`Self::to_bytes`]). Real v1 segments on disk are read by `from_bytes`.
    #[cfg(test)]
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

// `SegmentMeta` now lives in `proximadb-block-format` (next to the per-block
// `BlockStats` it aggregates) so segment-level consumers below the storage
// layer — e.g. the catalog's segment registry — can reach it without a
// `catalog -> storage-common` cycle. Re-exported here so the historical
// `proximadb_storage_common::pax_block::SegmentMeta` path keeps working.
pub use proximadb_block_format::SegmentMeta;

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
    /// Opt-in exact-f32 tier (P3 Phase D) — re-applied to every block writer so
    /// each block carries the f32 stripe when enabled.
    f32_tier: bool,
    /// Tier-2 rerank quant (default Sq8) — re-applied to every block writer.
    rerank_quant: VectorQuant,
    /// P-Shred (ADR-055): `(prop_key, user_col_id)` to shred into typed user-columns —
    /// re-applied to every block writer so all blocks in the segment shred uniformly.
    shred_spec: Vec<(String, i32)>,

    current_writer: PaxBlockWriter,
    index: SegmentIndex,
    block_stats: Vec<BlockStats>,
    file_buf: Vec<u8>,
    row_count: u64,

    /// TD-RDSTRAT-5 S1: when true, accumulate each block's centroid (mean of its
    /// embedding-0 f32 vectors) into `block_centroids` as blocks flush. Off by
    /// default — zero cost for callers that don't opt in.
    compute_centroids: bool,
    /// Running sum of the current (not-yet-flushed) block's embedding-0 vectors.
    cur_centroid_sum: Vec<f64>,
    /// Count of vectors summed into `cur_centroid_sum` for the current block.
    cur_centroid_n: u64,
    /// TD-RDSTRAT-5 lever-3: running sum of ‖x‖² over the current block's
    /// embedding-0 vectors. With `cur_centroid_sum` this yields the block's RMS
    /// spread in ONE pass: `radius² = mean(‖x‖²) − ‖centroid‖²` (trace of the
    /// covariance). The read-side prune ranks blocks by the distance lower bound
    /// `d(q,centroid) − k·radius`, so spread-aware blocks aren't wrongly pruned.
    cur_centroid_sumsq: f64,
    /// Finalised per-block centroids, one per flushed block (emission order).
    block_centroids: Vec<Vec<f32>>,
    /// Finalised per-block RMS radius (spread), 1:1 with `block_centroids`
    /// (`0.0` for a block with no Fp32 vector). Empty unless centroids opted in.
    block_radii: Vec<f32>,

    /// ADR-062 / TD-RDSTRAT-6: emit the coalesced-RaBitQ layout — a file-level
    /// header region (cluster-ordered, single segment centroid) + self-describing
    /// footer-index — so the read path scans ALL RaBitQ codes in one GET
    /// (keep=100%, ~0.99 recall) and reranks survivors via coalesced block GETs.
    /// Default OFF; the flush path opts in for RaBitQ-quantized collections. When
    /// on, data blocks are written as `VectorQuant::Sq8` (the survivor-rerank
    /// data) — the unchanged SQ8 decode path reconstructs + reranks them.
    coalesced_rabitq: bool,
    /// Embedding-0 f32 vectors in cluster (add) order, buffered for the
    /// segment-level RaBitQ region. Populated only when `coalesced_rabitq` is on.
    rabitq_vectors: Vec<Option<Vec<f32>>>,
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
            f32_tier: false,
            rerank_quant: VectorQuant::Sq8,
            shred_spec: Vec::new(),
            current_writer: writer,
            index: SegmentIndex { blocks: Vec::new() },
            block_stats: Vec::new(),
            file_buf: Vec::new(),
            row_count: 0,
            compute_centroids: false,
            cur_centroid_sum: Vec::new(),
            cur_centroid_n: 0,
            cur_centroid_sumsq: 0.0,
            block_centroids: Vec::new(),
            block_radii: Vec::new(),
            coalesced_rabitq: false,
            rabitq_vectors: Vec::new(),
        }
    }

    /// TD-RDSTRAT-5 S1: opt in to per-block **centroid** computation. When on, the
    /// writer accumulates each block's embedding-0 f32 mean and returns them in
    /// [`SegmentMeta::block_centroids`] — the vector zone-map the Vector Object
    /// Economy directory prunes on. Default OFF (zero cost otherwise). Builder
    /// form mirroring [`with_quant`]; no block-writer rebuild needed (centroids
    /// are accumulated in the segment writer, orthogonal to block encoding).
    pub fn with_block_centroids(mut self, enabled: bool) -> Self {
        self.compute_centroids = enabled;
        self
    }

    /// Set the vector quantization strategy for this segment (P3 Phase D). Builder form
    /// so existing `new(..)` callers are unchanged; rebuilds the (still-empty) current
    /// block writer so the strategy applies from the first record. `Auto` = env default.
    pub fn with_quant(mut self, quant: VectorQuant) -> Self {
        self.quant = quant;
        self.current_writer = self.fresh_block_writer();
        self
    }

    /// Enable (or disable) the optional exact-f32 tier (P3 Phase D) for every
    /// block in this segment. Builder form mirroring [`with_quant`]; rebuilds the
    /// (still-empty) current block writer so it applies from the first record.
    /// Default OFF; the flush path enables it from the `pax_f32_tier` tag / env.
    pub fn with_f32_tier(mut self, enabled: bool) -> Self {
        self.f32_tier = enabled;
        self.current_writer = self.fresh_block_writer();
        self
    }

    /// Set the tier-2 rerank quantization strategy for every block in this
    /// segment. Default `Sq8` (the validated tier-2); `Fp16` for near-lossless;
    /// `RawF32` for exact. Only used when tier 1 is RaBitQ.
    pub fn with_rerank_quant(mut self, quant: VectorQuant) -> Self {
        self.rerank_quant = quant;
        self.current_writer = self.fresh_block_writer();
        self
    }

    /// P-Shred (ADR-055): shred the given props keys `(prop_key, user_col_id)` into
    /// typed user-columns for every block in this segment. Builder form mirroring
    /// [`with_quant`]; empty ⇒ no shredding (byte-for-byte today's output).
    pub fn with_shred_spec(mut self, spec: Vec<(String, i32)>) -> Self {
        self.shred_spec = spec;
        self.current_writer = self.fresh_block_writer();
        self
    }

    /// ADR-062 / TD-RDSTRAT-6: emit the coalesced-RaBitQ layout. When on, the
    /// RaBitQ binary tier is hoisted into a coalesced file-level header region
    /// (single segment centroid) and data blocks are written as SQ8 (the
    /// survivor-rerank data); the read path scans the region in one GET and
    /// reranks survivors via coalesced block GETs. Builder form mirroring
    /// [`with_quant`]; rebuilds the (still-empty) current block writer so the SQ8
    /// block encoding applies from the first record.
    pub fn with_coalesced_rabitq(mut self, enabled: bool) -> Self {
        self.coalesced_rabitq = enabled;
        self.current_writer = self.fresh_block_writer();
        self
    }

    /// Build a fresh (empty) block writer carrying ALL of this segment's accumulated
    /// settings. Single source of truth so every builder AND every mid-segment block
    /// rotation re-applies the same config (quant, f32 tier, rerank quant, shred spec)
    /// — no per-builder drift. In coalesced mode the block's tier-1 encoding is forced
    /// to SQ8 (the RaBitQ binary tier lives in the header region, not the block).
    fn fresh_block_writer(&self) -> PaxBlockWriter {
        let block_quant = if self.coalesced_rabitq {
            VectorQuant::Sq8
        } else {
            self.quant
        };
        PaxBlockWriter::new(
            self.mode,
            self.compression,
            &self.collection_id,
            self.schema_fingerprint,
            self.embedding_count,
        )
        .with_quant(block_quant)
        .with_f32_tier(self.f32_tier)
        .with_rerank_quant(self.rerank_quant)
        .with_shred_spec(self.shred_spec.clone())
    }

    /// Append a record to the current block.
    ///
    /// Flushes the block automatically when it exceeds `block_size_threshold`.
    pub fn add_record(&mut self, record: &ProximaRecord) -> Result<()> {
        self.current_writer.add_record(record)?;
        self.row_count += 1;
        if self.compute_centroids {
            self.accumulate_centroid(record);
        }
        // ADR-062: buffer the embedding-0 f32 vector (in cluster/add order) for
        // the segment-level RaBitQ region. The caller has already reordered
        // records by `cluster_order_pca_ivf`, so this preserves survivor locality.
        if self.coalesced_rabitq {
            let v = record.embeddings.first().and_then(|e| match &e.values {
                EmbeddingValues::Fp32(v) => Some(v.clone()),
                _ => None,
            });
            self.rabitq_vectors.push(v);
        }

        // Rough size estimate: each record contributes ~1 KB in the worst case.
        // We flush based on row count as a proxy when threshold is not hit yet.
        let approx_bytes = self.current_writer.row_count() * 1024;
        if approx_bytes >= self.block_size_threshold {
            self.flush_current_block()?;
        }
        Ok(())
    }

    /// TD-RDSTRAT-5 S1: fold `record`'s embedding-0 f32 vector into the current
    /// block's running centroid sum. Only the canonical Fp32 write-time
    /// representation contributes; other variants are skipped (the block's
    /// centroid is the mean over its Fp32 rows).
    fn accumulate_centroid(&mut self, record: &ProximaRecord) {
        let Some(EmbeddingValues::Fp32(v)) = record.embeddings.first().map(|e| &e.values) else {
            return;
        };
        if v.is_empty() {
            return;
        }
        if self.cur_centroid_sum.is_empty() {
            self.cur_centroid_sum = vec![0f64; v.len()];
        }
        if self.cur_centroid_sum.len() == v.len() {
            let mut norm_sq = 0f64;
            for (s, &x) in self.cur_centroid_sum.iter_mut().zip(v) {
                let x = x as f64;
                *s += x;
                norm_sq += x * x;
            }
            self.cur_centroid_sumsq += norm_sq;
            self.cur_centroid_n += 1;
        }
    }

    /// Finalise the current block's centroid (mean = sum / n) into
    /// `block_centroids` and reset the accumulator. Pushes an empty centroid when
    /// the block carried no Fp32 vector, so `block_centroids` stays 1:1 with
    /// blocks. No-op unless centroid computation was opted in.
    fn finalize_block_centroid(&mut self) {
        if !self.compute_centroids {
            return;
        }
        let (centroid, radius) = if self.cur_centroid_n > 0 {
            let n = self.cur_centroid_n as f64;
            let centroid: Vec<f32> = self
                .cur_centroid_sum
                .iter()
                .map(|&s| (s / n) as f32)
                .collect();
            // RMS spread in one pass: radius² = mean(‖x‖²) − ‖centroid‖². Clamp at
            // 0 to absorb float error when all rows are identical.
            let centroid_norm_sq: f64 = self
                .cur_centroid_sum
                .iter()
                .map(|&s| (s / n) * (s / n))
                .sum();
            let var = (self.cur_centroid_sumsq / n - centroid_norm_sq).max(0.0);
            (centroid, var.sqrt() as f32)
        } else {
            (Vec::new(), 0.0)
        };
        self.block_centroids.push(centroid);
        self.block_radii.push(radius);
        self.cur_centroid_sum.clear();
        self.cur_centroid_n = 0;
        self.cur_centroid_sumsq = 0.0;
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

        // TD-167 / ADR-034 P1: capture each block's canonical-column bounds (already
        // in hand via the just-opened reader) into the **v2** segment index, so the
        // reader prunes from the cached index with NO per-block metadata GET. Always
        // emitted now — v1 (no zone summaries → per-block footer read to prune) is
        // write-deprecated. A block with no canonical bounds gets an empty summary
        // (it simply doesn't prune — never a per-block read), so the depth-collapse
        // is unconditional.
        let zone = Some(BlockZoneSummary::from_column_metas(
            row_count,
            reader.column_metas(),
        ));
        self.index.blocks.push(BlockIndexEntry {
            offset,
            size: block_size,
            zone,
        });
        self.file_buf.extend_from_slice(&block_bytes);
        self.block_stats.push(stats);
        // Finalise this block's centroid (1:1 with the index entry just pushed).
        self.finalize_block_centroid();

        // Reset writer for the next block (preserving the segment's quant + f32-tier +
        // rerank + shred-spec strategy — see `fresh_block_writer`).
        self.current_writer = self.fresh_block_writer();
        Ok(())
    }

    /// Finalise the segment: flush remaining records, then either append the
    /// legacy `[blocks][index][magic]` tail or — when coalesced-RaBitQ is on —
    /// assemble the ADR-062 layout `[header][RaBitQ region][blocks][footer][tail]`.
    /// Persists to `self.path`; returns `SegmentMeta` for Iceberg manifest use.
    pub fn finish(mut self) -> Result<SegmentMeta> {
        // Flush any remaining rows as the last block
        self.flush_current_block()?;

        if self.index.blocks.is_empty() {
            bail!("segment is empty — nothing to write");
        }

        // Coalesced-RaBitQ requires embedding-0 f32 vectors to build the region.
        // If there are none (a non-vector or malformed batch), fall through to the
        // legacy layout rather than failing the flush — mixed-read-safe.
        let coalesced_dim = self
            .rabitq_vectors
            .iter()
            .flatten()
            .next()
            .map(|v| v.len())
            .unwrap_or(0) as u32;

        if self.coalesced_rabitq && coalesced_dim > 0 {
            self.finish_coalesced(coalesced_dim)
        } else {
            self.finish_legacy()
        }
    }

    /// Legacy `[blocks][SegmentIndex][SEGMENT_MAGIC]` layout (readability-preserving
    /// fallback for non-coalesced writes — the reader detects it via the `PBLK`
    /// head + `PAXSEG01` tail).
    fn finish_legacy(&mut self) -> Result<SegmentMeta> {
        // Append segment index
        let index_bytes = self.index.to_bytes();
        self.file_buf.extend_from_slice(&index_bytes);
        // Append magic
        self.file_buf.extend_from_slice(SEGMENT_MAGIC);

        let total_bytes = self.write_file(&self.file_buf)?;
        Ok(SegmentMeta {
            path: self.path.clone(),
            size_bytes: total_bytes,
            block_count: self.index.blocks.len() as u32,
            row_count: self.row_count,
            block_stats: std::mem::take(&mut self.block_stats),
            block_centroids: std::mem::take(&mut self.block_centroids),
            block_radii: std::mem::take(&mut self.block_radii),
            rabitq_off: 0,
            rabitq_len: 0,
        })
    }

    /// ADR-062 / TD-RDSTRAT-6 coalesced layout: `[HEADER-PREFIX][RaBitQ region]
    /// [blocks][FOOTER-INDEX][footer_len][SEGMENT_MAGIC]`. The RaBitQ region is
    /// one ranged GET (keep=100% scan); the footer-index block table carries
    /// absolute offsets so the read path maps survivors → blocks → coalesced GETs.
    fn finish_coalesced(&mut self, dim: u32) -> Result<SegmentMeta> {
        // 1. Build the coalesced RaBitQ region (single segment centroid) over the
        //    cluster-ordered embedding-0 vectors. `self.file_buf` already holds the
        //    blocks at 0-based offsets (relative to the blocks region).
        let refs: Vec<Option<&[f32]>> = self.rabitq_vectors.iter().map(|o| o.as_deref()).collect();
        let seed = RABITQ_SEED_BASE ^ (col_id::EMBED_BASE as u64);
        let (region_bytes, _centroid) = encode_region(&refs, dim, seed)?;

        let header_len = SEG_HEADER_PREFIX_LEN as u64;
        let rabitq_off = header_len;
        let rabitq_len = region_bytes.len() as u64;
        // Blocks begin right after the header + region.
        let data_offset = header_len + rabitq_len;

        // 2. Footer block table: absolute offsets = data_offset + block's 0-based
        //    offset; row_count from the block's zone summary (1:1 with the index).
        let blocks: Vec<FooterBlockEntry> = self
            .index
            .blocks
            .iter()
            .map(|b| FooterBlockEntry {
                offset: data_offset + b.offset,
                size: b.size,
                row_count: b.zone.as_ref().map(|z| z.row_count).unwrap_or(0),
                stats_kind: StatsKind::None,
            })
            .collect();

        // 3. Footer-index body + header-prefix offsets. The footer sits after the
        //    blocks; its length is known once serialized.
        let footer = SegmentFooterIndex {
            row_count: self.row_count,
            rabitq_off,
            rabitq_len,
            embed_dim: dim,
            embed_count: self.embedding_count as u32,
            embed_quant_tag: 1, // SQ8 (the survivor-rerank data tier)
            has_f32_tier: self.f32_tier,
            blocks,
        };
        let footer_body = footer.to_bytes();
        let footer_off = data_offset + self.file_buf.len() as u64;
        let footer_len = footer_body.len() as u64;

        let header = SegmentHeaderPrefix {
            layout_version: SEG_LAYOUT_VERSION,
            rabitq_off,
            rabitq_len,
            footer_off,
            footer_len,
        };

        // 4. Assemble: header + region + blocks + [footer][footer_len][magic].
        let mut out = Vec::with_capacity(
            SEG_HEADER_PREFIX_LEN
                + region_bytes.len()
                + self.file_buf.len()
                + footer_body.len()
                + 16,
        );
        out.extend_from_slice(&header.to_bytes());
        out.extend_from_slice(&region_bytes);
        out.extend_from_slice(&self.file_buf);
        out.extend(segment_tail(&footer_body));

        let total_bytes = self.write_file(&out)?;
        Ok(SegmentMeta {
            path: self.path.clone(),
            size_bytes: total_bytes,
            block_count: self.index.blocks.len() as u32,
            row_count: self.row_count,
            block_stats: std::mem::take(&mut self.block_stats),
            block_centroids: std::mem::take(&mut self.block_centroids),
            block_radii: std::mem::take(&mut self.block_radii),
            rabitq_off,
            rabitq_len,
        })
    }

    /// Create the parent dir + write `buf` to `self.path`; returns bytes written.
    fn write_file(&self, buf: &[u8]) -> Result<u64> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&self.path)?;
        f.write_all(buf)?;
        Ok(buf.len() as u64)
    }

    /// Test-only: finish writing a **legacy v1** segment (segment index without
    /// inline zone-map summaries). Production always writes v2 ([`Self::finish`]);
    /// this fixture exercises the mixed-read-safe legacy path — a v1 segment on
    /// disk must still read correctly and prune via per-block footer reads.
    #[cfg(test)]
    pub fn finish_v1(mut self) -> Result<SegmentMeta> {
        self.flush_current_block()?;
        if self.index.blocks.is_empty() {
            bail!("segment is empty — nothing to write");
        }
        let index_bytes = self.index.to_bytes_v1();
        self.file_buf.extend_from_slice(&index_bytes);
        self.file_buf.extend_from_slice(SEGMENT_MAGIC);
        let total_bytes = self.file_buf.len() as u64;
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
            block_centroids: self.block_centroids,
            block_radii: self.block_radii,
            rabitq_off: 0,
            rabitq_len: 0,
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
    ///
    /// Mixed-read-safe (ADR-061 amendment): a coalesced-RaBitQ segment
    /// (`SEG_HEADER_MAGIC` head) is parsed from its self-describing footer-index
    /// (the block table there carries absolute offsets); a legacy segment
    /// (`PBLK` head) keeps using `SegmentIndex::locate`. Both feed `next_block`
    /// the same `SegmentIndex` shape.
    pub fn from_bytes(data: Vec<u8>, predicate: ScanPredicate) -> Result<Self> {
        // Validate magic (both layouts tail with SEGMENT_MAGIC).
        if data.len() < 8 || &data[data.len() - 8..] != SEGMENT_MAGIC {
            bail!("not a valid PAX segment file (bad magic)");
        }
        let index = if is_coalesced_segment(&data) {
            Self::parse_coalesced_index(&data)?
        } else {
            let magic_start = data.len() - 8;
            Self::parse_index(&data[..magic_start])?
        };

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

    /// Build the block list for a coalesced-RaBitQ segment from its footer-index.
    /// The footer block table carries absolute offsets + row counts; we mirror
    /// them into a `SegmentIndex` (with a row-count-only zone summary) so the
    /// shared `next_block` / `read_records` paths work unchanged.
    fn parse_coalesced_index(data: &[u8]) -> Result<SegmentIndex> {
        let footer = SegmentFooterIndex::locate_in_segment(data)?
            .ok_or_else(|| anyhow::anyhow!("coalesced segment missing footer-index"))?;
        let blocks = footer
            .blocks
            .iter()
            .map(|b| BlockIndexEntry {
                offset: b.offset,
                size: b.size,
                zone: Some(BlockZoneSummary::empty(b.row_count)),
            })
            .collect();
        Ok(SegmentIndex { blocks })
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
    let preserve_exact = !inputs.is_empty()
        && inputs.iter().all(|input| {
            let Ok(mut scanner) = PaxSegmentScanner::open(input, ScanPredicate::default()) else {
                return false;
            };
            let mut saw_block = false;
            while let Some(block) = scanner.next_block() {
                saw_block = true;
                if !block.has_exact_vector_authority() {
                    return false;
                }
            }
            saw_block
        });
    let mut writer = PaxSegmentWriter::new(
        output,
        mode,
        compression,
        collection_id,
        schema_fingerprint,
        embedding_count,
        None,
    )
    .with_f32_tier(preserve_exact);
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

    /// TD-RDSTRAT-5 S1: with `with_block_centroids(true)`, the segment writer
    /// returns one centroid per block = the exact mean of that block's embedding-0
    /// f32 vectors. Block-cutting is by current-block row count (`row_count*1024 >=
    /// threshold`), so `threshold = 2*1024` puts exactly 2 rows per block.
    #[test]
    fn block_centroids_are_exact_per_block_means() {
        use proximadb_records::{EmbeddingCell, EmbeddingValues};
        let rec = |oid: &str, v: Vec<f32>| {
            let dim = v.len() as u32;
            let mut r = ProximaRecord {
                oid: oid.into(),
                tenant_id: "t".into(),
                created_at_ns: 1,
                updated_at_ns: 1,
                ..Default::default()
            };
            r.embeddings.push(EmbeddingCell {
                modality: "dense".into(),
                dim,
                values: EmbeddingValues::Fp32(v),
                ..Default::default()
            });
            r
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            Some(2 * 1024), // 2 rows/block
        )
        .with_quant(VectorQuant::RawF32)
        .with_block_centroids(true);
        for r in [
            rec("a", vec![0.0, 0.0]),
            rec("b", vec![2.0, 2.0]),
            rec("c", vec![10.0, 10.0]),
            rec("d", vec![12.0, 12.0]),
        ] {
            w.add_record(&r).unwrap();
        }
        let meta = w.finish().unwrap();
        assert_eq!(meta.block_count, 2, "2 rows/block ⇒ 2 blocks");
        assert_eq!(
            meta.block_centroids.len(),
            meta.block_count as usize,
            "one centroid per block"
        );
        assert_eq!(
            meta.block_centroids[0],
            vec![1.0, 1.0],
            "mean of [0,0],[2,2]"
        );
        assert_eq!(
            meta.block_centroids[1],
            vec![11.0, 11.0],
            "mean of [10,10],[12,12]"
        );
    }

    /// TD-WLP-3 (TD-RDSTRAT-5 lever-3): the writer emits one RMS radius per
    /// block, 1:1 with `block_centroids`, computed in ONE pass as
    /// `radius² = mean(‖x‖²) − ‖centroid‖²`. Block [0,0],[2,2]: centroid
    /// [1,1], mean‖x‖² = (0+8)/2 = 4, ‖c‖² = 2 ⇒ radius = √2 (both blocks by
    /// symmetry). A block of identical rows must clamp to exactly 0.0.
    #[test]
    fn block_radii_are_exact_rms_spread() {
        use proximadb_records::{EmbeddingCell, EmbeddingValues};
        let rec = |oid: &str, v: Vec<f32>| {
            let dim = v.len() as u32;
            let mut r = ProximaRecord {
                oid: oid.into(),
                tenant_id: "t".into(),
                created_at_ns: 1,
                updated_at_ns: 1,
                ..Default::default()
            };
            r.embeddings.push(EmbeddingCell {
                modality: "dense".into(),
                dim,
                values: EmbeddingValues::Fp32(v),
                ..Default::default()
            });
            r
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg_radii.pax");
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            Some(2 * 1024), // 2 rows/block
        )
        .with_quant(VectorQuant::RawF32)
        .with_block_centroids(true);
        for r in [
            rec("a", vec![0.0, 0.0]),
            rec("b", vec![2.0, 2.0]),
            rec("c", vec![7.0, 7.0]), // identical pair: zero spread
            rec("d", vec![7.0, 7.0]),
        ] {
            w.add_record(&r).unwrap();
        }
        let meta = w.finish().unwrap();
        assert_eq!(meta.block_count, 2, "2 rows/block ⇒ 2 blocks");
        assert_eq!(
            meta.block_radii.len(),
            meta.block_count as usize,
            "one radius per block, 1:1 with block_centroids"
        );
        let expected = 2f32.sqrt();
        assert!(
            (meta.block_radii[0] - expected).abs() < 1e-6,
            "RMS spread of [0,0],[2,2] must be √2, got {}",
            meta.block_radii[0]
        );
        assert_eq!(
            meta.block_radii[1], 0.0,
            "a block of identical rows must clamp to exactly 0.0"
        );

        // Not opted in ⇒ no radii (parity with block_centroids).
        let path2 = dir.path().join("seg_no_radii.pax");
        let mut w2 = PaxSegmentWriter::new(
            &path2,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            Some(2 * 1024),
        )
        .with_quant(VectorQuant::RawF32);
        w2.add_record(&rec("a", vec![0.0, 0.0])).unwrap();
        w2.add_record(&rec("b", vec![2.0, 2.0])).unwrap();
        let meta2 = w2.finish().unwrap();
        assert!(
            meta2.block_radii.is_empty(),
            "radii must be empty unless centroids are opted in"
        );
    }

    /// TD-RDSTRAT-5 S1: exact per-block means at a realistic **16-dim** width
    /// (2-byte sign-key territory) — 4 rows, 2 rows/block ⇒ 2 blocks, each
    /// centroid the elementwise mean of its block's 16-dim vectors.
    #[test]
    fn block_centroids_16dim_exact_means() {
        use proximadb_records::{EmbeddingCell, EmbeddingValues};
        let dim = 16usize;
        let rec = |oid: &str, fill: f32| {
            let mut r = ProximaRecord {
                oid: oid.into(),
                tenant_id: "t".into(),
                created_at_ns: 1,
                updated_at_ns: 1,
                ..Default::default()
            };
            r.embeddings.push(EmbeddingCell {
                modality: "dense".into(),
                dim: dim as u32,
                values: EmbeddingValues::Fp32(vec![fill; dim]),
                ..Default::default()
            });
            r
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            Some(2 * 1024), // 2 rows/block
        )
        .with_quant(VectorQuant::RawF32)
        .with_block_centroids(true);
        // block 0: fills 0 & 4 → mean 2; block 1: fills 10 & 20 → mean 15.
        for r in [rec("a", 0.0), rec("b", 4.0), rec("c", 10.0), rec("d", 20.0)] {
            w.add_record(&r).unwrap();
        }
        let meta = w.finish().unwrap();
        assert_eq!(meta.block_count, 2);
        assert_eq!(meta.block_centroids.len(), 2);
        assert_eq!(meta.block_centroids[0], vec![2.0f32; dim]);
        assert_eq!(meta.block_centroids[1], vec![15.0f32; dim]);
    }

    /// Default off: a writer built WITHOUT `with_block_centroids` returns no
    /// centroids (zero cost for pre-existing callers).
    #[test]
    fn block_centroids_empty_when_not_opted_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        use proximadb_records::{EmbeddingCell, EmbeddingValues};
        let mut r = make_record("a", "t", 1);
        r.embeddings.push(EmbeddingCell {
            modality: "dense".into(),
            dim: 2,
            values: EmbeddingValues::Fp32(vec![1.0, 2.0]),
            ..Default::default()
        });
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            None,
        )
        .with_quant(VectorQuant::RawF32);
        w.add_record(&r).unwrap();
        let meta = w.finish().unwrap();
        assert!(meta.block_centroids.is_empty());
    }

    #[test]
    fn segment_index_round_trip_is_always_v2_even_without_zones() {
        // Zone-less blocks: writes are now ALWAYS v2 (PAXZ trailer) so pruning
        // needs zero per-block metadata GETs; the blocks get empty zone summaries.
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
        assert_eq!(
            &bytes[bytes.len() - 4..],
            ZONE_INDEX_MARKER,
            "writes are always v2 (PAXZ trailer), even with no zones"
        );
        // `locate` is the production read path — it dispatches v1/v2 via the
        // self-describing trailer (the v1 `from_bytes` would mis-read v2 bytes).
        let idx2 = SegmentIndex::locate(&bytes).unwrap();
        assert_eq!(idx2.blocks.len(), 2);
        assert_eq!(idx2.blocks[0].size, 4096);
        assert_eq!(idx2.blocks[1].offset, 4096);
        // Every block has a zone (empty) ⇒ the reader never falls back to a
        // per-block footer read on a v2 segment.
        assert!(idx2.blocks.iter().all(|b| b.zone.is_some()));
    }

    #[test]
    fn segment_index_v1_legacy_still_reads() {
        // Mixed-read-safety: v1 segments already on disk (no PAXZ trailer) must
        // still read correctly even though writes are now v2-only.
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
        let v1 = idx.to_bytes_v1(); // legacy fixture (write-deprecated)
        assert_ne!(
            &v1[v1.len() - 4..],
            ZONE_INDEX_MARKER,
            "v1 fixture has no PAXZ trailer"
        );
        let read = SegmentIndex::from_bytes(&v1).unwrap();
        assert_eq!(read.blocks.len(), 2);
        assert_eq!(read.blocks[1].offset, 4096);
        // v1 has no zones ⇒ reader prunes via the per-block footer fallback.
        assert!(read.blocks.iter().all(|b| b.zone.is_none()));
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
    fn compaction_preserves_exact_only_when_every_input_has_authority() {
        use proximadb_records::{EmbeddingCell, EmbeddingValues};

        let vector_record = |oid: &str, values: Vec<f32>| {
            let mut record = make_record(oid, "t", 1);
            record.embeddings.push(EmbeddingCell {
                modality: "dense".into(),
                dim: values.len() as u32,
                values: EmbeddingValues::Fp32(values),
                ..Default::default()
            });
            record
        };
        let dir = tempfile::tempdir().unwrap();
        let exact_input = dir.path().join("exact.pax");
        let lossy_input = dir.path().join("lossy.pax");
        let exact_output = dir.path().join("exact-output.pax");
        let mixed_output = dir.path().join("mixed-output.pax");
        let exact_records = vec![
            vector_record("exact-a", vec![-3.25, 0.123_456_7, 8.75]),
            vector_record("exact-b", vec![2.5, 4.765_432, -1.125]),
        ];
        let lossy_records = vec![
            vector_record("lossy-a", vec![11.0, -7.123_456, 0.375]),
            vector_record("lossy-b", vec![6.25, 9.765_432, -4.5]),
        ];

        let mut exact_writer = PaxSegmentWriter::new(
            &exact_input,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            None,
        )
        .with_quant(VectorQuant::RaBitQ)
        .with_f32_tier(true);
        for record in &exact_records {
            exact_writer.add_record(record).unwrap();
        }
        exact_writer.finish().unwrap();

        let mut lossy_writer = PaxSegmentWriter::new(
            &lossy_input,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            None,
        )
        .with_quant(VectorQuant::RaBitQ);
        for record in &lossy_records {
            lossy_writer.add_record(record).unwrap();
        }
        lossy_writer.finish().unwrap();

        compact_pax_segments(
            std::slice::from_ref(&exact_input),
            &exact_output,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            &[],
            &[],
            None,
            i64::MAX,
        )
        .unwrap();
        {
            let mut exact_scanner =
                PaxSegmentScanner::open(&exact_output, ScanPredicate::default()).unwrap();
            let exact_block = exact_scanner.next_block().expect("exact compacted block");
            assert!(exact_block.has_exact_vector_authority());
        }
        let mut exact_scanner =
            PaxSegmentScanner::open(&exact_output, ScanPredicate::default()).unwrap();
        let exact_back = exact_scanner.read_records(&[], &[], None).unwrap();
        for got in &exact_back {
            let want = exact_records
                .iter()
                .find(|record| record.oid == got.oid)
                .unwrap();
            assert_eq!(
                got.embeddings[0].as_fp32_slice(),
                want.embeddings[0].as_fp32_slice()
            );
        }

        compact_pax_segments(
            &[exact_input, lossy_input],
            &mixed_output,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            &[],
            &[],
            None,
            i64::MAX,
        )
        .unwrap();
        let mut mixed_scanner =
            PaxSegmentScanner::open(&mixed_output, ScanPredicate::default()).unwrap();
        let mixed_block = mixed_scanner.next_block().expect("mixed compacted block");
        assert!(
            !mixed_block.has_exact_vector_authority(),
            "a lossy sibling must prevent exact output authority"
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
