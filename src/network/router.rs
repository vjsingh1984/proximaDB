use anyhow::{Context, Result};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::metrics::InternalMetricsUpdater;
use crate::proto::proximadb_v1::{
    CollectionRequest, CollectionResponse, VectorBatchRequest, VectorOperationResponse,
    VectorSearchRequest, HybridSearchRequest, HybridSearchResponse, ExecuteQueryResponse, SqlValue,
};

// --- CustomerRouter Struct ---

pub struct CustomerRouter {
    request_handlers: Arc<UnifiedHandlers>,
    metrics_updater: Arc<dyn InternalMetricsUpdater>,
    // Configuration for tenant ID header name, etc.
    tenant_id_header_name: String,
    // Add clients for remote proximaDB instances if needed in future phases
    // For now, assuming all instances are stateless and can serve any tenant's data
}

impl CustomerRouter {
    pub fn new(
        request_handlers: Arc<UnifiedHandlers>,
        metrics_updater: Arc<dyn InternalMetricsUpdater>,
        tenant_id_header_name: Option<String>,
    ) -> Self {
        Self {
            request_handlers,
            metrics_updater,
            tenant_id_header_name: tenant_id_header_name
                .unwrap_or_else(|| "x-tenant-id".to_string()),
        }
    }

    // Helper to extract tenant_id from gRPC request metadata
    fn extract_tenant_id_grpc<T>(&self, request: &Request<T>) -> String {
        request
            .metadata()
            .get(&self.tenant_id_header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                warn!("Tenant ID header '{}' not found in gRPC request. Using default 'guest'.", self.tenant_id_header_name);
                "guest".to_string() // Default tenant ID
            })
    }

    // Helper to extract tenant_id from HTTP request headers (for axum)
    // This will be used in src/network/rest/v1/handlers.rs
    pub fn extract_tenant_id_http(&self, headers: &axum::http::HeaderMap) -> String {
        headers
            .get(&self.tenant_id_header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                warn!("Tenant ID header '{}' not found in HTTP request. Using default 'guest'.", self.tenant_id_header_name);
                "guest".to_string() // Default tenant ID
            })
    }

    // --- Methods mirroring UnifiedHandlers API calls ---

    pub async fn handle_collection_operation(
        &self,
        request: Request<CollectionRequest>,
    ) -> Result<Response<CollectionResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;

        let result = self
            .request_handlers
            .handle_collection_operation(request.into_inner(), &tenant_id) // Pass tenant_id
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Collection operation failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "collection_operation".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: 0, // Not directly applicable here, or needs deeper logic
                data_scanned_bytes: 0,  // Not directly applicable here, or needs deeper logic
            },
        ).await;

        result
    }

    pub async fn handle_vector_batch_v1(
        &self,
        request: Request<VectorBatchRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;
        let num_vectors = request.get_ref().vectors.len() as u64; // Example of data_inserted_bytes

        let result = self
            .request_handlers
            .handle_vector_batch_v1(request.into_inner(), &tenant_id) // Pass tenant_id
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector batch failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "vector_batch".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: num_vectors, // Example: count of vectors inserted
                data_scanned_bytes: 0,
            },
        ).await;

        result
    }

    pub async fn handle_vector_search_v1(
        &self,
        request: Request<VectorSearchRequest>,
    ) -> Result<Response<VectorOperationResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;

        let result = self
            .request_handlers
            .handle_vector_search_v1(request.into_inner(), &tenant_id) // Pass tenant_id
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector search failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);
        let data_scanned = result.as_ref().ok().and_then(|res| res.get_ref().metrics.as_ref()).map_or(0, |m| m.total_processed as u64); // Example: total_processed from metrics

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "vector_search".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: 0,
                data_scanned_bytes: data_scanned, // Example: total_processed vectors
            },
        ).await;

        result
    }

    pub async fn handle_vector_v1(
        &self,
        request: Request<proximadb_v1::VectorGetRequest>,
    ) -> Result<Response<proximadb_v1::VectorOperationResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;

        let req_inner = request.into_inner();
        let collection_id = req_inner.collection_id.clone();
        let vector_id = req_inner.vector_id.clone();
        let include_vector = req_inner.include_vector.unwrap_or(false);
        let include_metadata = req_inner.include_metadata.unwrap_or(true);

        let result = self
            .request_handlers
            .handle_vector_v1(
                &collection_id,
                &vector_id,
                include_vector,
                include_metadata,
                &tenant_id, // Pass tenant_id
            )
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Vector get failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);
        let data_scanned = if success { 1 } else { 0 }; // Getting one vector

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "vector_get".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: 0,
                data_scanned_bytes: data_scanned,
            },
        ).await;

        result
    }

    pub async fn execute_sql_v1(
        &self,
        request: Request<proximadb_v1::ExecuteQueryRequest>,
    ) -> Result<Response<ExecuteQueryResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;

        let req_inner = request.into_inner();
        let query = req_inner.query.clone();
        let parameters = req_inner.parameters.clone();
        let collection = req_inner.collection.clone();

        let result = self
            .request_handlers
            .execute_sql_v1(
                query,
                parameters,
                collection,
                &tenant_id, // Pass tenant_id
            )
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("SQL execution failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);
        let rows_returned = result.as_ref().ok().map_or(0, |res| res.get_ref().rows_returned);

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "execute_sql".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: 0,
                data_scanned_bytes: rows_returned, // Example: rows returned
            },
        ).await;

        result
    }

    pub async fn execute_hybrid_query(
        &self,
        request: Request<HybridSearchRequest>,
    ) -> Result<Response<HybridSearchResponse>, Status> {
        let tenant_id = self.extract_tenant_id_grpc(&request);
        let start_time = std::time::Instant::now();
        let req_size = request.get_ref().encoded_len() as u64;

        let result = self
            .request_handlers
            .execute_hybrid_query(request.into_inner(), &tenant_id) // Pass tenant_id
            .await
            .map(Response::new)
            .map_err(|e| Status::internal(format!("Hybrid query failed: {}", e)));

        let end_time = std::time::Instant::now();
        let processing_time_us = end_time.duration_since(start_time).as_micros() as u64;
        let success = result.is_ok();
        let res_size = result.as_ref().map_or(0, |res| res.get_ref().encoded_len() as u64);
        let vector_results_count = result.as_ref().ok().and_then(|res| res.get_ref().stats.as_ref()).map_or(0, |s| s.vector_results_count as u64);

        // Record for internal diagnostics (not billing)
        let _ = self.metrics_updater.record_customer_api_call(
            &tenant_id,
            crate::metrics::schema::CustomerApiCallUpdate {
                api_type: "hybrid_query".to_string(),
                request_size_bytes: req_size,
                response_size_bytes: res_size,
                processing_time_us,
                success,
                data_inserted_bytes: 0,
                data_scanned_bytes: vector_results_count, // Example: vector results count
            },
        ).await;

        result
    }
}
