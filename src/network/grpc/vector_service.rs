use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::api_handlers::UnifiedHandlers;
use crate::proto::{proximadb, proximadb_v1};
use crate::proto::proximadb_v1::vector_service_server::{VectorService, VectorServiceServer};

pub struct VectorServiceImpl {
    unified_handlers: Arc<UnifiedHandlers>,
}

impl VectorServiceImpl {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self { unified_handlers }
    }

    pub fn into_server(self) -> VectorServiceServer<Self> { VectorServiceServer::new(self) }
}

#[tonic::async_trait]
impl VectorService for VectorServiceImpl {
    async fn vector_batch(
        &self,
        request: Request<proximadb_v1::VectorBatchRequest>,
    ) -> Result<Response<proximadb_v1::VectorOperationResponse>, Status> {
        let req_v1 = request.into_inner();
        let legacy = proximadb::VectorBatchRequest {
            collection_id: req_v1.collection_id.clone(),
            vectors: req_v1
                .vectors
                .into_iter()
                .map(|v| proximadb::VectorRecord {
                    id: v.id,
                    vector: v.vector,
                    metadata: std::collections::HashMap::new(),
                    timestamp: v.timestamp,
                    updated_at: v.updated_at,
                    expires_at: v.expires_at,
                    version: v.version,
                    quantized_vector: v.quantized_vector,
                    source: v.source,
                })
                .collect(),
        };
        self.unified_handlers
            .handle_vector_batch(legacy)
            .await
            .map(|resp| {
                let v1 = proximadb_v1::VectorOperationResponse {
                    success: resp.success,
                    operation: resp.operation,
                    metrics: resp.metrics.map(|m| proximadb_v1::OperationMetrics {
                        total_processed: m.total_processed,
                        successful_count: m.successful_count,
                        failed_count: m.failed_count,
                        updated_count: m.updated_count,
                        processing_time_us: m.processing_time_us,
                        wal_write_time_us: m.wal_write_time_us,
                        index_update_time_us: m.index_update_time_us,
                    }),
                    results: resp.results.map(|r| proximadb_v1::SearchResult {
                        results: r
                            .results
                            .into_iter()
                            .map(|rec| proximadb_v1::SearchVectorRecord {
                                id: rec.id,
                                score: rec.score,
                                vector: rec.vector,
                                metadata: std::collections::HashMap::new(),
                                version: rec.version,
                            })
                            .collect(),
                        total_found: r.total_found,
                        collection_id: r.collection_id,
                    }),
                    vector_ids: resp.vector_ids,
                    error_message: resp.error_message,
                    error_code: resp.error_code,
                };
                Response::new(v1)
            })
            .map_err(|e| Status::internal(format!("Vector batch failed: {}", e)))
    }

    async fn vector_search(
        &self,
        request: Request<proximadb_v1::VectorSearchRequest>,
    ) -> Result<Response<proximadb_v1::VectorOperationResponse>, Status> {
        let req_v1 = request.into_inner();
        let legacy = proximadb::VectorSearchRequest {
            collection_id: req_v1.collection_id.clone(),
            queries: req_v1
                .queries
                .into_iter()
                .map(|q| proximadb::SearchQuery { vector: q.vector, metadata_filter: None })
                .collect(),
            top_k: req_v1.top_k,
            include_fields: req_v1.include_fields.map(|f| proximadb::IncludeFields { vector: f.vector, metadata: f.metadata }),
            search_params: None,
            distance_metric_override: req_v1.distance_metric_override,
            search_optimization: None,
        };
        self.unified_handlers
            .handle_vector_search(legacy)
            .await
            .map(|response| {
                let v1 = proximadb_v1::VectorOperationResponse {
                    success: response.success,
                    operation: response.operation,
                    metrics: response.metrics.map(|m| proximadb_v1::OperationMetrics {
                        total_processed: m.total_processed,
                        successful_count: m.successful_count,
                        failed_count: m.failed_count,
                        updated_count: m.updated_count,
                        processing_time_us: m.processing_time_us,
                        wal_write_time_us: m.wal_write_time_us,
                        index_update_time_us: m.index_update_time_us,
                    }),
                    results: response.results.map(|r| proximadb_v1::SearchResult {
                        results: r
                            .results
                            .into_iter()
                            .map(|rec| proximadb_v1::SearchVectorRecord {
                                id: rec.id,
                                score: rec.score,
                                vector: rec.vector,
                                metadata: std::collections::HashMap::new(),
                                version: rec.version,
                            })
                            .collect(),
                        total_found: r.total_found,
                        collection_id: r.collection_id,
                    }),
                    vector_ids: response.vector_ids,
                    error_message: response.error_message,
                    error_code: response.error_code,
                };
                Response::new(v1)
            })
            .map_err(|e| Status::internal(format!("Vector search failed: {}", e)))
    }

    async fn vector_get(
        &self,
        request: Request<proximadb_v1::VectorGetRequest>,
    ) -> Result<Response<proximadb_v1::VectorOperationResponse>, Status> {
        let req = request.into_inner();
        let include_vector = req.include_vector.unwrap_or(false);
        let include_metadata = req.include_metadata.unwrap_or(true);
        self.unified_handlers
            .handle_vector(&req.collection_id, &req.vector_id, include_vector, include_metadata)
            .await
            .map(|resp| {
                let v1 = proximadb_v1::VectorOperationResponse {
                    success: resp.success,
                    operation: resp.operation,
                    metrics: resp.metrics.map(|m| proximadb_v1::OperationMetrics {
                        total_processed: m.total_processed,
                        successful_count: m.successful_count,
                        failed_count: m.failed_count,
                        updated_count: m.updated_count,
                        processing_time_us: m.processing_time_us,
                        wal_write_time_us: m.wal_write_time_us,
                        index_update_time_us: m.index_update_time_us,
                    }),
                    results: resp.results.map(|r| proximadb_v1::SearchResult {
                        results: r
                            .results
                            .into_iter()
                            .map(|rec| proximadb_v1::SearchVectorRecord {
                                id: rec.id,
                                score: rec.score,
                                vector: rec.vector,
                                metadata: std::collections::HashMap::new(),
                                version: rec.version,
                            })
                            .collect(),
                        total_found: r.total_found,
                        collection_id: r.collection_id,
                    }),
                    vector_ids: resp.vector_ids,
                    error_message: resp.error_message,
                    error_code: resp.error_code,
                };
                Response::new(v1)
            })
            .map_err(|e| Status::internal(format!("Vector get failed: {}", e)))
    }
}
