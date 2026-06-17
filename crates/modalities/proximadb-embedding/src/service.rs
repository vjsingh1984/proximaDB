//! The `EmbeddingService` singleton.
//!
//! Loaded once at process start. All worker threads clone the `Arc` (cheap
//! atomic refcount) and call `embed_*` through `&self`. No `RwLock` on the
//! hot path — every member is read-only after `initialize()`.

use std::sync::Arc;

use dashmap::DashMap;
use once_cell::sync::OnceCell;
use tracing::info;

use crate::config::{EmbedRoute, EmbeddingConfig};
use crate::models::ModelRegistry;
use crate::scheduler::{EmbedScheduler, EmbedSchedulerConfig, IngestMode, SchedulerStats};
use crate::tokenizer::SharedTokenizer;
use crate::{EmbeddingError, Result};

/// A single record submitted for embedding. The optional `tenant_id` lets
/// the service resolve the embedding route per-record (Approach B drainer
/// batches records by tenant before dispatch).
#[derive(Debug, Clone)]
pub struct EmbedRecord {
    pub id: String,
    pub text: String,
    pub tenant_id: String,
}

/// A batch of records to embed together. All records in a batch SHOULD share
/// the same `EmbedRoute`; the drainer enforces this by grouping records by
/// route before submission.
#[derive(Debug, Clone)]
pub struct EmbedBatch {
    pub records: Vec<EmbedRecord>,
    pub mode: IngestMode,
}

/// Result of an embedding batch — one vector per input record, plus the
/// route that was actually used (so the catalog can validate dimension).
#[derive(Debug, Clone)]
pub struct EmbedResult {
    pub vectors: Vec<Vec<f32>>,
    pub route: EmbedRoute,
}

/// INT-2.5c result for the precision-aware embed path
/// ([`EmbeddingService::embed_sync_at_precision`]).
///
/// Returns typed [`crate::EmbeddingValues`] so the caller can hand them
/// directly to [`proximadb_records::EmbeddingCell::new_typed`] without a
/// fp32 round-trip. Also returns the [`crate::BatchConversionSummary`]
/// so the caller can populate the PR 7b
/// `proximadb_embedding_precision_conversions_total{from,to,site}`
/// counter — same accounting hand-off as INT-1's
/// `Models::embed_batch_at_precision`.
#[derive(Debug, Clone)]
pub struct EmbedResultTyped {
    pub values: Vec<crate::EmbeddingValues>,
    pub route: EmbedRoute,
    pub summary: crate::BatchConversionSummary,
}

/// Cached tenant route. The DashMap entry expires after `ttl_secs` since
/// the tenant registry can change (tier upgrade, BYO endpoint change).
#[derive(Debug, Clone)]
struct CachedRoute {
    route: EmbedRoute,
    cached_at: std::time::Instant,
}

const ROUTE_CACHE_TTL_SECS: u64 = 60;

pub struct EmbeddingService {
    models: ModelRegistry,
    tokenizer: SharedTokenizer,
    scheduler: EmbedScheduler,
    tenant_cache: DashMap<String, CachedRoute>,
    default_config: EmbeddingConfig,
}

static GLOBAL: OnceCell<Arc<EmbeddingService>> = OnceCell::new();

impl EmbeddingService {
    /// Initialize the global singleton. Call once at process start
    /// (typically from `proximadb-server` main during startup).
    pub fn initialize(
        default_config: EmbeddingConfig,
        scheduler_config: EmbedSchedulerConfig,
    ) -> Result<Arc<Self>> {
        let service = Self {
            models: ModelRegistry::initialize()?,
            tokenizer: SharedTokenizer::initialize()?,
            scheduler: EmbedScheduler::new(scheduler_config)?,
            tenant_cache: DashMap::with_capacity(1024),
            default_config,
        };
        let arc = Arc::new(service);
        GLOBAL.set(arc.clone()).map_err(|_| {
            EmbeddingError::Other(anyhow::anyhow!("EmbeddingService already initialized"))
        })?;
        info!("proximadb-embedding singleton initialized");
        Ok(arc)
    }

    /// Access the global singleton. Panics if [`initialize`] was not called
    /// during process startup.
    pub fn global() -> Arc<Self> {
        GLOBAL
            .get()
            .cloned()
            .expect("EmbeddingService::initialize() must be called at process start")
    }

    /// Try-get the singleton (None if not initialized). Useful in tests.
    pub fn try_global() -> Option<Arc<Self>> {
        GLOBAL.get().cloned()
    }

    /// Resolve the embedding route for a tenant.
    ///
    /// Hot path: consults `tenant_cache` first. On miss or stale, the caller
    /// (typically the AnvaiOps tenant registry adapter) supplies the fresh
    /// route via [`update_tenant_route`].
    pub fn resolve_route(&self, tenant_id: &str) -> EmbedRoute {
        if let Some(cached) = self.tenant_cache.get(tenant_id)
            && cached.cached_at.elapsed().as_secs() < ROUTE_CACHE_TTL_SECS
        {
            return cached.route.clone();
        }
        // Cache miss — return default route. Higher layers refresh the cache
        // by calling `update_tenant_route()` after consulting the tenant registry.
        self.default_config.route.clone()
    }

    /// Refresh the cached route for a tenant. Typically called after a
    /// tenant registry lookup.
    pub fn update_tenant_route(&self, tenant_id: impl Into<String>, route: EmbedRoute) {
        let id = tenant_id.into();
        self.tenant_cache.insert(
            id,
            CachedRoute {
                route,
                cached_at: std::time::Instant::now(),
            },
        );
    }

    /// Synchronous embed — used by Approach A (pre-WAL) sync ingest path.
    /// Returns once the embedding work completes.
    pub async fn embed_sync(self: &Arc<Self>, batch: EmbedBatch) -> Result<EmbedResult> {
        let route = if batch.records.is_empty() {
            self.default_config.route.clone()
        } else {
            // Records in a batch share a tenant by construction (the drainer
            // groups by tenant before dispatching).
            self.resolve_route(&batch.records[0].tenant_id)
        };

        let service = self.clone();
        let texts: Vec<String> = batch.records.iter().map(|r| r.text.clone()).collect();
        let route_inner = route.clone();
        let rx = self
            .scheduler
            .submit_sync(move || service.models.embed_batch(&route_inner, &texts))?;
        rx.await
            .map_err(|_| EmbeddingError::Other(anyhow::anyhow!("scheduler dropped")))?
            .map(|vectors| EmbedResult { vectors, route })
    }

    /// INT-2.5c: synchronous embed at a caller-declared canonical
    /// precision. Returns typed [`crate::EmbeddingValues`] so callers
    /// can build [`proximadb_records::EmbeddingCell`]s via `new_typed`
    /// without a fp32 round-trip + downconvert.
    ///
    /// The `canonical` parameter should come from the target collection's
    /// `canonical_embedding_precision` field (PR 6b CatalogTableSchema).
    /// The drainer's current shape batches across collections so per-
    /// collection lookup + grouping is a follow-up; this method is the
    /// contract new callers (e.g. precision-aware REST handlers) use
    /// once they know the canonical precision for their request.
    ///
    /// Legacy `embed_sync` stays unchanged for callers that don't (yet)
    /// know the canonical precision — they get fp32 records like today.
    pub async fn embed_sync_at_precision(
        self: &Arc<Self>,
        batch: EmbedBatch,
        canonical: proximadb_records::EmbeddingScalarType,
    ) -> Result<EmbedResultTyped> {
        let route = if batch.records.is_empty() {
            self.default_config.route.clone()
        } else {
            self.resolve_route(&batch.records[0].tenant_id)
        };

        let service = self.clone();
        let texts: Vec<String> = batch.records.iter().map(|r| r.text.clone()).collect();
        let route_inner = route.clone();
        let rx = self.scheduler.submit_sync(move || {
            service
                .models
                .embed_batch_at_precision(&route_inner, &texts, canonical)
        })?;
        rx.await
            .map_err(|_| EmbeddingError::Other(anyhow::anyhow!("scheduler dropped")))?
            .map(|(values, summary)| EmbedResultTyped {
                values,
                route,
                summary,
            })
    }

    /// Asynchronous embed — Approach B post-WAL drainer dispatches a batch.
    /// Fire-and-forget; the drainer reads back from the WAL `pending_embed`
    /// state to track completion.
    pub fn embed_async(
        self: &Arc<Self>,
        batch: EmbedBatch,
        on_complete: impl FnOnce(Result<EmbedResult>) + Send + 'static,
    ) -> Result<()> {
        let route = if batch.records.is_empty() {
            self.default_config.route.clone()
        } else {
            self.resolve_route(&batch.records[0].tenant_id)
        };
        let service = self.clone();
        let route_inner = route.clone();
        self.scheduler.submit_async(move || {
            let texts: Vec<String> = batch.records.iter().map(|r| r.text.clone()).collect();
            let outcome = service
                .models
                .embed_batch(&route_inner, &texts)
                .map(|vectors| EmbedResult { vectors, route });
            on_complete(outcome);
            Ok(())
        })
    }

    /// Scheduler health for the `/api/v3/embed-scheduler/stats` endpoint
    /// and Prometheus exposition.
    pub fn scheduler_stats(&self) -> SchedulerStats {
        self.scheduler.stats()
    }

    /// Access the shared tokenizer (used by the chunker).
    pub fn tokenizer(&self) -> &SharedTokenizer {
        &self.tokenizer
    }
}
