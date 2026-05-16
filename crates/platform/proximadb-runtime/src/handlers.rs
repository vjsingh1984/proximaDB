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

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use proximadb_proto::v1::{
    CollectionOperation, CollectionRequest, CollectionResponse, ExecuteSqlResponse, HybridSearchRequest,
    HybridSearchResponse, SqlRow, SqlRowField, SqlValue, VectorBatchRequest, VectorOperationResponse,
    VectorSearchRequest, sql_value,
};

use crate::port::ApiHandlersPort;
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

// ── ApiHandlersPort implementation ───────────────────────────────────────────

#[async_trait]
impl ApiHandlersPort for UnifiedHandlers {
    async fn handle_collection_operation_for_tenant(
        &self,
        request: CollectionRequest,
        tenant_id: Option<&str>,
    ) -> Result<CollectionResponse> {
        let op = CollectionOperation::try_from(request.operation).unwrap_or(CollectionOperation::Unspecified);
        let collection_id = request.collection_id.as_deref().unwrap_or("");
        let start = Instant::now();

        let mut resp = CollectionResponse {
            operation: request.operation,
            ..Default::default()
        };

        match op {
            CollectionOperation::CollectionCreate => {
                let config = request
                    .collection_config
                    .ok_or_else(|| anyhow!("collection_config required for CREATE"))?;
                let col = self.collection.create_collection(config, tenant_id).await?;
                resp.success = true;
                resp.collection = Some(col);
            }
            CollectionOperation::CollectionUpdate => {
                let config = request
                    .collection_config
                    .ok_or_else(|| anyhow!("collection_config required for UPDATE"))?;
                let col = self.collection.update_collection(collection_id, config, tenant_id).await?;
                resp.success = true;
                resp.collection = Some(col);
            }
            CollectionOperation::CollectionGet => {
                let col = self.collection.get_collection(collection_id, tenant_id).await?;
                resp.success = col.is_some();
                resp.collection = col;
            }
            CollectionOperation::CollectionList => {
                let cols = self.collection.list_collections(tenant_id).await?;
                resp.success = true;
                resp.total_count = cols.len() as i64;
                resp.collections = cols;
            }
            CollectionOperation::CollectionDelete => {
                let deleted = self.collection.delete_collection(collection_id, tenant_id).await?;
                resp.success = deleted;
                resp.affected_count = if deleted { 1 } else { 0 };
            }
            CollectionOperation::CollectionGetIdByName => {
                let resolved = self.collection.resolve_collection_id(collection_id).await?;
                resp.success = resolved.is_some();
                if let Some(id) = resolved {
                    resp.metadata.insert("collection_id".to_string(), id);
                }
            }
            CollectionOperation::CollectionMigrate | CollectionOperation::Unspecified => {
                return Err(anyhow!("collection operation {:?} not implemented", op));
            }
        }

        resp.processing_time_us = start.elapsed().as_micros() as i64;
        Ok(resp)
    }

    async fn handle_vector_search_v1_for_tenant(
        &self,
        request: VectorSearchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse> {
        self.vector_ops.search(request, tenant_id).await
    }

    async fn handle_vector_search_v1(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorOperationResponse> {
        self.vector_ops.search(request, None).await
    }

    async fn handle_vector_batch_v1_for_tenant(
        &self,
        request: VectorBatchRequest,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse> {
        self.vector_ops.batch_upsert(request, tenant_id).await
    }

    async fn handle_vector_v1_for_tenant(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
        tenant_id: Option<&str>,
    ) -> Result<VectorOperationResponse> {
        self.vector_ops
            .get_vector(collection_id, vector_id, include_vector, include_metadata, tenant_id)
            .await
    }

    async fn execute_hybrid_query(
        &self,
        request: HybridSearchRequest,
    ) -> Result<HybridSearchResponse> {
        let adapter = self
            .query_adapter
            .as_ref()
            .ok_or_else(|| anyhow!("hybrid query requires QueryAdapterPort (not wired)"))?;
        adapter.execute_hybrid(request).await
    }

    async fn execute_sql_v1(
        &self,
        query: String,
        _parameters: Option<Vec<SqlValue>>,
        collection: Option<String>,
    ) -> Result<ExecuteSqlResponse> {
        let adapter = self
            .query_adapter
            .as_ref()
            .ok_or_else(|| anyhow!("SQL execution requires QueryAdapterPort (not wired)"))?;

        let start = Instant::now();
        let json_result = adapter.execute_sql(query, collection).await?;

        let records = json_result
            .get("records")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| json_result.as_array().cloned())
            .unwrap_or_default();

        let columns: Vec<String> = json_result
            .get("columns")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let rows: Vec<SqlRow> = records
            .iter()
            .map(|record| {
                let fields: Vec<SqlRowField> = match record.as_object() {
                    Some(obj) => obj
                        .iter()
                        .map(|(k, v)| SqlRowField {
                            key: k.clone(),
                            value: Some(json_to_sql_value(v)),
                        })
                        .collect(),
                    None => vec![SqlRowField {
                        key: "value".to_string(),
                        value: Some(json_to_sql_value(record)),
                    }],
                };
                SqlRow { fields, similarity: None }
            })
            .collect();

        let rows_returned = rows.len() as u64;
        let rows_scanned = json_result
            .get("total_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(rows_returned);

        Ok(ExecuteSqlResponse {
            rows,
            rows_scanned,
            rows_returned,
            execution_time_ms: start.elapsed().as_millis() as u64,
            columns,
            column_types: vec![],
        })
    }
}

fn json_to_sql_value(v: &serde_json::Value) -> SqlValue {
    let inner = match v {
        serde_json::Value::String(s) => sql_value::Value::StringValue(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                sql_value::Value::Int64Value(i)
            } else {
                sql_value::Value::NumberValue(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::Bool(b) => sql_value::Value::BoolValue(*b),
        serde_json::Value::Null => sql_value::Value::NullValue(0),
        other => sql_value::Value::StringValue(other.to_string()),
    };
    SqlValue { value: Some(inner) }
}
