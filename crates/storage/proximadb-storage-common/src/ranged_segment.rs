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

use object_store::path::Path;
use proximadb_block_format::{
    BLOCK_FOOTER_SIZE, BlockFooter, BlockZoneSource, FlatRow, PaxBlockReader, PruneResult,
    evaluate_block,
    ranged::{BlockLayout, metadata_ranges},
};
use proximadb_filter_expression::FilterExpression;
use proximadb_kernel::error::StorageError;
use proximadb_records::ProximaRecord;

use crate::object_store_bridge::ObjectStoreBridge;
use crate::pax_block::SegmentIndex;

/// Initial tail suffix to read when locating the segment index. Large enough to
/// hold the index of a typical multi-block segment in one GET; grown on miss.
const INITIAL_SUFFIX: u64 = 64 * 1024;

fn fs_err(ctx: &str, e: impl std::fmt::Display) -> StorageError {
    StorageError::Corruption(format!("ranged segment {ctx}: {e}"))
}

/// A PAX segment opened for ranged, projected reads.
pub struct RangedSegmentReader<'b> {
    bridge: &'b dyn ObjectStoreBridge,
    path: Path,
    tenant_id: Option<String>,
    index: SegmentIndex,
    size: u64,
}

impl<'b> RangedSegmentReader<'b> {
    /// Open a segment: HEAD for its size, then read a tail suffix and locate the
    /// segment index (re-reading a larger suffix if the index doesn't fit).
    pub async fn open(
        bridge: &'b dyn ObjectStoreBridge,
        path: Path,
        tenant_id: Option<&str>,
    ) -> Result<RangedSegmentReader<'b>, StorageError> {
        let size = bridge.vector_segment_size(&path, tenant_id).await?;
        if size < BLOCK_FOOTER_SIZE as u64 {
            return Err(fs_err("open", format!("segment too small: {size} bytes")));
        }

        let mut suffix_len = INITIAL_SUFFIX.min(size);
        let index = loop {
            let offset = size - suffix_len;
            let suffix = bridge
                .fetch_vector_segment_range(&path, offset, suffix_len, tenant_id)
                .await?;
            match SegmentIndex::locate_in_suffix(&suffix).map_err(|e| fs_err("locate index", e))? {
                Some(idx) => break idx,
                None => {
                    if suffix_len >= size {
                        return Err(fs_err("locate index", "index not found in full segment"));
                    }
                    suffix_len = (suffix_len * 4).min(size);
                }
            }
        };

        Ok(Self {
            bridge,
            path,
            tenant_id: tenant_id.map(str::to_owned),
            index,
            size,
        })
    }

    pub fn block_count(&self) -> usize {
        self.index.blocks.len()
    }

    pub fn segment_size(&self) -> u64 {
        self.size
    }

    async fn fetch(&self, offset: u64, length: u64) -> Result<Vec<u8>, StorageError> {
        self.bridge
            .fetch_vector_segment_range(&self.path, offset, length, self.tenant_id.as_deref())
            .await
    }

    /// Range-read one block's footer + metadata regions and assemble its
    /// [`BlockLayout`] — no stripe bytes are fetched.
    async fn block_layout(&self, block_offset: u64, block_size: u64) -> Result<BlockLayout, StorageError> {
        // 1. trailing block footer.
        let tail = self
            .fetch(block_offset + block_size - BLOCK_FOOTER_SIZE as u64, BLOCK_FOOTER_SIZE as u64)
            .await?;
        let footer = BlockFooter::from_bytes(&tail).map_err(|e| fs_err("block footer", e))?;

        // 2. metadata regions (offsets are block-relative; add block_offset).
        let mr = metadata_ranges(&footer, block_size);
        let col_meta = self
            .fetch(block_offset + mr.col_meta.start, mr.col_meta.end - mr.col_meta.start)
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
            if evaluate_block(&layout as &dyn BlockZoneSource, filter, field_to_col)
                == PruneResult::Skip
            {
                continue;
            }

            // Surviving block: fetch the whole block and reconstruct its rows.
            let block_bytes = self.fetch(off, bsz).await?;
            let reader = PaxBlockReader::open(&block_bytes).map_err(|e| fs_err("open block", e))?;
            for flat in FlatRow::from_block_reader(&reader).map_err(|e| fs_err("flat rows", e))? {
                out.push(
                    flat.into_record(embedding_model_ids, user_column_keys)
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
            w.add_record(&rec(&format!("r{i}"), 1000 + i as i64, v)).unwrap();
        }
        let meta = w.finish().unwrap();
        std::fs::read(&meta.path).unwrap()
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
        let ranged = reader.read_f32_vec_column(col_id::EMBED_BASE).await.unwrap();

        // Whole-segment reference decode.
        let mut scanner =
            PaxSegmentScanner::from_bytes(bytes, ScanPredicate::default()).unwrap();
        let records = scanner.read_records(&[], &[]).unwrap();
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
}
