//! Unified API Handlers for the platform runtime layer.
//!
//! ## Architecture
//!
//! `UnifiedHandlers` is the composition root that wires together injected service
//! ports into a coherent API surface.  Each service dependency is expressed as a
//! port trait (`CollectionPort`, `VectorOpsPort`, `QueryAdapterPort`) so no
//! root-crate concrete type crosses the crate boundary.
//!
//! ## Migration status
//!
//! The real implementation still lives in `src/api_handlers/request_handlers.rs`
//! in the root crate, which implements `ApiHandlersPort` via delegation.  This
//! stub will replace it once the concrete services are extracted to this crate.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::service_ports::{CollectionPort, QueryAdapterPort, VectorOpsPort};

/// Global request counter for generating unique request IDs.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a 16-char hex request ID (8 chars timestamp + 8 chars counter).
pub fn generate_request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0);
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    format!("{:08x}{:08x}", timestamp, counter)
}

// ── Collection ID cache ───────────────────────────────────────────────────────

const COLLECTION_ID_CACHE_TTL_SECS: u64 = 300;
const COLLECTION_ID_CACHE_MAX_SIZE: usize = 1000;

#[derive(Clone)]
struct CollectionIdCacheEntry {
    collection_id: String,
    cached_at: Instant,
}

/// Thread-safe TTL-based cache for collection ID resolution.
///
/// Reduces metadata backend lookups from ~5 ms/request to ~0.1 ms on cache hits.
pub struct CollectionIdCache {
    cache: std::sync::RwLock<HashMap<String, CollectionIdCacheEntry>>,
    ttl: Duration,
    max_size: usize,
}

impl CollectionIdCache {
    pub fn new() -> Self {
        Self {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(COLLECTION_ID_CACHE_TTL_SECS),
            max_size: COLLECTION_ID_CACHE_MAX_SIZE,
        }
    }

    pub fn get(&self, collection_name: &str) -> Option<String> {
        let cache = self.cache.read().ok()?;
        let entry = cache.get(collection_name)?;
        if entry.cached_at.elapsed() > self.ttl {
            return None;
        }
        Some(entry.collection_id.clone())
    }

    pub fn put(&self, collection_name: String, collection_id: String) {
        if let Ok(mut cache) = self.cache.write() {
            let now = Instant::now();
            cache.retain(|_, entry| now.duration_since(entry.cached_at) < self.ttl);
            while cache.len() >= self.max_size {
                let oldest_key = cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.cached_at)
                    .map(|(key, _)| key.clone());
                if let Some(key) = oldest_key {
                    cache.remove(&key);
                } else {
                    break;
                }
            }
            cache.insert(
                collection_name,
                CollectionIdCacheEntry {
                    collection_id,
                    cached_at: now,
                },
            );
        }
    }
}

impl Default for CollectionIdCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Placeholder for hybrid runtime config ─────────────────────────────────────

/// Placeholder for hybrid runtime configuration (weights, seeding).
pub struct HybridRuntimeConfig;

// ── UnifiedHandlers ───────────────────────────────────────────────────────────

/// Composition root that wires service ports into the API surface.
///
/// Hold `Arc<dyn CollectionPort>`, `Arc<dyn VectorOpsPort>`, and optionally an
/// `Arc<dyn QueryAdapterPort>` so the actual business logic can be injected at
/// server startup (root crate's concrete services) without `proximadb-runtime`
/// knowing about their implementations.
pub struct UnifiedHandlers {
    /// Collection lifecycle operations.
    pub collection: Arc<dyn CollectionPort>,
    /// Vector CRUD and search operations.
    pub vector_ops: Arc<dyn VectorOpsPort>,
    /// Optional unified query facade for SQL / hybrid routing.
    pub query_adapter: Option<Arc<dyn QueryAdapterPort>>,
    /// Optional hybrid runtime configuration.
    pub hybrid_runtime: Arc<std::sync::RwLock<Option<HybridRuntimeConfig>>>,
    /// Cache for collection ID resolution.
    pub collection_id_cache: CollectionIdCache,
}

impl UnifiedHandlers {
    /// Construct from injected service port implementations.
    pub fn new(
        collection: Arc<dyn CollectionPort>,
        vector_ops: Arc<dyn VectorOpsPort>,
        query_adapter: Option<Arc<dyn QueryAdapterPort>>,
    ) -> Self {
        Self {
            collection,
            vector_ops,
            query_adapter,
            hybrid_runtime: Arc::new(std::sync::RwLock::new(None)),
            collection_id_cache: CollectionIdCache::new(),
        }
    }
}
