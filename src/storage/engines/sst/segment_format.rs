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
use proximadb_block_format::prune::{FieldToColumn, PruneResult, evaluate_block};
use proximadb_block_format::{
    BLOCK_MAGIC, BlockCompression, BlockMode, RankMetric, VectorQuant, col_id,
};
use proximadb_filter_expression::FilterExpression;
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
/// D). When `f32_tier` is true (and the quant is RaBitQ), each embedding also
/// gets a co-located raw-f32 stripe at `col_id::F32_TIER_BASE + i` for an exact
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

/// True if any input `.pax` segment carries the opt-in exact-f32 tier
/// (`F32_TIER_BASE`). Compaction calls this to PRESERVE the tier across
/// re-encoding — otherwise [`write_pax_segment`] drops it. Reads each `.pax`
/// input's block metadata only; the inputs are page-cached (just read into
/// records), so this is cheap. Correct for both env- and tag-opt-in (the source
/// segment reflects whatever the collection wrote).
pub fn pax_inputs_have_f32_tier(input_files: &[std::path::PathBuf]) -> bool {
    use proximadb_block_format::col_id;
    for f in input_files {
        if f.extension().and_then(|e| e.to_str()) != Some("pax") {
            continue;
        }
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        // Read block 0's column metadata via the segment scanner. A `.pax`
        // SEGMENT file is laid out `[block(s)][segment_index][SEGMENT_MAGIC]`;
        // `PaxBlockReader::open` parses a SINGLE block and reads its footer from
        // the trailing `BLOCK_FOOTER_SIZE` bytes, so opening the whole file
        // misreads the segment index/magic as a block footer. The scanner parses
        // the trailing index + magic and hands back block 0 correctly.
        let Ok(mut scanner) = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()) else {
            continue;
        };
        let Some(reader) = scanner.next_block() else {
            continue;
        };
        if reader.vector_params().get(col_id::F32_TIER_BASE).is_some() {
            return true;
        }
    }
    false
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

/// One scored hit from a RaBitQ cascade segment scan: the record's `oid` and its
/// L2 distance to the query (smaller = nearer), reranked against the co-located
/// SQ8 column.
#[derive(Debug, Clone)]
pub struct CascadeHit {
    pub oid: String,
    pub distance: f32,
    /// Exact f32 vector (from the opt-in f32 tier) for `include_vectors`
    /// materialization. `None` when no f32 tier is present (the caller gets
    /// id+score only, as before). Populated lazily for the top-k rows only.
    pub vector: Option<Vec<f32>>,
}

/// A metadata pre-prune for the cascade scan: a [`FilterExpression`] plus the
/// field→column resolver that maps its leaf field names to canonical PAX column
/// ids. Threaded through [`rabitq_search_segment`] so a block whose zone maps
/// provably exclude the predicate is skipped **before** the (expensive) RaBitQ
/// vector pass — the cheapest, most-selective stage of the pruning cascade.
pub struct MetaPrune<'a> {
    pub filter: &'a FilterExpression,
    pub field_to_col: &'a FieldToColumn<'a>,
}

/// Canonical resolver from a filter field name to its PAX column id for
/// block/row-group pruning — the single mapper shared by every PAX prune path
/// (the relational ranged `read_records_pruned` scan and the RaBitQ cascade's
/// [`MetaPrune`]). Only the fixed canonical columns carry zone-map stripes; user
/// metadata lives in opaque `props` (not prunable), so unknown fields return
/// `None` and the pruner conservatively keeps the block (no false negatives).
///
/// A plain `fn` (not a closure) so it coerces to both `&FieldToColumn` and the
/// `Sync` form the object-storage ranged path requires.
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

/// Cold-scan a PAX+RaBitQ segment for the `k` nearest neighbours of `query` using
/// the co-designed cascade (P3 C.2): per block, an optional **metadata stats
/// pre-prune** skips blocks the predicate provably excludes (the cheap pre-vector
/// filter), then the RaBitQ codes drive a candidate prefilter (`pool` rows via
/// `rabitq_rank`), and ONLY those candidates are reranked against the co-located
/// SQ8 column (`rerank_rows`) — full f32 is never decoded. Hits merge across
/// blocks into a global top-`k` (nearest-first).
///
/// `prune` runs first because it moves the dominant cost term (I/O round-trips):
/// a skipped block reads neither its codes stripe nor any SQ8 candidate, so the
/// trace below records fewer bytes/gets for a selective query than the unpruned
/// baseline. Pruning is conservative (a block is skipped only if it *cannot*
/// match), so it never drops a true match; final per-row metadata filtering
/// remains the caller's responsibility (consistent with the materialize-and-score
/// search path).
///
/// Returns `Ok(None)` when `bytes` is not a PAX segment, or is a PAX segment with
/// no RaBitQ-coded embedding (e.g. SQ8/raw): the caller then falls back to its
/// normal materialize-and-score path, so this is additive and mixed-read-safe.
///
/// Emits a query-scoped I/O trace into the active
/// [`io_trace`](crate::observability::io_trace) scope (no-op outside one),
/// modelling the cascade's *logical* reads per kept block — the RaBitQ codes
/// stripe (stage 1) plus the candidate SQ8 bytes (stage 2). Pruned blocks
/// contribute nothing, so the trace makes the pruning win observable per the
/// co-design measure-first mandate. (Whole-segment transport bytes are traced by
/// the caller that fetched them; ranged codes-only fetches are a follow-up.)
pub fn rabitq_search_segment(
    bytes: &[u8],
    query: &[f32],
    k: usize,
    pool: usize,
    metric: RankMetric,
    prune: Option<MetaPrune<'_>>,
) -> Result<Option<Vec<CascadeHit>>> {
    use crate::observability::io_trace;

    if SegmentFormat::detect(bytes) != SegmentFormat::Pax {
        return Ok(None);
    }
    let mut scanner = PaxSegmentScanner::from_bytes(bytes.to_vec(), ScanPredicate::default())?;

    let pool = pool.max(k);
    let mut any_rabitq = false;
    let mut hits: Vec<CascadeHit> = Vec::new();
    while let Some(block) = scanner.next_block() {
        // Stage 0: metadata stats pre-prune. Skip the whole block — no codes, no
        // SQ8 — when its zone maps provably exclude the predicate (conservative:
        // `Skip` only when the block cannot match, so no true match is lost).
        if let Some(mp) = &prune
            && evaluate_block(&block, mp.filter, mp.field_to_col) == PruneResult::Skip
        {
            continue;
        }

        // Stage 1: RaBitQ candidate prefilter on the hot codes column. A block
        // whose EMBED_BASE isn't RaBitQ-encoded yields `None` and is skipped
        // (mixed-quant segments stay safe).
        let Some(cand) = block.rabitq_rank(query, pool, metric) else {
            continue;
        };
        any_rabitq = true;

        // Project the cascade's LOGICAL striped read for this kept block: the RaBitQ
        // codes stripe (stage 1) + the SQ8 bytes for the candidate pool (stage 2) —
        // what a selective striped read WOULD move for the *real* candidate set. This
        // is recorded into the distinct `logical_striped_*` counters (NOT the physical
        // `bytes_read`/`range_gets`, which reflect the whole-segment `fs.read`), so the
        // striped-vs-whole headroom is measurable per query on real candidate scatter
        // (ADR-057 / TD-RDSTRAT-3). Projection-only; moves no bytes. Fixes the
        // TD-RDSTRAT-2 double-count (it previously inflated the physical byte total).
        let dim = block
            .vector_params()
            .get(col_id::EMBED_BASE)
            .map(|e| e.dim as u64)
            .unwrap_or(0);
        let codes_len = block
            .column_metas()
            .iter()
            .find(|m| m.column_id == col_id::EMBED_BASE)
            .map(|m| m.stripe_len as u64)
            .unwrap_or(0);
        io_trace::record_logical_striped(codes_len + cand.len() as u64 * dim, 1);

        // Stage 2 rerank over ONLY the candidate rows. Prefer the EXACT-f32 tier
        // when present (P3 Phase D: recall ≈ 1.0), else the co-located SQ8 rerank
        // column, else keep the RaBitQ-coarse order.
        let scored = block
            .rerank_rows_f32_exact(0, query, &cand, metric)
            .or_else(|| block.rerank_rows(0, query, &cand, metric))
            .unwrap_or_else(|| {
                cand.iter()
                    .enumerate()
                    .map(|(rank, &row)| (row, rank as f32))
                    .collect()
            });
        let oids = block.decode_str_stripe(col_id::OID);
        // include_vectors: when the f32 tier is present, decode the top-k exact
        // vectors (top-k rows only — lazy, read-budget-tight) and attach them so
        // the caller can materialize exact vectors without a second segment scan.
        let topk: Vec<(usize, f32)> = scored.into_iter().take(k).collect();
        let topk_rows: Vec<usize> = topk.iter().map(|(r, _)| *r).collect();
        let f32_vecs = block.decode_f32_tier_rows(0, &topk_rows);
        for (i, (row, dist)) in topk.into_iter().enumerate() {
            let oid = oids
                .as_ref()
                .and_then(|o| o.get(row).cloned().flatten())
                .unwrap_or_default();
            let vector = f32_vecs.as_ref().and_then(|v| v.get(i).cloned().flatten());
            hits.push(CascadeHit {
                oid,
                distance: dist,
                vector,
            });
        }
    }
    if !any_rabitq {
        return Ok(None);
    }
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    Ok(Some(hits))
}

/// Whether the coalesced-RaBitQ layout is engaged for new RaBitQ writes
/// (ADR-062 / TD-RDSTRAT-6). **Opt-in** (`PROXIMADB_PAX_COALESCED_RABITQ=1`),
/// default OFF, per the storage-format-migration mandate (default-OFF until the
/// recall/GET measurement bakes it — the SIFT ratchet in this PR is that bake).
/// The reader handles BOTH layouts via the `SEG_HEADER_MAGIC` presence-field, so
/// this is mixed-read-safe either way; flipping to default-on is the gated
/// follow-up once the SIFT measurement clears recall ≈ 0.99 + GETs ≪ 370.
pub fn coalesced_rabitq_enabled() -> bool {
    matches!(
        std::env::var("PROXIMADB_PAX_COALESCED_RABITQ")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "on" | "true" | "yes")
    )
}

/// Whether the cost-driven selective (striped) read is engaged (ADR-057 /
/// TD-RDSTRAT-3 Slice C). Default OFF — the whole-segment read stays the default
/// until the observe→flip gate; `PROXIMADB_PAX_STRIPED_READ=1` opts in.
pub fn striped_read_enabled() -> bool {
    matches!(
        std::env::var("PROXIMADB_PAX_STRIPED_READ")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "on" | "yes")
    )
}

/// Bounds-checked slice of a block-relative metadata `range` from a buffer that
/// begins at block-relative `base` (fail-closed on a corrupt footer, no panic).
fn slice_at<'a>(buf: &'a [u8], base: u64, range: &std::ops::Range<u64>) -> Result<&'a [u8]> {
    let (s, e) = (range.start.checked_sub(base), range.end.checked_sub(base));
    match (s, e) {
        (Some(s), Some(e)) if s <= e && (e as usize) <= buf.len() => {
            Ok(&buf[s as usize..e as usize])
        }
        _ => anyhow::bail!("striped read: metadata range {range:?} outside buffer (base {base})"),
    }
}

/// **Selective (striped) cascade** over a PAX segment on the engine's own
/// filesystem (ADR-057 / TD-RDSTRAT-3 Slice C): the ranged analogue of
/// [`rabitq_search_segment`]. Reads ONLY the tail segment index + per surviving
/// block the RaBitQ codes stripe (Stage-1 rank) + the SQ8 candidate-rerank stripe
/// (Stage-2) + the OID stripe — never the whole segment — via `fs.read_range`,
/// composing the `BlockLayout` ranged primitives. Returns the SAME top-`k`
/// `CascadeHit`s as the whole-segment cascade for the default SQ8 config (parity
/// gated in tests); `Ok(None)` when the tail index can't be located from a bounded
/// suffix or no block is RaBitQ-coded, so the caller falls back to the whole read
/// (mixed-read-safe). Physical bytes/GETs are recorded by the fs layer, so the
/// striped read's real cost is observable in `io_trace`. Metadata pruning (filtered
/// queries) is a follow-up — the caller routes only unfiltered queries here.
pub async fn rabitq_search_segment_ranged(
    fs: &dyn proximadb_storage_filesystem_types::FileSystem,
    path: &str,
    query: &[f32],
    k: usize,
    pool: usize,
    metric: RankMetric,
    // TD-RDSTRAT-5 S3: when `Some`, scan ONLY these block indices (the centroid
    // probe-prune survivors). `None` scans every block (unchanged behaviour). An
    // index out of range is simply never matched (no panic).
    selected: Option<&[usize]>,
) -> Result<Option<Vec<CascadeHit>>> {
    use proximadb_block_format::BlockFooter;
    use proximadb_block_format::ranged::{BlockLayout, footer_tail_range, metadata_ranges};
    use proximadb_storage_common::pax_block::SegmentIndex;

    let size = fs
        .metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!("striped read stat {path}: {e}"))?
        .size;
    if size < SEGMENT_MAGIC.len() as u64 {
        return Ok(None);
    }

    // Locate the tail segment index from a bounded suffix (grown ×8 on miss); if it
    // still doesn't fit at full size, decline → caller reads the whole segment.
    let mut suffix_len = (64 * 1024u64).min(size);
    let index = loop {
        let suffix = fs
            .read_range(path, size - suffix_len, suffix_len)
            .await
            .map_err(|e| anyhow::anyhow!("striped read suffix {path}: {e}"))?;
        if !suffix.ends_with(SEGMENT_MAGIC) {
            return Ok(None); // not a PAX segment
        }
        let before_magic = &suffix[..suffix.len() - SEGMENT_MAGIC.len()];
        match SegmentIndex::locate(before_magic) {
            Ok(idx) => break idx,
            Err(_) if suffix_len < size => suffix_len = (suffix_len * 8).min(size),
            Err(_) => return Ok(None),
        }
    };

    let pool = pool.max(k);
    let mut any_rabitq = false;
    let mut hits: Vec<CascadeHit> = Vec::new();
    for (block_idx, entry) in index.blocks.iter().enumerate() {
        // S3 centroid prune: skip blocks the probe-prune didn't select.
        if let Some(sel) = selected
            && !sel.contains(&block_idx)
        {
            continue;
        }
        let (off, bsz) = (entry.offset, entry.size as u64);
        if off + bsz > size {
            anyhow::bail!("striped read: block [{off}..+{bsz}] past segment {size}");
        }

        // Footer (last 32 B) → metadata extent (one ranged read) → BlockLayout.
        let fr = footer_tail_range(bsz)?;
        let footer_bytes = fs
            .read_range(path, off + fr.start, fr.end - fr.start)
            .await?;
        let footer = BlockFooter::from_bytes(&footer_bytes)?;
        let mr = metadata_ranges(&footer, bsz);
        let footer_start = bsz - proximadb_block_format::BLOCK_FOOTER_SIZE as u64;
        let mut meta_start = mr.col_meta.start;
        if let Some(r) = &mr.vparam {
            meta_start = meta_start.min(r.start);
        }
        if let Some(r) = &mr.rgdir {
            meta_start = meta_start.min(r.start);
        }
        let meta_buf = if footer_start > meta_start {
            fs.read_range(path, off + meta_start, footer_start - meta_start)
                .await?
        } else {
            Vec::new()
        };
        let col_meta = slice_at(&meta_buf, meta_start, &mr.col_meta)?;
        let vparam = match &mr.vparam {
            Some(r) => Some(slice_at(&meta_buf, meta_start, r)?),
            None => None,
        };
        let rgdir = match &mr.rgdir {
            Some(r) => Some(slice_at(&meta_buf, meta_start, r)?),
            None => None,
        };
        let layout = BlockLayout::assemble(footer, col_meta, vparam, rgdir)?;

        // Stage 1: fetch ONLY the codes stripe, rank.
        let Some(cr) = layout.column_stripe_range(col_id::EMBED_BASE) else {
            continue;
        };
        let codes = fs
            .read_range(path, off + cr.start, cr.end - cr.start)
            .await?;
        let Some(cand) = layout.rabitq_rank(query, pool, metric, &codes) else {
            continue; // not RaBitQ-coded
        };
        any_rabitq = true;
        if cand.is_empty() {
            continue;
        }

        // Stage 2: fetch the SQ8 rerank stripe, rerank ONLY the candidate rows.
        let scored: Vec<(usize, f32)> = match layout.column_stripe_range(col_id::RERANK_BASE) {
            Some(rr) => {
                let rer = fs
                    .read_range(path, off + rr.start, rr.end - rr.start)
                    .await?;
                layout
                    .rerank_candidate_rows(col_id::RERANK_BASE, query, &cand, metric, &rer)
                    .unwrap_or_else(|| {
                        cand.iter()
                            .enumerate()
                            .map(|(rank, &row)| (row, rank as f32))
                            .collect()
                    })
            }
            None => cand
                .iter()
                .enumerate()
                .map(|(rank, &row)| (row, rank as f32))
                .collect(),
        };
        let topk: Vec<(usize, f32)> = scored.into_iter().take(k).collect();

        // Fetch the OID stripe once for this block, attach ids.
        let oids = match layout.column_stripe_range(col_id::OID) {
            Some(o) => {
                let ob = fs.read_range(path, off + o.start, o.end - o.start).await?;
                layout.decode_str_column(col_id::OID, &ob)
            }
            None => None,
        };
        for (row, dist) in topk {
            let oid = oids
                .as_ref()
                .and_then(|o| o.get(row).cloned().flatten())
                .unwrap_or_default();
            hits.push(CascadeHit {
                oid,
                distance: dist,
                vector: None,
            });
        }
    }

    if !any_rabitq {
        return Ok(None);
    }
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    Ok(Some(hits))
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

/// ADR-062 / TD-RDSTRAT-6 **scan-then-rerank** over a coalesced-RaBitQ segment
/// on the engine's own filesystem — the ranged analogue of
/// [`rabitq_search_segment`] for the new layout:
///
/// 1. **Scan** the coalesced RaBitQ header region (one ranged GET, header-prefix
///    coalesced in) — `keep=100%`: rank ALL codes → approximate distance for
///    every vector (zero prune loss).
/// 2. **Select** the top-M survivors (M = `pool`, the adaptive GET-budget).
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
    pool: usize,
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

    // 5. Plan coalesced ranged GETs over the survivor blocks (the GET-win policy).
    let policy =
        crate::storage::engines::sst::readers::sst_query_engine::ObjectRangeCoalescePolicy::default(
        );
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
    Ok(Some(hits))
}

/// Cheap cost inputs for the S2 read-strategy chooser, read from the tail segment
/// index alone (one small ranged read — no per-block metadata, no stripe bytes):
/// `(n_blocks, total_block_bytes, segment_size)`. Returns `None` when the index
/// can't be located from a bounded suffix (caller then defaults to the whole read).
/// `total_block_bytes` (Σ block sizes) is the whole-read byte cost; the striped read
/// touches only a fraction (codes + candidate rerank), estimated by the caller.
pub async fn segment_index_summary(
    fs: &dyn proximadb_storage_filesystem_types::FileSystem,
    path: &str,
) -> Result<Option<(u64, u64, u64)>> {
    use proximadb_storage_common::pax_block::SegmentIndex;

    let size = fs
        .metadata(path)
        .await
        .map_err(|e| anyhow::anyhow!("index-summary stat {path}: {e}"))?
        .size;
    if size < SEGMENT_MAGIC.len() as u64 {
        return Ok(None);
    }
    let mut suffix_len = (64 * 1024u64).min(size);
    loop {
        let suffix = fs
            .read_range(path, size - suffix_len, suffix_len)
            .await
            .map_err(|e| anyhow::anyhow!("index-summary suffix {path}: {e}"))?;
        if !suffix.ends_with(SEGMENT_MAGIC) {
            return Ok(None);
        }
        let before_magic = &suffix[..suffix.len() - SEGMENT_MAGIC.len()];
        match SegmentIndex::locate(before_magic) {
            Ok(idx) => {
                let n_blocks = idx.blocks.len() as u64;
                let total_block_bytes: u64 = idx.blocks.iter().map(|b| b.size as u64).sum();
                return Ok(Some((n_blocks, total_block_bytes, size)));
            }
            Err(_) if suffix_len < size => suffix_len = (suffix_len * 8).min(size),
            Err(_) => return Ok(None),
        }
    }
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

    /// P3 Phase D: `write_pax_segment` with `VectorQuant::RaBitQ` produces a segment
    /// whose `EMBED_BASE` column is RaBitQ-quantized (~30×) — the per-collection write
    /// selection that gives the RaBitQ ANN scan real segments to read. Records still
    /// round-trip through the format router.
    #[test]
    fn write_pax_segment_rabitq_selects_rabitq_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rabitq.pax");
        let records: Vec<ProximaRecord> = (0..8)
            .map(|i| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i,
                    (0..16).map(|d| (i as f32 + d as f32) * 0.1).collect(),
                )
            })
            .collect();
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();

        // The written segment's vector column must be RaBitQ-quantized.
        let bytes = std::fs::read(&path).unwrap();
        let mut scanner = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()).unwrap();
        let block = scanner.next_block().expect("segment has one block");
        assert!(
            block.decode_rabitq_codes(col_id::EMBED_BASE).is_some(),
            "VectorQuant::RaBitQ must encode EMBED_BASE as RaBitQ codes"
        );

        // And it still reads back through the mixed-format router.
        let bytes2 = std::fs::read(&path).unwrap();
        let back = read_segment_records(&bytes2, &[], &[], None).unwrap();
        assert_eq!(back.len(), records.len());
    }

    /// `pax_segment_is_coarse_rabitq_without_f32_tier` classifies a segment by its
    /// block-0 column metadata: true ONLY for RaBitQ `EMBED_BASE` without an f32
    /// tier (the default → coarse rebuild → skip AXIS), false for RawF32/SQ8
    /// (exact / near-exact → rebuild) and RaBitQ-with-tier (exact via tier →
    /// rebuild). Drives the cold-read recall fix in `ensure_axis_index_from_sst`.
    #[test]
    fn pax_segment_is_coarse_rabitq_without_f32_tier_classifies_segments() {
        let records: Vec<ProximaRecord> = (0..8)
            .map(|i| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i,
                    (0..16).map(|d| (i as f32 + d as f32) * 0.1).collect(),
                )
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();

        // RaBitQ, no f32 tier (the default) → coarse → true (skip AXIS rebuild).
        let path = dir.path().join("rabitq.pax");
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            pax_segment_is_coarse_rabitq_without_f32_tier(&bytes),
            "RaBitQ-PAX without f32 tier is coarse → skip AXIS rebuild"
        );

        // RawF32 → exact → false.
        let path = dir.path().join("rawf32.pax");
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RawF32, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !pax_segment_is_coarse_rabitq_without_f32_tier(&bytes),
            "RawF32-PAX is exact → rebuild AXIS"
        );

        // SQ8 → near-exact → false.
        let path = dir.path().join("sq8.pax");
        write_pax_segment(&path, &records, "col", 1, VectorQuant::Sq8, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !pax_segment_is_coarse_rabitq_without_f32_tier(&bytes),
            "SQ8-PAX is near-exact → rebuild AXIS"
        );

        // RaBitQ WITH f32 tier → exact via tier → false.
        let path = dir.path().join("rabitq_tier.pax");
        write_pax_segment_with_f32_tier(&path, &records, "col", 1, VectorQuant::RaBitQ, true, None)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !pax_segment_is_coarse_rabitq_without_f32_tier(&bytes),
            "RaBitQ-PAX with f32 tier is exact → rebuild AXIS"
        );

        // Non-PAX bytes → false (treated as exact-readable → rebuild).
        assert!(
            !pax_segment_is_coarse_rabitq_without_f32_tier(&[0u8; 64]),
            "non-PAX bytes → false"
        );
    }

    /// M1 (ADR-049): a `VectorQuant::RawF32` PAX segment carries no RaBitQ codes,
    /// so the RaBitQ cascade (`rabitq_search_segment`) returns `None` for it. The
    /// search dispatch then takes the exact PAX scan (`search_pax_file_exact`),
    /// which materialises records via `read_segment_records`. This proves the two
    /// foundations of that path: a RawF32 PAX segment round-trips to EXACT f32
    /// vectors (recall 1.0), and the cascade correctly reports it as a miss.
    #[test]
    fn rawf32_pax_segment_round_trips_exact_and_misses_rabitq_cascade() {
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

        // The RaBitQ cascade must report a miss (no RaBitQ codes) — the exact
        // PAX scan in the search dispatch handles this segment instead.
        let query = records[0]
            .embeddings
            .first()
            .expect("query embedding")
            .as_fp32_slice()
            .to_vec();
        assert!(
            rabitq_search_segment(&bytes, &query, 4, 8, RankMetric::L2, None)
                .unwrap()
                .is_none(),
            "RawF32 PAX has no RaBitQ codes → cascade must return None (exact scan handles it)"
        );
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

    /// P3 C.2 cascade primitive — recall + trace gate. A real PAX+RaBitQ segment
    /// is cold-scanned via `rabitq_search_segment` (RaBitQ candidate prefilter →
    /// SQ8 rerank → top-k); recall@10 must hold ≥ 0.90 vs the exact-f32 baseline,
    /// and the query-scoped I/O trace must meter the segment read (measure-first
    /// mandate). Non-PAX / non-RaBitQ input returns `None` so the caller falls
    /// back — exercised at the end.
    #[test]
    fn rabitq_search_segment_cascade_recall_and_trace() {
        const DIM: usize = 64;
        const N: usize = 512;
        const Q: usize = 30;
        const K: usize = 10;
        const POOL: usize = 100;
        const RATCHET: f32 = 0.90;

        let gen_vec = |seed: u64| -> Vec<f32> {
            let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
            (0..DIM)
                .map(|_| {
                    s ^= s >> 30;
                    s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    s ^= s >> 27;
                    ((s >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
                })
                .collect()
        };
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| gen_vec(i as u64)).collect();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rabitq_search.pax");
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i as i64,
                    v.clone(),
                )
            })
            .collect();
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();

        let l2 =
            |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum() };
        let exact_topk = |q: &[f32]| -> std::collections::HashSet<String> {
            let mut idx: Vec<usize> = (0..N).collect();
            idx.sort_by(|&a, &b| {
                l2(&corpus[a], q)
                    .partial_cmp(&l2(&corpus[b], q))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            idx.into_iter().take(K).map(|i| format!("v{i}")).collect()
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(crate::observability::io_trace::scope(async {
            let mut recalls = Vec::with_capacity(Q);
            for qi in 0..Q {
                let base = (qi * (N / Q)) % N;
                let noise = gen_vec((qi as u64).wrapping_add(7_000_000));
                let query: Vec<f32> = corpus[base]
                    .iter()
                    .zip(&noise)
                    .map(|(v, n)| v + n * 0.01)
                    .collect();

                let hits = rabitq_search_segment(&bytes, &query, K, POOL, RankMetric::L2, None)
                    .unwrap()
                    .expect("PAX+RaBitQ segment must run the cascade");
                let got: std::collections::HashSet<String> =
                    hits.into_iter().map(|h| h.oid).collect();
                let truth = exact_topk(&query);
                let hit = truth.iter().filter(|o| got.contains(*o)).count();
                recalls.push(hit as f32 / K as f32);
            }
            let mean = recalls.iter().sum::<f32>() / recalls.len() as f32;
            assert!(
                mean >= RATCHET,
                "P3 C.2: RaBitQ→SQ8 cascade recall@{K} at N={N} = {mean:.3} < ratchet {RATCHET}"
            );

            // The cold scan must meter the segment read on the active trace.
            // `bytes_read` is always-on; `range_gets` is gated behind the
            // `io-trace` feature (ADR-027 two-class trace) — so the range_gets
            // assertion only runs when the feature is enabled.
            let snap = crate::observability::io_trace::snapshot().unwrap();
            // The cascade meters its selective-read PROJECTION on the distinct
            // `logical_striped_*` counters (ADR-057 / TD-RDSTRAT-3), not the physical
            // `bytes_read`/`range_gets` (which reflect the whole-segment `fs.read` —
            // absent here since this unit test passes in-memory bytes).
            assert!(
                snap.logical_striped_bytes > 0,
                "io_trace must record logical_striped_bytes for the PAX cold scan"
            );
            assert!(
                snap.logical_striped_gets >= 1,
                "io_trace must record at least one logical striped GET for the PAX cold scan"
            );
        }));

        // Non-RaBitQ input returns None → the caller keeps its normal path.
        let legacy = ProximaDataBlock::new(
            vec![rec("z", 1, vec![0.0; DIM])],
            BlockCompressionConfig::default(),
        )
        .serialize()
        .unwrap();
        assert!(
            rabitq_search_segment(&legacy, &vec![0.0; DIM], K, POOL, RankMetric::L2, None)
                .unwrap()
                .is_none(),
            "non-PAX input must return None (caller falls back)"
        );
    }

    /// P3 metadata stats pre-prune — the round-trip lever. A multi-block PAX+RaBitQ
    /// segment with monotonically increasing `created_at` is cold-scanned twice: an
    /// unpruned baseline, then with a `created_at >= threshold` filter. The filter
    /// must (1) make the trace read STRICTLY fewer bytes and gets (early blocks
    /// skipped before the vector pass), and (2) stay SOUND — a vector in a surviving
    /// block is still returned. This is the cheap pre-vector filter that moves the
    /// dominant cost term (I/O round-trips), measured by the I/O trace, not asserted.
    /// Phase D completion: when the opt-in f32 tier is present, the cascade
    /// materializes the EXACT f32 vector for each top-k hit (`include_vectors`).
    /// Without the tier, hits carry `vector = None` (id+score only).
    #[test]
    fn rabitq_search_segment_materializes_vectors_when_f32_tier_present() {
        const DIM: usize = 64;
        const N: usize = 256;
        const K: usize = 10;
        const POOL: usize = 100;

        let gen_vec = |seed: u64| -> Vec<f32> {
            let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
            (0..DIM)
                .map(|_| {
                    s ^= s >> 30;
                    s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    s ^= s >> 27;
                    ((s >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
                })
                .collect()
        };
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| gen_vec(i as u64)).collect();
        let by_oid = |oid: &str| -> Vec<f32> {
            let i = oid[1..].parse::<usize>().unwrap();
            corpus[i].clone()
        };

        let dir = tempfile::tempdir().unwrap();
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| {
                rec(
                    &format!("v{i}"),
                    1_700_000_000_000_000_000 + i as i64,
                    v.clone(),
                )
            })
            .collect();

        // WITH the f32 tier → hits carry their exact f32 vector.
        let path = dir.path().join("f32_tier.pax");
        write_pax_segment_with_f32_tier(&path, &records, "col", 1, VectorQuant::RaBitQ, true, None)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let query = corpus[0].clone();
        let hits = rabitq_search_segment(&bytes, &query, K, POOL, RankMetric::L2, None)
            .unwrap()
            .expect("PAX+RaBitQ+tier segment runs the cascade");
        assert!(!hits.is_empty(), "cascade must return top-k");
        for h in &hits {
            let v = h
                .vector
                .as_ref()
                .expect("hit carries exact f32 vector when the tier is present");
            assert_eq!(
                v,
                &by_oid(&h.oid),
                "materialized vector must be exact for {}",
                h.oid
            );
        }

        // WITHOUT the tier → hits carry no vector (id+score only).
        let path2 = dir.path().join("no_tier.pax");
        write_pax_segment(&path2, &records, "col", 1, VectorQuant::RaBitQ, None).unwrap();
        let bytes2 = std::fs::read(&path2).unwrap();
        let hits2 = rabitq_search_segment(&bytes2, &query, K, POOL, RankMetric::L2, None)
            .unwrap()
            .expect("cascade runs");
        assert!(
            hits2.iter().all(|h| h.vector.is_none()),
            "no f32 tier → hits must not carry vectors"
        );
    }

    /// `pax_inputs_rerank_quant` detects the tier-2 rerank quant of a `.pax`
    /// source segment so compaction PRESERVES it (an FP16/f32 rerank stays
    /// FP16/f32; only a missing rerank column → default Sq8). Mirrors the
    /// f32-tier detection contract — guards against compaction silently
    /// downgrading a configured higher-fidelity rerank tier to SQ8.
    #[test]
    fn pax_inputs_rerank_quant_detects_source_tier() {
        let dir = tempfile::tempdir().unwrap();
        let records: Vec<ProximaRecord> = (0..8)
            .map(|i| {
                let vec: Vec<f32> = (0..32).map(|j| (i * 32 + j) as f32 * 0.01).collect();
                rec(&format!("v{i}"), 1_700_000_000_000_000_000 + i as i64, vec)
            })
            .collect();

        // FP16 rerank → detected as Fp16.
        let fp16_path = dir.path().join("fp16.pax");
        write_pax_segment_full(
            &fp16_path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Fp16,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            pax_inputs_rerank_quant(&[fp16_path.clone()]),
            VectorQuant::Fp16
        );

        // f32 rerank → detected as RawF32.
        let f32_path = dir.path().join("f32.pax");
        write_pax_segment_full(
            &f32_path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::RawF32,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            pax_inputs_rerank_quant(&[f32_path.clone()]),
            VectorQuant::RawF32
        );

        // Default (Sq8) rerank → detected as Sq8.
        let sq8_path = dir.path().join("sq8.pax");
        write_pax_segment_full(
            &sq8_path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            pax_inputs_rerank_quant(&[sq8_path.clone()]),
            VectorQuant::Sq8
        );

        // No `.pax` input (or none with a rerank column) → default Sq8.
        let non_pax = dir.path().join("notpax.sst");
        std::fs::write(&non_pax, b"junk").unwrap();
        assert_eq!(pax_inputs_rerank_quant(&[non_pax]), VectorQuant::Sq8);
    }

    #[test]
    fn rabitq_search_segment_metadata_pruning_skips_blocks() {
        use proximadb_filter_expression::ComparisonOperator;

        const DIM: usize = 32;
        const N: usize = 300;
        const K: usize = 10;
        const POOL: usize = 50;
        // Two clusters with a large gap in created_at so no PAX block can
        // straddle the filter boundary. The gap (100k ns ≈ many block widths)
        // guarantees the zone-map min/max of any block falls entirely on one
        // side of THRESHOLD, making the prune deterministic regardless of the
        // exact block boundaries the writer produces on different runners.
        //   records 0..199:   created_at = 1_000 + i     (prunable)
        //   records 200..299: created_at = 100_000 + i   (surviving)
        const THRESHOLD: i64 = 50_000;

        let gen_vec = |seed: u64| -> Vec<f32> {
            let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
            (0..DIM)
                .map(|_| {
                    s ^= s >> 30;
                    s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    s ^= s >> 27;
                    ((s >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
                })
                .collect()
        };
        let corpus: Vec<Vec<f32>> = (0..N).map(|i| gen_vec(i as u64)).collect();

        // Force several blocks with a small block-size threshold so early blocks
        // fall entirely below THRESHOLD and become prunable.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metaprune.pax");
        let mut w = PaxSegmentWriter::new(
            &path,
            BlockMode::Pax,
            BlockCompression::None,
            "col",
            0,
            1,
            Some(2048),
        )
        .with_quant(VectorQuant::RaBitQ);
        for (i, v) in corpus.iter().enumerate() {
            // Two clusters: [0..200) at 1k+i, [200..300) at 100k+i.
            let ts = if i < 200 {
                1_000 + i as i64
            } else {
                100_000 + i as i64
            };
            w.add_record(&rec(&format!("v{i}"), ts, v.clone())).unwrap();
        }
        let meta = w.finish().unwrap();
        assert!(
            meta.block_count >= 3,
            "test needs multiple blocks, got {}",
            meta.block_count
        );
        let bytes = std::fs::read(&path).unwrap();

        // A query that lives in a SURVIVING block (created_at 100290 >= THRESHOLD).
        let query = corpus[290].clone();

        let filter = FilterExpression::Comparison {
            field: "created_at".to_string(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: serde_json::json!(THRESHOLD),
        };
        // Use the SAME canonical mapper the relational pruned-read path uses.
        let f2c = pax_field_to_col;

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        // Baseline: no prune — scans every block.
        let baseline = rt.block_on(crate::observability::io_trace::scope(async {
            let hits = rabitq_search_segment(&bytes, &query, K, POOL, RankMetric::L2, None)
                .unwrap()
                .expect("PAX+RaBitQ segment");
            let snap = crate::observability::io_trace::snapshot().unwrap();
            (hits, snap.logical_striped_bytes, snap.logical_striped_gets)
        }));

        // Pruned: created_at filter skips the early blocks before the vector pass.
        let pruned = rt.block_on(crate::observability::io_trace::scope(async {
            let mp = MetaPrune {
                filter: &filter,
                field_to_col: &f2c,
            };
            let hits = rabitq_search_segment(&bytes, &query, K, POOL, RankMetric::L2, Some(mp))
                .unwrap()
                .expect("PAX+RaBitQ segment");
            let snap = crate::observability::io_trace::snapshot().unwrap();
            (hits, snap.logical_striped_bytes, snap.logical_striped_gets)
        }));

        // (1) The round-trip win: pruning projects strictly fewer striped bytes.
        // The cascade's logical striped-read counters (`logical_striped_bytes` /
        // `_gets`, ADR-057 / TD-RDSTRAT-3) are always-on, so both are asserted.
        assert!(
            pruned.1 < baseline.1,
            "metadata prune must reduce logical striped bytes: pruned ({}) vs baseline ({})",
            pruned.1,
            baseline.1,
        );
        assert!(
            pruned.2 < baseline.2,
            "metadata prune must reduce logical striped GETs: pruned ({}) vs baseline ({})",
            pruned.2,
            baseline.2,
        );
        assert!(
            pruned.2 >= 1,
            "at least the surviving block must be scanned"
        );

        // (2) Soundness: the surviving block is not dropped — its vector is returned.
        assert!(
            pruned.0.iter().any(|h| h.oid == "v290"),
            "pruning must keep blocks that can match (v290 lives in a surviving block)"
        );
    }

    /// TD-RDSTRAT-3 S1b Slice C integration parity: the selective (striped)
    /// `rabitq_search_segment_ranged` — reading a real multi-block RaBitQ segment
    /// from disk via `LocalFileSystem::read_range` — must return **byte-identical**
    /// top-k `(oid, distance)` to the whole-segment `rabitq_search_segment` for the
    /// default SQ8 config (L2 + Cosine). This is the S1b integration gate; the SIFT1M
    /// recall ratchet is the CI-time gate.
    #[tokio::test]
    async fn ranged_cascade_matches_whole_segment() {
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let (dim, n) = (64usize, 400usize);
        let corpus: Vec<Vec<f32>> = (0..n)
            .map(|i| {
                (0..dim)
                    .map(|d| (((i * 131 + d * 17) % 251) as f32) * 0.01)
                    .collect()
            })
            .collect();
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| rec(&format!("r{i}"), 1000 + i as i64, v.clone()))
            .collect();
        // Small target block ⇒ a multi-block segment (exercises the ranged index walk).
        write_pax_segment_full(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            Some(16 * 1024),
        )
        .unwrap();
        let path_str = path.to_str().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();

        let query = corpus[137].clone();
        for metric in [RankMetric::L2, RankMetric::Cosine] {
            let whole = rabitq_search_segment(&bytes, &query, 10, 100, metric, None)
                .unwrap()
                .expect("whole cascade hits");
            let ranged = rabitq_search_segment_ranged(&fs, path_str, &query, 10, 100, metric, None)
                .await
                .unwrap()
                .expect("ranged cascade hits");
            let w: Vec<(String, f32)> = whole.iter().map(|h| (h.oid.clone(), h.distance)).collect();
            let r: Vec<(String, f32)> =
                ranged.iter().map(|h| (h.oid.clone(), h.distance)).collect();
            assert_eq!(
                r, w,
                "striped read must byte-identically match the whole segment ({metric:?})"
            );
        }
    }

    /// TD-RDSTRAT-5 S3: the block filter on the ranged reader. Selecting ALL block
    /// indices is byte-identical to `None` (parity); a strict subset scans only
    /// those blocks (fewer/equal hits); the empty set scans nothing (`None`).
    #[tokio::test]
    async fn ranged_block_filter_selects_only_chosen_blocks() {
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let (dim, n) = (64usize, 400usize);
        let corpus: Vec<Vec<f32>> = (0..n)
            .map(|i| {
                (0..dim)
                    .map(|d| (((i * 131 + d * 17) % 251) as f32) * 0.01)
                    .collect()
            })
            .collect();
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| rec(&format!("r{i}"), 1000 + i as i64, v.clone()))
            .collect();
        write_pax_segment_full(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            Some(16 * 1024),
        )
        .unwrap();
        let path_str = path.to_str().unwrap();
        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();
        let query = corpus[137].clone();
        let (n_blocks, _, _) = segment_index_summary(&fs, path_str)
            .await
            .unwrap()
            .expect("summary");
        assert!(
            n_blocks >= 2,
            "need a multi-block segment to exercise the filter"
        );
        let key = |h: &[CascadeHit]| {
            h.iter()
                .map(|x| (x.oid.clone(), x.distance))
                .collect::<Vec<_>>()
        };

        let none =
            rabitq_search_segment_ranged(&fs, path_str, &query, 10, 100, RankMetric::L2, None)
                .await
                .unwrap()
                .expect("hits");
        let all_sel: Vec<usize> = (0..n_blocks as usize).collect();
        let all = rabitq_search_segment_ranged(
            &fs,
            path_str,
            &query,
            10,
            100,
            RankMetric::L2,
            Some(&all_sel),
        )
        .await
        .unwrap()
        .expect("hits");
        assert_eq!(
            key(&none),
            key(&all),
            "selecting all blocks == None (parity)"
        );

        let b0 = rabitq_search_segment_ranged(
            &fs,
            path_str,
            &query,
            10,
            100,
            RankMetric::L2,
            Some(&[0]),
        )
        .await
        .unwrap()
        .expect("block 0 hits");
        assert!(!b0.is_empty(), "block 0 carries RaBitQ candidates");
        assert!(
            b0.len() <= none.len(),
            "a strict subset scans fewer-or-equal blocks"
        );

        let empty =
            rabitq_search_segment_ranged(&fs, path_str, &query, 10, 100, RankMetric::L2, Some(&[]))
                .await
                .unwrap();
        assert!(
            empty.is_none_or(|h| h.is_empty()),
            "empty selection scans no blocks ⇒ no hits"
        );
    }

    /// TD-RDSTRAT-3 S2: `segment_index_summary` reads the cost inputs from the tail
    /// index alone — block count + Σ block sizes + segment size — for a real
    /// multi-block segment, without touching stripe bytes.
    #[tokio::test]
    async fn segment_index_summary_reads_cost_inputs_from_tail() {
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let records: Vec<ProximaRecord> = (0..400)
            .map(|i| {
                rec(
                    &format!("r{i}"),
                    1000 + i as i64,
                    (0..64)
                        .map(|d| ((i * 131 + d * 17) % 251) as f32 * 0.01)
                        .collect(),
                )
            })
            .collect();
        write_pax_segment_full(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            Some(16 * 1024),
        )
        .unwrap();
        let file_size = std::fs::metadata(&path).unwrap().len();
        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();

        let (n_blocks, total_block_bytes, seg_size) =
            segment_index_summary(&fs, path.to_str().unwrap())
                .await
                .unwrap()
                .expect("index summary for a RaBitQ segment");
        assert!(
            n_blocks >= 2,
            "small block threshold ⇒ multi-block, got {n_blocks}"
        );
        assert_eq!(seg_size, file_size, "segment size = file size");
        // Σ block bytes < segment size (the index + magic are the remainder).
        assert!(
            total_block_bytes > 0 && total_block_bytes < seg_size,
            "Σ block bytes {total_block_bytes} must be in (0, {seg_size})"
        );
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
        const POOL: usize = 100;

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
        let hits = rabitq_search_segment_coalesced(
            &fs,
            path.to_str().unwrap(),
            &query,
            K,
            POOL,
            RankMetric::L2,
        )
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
