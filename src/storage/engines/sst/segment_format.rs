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
use proximadb_block_format::{BLOCK_MAGIC, BlockCompression, BlockMode, VectorQuant, col_id};
use proximadb_filter_expression::FilterExpression;
use proximadb_records::ProximaRecord;
use proximadb_storage_common::pax_block::{
    PaxSegmentScanner, PaxSegmentWriter, SEGMENT_MAGIC, ScanPredicate, SegmentMeta,
};

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

/// True iff `bytes` is a PAX segment: leading PAX block magic AND trailing segment
/// magic. Both ends are checked so a stray prefix or suffix alone can't false-positive.
fn is_pax_segment(bytes: &[u8]) -> bool {
    bytes.len() >= BLOCK_MAGIC.len() + SEGMENT_MAGIC.len()
        && bytes.starts_with(&BLOCK_MAGIC)
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
pub fn read_segment_records(
    bytes: &[u8],
    embedding_model_ids: &[String],
    user_column_keys: &[String],
) -> Result<Vec<ProximaRecord>> {
    match SegmentFormat::detect(bytes) {
        SegmentFormat::Pax => {
            let mut scanner =
                PaxSegmentScanner::from_bytes(bytes.to_vec(), ScanPredicate::default())?;
            scanner.read_records(embedding_model_ids, user_column_keys)
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
pub fn write_pax_segment(
    path: &Path,
    records: &[ProximaRecord],
    collection_id: &str,
    embedding_count: usize,
    quant: VectorQuant,
) -> Result<SegmentMeta> {
    let mut writer = PaxSegmentWriter::new(
        path,
        BlockMode::Pax,
        BlockCompression::None,
        collection_id,
        0, // schema_fingerprint — derived from the catalog schema in a later phase
        embedding_count.max(1),
        None,
    )
    .with_quant(quant);
    for record in records {
        writer.add_record(record)?;
    }
    writer.finish()
}

/// One scored hit from a RaBitQ cascade segment scan: the record's `oid` and its
/// L2 distance to the query (smaller = nearer), reranked against the co-located
/// SQ8 column.
#[derive(Debug, Clone)]
pub struct CascadeHit {
    pub oid: String,
    pub distance: f32,
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
        let Some(cand) = block.rabitq_rank(query, pool) else {
            continue;
        };
        any_rabitq = true;

        // Trace the cascade's logical reads for this kept block: the RaBitQ codes
        // stripe (stage 1) + the SQ8 bytes decoded for the candidate pool (stage 2).
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
        io_trace::record_bytes_read(codes_len + cand.len() as u64 * dim);
        io_trace::record_range_gets(1);

        // Stage 2: SQ8 cascade rerank over ONLY the candidate rows (no f32 GET).
        // Without a co-located SQ8 column, keep the RaBitQ-coarse order.
        let scored = block.rerank_rows(0, query, &cand).unwrap_or_else(|| {
            cand.iter()
                .enumerate()
                .map(|(rank, &row)| (row, rank as f32))
                .collect()
        });
        let oids = block.decode_str_stripe(col_id::OID);
        for (row, dist) in scored.into_iter().take(k) {
            let oid = oids
                .as_ref()
                .and_then(|o| o.get(row).cloned().flatten())
                .unwrap_or_default();
            hits.push(CascadeHit {
                oid,
                distance: dist,
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

        let records = read_segment_records(&bytes, &[], &[]).unwrap();
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

        let back = read_segment_records(&bytes, &[], &[]).unwrap();
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
        let meta = write_pax_segment(&path, &records, "col", 1, VectorQuant::Auto).unwrap();
        assert!(
            meta.block_count >= 1,
            "segment should have at least one block"
        );

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(SegmentFormat::detect(&bytes), SegmentFormat::Pax);
        let back = read_segment_records(&bytes, &[], &[]).unwrap();
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
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ).unwrap();

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
        let back = read_segment_records(&bytes2, &[], &[]).unwrap();
        assert_eq!(back.len(), records.len());
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
        write_pax_segment(&path, &records, "col", 1, VectorQuant::RaBitQ).unwrap();
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

                let hits = rabitq_search_segment(&bytes, &query, K, POOL, None)
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
            let snap = crate::observability::io_trace::snapshot().unwrap();
            assert!(
                snap.bytes_read > 0,
                "io_trace must record bytes_read for the PAX cold scan"
            );
            assert!(
                snap.range_gets >= 1,
                "io_trace must record at least one range-get for the PAX cold scan"
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
            rabitq_search_segment(&legacy, &vec![0.0; DIM], K, POOL, None)
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
    #[test]
    fn rabitq_search_segment_metadata_pruning_skips_blocks() {
        use proximadb_filter_expression::ComparisonOperator;

        const DIM: usize = 32;
        const N: usize = 300;
        const K: usize = 10;
        const POOL: usize = 50;
        // created_at_ns = BASE_TS + i; the filter keeps only the tail.
        const BASE_TS: i64 = 1_000;
        const THRESHOLD: i64 = BASE_TS + 280; // records 280..299 survive

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
            w.add_record(&rec(&format!("v{i}"), BASE_TS + i as i64, v.clone()))
                .unwrap();
        }
        let meta = w.finish().unwrap();
        assert!(
            meta.block_count >= 3,
            "test needs multiple blocks, got {}",
            meta.block_count
        );
        let bytes = std::fs::read(&path).unwrap();

        // A query that lives in a SURVIVING block (created_at 1290 >= THRESHOLD).
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
            let hits = rabitq_search_segment(&bytes, &query, K, POOL, None)
                .unwrap()
                .expect("PAX+RaBitQ segment");
            let snap = crate::observability::io_trace::snapshot().unwrap();
            (hits, snap.bytes_read, snap.range_gets)
        }));

        // Pruned: created_at filter skips the early blocks before the vector pass.
        let pruned = rt.block_on(crate::observability::io_trace::scope(async {
            let mp = MetaPrune {
                filter: &filter,
                field_to_col: &f2c,
            };
            let hits = rabitq_search_segment(&bytes, &query, K, POOL, Some(mp))
                .unwrap()
                .expect("PAX+RaBitQ segment");
            let snap = crate::observability::io_trace::snapshot().unwrap();
            (hits, snap.bytes_read, snap.range_gets)
        }));

        // (1) The round-trip win: pruning reads strictly fewer bytes and gets.
        assert!(
            pruned.1 < baseline.1 && pruned.2 < baseline.2,
            "metadata prune must reduce I/O: pruned ({} bytes, {} gets) vs baseline \
             ({} bytes, {} gets)",
            pruned.1,
            pruned.2,
            baseline.1,
            baseline.2
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
}
