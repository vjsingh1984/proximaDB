//! Mixed-format vector-segment read primitives (P3 Phase A).
//!
//! ProximaDB's vector engines (SST/HELIX/SWIFT) historically persist segments as
//! row-based [`ProximaDataBlock`] (raw f32). The RaBitQ path (P3) stores vectors in
//! the columnar PAX segment format, which carries quantized codes and enables ~30×
//! ANN. To migrate additively — mixed-read-safe and default-OFF — a reader must
//! recognise both formats straight from the on-disk bytes.
//!
//! This module is the read-side foundation: a byte-based [`SegmentFormat::detect`]
//! and a [`read_segment_records`] router that decodes either format back to
//! `Vec<ProximaRecord>`, reusing the canonical PAX inverse
//! ([`PaxSegmentScanner::read_records`]). It writes nothing and changes no live read
//! path. The wiring into the cold-search / compaction / recovery sites (Phase A.2)
//! and the PAX write side (Phase B) build on top of this primitive.
//!
//! Detection is unambiguous by magic: a PAX segment starts with the PAX block magic
//! `PBLK` (`0x50…`) and ends with the segment magic `PAXSEG01`; a `ProximaDataBlock`
//! starts with the columnar version byte `0x01` or a compression marker
//! `0x02..=0x0E` — disjoint from `PBLK`, so the legacy path is never mis-routed.

use std::path::Path;

use anyhow::Result;
use proximadb_block_format::{
    BLOCK_MAGIC, BlockCompression, BlockMode, RankMetric, VectorQuant, col_id,
};
use proximadb_records::ProximaRecord;
use proximadb_storage_common::pax_block::{
    PaxSegmentScanner, PaxSegmentWriter, SEGMENT_MAGIC, ScanPredicate, SegmentMeta,
};
use proximadb_storage_common::segment_layout::is_coalesced_segment;

use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

/// On-disk format of a persisted vector segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SegmentFormat {
    /// Legacy row-based block (raw f32). The default for segments written before P3
    /// and for any manifest entry that predates the format field.
    #[default]
    ProximaBlocks,
    /// Columnar PAX segment (carries SQ8 / RaBitQ codes; enables quantized ANN).
    Pax,
}

impl SegmentFormat {
    /// Detect a persisted segment's format from its raw bytes, mixed-read-safe.
    ///
    /// Returns [`SegmentFormat::ProximaBlocks`] for anything not recognisably a PAX
    /// segment, so the legacy path is never mis-routed on truncated or unknown input.
    pub fn detect(bytes: &[u8]) -> Self {
        if is_pax_segment(bytes) {
            SegmentFormat::Pax
        } else {
            SegmentFormat::ProximaBlocks
        }
    }
}

/// True iff `bytes` is a PAX segment: a recognised PAX head AND the trailing
/// segment magic. Both ends are checked so a stray prefix or suffix alone can't
/// false-positive. The head is EITHER the legacy block magic `PBLK` (block 0 at
/// offset 0) OR the coalesced-RaBitQ header magic (ADR-062) — both tail with
/// `SEGMENT_MAGIC`, so the trailing check is the common anchor.
fn is_pax_segment(bytes: &[u8]) -> bool {
    bytes.len() >= BLOCK_MAGIC.len() + SEGMENT_MAGIC.len()
        && (bytes.starts_with(&BLOCK_MAGIC) || is_coalesced_segment(bytes))
        && bytes.ends_with(SEGMENT_MAGIC)
}

/// Decode a persisted vector segment back to records, routing on the detected format.
///
/// PAX segments are decoded via the canonical inverse
/// ([`PaxSegmentScanner::read_records`]); legacy segments via
/// [`ProximaDataBlock::deserialize`]. This is the single mixed-read entry the
/// Phase A.2 wiring will call from the cold-search, compaction, and recovery paths.
///
/// `embedding_model_ids` / `user_column_keys` are the collection's schema keys used
/// to reconstruct PAX records positionally (empty slices = best-effort defaults);
/// they are ignored for the legacy format, which is self-describing.
///
/// `tenant_ctx` is the segment's owning tenant (from the catalog/path); it is
/// stamped onto rows whose tenant column was dropped (catalog-resolution) and is
/// ignored when the column is present. `None` keeps stored values verbatim.
pub fn read_segment_records(
    bytes: &[u8],
    embedding_model_ids: &[String],
    user_column_keys: &[String],
    tenant_ctx: Option<&str>,
) -> Result<Vec<ProximaRecord>> {
    match SegmentFormat::detect(bytes) {
        SegmentFormat::Pax => {
            let mut scanner =
                PaxSegmentScanner::from_bytes(bytes.to_vec(), ScanPredicate::default())?;
            scanner.read_records(embedding_model_ids, user_column_keys, tenant_ctx)
        }
        SegmentFormat::ProximaBlocks => Ok(ProximaDataBlock::deserialize(bytes, None)?.records),
    }
}

/// Write `records` as a PAX vector segment at `path` — the write-side inverse of
/// [`read_segment_records`] for the PAX format, via the canonical [`PaxSegmentWriter`]
/// (no hand-rolled encoder, per the storage-format-migration mandate). Phase B's
/// flag-gated SST flush arm calls this against the local staging path; the resulting
/// file is detected as [`SegmentFormat::Pax`] and reads back through the same router.
///
/// `embedding_count` is the collection's embedding-modality count (≥1). `quant` selects
/// the vector quantization strategy (P3 Phase D): `VectorQuant::Auto` keeps the env
/// default (SQ8 unless `PROXIMADB_VECTOR_RABITQ`), `RaBitQ` writes ~30× binary codes.
///
/// `target_block` is the optional target block size in bytes (TD-156 / ADR-026
/// configurable geometry). `None` keeps the writer default; a larger value (e.g.
/// 8-16 MiB for object storage) coalesces rows into fewer blocks, cutting the
/// per-block ranged-GET count — the fragmentation lever measured by the
/// footer-cache economics harness. Mixed-read-safe: block size is a per-segment
/// write choice and is irrelevant to the magic-detected read router.
pub fn write_pax_segment(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
    target_block: Option<usize>,
) -> Result<SegmentMeta> {
    write_pax_segment_with_f32_tier(
        path,
        records,
        collection_id,
        embedding_count,
        quant,
        false,
        target_block,
    )
}

/// Like [`write_pax_segment`] but optionally emits the exact-f32 tier (P3 Phase
/// D). When `f32_tier` is true, each embedding also gets a co-located raw-f32
/// stripe at `col_id::F32_TIER_BASE + i` for an exact
/// final rerank / `include_vectors`. The flush path calls this with the resolved
/// `pax_f32_tier` opt-in; compaction/tests use [`write_pax_segment`] (no tier).
pub fn write_pax_segment_with_f32_tier(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
    f32_tier: bool,
    target_block: Option<usize>,
) -> Result<SegmentMeta> {
    write_pax_segment_full(
        path,
        records,
        collection_id,
        embedding_count,
        quant,
        VectorQuant::Sq8,
        f32_tier,
        target_block,
    )
}

/// Full PAX segment write with configurable tier-1 quant, tier-2 rerank quant,
/// and optional f32 tier. This is the canonical write entry point; the legacy
/// `write_pax_segment_with_f32_tier` delegates with `Sq8` rerank (the default).
#[allow(clippy::too_many_arguments)]
pub fn write_pax_segment_full(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
    rerank_quant: VectorQuant,
    f32_tier: bool,
    target_block: Option<usize>,
) -> Result<SegmentMeta> {
    // TD-RDSTRAT-5 S1 / TD-WLP-4 (default ON, kill-switch
    // `PROXIMADB_PAX_BLOCK_CLUSTER=0`): reorder records by the model-free
    // sign-code bootstrap so spatially-close vectors co-locate into the same
    // block, and compute each block's centroid+radius for the Vector Object
    // Economy directory. Reordering is result-preserving (the reader
    // ranks/dedups by distance + OID). Compaction upgrades the ordering to
    // PCA+IVF via `write_pax_segment_compacted`.
    let cluster = crate::storage::engines::sst::block_cluster::block_cluster_enabled();
    let order = if cluster {
        // TD-WLP-4/WLP-9 eval opt-in (`PROXIMADB_PAX_FLUSH_CLUSTER=ivf`): apply
        // the compaction-grade PCA+IVF re-cluster at flush instead of the sign-bit
        // bootstrap, so clustering quality is measurable without the (unwired)
        // flush→compaction scheduler. Default OFF ⇒ bootstrap.
        if crate::storage::engines::sst::block_cluster::flush_cluster_ivf() {
            crate::storage::engines::sst::block_cluster::cluster_order_pca_ivf(records, 0)
        } else {
            crate::storage::engines::sst::block_cluster::cluster_order(records, 0)
        }
    } else {
        None
    };
    write_pax_segment_ordered(
        path,
        records,
        collection_id,
        embedding_count,
        quant,
        rerank_quant,
        f32_tier,
        target_block,
        cluster,
        order,
    )
}

/// TD-WLP-4 (ADR-061 D3): the **compaction** write entry point — identical to
/// [`write_pax_segment_full`] except the record ordering is the PCA+IVF
/// re-cluster (`cluster_order_pca_ivf`) instead of the L0 sign-code bootstrap.
/// Compaction is the re-cluster event: the merged batch is large enough to
/// train a write-time PCA, and same-cell rows co-locate into blocks whose
/// centroids+radii make the read-side prune effective. Honors the same
/// kill-switch (`PROXIMADB_PAX_BLOCK_CLUSTER=0` ⇒ insertion order, no
/// centroids). Gating by profile happens at compaction *arming*
/// (`resolve_compaction_armed`): Churn collections never schedule compaction,
/// so this path only serves AppendBulk (or explicit `compaction:on`) work.
#[allow(clippy::too_many_arguments)]
pub fn write_pax_segment_compacted(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
    rerank_quant: VectorQuant,
    f32_tier: bool,
    target_block: Option<usize>,
) -> Result<SegmentMeta> {
    let cluster = crate::storage::engines::sst::block_cluster::block_cluster_enabled();
    let order = if cluster {
        crate::storage::engines::sst::block_cluster::cluster_order_pca_ivf(records, 0)
    } else {
        None
    };
    write_pax_segment_ordered(
        path,
        records,
        collection_id,
        embedding_count,
        quant,
        rerank_quant,
        f32_tier,
        target_block,
        cluster,
        order,
    )
}

/// Shared writer loop for the flush (bootstrap-ordered) and compaction
/// (IVF-ordered) entry points.
#[allow(clippy::too_many_arguments)]
fn write_pax_segment_ordered(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
    rerank_quant: VectorQuant,
    f32_tier: bool,
    target_block: Option<usize>,
    cluster: bool,
    order: Option<Vec<usize>>,
) -> Result<SegmentMeta> {
    let mut writer = PaxSegmentWriter::new(
        path,
        BlockMode::Pax,
        BlockCompression::None,
        collection_id,
        0, // schema_fingerprint — derived from the catalog schema in a later phase
        embedding_count.max(1),
        target_block,
    )
    .with_quant(quant)
    .with_f32_tier(f32_tier)
    .with_rerank_quant(rerank_quant)
    .with_block_centroids(cluster)
    // ADR-062 / TD-RDSTRAT-6: hoist the RaBitQ binary tier into a coalesced
    // file-level header region for RaBitQ-quantized writes (default ON per the
    // ADR-061 pre-GA in-place amendment; `PROXIMADB_PAX_COALESCED_RABITQ=0` opts
    // out to the legacy in-block RaBitQ layout for mixed-read / measurement).
    .with_coalesced_rabitq(quant == VectorQuant::RaBitQ && coalesced_rabitq_enabled());
    match &order {
        Some(perm) => {
            for &i in perm {
                writer.add_record(&records[i])?;
            }
        }
        None => {
            for record in records {
                writer.add_record(record)?;
            }
        }
    }
    writer.finish()
}

/// True only when every vector row in every input `.pax` segment has an exact
/// f32 authority. Compaction uses this to decide whether its rewritten RaBitQ
/// output may retain an exact tier. A raw-f32 `EMBED_BASE` is authoritative;
/// otherwise every non-null base vector must have a matching raw
/// `F32_TIER_BASE` row. One exact input never upgrades lossy siblings.
///
/// Legacy ProximaBlocks (`.sst` or no extension) are exact authorities. Other
/// physical formats are not assumed exact without an explicit contract. At
/// least one PAX input must request/preserve exactness; this keeps the optional
/// tier default-OFF for legacy-only compactions. Read/parse/decode failures fail
/// closed to `false`.
pub fn pax_inputs_have_f32_tier(input_files: &[std::path::PathBuf]) -> bool {
    let mut saw_pax = false;
    for f in input_files {
        match f.extension().and_then(|e| e.to_str()) {
            Some("pax") => {}
            Some("sst") | None => continue,
            Some(_) => return false,
        }
        saw_pax = true;
        let Ok(bytes) = std::fs::read(f) else {
            return false;
        };
        let Ok(mut scanner) = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()) else {
            return false;
        };

        let mut saw_block = false;
        while let Some(reader) = scanner.next_block() {
            saw_block = true;
            if !reader.has_exact_vector_authority() {
                return false;
            }
        }
        if !saw_block {
            return false;
        }
    }
    saw_pax
}

/// True iff `bytes` is a `.pax` SEGMENT whose `EMBED_BASE` column is RaBitQ-coded
/// AND no `F32_TIER_BASE` column is present — i.e. the segment carries NO exact
/// vectors, so an AXIS index rebuilt from it (via [`read_segment_records`] →
/// `decode_rabitq_reconstruct`) would be COARSE (low recall). The reopen-rebuild
/// path (`ensure_axis_index_from_sst`) uses this to SKIP rebuilding a lossy AXIS
/// for default RaBitQ-PAX collections, so cold reads fall through to the RaBitQ
/// cascade (`try_pax_cascade`, which ranks the RaBitQ codes properly, ~0.93
/// recall) instead of a coarse rebuilt index (~0.46). Returns `false` for non-Pax,
/// non-RaBitQ quants (RawF32/SQ8/FP16 = exact/near-exact → rebuild), RaBitQ WITH
/// an f32 tier (exact via the tier → rebuild), or any parse error (→ rebuild).
///
/// Pure over `bytes` so it's unit-testable; the engine reads bytes via the
/// filesystem trait (object-store-safe) and calls this. Mirrors
/// [`pax_inputs_have_f32_tier`]'s `PaxSegmentScanner` block-0 metadata read.
pub fn pax_segment_is_coarse_rabitq_without_f32_tier(bytes: &[u8]) -> bool {
    use proximadb_block_format::vparam::QUANT_RABITQ;
    if SegmentFormat::detect(bytes) != SegmentFormat::Pax {
        return false;
    }
    // A `.pax` SEGMENT is `[block(s)][segment_index][SEGMENT_MAGIC]`; the scanner
    // parses the trailing index + magic and yields block 0 (see
    // [`pax_inputs_have_f32_tier`]).
    let Ok(mut scanner) = PaxSegmentScanner::from_bytes(bytes.to_vec(), ScanPredicate::default())
    else {
        return false;
    };
    let Some(reader) = scanner.next_block() else {
        return false;
    };
    let vparams = reader.vector_params();
    let Some(embed) = vparams.get(col_id::EMBED_BASE) else {
        return false;
    };
    embed.quant_kind == QUANT_RABITQ && vparams.get(col_id::F32_TIER_BASE).is_none()
}

/// Detect the tier-2 rerank quant strategy of the source `.pax` segments, so
/// compaction PRESERVES it when re-encoding (mirrors [`pax_inputs_have_f32_tier`]
/// for the f32 tier). Returns the first detected strategy from the co-located
/// `RERANK_BASE` column; defaults to [`VectorQuant::Sq8`] (the validated
/// tier-2) when no `.pax` input carries a rerank column. Correct for both env-
/// and tag-opt-in: the source segment reflects whatever the collection wrote,
/// so preserving it keeps a configured FP16/f32 rerank from being silently
/// downgraded to SQ8 on the first compaction.
pub fn pax_inputs_rerank_quant(
    input_files: &[std::path::PathBuf],
) -> proximadb_block_format::VectorQuant {
    use proximadb_block_format::vparam::{QUANT_FP16, QUANT_RAW_F32};
    for f in input_files {
        if f.extension().and_then(|e| e.to_str()) != Some("pax") {
            continue;
        }
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        // Read block 0's column metadata via the segment scanner — a `.pax`
        // SEGMENT file is `[block(s)][segment_index][SEGMENT_MAGIC]`, so opening
        // the whole file as a single block misreads the footer (see
        // [`pax_inputs_have_f32_tier`]). The rerank column is co-located in every
        // block, so block 0 suffices to detect the segment's tier-2 strategy.
        let Ok(mut scanner) = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()) else {
            continue;
        };
        let Some(reader) = scanner.next_block() else {
            continue;
        };
        if let Some(entry) = reader.vector_params().get(col_id::RERANK_BASE) {
            return match entry.quant_kind {
                QUANT_FP16 => VectorQuant::Fp16,
                QUANT_RAW_F32 => VectorQuant::RawF32,
                // SQ8 (the default) or any unknown/reserved kind → Sq8.
                _ => VectorQuant::Sq8,
            };
        }
    }
    VectorQuant::Sq8
}

/// One scored hit from the coalesced RaBitQ segment scan, reranked against the
/// co-located SQ8 column. `distance` carries the canonical public metric value:
/// Euclidean distance, cosine distance, or positive dot-product similarity.
#[derive(Debug, Clone)]
pub struct CascadeHit {
    pub oid: String,
    pub distance: f32,
    /// Exact f32 vector (from the opt-in f32 tier) for `include_vectors`
    /// materialization. `None` when no f32 tier is present (the caller gets
    /// id+score only, as before). Populated lazily for the top-k rows only.
    pub vector: Option<Vec<f32>>,
}

/// Canonical resolver from a filter field name to its PAX column id for
/// block/row-group pruning — the single mapper shared by every PAX prune path
/// (e.g. the relational ranged `read_records_pruned` scan). Only the fixed
/// canonical columns carry zone-map stripes; user metadata lives in opaque
/// `props` (not prunable), so unknown fields return `None` and the pruner
/// conservatively keeps the block (no false negatives).
///
/// A plain `fn` (not a closure) so it coerces to both the field→column-mapper
/// trait object form and the `Sync` form the object-storage ranged path requires.
pub(crate) fn pax_field_to_col(field: &str) -> Option<i32> {
    match field {
        "id" | "oid" => Some(col_id::OID),
        "tenant_id" => Some(col_id::TENANT_ID),
        "created_at" | "created_at_ns" => Some(col_id::CREATED_AT),
        "updated_at" | "updated_at_ns" => Some(col_id::UPDATED_AT),
        "valid_from" | "valid_from_ns" => Some(col_id::VALID_FROM),
        "valid_to" | "valid_to_ns" => Some(col_id::VALID_TO),
        _ => None,
    }
}

/// Whether the coalesced-RaBitQ layout is engaged for new RaBitQ writes
/// (ADR-062 / TD-RDSTRAT-6). **Default ON** — coalesced scan-then-rerank is the
/// canonical PAX RaBitQ path (pre-GA: no serialized legacy data, so no back-compat
/// — per the ADR-061 in-place amendment). The reader handles BOTH layouts via the
/// `SEG_HEADER_MAGIC` presence-field. `PROXIMADB_PAX_COALESCED_RABITQ=0|off|false`
/// is an emergency kill-switch back to the legacy in-block RaBitQ layout.
pub fn coalesced_rabitq_enabled() -> bool {
    !matches!(
        std::env::var("PROXIMADB_PAX_COALESCED_RABITQ")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// Parse a `u64` env override, or return `default`. Used for the coalesced
/// read-path tuning knobs (survivor coalesce gap/range, pool mult/min/rate).
fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// A coalesced ranged GET over one or more survivor data blocks.
struct CoalescedFetch {
    start: u64,
    end: u64,
    blocks: Vec<usize>,
}

/// Plan coalesced ranged GETs over a set of survivor block indices using the
/// shared `ObjectRangeCoalescePolicy` thresholds (ADR-062 D3 — the GET win is the
/// *policy* planning merged ranges; `FileSystem::read_ranges` only executes them).
/// Adjacent blocks within `max_gap_bytes` (and under `max_range_bytes`) merge into
/// one GET, so cluster-adjacent survivors fetch in a handful of coalesced reads.
fn plan_coalesced_block_ranges(
    footer: &proximadb_storage_common::segment_layout::SegmentFooterIndex,
    block_indices: &[usize],
    policy: &crate::storage::engines::sst::readers::sst_query_engine::ObjectRangeCoalescePolicy,
) -> Vec<CoalescedFetch> {
    let mut sorted: Vec<usize> = block_indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out: Vec<CoalescedFetch> = Vec::new();
    for bi in sorted {
        let Some(b) = footer.blocks.get(bi) else {
            continue;
        };
        let start = b.offset;
        let end = b.offset + b.size as u64;
        let merge = match out.last_mut() {
            Some(last) => {
                let gap = start.saturating_sub(last.end);
                let merged_len = end - last.start;
                let within_gap = gap <= policy.max_gap_bytes;
                let within_max =
                    policy.max_range_bytes == 0 || merged_len <= policy.max_range_bytes;
                if within_gap && within_max {
                    last.end = last.end.max(end);
                    last.blocks.push(bi);
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if !merge {
            out.push(CoalescedFetch {
                start,
                end,
                blocks: vec![bi],
            });
        }
    }
    out
}

/// Per-metric "lower = nearer" rerank score against a reconstructed vector.
fn rerank_distance(metric: RankMetric, q: &[f32], v: &[f32]) -> f32 {
    match metric {
        RankMetric::L2 => q.iter().zip(v).map(|(a, b)| (a - b) * (a - b)).sum(),
        RankMetric::Cosine => {
            let dot: f32 = q.iter().zip(v).map(|(a, b)| a * b).sum();
            let nq: f32 = q.iter().map(|a| a * a).sum::<f32>().sqrt();
            let nv: f32 = v.iter().map(|a| a * a).sum::<f32>().sqrt();
            1.0 - dot / (nq * nv + 1e-12)
        }
        RankMetric::DotProduct => -q.iter().zip(v).map(|(a, b)| a * b).sum::<f32>(),
    }
}

/// Convert the coalesced reranker's lower-is-better ordering score into the
/// canonical metric value consumed by the public score conversion. This is a
/// one-way boundary: sorting happens before it, and callers must not negate or
/// square-root the value again.
fn canonical_score_from_rank_score(metric: RankMetric, rank_score: f32) -> f32 {
    match metric {
        RankMetric::L2 => rank_score.max(0.0).sqrt(),
        RankMetric::Cosine => rank_score,
        RankMetric::DotProduct => -rank_score,
    }
}

/// Adaptive RaBitQ candidate-pool size for top-`k` over a segment of `n` rows.
///
/// `M = max(k · mult, ceil(n · rate), min)` — the survivor pool scales with the
/// segment size so it stays a roughly constant *fraction* of the corpus (rate,
/// default 1%) instead of a fixed 1000 that starves recall at scale (0.1% of 1M
/// → measured recall 0.968; 1% of 100k → 0.991). Env-overridable:
/// `PROXIMADB_PAX_RABITQ_POOL_MULT` (default 100), `..._MIN` (1000),
/// `PROXIMADB_PAX_RABITQ_POOL_RATE` (default 0.01).
pub fn pax_rabitq_pool_for_top_k(k: usize, n: usize) -> usize {
    static C: std::sync::OnceLock<(usize, usize, f64)> = std::sync::OnceLock::new();
    let (mult, min, rate) = C.get_or_init(|| {
        let mult = std::env::var("PROXIMADB_PAX_RABITQ_POOL_MULT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100);
        let min = std::env::var("PROXIMADB_PAX_RABITQ_POOL_MIN")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1000);
        let rate = std::env::var("PROXIMADB_PAX_RABITQ_POOL_RATE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|f| *f > 0.0)
            .unwrap_or(0.01);
        (mult, min, rate)
    });
    let by_k = k.saturating_mul(*mult);
    let by_n = ((n as f64) * rate).ceil() as usize;
    by_k.max(by_n).max(*min)
}

/// ADR-062 / TD-RDSTRAT-6 **scan-then-rerank** over a coalesced-RaBitQ segment
/// on the engine's own filesystem — the ranged analogue of
/// [`rabitq_search_segment`] for the new layout:
///
/// 1. **Scan** the coalesced RaBitQ header region (one ranged GET, header-prefix
///    coalesced in) — `keep=100%`: rank ALL codes → approximate distance for
///    every vector (zero prune loss).
/// 2. **Select** the top-M survivors (M = `pax_rabitq_pool_for_top_k(k, n)`,
///    scaled with the segment's row count so it stays ~1% of the corpus).
/// 3. **Rerank** survivors: because the segment is cluster-ordered
///    (`cluster_order_pca_ivf`), survivors fall in few adjacent blocks → the
///    `ObjectRangeCoalescePolicy` plans merged/coalesced ranged GETs → fetch the
///    survivor blocks, decode the UNCHANGED SQ8 `EMBED_BASE` stripe, score.
/// 4. **Finalize** top-k.
///
/// Net: ~1 RaBitQ GET + 1 footer GET + a few coalesced survivor GETs — vs the
/// legacy per-block cascade's ~4-5 GETs × every selected block. Returns
/// `Ok(None)` when `path` is not a coalesced segment (the caller falls back to
/// the legacy in-block RaBitQ path); physical bytes/GETs are traced by the fs
/// layer's `read_range` calls.
pub async fn rabitq_search_segment_coalesced(
    fs: &dyn proximadb_storage_filesystem_types::FileSystem,
    path: &str,
    query: &[f32],
    k: usize,
    metric: RankMetric,
) -> Result<Option<Vec<CascadeHit>>> {
    use proximadb_block_format::{PaxBlockReader, RaBitQRegion, col_id};
    use proximadb_storage_common::segment_layout::{
        SEG_HEADER_PREFIX_LEN, SegmentFooterIndex, SegmentHeaderPrefix,
    };

    let size = fs
        .metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!("coalesced scan stat {path}: {e}"))?
        .size;
    if size < (SEG_HEADER_PREFIX_LEN as u64 + SEGMENT_MAGIC.len() as u64) {
        return Ok(None);
    }

    // 1. Header-prefix → RaBitQ region + footer extents (one tiny GET). A bad
    //    magic means this is a legacy segment → caller falls back.
    let header_bytes = fs
        .read_range(path, 0, SEG_HEADER_PREFIX_LEN as u64)
        .await
        .map_err(|e| anyhow::anyhow!("coalesced scan header {path}: {e}"))?;
    let header = match SegmentHeaderPrefix::parse(&header_bytes) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };

    // 2. Scan the RaBitQ region (one GET) + rank ALL codes → top-M survivors.
    let region_bytes = fs
        .read_range(path, header.rabitq_off, header.rabitq_len)
        .await
        .map_err(|e| anyhow::anyhow!("coalesced scan region {path}: {e}"))?;
    let region = RaBitQRegion::from_bytes(&region_bytes)?;
    // ADR-062 PR2: adaptive survivor pool — scale M with the segment's row count
    // (region.n_rows()) so it stays ~1% of the corpus instead of a fixed 1000
    // that starves recall at scale (0.1% of 1M → 0.968 recall).
    let pool = pax_rabitq_pool_for_top_k(k, region.n_rows());
    let survivors = region.rank(query, metric, pool.max(k));
    if survivors.is_empty() {
        return Ok(Some(Vec::new()));
    }

    // 3. Footer-index → block table (absolute offsets + per-block row counts).
    let footer_bytes = fs
        .read_range(path, header.footer_off, header.footer_len)
        .await
        .map_err(|e| anyhow::anyhow!("coalesced scan footer {path}: {e}"))?;
    let footer = SegmentFooterIndex::parse(&footer_bytes)?;

    // 4. Map survivor global rows → blocks via cumulative row counts; collect the
    //    per-block local row indices that need SQ8 rerank.
    let mut block_start: Vec<u64> = Vec::with_capacity(footer.blocks.len());
    let mut acc = 0u64;
    for b in &footer.blocks {
        block_start.push(acc);
        acc += b.row_count as u64;
    }
    let mut block_rows: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for &g in &survivors {
        if block_start.is_empty() {
            break;
        }
        let bi = match block_start.binary_search(&(g as u64)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let local = g as u64 - block_start[bi];
        block_rows.entry(bi).or_default().push(local as usize);
    }
    if block_rows.is_empty() {
        return Ok(Some(Vec::new()));
    }

    // 5. Plan coalesced ranged GETs over the survivor blocks. PR2: aggressive
    //    coalescing — bytes are ~free on same-AZ object storage, so merge survivor
    //    blocks across larger gaps to cut the GET count (the dominant cost term).
    //    Env-overridable; defaults merge adjacent blocks within 256 KiB gaps up to
    //    16 MiB per coalesced range (vs the 64 KiB / 8 MiB cross-block default).
    let policy =
        crate::storage::engines::sst::readers::sst_query_engine::ObjectRangeCoalescePolicy {
            max_gap_bytes: env_u64_or("PROXIMADB_PAX_COALESCE_GAP", 256 * 1024),
            max_range_bytes: env_u64_or("PROXIMADB_PAX_COALESCE_RANGE", 16 * 1024 * 1024),
        };
    let survivor_blocks: Vec<usize> = block_rows.keys().copied().collect();
    let fetches = plan_coalesced_block_ranges(&footer, &survivor_blocks, &policy);

    // 6. Fetch each coalesced range, decode the survivor blocks' SQ8 EMBED_BASE
    //    stripe (UNCHANGED block decoder), rerank the survivor rows, attach OIDs.
    let mut hits: Vec<CascadeHit> = Vec::with_capacity(survivors.len());
    for fetch in &fetches {
        let buf = fs
            .read_range(path, fetch.start, fetch.end - fetch.start)
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan block fetch {path}: {e}"))?;
        for &bi in &fetch.blocks {
            let Some(b) = footer.blocks.get(bi) else {
                continue;
            };
            let rel = match b.offset.checked_sub(fetch.start) {
                Some(r) => r as usize,
                None => continue,
            };
            let end = rel + b.size as usize;
            if end > buf.len() {
                continue;
            }
            let block_bytes = &buf[rel..end];
            let Ok(reader) = PaxBlockReader::open(block_bytes) else {
                continue;
            };
            let Some(vecs) = reader.decode_f32_vec_stripe(col_id::EMBED_BASE) else {
                continue;
            };
            let oids = reader.decode_str_stripe(col_id::OID).unwrap_or_default();
            if let Some(rows) = block_rows.get(&bi) {
                for &local in rows {
                    if let Some(Some(v)) = vecs.get(local) {
                        hits.push(CascadeHit {
                            oid: oids.get(local).cloned().flatten().unwrap_or_default(),
                            distance: rerank_distance(metric, query, v),
                            vector: None,
                        });
                    }
                }
            }
        }
    }

    // 7. Finalize global top-k (nearest-first).
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    for hit in &mut hits {
        hit.distance = canonical_score_from_rank_score(metric, hit.distance);
    }
    Ok(Some(hits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::proximablocks::BlockCompressionConfig;
    use proximadb_records::{EmbeddingCell, EmbeddingValues};

    fn rec(oid: &str, ts: i64, vec: Vec<f32>) -> ProximaRecord {
        let mut r = ProximaRecord {
            oid: oid.into(),
            tenant_id: "t".into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        };
        let dim = vec.len();
        r.embeddings.push(EmbeddingCell {
            modality: "dense".into(),
            dim: dim as u32,
            values: EmbeddingValues::Fp32(vec),
            ..Default::default()
        });
        r
    }

    /// A PAX segment written by `PaxSegmentWriter` is detected as `Pax` and round-trips
    /// back to the same records through the format-routing reader.
    #[test]
    fn pax_segment_detected_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");

        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1, // embedding_count
            None,
        );
        w.add_record(&rec("a", 1_700_000_000_000_000_000, vec![1.0, 2.0, 3.0]))
            .unwrap();
        w.add_record(&rec("b", 1_700_000_000_000_000_001, vec![4.0, 5.0, 6.0]))
            .unwrap();
        w.finish().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            SegmentFormat::detect(&bytes),
            SegmentFormat::Pax,
            "PAX segment must be detected as Pax"
        );

        let records = read_segment_records(&bytes, &[], &[], None).unwrap();
        assert_eq!(records.len(), 2);
        let mut oids: Vec<&str> = records.iter().map(|r| r.oid.as_str()).collect();
        oids.sort_unstable();
        assert_eq!(oids, vec!["a", "b"]);
    }

    /// A legacy `ProximaDataBlock` is detected as `ProximaBlocks` and round-trips back
    /// to the same records — proving the router preserves the existing path.
    #[test]
    fn proxima_block_detected_and_read_back() {
        let records = vec![
            rec("x", 1_700_000_000_000_000_000, vec![1.0, 2.0, 3.0, 4.0]),
            rec("y", 1_700_000_000_000_000_001, vec![5.0, 6.0, 7.0, 8.0]),
        ];
        let bytes = ProximaDataBlock::new(records, BlockCompressionConfig::default())
            .serialize()
            .unwrap();

        assert_eq!(
            SegmentFormat::detect(&bytes),
            SegmentFormat::ProximaBlocks,
            "ProximaDataBlock must be detected as ProximaBlocks"
        );

        let back = read_segment_records(&bytes, &[], &[], None).unwrap();
        let mut oids: Vec<&str> = back.iter().map(|r| r.oid.as_str()).collect();
        oids.sort_unstable();
        assert_eq!(oids, vec!["x", "y"]);
    }

    /// Detection never mis-routes the legacy path: version/compression-marker first
    /// bytes, empty input, and a PBLK prefix without the trailing segment magic all
    /// resolve to `ProximaBlocks`.
    #[test]
    fn detection_defaults_to_legacy_and_guards_false_positives() {
        assert_eq!(SegmentFormat::detect(&[]), SegmentFormat::ProximaBlocks);
        assert_eq!(
            SegmentFormat::detect(&[0x01, 0, 0]),
            SegmentFormat::ProximaBlocks
        );
        assert_eq!(
            SegmentFormat::detect(&[0x05, 0, 0]),
            SegmentFormat::ProximaBlocks
        );
        // Leading PBLK but no trailing PAXSEG01 → not a PAX segment.
        let mut faux = BLOCK_MAGIC.to_vec();
        faux.extend_from_slice(&[0u8; 16]);
        assert_eq!(SegmentFormat::detect(&faux), SegmentFormat::ProximaBlocks);
    }

    /// The default format is the legacy one (serde-absent manifests recover unchanged).
    #[test]
    fn default_is_proxima_blocks() {
        assert_eq!(SegmentFormat::default(), SegmentFormat::ProximaBlocks);
    }

    /// `write_pax_segment` is the write-side inverse: what it writes is detected as PAX
    /// and reads back through `read_segment_records` (the Phase B flush↔read round-trip,
    /// exercised without the flush harness).
    #[test]
    fn write_pax_segment_round_trips_through_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("written.pax");
        let records = vec![
            rec("w1", 1_700_000_000_000_000_000, vec![1.0, 2.0, 3.0]),
            rec("w2", 1_700_000_000_000_000_001, vec![4.0, 5.0, 6.0]),
        ];
        let meta = write_pax_segment(&path, &records, "col", 1, VectorQuant::Auto, None).unwrap();
        assert!(
            meta.block_count >= 1,
            "segment should have at least one block"
        );

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(SegmentFormat::detect(&bytes), SegmentFormat::Pax);
        let back = read_segment_records(&bytes, &[], &[], None).unwrap();
        let mut oids: Vec<&str> = back.iter().map(|r| r.oid.as_str()).collect();
        oids.sort_unstable();
        assert_eq!(oids, vec!["w1", "w2"]);
    }

    /// M1 (ADR-049): a `VectorQuant::RawF32` PAX segment round-trips to EXACT f32
    /// vectors (recall 1.0) — proving the foundation of the exact PAX scan path
    /// (`search_pax_file_exact`, which materialises records via
    /// `read_segment_records`). RawF32 decodes to the exact input vectors, not an
    /// approximation (unlike RaBitQ/SQ8 reconstruction).
    #[test]
    fn rawf32_pax_segment_round_trips_exact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rawf32.pax");
        let records: Vec<ProximaRecord> = (0..8)
            .map(|i| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i,
                    (0..16).map(|d| (i as f32 + d as f32) * 0.1).collect(),
                )
            })
            .collect();
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RawF32, None).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(SegmentFormat::detect(&bytes), SegmentFormat::Pax);

        // Round-trip: RawF32 decodes to the EXACT input vectors (not an
        // approximation, unlike RaBitQ/SQ8 reconstruction).
        let back = read_segment_records(&bytes, &[], &[], None).unwrap();
        assert_eq!(back.len(), records.len());
        for (got, want) in back.iter().zip(records.iter()) {
            assert_eq!(got.oid, want.oid, "oid must round-trip");
            let got_vec = got
                .embeddings
                .first()
                .expect("RawF32 segment must reconstruct its embedding");
            let want_vec = want.embeddings.first().expect("input embedding");
            assert_eq!(
                got_vec.as_fp32_slice(),
                want_vec.as_fp32_slice(),
                "RawF32 PAX must reconstruct the exact vector for {}",
                want.oid
            );
        }
    }

    /// Mixed-read-safe coexistence (storage-format mandate #8). A store rolling
    /// PAX quantization on will have BOTH legacy `ProximaBlocks` segments (written
    /// before the flip) and new PAX segments side by side. The magic-byte router
    /// must read BOTH back through the single `read_segment_records` entry —
    /// disjoint oids, no mis-route. This is the property that makes the staged
    /// default-on flip safe (default OFF in v0.2, flip gated by the recall ratchet).
    #[test]
    fn mixed_format_legacy_and_pax_segments_coexist_and_read_back() {
        let legacy_records = vec![
            rec(
                "legacy_a",
                1_700_000_000_000_000_000,
                vec![1.0, 2.0, 3.0, 4.0],
            ),
            rec(
                "legacy_b",
                1_700_000_000_000_000_001,
                vec![5.0, 6.0, 7.0, 8.0],
            ),
        ];
        let legacy_bytes = ProximaDataBlock::new(legacy_records, BlockCompressionConfig::default())
            .serialize()
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let pax_path = dir.path().join("coexist.pax");
        let pax_records = vec![
            rec(
                "pax_c",
                1_700_000_000_000_000_002,
                vec![9.0, 10.0, 11.0, 12.0],
            ),
            rec(
                "pax_d",
                1_700_000_000_000_000_003,
                vec![13.0, 14.0, 15.0, 16.0],
            ),
        ];
        write_pax_segment(&pax_path, &pax_records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
        let pax_bytes = std::fs::read(&pax_path).unwrap();

        // The two segments are detected as DIFFERENT formats (never mis-routed).
        assert_eq!(
            SegmentFormat::detect(&legacy_bytes),
            SegmentFormat::ProximaBlocks
        );
        assert_eq!(SegmentFormat::detect(&pax_bytes), SegmentFormat::Pax);

        // ...and BOTH read back through the single mixed-format router.
        let legacy_back = read_segment_records(&legacy_bytes, &[], &[], None).unwrap();
        let pax_back = read_segment_records(&pax_bytes, &[], &[], None).unwrap();

        let mut legacy_oids: Vec<&str> = legacy_back.iter().map(|r| r.oid.as_str()).collect();
        legacy_oids.sort_unstable();
        let mut pax_oids: Vec<&str> = pax_back.iter().map(|r| r.oid.as_str()).collect();
        pax_oids.sort_unstable();

        assert_eq!(
            legacy_oids,
            vec!["legacy_a", "legacy_b"],
            "legacy segment oids"
        );
        assert_eq!(
            pax_oids,
            vec!["pax_c", "pax_d"],
            "PAX segment oids (disjoint from legacy)"
        );
    }

    /// TD-156 / ADR-026: block geometry is the fragmentation lever. A larger
    /// `target_block` packs rows into fewer blocks — so the per-block ranged-GET
    /// count on the object-store read path drops (the +29,900% GET fragmentation
    /// measured by the footer-cache economics harness shrinks as block count
    /// falls). Mixed-read-safe: block size is a per-segment write choice, opaque
    /// to the magic-detected read router.
    #[test]
    fn larger_target_block_yields_fewer_blocks() {
        let records: Vec<ProximaRecord> = (0..512)
            .map(|i| rec(&format!("r{i}"), 1000 + i as i64, vec![i as f32 * 0.1; 64]))
            .collect();
        let blocks_for = |target: Option<usize>| -> usize {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("geo.pax");
            write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, target)
                .unwrap()
                .block_count as usize
        };
        let small = blocks_for(Some(8 * 1024));
        let large = blocks_for(Some(256 * 1024));
        eprintln!(
            "[geometry] 512x64 vectors: 8KiB target -> {small} blocks, 256KiB target -> {large} blocks"
        );
        assert!(
            large < small,
            "larger target_block must yield fewer blocks: {large} >= {small}"
        );
        assert!(large >= 1, "at least one block");
    }

    // ── ADR-062 / TD-RDSTRAT-6 coalesced-RaBitQ layout ───────────────────────

    /// Opt into the coalesced-RaBitQ layout for one test. SAFETY: nextest runs
    /// process-per-test, so the env mutation is isolated to this test process.
    fn enable_coalesced_rabitq() {
        unsafe {
            std::env::set_var("PROXIMADB_PAX_COALESCED_RABITQ", "1");
        }
    }

    fn disable_coalesced_rabitq() {
        unsafe {
            std::env::set_var("PROXIMADB_PAX_COALESCED_RABITQ", "0");
        }
    }

    #[test]
    fn coalesced_requested_f32_tier_is_emitted_and_materializes_exact() {
        enable_coalesced_rabitq();
        use proximadb_block_format::col_id;
        use proximadb_storage_common::segment_layout::SegmentFooterIndex;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coalesced-exact.pax");
        let records = vec![
            rec("a", 1, vec![-7.25, 0.123_456_7, 3.141_592_7]),
            rec("b", 2, vec![2.75, 8.765_432, -0.333_333_34]),
            rec("c", 3, vec![11.125, -4.567_891, 9.999_991]),
        ];

        write_pax_segment_with_f32_tier(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            true,
            Some(1024),
        )
        .unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let footer = SegmentFooterIndex::locate_in_segment(&bytes)
            .unwrap()
            .expect("coalesced footer");
        assert!(footer.has_f32_tier, "footer must declare the exact tier");

        let mut scanner =
            PaxSegmentScanner::from_bytes(bytes.clone(), ScanPredicate::default()).unwrap();
        while let Some(block) = scanner.next_block() {
            assert!(
                block.vector_params().get(col_id::F32_TIER_BASE).is_some(),
                "every coalesced block must emit the requested exact tier"
            );
        }

        let materialized = read_segment_records(&bytes, &[], &[], None).unwrap();
        assert_eq!(materialized.len(), records.len());
        for got in &materialized {
            let want = records.iter().find(|r| r.oid == got.oid).unwrap();
            assert_eq!(
                got.embeddings[0].as_fp32_slice(),
                want.embeddings[0].as_fp32_slice(),
                "compaction materialization must prefer exact f32 for {}",
                got.oid
            );
        }
    }

    #[test]
    fn mixed_exact_and_lossy_inputs_do_not_claim_exact_output() {
        disable_coalesced_rabitq();
        let dir = tempfile::tempdir().unwrap();
        let exact = dir.path().join("exact.pax");
        let lossy = dir.path().join("lossy.pax");
        let records = vec![
            rec("a", 1, vec![-1.0, 0.123_456_7, 7.0]),
            rec("b", 2, vec![3.0, 4.765_432, -2.0]),
        ];

        write_pax_segment_with_f32_tier(
            &exact,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            true,
            None,
        )
        .unwrap();
        write_pax_segment_with_f32_tier(
            &lossy,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            false,
            None,
        )
        .unwrap();

        assert!(pax_inputs_have_f32_tier(std::slice::from_ref(&exact)));
        assert!(
            !pax_inputs_have_f32_tier(&[exact.clone(), lossy]),
            "one exact input must not make a mixed compacted output exact"
        );
        assert!(
            !pax_inputs_have_f32_tier(&[exact, dir.path().join("unknown.arrow")]),
            "an unrelated physical format must not be assumed exact"
        );
    }

    #[test]
    fn coalesced_rank_scores_cross_the_public_metric_boundary_once() {
        assert_eq!(
            canonical_score_from_rank_score(RankMetric::L2, 25.0),
            5.0,
            "squared L2 rank score must become canonical Euclidean distance"
        );
        assert_eq!(
            canonical_score_from_rank_score(RankMetric::DotProduct, -6.0),
            6.0,
            "negated-dot rank score must become public positive-dot similarity"
        );
        assert_eq!(
            canonical_score_from_rank_score(RankMetric::Cosine, 0.25),
            0.25,
            "cosine distance is already canonical"
        );
    }

    /// A coalesced-RaBitQ segment (opt-in) is detected as Pax, carries a non-zero
    /// RaBitQ region, and round-trips back to the same records through the
    /// mixed-format reader. The RaBitQ binary tier lives in the header region;
    /// the block's EMBED_BASE is SQ8 (the rerank data), which the reader
    /// reconstructs via the unchanged SQ8 decode path.
    #[test]
    fn coalesced_rabitq_segment_round_trips_and_detects_as_pax() {
        enable_coalesced_rabitq();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coalesced.pax");
        let records: Vec<ProximaRecord> = (0..64)
            .map(|i| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i,
                    (0..48).map(|d| (i as f32 + d as f32) * 0.1).collect(),
                )
            })
            .collect();
        let meta = write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
        // Coalesced layout: a non-empty RaBitQ region pinned right after the
        // 40 B header-prefix.
        assert!(
            meta.rabitq_len > 0,
            "coalesced segment must carry a RaBitQ region"
        );
        assert_eq!(
            meta.rabitq_off,
            proximadb_storage_common::segment_layout::SEG_HEADER_PREFIX_LEN as u64
        );

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(SegmentFormat::detect(&bytes), SegmentFormat::Pax);
        assert!(
            is_coalesced_segment(&bytes),
            "opt-in write must produce the coalesced presence-field"
        );

        let back = read_segment_records(&bytes, &[], &[], None).unwrap();
        assert_eq!(back.len(), records.len());
        let got: std::collections::HashSet<String> = back.iter().map(|r| r.oid.clone()).collect();
        let want: std::collections::HashSet<String> = (0..64).map(|i| format!("v{i}")).collect();
        assert_eq!(got, want, "round-tripped oids must match the input set");
    }

    /// The presence-field cleanly distinguishes a coalesced segment from a legacy
    /// one (mixed-read): opt-out writes carry no RaBitQ region and are NOT
    /// coalesced; opt-in writes are. Both detect as Pax.
    #[test]
    fn coalesced_presence_field_distinguishes_legacy_from_coalesced() {
        let mk = |coalesced: bool, name: &str| -> (Vec<u8>, u64) {
            if coalesced {
                enable_coalesced_rabitq();
            } else {
                disable_coalesced_rabitq();
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(name);
            let records: Vec<ProximaRecord> = (0..16)
                .map(|i| {
                    rec(
                        &format!("v{i}"),
                        1_700_000_000_000_000_000 + i,
                        vec![i as f32 * 0.1; 32],
                    )
                })
                .collect();
            let meta =
                write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
            (std::fs::read(&path).unwrap(), meta.rabitq_len)
        };

        let (legacy_bytes, legacy_len) = mk(false, "legacy.pax");
        assert_eq!(legacy_len, 0, "legacy write has no coalesced region");
        assert!(
            !is_coalesced_segment(&legacy_bytes),
            "legacy write must not carry the coalesced presence-field"
        );
        assert_eq!(SegmentFormat::detect(&legacy_bytes), SegmentFormat::Pax);

        let (coalesced_bytes, coalesced_len) = mk(true, "coalesced.pax");
        assert!(coalesced_len > 0, "opt-in write carries a RaBitQ region");
        assert!(is_coalesced_segment(&coalesced_bytes));
        assert_eq!(SegmentFormat::detect(&coalesced_bytes), SegmentFormat::Pax);
    }

    /// The coalesced region + footer-index of a real written segment parse and the
    /// region ranks: the RaBitQ region decodes (Slice 2a codec over writer output)
    /// and the footer's block table + rabitq mirror match the SegmentMeta. This
    /// ties the writer (2d), scanner footer parse (2e), and region codec (2a).
    #[test]
    fn coalesced_region_and_footer_parse_from_written_segment() {
        enable_coalesced_rabitq();
        use proximadb_block_format::{RaBitQRegion, col_id};
        use proximadb_storage_common::segment_layout::SegmentFooterIndex;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coalesced.pax");
        let records: Vec<ProximaRecord> = (0..128)
            .map(|i| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i,
                    (0..64).map(|d| (i as f32 + d as f32) * 0.1).collect(),
                )
            })
            .collect();
        let meta = write_pax_segment(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            Some(8 * 1024),
        )
        .unwrap();
        assert!(meta.block_count >= 1);

        let bytes = std::fs::read(&path).unwrap();

        // Footer-index: block table + rabitq mirror match the SegmentMeta.
        let footer = SegmentFooterIndex::locate_in_segment(&bytes)
            .unwrap()
            .expect("coalesced footer located");
        assert_eq!(footer.blocks.len(), meta.block_count as usize);
        assert_eq!(footer.rabitq_off, meta.rabitq_off);
        assert_eq!(footer.rabitq_len, meta.rabitq_len);
        assert_eq!(footer.row_count, meta.row_count);
        assert_eq!(footer.embed_quant_tag, 1, "block tier-1 is SQ8");

        // RaBitQ region: decodes + ranks nearest-first; n_rows == record count.
        let off = meta.rabitq_off as usize;
        let region = &bytes[off..off + meta.rabitq_len as usize];
        let parsed = RaBitQRegion::from_bytes(region).unwrap();
        assert_eq!(parsed.n_rows(), records.len());

        // The block decoder no longer finds an in-block RaBitQ stripe (it moved to
        // the header) — mixed-read: the SQ8 EMBED_BASE column is what the block
        // carries now.
        let mut scanner =
            PaxSegmentScanner::from_bytes(bytes.clone(), ScanPredicate::default()).unwrap();
        let block = scanner.next_block().expect("segment has a block");
        let embed = block.vector_params().get(col_id::EMBED_BASE);
        assert!(
            embed.is_some_and(|e| e.quant_kind != proximadb_block_format::QUANT_RABITQ),
            "coalesced block EMBED_BASE must not be RaBitQ (it is SQ8 now)"
        );

        // Rank a query that is one of the records; the top-1 survivor is present.
        let query = records[10]
            .embeddings
            .first()
            .unwrap()
            .as_fp32_slice()
            .to_vec();
        let ranked = parsed.rank(&query, RankMetric::L2, records.len());
        assert!(!ranked.is_empty());
        assert!(parsed.code(ranked[0]).is_some());
    }

    /// ADR-062 scan-then-rerank over a coalesced segment via the filesystem:
    /// recall@k holds vs brute-force ground truth, and the path returns the
    /// nearest neighbours. (The GET-count win is quantified by the SIFT ratchet;
    /// this proves correctness of the new read path end-to-end.)
    #[tokio::test]
    async fn coalesced_scan_then_rerank_recall_vs_bruteforce() {
        enable_coalesced_rabitq();
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

        const DIM: usize = 64;
        const N: usize = 400;
        const K: usize = 10;

        let corpus: Vec<Vec<f32>> = (0..N)
            .map(|i| {
                (0..DIM)
                    .map(|d| (((i * 131 + d * 17) % 251) as f32) * 0.01)
                    .collect()
            })
            .collect();
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| rec(&format!("r{i}"), 1000 + i as i64, v.clone()))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        // Small target block ⇒ multi-block segment (exercises survivor→block mapping).
        write_pax_segment(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            Some(16 * 1024),
        )
        .unwrap();
        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();

        let query = corpus[137].clone();
        let hits =
            rabitq_search_segment_coalesced(&fs, path.to_str().unwrap(), &query, K, RankMetric::L2)
                .await
                .unwrap()
                .expect("coalesced scan-then-rerank must return hits");
        assert!(!hits.is_empty(), "cascade must return top-k");

        // Brute-force ground-truth top-k over the f32 corpus.
        let l2 =
            |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>();
        let mut idx: Vec<usize> = (0..N).collect();
        idx.sort_by(|&a, &b| {
            l2(&corpus[a], &query)
                .partial_cmp(&l2(&corpus[b], &query))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let truth: std::collections::HashSet<String> =
            idx.iter().take(K).map(|i| format!("r{i}")).collect();
        let got: std::collections::HashSet<String> = hits.iter().map(|h| h.oid.clone()).collect();
        let recall = truth.iter().filter(|o| got.contains(*o)).count() as f32 / K as f32;
        assert!(
            recall >= 0.90,
            "coalesced scan-then-rerank recall@{K} = {recall:.2} (N={N}, pool={POOL})"
        );

        // The nearest neighbour (r137 is the query itself) must be ranked first.
        assert_eq!(
            hits[0].oid, "r137",
            "the query vector itself must be the top hit"
        );
    }
}
