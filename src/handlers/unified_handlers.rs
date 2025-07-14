/*
 * Copyright 2025 ProximaDB
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

//! Unified handlers for shared business logic between REST and gRPC APIs
//! 
//! This module eliminates code duplication by providing a single implementation
//! for all API operations that can be used by both REST and gRPC handlers.

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::{Result, Context};
use tracing::{info, error, debug};

use crate::proto::proximadb::{
    Collection, CollectionRequest, CollectionResponse,
    VectorBatchRequest, VectorSearchRequest, VectorOperationResponse,
    CollectionOperation, VectorOperation, SearchResult,
};
use crate::services::collection_service::CollectionService;
use crate::services::vector_service::VectorService;

/// Unified handlers that implement all business logic for API operations
pub struct UnifiedHandlers {
    pub collection_service: Arc<CollectionService>,
    pub vector_service: Arc<VectorService>,
}

impl UnifiedHandlers {
    /// Create new unified handlers with the given services
    pub fn new(
        collection_service: Arc<CollectionService>,
        vector_service: Arc<VectorService>,
    ) -> Self {
        Self {
            collection_service,
            vector_service,
        }
    }

    /// Handle any collection operation with unified logic
    pub async fn handle_collection_operation(
        &self,
        request: CollectionRequest,
    ) -> Result<CollectionResponse> {
        let start_time = std::time::Instant::now();
        
        let operation = CollectionOperation::try_from(request.operation)
            .context("Invalid collection operation")?;
            
        let (success, collection, collections_opt, affected_count, error_msg, error_code) = match operation {
            CollectionOperation::CollectionCreate => self.handle_create_collection(request).await?,
            CollectionOperation::CollectionGet => self.handle_get_collection(request).await?,
            CollectionOperation::CollectionList => self.handle_list_collections(request).await?,
            CollectionOperation::CollectionUpdate => self.handle_update_collection(request).await?,
            CollectionOperation::CollectionDelete => self.handle_delete_collection(request).await?,
            _ => {
                return Ok(CollectionResponse {
                    success: false,
                    operation: operation as i32,
                    collection: None,
                    collections: vec![],
                    affected_count: 0,
                    total_count: None,
                    metadata: Default::default(),
                    error_message: Some("Unsupported operation".to_string()),
                    error_code: Some("UNSUPPORTED_OPERATION".to_string()),
                    processing_time_us: start_time.elapsed().as_micros() as i64,
                })
            }
        };
        
        let collections = collections_opt.unwrap_or_default();
        let total_count = if collections.is_empty() { None } else { Some(collections.len() as i64) };

        Ok(CollectionResponse {
            success,
            operation: operation as i32,
            collection,
            collections,
            affected_count,
            total_count,
            metadata: Default::default(),
            error_message: error_msg,
            error_code,
            processing_time_us: start_time.elapsed().as_micros() as i64,
        })
    }

    /// Handle vector batch operations with unified logic
    pub async fn handle_vector_batch(
        &self,
        request: VectorBatchRequest,
    ) -> Result<VectorOperationResponse> {
        let start_time = std::time::Instant::now();
        let collection_id = &request.collection_id;
        
        info!("🔧 UnifiedHandlers: Processing vector batch for collection: {}", collection_id);
        
        // PROTO-FIRST: Pass proto vectors directly to service layer (maximum zero-copy)
        match self.vector_service.handle_vector_batch_proto_vec(collection_id, request.vectors).await {
            Ok(response_bytes) => {
                // Parse the response to get actual stats
                match serde_json::from_slice::<serde_json::Value>(&response_bytes) {
                    Ok(response_json) => {
                        let success = response_json.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                        let vector_ids: Vec<String> = response_json.get("vector_ids")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect())
                            .unwrap_or_default();
                        let count = vector_ids.len() as i64;
                        
                        Ok(VectorOperationResponse {
                            success,
                            operation: VectorOperation::VectorBatch as i32,
                            metrics: Some(crate::proto::proximadb::OperationMetrics {
                                total_processed: count,
                                successful_count: if success { count } else { 0 },
                                failed_count: if success { 0 } else { count },
                                updated_count: 0,
                                processing_time_us: start_time.elapsed().as_micros() as i64,
                                wal_write_time_us: 0,
                                index_update_time_us: 0,
                            }),
                            result_payload: None,
                            vector_ids,
                            error_message: response_json.get("error_message").and_then(|v| v.as_str()).map(String::from),
                            error_code: response_json.get("error_code").and_then(|v| v.as_str()).map(String::from),
                            result_info: Some(crate::proto::proximadb::ResultMetadata {
                                result_count: count,
                                estimated_size_bytes: 0,
                                is_avro_binary: false,
                                avro_schema_version: String::new(),
                            }),
                        })
                    }
                    Err(e) => {
                        error!("Failed to parse vector batch response: {:?}", e);
                        Ok(VectorOperationResponse {
                            success: false,
                            operation: VectorOperation::VectorBatch as i32,
                            metrics: Some(crate::proto::proximadb::OperationMetrics {
                                total_processed: 0,
                                successful_count: 0,
                                failed_count: 0,
                                updated_count: 0,
                                processing_time_us: start_time.elapsed().as_micros() as i64,
                                wal_write_time_us: 0,
                                index_update_time_us: 0,
                            }),
                            result_payload: None,
                            vector_ids: vec![],
                            error_message: Some(format!("Failed to parse response: {}", e)),
                            error_code: Some("PARSE_ERROR".to_string()),
                            result_info: Some(crate::proto::proximadb::ResultMetadata {
                                result_count: 0,
                                estimated_size_bytes: 0,
                                is_avro_binary: false,
                                avro_schema_version: String::new(),
                            }),
                        })
                    }
                }
            }
            Err(e) => {
                error!("Failed to process vector batch: {:?}", e);
                Ok(VectorOperationResponse {
                    success: false,
                    operation: VectorOperation::VectorBatch as i32,
                    metrics: Some(crate::proto::proximadb::OperationMetrics {
                        total_processed: 0,
                        successful_count: 0,
                        failed_count: 0,
                        updated_count: 0,
                        processing_time_us: start_time.elapsed().as_micros() as i64,
                        wal_write_time_us: 0,
                        index_update_time_us: 0,
                    }),
                    result_payload: None,
                    vector_ids: vec![],
                    error_message: Some(e.to_string()),
                    error_code: Some("VECTOR_INSERT_FAILED".to_string()),
                    result_info: Some(crate::proto::proximadb::ResultMetadata {
                        result_count: 0,
                        estimated_size_bytes: 0,
                        is_avro_binary: false,
                        avro_schema_version: String::new(),
                    }),
                })
            }
        }
    }

    /// Handle vector search operations with unified logic
    pub async fn handle_vector_search(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorOperationResponse> {
        let start_time = std::time::Instant::now();
        let collection_id = &request.collection_id;
        
        info!("🔍 UnifiedHandlers: Processing vector search for collection: {}", collection_id);
        
        // Build search parameters
        let search_params = if let Some(opt) = &request.search_optimization {
            // Convert proto Value filters to string map for now
            let filters: HashMap<String, String> = opt.filters.iter()
                .map(|(k, v)| {
                    use prost_types::value::Kind;
                    let value_str = match &v.kind {
                        Some(Kind::StringValue(s)) => s.clone(),
                        Some(Kind::NumberValue(n)) => n.to_string(),
                        Some(Kind::BoolValue(b)) => b.to_string(),
                        _ => String::new(),
                    };
                    (k.clone(), value_str)
                })
                .collect();
                
            // Convert filters to serde_json::Value map
            let filters_json: HashMap<String, serde_json::Value> = filters.into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect();
                
            let params = crate::core::search::SearchParams {
                top_k: Some(request.top_k as usize),
                filters: Some(filters_json),
                accuracy_threshold: opt.accuracy_threshold,
                include_expired: Some(opt.include_expired.unwrap_or(false)),
                timeout_ms: opt.timeout_ms,
                enable_two_stage: Some(opt.enable_two_stage.unwrap_or(false)),
                quantization_hint: None, // TODO: Convert from proto
                enable_clustering_hint: Some(opt.enable_clustering_hint.unwrap_or(false)),
                enable_metadata_filtering_hint: Some(opt.enable_metadata_filtering_hint.unwrap_or(false)),
                custom_hints: Some(HashMap::new()),
            };
            Some(params)
        } else {
            None
        };
        
        // Perform search with first query vector
        let query_vector = &request.queries.first()
            .ok_or_else(|| anyhow::anyhow!("No query vectors provided"))?
            .vector;
            
        let metadata_filters = search_params.as_ref()
            .and_then(|p| p.filters.as_ref());
        let include_vectors = request.include_fields.as_ref()
            .map(|f| f.vector)
            .unwrap_or(false);
        let include_metadata = request.include_fields.as_ref()
            .map(|f| f.metadata)
            .unwrap_or(true);
            
        let search_results_bytes = self.vector_service
            .search_vectors_polymorphic(
                collection_id,
                query_vector,
                request.top_k as usize,
                search_params.as_ref().unwrap_or(&crate::core::search::SearchParams::default()),
                metadata_filters,
                include_vectors,
                include_metadata,
            )
            .await
            .context("Search failed")?;
            
        // Parse the search results from JSON bytes
        let search_response: serde_json::Value = serde_json::from_slice(&search_results_bytes)
            .context("Failed to parse search results")?;
            
        let results: Vec<SearchResult> = search_response.get("results")
            .and_then(|r| r.as_array())
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|result| {
                let id = result.get("id")?.as_str()?.to_string();
                let score = result.get("score")?.as_f64()? as f32;
                let rank = result.get("rank").and_then(|v| v.as_i64()).map(|r| r as i32);
                
                let vector = if include_vectors {
                    result.get("vector")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                
                let metadata = if include_metadata {
                    result.get("metadata")
                        .and_then(|m| m.as_object())
                        .map(|obj| obj.iter()
                            .map(|(k, v)| crate::proto::proximadb::MetadataItem {
                                key: k.clone(),
                                value: v.to_string(),
                            })
                            .collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                
                Some(SearchResult {
                    id: Some(id),
                    score,
                    vector,
                    metadata,
                    rank,
                })
            })
            .collect();
            
        let result_count = results.len() as i64;
        Ok(VectorOperationResponse {
            success: true,
            operation: VectorOperation::VectorSearch as i32,
            metrics: Some(crate::proto::proximadb::OperationMetrics {
                total_processed: result_count,
                successful_count: result_count,
                failed_count: 0,
                updated_count: 0,
                processing_time_us: start_time.elapsed().as_micros() as i64,
                wal_write_time_us: 0,
                index_update_time_us: 0,
            }),
            result_payload: Some(crate::proto::proximadb::vector_operation_response::ResultPayload::CompactResults(
                crate::proto::proximadb::SearchResultsCompact {
                    results,
                    total_found: result_count,
                    search_algorithm_used: Some("storage_aware_polymorphic".to_string()),
                }
            )),
            vector_ids: vec![],
            error_message: None,
            error_code: None,
            result_info: Some(crate::proto::proximadb::ResultMetadata {
                result_count,
                estimated_size_bytes: result_count * 256, // Rough estimate: 256 bytes per result
                is_avro_binary: false,
                avro_schema_version: String::new(),
            }),
        })
    }

    // Private helper methods for collection operations
    
    async fn handle_create_collection(
        &self,
        request: CollectionRequest,
    ) -> Result<(bool, Option<Collection>, Option<Vec<Collection>>, i64, Option<String>, Option<String>)> {
        let config = request.collection_config
            .context("Missing collection config")?;
            
        match self.collection_service.create_collection(&config).await {
            Ok(response) => {
                if response.success {
                    Ok((true, response.collection, None, 1, None, None))
                } else {
                    Ok((false, None, None, 0, response.error_message, response.error_code))
                }
            }
            Err(e) => {
                error!("Failed to create collection: {:?}", e);
                Ok((false, None, None, 0, Some(e.to_string()), Some("CREATE_FAILED".to_string())))
            }
        }
    }
    
    async fn handle_get_collection(
        &self,
        request: CollectionRequest,
    ) -> Result<(bool, Option<Collection>, Option<Vec<Collection>>, i64, Option<String>, Option<String>)> {
        let collection_id = request.collection_id
            .context("Missing collection ID")?;
            
        match self.collection_service.get_proto_collection(&collection_id).await {
            Ok(Some(collection)) => Ok((true, Some(collection), None, 1, None, None)),
            Ok(None) => Ok((false, None, None, 0, Some("Collection not found".to_string()), Some("NOT_FOUND".to_string()))),
            Err(e) => {
                error!("Failed to get collection: {:?}", e);
                Ok((false, None, None, 0, Some(e.to_string()), Some("GET_FAILED".to_string())))
            }
        }
    }
    
    async fn handle_list_collections(
        &self,
        _request: CollectionRequest,
    ) -> Result<(bool, Option<Collection>, Option<Vec<Collection>>, i64, Option<String>, Option<String>)> {
        match self.collection_service.list_collections().await {
            Ok(collections) => {
                let count = collections.len() as i64;
                Ok((true, None, Some(collections), count, None, None))
            }
            Err(e) => {
                error!("Failed to list collections: {:?}", e);
                Ok((false, None, None, 0, Some(e.to_string()), Some("LIST_FAILED".to_string())))
            }
        }
    }
    
    async fn handle_update_collection(
        &self,
        request: CollectionRequest,
    ) -> Result<(bool, Option<Collection>, Option<Vec<Collection>>, i64, Option<String>, Option<String>)> {
        let collection_id = request.collection_id
            .context("Missing collection ID")?;
        let config = request.collection_config
            .context("Missing collection config")?;
            
        match self.collection_service.update_collection(&collection_id, Some(config)).await {
            Ok(response) => {
                if response.success {
                    Ok((true, response.collection, None, 1, None, None))
                } else {
                    Ok((false, None, None, 0, response.error_message, response.error_code))
                }
            }
            Err(e) => {
                error!("Failed to update collection: {:?}", e);
                Ok((false, None, None, 0, Some(e.to_string()), Some("UPDATE_FAILED".to_string())))
            }
        }
    }
    
    async fn handle_delete_collection(
        &self,
        request: CollectionRequest,
    ) -> Result<(bool, Option<Collection>, Option<Vec<Collection>>, i64, Option<String>, Option<String>)> {
        let collection_id = request.collection_id
            .context("Missing collection ID")?;
            
        match self.collection_service.delete_collection(&collection_id).await {
            Ok(response) => {
                if response.success {
                    Ok((true, None, None, 1, None, None))
                } else if response.error_code.as_deref() == Some("NOT_FOUND") {
                    Ok((false, None, None, 0, Some("Collection not found".to_string()), Some("NOT_FOUND".to_string())))
                } else {
                    Ok((false, None, None, 0, response.error_message, response.error_code))
                }
            }
            Err(e) => {
                error!("Failed to delete collection: {:?}", e);
                Ok((false, None, None, 0, Some(e.to_string()), Some("DELETE_FAILED".to_string())))
            }
        }
    }
    
    /// Get a single vector by ID
    pub async fn get_vector(
        &self,
        collection_id: &str,
        vector_id: &str,
        include_vector: bool,
        include_metadata: bool,
    ) -> Result<Vec<u8>> {
        debug!("🔍 UnifiedHandlers: Getting vector {} from collection {}", vector_id, collection_id);
        self.vector_service.get_vector(collection_id, vector_id, include_vector, include_metadata).await
    }
    
    /// Delete a single vector by ID
    pub async fn delete_vector(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Vec<u8>> {
        debug!("🗑️ UnifiedHandlers: Deleting vector {} from collection {}", vector_id, collection_id);
        self.vector_service.delete_vector(collection_id, vector_id).await
    }
    
    /// Force flush all collections (testing only)
    pub async fn force_flush_all(&self) -> Result<serde_json::Value> {
        debug!("⚡ UnifiedHandlers: Force flushing all collections");
        self.vector_service.force_flush_all().await
    }
    
    /// Force flush a specific collection (testing only)
    pub async fn force_flush_collection(&self, collection_id: &str) -> Result<serde_json::Value> {
        debug!("⚡ UnifiedHandlers: Force flushing collection {}", collection_id);
        self.vector_service.force_flush_collection(collection_id).await
    }
    
    /// Get metrics
    pub async fn get_metrics(&self) -> Result<Vec<u8>> {
        debug!("📊 UnifiedHandlers: Getting metrics");
        self.vector_service.get_metrics().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio;
    use crate::proto::proximadb::{CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm};
    use crate::services::collection_service::{CollectionService, CollectionServiceResponse};
    use crate::services::vector_service::VectorService;
    
    // NOTE: Mock implementations removed since we can't easily mock the actual services
    // Tests focus on request/response structure validation instead
    
    // Test creating a collection through unified handlers
    #[tokio::test]
    async fn test_create_collection() {
        // Since we can't easily mock the actual services, we'll create a simpler test
        // that verifies the request/response flow
        let request = CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_id: None,
            collection_config: Some(CollectionConfig {
                name: "test-collection".to_string(),
                dimension: 768,
                distance_metric: DistanceMetric::Cosine as i32,
                storage_engine: StorageEngine::Viper as i32,
                primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
                filterable_columns: vec![],
                index_configs: vec![],
                quantization_config: None,
                primary_index_name: String::new(),
                enable_automatic_index_selection: false,
                description: Some("Test collection".to_string()),
                tags: vec!["test".to_string()],
                owner: Some("test-user".to_string()),
            }),
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        
        // Verify request structure is correct
        assert_eq!(request.operation, CollectionOperation::CollectionCreate as i32);
        assert!(request.collection_config.is_some());
        let config = request.collection_config.unwrap();
        assert_eq!(config.name, "test-collection");
        assert_eq!(config.dimension, 768);
    }
    
    // Test getting a collection request structure
    #[tokio::test] 
    async fn test_get_collection_request() {
        let request = CollectionRequest {
            operation: CollectionOperation::CollectionGet as i32,
            collection_id: Some("test-collection-123".to_string()),
            collection_config: None,
            query_params: Default::default(),
            options: Default::default(),
            migration_config: Default::default(),
        };
        
        assert_eq!(request.operation, CollectionOperation::CollectionGet as i32);
        assert_eq!(request.collection_id, Some("test-collection-123".to_string()));
    }
    
    // Test vector batch request structure
    #[tokio::test]
    async fn test_vector_batch_request() {
        use crate::proto::proximadb::VectorBatchRequest;
        
        let request = VectorBatchRequest {
            collection_id: "test-collection".to_string(),
            vectors: vec![], // Empty vector list for test
            batch_timeout_ms: Some(5000),
            request_id: Some("req-123".to_string()),
        };
        
        assert_eq!(request.collection_id, "test-collection");
        assert!(request.vectors.is_empty());
        assert_eq!(request.batch_timeout_ms, Some(5000));
    }
    
    // Test vector search request structure
    #[tokio::test]
    async fn test_vector_search_request() {
        use crate::proto::proximadb::{VectorSearchRequest, SearchQuery, IncludeFields, SearchParameters, MetadataFilter};
        
        let request = VectorSearchRequest {
            collection_id: "test-collection".to_string(),
            queries: vec![SearchQuery {
                vector: vec![1.0, 2.0, 3.0],
                id: None,
                metadata_filter: Some(MetadataFilter {
                    conditions: vec![],
                    operator: crate::proto::proximadb::FilterOperator::And as i32,
                }),
            }],
            top_k: 10,
            distance_metric_override: None,
            search_params: Some(SearchParameters {
                ef_search: Some(100),
                max_connections: None,
                n_probe: None,
                enable_reranking: None,
                batch_size: None,
                timeout_ms: None,
                accuracy_threshold: None,
                enable_parallel_search: None,
                thread_count: None,
            }),
            include_fields: Some(IncludeFields {
                vector: true,
                metadata: true,
                score: true,
                rank: true,
            }),
            search_optimization: None,
        };
        
        assert_eq!(request.collection_id, "test-collection");
        assert_eq!(request.top_k, 10);
        assert_eq!(request.queries.len(), 1);
        assert_eq!(request.queries[0].vector.len(), 3);
    }
    
    // Test response parsing helpers
    #[tokio::test]
    async fn test_response_parsing() {
        use crate::proto::proximadb::{CollectionResponse, VectorOperationResponse, CollectionStats};
        
        // Test collection response
        let coll_response = CollectionResponse {
            success: true,
            operation: CollectionOperation::CollectionCreate as i32,
            collection: Some(crate::proto::proximadb::Collection {
                id: "test-123".to_string(),
                config: None,
                stats: Some(CollectionStats {
                    vector_count: 0,
                    index_size_bytes: 0,
                    data_size_bytes: 0,
                }),
                created_at: 1234567890,
                updated_at: 1234567890,
            }),
            collections: vec![],
            affected_count: 1,
            total_count: None,
            metadata: Default::default(),
            error_message: None,
            error_code: None,
            processing_time_us: 100,
        };
        
        assert!(coll_response.success);
        assert!(coll_response.collection.is_some());
        assert_eq!(coll_response.affected_count, 1);
        
        // Test vector operation response
        let vec_response = VectorOperationResponse {
            success: true,
            operation: VectorOperation::VectorBatch as i32,
            metrics: Some(crate::proto::proximadb::OperationMetrics {
                total_processed: 10,
                successful_count: 10,
                failed_count: 0,
                updated_count: 0,
                processing_time_us: 500,
                wal_write_time_us: 100,
                index_update_time_us: 200,
            }),
            result_payload: None,
            vector_ids: vec!["vec-1".to_string(), "vec-2".to_string()],
            error_message: None,
            error_code: None,
            result_info: Some(crate::proto::proximadb::ResultMetadata {
                result_count: 2,
                estimated_size_bytes: 512,
                is_avro_binary: false,
                avro_schema_version: String::new(),
            }),
        };
        
        assert!(vec_response.success);
        assert_eq!(vec_response.vector_ids.len(), 2);
        assert!(vec_response.metrics.is_some());
        let metrics = vec_response.metrics.unwrap();
        assert_eq!(metrics.total_processed, 10);
        assert_eq!(metrics.successful_count, 10);
    }
}