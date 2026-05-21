/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! `POST /api/v3/collections/{collection_id}/documents`
//!
//! Text-only ingest endpoint. Accepts records without vector fields; the
//! server populates them via the in-process EmbeddingService singleton
//! (see `proximadb_embedding::EmbeddingService::global`).
//!
//! Reuses [`ProximaFlightService::embed_text_only_records`] so the REST and
//! Arrow Flight paths share one dispatch implementation.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::api_handlers::RichRecordBatchRequest;
use crate::errors::{ApiError, ApiResult};
use crate::network::arrow_ipc::ProximaFlightService;
use crate::network::rest::v1::handlers::AppState;
use crate::services::{
    WriteDurabilityRequirement, WriteIntent, WriteLaneRouter, WriteOperationKind,
};

// ── Request / response DTOs ────────────────────────────────────────────────

/// A single record submitted for native embedding + indexing.
#[derive(Debug, Deserialize)]
pub struct IngestDocument {
    pub id: String,
    /// Raw text content. Required when `X-Embed-Source: native` (default);
    /// optional when the client also supplied a vector.
    #[serde(default)]
    pub text: Option<String>,
    /// Optional client-provided vector. When present, the server skips
    /// embedding for this record. Use case: SDK that already embedded
    /// locally (legacy path).
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    /// Arbitrary metadata fields. Stored as ProximaRecord props.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct IngestDocumentsRequest {
    pub records: Vec<IngestDocument>,
}

#[derive(Debug, Serialize)]
pub struct IngestedRecord {
    pub id: String,
    pub dim: u32,
}

#[derive(Debug, Serialize)]
pub struct IngestDocumentsResponse {
    pub mode: String,
    pub records: Vec<IngestedRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

// ── Handler ────────────────────────────────────────────────────────────────

pub async fn ingest_documents(
    Path(collection): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<IngestDocumentsRequest>,
) -> Response {
    match ingest_documents_inner(collection, state, headers, request).await {
        Ok((status, response)) => (status, Json(response)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn ingest_documents_inner(
    collection: String,
    state: AppState,
    headers: HeaderMap,
    request: IngestDocumentsRequest,
) -> ApiResult<(StatusCode, IngestDocumentsResponse)> {
    if collection.is_empty() {
        return Err(ApiError::InvalidArgument(
            "Collection name is required".to_string(),
        ));
    }
    if request.records.is_empty() {
        return Err(ApiError::InvalidArgument(
            "At least one record is required".to_string(),
        ));
    }

    let tenant_id = headers
        .get("X-Tenant-ID")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);

    let mode = headers
        .get("X-Ingest-Mode")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("sync")
        .to_lowercase();
    if mode != "sync" && mode != "async" {
        return Err(ApiError::InvalidArgument(format!(
            "X-Ingest-Mode must be 'sync' or 'async', got {mode:?}"
        )));
    }

    let embed_source = headers
        .get("X-Embed-Source")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("native")
        .to_lowercase();
    if embed_source != "native" && embed_source != "sdk-vector" {
        return Err(ApiError::InvalidArgument(format!(
            "X-Embed-Source must be 'native' or 'sdk-vector', got {embed_source:?}"
        )));
    }

    info!(
        collection = %collection,
        tenant = ?tenant_id,
        mode = %mode,
        embed_source = %embed_source,
        records = request.records.len(),
        "v3 documents: ingesting"
    );

    // Build ProximaRecords. Reserved column names (text, metadata, etc.) get
    // hoisted into props so the codec-shared embed_text_only_records dispatch
    // can read them out the same way the Arrow Flight path does.
    let mut records: Vec<ProximaRecord> = Vec::with_capacity(request.records.len());
    for doc in request.records {
        let now_ns = chrono::Utc::now()
            .timestamp_millis()
            .saturating_mul(1_000_000);

        let mut props = HashMap::new();
        if let Some(text) = doc.text.as_ref() {
            props.insert(
                "text".to_string(),
                ProximaTreeNode::Value(ProximaValue::String(text.clone())),
            );
        }
        for (k, v) in doc.metadata.into_iter() {
            props.insert(k, ProximaTreeNode::Value(json_to_proxima_value(v)));
        }

        let embeddings = match doc.vector {
            Some(values) if !values.is_empty() => vec![EmbeddingCell {
                model_id: "sdk-provided".to_string(),
                modality: "dense_vector".to_string(),
                dim: values.len() as u32,
                values,
            }],
            _ => vec![],
        };

        records.push(ProximaRecord {
            oid: doc.id.clone(),
            local_id: Some(doc.id),
            tenant_id: tenant_id.clone().unwrap_or_default(),
            created_at_ns: now_ns,
            updated_at_ns: now_ns,
            origin: Some("v3_documents".to_string()),
            props,
            embeddings,
            ..ProximaRecord::default()
        });
    }

    // Dispatch text-only records through the shared embedding helper. Records
    // that already carry an SDK-provided vector skip the dispatch entirely.
    if embed_source == "native" {
        ProximaFlightService::embed_text_only_records(&mut records, tenant_id.as_deref())
            .await
            .map_err(|e| {
                warn!(error = %e, "v3 documents: embedding dispatch failed");
                ApiError::Internal(format!("embedding dispatch failed: {e}"))
            })?;
    }

    // Reject any records still missing an embedding (caller asked for
    // sdk-vector but didn't provide one, or the dispatcher couldn't extract
    // text). Better to fail loudly than to silently drop to defaults.
    let missing: Vec<&str> = records
        .iter()
        .filter(|r| r.embeddings.is_empty())
        .map(|r| r.oid.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::InvalidArgument(format!(
            "{} record(s) have no vector and no text to embed: {}",
            missing.len(),
            missing.join(", ")
        )));
    }

    let inserted_ids: Vec<IngestedRecord> = records
        .iter()
        .map(|r| IngestedRecord {
            id: r.oid.clone(),
            dim: r.embeddings.first().map(|e| e.dim).unwrap_or(0),
        })
        .collect();
    let row_count = records.len() as u64;

    // Async ack path: in Phase 1 the EmbeddingService dispatch is in-process
    // and synchronous from the request handler's perspective, so we don't yet
    // get a true sub-10ms ack from `mode=async`. Still set the response code
    // to 202 so clients build against the right contract; the latency
    // optimization (WAL pending_embed flag + background drain) lands when
    // the drainer task is registered in BackgroundMaintenanceManager.
    let intent = WriteIntent::new(&collection, WriteOperationKind::Insert)
        .with_durability(WriteDurabilityRequirement::WalRequired)
        .with_row_count_hint(row_count);
    let lane = WriteLaneRouter::new().route(&intent);
    debug!(
        collection_id = %collection,
        write_lane = ?lane.lane,
        guards = ?lane.required_guards,
        "v3 documents write-lane decision"
    );

    let batch_request = RichRecordBatchRequest {
        collection_id: collection.clone(),
        records,
    };

    match state
        .request_handlers
        .handle_record_batch_for_tenant(batch_request, tenant_id.as_deref())
        .await
    {
        Ok(_resp) => {
            let status = if mode == "async" {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            let response = IngestDocumentsResponse {
                mode,
                records: inserted_ids,
                retry_after_ms: None,
            };
            Ok((status, response))
        }
        Err(e) => Err(ApiError::Internal(format!("Insert failed: {e}"))),
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn json_to_proxima_value(value: serde_json::Value) -> ProximaValue {
    match value {
        // ProximaValue has no Null variant — represent JSON null as empty
        // string so the metadata key survives without crashing the type
        // system. Callers can skip null fields earlier if they prefer.
        serde_json::Value::Null => ProximaValue::String(String::new()),
        serde_json::Value::Bool(b) => ProximaValue::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ProximaValue::Int64(i)
            } else if let Some(f) = n.as_f64() {
                ProximaValue::Float64(f)
            } else {
                ProximaValue::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => ProximaValue::String(s),
        // Arrays and objects get preserved as full JSON so downstream
        // query/filter layers retain structural fidelity.
        v @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)) => ProximaValue::Json(v),
    }
}
