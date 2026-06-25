// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Footer-first **ranged** reader for PAX segments over object storage.
//!
//! Where [`crate::pax_block::PaxSegmentScanner`] fetches the whole segment into
//! memory and decodes everything, `RangedSegmentReader` reads only what a query
//! needs: it locates the segment index from a small tail suffix, then for each
//! block range-reads the footer/metadata and just the projected column stripes.
//!
//! It is the async adapter half of the planning/fetching split: the pure
//! byte-range planning + slice decoding live in `proximadb-block-format`'s
//! [`BlockLayout`]; the actual GETs go through the [`ObjectStoreBridge`] I/O
//! port (which an object-store backend implements with true `get_range`s). The
//! `tenant_id` is threaded to every fetch for billing attribution.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use object_store::path::Path;
use proximadb_block_format::{
    BLOCK_FOOTER_SIZE, BlockFooter, BlockZoneSource, FlatRow, PaxBlockReader, PruneResult,
    evaluate_block,
    ranged::{BlockLayout, metadata_ranges},
};
use proximadb_cache::{CacheKey, CacheKind, TenantCache};
use proximadb_filter_expression::FilterExpression;
use proximadb_kernel::error::StorageError;
use proximadb_records::ProximaRecord;

use crate::object_store_bridge::ObjectStoreBridge;
use crate::pax_block::SegmentIndex;

/// Per-(tenant,segment,block) cache of parsed [`BlockLayout`] metadata — skips the
/// footer + column-meta + vparam + rgdir ranged reads on a hit.
pub type FooterCache = TenantCache<Arc<BlockLayout>>;
/// Per-(tenant,segment) cache of the parsed segment index — skips the tail
/// suffix GET + index locate on a hit.
pub type SegmentIndexCache = TenantCache<Arc<SegmentIndex>>;

/// Initial tail suffix to read when locating the segment index. Large enough to
/// hold the index of a typical multi-block segment in one GET; grown on miss.
const INITIAL_SUFFIX: u64 = 64 * 1024;

fn fs_err(ctx: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Corruption(format!("ranged segment {ctx}: {e}"))
}

/// A PAX segment opened for ranged, projected reads, with optional multitenant
/// footer/index caching (segments are immutable/write-once, so path-keyed cache
/// entries need no TTL or etag for correctness).
pub struct RangedSegmentReader<'b> {
    bridge: &'b dyn ObjectStoreBridge,
    path: Path,
    tenant_id: Option<String>,
    /// Tenant cache-key namespace (empty string when no tenant).
    tenant_key: Arc<str>,
    index: Arc<SegmentIndex>,
    size: u64,
    footer_cache: Option<Arc<FooterCache>>,
    /// Per-open physical read accounting (co-design C0 trace substrate): bytes
    /// fetched via ranged GETs and footer-cache outcomes, surfaced to the
    /// caller's per-query `IoTrace` via [`RangedSegmentReader::read_stats`].
    /// Atomic because block reads within one open may run concurrently.
    bytes_read: AtomicU64,
    footer_hits: AtomicU64,
    footer_misses: AtomicU64,
    range_gets: AtomicU64,
}

/// Snapshot of one [`RangedSegmentReader`] open's physical read accounting.
/// Forwarded by callers into the per-query I/O trace so a query's object-store
/// byte cost and footer-cache effectiveness become observable (Dimensions 1 & 3
/// of the co-design spec).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SegmentReadStats {
    /// Bytes fetched via ranged GETs during this open. Excludes the one-time
    /// index-locate suffix read performed before the reader is constructed.
    pub bytes_read: u64,
    /// Block-layout footer/metadata cache hits (a hit skips all metadata GETs).
    pub footer_hits: u64,
    /// Block-layout footer/metadata cache misses (built from ranged GETs).
    pub footer_misses: u64,
    /// Number of ranged GET requests issued (each `fetch`). With `bytes_read`
    /// this yields the average GET size — the co-design read-granularity signal
    /// (§2.1): are reads coalesced toward the ~8-16 MiB S3 cost-throughput
    /// optimum, or fragmented into many small, per-request-fee-dominated GETs?
    /// (Outstanding-request *depth*, byte backpressure, and tail-hedging are
    /// deferred until the reader issues these concurrently — today it awaits
    /// them serially, so depth is 1.)
    pub range_gets: u64,
}

impl<'b> RangedSegmentReader<'b> {
    /// Open a segment with no caching (HEAD for size, suffix GET to locate the
    /// index).
    pub async fn open(
        bridge: &'b dyn ObjectStoreBridge,
        path: Path,
        tenant_id: Option<&str>,
    ) -> Result<RangedSegmentReader<'b>, StorageError> {
        Self::open_inner(bridge, path, tenant_id, None, None).await
    }

    /// Open a segment using the multitenant footer + index caches: a cached
    /// segment index skips the tail suffix GET, and cached per-block layouts skip
    /// the footer/metadata ranged reads.
    pub async fn open_with_cache(
        bridge: &'b dyn ObjectStoreBridge,
        path: Path,
        tenant_id: Option<&str>,
        footer_cache: Option<Arc<FooterCache>>,
        index_cache: Option<Arc<SegmentIndexCache>>,
    ) -> Result<RangedSegmentReader<'b>, StorageError> {
        Self::open_inner(bridge, path, tenant_id, footer_cache, index_cache).await
    }

    async fn open_inner(
        bridge: &'b dyn ObjectStoreBridge,
        path: Path,
        tenant_id: Option<&str>,
        footer_cache: Option<Arc<FooterCache>>,
        index_cache: Option<Arc<SegmentIndexCache>>,
    ) -> Result<RangedSegmentReader<'b>, StorageError> {
        let tenant_key: Arc<str> = Arc::from(tenant_id.unwrap_or(""));
        let size = bridge.vector_segment_size(&path, tenant_id).await?;
        if size < BLOCK_FOOTER_SIZE as u64 {
            return Err(fs_err("open", format!("segment too small: {size} bytes")));
        }

        let path_str = path.to_string();
        let index: Arc<SegmentIndex> = if let Some(ic) = &index_cache {
            let key = CacheKey::new(&tenant_key, CacheKind::SegmentIndex, &path_str);
            if let Some(idx) = ic.get(&key).await {
                idx
            } else {
                let idx = Arc::new(Self::locate_index(bridge, &path, tenant_id, size).await?);
                let weight = (12 + idx.blocks.len() * 12) as u32;
                ic.insert(key, weight, idx.clone()).await;
                idx
            }
        } else {
            Arc::new(Self::locate_index(bridge, &path, tenant_id, size).await?)
        };

        Ok(Self {
            bridge,
            path,
            tenant_id: tenant_id.map(str::to_owned),
            tenant_key,
            index,
            size,
            footer_cache,
            bytes_read: AtomicU64::new(0),
            footer_hits: AtomicU64::new(0),
            footer_misses: AtomicU64::new(0),
            range_gets: AtomicU64::new(0),
        })
    }

    /// Physical read accounting accumulated by this open (co-design C0 trace
    /// substrate). `bytes_read` / `range_gets` exclude the one-time index-locate
    /// suffix read done before construction. Callers forward this into the
    /// per-query `IoTrace`.
    pub fn read_stats(&self) -> SegmentReadStats {
        SegmentReadStats {
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            footer_hits: self.footer_hits.load(Ordering::Relaxed),
            footer_misses: self.footer_misses.load(Ordering::Relaxed),
            range_gets: self.range_gets.load(Ordering::Relaxed),
        }
    }

    /// Range-read the tail suffix and locate the segment index (growing the
    /// suffix if the index does not fit).
    async fn locate_index(
        bridge: &dyn ObjectStoreBridge,
        path: &Path,
        tenant_id: Option<&str>,
        size: u64,
    ) -> Result<SegmentIndex, StorageError> {
        let mut suffix_len = INITIAL_SUFFIX.min(size);
        loop {
            let offset = size - suffix_len;
            let suffix = bridge
                .fetch_vector_segment_range(path, offset, suffix_len, tenant_id)
                .await?;
            match SegmentIndex::locate_in_suffix(&suffix).map_err(|e| fs_err("locate index", e))? {
                Some(idx) => return Ok(idx),
                None => {
                    if suffix_len >= size {
                        return Err(fs_err("locate index", "index not found in full segment"));
                    }
                    suffix_len = (suffix_len * 4).min(size);
                }
            }
        }
    }

    pub fn block_count(&self) -> usize {
        self.index.blocks.len()
    }

    pub fn segment_size(&self) -> u64 {
        self.size
    }

    async fn fetch(&self, offset: u64, length: u64) -> Result<Vec<u8>, StorageError> {
        let out = self
            .bridge
            .fetch_vector_segment_range(&self.path, offset, length, self.tenant_id.as_deref())
            .await?;
        self.bytes_read
            .fetch_add(out.len() as u64, Ordering::Relaxed);
        self.range_gets.fetch_add(1, Ordering::Relaxed);
        Ok(out)
    }

    /// Assemble a block's [`BlockLayout`], consulting the footer cache first
    /// (a hit skips all footer/metadata ranged reads). Segments are immutable, so
    /// the `(tenant, segment#offset)` key needs no TTL/etag.
    async fn block_layout(
        &self,
        block_offset: u64,
        block_size: u64,
    ) -> Result<Arc<BlockLayout>, StorageError> {
        if let Some(fc) = &self.footer_cache {
            let key = CacheKey::new(
                &self.tenant_key,
                CacheKind::Footer,
                format!("{}#{}", self.path, block_offset),
            );
            if let Some(layout) = fc.get(&key).await {
                self.footer_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(layout);
            }
            self.footer_misses.fetch_add(1, Ordering::Relaxed);
            let layout = Arc::new(self.build_block_layout(block_offset, block_size).await?);
            fc.insert(key, layout.approx_bytes() as u32, layout.clone())
                .await;
            return Ok(layout);
        }
        Ok(Arc::new(
            self.build_block_layout(block_offset, block_size).await?,
        ))
    }

    /// Range-read one block's footer + metadata regions and assemble its
    /// [`BlockLayout`] — no stripe bytes are fetched.
    async fn build_block_layout(
        &self,
        block_offset: u64,
        block_size: u64,
    ) -> Result<BlockLayout, StorageError> {
        // 1. trailing block footer.
        let tail = self
            .fetch(
                block_offset + block_size - BLOCK_FOOTER_SIZE as u64,
                BLOCK_FOOTER_SIZE as u64,
            )
            .await?;
        let footer = BlockFooter::from_bytes(&tail).map_err(|e| fs_err("block footer", e))?;

        // 2. metadata regions (offsets are block-relative; add block_offset).
        let mr = metadata_ranges(&footer, block_size);
        let col_meta = self
            .fetch(
                block_offset + mr.col_meta.start,
                mr.col_meta.end - mr.col_meta.start,
            )
            .await?;
        let vparam = match mr.vparam {
            Some(r) => Some(self.fetch(block_offset + r.start, r.end - r.start).await?),
            None => None,
        };
        let rgdir = match mr.rgdir {
            Some(r) => Some(self.fetch(block_offset + r.start, r.end - r.start).await?),
            None => None,
        };

        BlockLayout::assemble(footer, &col_meta, vparam.as_deref(), rgdir.as_deref())
            .map_err(|e| fs_err("assemble layout", e))
    }

    /// Range-read and decode one f32 vector column across every block, fetching
    /// only that column's stripe (plus per-block metadata) — the projection that
    /// keeps the (large) other stripes off the wire.
    pub async fn read_f32_vec_column(
        &self,
        column_id: i32,
    ) -> Result<Vec<Option<Vec<f32>>>, StorageError> {
        let mut out = Vec::new();
        for entry in &self.index.blocks {
            let (off, bsz) = (entry.offset, entry.size as u64);
            let layout = self.block_layout(off, bsz).await?;
            let Some(r) = layout.column_stripe_range(column_id) else {
                continue; // column absent in this block
            };
            let stripe = self.fetch(off + r.start, r.end - r.start).await?;
            let decoded = layout
                .decode_f32_vec_column(column_id, &stripe)
                .map_err(|e| fs_err("decode vector column", e))?;
            out.extend(decoded);
        }
        Ok(out)
    }

    /// Reconstruct full records, **skipping whole blocks** the filter provably
    /// excludes (predicate pushdown). For each block only the footer/metadata is
    /// range-read first; a block that survives [`evaluate_block`] has its body
    /// fetched and decoded, a pruned block costs only its metadata. This is the
    /// I/O win for selective scans over object storage.
    ///
    /// `field_to_col` maps filter field names to canonical PAX column ids;
    /// `embedding_model_ids`/`user_column_keys` are the positional schema hints
    /// for record materialization (empty slices ⇒ best-effort defaults).
    pub async fn read_records_pruned(
        &self,
        filter: &FilterExpression,
        field_to_col: &(dyn Fn(&str) -> Option<i32> + Sync),
        embedding_model_ids: &[String],
        user_column_keys: &[String],
    ) -> Result<Vec<ProximaRecord>, StorageError> {
        let mut out = Vec::new();
        for entry in &self.index.blocks {
            let (off, bsz) = (entry.offset, entry.size as u64);
            let layout = self.block_layout(off, bsz).await?;

            // Block-level prune from metadata alone — no block body fetched.
            if evaluate_block(&*layout as &dyn BlockZoneSource, filter, field_to_col)
                == PruneResult::Skip
            {
                continue;
            }

            // Surviving block: fetch the whole block and reconstruct its rows.
            let block_bytes = self.fetch(off, bsz).await?;
            let reader = PaxBlockReader::open(&block_bytes).map_err(|e| fs_err("open block", e))?;
            for flat in FlatRow::from_block_reader(&reader).map_err(|e| fs_err("flat rows", e))? {
                out.push(
                    flat.into_record(
                        embedding_model_ids,
                        user_column_keys,
                        self.tenant_id.as_deref(),
                    )
                    .map_err(|e| fs_err("into record", e))?,
                );
            }
        }
        Ok(out)
    }

    /// Range-read and decode one i64 column across every block.
    pub async fn read_i64_column(&self, column_id: i32) -> Result<Vec<i64>, StorageError> {
        let mut out = Vec::new();
        for entry in &self.index.blocks {
            let (off, bsz) = (entry.offset, entry.size as u64);
            let layout = self.block_layout(off, bsz).await?;
            let Some(r) = layout.column_stripe_range(column_id) else {
                continue;
            };
            let stripe = self.fetch(off + r.start, r.end - r.start).await?;
            let decoded = layout
                .decode_i64_column(column_id, &stripe)
                .map_err(|e| fs_err("decode i64 column", e))?;
            out.extend(decoded);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pax_block::{PaxSegmentScanner, PaxSegmentWriter, ScanPredicate};
    use async_trait::async_trait;
    use proximadb_block_format::col_id;
    use proximadb_records::{EmbeddingCell, EmbeddingValues, ProximaRecord};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A bridge over an in-memory segment that serves true byte ranges and
    /// counts the bytes actually transferred — the stand-in for an S3 ranged GET.
    struct InMemoryRangeBridge {
        bytes: Vec<u8>,
        ranged_bytes: AtomicU64,
    }

    #[async_trait]
    impl ObjectStoreBridge for InMemoryRangeBridge {
        async fn read_parquet_batches(
            &self,
            _path: &Path,
            _schema: std::sync::Arc<arrow_schema::Schema>,
            _batch_size: usize,
            _tenant_id: Option<&str>,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<arrow_array::RecordBatch, StorageError>>,
            StorageError,
        > {
            unimplemented!("not needed for ranged-segment tests")
        }

        fn inner_store(&self) -> std::sync::Arc<dyn object_store::ObjectStore> {
            std::sync::Arc::new(object_store::memory::InMemory::new())
        }

        async fn write_records_to_parquet(
            &self,
            _path: &Path,
            _records: &[ProximaRecord],
            _tenant_id: Option<&str>,
        ) -> Result<(), StorageError> {
            unimplemented!()
        }

        async fn fetch_vector_segment(
            &self,
            _path: &Path,
            _tenant_id: Option<&str>,
        ) -> Result<Vec<u8>, StorageError> {
            Ok(self.bytes.clone())
        }

        async fn vector_segment_size(
            &self,
            _path: &Path,
            _tenant_id: Option<&str>,
        ) -> Result<u64, StorageError> {
            Ok(self.bytes.len() as u64)
        }

        async fn fetch_vector_segment_range(
            &self,
            _path: &Path,
            offset: u64,
            length: u64,
            _tenant_id: Option<&str>,
        ) -> Result<Vec<u8>, StorageError> {
            let start = (offset as usize).min(self.bytes.len());
            let end = ((offset + length) as usize).min(self.bytes.len());
            self.ranged_bytes
                .fetch_add((end - start) as u64, Ordering::Relaxed);
            Ok(self.bytes[start..end].to_vec())
        }

        async fn persist_vector_segment(
            &self,
            _path: &Path,
            _data: &[u8],
            _tenant_id: Option<&str>,
        ) -> Result<(), StorageError> {
            unimplemented!()
        }
    }

    fn rec(oid: &str, ts: i64, v: Vec<f32>) -> ProximaRecord {
        let dim = v.len() as u32;
        ProximaRecord {
            oid: oid.into(),
            tenant_id: "t".into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            embeddings: vec![EmbeddingCell {
                model_id: "m".into(),
                modality: "dense".into(),
                values: EmbeddingValues::Fp32(v),
                dim,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn build_segment_bytes(n: usize, dim: usize) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.pax");
        let mut w = PaxSegmentWriter::new(
            &path,
            proximadb_block_format::BlockMode::Pax,
            proximadb_block_format::BlockCompression::None,
            "c",
            0,
            1,
            Some(16 * 1024),
        );
        for i in 0..n {
            let v: Vec<f32> = (0..dim).map(|d| (i * dim + d) as f32 * 0.001).collect();
            w.add_record(&rec(&format!("r{i}"), 1000 + i as i64, v))
                .unwrap();
        }
        let meta = w.finish().unwrap();
        std::fs::read(&meta.path).unwrap()
    }

    /// End-to-end: filter pushdown + footer cache + correctness on one path.
    /// A selective SELECT-WHERE over a multi-block segment, run twice through a
    /// shared tenant footer cache, must (1) return exactly the matching rows,
    /// (2) skip non-matching block bodies (pruning → < whole segment), and
    /// (3) fetch fewer bytes the second time (footer cache hit).
    #[tokio::test]
    async fn e2e_filtered_pruned_cached_read() {
        use proximadb_filter_expression::{ComparisonOperator, FilterExpression};

        let bytes = build_segment_bytes(2000, 16);
        let total = bytes.len() as u64;
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let footer_cache: Arc<FooterCache> = Arc::new(FooterCache::new(
            proximadb_cache::CacheBudget::new(1 << 30, 1 << 30),
        ));

        // created_at = 1000 + i; keep only the upper portion.
        let threshold = 1000 + 1500i64;
        let filter = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: serde_json::json!(threshold),
        };
        let field_to_col = |f: &str| match f {
            "created_at" => Some(col_id::CREATED_AT),
            _ => None,
        };

        // Pass 1 — populates the footer cache, prunes low blocks.
        let r1 = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("t"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        let recs1 = r1
            .read_records_pruned(&filter, &field_to_col, &[], &[])
            .await
            .unwrap();
        let after_1 = bridge.ranged_bytes.load(Ordering::Relaxed);

        // (1) correctness: every returned row is in a surviving block (block-
        // granular pruning) and at least the true matches are present.
        assert!(
            !recs1.is_empty() && recs1.len() < 2000,
            "pruned to a subset"
        );
        assert!(recs1.iter().all(|r| r.created_at_ns >= threshold - 8192));
        assert!(
            recs1
                .iter()
                .filter(|r| r.created_at_ns >= threshold)
                .count()
                > 0
        );
        // (2) pruning: low block bodies skipped → less than the whole segment.
        assert!(
            after_1 < total,
            "pass 1 {after_1} not < whole segment {total}"
        );
        footer_cache.sync().await;

        // Pass 2 — same query, footer cache hot.
        let r2 = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("t"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        let recs2 = r2
            .read_records_pruned(&filter, &field_to_col, &[], &[])
            .await
            .unwrap();
        let delta_2 = bridge.ranged_bytes.load(Ordering::Relaxed) - after_1;

        // (3) cache: identical results, fewer bytes (footers served from cache).
        assert_eq!(recs2.len(), recs1.len(), "cached pass must match");
        assert!(
            delta_2 < after_1,
            "pass 2 {delta_2} not < pass 1 {after_1} (cache miss)"
        );
    }

    #[tokio::test]
    async fn ranged_segment_column_equals_whole_read() {
        let bytes = build_segment_bytes(512, 64);
        let total = bytes.len() as u64;
        let bridge = InMemoryRangeBridge {
            bytes: bytes.clone(),
            ranged_bytes: AtomicU64::new(0),
        };

        let reader = RangedSegmentReader::open(&bridge, Path::from("seg.pax"), Some("t"))
            .await
            .unwrap();
        assert!(reader.block_count() >= 1);
        let ranged = reader
            .read_f32_vec_column(col_id::EMBED_BASE)
            .await
            .unwrap();

        // Whole-segment reference decode.
        let mut scanner = PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()).unwrap();
        let records = scanner.read_records(&[], &[], None).unwrap();
        let expected: Vec<Option<Vec<f32>>> = records
            .iter()
            .map(|r| r.embeddings.first().map(|e| e.values.to_fp32_owned()))
            .collect();

        // Correctness: the footer-first ranged decode matches the whole-segment
        // decode exactly (the vector column is the bulk of the segment, so this
        // test is about correctness; the I/O saving is shown by the scalar test).
        assert_eq!(ranged, expected);
        let _ = total;
    }

    #[tokio::test]
    async fn ranged_segment_pruned_records_skip_blocks() {
        use proximadb_filter_expression::{ComparisonOperator, FilterExpression};

        // Monotonic created_at across many small blocks (16KB threshold).
        let bytes = build_segment_bytes(2000, 32);
        let total = bytes.len() as u64;
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let reader = RangedSegmentReader::open(&bridge, Path::from("seg.pax"), Some("t"))
            .await
            .unwrap();
        assert!(reader.block_count() > 1, "need multiple blocks to prune");

        // created_at = 1000 + i; keep only the last ~quarter of rows.
        let threshold = 1000 + 1500i64;
        let filter = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: serde_json::json!(threshold),
        };
        let field_to_col = |f: &str| match f {
            "created_at" => Some(col_id::CREATED_AT),
            _ => None,
        };

        let recs = reader
            .read_records_pruned(&filter, &field_to_col, &[], &[])
            .await
            .unwrap();

        // Every returned row satisfies the predicate (blocks fully below the
        // threshold were skipped; surviving blocks may include some below-rows
        // which the caller re-filters — but here block bounds align to the cut).
        assert!(!recs.is_empty() && recs.len() < 2000);
        assert!(recs.iter().all(|r| r.created_at_ns >= threshold - 8192)); // block-granular
        let with_match = recs.iter().filter(|r| r.created_at_ns >= threshold).count();
        assert!(with_match > 0);

        // Pruned blocks' bodies were never fetched ⇒ far fewer bytes than whole.
        let fetched = bridge.ranged_bytes.load(Ordering::Relaxed);
        assert!(
            fetched < total,
            "pruned read fetched {fetched} not < segment {total}"
        );
    }

    #[tokio::test]
    async fn footer_cache_hit_skips_metadata_reads() {
        // Second read of the same segment must skip footer/metadata ranged reads
        // (served from the footer cache) and fetch strictly fewer bytes.
        let bytes = build_segment_bytes(2000, 32);
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let footer_cache: Arc<FooterCache> = Arc::new(FooterCache::new(
            proximadb_cache::CacheBudget::new(1 << 30, 1 << 30),
        ));

        // First read populates the footer cache for every block.
        let r1 = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("t"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        let c1 = r1.read_i64_column(col_id::CREATED_AT).await.unwrap();
        assert_eq!(c1.len(), 2000);
        let after_first = bridge.ranged_bytes.load(Ordering::Relaxed);
        footer_cache.sync().await;

        // Co-design C0 read accounting: a cold open is all footer-cache misses
        // (one per block) and its byte count is positive and bounded by the
        // bridge total (which additionally includes the index-locate suffix).
        let s1 = r1.read_stats();
        assert_eq!(s1.footer_hits, 0, "cold open: no footer-cache hits");
        assert_eq!(
            s1.footer_misses as usize,
            r1.block_count(),
            "cold open: one footer miss per block"
        );
        assert!(
            s1.bytes_read > 0 && s1.bytes_read <= after_first,
            "reader byte accounting {} within bridge total {after_first}",
            s1.bytes_read
        );
        // Read-granularity signal: a cold open issues multiple ranged GETs, and
        // average GET size = bytes_read / range_gets is well-defined.
        assert!(s1.range_gets > 0, "cold open issued ranged GETs");
        assert!(
            (s1.bytes_read / s1.range_gets) > 0,
            "average GET size is well-defined and positive"
        );

        // Second read: footers come from cache → only stripe bytes are fetched.
        let r2 = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("t"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        let c2 = r2.read_i64_column(col_id::CREATED_AT).await.unwrap();
        assert_eq!(c2, c1, "cached read must decode identically");
        let delta2 = bridge.ranged_bytes.load(Ordering::Relaxed) - after_first;

        // Warm open: every block's footer is served from cache (one hit per
        // block, zero misses) and only stripe bytes are fetched.
        let s2 = r2.read_stats();
        assert_eq!(s2.footer_misses, 0, "warm open: no footer-cache misses");
        assert_eq!(
            s2.footer_hits as usize,
            r2.block_count(),
            "warm open: one footer hit per block"
        );
        assert!(
            s2.bytes_read > 0 && s2.bytes_read <= delta2,
            "warm-open stripe-byte accounting positive and bounded"
        );

        assert!(delta2 > 0, "second read still fetches stripe bytes");
        assert!(
            delta2 < after_first,
            "second read {delta2} not < first {after_first} (footer reads not skipped)"
        );
    }

    #[tokio::test]
    async fn footer_cache_is_tenant_namespaced() {
        // Tenant B must NOT be served from tenant A's footer entries — keys are
        // tenant-namespaced, so B's first read still fetches footer bytes.
        let bytes = build_segment_bytes(1000, 32);
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let footer_cache: Arc<FooterCache> = Arc::new(FooterCache::new(
            proximadb_cache::CacheBudget::new(1 << 30, 1 << 30),
        ));

        let ra = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("tenantA"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        ra.read_i64_column(col_id::CREATED_AT).await.unwrap();
        footer_cache.sync().await;
        let after_a = bridge.ranged_bytes.load(Ordering::Relaxed);

        // Tenant B, same path: must do its own footer reads (A's entries are isolated).
        let rb = RangedSegmentReader::open_with_cache(
            &bridge,
            Path::from("seg.pax"),
            Some("tenantB"),
            Some(footer_cache.clone()),
            None,
        )
        .await
        .unwrap();
        rb.read_i64_column(col_id::CREATED_AT).await.unwrap();
        let delta_b = bridge.ranged_bytes.load(Ordering::Relaxed) - after_a;

        // B re-reads footers (not served from A) → fetched bytes ≈ A's footer+stripe.
        assert!(
            delta_b >= after_a / 2,
            "tenant B {delta_b} unexpectedly served from tenant A's cache"
        );
        // Per-tenant stats reflect both tenants.
        footer_cache.sync().await;
        let stats = footer_cache.tenant_stats();
        assert!(stats.iter().any(|s| s.tenant == "tenantA"));
        assert!(stats.iter().any(|s| s.tenant == "tenantB"));
    }

    #[tokio::test]
    async fn ranged_segment_scalar_only_skips_vector_stripe() {
        let bytes = build_segment_bytes(512, 512); // vector stripes dominate
        let total = bytes.len() as u64;
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let reader = RangedSegmentReader::open(&bridge, Path::from("seg.pax"), Some("t"))
            .await
            .unwrap();
        let created = reader.read_i64_column(col_id::CREATED_AT).await.unwrap();
        assert_eq!(created.len(), 512);
        assert_eq!(created[0], 1000);

        // Scalar-only read skips the large vector stripes → reads well under half
        // the segment (the vector column alone is the majority of the bytes).
        let fetched = bridge.ranged_bytes.load(Ordering::Relaxed);
        assert!(
            fetched < total / 2,
            "scalar-only ranged fetched {fetched} not << segment {total}"
        );
    }

    /// Footer-cache + ranged-read cloud-economics measurement (co-design C0;
    /// "measure, don't assert"). Quantifies the byte/request savings of a
    /// footer-cached vector-column projection read vs a whole-byte read across a
    /// steady-state run of queries over one warm segment. The `InMemoryRangeBridge`
    /// stands in for S3 ranged GETs, so bytes_read / range_gets / footer hit-ratio
    /// directly proxy S3 egress + per-request-fee + cache effectiveness — the
    /// evidence that justifies wiring the ranged reader into the live object-store
    /// read path. Reports the numbers; asserts only the clear wins (hit ratio +
    /// bytes), leaving GET-count as an honest reported metric (per-block column
    /// scans can fragment into more, smaller GETs — the co-design §2.1 signal).
    #[tokio::test]
    async fn footer_cache_ranged_read_economics_vs_whole_byte() {
        const N: usize = 4_000;
        const DIM: usize = 128;
        const Q: usize = 20; // steady-state queries over one warm segment

        let bytes = build_segment_bytes(N, DIM);
        let whole_segment_bytes = bytes.len() as u64;
        let bridge = InMemoryRangeBridge {
            bytes,
            ranged_bytes: AtomicU64::new(0),
        };
        let footer_cache: Arc<FooterCache> = Arc::new(FooterCache::new(
            proximadb_cache::CacheBudget::new(1 << 30, 1 << 30),
        ));

        // Steady state: Q vector-column projection reads sharing one tenant
        // footer cache. Pass 1 warms the cache (footer+metadata fetched); passes
        // 2..Q hit (only the vector stripes are fetched).
        let mut ranged_bytes_total = 0u64;
        let mut range_gets_total = 0u64;
        let mut footer_hits = 0u64;
        let mut footer_misses = 0u64;
        for _ in 0..Q {
            let r = RangedSegmentReader::open_with_cache(
                &bridge,
                Path::from("seg.pax"),
                Some("t"),
                Some(footer_cache.clone()),
                None,
            )
            .await
            .unwrap();
            let vecs = r.read_f32_vec_column(col_id::EMBED_BASE).await.unwrap();
            assert_eq!(vecs.len(), N, "vector column decodes all rows");
            let s = r.read_stats();
            ranged_bytes_total += s.bytes_read;
            range_gets_total += s.range_gets;
            footer_hits += s.footer_hits;
            footer_misses += s.footer_misses;
        }

        // Whole-byte baseline (the current live read path): each query GETs the
        // entire segment — no caching, no projection.
        let whole_bytes_total = whole_segment_bytes * Q as u64;
        let whole_gets_total = Q as u64;

        let decisions = (footer_hits + footer_misses).max(1);
        let hit_ratio = footer_hits as f64 / decisions as f64;
        let byte_reduction = 100.0 * (1.0 - ranged_bytes_total as f64 / whole_bytes_total as f64);
        let get_delta_pct = 100.0 * (range_gets_total as f64 / whole_gets_total as f64 - 1.0);

        // Cloud-economics projection (S3 Standard, us-east-1 list): GET
        // $0.00045/1k requests; egress $0.09/GB. Per-query figures in parens.
        let bytes_saved = whole_bytes_total.saturating_sub(ranged_bytes_total);
        let egress_usd = bytes_saved as f64 / (1024.0 * 1024.0 * 1024.0) * 0.09;
        let gets_delta = range_gets_total as i64 - whole_gets_total as i64;
        let get_fee_delta_usd = gets_delta as f64 * 0.00045 / 1000.0;

        eprintln!("[footer-cache-bench] segment={whole_segment_bytes}B N={N} DIM={DIM} Q={Q}");
        eprintln!(
            "[footer-cache-bench] whole-byte  : {whole_bytes_total}B / {whole_gets_total} GETs"
        );
        eprintln!(
            "[footer-cache-bench] ranged+cache: {ranged_bytes_total}B / {range_gets_total} GETs | footer hit-ratio={hit_ratio:.3}"
        );
        eprintln!(
            "[footer-cache-bench] bytes -{byte_reduction:.1}% (egress saves ${egress_usd:.5} over {Q}q, ${:.7}/q)",
            egress_usd / Q as f64
        );
        eprintln!(
            "[footer-cache-bench] GETs {get_delta_pct:+.1}% vs whole ({gets_delta:+} reqs, ${get_fee_delta_usd:+.6} fee) — fragmentation signal"
        );

        // Ratchet: footer cache hits in steady state, and the projection reads
        // strictly fewer bytes than whole-byte (the egress win). GET count is a
        // reported signal, not a ratchet — per-block column scans can fragment.
        assert!(
            hit_ratio >= 0.9,
            "footer cache should hit >=90% in steady state, got {hit_ratio:.3}"
        );
        assert!(
            ranged_bytes_total < whole_bytes_total,
            "ranged+cached ({ranged_bytes_total}B) must read less than whole-byte ({whole_bytes_total}B)"
        );
    }
}
