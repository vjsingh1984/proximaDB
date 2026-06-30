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
use proximadb_data_model::ProximaValue;
use proximadb_proto::v1::{
    CollectionOperation, CollectionRequest, CollectionResponse, ExecuteQueryResponse,
    HybridSearchRequest, HybridSearchResponse, SqlRow, SqlRowField, SqlValue, VectorBatchRequest,
    VectorOperationResponse, VectorSearchRequest, sql_value,
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
        let op = CollectionOperation::try_from(request.operation)
            .unwrap_or(CollectionOperation::Unspecified);
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
                let col = self
                    .collection
                    .update_collection(collection_id, config, tenant_id)
                    .await?;
                resp.success = true;
                resp.collection = Some(col);
            }
            CollectionOperation::CollectionGet => {
                let col = self
                    .collection
                    .get_collection(collection_id, tenant_id)
                    .await?;
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
                let deleted = self
                    .collection
                    .delete_collection(collection_id, tenant_id)
                    .await?;
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
            .get_vector(
                collection_id,
                vector_id,
                include_vector,
                include_metadata,
                tenant_id,
            )
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
        _parameters: Option<Vec<ProximaValue>>,
        collection: Option<String>,
        _tenant_id: Option<&str>,
    ) -> Result<ExecuteQueryResponse> {
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
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let column_types: Vec<String> = json_result
            .get("column_types")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
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
                SqlRow {
                    fields,
                    similarity: None,
                }
            })
            .collect();

        let rows_returned = rows.len() as u64;
        let rows_scanned = json_result
            .get("total_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(rows_returned);

        Ok(ExecuteQueryResponse {
            rows,
            rows_scanned,
            rows_returned,
            execution_time_ms: start.elapsed().as_millis() as u64,
            columns,
            column_types,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use proximadb_proto::v1::{Collection, CollectionConfig, VectorRecord};
    use serde_json::json;

    #[derive(Default)]
    struct MockCollectionPort {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CollectionPort for MockCollectionPort {
        async fn get_collection(
            &self,
            identifier: &str,
            tenant_id: Option<&str>,
        ) -> Result<Option<Collection>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("get:{identifier}:{tenant_id:?}"));
            Ok(Some(Collection {
                id: identifier.to_string(),
                ..Collection::default()
            }))
        }

        async fn create_collection(
            &self,
            config: CollectionConfig,
            tenant_id: Option<&str>,
        ) -> Result<Collection> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("create:{}:{tenant_id:?}", config.name));
            Ok(Collection {
                id: config.name.clone(),
                config: Some(config),
                ..Collection::default()
            })
        }

        async fn update_collection(
            &self,
            id: &str,
            config: CollectionConfig,
            tenant_id: Option<&str>,
        ) -> Result<Collection> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("update:{id}:{}:{tenant_id:?}", config.name));
            Ok(Collection {
                id: id.to_string(),
                config: Some(config),
                ..Collection::default()
            })
        }

        async fn delete_collection(&self, id: &str, tenant_id: Option<&str>) -> Result<bool> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("delete:{id}:{tenant_id:?}"));
            Ok(true)
        }

        async fn list_collections(&self, tenant_id: Option<&str>) -> Result<Vec<Collection>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("list:{tenant_id:?}"));
            Ok(vec![Collection {
                id: "docs".to_string(),
                ..Collection::default()
            }])
        }

        async fn resolve_collection_id(&self, identifier: &str) -> Result<Option<String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("resolve:{identifier}"));
            Ok(Some(format!("{identifier}-id")))
        }
    }

    #[derive(Default)]
    struct MockVectorOpsPort {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl VectorOpsPort for MockVectorOpsPort {
        async fn search(
            &self,
            request: VectorSearchRequest,
            tenant_id: Option<&str>,
        ) -> Result<VectorOperationResponse> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("search:{}:{tenant_id:?}", request.collection_id));
            Ok(VectorOperationResponse::default())
        }

        async fn batch_upsert(
            &self,
            request: VectorBatchRequest,
            tenant_id: Option<&str>,
        ) -> Result<VectorOperationResponse> {
            self.calls.lock().unwrap().push(format!(
                "batch:{}:{}:{tenant_id:?}",
                request.collection_id,
                request.vectors.len()
            ));
            Ok(VectorOperationResponse::default())
        }

        async fn get_vector(
            &self,
            collection_id: &str,
            vector_id: &str,
            include_vector: bool,
            include_metadata: bool,
            tenant_id: Option<&str>,
        ) -> Result<VectorOperationResponse> {
            self.calls.lock().unwrap().push(format!(
                "get:{collection_id}:{vector_id}:{include_vector}:{include_metadata}:{tenant_id:?}"
            ));
            Ok(VectorOperationResponse::default())
        }

        async fn flush_all(&self) -> Result<()> {
            Ok(())
        }

        async fn metrics(&self) -> Result<serde_json::Value> {
            Ok(json!({"ok": true}))
        }
    }

    #[derive(Default)]
    struct MockQueryAdapterPort;

    #[async_trait]
    impl QueryAdapterPort for MockQueryAdapterPort {
        async fn vector_search(
            &self,
            _request: VectorSearchRequest,
        ) -> Result<VectorOperationResponse> {
            Ok(VectorOperationResponse::default())
        }

        async fn execute_hybrid(
            &self,
            _request: HybridSearchRequest,
        ) -> Result<HybridSearchResponse> {
            Ok(HybridSearchResponse::default())
        }

        async fn execute_sql(
            &self,
            _query: String,
            _collection: Option<String>,
        ) -> Result<serde_json::Value> {
            Ok(json!({
                "columns": ["id", "score", "flag", "none", "obj"],
                "total_count": 9,
                "records": [{
                    "id": "r1",
                    "score": 1.5,
                    "flag": true,
                    "none": null,
                    "obj": {"nested": 1}
                }]
            }))
        }
    }

    fn make_handlers(
        collection: Arc<MockCollectionPort>,
        vector_ops: Arc<MockVectorOpsPort>,
        query_adapter: Option<Arc<dyn QueryAdapterPort>>,
    ) -> UnifiedHandlers {
        UnifiedHandlers::new(collection, vector_ops, query_adapter)
    }

    fn collection_request(operation: CollectionOperation) -> CollectionRequest {
        CollectionRequest {
            operation: operation as i32,
            collection_id: Some("docs".to_string()),
            collection_config: Some(CollectionConfig {
                name: "docs".to_string(),
                ..CollectionConfig::default()
            }),
            ..CollectionRequest::default()
        }
    }

    #[test]
    fn request_ids_are_hex_length_stable_and_collection_id_cache_handles_ttl_and_eviction() {
        let first = generate_request_id();
        let second = generate_request_id();
        assert_eq!(first.len(), 16);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);

        let cache = CollectionIdCache::new();
        cache.put("docs".to_string(), "docs-id".to_string());
        assert_eq!(cache.get("docs").as_deref(), Some("docs-id"));
        assert_eq!(CollectionIdCache::default().get("missing"), None);

        let expiring = CollectionIdCache {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_nanos(0),
            max_size: 1,
        };
        expiring.put("old".to_string(), "old-id".to_string());
        assert_eq!(expiring.get("old"), None);

        let bounded = CollectionIdCache {
            cache: std::sync::RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(60),
            max_size: 1,
        };
        bounded.put("a".to_string(), "a-id".to_string());
        bounded.put("b".to_string(), "b-id".to_string());
        assert_eq!(bounded.get("a"), None);
        assert_eq!(bounded.get("b").as_deref(), Some("b-id"));
    }

    #[tokio::test]
    async fn unified_handlers_route_collection_operations_to_collection_port() {
        let collection = Arc::new(MockCollectionPort::default());
        let vector_ops = Arc::new(MockVectorOpsPort::default());
        let handlers = make_handlers(collection.clone(), vector_ops, None);

        for operation in [
            CollectionOperation::CollectionCreate,
            CollectionOperation::CollectionUpdate,
            CollectionOperation::CollectionGet,
            CollectionOperation::CollectionList,
            CollectionOperation::CollectionDelete,
            CollectionOperation::CollectionGetIdByName,
        ] {
            let response = handlers
                .handle_collection_operation_for_tenant(
                    collection_request(operation),
                    Some("tenant-a"),
                )
                .await
                .unwrap();
            assert!(response.success);
            assert!(response.processing_time_us >= 0);
        }

        let calls = collection.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.starts_with("create:docs")));
        assert!(calls.iter().any(|call| call.starts_with("update:docs")));
        assert!(calls.iter().any(|call| call.starts_with("get:docs")));
        assert!(calls.iter().any(|call| call.starts_with("list:")));
        assert!(calls.iter().any(|call| call.starts_with("delete:docs")));
        assert!(calls.iter().any(|call| call == "resolve:docs"));

        let mut missing_config = collection_request(CollectionOperation::CollectionCreate);
        missing_config.collection_config = None;
        assert!(
            handlers
                .handle_collection_operation_for_tenant(missing_config, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("collection_config required")
        );
        assert!(
            handlers
                .handle_collection_operation_for_tenant(
                    collection_request(CollectionOperation::Unspecified),
                    None,
                )
                .await
                .unwrap_err()
                .to_string()
                .contains("not implemented")
        );
    }

    #[tokio::test]
    async fn unified_handlers_route_vector_hybrid_and_sql_operations() {
        let collection = Arc::new(MockCollectionPort::default());
        let vector_ops = Arc::new(MockVectorOpsPort::default());
        let handlers = make_handlers(
            collection,
            vector_ops.clone(),
            Some(Arc::new(MockQueryAdapterPort)),
        );

        handlers
            .handle_vector_search_v1(VectorSearchRequest {
                collection_id: "global".to_string(),
                ..VectorSearchRequest::default()
            })
            .await
            .unwrap();
        handlers
            .handle_vector_search_v1_for_tenant(
                VectorSearchRequest {
                    collection_id: "tenant".to_string(),
                    ..VectorSearchRequest::default()
                },
                Some("tenant-a"),
            )
            .await
            .unwrap();
        handlers
            .handle_vector_batch_v1_for_tenant(
                VectorBatchRequest {
                    collection_id: "docs".to_string(),
                    vectors: vec![VectorRecord {
                        id: "v1".to_string(),
                        vector: vec![0.1],
                        ..VectorRecord::default()
                    }],
                },
                Some("tenant-a"),
            )
            .await
            .unwrap();
        handlers
            .handle_vector_v1_for_tenant("docs", "v1", true, false, Some("tenant-a"))
            .await
            .unwrap();

        assert_eq!(vector_ops.calls.lock().unwrap().len(), 4);
        assert!(
            handlers
                .execute_hybrid_query(HybridSearchRequest::default())
                .await
                .is_ok()
        );

        let sql = handlers
            .execute_sql_v1(
                "select * from docs".to_string(),
                None,
                Some("docs".to_string()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(sql.columns, vec!["id", "score", "flag", "none", "obj"]);
        assert_eq!(sql.rows_scanned, 9);
        assert_eq!(sql.rows_returned, 1);
        let fields = &sql.rows[0].fields;
        assert!(matches!(
            fields.iter().find(|field| field.key == "id").and_then(|field| field.value.as_ref()).and_then(|value| value.value.as_ref()),
            Some(sql_value::Value::StringValue(value)) if value == "r1"
        ));
        assert!(matches!(
            fields.iter().find(|field| field.key == "score").and_then(|field| field.value.as_ref()).and_then(|value| value.value.as_ref()),
            Some(sql_value::Value::NumberValue(value)) if (*value - 1.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            fields
                .iter()
                .find(|field| field.key == "flag")
                .and_then(|field| field.value.as_ref())
                .and_then(|value| value.value.as_ref()),
            Some(sql_value::Value::BoolValue(true))
        ));
        assert!(matches!(
            fields
                .iter()
                .find(|field| field.key == "none")
                .and_then(|field| field.value.as_ref())
                .and_then(|value| value.value.as_ref()),
            Some(sql_value::Value::NullValue(_))
        ));
        assert!(matches!(
            fields.iter().find(|field| field.key == "obj").and_then(|field| field.value.as_ref()).and_then(|value| value.value.as_ref()),
            Some(sql_value::Value::StringValue(value)) if value.contains("nested")
        ));
    }

    #[tokio::test]
    async fn unified_handlers_report_missing_query_adapter_explicitly_and_sql_arrays_lower() {
        let handlers = make_handlers(
            Arc::new(MockCollectionPort::default()),
            Arc::new(MockVectorOpsPort::default()),
            None,
        );
        assert!(
            handlers
                .execute_hybrid_query(HybridSearchRequest::default())
                .await
                .unwrap_err()
                .to_string()
                .contains("QueryAdapterPort")
        );
        assert!(
            handlers
                .execute_sql_v1("select 1".to_string(), None, None, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("QueryAdapterPort")
        );

        struct ArrayQueryAdapter;

        #[async_trait]
        impl QueryAdapterPort for ArrayQueryAdapter {
            async fn vector_search(
                &self,
                _request: VectorSearchRequest,
            ) -> Result<VectorOperationResponse> {
                Ok(VectorOperationResponse::default())
            }

            async fn execute_hybrid(
                &self,
                _request: HybridSearchRequest,
            ) -> Result<HybridSearchResponse> {
                Ok(HybridSearchResponse::default())
            }

            async fn execute_sql(
                &self,
                _query: String,
                _collection: Option<String>,
            ) -> Result<serde_json::Value> {
                Ok(json!(["text", 7, false, null, {"shape": "object"}]))
            }
        }

        let handlers = make_handlers(
            Arc::new(MockCollectionPort::default()),
            Arc::new(MockVectorOpsPort::default()),
            Some(Arc::new(ArrayQueryAdapter)),
        );
        let sql = handlers
            .execute_sql_v1("select values".to_string(), None, None, None)
            .await
            .unwrap();
        assert_eq!(sql.rows_returned, 5);
        assert!(sql.rows[..4].iter().all(|row| row.fields[0].key == "value"));
        assert_eq!(sql.rows[4].fields[0].key, "shape");
    }
}
