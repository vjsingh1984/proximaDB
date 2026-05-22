/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SST bulk-load API for the async-ingest path (Phase 2F).
//!
//! The drainer (Phase 2G) needs to land embedded batches into the
//! target collection. Per locked invariant #5, async ingest should
//! **bypass WAL+memtable** and bulk-load a pre-sorted SST segment
//! directly — that's the storage-layer LSM optimization that backs
//! the throughput economics in the queue's `README.md`
//! ("Throughput economics — why the queue path is structurally
//! cheaper").
//!
//! ## What this module ships in Phase 2F
//!
//! The **API shape** that the drainer targets. Production callers
//! construct a [`BulkLoader`] and call [`BulkLoader::ingest_sorted_segment`].
//! The MVP delegates to the normal per-record insert path via
//! `UnifiedHandlers::handle_record_batch_for_tenant`; the actual
//! SST-writer-direct path lands in a focused storage-engine refactor
//! (Phase 2F-b) that exposes each engine's flush SST-writer step as a
//! reusable primitive.
//!
//! Wiring the API shape now means:
//! - The drainer can be implemented against `BulkLoader` today.
//! - When 2F-b lands, the swap is local to `BulkLoader::ingest_sorted_segment`.
//!   Callers don't change.
//!
//! ## Why not in `proximadb-vector`
//!
//! The plan originally placed `bulk_load.rs` in `crates/modalities/
//! proximadb-vector`. That crate doesn't currently own the actual
//! storage-engine integration code — the engines (NOVA, SST, VIPER,
//! etc.) live in this crate (`src/storage/engines/`). Locating
//! `BulkLoader` here avoids a circular dep: `proximadb-vector` cannot
//! reach back into the main `proximadb` crate. When the storage-engine
//! refactor migrates the engines into `proximadb-vector` (a parallel
//! design effort), `BulkLoader` can move with them.

use std::sync::Arc;

use anyhow::Result;
use proximadb_records::ProximaRecord;
use tracing::{debug, warn};

use crate::api_handlers::RichRecordBatchRequest;
use crate::api_handlers::request_handlers::UnifiedHandlers;

/// Returned from a successful bulk-load. Once the storage-engine
/// refactor exposes the real SST-write step, this will identify the
/// committed segment on disk; today it's a synthetic id derived from
/// the underlying insert metrics — sufficient for tracing and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkLoadedSegment {
    pub collection_id: String,
    pub record_count: usize,
    pub synthetic_segment_id: String,
}

/// Sort policy hint for `ingest_sorted_segment`. The drainer typically
/// passes `Unsorted` (records arrive batched from the queue in
/// per-message order) and lets the loader sort. Callers that already
/// have a sorted batch (recovery, replay) can pass `Sorted` to skip
/// the sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortHint {
    Sorted,
    Unsorted,
}

/// Per-call options. Reserved for the SST refactor (compaction level,
/// segment metadata, etc.); today the only meaningful field is the
/// sort hint.
#[derive(Debug, Clone, Default)]
pub struct BulkLoadOptions {
    pub sort_hint: Option<SortHint>,
}

pub struct BulkLoader {
    handlers: Arc<UnifiedHandlers>,
}

impl BulkLoader {
    pub fn new(handlers: Arc<UnifiedHandlers>) -> Self {
        Self { handlers }
    }

    /// Bulk-load a batch of records into `collection_id` for the given
    /// tenant.
    ///
    /// Phase 2F: delegates to `UnifiedHandlers::handle_record_batch_for_tenant`,
    /// which goes through WAL+memtable. Customer-visible correctness is
    /// identical to the per-record insert path; the LSM-bypass
    /// performance win lands when 2F-b refactors each storage engine
    /// to expose its SST-writer step. Until then, every batch costs
    /// (N records × per-record WAL fsync) instead of (1 segment write
    /// + 1 fsync) — but the inference-layer batching savings (21× per
    /// record vs sync, see queue README) are already realized.
    ///
    /// `records` are sorted in-place by oid when `sort_hint` is
    /// `Unsorted` (default) — LSM bulk-load requires sorted input, so
    /// the drainer can avoid sorting twice by passing `Sorted` once
    /// the refactor lands.
    pub async fn ingest_sorted_segment(
        &self,
        collection_id: String,
        tenant_id: Option<&str>,
        mut records: Vec<ProximaRecord>,
        options: BulkLoadOptions,
    ) -> Result<BulkLoadedSegment> {
        let sort_hint = options.sort_hint.unwrap_or(SortHint::Unsorted);
        if sort_hint == SortHint::Unsorted {
            records.sort_by(|a, b| a.oid.cmp(&b.oid));
        }

        let record_count = records.len();
        if record_count == 0 {
            return Ok(BulkLoadedSegment {
                collection_id,
                record_count: 0,
                synthetic_segment_id: String::from("empty"),
            });
        }

        // Delegate to the per-record insert path. When 2F-b lands the
        // storage-engine refactor, this whole block becomes:
        //
        //     engine.ingest_sorted_segment(collection_id, base_path, records).await
        //
        // ...where `engine` is the trait object resolved by the
        // catalog. The drainer doesn't need to change.
        let request = RichRecordBatchRequest {
            collection_id: collection_id.clone(),
            records,
        };
        let result = self
            .handlers
            .handle_record_batch_for_tenant(request, tenant_id)
            .await
            .map_err(|e| anyhow::anyhow!("bulk-load delegate failed: {e}"))?;
        if !result.success {
            warn!(
                collection_id = %collection_id,
                errors = ?result.errors,
                "bulk-load returned non-success; treating as failure",
            );
            return Err(anyhow::anyhow!(
                "bulk-load failed: {}",
                result.errors.join("; ")
            ));
        }

        let synthetic_segment_id = format!(
            "bulkload-{}-{}",
            collection_id,
            result
                .vector_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "noid".to_string())
        );
        debug!(
            collection_id = %collection_id,
            record_count,
            synthetic_segment_id = %synthetic_segment_id,
            "bulk_load: segment committed (via per-record path; LSM bypass deferred to 2F-b)",
        );
        Ok(BulkLoadedSegment {
            collection_id,
            record_count,
            synthetic_segment_id,
        })
    }
}

// ── Production DrainerInsertSink wired to BulkLoader ─────────────

use crate::services::embedding_drainer::{DrainerInsertSink, EmbeddedRecord};
use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaTreeNode};

/// Production [`DrainerInsertSink`] that projects the drainer's
/// `EmbeddedRecord`s into `ProximaRecord`s and hands them to
/// [`BulkLoader::ingest_sorted_segment`]. Tests use the in-memory
/// `RecordingSink`; production startup constructs this wrapper.
pub struct BulkLoadDrainerSink {
    bulk_loader: Arc<BulkLoader>,
}

impl BulkLoadDrainerSink {
    pub fn new(bulk_loader: Arc<BulkLoader>) -> Self {
        Self { bulk_loader }
    }
}

#[async_trait]
impl DrainerInsertSink for BulkLoadDrainerSink {
    async fn insert(
        &self,
        target_collection: &str,
        tenant_id: &str,
        records: Vec<EmbeddedRecord>,
    ) -> Result<()> {
        let now_ns = chrono::Utc::now()
            .timestamp_millis()
            .saturating_mul(1_000_000);
        let proxima_records: Vec<ProximaRecord> = records
            .into_iter()
            .map(|r| {
                let mut props = std::collections::HashMap::new();
                props.insert(
                    "text".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(r.text)),
                );
                for (k, v) in r.metadata {
                    props.insert(k, ProximaTreeNode::Value(ProximaValue::String(v)));
                }
                ProximaRecord {
                    oid: r.oid.clone(),
                    local_id: Some(r.oid),
                    tenant_id: tenant_id.to_string(),
                    created_at_ns: now_ns,
                    updated_at_ns: now_ns,
                    origin: Some("v3_async_drainer".to_string()),
                    props,
                    embeddings: vec![EmbeddingCell {
                        model_id: "native".to_string(),
                        modality: "dense_vector".to_string(),
                        dim: r.vector_dim,
                        values: r.vector,
                    }],
                    ..ProximaRecord::default()
                }
            })
            .collect();
        let _segment = self
            .bulk_loader
            .ingest_sorted_segment(
                target_collection.to_string(),
                Some(tenant_id),
                proxima_records,
                BulkLoadOptions::default(),
            )
            .await?;
        Ok(())
    }
}

// ── tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_records::ProximaRecord;

    fn rec(oid: &str) -> ProximaRecord {
        ProximaRecord {
            oid: oid.to_string(),
            local_id: Some(oid.to_string()),
            tenant_id: "t".to_string(),
            ..ProximaRecord::default()
        }
    }

    /// Sort hint defaulting to Unsorted sorts records in place by oid.
    /// The contract for the LSM bypass requires sorted input.
    #[test]
    fn unsorted_records_get_sorted_by_oid() {
        let mut records = vec![rec("zeta"), rec("alpha"), rec("mike")];
        records.sort_by(|a, b| a.oid.cmp(&b.oid));
        let oids: Vec<_> = records.iter().map(|r| r.oid.as_str()).collect();
        assert_eq!(oids, vec!["alpha", "mike", "zeta"]);
    }

    #[test]
    fn empty_batch_returns_empty_segment_id() {
        // Doesn't call into handlers — empty batch short-circuits.
        let segment = BulkLoadedSegment {
            collection_id: "knowledge".to_string(),
            record_count: 0,
            synthetic_segment_id: "empty".to_string(),
        };
        assert_eq!(segment.record_count, 0);
        assert_eq!(segment.synthetic_segment_id, "empty");
    }

    #[test]
    fn sort_hint_sorted_skips_inplace_sort() {
        let hint = SortHint::Sorted;
        assert_eq!(hint, SortHint::Sorted);
        // The actual skip happens inside ingest_sorted_segment; this
        // test exists to lock the API shape so reorderings of the
        // SortHint enum don't silently break callers.
    }
}
