//! Shared per-collection flush materialization.
//!
//! The server shutdown flush (`StorageEngine::flush_memtable_to_storage`) and the
//! embedded flush (`EmbeddedDb::flush`) both have to turn a collection's unflushed
//! WAL batches into a materialized storage segment. Historically only the embedded
//! path worked; the server path relied on a never-populated `sst_storages` registry
//! plus a metadata-less coordinator and silently materialized nothing (TD-163). This
//! module is the single, shared materialization core both call, so the proven
//! recipe lives in exactly one place:
//!
//! ```text
//! resolve engine from catalog metadata → StorageFormatFactory::create_from_proto_async
//! → build collection_config + FlushParameters → engine.flush()
//! → clear flushed batches → delete the now-redundant WAL files.
//! ```
//!
//! It also applies the **A6 storage-write fence** (default-OFF) before the write,
//! so a pod displaced by a lease takeover cannot publish stale data on its way out
//! (CLAUDE.md #16). The caller supplies the resolved per-collection metadata (the
//! server reads the catalog; embedded uses its collection port) and, optionally, a
//! fallback engine to reuse.

use std::sync::Arc;

use anyhow::Result;

use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, StorageAssignment, StorageEngine as ProtoStorageEngine,
};
use crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper;
use crate::storage::traits::{FlushParameters, UnifiedStorageFormat};
use crate::storage::write_fence::{
    FenceDecision, StorageWriteFence, evaluate_fence, write_fencing_enabled,
};

/// Resolved materialization inputs for one collection. The caller fills these from
/// its own metadata source (catalog for the server, collection port for embedded).
pub struct CollectionFlushPlan {
    /// Key used at write time in the WAL / global write buffer (collection name).
    pub wal_key: String,
    /// Canonical UUID used for the on-disk storage layout.
    pub canonical_id: String,
    /// Base storage location (`file://…` / `s3://…`) for this collection.
    pub base_location: String,
    /// Proto `StorageEngine` discriminant for the collection's configured engine.
    pub engine_type: i32,
    /// Vector dimension (required by VIPER/NOVA flush).
    pub dimension: u32,
    /// Owning tenant, when known — the A6 fence input. `None` ⇒ fence fails open.
    pub tenant_id: Option<String>,
}

/// What a single collection's materialization produced.
pub struct CollectionFlushOutcome {
    /// Records submitted to the engine (post tombstone-filter) — the count used
    /// for collection stats / optimizer row estimates.
    pub vectors_submitted: u64,
    /// Records the engine reported written (`FlushResult::entries_flushed`) — the
    /// count used for flush summaries.
    pub entries_flushed: u64,
    pub bytes: u64,
}

/// Materialize one collection's unflushed WAL batches to its configured storage
/// engine. Returns `Ok(None)` when there is nothing to flush, `Err` when the A6
/// fence rejects the write or the storage flush fails.
///
/// A6 fence (default-OFF; `PROXIMADB_WRITE_FENCING=1`) is checked first — before
/// any record read or storage write — so a fenced-out pod is rejected at the
/// boundary. Fail-open when the gate is off, no fence is wired, or the tenant is
/// unknown.
///
/// `free_wal` controls whether the source WAL is reclaimed after the segment is
/// written. Post-TD-165 (cold-read recall fixed) **both callers pass `true`**: clear
/// the flushed batches and delete the WAL files (no 2× WAL+segment overhead), so the
/// materialized segment — not a WAL replay — is the durable restart-recall source.
/// The flag is retained (rather than inlined) as an explicit safety valve: passing
/// `false` keeps the WAL so recovery replays it into the FP32 memtable (exact recall
/// that bypasses the cold SST path) — the escape hatch if a cold-read regression
/// ever resurfaces. No caller uses `false` today.
pub async fn materialize_collection(
    write_buffer: &Arc<WALBehaviorWrapper>,
    plan: &CollectionFlushPlan,
    fence: Option<&Arc<dyn StorageWriteFence>>,
    fallback_engine: Option<Arc<dyn UnifiedStorageFormat>>,
    free_wal: bool,
) -> Result<Option<CollectionFlushOutcome>> {
    // A6 storage-write fence — reject a displaced pod before touching storage.
    if write_fencing_enabled() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        if evaluate_fence(
            true,
            fence,
            plan.tenant_id.as_deref(),
            &plan.wal_key,
            now_ms,
        )
        .await
            == FenceDecision::Fenced
        {
            tracing::warn!(
                target: "proximadb.fence",
                tenant_id = plan.tenant_id.as_deref().unwrap_or("<unknown>"),
                collection_id = %plan.wal_key,
                "🛡️ A6 fence: this pod is fenced out; rejecting stale-pod flush before storage write"
            );
            return Err(anyhow::anyhow!(
                "A6 storage-write fence: pod is fenced out of collection '{}' (tenant '{}') — a live lease is held by another pod",
                plan.wal_key,
                plan.tenant_id.as_deref().unwrap_or("<unknown>")
            ));
        }
    }

    let batches = write_buffer.get_unflushed_batches(&plan.wal_key).await?;
    if batches.is_empty() {
        return Ok(None);
    }

    // Combine canonical records from all unflushed batches. Tombstones (no
    // embeddings) are dropped: the SST writer's centroid/clustering pipeline
    // cannot handle empty vectors, and the deleted ids are simply absent from the
    // resulting segment (correct for this single-level flush — no older segment
    // holds a stale copy).
    let vector_records: Vec<proximadb_records::ProximaRecord> = batches
        .iter()
        .flat_map(|batch| batch.vector_records.iter().cloned())
        .filter(|r| r.embeddings.first().is_some_and(|e| !e.values.is_empty()))
        .collect();
    let vector_count = vector_records.len() as u64;

    // Resolve + create the collection's configured engine (proven factory path).
    let proto_engine =
        ProtoStorageEngine::try_from(plan.engine_type).unwrap_or(ProtoStorageEngine::Sst);
    let engine =
        match crate::storage::engines::factory::StorageFormatFactory::create_from_proto_async(
            proto_engine,
        )
        .await
        {
            Ok(engine) => engine,
            Err(e) => {
                tracing::warn!(
                    "flush: failed to create {:?} engine for '{}': {}; falling back to SST",
                    proto_engine,
                    plan.wal_key,
                    e
                );
                match fallback_engine {
                    Some(engine) => engine,
                    None => {
                        crate::storage::engines::factory::StorageFormatFactory::create_sst_async()
                            .await?
                    }
                }
            }
        };

    // Collection config carries engine + dimension + the on-disk layout path so the
    // flush writes into the same directory recovery reads from.
    let collection_config = Collection {
        id: plan.canonical_id.clone(),
        storage_assignment: Some(StorageAssignment {
            base_location: plan.base_location.clone(),
            engine: plan.engine_type,
            ..Default::default()
        }),
        config: Some(CollectionConfig {
            name: plan.wal_key.clone(),
            storage_engine: Some(plan.engine_type),
            dimension: plan.dimension,
            ..Default::default()
        }),
        ..Default::default()
    };

    let flush_params = FlushParameters {
        collection_id: Some(plan.canonical_id.clone()),
        force: true,
        synchronous: true,
        vector_records,
        batch_ids: batches.iter().map(|batch| batch.batch_id).collect(),
        collection_config: Some(collection_config),
        ..Default::default()
    };

    // The single trait `flush()` funnel (validation + post-processing + do_flush).
    let result = engine.flush(flush_params).await?;
    let bytes = result.bytes_written.unwrap_or(0);
    let entries_flushed = result.entries_flushed.unwrap_or(0);

    // WAL cleanup after a successful flush: clear the memtable batches and delete
    // the now-redundant WAL files (avoids 2× WAL+segment storage overhead). Gated
    // by `free_wal`: the server keeps the WAL until the cold SST read path serves
    // exact recall, so recovery's WAL replay remains the recall source post-restart.
    if free_wal {
        if let Err(e) = write_buffer.clear_flushed(&plan.wal_key).await {
            tracing::warn!(
                "flush: failed to clear flushed batches for '{}': {}",
                plan.wal_key,
                e
            );
        }
        let batch_id_strings: Vec<String> = batches
            .iter()
            .map(|batch| batch.batch_id.to_base62())
            .collect();
        if !batch_id_strings.is_empty()
            && let Err(e) = crate::storage::persistence::write_ahead_log::manifest::mark_flushed_and_delete_files(
                &batch_id_strings,
            )
            .await
        {
            tracing::warn!(
                "flush: failed to delete WAL files for '{}': {}",
                plan.wal_key,
                e
            );
        }
    } else {
        tracing::debug!(
            "flush: kept WAL for '{}' (free_wal=false) — recovery replays it for exact recall",
            plan.wal_key
        );
    }

    Ok(Some(CollectionFlushOutcome {
        vectors_submitted: vector_count,
        entries_flushed,
        bytes,
    }))
}
