/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SST Engine Search Module
//!
//! Contains search operations and coordination logic for the SST engine.
//! This module implements the three-stage filtering pipeline:
//! 1. Bloom filter stage - eliminate non-matching SST files
//! 2. Row filter stage - filter records within SST files
//! 3. Vector stage - compute distances for remaining candidates
//!
//! The module provides:
//! - Main unified search implementation
//! - Direct search fallback for simple queries
//! - Search coordination and optimization
//! - File discovery and routing logic

pub mod coordinator;
pub mod operations;
pub mod optimizer;

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info, trace, warn};

use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::engines::core::formats::arrow_block::ArrowBlockReader;
use crate::storage::engines::sst::{SstEngine, SstError};
use crate::storage::traits::StorageQueryContext;
use proximadb_distance_kernel::DistanceMetric;
use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use proximadb_index_traits::{
    IndexFilterOperator, IndexHybridQuery, IndexMetadataFilter, IndexSearchEffort, IndexVectorQuery,
};

pub use coordinator::SearchCoordinator;
pub use operations::SearchOperations;
pub use optimizer::SearchOptimizer;

/// TD-112: per-(process, collection) in-flight guard so lazy rebuild-from-SST
/// does not stampede when several cold queries race. It is not a completion
/// cache; `registered_vector_count` is the durable warm signal. A rebuild permit
/// clears this set on drop so cancellation/panic cannot leave a stale guard.
static AXIS_REBUILD_GUARD: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::OnceLock::new();

fn axis_rebuild_guard() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    AXIS_REBUILD_GUARD.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

struct AxisRebuildPermit {
    collection_id: String,
}

impl Drop for AxisRebuildPermit {
    fn drop(&mut self) {
        if let Some(guard) = AXIS_REBUILD_GUARD.get() {
            let mut in_flight = guard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            in_flight.remove(&self.collection_id);
        }
    }
}

fn try_begin_axis_rebuild(collection_id: &str) -> Option<AxisRebuildPermit> {
    let mut in_flight = axis_rebuild_guard()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !in_flight.insert(collection_id.to_string()) {
        return None;
    }
    Some(AxisRebuildPermit {
        collection_id: collection_id.to_string(),
    })
}

/// ADR-030 / TD-158: attribute an SST vector-search query's compute time to KRU
/// on **any** exit path (the search has several early returns — exact-scan,
/// orchestrated, direct). On drop it records the elapsed millis to the active
/// per-query I/O trace under the given engine label; `record_compute_ms` no-ops
/// outside an `io_trace` scope, so internal/test callers are unaffected.
struct ComputeMsGuard {
    engine: &'static str,
    started: std::time::Instant,
}

impl ComputeMsGuard {
    fn new(engine: &'static str) -> Self {
        Self {
            engine,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for ComputeMsGuard {
    fn drop(&mut self) {
        crate::observability::io_trace::record_compute_ms(
            self.engine,
            self.started.elapsed().as_millis() as u64,
        );
    }
}

/// TD-165: the exact-vs-approximate route is a *cost* decision, and for a cloud DB
/// the dominant term is bytes read from object storage, not vectors or CPU. An
/// `Adaptive` search takes the exact brute-force segment scan when the whole scan
/// fits one efficient ranged GET (`N · dim · 4 bytes ≤` this budget) — cheaper than
/// rebuilding a cold in-memory index, which would read the same bytes anyway, and
/// 100% recall. Above the budget the persisted/approximate index is used. 64 MiB is
/// a conservative single-ranged-read size; full storage-class-aware tuning (object
/// store round-trips vs local NVMe bandwidth) is config-driven follow-up. See
/// `docs/12-design/EXACT_VS_ANN_ROUTING_COST_MODEL_2026_06_26.adoc`.
// TODO(serverless follow-up): source this from config per storage class.
const EXACT_SCAN_MAX_BYTES: usize = 64 * 1024 * 1024;

/// ADR-028: resolve the collection's `index_policy` into the exact-scan decision
/// inputs for the SST adaptive route. Returns `(byte_budget, pin_exact)`:
///
/// * `pin_exact` — the owner set `mode = "exact"`, so the adaptive route must
///   scan exactly at any N (the 100%-recall SLA).
/// * `byte_budget` — a non-zero `byte_budget` override, else the storage-class
///   default `EXACT_SCAN_MAX_BYTES`.
///
/// Pure + total so it is unit-testable without a live collection. Query-time
/// precedence (per-query `SearchMode` over policy) is enforced by the caller: this
/// only refines the `Adaptive` (no explicit per-query intent) arm.
fn resolve_exact_budget(policy: Option<&crate::proto::proximadb_v1::IndexPolicy>) -> (usize, bool) {
    match policy {
        Some(p) => {
            let pin_exact = p.mode.trim().eq_ignore_ascii_case("exact");
            let budget = if p.byte_budget > 0 {
                p.byte_budget as usize
            } else {
                EXACT_SCAN_MAX_BYTES
            };
            (budget, pin_exact)
        }
        None => (EXACT_SCAN_MAX_BYTES, false),
    }
}

impl SstEngine {
    /// TD-165: best-effort total vector count across a collection's segments, read
    /// cheaply from each segment header (8-byte prefix + the bincode header — no
    /// data blocks). Feeds the small-collection exact-recall gate. Returns 0 on any
    /// error or for non-`SST1` segments (Arrow/PAX), so the gate then falls through
    /// to the normal approximate/orchestrated path.
    async fn segment_vector_count(&self, storage_url: &str) -> usize {
        use crate::storage::engines::sst::SstableHeader;
        let files = match self.discover_sstable_files(storage_url).await {
            Ok(files) => files,
            Err(_) => return 0,
        };
        let mut total = 0usize;
        for file_path in &files {
            let Ok(fs) = self.filesystem().get_filesystem(file_path) else {
                continue;
            };
            let Ok(prefix) = fs.read_range(file_path, 0, 8).await else {
                continue;
            };
            if prefix.len() < 8 || &prefix[0..4] != b"SST1" {
                continue;
            }
            let header_len =
                u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]) as u64;
            let Ok(header_data) = fs.read_range(file_path, 8, header_len).await else {
                continue;
            };
            if let Ok(header) = bincode::deserialize::<SstableHeader>(&header_data) {
                total += header.entry_count as usize;
            }
        }
        total
    }

    /// TD-165: exact brute-force search over the collection's segment(s), bypassing
    /// any approximate index. Forces `block_prune.force_exact` so neither the
    /// centroid file-pruning nor the Z-order block-pruning can drop the true NN —
    /// the same guarantee the index-less embedded path already provides.
    async fn execute_exact_segment_scan(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let mut exact_params = (*ctx.search_params).clone();
        exact_params.block_prune.force_exact = true;
        let exact_ctx = StorageQueryContext {
            search_params: std::sync::Arc::new(exact_params),
            collection: ctx.collection.clone(),
            metadata: ctx.metadata.clone(),
            user_context: ctx.user_context.clone(),
            tenant_context: ctx.tenant_context.clone(),
        };
        self.fallback_to_direct_search(
            &exact_ctx,
            collection_id,
            storage_url,
            query_vector,
            k,
            distance_metric,
            filter_expression,
            true, // include_vectors
            true, // include_metadata
        )
        .await
    }

    /// Main unified search implementation with orchestration
    ///
    /// This is the primary search entry point that implements intelligent
    /// search routing and the three-stage filtering pipeline.
    pub async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let _search_start = std::time::Instant::now();
        // ADR-030 / TD-158: attribute this query's SST compute to KRU on any exit
        // path (records the elapsed time to the active io_trace on drop).
        let _compute_guard = ComputeMsGuard::new("sst");

        // Track metadata access for cache optimization
        if let Some(orch) = self.orchestrator() {
            (**orch).pattern_tracker().track_access_async(
                format!("{}::sst::metadata", ctx.collection_id()),
                crate::storage::cache::orchestrator::CacheType::Metadata,
            );
        }

        // Extract search parameters from context
        let collection_id = ctx.collection_id();
        let storage_url = ctx
            .collection_storage_path()
            .ok_or_else(|| SstError::InvalidArgument("No storage URL in context".into()))?;
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| SstError::InvalidArgument("No query vector in context".into()))?;
        let k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let filter_expression = ctx.search_params.filter_expression.as_ref();

        info!(
            "🚀 SST: Starting unified search for collection {} with {} dimensions",
            collection_id,
            query_vector.len()
        );

        // TD-165: route exact-vs-approximate by the query's `SearchMode` intent —
        // the orchestrated index path below otherwise ignores it, so an `Exact`
        // query (the default) silently received approximate results, and a cold
        // HNSW rebuild was observed excluding the true NN from its candidate pool
        // entirely (f32-rerank cannot recover a candidate that was never surfaced).
        //   - `Exact`        → exact segment scan (honor the 100% contract).
        //   - `Approximate`  → orchestrated index (caller accepted the recall trade).
        //   - `Adaptive{t}`  → exact when the scan fits one ranged GET (cost gate)
        //                      and within the per-query count cap `t`, else index.
        // The cost gate is dim-aware (bytes = N·dim·4), not a flat vector count.
        // `segment_vector_count` is read only for `Adaptive`, and only the exact arm
        // is taken here — which also skips the multi-second cold AXIS rebuild below.
        // See `docs/12-design/EXACT_VS_ANN_ROUTING_COST_MODEL_2026_06_26.adoc`.
        // ADR-028 precedence: an explicit per-query `SearchMode` (Exact/Approximate)
        // wins outright; the default `Adaptive` arm defers to the collection
        // `index_policy` (mode=exact pin, or a byte_budget override) and then the
        // cost-derived auto-default.
        let want_exact = {
            use crate::core::search::SearchMode;
            match &ctx.search_params.search_mode {
                SearchMode::Approximate { .. } => false,
                SearchMode::Exact => true,
                SearchMode::Adaptive { threshold } => {
                    let policy = ctx
                        .collection
                        .config
                        .as_ref()
                        .and_then(|c| c.index_policy.as_ref());
                    let (byte_budget, pin_exact) = resolve_exact_budget(policy);
                    if pin_exact {
                        // Owner pinned exact — always brute-force, any N.
                        true
                    } else {
                        let count = self.segment_vector_count(&storage_url).await;
                        let dim = query_vector.len().max(1);
                        let scan_bytes = count.saturating_mul(dim).saturating_mul(4);
                        count > 0 && scan_bytes <= byte_budget && count <= *threshold
                    }
                }
            }
        };
        if want_exact {
            info!(
                "🎯 SST: exact segment scan for collection {} (SearchMode honored; cost-gated) — guaranteed recall (TD-165)",
                collection_id
            );
            return self
                .execute_exact_segment_scan(
                    ctx,
                    collection_id,
                    &storage_url,
                    query_vector,
                    k,
                    distance_metric,
                    filter_expression,
                )
                .await;
        }

        // Determine search strategy based on context
        // Use orchestration if:
        // 1. AXIS indexes are explicitly configured, OR
        // 2. Quantization is enabled, OR
        // 3. AXIS manager is available (for collections built after AXIS became available)
        let has_axis_manager = self.axis_manager().is_some();
        let use_orchestration =
            ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization || has_axis_manager;

        if has_axis_manager {
            debug!("🔍 SST: AXIS manager is available for HNSW/IVF search");
            // TD-112: if the in-memory AXIS index is absent (e.g. after a
            // restart), rebuild it from the durable SST segments before
            // searching, so post-flush recall does not silently degrade to a
            // brute-force segment scan.
            self.ensure_axis_index_from_sst(collection_id, &storage_url)
                .await;
        }

        if use_orchestration {
            // Use advanced orchestration when available
            self.execute_orchestrated_search(
                ctx,
                collection_id,
                &storage_url,
                query_vector,
                k,
                distance_metric,
                filter_expression,
            )
            .await
        } else {
            // Use direct search for simple queries
            self.execute_direct_search(
                ctx,
                collection_id,
                &storage_url,
                query_vector,
                k,
                distance_metric,
                filter_expression,
            )
            .await
        }
    }

    /// TD-112: lazily rebuild a collection's AXIS index from its durable SST
    /// segments when the in-memory index is absent.
    ///
    /// AXIS indexes are in-memory; after a restart they are empty and — for HNSW,
    /// or IVF flushed in sub-train-threshold batches — are not persisted, so
    /// post-flush search would silently degrade to a brute-force segment scan.
    /// This reads the flushed records back from the segments and re-indexes them
    /// through the same `handle_flushed_vectors` hook the flush path uses,
    /// covering every index type (unlike the IVF-only persist+cold-load path).
    /// Best-effort and guarded so at most one potentially expensive rebuild runs
    /// for a collection at a time.
    async fn ensure_axis_index_from_sst(&self, collection_id: &str, storage_url: &str) {
        let Some(axis) = self.axis_manager() else {
            return;
        };
        // Already present in the AXIS store (warm / cold-loaded / rebuilt)?
        // Nothing to do. (We key on the store rather than HNSW/IVF presence,
        // since those structures are built lazily and aren't a reliable signal
        // for small collections.)
        if axis.registered_vector_count(collection_id).await > 0 {
            return;
        }
        // Attempt at most one rebuild per collection concurrently. Attempts are
        // not sticky; the permit drop removes them so a later query can retry
        // after transient filesystem/index errors or explicit AXIS drops.
        let Some(_rebuild_permit) = try_begin_axis_rebuild(collection_id) else {
            return;
        };

        let files = match self.discover_sstable_files(storage_url).await {
            Ok(files) if !files.is_empty() => files,
            _ => return,
        };

        // An engine writes a single block format, so dispatch once rather than
        // detecting per file.
        let block_format =
            crate::storage::engines::sst::block_format::BlockFormat::parse_block_format(
                &self.config().block_format,
            );
        let mut records: Vec<proximadb_records::ProximaRecord> = Vec::new();
        for file in &files {
            match self.read_segment_records(file, block_format).await {
                Ok(mut recs) => records.append(&mut recs),
                Err(e) => {
                    tracing::warn!("TD-112 rebuild: failed reading segment {file}: {e}");
                }
            }
        }
        if records.is_empty() {
            return;
        }

        let count = records.len();
        match axis
            .handle_flushed_vectors(collection_id, records, files.clone())
            .await
        {
            Ok(()) => {
                tracing::info!(
                    "TD-112: rebuilt AXIS index for '{collection_id}' from {count} vectors across {} segments",
                    files.len()
                );
            }
            Err(e) => tracing::warn!(
                "TD-112 rebuild: AXIS rebuild-from-SST failed for '{collection_id}': {e}"
            ),
        }
    }

    /// Read all records from one SST segment, dispatching on the engine's block
    /// format (an engine writes a single format, so no per-file detection).
    async fn read_segment_records(
        &self,
        file: &str,
        format: crate::storage::engines::sst::block_format::BlockFormat,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        use crate::storage::engines::sst::block_format::BlockFormat;
        match format {
            BlockFormat::ArrowBlock => {
                let local = file.strip_prefix("file://").unwrap_or(file);
                let reader = ArrowBlockReader::open(local)
                    .map_err(|e| anyhow::anyhow!("open arrow segment {file}: {e}"))?;
                reader
                    .read_all()
                    .map_err(|e| anyhow::anyhow!("read arrow segment {file}: {e}"))
            }
            BlockFormat::ProximaBlocks => {
                self.sstable_reader()
                    .read_all_records_for_compaction(&[file.to_string()])
                    .await
            }
            BlockFormat::PaxBlock => {
                // P3: read a PAX segment via the mixed-format primitive (magic-detected,
                // reuses PaxSegmentScanner). Best-effort schema keys (empty) for now.
                let local = file.strip_prefix("file://").unwrap_or(file);
                let bytes = tokio::fs::read(local)
                    .await
                    .map_err(|e| anyhow::anyhow!("read pax segment {file}: {e}"))?;
                // tenant_ctx None: the AXIS rebuild indexes vectors per-collection and
                // does not consume record.tenant_id. (Deriving the owning tenant from
                // the DrPathBuilder segment path is a follow-up for when the
                // tenant-column-drop flag is enabled end-to-end.)
                crate::storage::engines::sst::segment_format::read_segment_records(
                    &bytes,
                    &[],
                    &[],
                    None,
                )
            }
        }
    }

    /// Execute orchestrated search with intelligent routing
    ///
    /// This method uses the AXIS manager for HNSW/IVF-based approximate search
    /// when available, falling back to direct search otherwise.
    async fn execute_orchestrated_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("🎯 SST: Using orchestrated search strategy");

        // Check if AXIS manager is available for HNSW/IVF search
        if let Some(axis_manager) = self.axis_manager() {
            info!(
                "🔗 SST: AXIS manager available, attempting HNSW index search for collection {}",
                collection_id
            );

            // Convert filter expression to index metadata filters
            let index_filters = Self::convert_filter_to_index(filter_expression);

            // Build hybrid query for the index trait
            let hybrid_query = IndexHybridQuery {
                collection_id: collection_id.to_string(),
                vector_query: Some(IndexVectorQuery::Dense {
                    vector: query_vector.to_vec(),
                    similarity_threshold: 0.0,
                }),
                metadata_filters: index_filters,
                id_filters: Vec::new(),
                top_k: k,
                include_expired: false,
                // Thread the accuracy-vs-latency knob into the index query so the
                // warm HNSW/IVF path honors `exact`/`approximate`/`approximate:N`
                // (mapped to HNSW `ef` / IVF `nprobe`). `None` ⇒ index default.
                search_effort: ctx
                    .search_params
                    .search_mode
                    .to_search_effort()
                    .map(|effort| match effort {
                        crate::core::search::SearchEffort::Exact => IndexSearchEffort::Exact,
                        crate::core::search::SearchEffort::Approximate { hint } => {
                            IndexSearchEffort::Approximate { hint }
                        }
                    }),
            };

            // Execute AXIS query (HNSW or IVF based on index type).
            //
            // Phase timer surfaces what fraction of the warm-path
            // latency is the actual AXIS walk vs the surrounding
            // dispatch (HybridQuery construction above, result
            // conversion below, outer get-or-create-via-OnceLock).
            // Empirically (10K × 128d cosine) the AXIS walk itself
            // is ~1 ms; the rest is dispatch + conversion overhead.
            let axis_start = std::time::Instant::now();
            match axis_manager.query(hybrid_query).await {
                Ok(axis_results) => {
                    let axis_us = axis_start.elapsed().as_micros() as u64;
                    info!(
                        "✅ SST: AXIS HNSW search completed in {:?} - found {} candidates",
                        std::time::Duration::from_micros(axis_us),
                        axis_results.results.len()
                    );
                    tracing::info!(
                        target: "sst_warm_phase",
                        phase = "axis_query",
                        elapsed_us = axis_us,
                        n_results = axis_results.results.len(),
                        "phase done"
                    );
                    tracing::info!(
                        target: "axis_diag",
                        site = "execute_orchestrated_search.result",
                        n_results = axis_results.results.len(),
                        top1_id = axis_results.results.first().map(|r| r.vector_id.as_str()).unwrap_or(""),
                        top1_raw_similarity = ?axis_results.results.first().map(|r| r.similarity),
                        "AXIS query returned — values are raw ScoredResult.similarity, no further normalization is applied before returning to caller"
                    );

                    // Convert AXIS results to OptimizedSearchRecord
                    let convert_start = std::time::Instant::now();
                    let results: Vec<OptimizedSearchRecord> = axis_results
                        .results
                        .into_iter()
                        .take(k)
                        .map(|scored| OptimizedSearchRecord {
                            id: scored.vector_id.to_string(),
                            vector_id: Some(scored.vector_id.to_string()),
                            score: scored.similarity,
                            similarity: Some(scored.similarity),
                            vector: None, // AXIS doesn't return vectors by default
                            ..Default::default()
                        })
                        .collect();
                    let convert_us = convert_start.elapsed().as_micros() as u64;
                    tracing::info!(
                        target: "sst_warm_phase",
                        phase = "axis_result_convert",
                        elapsed_us = convert_us,
                        n_results = results.len(),
                        "phase done"
                    );

                    // If we need vectors or got fewer results, optionally refine with SST lookup
                    if results.is_empty() {
                        info!("⚠️ SST: AXIS returned no results, falling back to direct search");
                        return self
                            .fallback_to_direct_search(
                                ctx,
                                collection_id,
                                storage_url,
                                query_vector,
                                k,
                                distance_metric,
                                filter_expression,
                                true,
                                true,
                            )
                            .await;
                    }

                    return Ok(results);
                }
                Err(e) => {
                    warn!(
                        "⚠️ SST: AXIS query failed ({}), falling back to direct search",
                        e
                    );
                }
            }
        } else {
            debug!("🔍 SST: AXIS manager not available, using direct search");
        }

        // Fall back to direct search if AXIS is unavailable or failed
        self.fallback_to_direct_search(
            ctx,
            collection_id,
            storage_url,
            query_vector,
            k,
            distance_metric,
            filter_expression,
            true, // include_vectors
            true, // include_metadata
        )
        .await
    }

    /// Convert FilterExpression to AXIS MetadataFilter format
    fn convert_filter_to_index(
        filter_expression: Option<&FilterExpression>,
    ) -> Vec<IndexMetadataFilter> {
        let Some(filter) = filter_expression else {
            return Vec::new();
        };

        // Convert filter expressions to index metadata filters
        let mut index_filters = Vec::new();

        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Convert ComparisonOperator to IndexFilterOperator
                let index_operator = match operator {
                    ComparisonOperator::Equals => IndexFilterOperator::Equals,
                    ComparisonOperator::NotEquals => IndexFilterOperator::NotEquals,
                    ComparisonOperator::GreaterThan => IndexFilterOperator::GreaterThan,
                    ComparisonOperator::GreaterThanOrEqual => {
                        IndexFilterOperator::GreaterThanOrEqual
                    }
                    ComparisonOperator::LessThan => IndexFilterOperator::LessThan,
                    ComparisonOperator::LessThanOrEqual => IndexFilterOperator::LessThanOrEqual,
                    ComparisonOperator::In => IndexFilterOperator::In,
                    ComparisonOperator::NotIn => IndexFilterOperator::NotIn,
                    ComparisonOperator::Contains => IndexFilterOperator::Contains,
                    ComparisonOperator::StartsWith => IndexFilterOperator::StartsWith,
                    ComparisonOperator::EndsWith => IndexFilterOperator::EndsWith,
                    ComparisonOperator::Like => IndexFilterOperator::Like,
                    ComparisonOperator::Between => IndexFilterOperator::Between,
                    _ => {
                        debug!(
                            "Operator {:?} not directly supported by index, will use post-filtering",
                            operator
                        );
                        return index_filters;
                    }
                };

                index_filters.push(IndexMetadataFilter {
                    field: field.clone(),
                    operator: index_operator,
                    value: value.clone(),
                });
            }
            FilterExpression::And(filters) => {
                for f in filters {
                    index_filters.extend(Self::convert_filter_to_index(Some(f)));
                }
            }
            FilterExpression::Or(_) | FilterExpression::Not(_) => {
                // OR and NOT are not directly supported by the index, will use post-filtering
                debug!("OR/NOT filters not supported by index, will use post-filtering");
            }
        }

        index_filters
    }

    /// Execute direct search without orchestration
    async fn execute_direct_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        info!("🔍 SST: Using direct search strategy");

        self.fallback_to_direct_search(
            ctx,
            collection_id,
            storage_url,
            query_vector,
            k,
            distance_metric,
            filter_expression,
            true, // include_vectors
            true, // include_metadata
        )
        .await
    }

    /// Fallback direct search implementation
    ///
    /// This method implements a simplified but efficient search that:
    /// 1. Discovers relevant SSTable files
    /// 2. Searches each file using the unified reader
    /// 3. Combines and ranks results
    pub async fn fallback_to_direct_search(
        &self,
        ctx: &StorageQueryContext,
        collection_id: &str,
        storage_url: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: DistanceMetric,
        filter_expression: Option<&FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        tracing::debug!(
            "[SST] Starting direct search for collection {}, storage_url: {}",
            collection_id,
            storage_url
        );

        let mut all_candidates = Vec::new();

        // Phase 7.2 warm-path profiling: emit per-phase elapsed
        // timings under the dedicated `sst_warm_phase` target so a
        // bench-side tracing layer can aggregate without touching
        // unrelated logs. These calls cost one `Instant::now()` per
        // phase boundary — negligible at search granularity.
        let discovery_start = std::time::Instant::now();

        // Discover SSTable files for this collection with optional centroid pruning
        // When SearchMode is Approximate, uses centroid-based IVF-style optimization
        tracing::debug!(storage_url = %storage_url, "Discovering SSTable files");
        let search_mode = &ctx.search_params.search_mode;

        // **Honor SearchMode::Exact (2026-05-30)**: when the caller
        // asked for an exact search, force `block_prune.force_exact = true`
        // so the sqrt-based centroid block pruning doesn't silently
        // drop recall. Without this override, an Exact search at 100K
        // (where the SST has ≥100 blocks) keeps only `sqrt(num_blocks)`
        // blocks via centroid distance — measured 5% recall vs true
        // brute force on random data. Customers asking for `Exact` get
        // approximate results otherwise.
        //
        // For `SearchMode::Approximate` and `SearchMode::Adaptive` the
        // caller's `block_prune` config flows through unchanged.
        let prune_config_owned;
        let prune_config: &crate::core::search::BlockPruneConfig =
            if matches!(search_mode, crate::core::search::SearchMode::Exact)
                && !ctx.search_params.block_prune.force_exact
            {
                prune_config_owned = crate::core::search::BlockPruneConfig {
                    force_exact: true,
                    ..ctx.search_params.block_prune.clone()
                };
                &prune_config_owned
            } else {
                &ctx.search_params.block_prune
            };
        let sstable_files = self
            .discover_sstable_files_with_centroid_pruning(
                storage_url,
                query_vector,
                distance_metric,
                search_mode,
                prune_config,
            )
            .await?;
        let discovery_us = discovery_start.elapsed().as_micros() as u64;
        tracing::info!(
            target: "sst_warm_phase",
            phase = "discovery",
            elapsed_us = discovery_us,
            file_count = sstable_files.len(),
            "phase done"
        );
        tracing::debug!(
            "[SST] Discovered {} SSTable files (search_mode={:?})",
            sstable_files.len(),
            search_mode
        );
        for (i, file) in sstable_files.iter().enumerate() {
            tracing::trace!(index = i, file = %file, "Discovered SSTable file");
        }

        debug!(
            "🔍 SST: Found {} SSTable files for collection {}",
            sstable_files.len(),
            collection_id
        );

        // Search each SSTable file with block-level pruning. Reuse the
        // SearchMode::Exact-aware prune_config from above so the
        // per-file scan also honors `force_exact` when the caller
        // asked for an exact search.
        let scan_start = std::time::Instant::now();
        let block_prune = prune_config;
        for (file_idx, sstable_path) in sstable_files.iter().enumerate() {
            trace!(
                "SST: Searching file [{}/{}]: {} (force_exact={})",
                file_idx + 1,
                sstable_files.len(),
                sstable_path,
                block_prune.force_exact
            );

            // Dispatch based on file format (Arrow vs ProximaBlocks)
            let search_result = if sstable_path.ends_with(".arrow") {
                // Use ArrowBlockReader for Arrow format files
                self.search_arrow_file(
                    sstable_path,
                    query_vector,
                    filter_expression.cloned(),
                    k, // Use exact k
                    distance_metric,
                )
                .await
            } else {
                // Use SSTable reader for ProximaBlocks format
                // Choose execution strategy based on flags (TD-041, TD-039, TD-031)
                let use_parallel_morsels =
                    ctx.search_params.enable_parallel_morsels.unwrap_or(false);
                let use_vectorized = ctx
                    .search_params
                    .enable_vectorized_execution
                    .unwrap_or(false);
                let use_pipeline = ctx.search_params.enable_pipeline_execution.unwrap_or(false);

                if use_pipeline {
                    trace!("SST: Using pipeline-based execution path (TD-031)");
                    self.sstable_reader()
                        .search_with_pipeline_execution(
                            sstable_path,
                            query_vector,
                            filter_expression.cloned(),
                            k, // Use exact k
                            distance_metric,
                            Some(&*ctx.collection),
                            block_prune,
                        )
                        .await
                } else if use_parallel_morsels {
                    trace!("SST: Using parallel morsel execution path (TD-039)");
                    self.sstable_reader()
                        .search_with_filter_parallel_morsels(
                            sstable_path,
                            query_vector,
                            filter_expression.cloned(),
                            k, // Use exact k
                            distance_metric,
                            Some(&*ctx.collection),
                            block_prune,
                            None, // Use default worker count (CPU cores)
                        )
                        .await
                } else if use_vectorized {
                    trace!("SST: Using vectorized execution path (TD-041)");
                    self.sstable_reader()
                        .search_with_filter_vectorized(
                            sstable_path,
                            query_vector,
                            filter_expression.cloned(),
                            k, // Use exact k
                            distance_metric,
                            Some(&*ctx.collection),
                            block_prune,
                        )
                        .await
                } else {
                    trace!("SST: Using scalar execution path");
                    self.sstable_reader()
                        .search_with_filter_and_pruning(
                            sstable_path,
                            query_vector,
                            filter_expression.cloned(),
                            k, // Use exact k
                            distance_metric,
                            Some(&*ctx.collection), // Pass collection for type-safe metadata deserialization
                            block_prune, // Pass block pruning config for Z-order/centroid pruning
                        )
                        .await
                }
            };

            match search_result {
                Ok(results) => {
                    trace!(
                        "SST: Found {} candidates in file {}",
                        results.len(),
                        file_idx + 1
                    );
                    all_candidates.extend(results);
                }
                Err(e) => {
                    warn!("SST: Failed to search file {}: {}", sstable_path, e);
                    // Continue with other files
                }
            }
        }

        let scan_us = scan_start.elapsed().as_micros() as u64;
        let candidate_count_before_merge = all_candidates.len();
        tracing::info!(
            target: "sst_warm_phase",
            phase = "per_file_scan",
            elapsed_us = scan_us,
            file_count = sstable_files.len(),
            candidate_count = candidate_count_before_merge,
            "phase done"
        );

        // Use bounded priority queue for efficient top-k selection
        let merge_start = std::time::Instant::now();
        let mut priority_queue = BoundedPriorityQueue::new(k);

        // Insert all candidates into bounded queue
        for candidate in all_candidates {
            priority_queue.try_insert(candidate);
        }

        // Get sorted results from bounded queue
        let mut all_candidates = priority_queue.into_sorted_vec();
        let merge_us = merge_start.elapsed().as_micros() as u64;
        tracing::info!(
            target: "sst_warm_phase",
            phase = "topk_merge",
            elapsed_us = merge_us,
            candidate_count = candidate_count_before_merge,
            "phase done"
        );
        tracing::debug!(candidate_count = all_candidates.len(), "Before filtering");

        // Filter results based on include flags
        let filter_start = std::time::Instant::now();
        self.filter_search_results(&mut all_candidates, include_vectors, include_metadata);
        let filter_us = filter_start.elapsed().as_micros() as u64;
        tracing::info!(
            target: "sst_warm_phase",
            phase = "result_filter",
            elapsed_us = filter_us,
            "phase done"
        );
        tracing::debug!(filtered_count = all_candidates.len(), "After filtering");

        info!(
            "🏁 SST: Direct search completed - Collection: {}, Results: {}/{}",
            collection_id,
            all_candidates.len(),
            k
        );

        Ok(all_candidates)
    }

    /// Discover SSTable files with optional centroid-based pruning (LanceDB-inspired IVF optimization)
    ///
    /// When `search_mode` is Approximate, this method:
    /// 1. Loads headers from all SST files to get centroids
    /// 2. Computes distance from query to each centroid
    /// 3. Returns only the top nprobe files (closest centroids to query)
    /// 4. This can skip 80-90% of files for large datasets
    async fn discover_sstable_files_with_centroid_pruning(
        &self,
        storage_url: &str,
        query_vector: &[f32],
        distance_metric: proximadb_distance_kernel::DistanceMetric,
        search_mode: &crate::core::search::SearchMode,
        prune_config: &crate::core::search::BlockPruneConfig, // [AGENT_FIX] New parameter
    ) -> Result<Vec<String>> {
        use crate::core::search::SearchMode;

        // First get all files
        let all_files = self.discover_sstable_files(storage_url).await?;

        // [AGENT_FIX] Use the configured min_keep value instead of a hardcoded number.
        let min_keep = prune_config.min_keep.max(1);

        // OPTIMIZATION: Skip file-level pruning for small datasets where overhead exceeds benefit.
        // Loading centroids from each file header and computing distances has significant I/O
        // and CPU overhead. Only worth it when we have many files to prune.
        use crate::storage::engines::core::constants::pruning;
        if all_files.len() < pruning::MIN_FILES_FOR_PRUNING {
            tracing::debug!(
                "SST file pruning skipped: {} files < {} threshold (overhead would exceed benefit)",
                all_files.len(),
                pruning::MIN_FILES_FOR_PRUNING
            );
            return Ok(all_files);
        }

        // For exact mode or small datasets (<= min_keep), search all files
        if matches!(search_mode, SearchMode::Exact) || all_files.len() <= min_keep {
            if !matches!(search_mode, SearchMode::Exact) {
                tracing::warn!(
                    "Fewer than or equal to `min_keep` ({}) SST files, forcing exact search.",
                    min_keep
                );
            }
            return Ok(all_files);
        }

        // Only Approximate and large-dataset Adaptive searches should use centroid pruning.
        if !matches!(
            search_mode,
            SearchMode::Approximate { .. } | SearchMode::Adaptive { .. }
        ) {
            tracing::warn!(
                "Search mode is neither 'Approximate' nor 'Adaptive', forcing exact search by returning all files."
            );
            return Ok(all_files);
        }

        if let SearchMode::Adaptive { threshold } = search_mode {
            let estimated_dataset_size = all_files.len() * 1000;
            if estimated_dataset_size < *threshold {
                tracing::debug!(
                    "Adaptive SST pruning skipped: estimated dataset size {} < threshold {}",
                    estimated_dataset_size,
                    threshold
                );
                return Ok(all_files);
            }
        }

        // Calculate effective nprobe based on search mode and number of files
        let nprobe = search_mode.effective_nprobe(all_files.len(), all_files.len() * 1000); // Estimate 1000 vectors per file

        // If nprobe >= number of files, search all
        if nprobe >= all_files.len() {
            return Ok(all_files);
        }

        // Load headers and compute centroid distances
        let mut file_distances: Vec<(String, f32)> = Vec::new();

        for file_path in &all_files {
            match self.load_sst_header_centroid(file_path).await {
                Ok(Some((centroid, _max_distance_to_centroid))) => {
                    if centroid.len() == query_vector.len() {
                        // Compute distance from query to file centroid
                        let distance = self.compute_centroid_distance(
                            query_vector,
                            &centroid,
                            distance_metric,
                        );
                        file_distances.push((file_path.clone(), distance));
                    } else {
                        // Dimension mismatch - include file anyway
                        file_distances.push((file_path.clone(), 0.0));
                    }
                }
                Ok(None) => {
                    // No centroid - include file anyway (for backwards compatibility)
                    file_distances.push((file_path.clone(), 0.0));
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load centroid from {}: {}, including anyway",
                        file_path,
                        e
                    );
                    file_distances.push((file_path.clone(), 0.0));
                }
            }
        }

        // Sort by distance (ascending - closest first for similarity search)
        file_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Return top nprobe files
        let selected_files: Vec<String> = file_distances
            .into_iter()
            .take(nprobe)
            .map(|(path, _)| path)
            .collect();

        debug!(
            "🎯 SST Centroid pruning: selected {}/{} files (nprobe={})",
            selected_files.len(),
            all_files.len(),
            nprobe
        );

        Ok(selected_files)
    }

    /// Load centroid from SST header for partition-aware search
    async fn load_sst_header_centroid(&self, file_path: &str) -> Result<Option<(Vec<f32>, f32)>> {
        use crate::storage::engines::sst::SstableHeader;

        let fs = self.filesystem().get_filesystem(file_path)?;

        // Read just the first part of the file to get header
        // Format: SST1 (4 bytes) + header_len (4 bytes) + header data
        let header_prefix = fs.read_range(file_path, 0, 8).await?;

        // Verify magic
        if &header_prefix[0..4] != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SST file format"));
        }

        let header_len = u32::from_le_bytes([
            header_prefix[4],
            header_prefix[5],
            header_prefix[6],
            header_prefix[7],
        ]) as usize;

        // Read header data
        let header_data = fs.read_range(file_path, 8, header_len as u64).await?;

        // Deserialize header
        let header: SstableHeader = bincode::deserialize(&header_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize SST header: {}", e))?;

        // Return centroid and max_distance if available
        if let Some(centroid) = header.centroid {
            let max_dist = header.max_distance_to_centroid.unwrap_or(f32::MAX);
            Ok(Some((centroid, max_dist)))
        } else {
            Ok(None)
        }
    }

    /// Compute distance from query to centroid
    fn compute_centroid_distance(
        &self,
        query: &[f32],
        centroid: &[f32],
        metric: proximadb_distance_kernel::DistanceMetric,
    ) -> f32 {
        use proximadb_distance_kernel::DistanceMetric;

        match metric {
            DistanceMetric::Euclidean => {
                let mut sum = 0.0f32;
                for (q_val, c_val) in query.iter().zip(centroid.iter()) {
                    let diff = q_val - c_val;
                    sum += diff * diff;
                }
                sum.sqrt()
            }
            DistanceMetric::Cosine | DistanceMetric::DotProduct => {
                // For cosine/IP, we want to maximize similarity
                // Return 1 - cosine_similarity as "distance"
                let mut dot = 0.0f32;
                let mut norm_q = 0.0f32;
                let mut norm_c = 0.0f32;
                for (q_val, c_val) in query.iter().zip(centroid.iter()) {
                    dot += q_val * c_val;
                    norm_q += q_val * q_val;
                    norm_c += c_val * c_val;
                }
                let denom = (norm_q * norm_c).sqrt();
                if denom > 0.0 {
                    1.0 - (dot / denom)
                } else {
                    1.0
                }
            }
            _ => {
                // Default to Euclidean for other metrics
                let mut sum = 0.0f32;
                for (q_val, c_val) in query.iter().zip(centroid.iter()) {
                    let diff = q_val - c_val;
                    sum += diff * diff;
                }
                sum.sqrt()
            }
        }
    }

    /// Discover SSTable files for a collection.
    ///
    /// `pub(crate)` so the flush path can reuse it for the L0 compaction
    /// trigger (TD-114) without duplicating segment discovery.
    pub(crate) async fn discover_sstable_files(&self, storage_url: &str) -> Result<Vec<String>> {
        tracing::debug!(
            "[SST] discover_sstable_files called with storage_url: {}",
            storage_url
        );

        let mut files = Vec::new();

        // storage_url is already the correct data directory path from collection_storage_path()
        // No need to parse and reconstruct - use it directly
        let data_url = storage_url;

        // List files in the collection directory
        let fs = self.filesystem().get_filesystem(data_url)?;
        tracing::debug!("[SST] Got filesystem for data_url: {}", data_url);

        // Handle case where directory doesn't exist yet (e.g., before first flush)
        let entries = match fs.list(data_url).await {
            Ok(entries) => {
                tracing::debug!("[SST] Found {} entries in {}", entries.len(), data_url);
                entries
            }
            Err(e) if e.to_string().contains("No such file or directory") => {
                tracing::warn!("[SST] Directory doesn't exist yet: {}", data_url);
                return Ok(files);
            }
            Err(e) => {
                tracing::error!("[SST] Failed to list directory {}: {:?}", data_url, e);
                return Err(anyhow::anyhow!(
                    "Failed to list directory {}: {}",
                    data_url,
                    e
                ));
            }
        };

        for entry in entries {
            tracing::trace!(
                "[SST] Examining entry: name={}, url={}, is_dir={}",
                entry.name,
                entry.url,
                entry.metadata.is_directory
            );
            if !entry.metadata.is_directory
                && (entry.name.ends_with(".sst") || entry.name.ends_with(".arrow"))
            {
                files.push(entry.url);
                tracing::debug!("[SST] Found data file: {}", entry.name);
            }
        }

        tracing::debug!(
            "[SST] Discovered {} .sst files in {}",
            files.len(),
            data_url
        );
        Ok(files)
    }

    /// Parse storage URL to extract base URL and collection ID
    #[allow(dead_code)]
    #[allow(dead_code)]
    fn parse_storage_url(&self, storage_url: &str) -> Result<(String, String)> {
        // Fallback: assume storage_url is base_url/collection_id format
        if let Some(last_slash) = storage_url.rfind('/') {
            let base = &storage_url[..last_slash];
            let collection = &storage_url[last_slash + 1..];
            Ok((base.to_string(), collection.to_string()))
        } else {
            Err(
                SstError::InvalidArgument(format!("Invalid storage URL format: {}", storage_url))
                    .into(),
            )
        }
    }

    /// Filter search results based on include flags
    #[expect(clippy::ptr_arg)] // Accepting &mut Vec for API compatibility
    fn filter_search_results(
        &self,
        results: &mut Vec<OptimizedSearchRecord>,
        include_vectors: bool,
        include_metadata: bool,
    ) {
        if !include_vectors {
            for result in results.iter_mut() {
                result.vector = None;
            }
        }

        if !include_metadata {
            for result in results.iter_mut() {
                result.metadata = HashMap::new();
            }
        }
    }

    /// List SSTable files for search in a specific directory
    pub async fn list_sstable_files_for_search(&self, data_dir: &str) -> Result<Vec<String>> {
        let mut sstable_files = Vec::new();

        // Use filesystem to list files directly
        if let Ok(mut entries) = tokio::fs::read_dir(data_dir).await {
            while let Some(entry) = entries.next_entry().await? {
                if let Some(name) = entry.file_name().to_str()
                    && (name.ends_with(".sst") || name.ends_with(".arrow"))
                {
                    sstable_files.push(format!("{}/{}", data_dir, name));
                }
            }
        }

        debug!(
            "📋 Listed {} SSTable files in {}",
            sstable_files.len(),
            data_dir
        );
        Ok(sstable_files)
    }

    /// Search within an Arrow format file
    ///
    /// Uses ArrowBlockReader to read and search through Arrow IPC files,
    /// providing the same interface as SSTable searches for seamless integration.
    async fn search_arrow_file(
        &self,
        arrow_path: &str,
        query_vector: &[f32],
        _filter_expression: Option<FilterExpression>,
        limit: usize,
        distance_metric: DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        use proximadb_distance_kernel::engine::{SimilarityResult, UnifiedDistanceCompute};
        use std::sync::Arc;

        debug!("🏹 Searching Arrow file: {}", arrow_path);

        // Convert file:// URL to local path
        let local_path = arrow_path.strip_prefix("file://").unwrap_or(arrow_path);

        // Open the Arrow file reader
        let reader = ArrowBlockReader::open(local_path)
            .map_err(|e| anyhow::anyhow!("Failed to open Arrow file {}: {}", arrow_path, e))?;

        // Read all records from the Arrow file
        let records = reader
            .read_all()
            .map_err(|e| anyhow::anyhow!("Failed to read Arrow file {}: {}", arrow_path, e))?;

        trace!("🏹 Arrow file contains {} records", records.len());

        // Create distance computer with the specified metric
        let distance_computer = UnifiedDistanceCompute::new(distance_metric);

        // Score all records
        // Note: Metadata filtering for Arrow files is simplified - for full filter support,
        // use ProximaBlocks format which has optimized filter evaluation
        let mut candidates: Vec<OptimizedSearchRecord> =
            Vec::with_capacity(records.len().min(limit));

        for record in &records {
            let Some(embedding) = record.embeddings.first() else {
                continue;
            };
            if embedding.values.is_empty() {
                continue;
            }

            // Compute raw distance
            let raw_distance = distance_computer.distance(query_vector, &embedding.as_fp32_cow());

            // Use SimilarityResult to get normalized_score (higher = more similar)
            // This ensures consistency with the rest of the codebase and BoundedPriorityQueue
            let similarity_result = SimilarityResult::new(raw_distance, distance_metric);

            // OptimizedSearchRecord uses canonical ProximaValue metadata internally.
            let metadata = proximadb_records::conversions::proxima_tree_to_value_map(&record.props);

            candidates.push(OptimizedSearchRecord {
                id: record.oid.clone(),
                vector_id: Some(record.oid.clone()),
                score: similarity_result.normalized_score, // Use normalized_score (higher = better)
                similarity: Some(similarity_result.normalized_score),
                vector: Some(Arc::new(embedding.values.to_fp32_owned())),
                metadata,
                ..Default::default()
            });
        }

        // Sort by score descending (higher normalized_score = more similar = better)
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);

        debug!("🏹 Arrow search found {} candidates", candidates.len());
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use proximadb_data_model::ProximaValue;
    use proximadb_distance_kernel::engine::UnifiedDistanceCompute;
    use std::sync::Arc;

    /// ADR-030 / TD-158: the SST `ComputeMsGuard` records elapsed compute to the
    /// active per-query I/O trace on drop, under the engine label — so the
    /// billing observer can attribute KRU to "sst". No-op outside a scope.
    #[tokio::test]
    async fn compute_ms_guard_records_to_active_io_trace() {
        use crate::observability::io_trace;
        let snap = io_trace::scope(async {
            {
                let _g = ComputeMsGuard::new("sst");
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            } // guard drops here → records elapsed into the active trace
            io_trace::snapshot()
        })
        .await
        .expect("snapshot inside an active io_trace scope");
        assert!(
            snap.compute_ms.contains_key("sst"),
            "guard must record compute under the 'sst' engine label, got {:?}",
            snap.compute_ms
        );
        assert!(
            snap.total_compute_ms() >= 1,
            "elapsed compute must be >= 1ms after a 2ms sleep, got {}",
            snap.total_compute_ms()
        );
    }

    #[tokio::test]
    async fn test_parse_storage_url() {
        let engine = create_test_engine().await;

        // Test valid storage URL
        let (base, collection) = engine
            .parse_storage_url("file:///data/collections/test_collection")
            .unwrap();
        assert_eq!(base, "file:///data/collections");
        assert_eq!(collection, "test_collection");

        // Test invalid storage URL
        assert!(engine.parse_storage_url("invalid_url").is_err());
    }

    #[tokio::test]
    async fn test_filter_search_results() {
        let engine = create_test_engine().await;
        let mut results = vec![
            create_test_search_result("id1", vec![1.0, 2.0], 0.5),
            create_test_search_result("id2", vec![3.0, 4.0], 0.3),
        ];

        // Test removing vectors
        engine.filter_search_results(&mut results, false, true);
        assert!(results[0].vector.is_none());
        assert!(results[1].vector.is_none());

        // Test removing metadata
        let mut results = vec![create_test_search_result("id1", vec![1.0, 2.0], 0.5)];
        engine.filter_search_results(&mut results, true, false);
        assert!(results[0].metadata.is_empty());
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
    }

    fn create_test_search_result(id: &str, values: Vec<f32>, score: f32) -> OptimizedSearchRecord {
        let mut record = OptimizedSearchRecord::default();
        record.id = id.to_string();
        record.score = score;
        record.vector = Some(Arc::new(values));
        record.metadata = {
            let mut metadata = HashMap::new();
            metadata.insert(
                "test_key".to_string(),
                ProximaValue::String("test_value".to_string()),
            );
            metadata
        };
        record
    }

    // ---- ADR-028 index_policy → exact-scan budget resolution ----

    use crate::proto::proximadb_v1::IndexPolicy;

    #[test]
    fn resolve_budget_none_uses_storage_class_default() {
        let (budget, pin_exact) = resolve_exact_budget(None);
        assert_eq!(budget, EXACT_SCAN_MAX_BYTES);
        assert!(!pin_exact);
    }

    #[test]
    fn resolve_budget_mode_exact_pins_exact() {
        let p = IndexPolicy {
            mode: "Exact".to_string(),
            ..Default::default()
        };
        let (_, pin_exact) = resolve_exact_budget(Some(&p));
        assert!(pin_exact, "mode=exact must pin exact regardless of N");
    }

    #[test]
    fn resolve_budget_nonzero_override_wins() {
        let p = IndexPolicy {
            mode: "auto".to_string(),
            byte_budget: 8 * 1024 * 1024,
            ..Default::default()
        };
        let (budget, pin_exact) = resolve_exact_budget(Some(&p));
        assert_eq!(budget, 8 * 1024 * 1024);
        assert!(!pin_exact);
    }

    #[test]
    fn resolve_budget_zero_override_falls_back_to_default() {
        let p = IndexPolicy {
            mode: "auto".to_string(),
            byte_budget: 0,
            ..Default::default()
        };
        let (budget, _) = resolve_exact_budget(Some(&p));
        assert_eq!(budget, EXACT_SCAN_MAX_BYTES);
    }
}
