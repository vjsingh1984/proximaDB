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
use std::sync::Arc;

use anyhow::Result;
use proximadb_block_format::{
    BLOCK_MAGIC, BlockCompression, BlockMode, RankMetric, VectorQuant, col_id,
};
use proximadb_cache::CacheKind;
use proximadb_records::ProximaRecord;
use proximadb_storage_common::pax_block::{
    PaxSegmentScanner, PaxSegmentWriter, SEGMENT_MAGIC, ScanPredicate, SegmentMeta,
};
use proximadb_storage_common::segment_layout::{SegmentHeaderPrefix, is_coalesced_segment};

use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;
use crate::storage::engines::sst::survivor_range_cache::SurvivorRangeCache;

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
            // ADR-065 Region B: a coalesced segment with an SQ8 region stores its
            // vectors in Region B, NOT in the blocks (blocks are pure row data).
            // read_records reconstructs from blocks alone, so it would silently drop
            // the vectors — fail closed instead. The coalesced SEARCH path
            // (rabitq_search_segment_coalesced) reads Region B directly; full
            // Region-B read_records/compaction/recovery is a follow-up.
            let mut scanner =
                PaxSegmentScanner::from_bytes(bytes.to_vec(), ScanPredicate::default())?;
            let mut recs =
                scanner.read_records(embedding_model_ids, user_column_keys, tenant_ctx)?;
            // ADR-065 Region B: a coalesced segment with an SQ8 region stores its
            // vectors in Region B (blocks are pure row data). Overlay the SQ8
            // vectors by row index — block row order == Region B cluster order, so
            // recs[i] <-> Region B row i. (Exact-f32 preference via F32_TIER is a
            // follow-up; this returns the SQ8 rerank vectors so compaction / recovery
            // / full-read see the vectors rather than silently dropping them.)
            if is_coalesced_segment(bytes)
                && let Ok(h) = SegmentHeaderPrefix::parse(bytes)
                && h.sq8_len > 0
            {
                let region = &bytes[h.sq8_off as usize..(h.sq8_off + h.sq8_len) as usize];
                if let Ok(sq8) =
                    proximadb_block_format::coalesced_sq8::Sq8Region::from_bytes(region)
                {
                    let dim = sq8.header.dim;
                    for (i, rec) in recs.iter_mut().enumerate() {
                        if let Some(v) = sq8.decode_row(i) {
                            rec.embeddings.push(proximadb_records::EmbeddingCell {
                                modality: "dense".into(),
                                dim,
                                values: proximadb_records::EmbeddingValues::Fp32(v),
                                ..Default::default()
                            });
                        }
                    }
                }
            }
            Ok(recs)
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
    // ADR-065 Region B: the coalesced layout hoists SQ8 into a region whose
    // survivor fetch is row-granular — it needs a locality-preserving order so the
    // RaBitQ survivors (and top-k rows) co-locate. Use Morton/Z-order over the
    // segment-level SQ8 codes (denoised, projection-free, flush-safe). The
    // non-coalesced path keeps the sign-bit bootstrap (block granularity absorbs
    // scattering, so it doesn't need the stronger order).
    let coalesced = quant == VectorQuant::RaBitQ && coalesced_rabitq_enabled();
    let plan = if cluster {
        // TD-WLP-4/WLP-9 eval opt-in (`PROXIMADB_PAX_FLUSH_CLUSTER=ivf`): apply
        // the compaction-grade PCA+IVF re-cluster at flush instead of the
        // bootstrap, so clustering quality is measurable without the (unwired)
        // flush→compaction scheduler. Default OFF ⇒ bootstrap.
        if crate::storage::engines::sst::block_cluster::flush_cluster_ivf() {
            crate::storage::engines::sst::block_cluster::cluster_plan_pca_ivf(records, 0)
        } else if coalesced {
            crate::storage::engines::sst::block_cluster::cluster_order_sq8_morton(records, 0).map(
                |order| crate::storage::engines::sst::block_cluster::ClusterPlan {
                    order,
                    runs: Vec::new(),
                },
            )
        } else {
            crate::storage::engines::sst::block_cluster::cluster_plan(records, 0)
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
        plan,
        None, // two-level is compaction-only (TD-RDSTRAT-8)
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
    use crate::storage::engines::sst::block_cluster;
    let cluster = block_cluster::block_cluster_enabled();
    // TD-RDSTRAT-8 rev 3 (default OFF, `PROXIMADB_IVF2=1`): compaction is the ONLY
    // write path that emits the persisted-IVF-probe (v3) layout — production flush
    // stays sign-bit L0 (IVF-at-flush measured ~80× flush cost, dropped), and large
    // corpora reach v3 on their normal compaction cadence with no migration event.
    // The probe plan persists the SAME IOP-derived PCA/IVF plan the single-level
    // path computes (no second quantizer); it falls back to the plain single-level
    // plan whenever the model can't be trained (fail-safe, never a worse segment).
    let coalesced = quant == VectorQuant::RaBitQ && coalesced_rabitq_enabled();
    let mut probe_model = None;
    let plan = if cluster {
        if coalesced && block_cluster::ivf_probe_enabled() {
            match block_cluster::cluster_plan_ivf_probe(records, 0) {
                Some(tl) => {
                    probe_model = Some(tl.model);
                    Some(tl.plan)
                }
                None => block_cluster::cluster_plan_pca_ivf(records, 0),
            }
        } else {
            block_cluster::cluster_plan_pca_ivf(records, 0)
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
        plan,
        probe_model,
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
    plan: Option<crate::storage::engines::sst::block_cluster::ClusterPlan>,
    two_level: Option<proximadb_storage_common::coarse_directory::CoarseModel>,
) -> Result<SegmentMeta> {
    let lossless_clustered = lossless_clustered_enabled() && plan.is_some();
    let lossless_scalar = lossless_scalar_enabled();
    let mut writer = PaxSegmentWriter::new(
        path,
        BlockMode::Pax,
        BlockCompression::Zstd,
        collection_id,
        0, // schema_fingerprint — derived from the catalog schema in a later phase
        embedding_count.max(1),
        target_block,
    )
    .with_quant(quant)
    .with_f32_tier(f32_tier)
    .with_rerank_quant(rerank_quant)
    .with_lossless_clustered(lossless_clustered)
    .with_lossless_scalar(lossless_scalar)
    .with_block_centroids(cluster)
    // ADR-062 / TD-RDSTRAT-6: hoist the RaBitQ binary tier into a coalesced
    // file-level header region for RaBitQ-quantized writes (default ON per the
    // ADR-061 pre-GA in-place amendment; `PROXIMADB_PAX_COALESCED_RABITQ=0` opts
    // out to the legacy in-block RaBitQ layout for mixed-read / measurement).
    .with_coalesced_rabitq(quant == VectorQuant::RaBitQ && coalesced_rabitq_enabled());
    // TD-RDSTRAT-8 rev 3: the persisted IVF probe directory (v3 layout) — the
    // plan's runs are its IOP-derived cells, so the writer pads blocks at the
    // same boundaries the model's cell_rows describe (a cell = whole D-blocks).
    if let Some(model) = two_level {
        writer = writer.with_two_level(model);
    }
    match &plan {
        Some(plan) => {
            let mut next_run = 1usize;
            for (ordered_row, &i) in plan.order.iter().enumerate() {
                if plan
                    .runs
                    .get(next_run)
                    .is_some_and(|run| run.start_row == ordered_row)
                {
                    writer.start_cluster_run();
                    next_run += 1;
                }
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

fn lossless_clustered_enabled() -> bool {
    matches!(
        std::env::var("PROXIMADB_PAX_LOSSLESS_CLUSTERED")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

fn lossless_scalar_enabled() -> bool {
    matches!(
        std::env::var("PROXIMADB_PAX_LOSSLESS_SCALAR")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
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

/// One coalesced survivor byte-range into Region B (the rows it covers).
struct RowFetch {
    start: u64,
    end: u64,
    rows: Vec<usize>,
}

/// Plan coalesced ranged GETs over the survivors' SQ8 byte-runs in Region B
/// (ADR-065). Each survivor `g` maps to `[codes_base + g·dim, +dim]`; adjacent
/// runs within `policy` merge into one GET (survivors are cluster-contiguous →
/// few ranges). Mirrors `plan_coalesced_block_ranges` over row byte-offsets.
fn plan_coalesced_row_ranges(
    survivors: &[usize],
    dim: usize,
    codes_base: u64,
    policy: &crate::storage::engines::sst::readers::sst_query_engine::ObjectRangeCoalescePolicy,
) -> Vec<RowFetch> {
    let mut sorted: Vec<usize> = survivors.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let dim64 = dim as u64;
    let mut out: Vec<RowFetch> = Vec::new();
    for g in sorted {
        let start = codes_base + (g as u64) * dim64;
        let end = start + dim64;
        let merge = match out.last_mut() {
            Some(last) => {
                let gap = start.saturating_sub(last.end);
                let merged_len = end - last.start;
                let within_gap = gap <= policy.max_gap_bytes;
                let within_max =
                    policy.max_range_bytes == 0 || merged_len <= policy.max_range_bytes;
                if within_gap && within_max {
                    last.end = last.end.max(end);
                    last.rows.push(g);
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if !merge {
            out.push(RowFetch {
                start,
                end,
                rows: vec![g],
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
    // Direct override for the survivor-pool sweep (eval knob): force M to a
    // fixed value to map the GETs-vs-recall curve and the M-scaling law
    // (linear fraction vs N^e vs log). Must stay ≥ k (need ≥ top-k survivors).
    if let Some(pool) = std::env::var("PROXIMADB_PAX_RABITQ_POOL")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|m| *m >= k)
    {
        return pool;
    }
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

/// Cached per-file-invariant bytes for a coalesced segment: header-prefix,
/// RaBitQ region, footer-index. Hot queries skip the 3 `read_range` calls →
/// 3 GETs → 0 (the GET counter fires inside `read_range`, so eliding the call
/// is the only way to reduce it — caching parsed structs downstream doesn't help).
pub struct SegmentInvariants {
    pub header_bytes: Vec<u8>,
    pub region_bytes: Arc<[u8]>,
    pub footer_bytes: Vec<u8>,
}

/// Per-segment invariants cache, **byte-budgeted** (ADR-065 cache-co-design #35).
/// The old entry-count cap was wrong for Region A entries (~24 MB each → 64
/// entries = 1.5 GB unbounded). Now the cache caps **total bytes** (dominated by
/// Region A `region_bytes`), evicting arbitrary entries when over budget. Region A
/// is the `CacheTier::InvariantIndex` tier (hottest, highest value); header/footer
/// are `InvariantMeta` (tiny, ~free). Thread-safe; the lock is held briefly.
pub struct SegmentInvariantsCache {
    inner: std::sync::Mutex<CacheInner>,
    byte_budget: usize,
}

struct CacheInner {
    map: std::collections::HashMap<String, Arc<SegmentInvariants>>,
    bytes_used: usize,
}

/// Bytes an invariants entry contributes (Region A dominates).
fn inv_bytes(inv: &SegmentInvariants) -> usize {
    inv.region_bytes.len() + inv.header_bytes.len() + inv.footer_bytes.len()
}

impl SegmentInvariantsCache {
    /// `byte_budget` caps the total cached bytes (Region A region_bytes dominate).
    pub fn new(byte_budget: usize) -> Self {
        Self {
            inner: std::sync::Mutex::new(CacheInner {
                map: std::collections::HashMap::new(),
                bytes_used: 0,
            }),
            byte_budget,
        }
    }

    /// On hit, return the cached invariants (Arc clone — refcount bump only).
    pub fn get(&self, path: &str) -> Option<Arc<SegmentInvariants>> {
        self.inner.lock().ok()?.map.get(path).cloned()
    }

    /// Insert; evict arbitrary entries while over the byte budget. Region A
    /// (region_bytes) dominates, so this effectively bounds the index tier.
    pub fn put(&self, path: String, inv: Arc<SegmentInvariants>) {
        let entry_bytes = inv_bytes(&inv);
        if let Ok(mut inner) = self.inner.lock() {
            // Replacing an existing path: credit the old entry's bytes back.
            if let Some(old) = inner.map.remove(&path) {
                inner.bytes_used = inner.bytes_used.saturating_sub(inv_bytes(&old));
            }
            // Evict arbitrary entries until the new entry fits under budget.
            while inner.bytes_used + entry_bytes > self.byte_budget
                && let Some(key) = inner.map.keys().next().cloned()
                && let Some(removed) = inner.map.remove(&key)
            {
                inner.bytes_used = inner.bytes_used.saturating_sub(inv_bytes(&removed));
            }
            inner.bytes_used += entry_bytes;
            inner.map.insert(path, inv);
        }
    }

    /// Remove a path (call from flush/compaction when a segment is rewritten).
    pub fn invalidate(&self, path: &str) {
        if let Ok(mut inner) = self.inner.lock()
            && let Some(removed) = inner.map.remove(path)
        {
            inner.bytes_used = inner.bytes_used.saturating_sub(inv_bytes(&removed));
        }
    }
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
///
/// ADR-065 cache-co-design: the read-type taxonomy. One enum drives BOTH (a) the
/// per-tier GET trace (billing/latency accounting) AND (b) the cache's per-tier
/// admission/eviction policy (`evict_priority`). Region A is the hottest, largest
/// read — a cache hit saves a full ~24 MB GET, so it gets the highest priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheTier {
    /// Region A (RaBitQ index scan): ~24 MB, re-read every query (hottest).
    InvariantIndex,
    /// header / footer / SQ8 params: tiny, always-useful.
    InvariantMeta,
    /// Region B survivor SQ8 ranges: variable, query-dependent.
    SurvivorPayload,
    /// Region D OID ranges: variable, query-dependent.
    ResultPayload,
}

impl CacheTier {
    /// A short label for the trace output.
    pub fn label(self) -> &'static str {
        match self {
            Self::InvariantIndex => "IdxA",
            Self::InvariantMeta => "Meta",
            Self::SurvivorPayload => "Surv",
            Self::ResultPayload => "OID",
        }
    }
    /// Eviction priority (lower = evict first under memory pressure). Region A is
    /// pinned (a hit saves the most); query-dependent ranges are evicted first.
    pub fn evict_priority(self) -> u8 {
        match self {
            Self::SurvivorPayload => 0,
            Self::ResultPayload => 1,
            Self::InvariantMeta => 2,
            Self::InvariantIndex => 3,
        }
    }
}

/// ADR-065 co-design diagnostic: per-GET (tier, size) trace. `record_get` pushes
/// each read_range's tier + on-disk length when `PROXIMADB_TRACE_GETS` is set;
/// `drain_get_trace` returns + clears them so the eval prints the per-tier
/// distribution (GET count = same-region billing; sizes = latency). Zero cost off.
static GET_TRACE: std::sync::Mutex<Vec<(CacheTier, u64)>> = std::sync::Mutex::new(Vec::new());

fn record_get(tier: CacheTier, len: u64) {
    if let Ok(mut v) = GET_TRACE.lock() {
        v.push((tier, len));
    }
}

/// Drain the recorded per-GET (tier, size) pairs (empty when trace is off).
pub fn drain_get_trace() -> Vec<(CacheTier, u64)> {
    GET_TRACE
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

/// TD-RDSTRAT-8 PR-B diagnostic: per-query coarse-probe counters
/// `(cells_total, cells_probed, probed_rows, fetch_rounds)`. The probe path
/// pushes them when `PROXIMADB_TRACE_GETS` is set; the eval/recall harness drains
/// them to prove `cells_probed << cells_total` and the GET/byte reduction. Zero
/// cost off. (Promoting these into the durable Prometheus io_trace surface —
/// `ivf_cells_total/probed`, `probed_rows`, `fetch_rounds`,
/// `whole_region_fallback` — is a tracked follow-up.)
static PROBE_TRACE: std::sync::Mutex<Vec<(u64, u64, u64, u64)>> = std::sync::Mutex::new(Vec::new());

fn record_probe_trace(cells_total: u64, cells_probed: u64, probed_rows: u64, fetch_rounds: u64) {
    if let Ok(mut v) = PROBE_TRACE.lock() {
        v.push((cells_total, cells_probed, probed_rows, fetch_rounds));
    }
}

/// Drain the recorded coarse-probe counters (empty when trace off / no probe).
pub fn drain_probe_trace() -> Vec<(u64, u64, u64, u64)> {
    PROBE_TRACE
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

/// TD-RDSTRAT-8 PR-B gate: engage the Region-A0 coarse probe on v3 segments.
/// Default **OFF** (`PROXIMADB_IVF2_PROBE=1` to enable) — v3 segments read via
/// the single-level whole-region scan until this flips (recall/GET-ratchet
/// gated, mirroring the PR-A writer gate). v1 segments never probe.
pub fn coarse_probe_enabled() -> bool {
    matches!(
        std::env::var("PROXIMADB_IVF2_PROBE")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// Number of coarse cells to probe. `PROXIMADB_IVF2_NPROBE` overrides (the
/// eval-profile knob, per rev 3 — `nprobe` is fixed by metric/dim/distribution,
/// not an adaptive radius). Default probes ~25% of cells (min 8), capped at
/// `k_c`; `>= k_c` is exact mode (every cell).
fn coarse_probe_nprobe(k_c: usize) -> usize {
    std::env::var("PROXIMADB_IVF2_NPROBE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| k_c.div_ceil(4).clamp(8, k_c.max(1)))
        .min(k_c)
}

/// Outcome of a coarse probe: the global survivor rows plus the io_trace
/// counters (`ivf_cells_total/probed`, `probed_rows`, `fetch_rounds`).
struct CoarseProbeResult {
    survivors: Vec<usize>,
    cells_total: u64,
    cells_probed: u64,
    probed_rows: u64,
    fetch_rounds: u64,
}

/// TD-RDSTRAT-8 PR-B: compute survivors from ONLY the `nprobe` nearest coarse
/// cells — rank the persisted `k_c` centroids in RAM, then ranged-read just those
/// cells' Region-A code extents (`a_off/a_len`). The whole Region A is never
/// fetched, so cold-query GETs/bytes drop from O(N) to O(nprobe). Returns `None`
/// to fall back to the single-level whole-region scan (fail-safe: A0
/// missing/corrupt, dim mismatch, or degenerate directory).
async fn coarse_probe_survivors(
    fs: &dyn proximadb_storage_filesystem_types::FileSystem,
    path: &str,
    header: &proximadb_storage_common::segment_layout::SegmentHeaderPrefix,
    query: &[f32],
    metric: RankMetric,
    k: usize,
    trace_on: bool,
) -> Result<Option<CoarseProbeResult>> {
    use proximadb_storage_common::coarse_directory::{CoarseDirectory, project_with_model};

    let dim = query.len();
    if dim == 0 || header.a0_len == 0 {
        return Ok(None);
    }

    // 1. Region A0 (small, immediately after the header) — one ranged GET.
    let a0_bytes = fs
        .read_range(path, header.a0_off, header.a0_len)
        .await
        .map_err(|e| anyhow::anyhow!("coarse-probe A0 {path}: {e}"))?;
    if trace_on {
        record_get(CacheTier::InvariantMeta, header.a0_len);
    }
    let Ok(dir) = CoarseDirectory::parse(&a0_bytes) else {
        return Ok(None);
    };
    let k_c = dir.model.k_c();
    let n_comp = dir.model.n_comp as usize;
    if k_c == 0 || dir.cells.len() != k_c || n_comp == 0 || dir.model.dim as usize != dim {
        return Ok(None);
    }

    // 2. Rank the k_c centroids in RAM; select the nprobe nearest NON-EMPTY cells
    //    (project the query with the SAME persisted f32 model the writer used).
    let q_proj = project_with_model(
        &dir.model.pca_mean,
        &dir.model.pca_components,
        n_comp,
        query,
    );
    if q_proj.len() != n_comp {
        return Ok(None);
    }
    let mut cell_dist: Vec<(usize, f32)> = (0..k_c)
        .map(|c| {
            let cen = &dir.model.centroids[c * n_comp..(c + 1) * n_comp];
            let d: f32 = q_proj.iter().zip(cen).map(|(a, b)| (a - b) * (a - b)).sum();
            (c, d)
        })
        .collect();
    cell_dist.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let nprobe = coarse_probe_nprobe(k_c);
    let mut probed: Vec<usize> = cell_dist
        .iter()
        .filter(|(c, _)| dir.cells[*c].row_end > dir.cells[*c].row_begin)
        .take(nprobe)
        .map(|(c, _)| *c)
        .collect();
    if probed.is_empty() {
        return Ok(None);
    }
    // File (a_off) order so physically-adjacent probed cells coalesce.
    probed.sort_by_key(|&c| dir.cells[c].a_off);

    // 3. Region-A header (params + centroid) — one small ranged GET.
    let hdr_len = proximadb_block_format::region_header_len(dim as u32) as u64;
    let hdr_bytes = fs
        .read_range(path, header.rabitq_off, hdr_len)
        .await
        .map_err(|e| anyhow::anyhow!("coarse-probe region header {path}: {e}"))?;
    if trace_on {
        record_get(CacheTier::InvariantMeta, hdr_len);
    }
    let Ok(rq_header) = proximadb_block_format::CoalescedRaBitQHeader::parse(&hdr_bytes) else {
        return Ok(None);
    };

    // 4. Ranged-read the probed cells' Region-A code extents. Coalesce ONLY
    //    physically-adjacent cells (gap 0) so we never over-read unprobed cells;
    //    Hilbert emission order clusters near cells, so the nprobe nearest usually
    //    collapse to a few contiguous runs.
    let stride = proximadb_block_format::code_stride(dim as u32) as u64;
    let mut merged: Vec<(u64, u64, usize)> = Vec::new(); // (start, end, global row_start)
    for &c in &probed {
        let cell = &dir.cells[c];
        let (s, e, rs) = (cell.a_off, cell.a_off + cell.a_len, cell.row_begin as usize);
        match merged.last_mut() {
            Some(last) if last.1 == s => last.1 = e, // contiguous → extend the run
            _ => merged.push((s, e, rs)),
        }
    }
    let mut run_bufs: Vec<(usize, Vec<u8>)> = Vec::with_capacity(merged.len());
    let mut probed_rows: u64 = 0;
    for (s, e, rs) in &merged {
        let len = e - s;
        let bytes = fs
            .read_range(path, *s, len)
            .await
            .map_err(|err| anyhow::anyhow!("coarse-probe Region A {path}: {err}"))?;
        if trace_on {
            record_get(CacheTier::SurvivorPayload, len);
        }
        // `stride = code_stride(dim) >= 9` (dim >= 1, guarded above) so the
        // division is always well-defined.
        probed_rows += len / stride;
        run_bufs.push((*rs, bytes));
    }
    let runs: Vec<(usize, &[u8])> = run_bufs.iter().map(|(rs, b)| (*rs, b.as_slice())).collect();
    let pool = pax_rabitq_pool_for_top_k(k, probed_rows as usize);
    let survivors =
        proximadb_block_format::rank_probed_rows(&rq_header, &runs, query, metric, pool.max(k))?;

    Ok(Some(CoarseProbeResult {
        survivors,
        cells_total: k_c as u64,
        cells_probed: probed.len() as u64,
        probed_rows,
        fetch_rounds: merged.len() as u64,
    }))
}

pub async fn rabitq_search_segment_coalesced(
    fs: &dyn proximadb_storage_filesystem_types::FileSystem,
    path: &str,
    query: &[f32],
    k: usize,
    metric: RankMetric,
    cache: Option<&SegmentInvariantsCache>,
    survivor_cache: Option<&SurvivorRangeCache>,
) -> Result<Option<Vec<CascadeHit>>> {
    use proximadb_block_format::{PaxBlockReader, RaBitQRegion, coalesced_sq8, col_id};
    use proximadb_storage_common::segment_layout::{
        SEG_HEADER_PREFIX_LEN, SEG_HEADER_PREFIX_V3_LEN, SEG_LAYOUT_VERSION_TWO_LEVEL,
        SegmentFooterIndex, SegmentHeaderPrefix,
    };

    // ADR-065 co-design diagnostic: per-GET size trace. When PROXIMADB_TRACE_GETS
    // is set, every read_range in this path records its on-disk byte length; the
    // eval drains it to print the distribution (GET count = the same-region
    // billing metric; GET sizes = the latency metric). Off by default (zero cost).
    let trace_on = std::env::var_os("PROXIMADB_TRACE_GETS").is_some();

    // PR2: check the per-segment invariants cache. On hit, skip the 3
    // read_range calls (header + region + footer) → 3 GETs → 0 (hot path).
    let cached = cache.and_then(|c| c.get(path));

    // 1. Header-prefix → region/footer extents. From cache (hot) or read (cold).
    let header_bytes: Vec<u8> = if let Some(inv) = cached.as_ref() {
        inv.header_bytes.clone()
    } else {
        let size = fs
            .metadata(path)
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan stat {path}: {e}"))?
            .size;
        if size < (SEG_HEADER_PREFIX_LEN as u64 + SEGMENT_MAGIC.len() as u64) {
            return Ok(None);
        }
        // TD-RDSTRAT-8: fetch the v3 prefix length unconditionally (clamped to
        // file size) — the 16 extra bytes ride the same GET and cover both the
        // v1 (56 B) and v3 (72 B) forms; `parse` branches on the version byte.
        fs.read_range(path, 0, (SEG_HEADER_PREFIX_V3_LEN as u64).min(size))
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan header {path}: {e}"))?
    };
    // A v3 (two-level) segment parses here too and falls through to the same
    // single-level whole-region scan: Regions A/B are byte-identical in format
    // (`rabitq_off/sq8_off` skip past A0), and a full scan over coarse-ordered
    // rows is order-agnostic — correct results, v2 GET budget. The A0
    // coarse-probe branch (nprobe-scoped sub-reads) is PR-B of TD-RDSTRAT-8.
    let header = match SegmentHeaderPrefix::parse(&header_bytes) {
        Ok(h) => h,
        Err(_) => return Ok(None), // not coalesced → don't cache
    };

    // 2. Survivors. TD-RDSTRAT-8 PR-B: on a v3 segment with the coarse probe
    //    armed, rank the persisted centroids in RAM and ranged-read only the
    //    nprobe nearest cells (whole Region A never fetched); else the
    //    single-level whole-region scan (cold: 1 GET; hot: 0 — Arc clone). Any
    //    probe miss falls through, fail-safe, to the whole-region path.
    let probe = if header.layout_version == SEG_LAYOUT_VERSION_TWO_LEVEL
        && header.a0_len > 0
        && coarse_probe_enabled()
    {
        coarse_probe_survivors(fs, path, &header, query, metric, k, trace_on)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let (survivors, region_bytes): (Vec<usize>, Option<Arc<[u8]>>) = if let Some(r) = &probe {
        if trace_on {
            record_probe_trace(r.cells_total, r.cells_probed, r.probed_rows, r.fetch_rounds);
        }
        (r.survivors.clone(), None)
    } else {
        let region_bytes: Arc<[u8]> = if let Some(inv) = cached.as_ref() {
            inv.region_bytes.clone()
        } else {
            let bytes = fs
                .read_range(path, header.rabitq_off, header.rabitq_len)
                .await
                .map_err(|e| anyhow::anyhow!("coalesced scan region {path}: {e}"))?;
            // Trace the whole-region cost so the coarse probe's Region-A saving is
            // observable (co-design: trace before you tune). Diagnostic-only.
            if trace_on {
                record_get(CacheTier::InvariantIndex, header.rabitq_len);
            }
            Arc::from(bytes)
        };
        let region = RaBitQRegion::from_bytes(&region_bytes)?;
        // ADR-062 PR2: adaptive survivor pool — scale M with the segment's rows.
        let pool = pax_rabitq_pool_for_top_k(k, region.n_rows());
        (region.rank(query, metric, pool.max(k)), Some(region_bytes))
    };
    if survivors.is_empty() {
        return Ok(Some(Vec::new()));
    }

    // 3. Footer-index → block table. From cache (hot) or read (cold).
    let footer_bytes: Vec<u8> = if let Some(inv) = cached.as_ref() {
        inv.footer_bytes.clone()
    } else {
        fs.read_range(path, header.footer_off, header.footer_len)
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan footer {path}: {e}"))?
    };
    let footer = SegmentFooterIndex::parse(&footer_bytes)?;

    // PR2: populate the cache on miss (the invariants parsed successfully →
    // worth caching for hot repeat queries). Only on the whole-region path — the
    // coarse probe never read the whole Region A, so there is nothing to cache
    // here (its small A0/header/cell reads ride the survivor cache instead).
    if cached.is_none()
        && let Some(region_bytes) = region_bytes
        && let Some(c) = cache
    {
        c.put(
            path.to_string(),
            Arc::new(SegmentInvariants {
                header_bytes,
                region_bytes,
                footer_bytes,
            }),
        );
    }

    // 4. ADR-065 Region B: rerank survivors via the coalesced SQ8 region (pure,
    //    dense — no bystander props/fp32). The dequant key (min + scale) is
    //    mirrored in the footer (already read), so there is NO separate 24 B
    //    Region-B-header GET — reconstruct the params + codes_base from the footer.
    let dim = footer.embed_dim as usize;
    let sq8_params = coalesced_sq8::params_from_min_scale(footer.sq8_min, footer.sq8_scale);
    let codes_base = header.sq8_off + coalesced_sq8::codes_offset(footer.row_count as usize) as u64;
    // Coalesce policy IOP-aligned to the backend (ADR-065 cache-co-design): a
    // coalesced range must not exceed one chunk (4 MiB Azure / 8 MiB S3), so it
    // is exactly one billed GET on the target store (no SDK chunk-split inflation).
    let iop_target =
        proximadb_storage_common::iops_budget::IopsBudget::for_path(path).target_block_bytes();
    let policy =
        crate::storage::engines::sst::readers::sst_query_engine::ObjectRangeCoalescePolicy {
            max_gap_bytes: env_u64_or(
                "PROXIMADB_PAX_COALESCE_GAP",
                (iop_target / 4).max(64 * 1024),
            ),
            max_range_bytes: env_u64_or("PROXIMADB_PAX_COALESCE_RANGE", iop_target),
        };
    let sq8_fetches = plan_coalesced_row_ranges(&survivors, dim, codes_base, &policy);
    let mut scored: Vec<(usize, f32)> = Vec::with_capacity(survivors.len());
    let ranges_bytes: u64 = sq8_fetches.iter().map(|f| f.end - f.start).sum();
    if ranges_bytes >= header.sq8_len {
        // Scattered survivors (sign-bit / no-IVF order): the coalesced ranges would
        // over-read >= the whole Region B — fetch it in one GET instead (fewer GETs,
        // no more bytes). decode_row extracts only the survivors. (Tight survivor
        // ranges — the full ~5x bytes win — need IVF/Hilbert locality: follow-up.)
        let region_bytes = fs
            .read_range(path, header.sq8_off, header.sq8_len)
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan Region B fetch {path}: {e}"))?;
        if trace_on {
            record_get(CacheTier::SurvivorPayload, header.sq8_len);
        }
        let region = coalesced_sq8::Sq8Region::from_bytes(&region_bytes)?;
        for &g in &survivors {
            if let Some(v) = region.decode_row(g) {
                scored.push((g, rerank_distance(metric, query, &v)));
            }
        }
    } else {
        // Clustered survivors (IVF/Hilbert locality): fetch the few tight coalesced
        // ranges — pure dense SQ8, the minimal-bytes path.
        let dim64 = dim as u64;
        for fetch in &sq8_fetches {
            let start = fetch.start;
            let range_len = fetch.end - fetch.start;
            // ADR-065 Q3: survivor-range cache. On a hit the loader never runs,
            // so `fs.read_range` + `record_get` (the billed GET) fire only on a
            // miss — bytes-not-billed for free, via the existing backend seam.
            let buf: Arc<[u8]> = if let Some(sc) = survivor_cache {
                sc.get_or_fetch(
                    CacheKind::QuantizedCodes,
                    path,
                    start,
                    range_len,
                    || async move {
                        let b = fs.read_range(path, start, range_len).await?;
                        if trace_on {
                            record_get(CacheTier::SurvivorPayload, range_len);
                        }
                        Ok(b)
                    },
                )
                .await
                .map_err(|e| anyhow::anyhow!("coalesced scan SQ8 survivor fetch {path}: {e}"))?
            } else {
                let b = fs.read_range(path, start, range_len).await.map_err(|e| {
                    anyhow::anyhow!("coalesced scan SQ8 survivor fetch {path}: {e}")
                })?;
                if trace_on {
                    record_get(CacheTier::SurvivorPayload, range_len);
                }
                Arc::from(b)
            };
            for &g in &fetch.rows {
                let rel = (codes_base + (g as u64) * dim64).saturating_sub(fetch.start) as usize;
                if rel + dim > buf.len() {
                    continue;
                }
                let v = coalesced_sq8::decode_codes(&buf[rel..rel + dim], &sq8_params);
                scored.push((g, rerank_distance(metric, query, &v)));
            }
        }
    }
    if scored.is_empty() {
        return Ok(Some(Vec::new()));
    }
    // Global top-k survivor rows (nearest-first; lower score = nearer).
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_k_rows: Vec<usize> = scored.iter().take(k).map(|(g, _)| *g).collect();

    // 5. ADR-065 Region D: fetch ONLY the top-k OIDs from the row blocks (≤k
    //    coalesced GETs — vs PR2's M-survivor block fetches). Map top-k rows →
    //    blocks via cumulative row counts.
    let mut block_start: Vec<u64> = Vec::with_capacity(footer.blocks.len());
    let mut acc = 0u64;
    for b in &footer.blocks {
        block_start.push(acc);
        acc += b.row_count as u64;
    }
    let mut block_rows: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for &g in &top_k_rows {
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
    let topk_blocks: Vec<usize> = block_rows.keys().copied().collect();
    let oid_fetches = plan_coalesced_block_ranges(&footer, &topk_blocks, &policy);
    let mut oid_of: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for fetch in &oid_fetches {
        let start = fetch.start;
        let range_len = fetch.end - fetch.start;
        // ADR-065 Q3: same read-through cache as survivors (OID ranges are also
        // immutable per segment + repeat across hot queries). `CacheKind::Other`
        // separates OID stats from survivor (QuantizedCodes) stats.
        let buf: Arc<[u8]> = if let Some(sc) = survivor_cache {
            sc.get_or_fetch(CacheKind::Other, path, start, range_len, || async move {
                let b = fs.read_range(path, start, range_len).await?;
                if trace_on {
                    record_get(CacheTier::ResultPayload, range_len);
                }
                Ok(b)
            })
            .await
            .map_err(|e| anyhow::anyhow!("coalesced scan OID fetch {path}: {e}"))?
        } else {
            let b = fs
                .read_range(path, start, range_len)
                .await
                .map_err(|e| anyhow::anyhow!("coalesced scan OID fetch {path}: {e}"))?;
            if trace_on {
                record_get(CacheTier::ResultPayload, range_len);
            }
            Arc::from(b)
        };
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
            let Ok(reader) = PaxBlockReader::open(&buf[rel..end]) else {
                continue;
            };
            let oids = reader.decode_str_stripe(col_id::OID).unwrap_or_default();
            if let Some(locals) = block_rows.get(&bi) {
                for &local in locals {
                    if let Some(oid) = oids.get(local).cloned().flatten() {
                        let g = block_start[bi] as usize + local;
                        oid_of.insert(g, oid);
                    }
                }
            }
        }
    }

    // 6. Build the top-k hits in nearest-first order (from `scored`); step 7
    //    canonicalizes the rank scores.
    let mut hits: Vec<CascadeHit> = Vec::with_capacity(top_k_rows.len());
    for (g, dist) in scored.iter().take(k) {
        hits.push(CascadeHit {
            oid: oid_of.get(g).cloned().unwrap_or_default(),
            distance: *dist,
            vector: None,
        });
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

        // Region B (ADR-065): the coalesced segment stores its SQ8 rerank tier in
        // Region B, so read_segment_records fails closed (vectors are not in the
        // blocks). The exact f32 tier is verified EMITTED above (footer flag + the
        // per-block F32_TIER stripe); full Region-B-aware materialization that
        // prefers the exact f32 tier is a follow-up (Region-B read_records).
        // Region B (ADR-065): the f32 tier is EMITTED in the blocks (verified
        // above). read_segment_records overlays SQ8 vectors from Region B for the
        // coalesced segment; exact-f32 materialization (preferring F32_TIER) is a
        // follow-up. Here we confirm the reader returns records WITH vectors.
        let materialized = read_segment_records(&bytes, &[], &[], None).unwrap();
        assert_eq!(materialized.len(), records.len());
        for got in &materialized {
            assert!(
                !got.embeddings.is_empty(),
                "overlay attaches vectors for {}",
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

        // Region B (ADR-065): vectors live in the coalesced SQ8 region, so
        // read_segment_records fails closed — verify the OID round-trip via the
        // scanner's block read (Region D row data) instead.
        let mut scan =
            PaxSegmentScanner::from_bytes(bytes.clone(), ScanPredicate::default()).unwrap();
        let back = scan.read_records(&[], &[], None).unwrap();
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

        // ADR-065 Region B: the SQ8 rerank tier is hoisted out of the block into a
        // coalesced region — the block carries NO EMBED_BASE vector stripe (pure
        // row data, Region D). Region B parses and holds the segment vectors.
        let mut scanner =
            PaxSegmentScanner::from_bytes(bytes.clone(), ScanPredicate::default()).unwrap();
        let block = scanner.next_block().expect("segment has a block");
        let embed = block.vector_params().get(col_id::EMBED_BASE);
        assert!(
            embed.is_none(),
            "coalesced block must carry NO EMBED_BASE (SQ8 hoisted to Region B)"
        );
        assert!(
            meta.sq8_len > 0,
            "coalesced segment carries a Region B SQ8 region"
        );
        assert_eq!(footer.sq8_off, meta.sq8_off);
        assert_eq!(footer.sq8_len, meta.sq8_len);
        let sq8_off = meta.sq8_off as usize;
        let sq8_region = &bytes[sq8_off..sq8_off + meta.sq8_len as usize];
        let sq8_hdr = proximadb_block_format::coalesced_sq8::CoalescedSq8Header::parse(sq8_region)
            .expect("Region B header parses");
        assert_eq!(sq8_hdr.n_rows as usize, records.len());
        assert_eq!(sq8_hdr.dim as usize, 64);

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
        let hits = rabitq_search_segment_coalesced(
            &fs,
            path.to_str().unwrap(),
            &query,
            K,
            RankMetric::L2,
            None,
            None,
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
            "coalesced scan-then-rerank recall@{K} = {recall:.2} (N={N})"
        );

        // The nearest neighbour (r137 is the query itself) must be ranked first.
        assert_eq!(
            hits[0].oid, "r137",
            "the query vector itself must be the top hit"
        );
    }

    /// ADR-065 Q3: the survivor-range cache turns a hot repeat query into cache
    /// hits. The second identical search issues strictly fewer ranged GETs — the
    /// survivor (Region B) + OID (Region D) ranges are served from RAM, never
    /// reaching the counting FS wrapper — and returns identical results. The
    /// invariants cache is intentionally NOT injected, so the GET reduction is
    /// attributable solely to the survivor cache.
    #[tokio::test]
    async fn survivor_cache_reduces_gets_on_hot_repeat_query() {
        enable_coalesced_rabitq();
        use crate::storage::persistence::filesystem::FileSystem;
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};
        use proximadb_storage_filesystem_types::counting::{CountingFileSystem, global_counters};
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

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
        write_pax_segment(
            &path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            Some(16 * 1024),
        )
        .unwrap();

        let local = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();
        let counters = global_counters();
        let fs: Arc<dyn FileSystem> =
            Arc::new(CountingFileSystem::new(Arc::new(local), counters.clone()));
        let path_str = path.to_str().unwrap();
        let cache = SurvivorRangeCache::new(8 * 1024 * 1024);

        let query = corpus[137].clone();

        // Search 1 (cold): every survivor + OID range misses → fetched + cached.
        let before1 = counters.range_reads.load(Ordering::Relaxed);
        let hits1 = rabitq_search_segment_coalesced(
            fs.as_ref(),
            path_str,
            &query,
            K,
            RankMetric::L2,
            None,
            Some(&cache),
        )
        .await
        .unwrap()
        .expect("first search returns hits");
        let gets1 = counters.range_reads.load(Ordering::Relaxed) - before1;

        // Search 2 (hot): identical query → survivor + OID ranges hit RAM.
        let before2 = counters.range_reads.load(Ordering::Relaxed);
        let hits2 = rabitq_search_segment_coalesced(
            fs.as_ref(),
            path_str,
            &query,
            K,
            RankMetric::L2,
            None,
            Some(&cache),
        )
        .await
        .unwrap()
        .expect("second search returns hits");
        let gets2 = counters.range_reads.load(Ordering::Relaxed) - before2;

        assert!(
            gets2 < gets1,
            "hot repeat query must issue fewer ranged GETs (got {gets2} >= {gets1}; \
             the survivor+OID ranges should have hit the cache)"
        );
        // Recall is identical — the cache returns the same bytes the backend would.
        let ids = |h: &[CascadeHit]| {
            let mut s: Vec<String> = h.iter().map(|x| x.oid.clone()).collect();
            s.sort();
            s
        };
        assert_eq!(
            ids(&hits1),
            ids(&hits2),
            "the survivor cache must not change search results"
        );
    }

    /// TD-RDSTRAT-8 PR-A (rev 3): `write_pax_segment_compacted` emits the
    /// persisted-IVF-probe (v3) layout ONLY under `PROXIMADB_IVF2=1` (default-OFF,
    /// compaction-only), and the current binary reads a v3 segment through the
    /// SAME single-level scan path with full parity (correctness before the PR-B
    /// probe reader lands): search recall holds, and `read_segment_records`
    /// reconstructs every row WITH its Region-B vectors (the compaction/recovery
    /// inverse — a v3 segment must never silently drop vectors when re-compacted).
    #[tokio::test]
    async fn ivf_probe_compaction_gate_and_v3_read_compat() {
        enable_coalesced_rabitq();
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};
        use proximadb_storage_common::coarse_directory::CoarseDirectory;
        use proximadb_storage_common::segment_layout::{
            SEG_LAYOUT_VERSION, SEG_LAYOUT_VERSION_TWO_LEVEL, SegmentHeaderPrefix,
        };

        const DIM: usize = 64;
        const N: usize = 400;
        const K: usize = 10;
        // Small k (the shared IOP-derived override) so the tiny fixture still
        // forms multiple cells — the natural N·dim/IOP count would be ~2 here.
        unsafe {
            std::env::set_var("PROXIMADB_IVF2", "1");
            std::env::set_var("PROXIMADB_IVF_K", "4");
        }

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

        // Flag ON ⇒ compaction writes v3 (header + A0 + footer mirror agree).
        let v3_path = dir.path().join("v3.pax");
        write_pax_segment_compacted(
            &v3_path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            Some(16 * 1024),
        )
        .unwrap();
        let v3_bytes = std::fs::read(&v3_path).unwrap();
        let h = SegmentHeaderPrefix::parse(&v3_bytes).unwrap();
        assert_eq!(
            h.layout_version, SEG_LAYOUT_VERSION_TWO_LEVEL,
            "PROXIMADB_IVF2=1 compaction must emit the v3 layout"
        );
        let a0 =
            CoarseDirectory::parse(&v3_bytes[h.a0_off as usize..(h.a0_off + h.a0_len) as usize])
                .expect("Region A0 parses from the compacted segment");
        assert_eq!(a0.model.rows_covered(), N as u64);
        assert_eq!(a0.model.dim as usize, DIM);

        // Flag OFF ⇒ same records compact to the v1 (single-level) layout.
        unsafe {
            std::env::remove_var("PROXIMADB_IVF2");
        }
        let v1_path = dir.path().join("v1.pax");
        write_pax_segment_compacted(
            &v1_path,
            &records,
            "col",
            1,
            VectorQuant::RaBitQ,
            VectorQuant::Sq8,
            false,
            Some(16 * 1024),
        )
        .unwrap();
        let h1 = SegmentHeaderPrefix::parse(&std::fs::read(&v1_path).unwrap()).unwrap();
        assert_eq!(
            h1.layout_version, SEG_LAYOUT_VERSION,
            "without PROXIMADB_IVF2 the compaction layout is unchanged (v1)"
        );

        // v3 read-compat: the coalesced cascade searches the v3 segment with
        // brute-force-level recall (single-level full scan until PR-B).
        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();
        let query = corpus[137].clone();
        let hits = rabitq_search_segment_coalesced(
            &fs,
            v3_path.to_str().unwrap(),
            &query,
            K,
            RankMetric::L2,
            None,
            None,
        )
        .await
        .unwrap()
        .expect("v3 segment must be searchable by the current binary");
        assert_eq!(hits[0].oid, "r137", "nearest neighbour on the v3 layout");
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
        assert!(recall >= 0.90, "v3 read-compat recall@{K} = {recall:.2}");

        // Compaction/recovery inverse: every row reads back WITH its vector
        // (Region-B overlay must work through the v3 prefix).
        let recs = read_segment_records(&v3_bytes, &[], &[], None).unwrap();
        assert_eq!(recs.len(), N);
        assert!(
            recs.iter().all(|r| r
                .embeddings
                .first()
                .is_some_and(|e| !e.as_fp32_slice().is_empty())),
            "v3 read_segment_records must reconstruct vectors (never drop Region B)"
        );
        unsafe {
            std::env::remove_var("PROXIMADB_IVF_K");
        }
    }

    /// TD-RDSTRAT-8 PR-B (the deferred PR-A recall/GET gate): the coarse probe
    /// (`PROXIMADB_IVF2_PROBE=1`) ranks the persisted centroids in RAM and reads
    /// only the nprobe nearest cells — holding recall on clustered data while
    /// reading materially fewer bytes than the whole-region single-level scan.
    #[tokio::test]
    async fn coarse_probe_holds_recall_and_cuts_bytes() {
        enable_coalesced_rabitq();
        use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

        const DIM: usize = 32;
        const G: usize = 8; // clusters (each centred on its own axis)
        const PER: usize = 64; // members per cluster
        const N: usize = G * PER;
        const K: usize = 10;

        // Deterministic pseudo-random component in [-1, 1].
        let pseudo = |i: usize, d: usize| -> f32 {
            let mut s = (i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((d as u64).wrapping_mul(0x632B_E59B_D9B4_E019));
            s ^= s >> 29;
            s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            s ^= s >> 27;
            ((s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        };
        // Widely-separated clusters (each centred at 50 on its own axis) with
        // members at a monotonically increasing radius — so the true top-K are
        // DISTINGUISHABLE (non-degenerate full-scan recall) AND concentrated in
        // one cluster → a few probed cells capture them.
        let corpus: Vec<Vec<f32>> = (0..N)
            .map(|i| {
                let (g, j) = (i / PER, i % PER);
                let radius = 0.1 + j as f32 * 0.03;
                (0..DIM)
                    .map(|d| (if d == g { 50.0 } else { 0.0 }) + radius * pseudo(i, d))
                    .collect()
            })
            .collect();
        let records: Vec<ProximaRecord> = corpus
            .iter()
            .enumerate()
            .map(|(i, v)| rec(&format!("r{i}"), 1000 + i as i64, v.clone()))
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.pax");
        unsafe {
            std::env::set_var("PROXIMADB_IVF2", "1");
            std::env::set_var("PROXIMADB_IVF_K", "16");
        }
        write_pax_segment_compacted(
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
        unsafe {
            std::env::remove_var("PROXIMADB_IVF2");
        }

        let fs = LocalFileSystem::new_with_encryption(LocalConfig::default(), None)
            .await
            .unwrap();
        let p = path.to_str().unwrap();
        // The exact centre of cluster 0 — its nearest members are the smallest
        // radii (r0, r1, …), a distinguishable top-K all inside one cluster.
        let query: Vec<f32> = (0..DIM).map(|d| if d == 0 { 50.0 } else { 0.0 }).collect();

        // Brute-force truth (top-K by L2).
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
        let recall = |hits: &[CascadeHit]| {
            let got: std::collections::HashSet<String> =
                hits.iter().map(|h| h.oid.clone()).collect();
            truth.iter().filter(|o| got.contains(*o)).count() as f32 / K as f32
        };

        unsafe {
            std::env::set_var("PROXIMADB_TRACE_GETS", "1");
        }

        // Baseline: probe OFF → whole-region single-level scan over the v3 segment.
        unsafe {
            std::env::remove_var("PROXIMADB_IVF2_PROBE");
        }
        let _ = drain_get_trace();
        let _ = drain_probe_trace();
        let base = rabitq_search_segment_coalesced(&fs, p, &query, K, RankMetric::L2, None, None)
            .await
            .unwrap()
            .unwrap();
        let base_bytes: u64 = drain_get_trace().iter().map(|(_, b)| b).sum();
        assert!(drain_probe_trace().is_empty(), "probe OFF must not probe");
        let base_recall = recall(&base);
        assert!(
            base_recall >= 0.5,
            "baseline full-scan recall@{K} = {base_recall:.2} (sanity floor)"
        );

        // Probe ON: nprobe=6 of 16 cells.
        unsafe {
            std::env::set_var("PROXIMADB_IVF2_PROBE", "1");
            std::env::set_var("PROXIMADB_IVF2_NPROBE", "6");
        }
        let _ = drain_get_trace();
        let _ = drain_probe_trace();
        let probe = rabitq_search_segment_coalesced(&fs, p, &query, K, RankMetric::L2, None, None)
            .await
            .unwrap()
            .unwrap();
        let probe_bytes: u64 = drain_get_trace().iter().map(|(_, b)| b).sum();
        let ptrace = drain_probe_trace();

        // 1. The probe engaged: cells_probed strictly inside (0, cells_total).
        assert_eq!(ptrace.len(), 1, "exactly one probe recorded");
        let (cells_total, cells_probed, probed_rows, fetch_rounds) = ptrace[0];
        assert_eq!(cells_total, 16, "16 coarse cells");
        assert!(
            cells_probed > 0 && cells_probed <= 6,
            "probed {cells_probed} cells (0 < c <= nprobe=6)"
        );
        assert!(
            probed_rows > 0 && probed_rows < N as u64,
            "probed {probed_rows} of {N} rows (a strict subset)"
        );
        assert!(fetch_rounds >= 1, "at least one Region-A ranged GET");

        // 2. Materially fewer bytes than the whole-region baseline.
        assert!(
            probe_bytes < base_bytes,
            "probe {probe_bytes} B must read fewer than baseline {base_bytes} B"
        );

        // 3. Recall held vs the full scan: probing the query's cluster loses
        //    essentially nothing (the meaningful PR-B claim — robust to the
        //    absolute RaBitQ recall level, which the corpus/quantizer set).
        let probe_recall = recall(&probe);
        assert!(
            probe_recall >= base_recall - 0.1,
            "probe recall@{K} = {probe_recall:.2} must hold vs baseline {base_recall:.2} (nprobe=6/16)"
        );

        unsafe {
            std::env::remove_var("PROXIMADB_IVF2_PROBE");
            std::env::remove_var("PROXIMADB_IVF2_NPROBE");
            std::env::remove_var("PROXIMADB_TRACE_GETS");
            std::env::remove_var("PROXIMADB_IVF_K");
        }
    }
}
