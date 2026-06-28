// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! In-process `ReferenceBackend` (ADR-037 Decision 5).
//!
//! Projects the reference MCP tools onto the engine's existing services by
//! calling them **directly** (no REST/gRPC self-hop): catalog list/describe via
//! [`ApiHandlersPort`], the statistics envelope via the resident
//! `statistics_registry`, and explain/search via the [`UnifiedQueryPort`] (which
//! already returns `serde_json::Value`). The agent-state tools
//! (`memory`/`checkpoint`/`event`) are deferred to Stage 2 (threading
//! `AgenticGrpcBackend` into `AppState`) — they return an explicit "not yet
//! wired in-process" rather than fabricate.

use crate::core::statistics::{StatisticsSummary, statistics_registry};
use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};
use async_trait::async_trait;
use proximadb_mcp::{BackendError, ReferenceBackend};
use proximadb_runtime::{ApiHandlersPort, UnifiedQueryPort};
use serde_json::{Value, json};
use std::sync::Arc;

/// A `ReferenceBackend` that calls the engine's in-process services directly.
pub struct EngineBackend {
    api_handlers: Arc<dyn ApiHandlersPort>,
    query: Option<Arc<dyn UnifiedQueryPort>>,
    /// Tenant the MCP surface operates as. Stage 1 uses a single configured
    /// tenant (or the port default when `None`); per-request tenant scoping from
    /// MCP auth is a later refinement.
    tenant: Option<String>,
}

impl EngineBackend {
    pub fn new(
        api_handlers: Arc<dyn ApiHandlersPort>,
        query: Option<Arc<dyn UnifiedQueryPort>>,
        tenant: Option<String>,
    ) -> Self {
        Self {
            api_handlers,
            query,
            tenant,
        }
    }

    fn tenant(&self) -> Option<&str> {
        self.tenant.as_deref()
    }

    fn collection_request(
        op: CollectionOperation,
        collection_id: Option<String>,
    ) -> CollectionRequest {
        CollectionRequest {
            operation: op as i32,
            collection_id,
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        }
    }
}

fn require_str(args: &Value, key: &str) -> Result<String, BackendError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| BackendError::not_found(format!("missing required string arg `{key}`")))
}

/// The `query` arg as a query string (a string is used verbatim; any other
/// non-null value is serialized — the engine routes/parses it).
fn query_string(args: &Value) -> Result<String, BackendError> {
    match args.get("query") {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) if !v.is_null() => Ok(v.to_string()),
        _ => Err(BackendError::not_found(
            "missing required `query` (a query string)".to_string(),
        )),
    }
}

fn not_wired(tool: &str) -> BackendError {
    BackendError::internal(format!(
        "`{tool}` (ADR-022 agent state) is not yet wired in-process; Stage 2 threads \
         AgenticGrpcBackend into AppState to enable it"
    ))
}

#[async_trait]
impl ReferenceBackend for EngineBackend {
    async fn list_collections(&self, _args: &Value) -> Result<Value, BackendError> {
        let req = Self::collection_request(CollectionOperation::CollectionList, None);
        let resp = self
            .api_handlers
            .handle_collection_operation_for_tenant(req, self.tenant())
            .await
            .map_err(|e| BackendError::internal(format!("list collections failed: {e}")))?;
        let collections: Vec<Value> = resp
            .collections
            .into_iter()
            .map(|c| {
                let cfg = c.config.unwrap_or_default();
                let stats = c.stats.unwrap_or_default();
                let id = if c.id.is_empty() { cfg.name.clone() } else { c.id };
                json!({ "collection_id": id, "name": cfg.name, "record_count": stats.vector_count.max(0) })
            })
            .collect();
        Ok(json!({ "collections": collections }))
    }

    async fn describe(&self, args: &Value) -> Result<Value, BackendError> {
        let id = require_str(args, "collection_id")?;
        let req = Self::collection_request(CollectionOperation::CollectionGet, Some(id.clone()));
        let resp = self
            .api_handlers
            .handle_collection_operation_for_tenant(req, self.tenant())
            .await
            .map_err(|e| BackendError::internal(format!("describe failed: {e}")))?;
        let c = resp
            .collection
            .ok_or_else(|| BackendError::not_found(format!("collection `{id}` not found")))?;
        let cfg = c.config.unwrap_or_default();
        let stats = c.stats.unwrap_or_default();
        Ok(json!({
            "collection_id": if c.id.is_empty() { id } else { c.id },
            "name": cfg.name,
            "dimension": cfg.dimension,
            "record_count": stats.vector_count.max(0),
            "storage_size_bytes": stats.data_size_bytes.max(0),
            "index_size_bytes": stats.index_size_bytes.max(0),
        }))
    }

    async fn stats(&self, args: &Value) -> Result<Value, BackendError> {
        let id = require_str(args, "collection_id")?;
        // Resident envelope from the flush/compaction write boundary; an empty
        // (epoch-watermark) envelope when no snapshot has populated one yet.
        let envelope = statistics_registry()
            .envelope(&id)
            .unwrap_or_else(|| StatisticsSummary::new(&id).to_envelope());
        serde_json::to_value(&envelope)
            .map_err(|e| BackendError::internal(format!("serialize envelope failed: {e}")))
    }

    async fn explain(&self, args: &Value) -> Result<Value, BackendError> {
        let id = require_str(args, "collection_id")?;
        let q = query_string(args)?;
        let port = self
            .query
            .as_ref()
            .ok_or_else(|| BackendError::internal("query facade not available".to_string()))?;
        port.explain_unified_query(q, Some(id))
            .await
            .map_err(|e| BackendError::internal(format!("explain failed: {e}")))
    }

    async fn search(&self, args: &Value) -> Result<Value, BackendError> {
        let id = require_str(args, "collection_id")?;
        let q = query_string(args)?;
        let limit = args.get("k").and_then(Value::as_u64).map(|k| k as u32);
        let port = self
            .query
            .as_ref()
            .ok_or_else(|| BackendError::internal("query facade not available".to_string()))?;
        port.execute_unified_query(q, None, Some(id), limit)
            .await
            .map_err(|e| BackendError::internal(format!("search failed: {e}")))
    }

    async fn memory(&self, _args: &Value) -> Result<Value, BackendError> {
        Err(not_wired("memory"))
    }
    async fn checkpoint(&self, _args: &Value) -> Result<Value, BackendError> {
        Err(not_wired("checkpoint"))
    }
    async fn event(&self, _args: &Value) -> Result<Value, BackendError> {
        Err(not_wired("event"))
    }
}
