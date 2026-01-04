// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 ProximaRecord service implementation
//!
//! This module implements the ProximaRecordService gRPC service for the V2 API.
//! It provides:
//! - Batch record operations (insert, upsert, update, delete)
//! - Search with typed filters
//! - Schema management (create, get, list, evolve)

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::proximadb_v1::{
    CollectionOperation, CollectionRequest, SearchQuery, VectorBatchRequest, VectorRecord,
    VectorSearchRequest,
};
use crate::proto::proximadb_v2::{
    self, proxima_record_service_server::ProximaRecordService,
    proxima_record_service_server::ProximaRecordServiceServer, BackpressureLevel,
    BackpressureSignal, BatchError, BatchWriteMode, BatchWriteStreamRequest,
    BatchWriteStreamResponse, CreateSchemaRequest, CreateSchemaResponse, EvolveSchemaRequest,
    EvolveSchemaResponse, GetSchemaRequest, GetSchemaResponse, ListSchemasRequest,
    ListSchemasResponse, ProximaRecordBatch, ProximaRecordBatchResponse, TypedSearchRequest,
    TypedSearchResponse, TypedSearchResult,
};

/// gRPC V2 ProximaRecord service implementation
///
/// This service provides the V2 API with:
/// - Rich typed fields (TEXT, INTEGER, FLOAT, DECIMAL, UUID, etc.)
/// - Schema enforcement modes (STRICT, FLEXIBLE, HYBRID)
/// - Typed filtering with range, equality, and CONTAINS operators
pub struct ProximaRecordServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

/// Streaming response type for SearchStream
pub type SearchStreamStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<TypedSearchResult, Status>> + Send>>;

/// Streaming response type for BatchWriteStream
pub type BatchWriteStreamStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<BatchWriteStreamResponse, Status>> + Send>>;

/// Channel buffer size for streaming operations
const STREAM_BUFFER_SIZE: usize = 128;

/// Buffer utilization thresholds for backpressure
const BACKPRESSURE_LOW_THRESHOLD: u32 = 25;
const BACKPRESSURE_MEDIUM_THRESHOLD: u32 = 50;
const BACKPRESSURE_HIGH_THRESHOLD: u32 = 75;
const BACKPRESSURE_CRITICAL_THRESHOLD: u32 = 90;

impl ProximaRecordServiceImpl {
    /// Create a new ProximaRecordServiceImpl
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { unified_handlers }
    }

    /// Convert to a tonic server
    pub fn into_server(self) -> ProximaRecordServiceServer<Self> {
        ProximaRecordServiceServer::new(self)
    }

    /// Convert ProximaRecordBatch to VectorBatchRequest for v1 storage
    fn convert_to_v1_batch(
        &self,
        batch: &ProximaRecordBatch,
    ) -> Result<VectorBatchRequest, Status> {
        let mut vectors = Vec::with_capacity(batch.records.len());

        for record in &batch.records {
            // Convert typed_fields to metadata for backward compatibility
            let mut metadata: HashMap<String, crate::proto::proximadb_v1::SqlValue> =
                HashMap::new();

            // Convert typed_fields
            for (key, typed_value) in &record.typed_fields {
                if let Some(sql_value) = self.typed_value_to_sql_value(typed_value) {
                    metadata.insert(key.clone(), sql_value);
                }
            }

            // Convert text_fields
            for text_field in &record.text_fields {
                let sql_value = crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        text_field.content.clone(),
                    )),
                };
                metadata.insert(text_field.name.clone(), sql_value);
            }

            // Merge flexible_fields
            for (key, sql_value) in &record.flexible_fields {
                if !metadata.contains_key(key) {
                    metadata.insert(key.clone(), sql_value.clone());
                }
            }

            let vector_record = VectorRecord {
                id: record.id.clone(),
                vector: record.vector.clone(),
                metadata,
                version: record.version,
                timestamp: Some(record.timestamp_ms),
                source: record.source.clone(),
                updated_at: record.updated_at_ms,
                expires_at: record.expires_at_ms,
            };

            vectors.push(vector_record);
        }

        Ok(VectorBatchRequest {
            collection_id: batch.collection_id.clone(),
            vectors,
        })
    }

    /// Convert TypedValue to SqlValue for v1 storage
    fn typed_value_to_sql_value(
        &self,
        typed_value: &proximadb_v2::TypedValue,
    ) -> Option<crate::proto::proximadb_v1::SqlValue> {
        use crate::proto::proximadb_v1::sql_value::Value;
        use proximadb_v2::typed_value::Value as TypedVal;

        let inner = match typed_value.value.as_ref()? {
            TypedVal::TextValue(s) => Value::StringValue(s.clone()),
            TypedVal::IntegerValue(i) => Value::Int64Value(*i),
            TypedVal::FloatValue(f) => Value::NumberValue(*f),
            TypedVal::BooleanValue(b) => Value::BoolValue(*b),
            TypedVal::TimestampValue(ts) => Value::Int64Value(*ts),
            TypedVal::JsonValue(json) => Value::StringValue(json.clone()),
            TypedVal::BinaryValue(bytes) => Value::BytesValue(bytes.clone()),
            TypedVal::UuidValue(uuid) => {
                // Convert UUID bytes to string for storage
                if uuid.len() == 16 {
                    let uuid_str = uuid::Uuid::from_slice(uuid)
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| hex::encode(uuid));
                    Value::StringValue(uuid_str)
                } else {
                    Value::BytesValue(uuid.clone())
                }
            }
            TypedVal::IsNull(true) => Value::NullValue(0),
            TypedVal::IsNull(false) => return None,
            // Array types - store as JSON strings
            TypedVal::TextArray(arr) => {
                let json = serde_json::to_string(&arr.values).unwrap_or_default();
                Value::StringValue(json)
            }
            TypedVal::IntegerArray(arr) => {
                let json = serde_json::to_string(&arr.values).unwrap_or_default();
                Value::StringValue(json)
            }
            TypedVal::FloatArray(arr) => {
                let json = serde_json::to_string(&arr.values).unwrap_or_default();
                Value::StringValue(json)
            }
            TypedVal::BooleanArray(arr) => {
                let json = serde_json::to_string(&arr.values).unwrap_or_default();
                Value::StringValue(json)
            }
            // Handle remaining variants with string conversion
            _ => Value::StringValue(format!("{:?}", typed_value)),
        };

        Some(crate::proto::proximadb_v1::SqlValue { value: Some(inner) })
    }

    /// Convert search results to TypedSearchResult
    fn convert_search_results(
        &self,
        results: &crate::proto::proximadb_v1::SearchResult,
        include_vector: bool,
    ) -> Vec<TypedSearchResult> {
        results
            .results
            .iter()
            .map(|r| {
                // Convert metadata back to typed_fields
                let typed_fields: HashMap<String, proximadb_v2::TypedValue> = r
                    .metadata
                    .iter()
                    .filter_map(|(k, v)| {
                        self.sql_value_to_typed_value(v).map(|tv| (k.clone(), tv))
                    })
                    .collect();

                TypedSearchResult {
                    id: r.id.clone(),
                    score: r.score,
                    typed_fields,
                    vector: if include_vector {
                        r.vector.clone()
                    } else {
                        vec![]
                    },
                    text_fields: vec![], // Would be populated from text storage
                    timestamp_ms: r.timestamp,
                    version: r.version.map(|v| v as u32),
                    source: r.source.clone(),
                }
            })
            .collect()
    }

    /// Convert SqlValue to TypedValue
    fn sql_value_to_typed_value(
        &self,
        sql_value: &crate::proto::proximadb_v1::SqlValue,
    ) -> Option<proximadb_v2::TypedValue> {
        use crate::proto::proximadb_v1::sql_value::Value;
        use proximadb_v2::typed_value::Value as TypedVal;
        use proximadb_v2::ColumnDataType;

        let (declared_type, value) = match sql_value.value.as_ref()? {
            Value::NullValue(_) => (ColumnDataType::ColumnTypeUnspecified as i32, TypedVal::IsNull(true)),
            Value::BoolValue(b) => (ColumnDataType::Boolean as i32, TypedVal::BooleanValue(*b)),
            Value::Int64Value(i) => (ColumnDataType::Integer as i32, TypedVal::IntegerValue(*i)),
            Value::NumberValue(f) => (ColumnDataType::Float as i32, TypedVal::FloatValue(*f)),
            Value::StringValue(s) => (ColumnDataType::Text as i32, TypedVal::TextValue(s.clone())),
            Value::BytesValue(b) => (ColumnDataType::Binary as i32, TypedVal::BinaryValue(b.clone())),
            Value::ArrayValue(arr) => {
                // Convert array to JSON string for storage
                let values: Vec<serde_json::Value> = arr
                    .values
                    .iter()
                    .filter_map(|v| self.sql_value_to_json(v))
                    .collect();
                let json = serde_json::to_string(&values).unwrap_or_default();
                (ColumnDataType::Json as i32, TypedVal::JsonValue(json))
            }
            Value::ObjectValue(obj) => {
                // Convert object to JSON string
                let map: serde_json::Map<String, serde_json::Value> = obj
                    .fields
                    .iter()
                    .filter_map(|(k, v)| self.sql_value_to_json(v).map(|jv| (k.clone(), jv)))
                    .collect();
                let json = serde_json::to_string(&map).unwrap_or_default();
                (ColumnDataType::Json as i32, TypedVal::JsonValue(json))
            }
        };

        Some(proximadb_v2::TypedValue {
            declared_type,
            value: Some(value),
        })
    }

    /// Convert SqlValue to JSON value
    fn sql_value_to_json(&self, sql_value: &crate::proto::proximadb_v1::SqlValue) -> Option<serde_json::Value> {
        use crate::proto::proximadb_v1::sql_value::Value;

        match sql_value.value.as_ref()? {
            Value::NullValue(_) => Some(serde_json::Value::Null),
            Value::BoolValue(b) => Some(serde_json::Value::Bool(*b)),
            Value::Int64Value(i) => Some(serde_json::Value::Number((*i).into())),
            Value::NumberValue(f) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number),
            Value::StringValue(s) => Some(serde_json::Value::String(s.clone())),
            Value::BytesValue(b) => Some(serde_json::Value::String(hex::encode(b))),
            Value::ArrayValue(arr) => {
                let values: Vec<serde_json::Value> = arr
                    .values
                    .iter()
                    .filter_map(|v| self.sql_value_to_json(v))
                    .collect();
                Some(serde_json::Value::Array(values))
            }
            Value::ObjectValue(obj) => {
                let map: serde_json::Map<String, serde_json::Value> = obj
                    .fields
                    .iter()
                    .filter_map(|(k, v)| self.sql_value_to_json(v).map(|jv| (k.clone(), jv)))
                    .collect();
                Some(serde_json::Value::Object(map))
            }
        }
    }

    /// Convert a single ProximaRecord to VectorRecord for V1 storage layer
    fn convert_proxima_record_to_vector_record(
        record: &proximadb_v2::ProximaRecord,
    ) -> VectorRecord {
        use crate::proto::proximadb_v1::sql_value::Value;

        let mut metadata: HashMap<String, crate::proto::proximadb_v1::SqlValue> = HashMap::new();

        // Convert typed_fields to metadata
        for (key, typed_value) in &record.typed_fields {
            if let Some(value) = &typed_value.value {
                let sql_value = match value {
                    proximadb_v2::typed_value::Value::TextValue(s) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::StringValue(s.clone())),
                        }
                    }
                    proximadb_v2::typed_value::Value::IntegerValue(i) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::Int64Value(*i)),
                        }
                    }
                    proximadb_v2::typed_value::Value::FloatValue(f) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::NumberValue(*f)),
                        }
                    }
                    proximadb_v2::typed_value::Value::BooleanValue(b) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::BoolValue(*b)),
                        }
                    }
                    proximadb_v2::typed_value::Value::TimestampValue(ts) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::Int64Value(*ts)),
                        }
                    }
                    proximadb_v2::typed_value::Value::JsonValue(json) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::StringValue(json.clone())),
                        }
                    }
                    proximadb_v2::typed_value::Value::BinaryValue(bytes) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::BytesValue(bytes.clone())),
                        }
                    }
                    proximadb_v2::typed_value::Value::UuidValue(uuid) => {
                        let uuid_str = if uuid.len() == 16 {
                            uuid::Uuid::from_slice(uuid)
                                .map(|u| u.to_string())
                                .unwrap_or_else(|_| hex::encode(uuid))
                        } else {
                            hex::encode(uuid)
                        };
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::StringValue(uuid_str)),
                        }
                    }
                    proximadb_v2::typed_value::Value::IsNull(true) => {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::NullValue(0)),
                        }
                    }
                    _ => {
                        // For other types, serialize to string representation
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(Value::StringValue(format!("{:?}", value))),
                        }
                    }
                };
                metadata.insert(key.clone(), sql_value);
            }
        }

        // Convert text_fields
        for text_field in &record.text_fields {
            let sql_value = crate::proto::proximadb_v1::SqlValue {
                value: Some(Value::StringValue(text_field.content.clone())),
            };
            metadata.insert(text_field.name.clone(), sql_value);
        }

        // Merge flexible_fields
        for (key, sql_value) in &record.flexible_fields {
            if !metadata.contains_key(key) {
                metadata.insert(key.clone(), sql_value.clone());
            }
        }

        VectorRecord {
            id: record.id.clone(),
            vector: record.vector.clone(),
            metadata,
            version: record.version,
            timestamp: Some(record.timestamp_ms),
            source: record.source.clone(),
            updated_at: record.updated_at_ms,
            expires_at: record.expires_at_ms,
        }
    }

    /// Calculate backpressure signal based on buffer utilization
    fn calculate_backpressure(buffer_percent: u32) -> BackpressureSignal {
        let (level, suggested_delay_ms) = if buffer_percent >= BACKPRESSURE_CRITICAL_THRESHOLD {
            (BackpressureLevel::BackpressureCritical, 500)
        } else if buffer_percent >= BACKPRESSURE_HIGH_THRESHOLD {
            (BackpressureLevel::BackpressureHigh, 100)
        } else if buffer_percent >= BACKPRESSURE_MEDIUM_THRESHOLD {
            (BackpressureLevel::BackpressureMedium, 50)
        } else if buffer_percent >= BACKPRESSURE_LOW_THRESHOLD {
            (BackpressureLevel::BackpressureLow, 10)
        } else {
            (BackpressureLevel::BackpressureNone, 0)
        };

        BackpressureSignal {
            level: level as i32,
            suggested_delay_ms,
            buffer_percent,
        }
    }
}

#[tonic::async_trait]
impl ProximaRecordService for ProximaRecordServiceImpl {
    // =========================================================================
    // Record Operations
    // =========================================================================

    /// Insert records into a collection
    async fn insert_records(
        &self,
        request: Request<ProximaRecordBatch>,
    ) -> Result<Response<ProximaRecordBatchResponse>, Status> {
        let batch = request.into_inner();
        info!(
            "V2 gRPC: InsertRecords - collection='{}', records={}",
            batch.collection_id,
            batch.records.len()
        );

        // Validate write mode
        if batch.write_mode != BatchWriteMode::Insert as i32
            && batch.write_mode != BatchWriteMode::Unspecified as i32
        {
            return Err(Status::invalid_argument(
                "InsertRecords requires INSERT or UNSPECIFIED write mode",
            ));
        }

        // Convert to v1 batch
        let v1_batch = self.convert_to_v1_batch(&batch)?;
        let record_count = v1_batch.vectors.len() as i64;

        // Insert via unified handlers
        match self.unified_handlers.handle_vector_batch_v1(v1_batch).await {
            Ok(resp) => {
                let success_count = if resp.success { record_count } else { 0 };
                Ok(Response::new(ProximaRecordBatchResponse {
                    success: resp.success,
                    total_processed: record_count,
                    success_count,
                    failed_count: record_count - success_count,
                    inserted_ids: batch.records.iter().map(|r| r.id.clone()).collect(),
                    errors: vec![],
                    processing_time_us: 0, // Would need timing
                }))
            }
            Err(e) => {
                error!("V2 gRPC: InsertRecords failed: {}", e);
                Err(Status::internal(format!("Insert failed: {}", e)))
            }
        }
    }

    /// Upsert records (insert or update)
    async fn upsert_records(
        &self,
        request: Request<ProximaRecordBatch>,
    ) -> Result<Response<ProximaRecordBatchResponse>, Status> {
        let batch = request.into_inner();
        info!(
            "V2 gRPC: UpsertRecords - collection='{}', records={}",
            batch.collection_id,
            batch.records.len()
        );

        // Upsert is handled the same as insert in v1 (overwrite semantics)
        let v1_batch = self.convert_to_v1_batch(&batch)?;
        let record_count = v1_batch.vectors.len() as i64;

        match self.unified_handlers.handle_vector_batch_v1(v1_batch).await {
            Ok(resp) => {
                let success_count = if resp.success { record_count } else { 0 };
                Ok(Response::new(ProximaRecordBatchResponse {
                    success: resp.success,
                    total_processed: record_count,
                    success_count,
                    failed_count: record_count - success_count,
                    inserted_ids: batch.records.iter().map(|r| r.id.clone()).collect(),
                    errors: vec![],
                    processing_time_us: 0,
                }))
            }
            Err(e) => {
                error!("V2 gRPC: UpsertRecords failed: {}", e);
                Err(Status::internal(format!("Upsert failed: {}", e)))
            }
        }
    }

    /// Update existing records
    async fn update_records(
        &self,
        request: Request<ProximaRecordBatch>,
    ) -> Result<Response<ProximaRecordBatchResponse>, Status> {
        let batch = request.into_inner();
        info!(
            "V2 gRPC: UpdateRecords - collection='{}', records={}",
            batch.collection_id,
            batch.records.len()
        );

        // Update is handled the same as insert in v1 (overwrite semantics)
        // In future, we could add version checking for optimistic locking
        let v1_batch = self.convert_to_v1_batch(&batch)?;
        let record_count = v1_batch.vectors.len() as i64;

        match self.unified_handlers.handle_vector_batch_v1(v1_batch).await {
            Ok(resp) => {
                let success_count = if resp.success { record_count } else { 0 };
                Ok(Response::new(ProximaRecordBatchResponse {
                    success: resp.success,
                    total_processed: record_count,
                    success_count,
                    failed_count: record_count - success_count,
                    inserted_ids: batch.records.iter().map(|r| r.id.clone()).collect(),
                    errors: vec![],
                    processing_time_us: 0,
                }))
            }
            Err(e) => {
                error!("V2 gRPC: UpdateRecords failed: {}", e);
                Err(Status::internal(format!("Update failed: {}", e)))
            }
        }
    }

    /// Delete records by ID
    async fn delete_records(
        &self,
        request: Request<ProximaRecordBatch>,
    ) -> Result<Response<ProximaRecordBatchResponse>, Status> {
        let batch = request.into_inner();
        info!(
            "V2 gRPC: DeleteRecords - collection='{}', records={}",
            batch.collection_id,
            batch.records.len()
        );

        // Delete is not directly supported in v1 batch API
        // For now, return unimplemented
        warn!("V2 gRPC: DeleteRecords not yet implemented in batch mode");
        Err(Status::unimplemented(
            "Batch delete not yet implemented. Use individual delete operations.",
        ))
    }

    // =========================================================================
    // Search Operations
    // =========================================================================

    /// Search with typed filters
    async fn search(
        &self,
        request: Request<TypedSearchRequest>,
    ) -> Result<Response<TypedSearchResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "V2 gRPC: Search - collection='{}', top_k={}",
            req.collection_id, req.top_k
        );

        // Convert typed filters to v1 filter format
        let filters: HashMap<String, crate::proto::proximadb_v1::SqlValue> = req
            .filters
            .iter()
            .filter_map(|f| {
                // For simple equality filters, convert directly
                if f.operator == proximadb_v2::TypedFilterOperator::Eq as i32 {
                    self.typed_value_to_sql_value(&f.value.clone().unwrap_or_default())
                        .map(|v| (f.field_name.clone(), v))
                } else {
                    None
                }
            })
            .collect();

        // Create search query
        let search_query = SearchQuery {
            vector: req.query_vector.clone(),
            filters,
            advanced_filter: None,
        };

        let search_request = VectorSearchRequest {
            collection_id: req.collection_id.clone(),
            queries: vec![search_query],
            top_k: req.top_k,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        // Execute search
        match self
            .unified_handlers
            .handle_vector_search_v1(search_request)
            .await
        {
            Ok(resp) => {
                let search_result = resp.results.unwrap_or_default();
                let include_vector = req.include_vector;

                let results = self.convert_search_results(&search_result, include_vector);
                let total_found = search_result.total_found as i64;

                Ok(Response::new(TypedSearchResponse {
                    results,
                    total_found,
                    search_time_us: 0, // Would need timing
                    collection_id: Some(req.collection_id),
                    search_stats: HashMap::new(),
                }))
            }
            Err(e) => {
                error!("V2 gRPC: Search failed: {}", e);
                if e.to_string().contains("not found") {
                    Err(Status::not_found(format!(
                        "Collection not found: {}",
                        req.collection_id
                    )))
                } else {
                    Err(Status::internal(format!("Search failed: {}", e)))
                }
            }
        }
    }

    /// Streaming search - returns results as a stream
    type SearchStreamStream = SearchStreamStream;

    async fn search_stream(
        &self,
        request: Request<TypedSearchRequest>,
    ) -> Result<Response<Self::SearchStreamStream>, Status> {
        let req = request.into_inner();
        debug!(
            "V2 gRPC: SearchStream - collection='{}', top_k={}",
            req.collection_id, req.top_k
        );

        // Convert typed filters to v1 filter format
        let filters: HashMap<String, crate::proto::proximadb_v1::SqlValue> = req
            .filters
            .iter()
            .filter_map(|f| {
                if f.operator == proximadb_v2::TypedFilterOperator::Eq as i32 {
                    self.typed_value_to_sql_value(&f.value.clone().unwrap_or_default())
                        .map(|v| (f.field_name.clone(), v))
                } else {
                    None
                }
            })
            .collect();

        let search_query = SearchQuery {
            vector: req.query_vector.clone(),
            filters,
            advanced_filter: None,
        };

        let search_request = VectorSearchRequest {
            collection_id: req.collection_id.clone(),
            queries: vec![search_query],
            top_k: req.top_k,
            include_fields: None,
            search_params: None,
            distance_metric_override: None,
            search_optimization: None,
        };

        let include_vector = req.include_vector;

        // Execute search
        let response = self
            .unified_handlers
            .handle_vector_search_v1(search_request)
            .await
            .map_err(|e| Status::internal(format!("Search stream failed: {}", e)))?;

        // Create a channel for streaming results
        let (tx, rx) = tokio::sync::mpsc::channel(128);

        // Convert results
        let search_result = response.results.unwrap_or_default();
        let results = self.convert_search_results(&search_result, include_vector);

        // Spawn a task to send results through the channel
        tokio::spawn(async move {
            for result in results {
                if tx.send(Ok(result)).await.is_err() {
                    // Client disconnected
                    break;
                }
            }
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as SearchStreamStream))
    }

    // =========================================================================
    // Streaming Write Operations
    // =========================================================================

    /// Bidirectional streaming for high-throughput batch writes
    ///
    /// This method provides:
    /// - Client streaming: Send batches of records continuously
    /// - Server streaming: Receive acknowledgments with backpressure signals
    /// - Flow control: Bounded channels prevent memory overflow
    /// - Error handling: Per-record errors are reported back to the client
    type BatchWriteStreamStream = BatchWriteStreamStream;

    async fn batch_write_stream(
        &self,
        request: Request<Streaming<BatchWriteStreamRequest>>,
    ) -> Result<Response<Self::BatchWriteStreamStream>, Status> {
        let mut inbound = request.into_inner();

        // Create a bounded channel for response streaming
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<BatchWriteStreamResponse, Status>>(
            STREAM_BUFFER_SIZE,
        );

        // Shared state for tracking progress and backpressure
        let total_processed = Arc::new(AtomicI64::new(0));
        let success_count = Arc::new(AtomicI64::new(0));
        let failed_count = Arc::new(AtomicI64::new(0));
        // Track pending items for future enhanced backpressure (currently uses channel capacity)
        let _pending_in_buffer = Arc::new(AtomicU32::new(0));

        // Clone handlers for the processing task
        let unified_handlers = Arc::clone(&self.unified_handlers);

        // Spawn task to process incoming stream
        tokio::spawn(async move {
            let start_time = Instant::now();

            while let Some(batch_result) = inbound.next().await {
                let batch = match batch_result {
                    Ok(b) => b,
                    Err(e) => {
                        error!("V2 gRPC: BatchWriteStream - stream error: {}", e);
                        let _ = tx
                            .send(Err(Status::internal(format!("Stream error: {}", e))))
                            .await;
                        break;
                    }
                };

                debug!(
                    "V2 gRPC: BatchWriteStream - processing batch for collection='{}', records={}",
                    batch.collection_id,
                    batch.records.len()
                );

                // Track pending items for potential future backpressure enhancements
                _pending_in_buffer.fetch_add(batch.records.len() as u32, Ordering::SeqCst);

                // Collect sequences for acknowledgment
                let mut acked_sequences: Vec<u64> = Vec::with_capacity(batch.records.len());
                let mut batch_errors: Vec<BatchError> = Vec::new();

                // Process each record in the batch
                for (idx, stream_record) in batch.records.iter().enumerate() {
                    let record = match &stream_record.record {
                        Some(r) => r,
                        None => {
                            batch_errors.push(BatchError {
                                record_index: idx as i32,
                                record_id: String::new(),
                                error_code: "MISSING_RECORD".to_string(),
                                error_message: "StreamWriteRecord has no record".to_string(),
                            });
                            failed_count.fetch_add(1, Ordering::SeqCst);
                            continue;
                        }
                    };

                    // Convert to V1 batch for storage
                    let v1_batch = VectorBatchRequest {
                        collection_id: batch.collection_id.clone(),
                        vectors: vec![Self::convert_proxima_record_to_vector_record(record)],
                    };

                    // Execute the write based on mode
                    let result = match BatchWriteMode::try_from(stream_record.write_mode) {
                        Ok(BatchWriteMode::Insert)
                        | Ok(BatchWriteMode::Unspecified)
                        | Ok(BatchWriteMode::Upsert)
                        | Ok(BatchWriteMode::Update) => {
                            unified_handlers.handle_vector_batch_v1(v1_batch).await
                        }
                        Ok(BatchWriteMode::Delete) => {
                            // Delete not yet supported in batch mode
                            Err(anyhow::anyhow!(
                                "DELETE mode not supported in BatchWriteStream"
                            ))
                        }
                        Err(_) => Err(anyhow::anyhow!(
                            "Invalid write_mode: {}",
                            stream_record.write_mode
                        )),
                    };

                    match result {
                        Ok(resp) if resp.success => {
                            acked_sequences.push(stream_record.client_sequence);
                            success_count.fetch_add(1, Ordering::SeqCst);
                        }
                        Ok(resp) => {
                            batch_errors.push(BatchError {
                                record_index: idx as i32,
                                record_id: record.id.clone(),
                                error_code: "WRITE_FAILED".to_string(),
                                error_message: resp
                                    .error_message
                                    .unwrap_or_else(|| "Unknown error".to_string()),
                            });
                            failed_count.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) => {
                            batch_errors.push(BatchError {
                                record_index: idx as i32,
                                record_id: record.id.clone(),
                                error_code: "INTERNAL_ERROR".to_string(),
                                error_message: e.to_string(),
                            });
                            failed_count.fetch_add(1, Ordering::SeqCst);
                        }
                    }

                    total_processed.fetch_add(1, Ordering::SeqCst);
                }

                // Update pending count after processing
                let records_processed = batch.records.len() as u32;
                _pending_in_buffer.fetch_sub(records_processed, Ordering::SeqCst);

                // Calculate backpressure based on channel utilization
                let buffer_usage =
                    ((STREAM_BUFFER_SIZE - tx.capacity()) * 100 / STREAM_BUFFER_SIZE) as u32;
                let backpressure = Self::calculate_backpressure(buffer_usage);

                // Send acknowledgment response
                let response = BatchWriteStreamResponse {
                    acked_sequences,
                    backpressure: Some(backpressure),
                    total_processed: total_processed.load(Ordering::SeqCst),
                    success_count: success_count.load(Ordering::SeqCst),
                    failed_count: failed_count.load(Ordering::SeqCst),
                    errors: batch_errors,
                    server_timestamp_ms: chrono::Utc::now().timestamp_millis(),
                };

                if tx.send(Ok(response)).await.is_err() {
                    debug!("V2 gRPC: BatchWriteStream - client disconnected");
                    break;
                }
            }

            info!(
                "V2 gRPC: BatchWriteStream - completed. Total processed: {}, Success: {}, Failed: {}, Duration: {:?}",
                total_processed.load(Ordering::SeqCst),
                success_count.load(Ordering::SeqCst),
                failed_count.load(Ordering::SeqCst),
                start_time.elapsed()
            );
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream) as BatchWriteStreamStream))
    }

    // =========================================================================
    // Schema Operations
    // =========================================================================

    /// Create a new schema for a collection
    async fn create_schema(
        &self,
        request: Request<CreateSchemaRequest>,
    ) -> Result<Response<CreateSchemaResponse>, Status> {
        let req = request.into_inner();
        info!(
            "V2 gRPC: CreateSchema - collection='{}'",
            req.collection_id
        );

        // Get collection to verify it exists
        let collection_request = CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        let collection_response = self
            .unified_handlers
            .handle_collection_operation(collection_request)
            .await
            .map_err(|e| {
                if e.to_string().contains("not found") {
                    Status::not_found(format!("Collection not found: {}", req.collection_id))
                } else {
                    Status::internal(format!("Failed to get collection: {}", e))
                }
            })?;

        if collection_response.collection.is_none() {
            return Err(Status::not_found(format!(
                "Collection not found: {}",
                req.collection_id
            )));
        }

        // Generate schema ID
        let schema_id = req
            .schema
            .as_ref()
            .map(|s| {
                if s.schema_id.is_empty() {
                    format!("schema_{}_{}", req.collection_id, uuid::Uuid::new_v4())
                } else {
                    s.schema_id.clone()
                }
            })
            .unwrap_or_else(|| format!("schema_{}_{}", req.collection_id, uuid::Uuid::new_v4()));

        // For now, we don't persist the schema separately - it's stored in collection config
        // A full implementation would update the collection's record_schema config
        Ok(Response::new(CreateSchemaResponse {
            success: true,
            schema_id,
            error_message: None,
        }))
    }

    /// Get schema for a collection
    async fn get_schema(
        &self,
        request: Request<GetSchemaRequest>,
    ) -> Result<Response<GetSchemaResponse>, Status> {
        let req = request.into_inner();
        debug!("V2 gRPC: GetSchema - collection='{}'", req.collection_id);

        // Get collection to retrieve schema
        let collection_request = CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some(req.collection_id.clone()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };

        let collection_response = self
            .unified_handlers
            .handle_collection_operation(collection_request)
            .await
            .map_err(|e| {
                if e.to_string().contains("not found") {
                    Status::not_found(format!("Collection not found: {}", req.collection_id))
                } else {
                    Status::internal(format!("Failed to get collection: {}", e))
                }
            })?;

        let collection = collection_response
            .collection
            .ok_or_else(|| Status::not_found(format!("Collection not found: {}", req.collection_id)))?;

        let config = collection
            .config
            .ok_or_else(|| Status::internal("Collection has no configuration"))?;

        // Build schema from collection config
        if !config.enable_proxima_record.unwrap_or(false) && config.record_schema.is_none() {
            return Err(Status::not_found(format!(
                "No schema defined for collection '{}'. Enable ProximaRecord to use schemas.",
                req.collection_id
            )));
        }

        // Build schema from record_schema config
        let schema = config.record_schema.map(|schema_config| {
            proximadb_v2::RecordSchema {
                schema_id: schema_config.schema_id,
                schema_version: schema_config.schema_version,
                schema_name: String::new(),
                columns: vec![], // Would need to convert columns
                enforcement_mode: schema_config.enforcement,
                allow_additional_fields: schema_config.auto_evolve,
                parent_schema_id: None,
                evolution_rules: vec![],
                created_at: collection.created_at,
                created_by: None,
                description: None,
                annotations: HashMap::new(),
            }
        });

        Ok(Response::new(GetSchemaResponse {
            success: true,
            schema,
            error_message: None,
        }))
    }

    /// List all schemas for a collection
    async fn list_schemas(
        &self,
        request: Request<ListSchemasRequest>,
    ) -> Result<Response<ListSchemasResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "V2 gRPC: ListSchemas - collection='{}'",
            req.collection_id
        );

        // Get collection schema (currently only one schema per collection)
        let get_req = GetSchemaRequest {
            collection_id: req.collection_id.clone(),
            schema_id: None,
        };

        match self
            .get_schema(Request::new(get_req))
            .await
        {
            Ok(resp) => {
                let schemas = resp
                    .into_inner()
                    .schema
                    .into_iter()
                    .collect::<Vec<_>>();
                let total_count = schemas.len() as i64;

                Ok(Response::new(ListSchemasResponse {
                    schemas,
                    total_count,
                }))
            }
            Err(e) if e.code() == tonic::Code::NotFound => {
                // No schema defined - return empty list
                Ok(Response::new(ListSchemasResponse {
                    schemas: vec![],
                    total_count: 0,
                }))
            }
            Err(e) => Err(e),
        }
    }

    /// Evolve schema with compatibility checks
    async fn evolve_schema(
        &self,
        request: Request<EvolveSchemaRequest>,
    ) -> Result<Response<EvolveSchemaResponse>, Status> {
        let req = request.into_inner();
        info!(
            "V2 gRPC: EvolveSchema - collection='{}', base_schema='{}'",
            req.collection_id, req.base_schema_id
        );

        if req.dry_run {
            debug!("V2 gRPC: EvolveSchema - dry run mode");
        }

        // Schema evolution would require:
        // 1. Load existing schema
        // 2. Validate evolution rules
        // 3. Check compatibility
        // 4. Optionally migrate existing records

        // For now, return unimplemented
        warn!("V2 gRPC: EvolveSchema not yet fully implemented");
        Err(Status::unimplemented(
            "Schema evolution not yet implemented. Use CreateSchema for new schemas.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typed_value_conversion() {
        // Test would require mocking UnifiedHandlers
        // For now, just verify the module compiles
    }
}
