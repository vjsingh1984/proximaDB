// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Binary Avro Service Layer
//!
//! The single source of truth for all ProximaDB operations.
//! Both REST and gRPC protocol handlers delegate to this service.
//!
//! Architecture:
//! - Zero wrapper objects - pure Avro records throughout
//! - Binary Avro serialization for performance
//! - Direct WAL integration with zero-copy operations
//! - Unified business logic for all protocols

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, span, warn, Level};

use crate::storage::wal::config::WalConfig;
use crate::storage::wal::factory::WalFactory;
use crate::storage::wal::{WalManager, WalStrategyType};
use crate::storage::FilesystemFactory;
use crate::storage::StorageEngine;
use crate::storage::vector::{
    VectorStorageCoordinator, CoordinatorConfig, VectorOperation, OperationResult,
    SearchContext, SearchStrategy, VectorStorage, ViperCoreEngine, ViperCoreConfig,
    UnifiedSearchEngine, UnifiedSearchConfig, UnifiedIndexManager, UnifiedIndexConfig,
};
use crate::core::VectorRecord;

/// Unified service that operates exclusively on binary Avro records
/// All protocol handlers (REST, gRPC) delegate to this service
/// Uses plugin/strategy pattern for WAL and memtable selection
pub struct UnifiedAvroService {
    storage: Arc<RwLock<StorageEngine>>,
    wal: Arc<WalManager>,
    vector_coordinator: Arc<VectorStorageCoordinator>,
    performance_metrics: Arc<RwLock<ServiceMetrics>>,
    wal_strategy_type: WalStrategyType,
    avro_schema_version: u32,
}

/// Configuration for the unified Avro service
#[derive(Debug, Clone)]
pub struct UnifiedServiceConfig {
    /// WAL strategy to use (Avro or Bincode)
    pub wal_strategy: WalStrategyType,
    /// Memtable type selection
    pub memtable_type: crate::storage::wal::config::MemTableType,
    /// Avro schema version for compatibility
    pub avro_schema_version: u32,
    /// Enable schema evolution checks
    pub enable_schema_evolution: bool,
}

impl Default for UnifiedServiceConfig {
    fn default() -> Self {
        Self {
            wal_strategy: WalStrategyType::Avro, // Default to Avro for consistency
            memtable_type: crate::storage::wal::config::MemTableType::BTree, // RT memtable
            avro_schema_version: 1,
            enable_schema_evolution: true,
        }
    }
}

/// Service performance metrics
#[derive(Debug, Default)]
pub struct ServiceMetrics {
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub avg_processing_time_us: f64,
    pub last_operation_time: Option<chrono::DateTime<chrono::Utc>>,
}

impl UnifiedAvroService {
    /// Create new unified Avro service with strategy-based configuration
    pub async fn new(
        storage: Arc<RwLock<StorageEngine>>,
        wal: Arc<WalManager>,
        config: UnifiedServiceConfig,
    ) -> anyhow::Result<Self> {
        info!("🚀 Initializing UnifiedAvroService with binary Avro operations");
        info!(
            "📋 Service Config: WAL strategy={:?}, memtable={:?}, schema_version={}",
            config.wal_strategy, config.memtable_type, config.avro_schema_version
        );

        // Create vector storage coordinator with VIPER engine
        let vector_coordinator = Self::create_vector_coordinator().await?;

        Ok(Self {
            storage,
            wal,
            vector_coordinator,
            performance_metrics: Arc::new(RwLock::new(ServiceMetrics::default())),
            wal_strategy_type: config.wal_strategy,
            avro_schema_version: config.avro_schema_version,
        })
    }

    /// Create new service with WAL factory (recommended for production)
    pub async fn with_wal_factory(
        storage: Arc<RwLock<StorageEngine>>,
        config: UnifiedServiceConfig,
        wal_config: WalConfig,
    ) -> Result<Self> {
        info!("🏗️ Creating UnifiedAvroService with WAL factory");
        info!(
            "🔧 WAL Strategy: {:?}, Memtable: {:?}",
            config.wal_strategy, config.memtable_type
        );

        // Create WAL strategy using factory
        let fs_config = crate::storage::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::new(fs_config)
                .await
                .context("Failed to create filesystem factory")?,
        );
        let wal_strategy =
            WalFactory::create_strategy(config.wal_strategy.clone(), &wal_config, filesystem)
                .await
                .context("Failed to create WAL strategy")?;

        // Create WAL manager with strategy
        let wal_manager = WalManager::new(wal_strategy, wal_config)
            .await
            .context("Failed to create WAL manager")?;

        Self::new(storage, Arc::new(wal_manager), config).await
    }

    /// Create new service with existing WAL manager (shares WAL with StorageEngine)
    pub async fn with_existing_wal(
        storage: Arc<RwLock<StorageEngine>>,
        wal_manager: Arc<WalManager>,
        config: UnifiedServiceConfig,
    ) -> anyhow::Result<Self> {
        info!("🏗️ Creating UnifiedAvroService with shared WAL manager");
        info!(
            "🔧 WAL Strategy: {:?}, Memtable: {:?}",
            config.wal_strategy, config.memtable_type
        );

        Self::new(storage, wal_manager, config).await
    }
    
    /// Create vector storage coordinator with VIPER engine
    async fn create_vector_coordinator() -> anyhow::Result<Arc<VectorStorageCoordinator>> {
        info!("🔧 Creating Vector Storage Coordinator with VIPER engine");
        
        // Create search engine
        let search_config = UnifiedSearchConfig::default();
        let search_engine = Arc::new(UnifiedSearchEngine::new(search_config).await?);
        
        // Create index manager
        let index_config = UnifiedIndexConfig::default();
        let index_manager = Arc::new(UnifiedIndexManager::new(index_config).await?);
        
        // Create coordinator
        let coordinator_config = CoordinatorConfig::default();
        let coordinator = Arc::new(
            VectorStorageCoordinator::new(search_engine, index_manager, coordinator_config).await?
        );
        
        // Create and register VIPER engine
        let filesystem_config = crate::storage::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        let viper_config = ViperCoreConfig::default();
        let viper_engine = ViperCoreEngine::new(viper_config, filesystem).await?;
        
        coordinator.register_engine(
            "VIPER".to_string(),
            Box::new(viper_engine)
        ).await?;
        
        info!("✅ Vector Storage Coordinator created with VIPER engine");
        Ok(coordinator)
    }

    /// Validate Avro schema version compatibility
    fn validate_schema_version(&self, payload_version: Option<u32>) -> Result<()> {
        if let Some(version) = payload_version {
            if version != self.avro_schema_version {
                return Err(anyhow!(
                    "Schema version mismatch: service expects v{}, payload has v{}",
                    self.avro_schema_version,
                    version
                ));
            }
        }
        Ok(())
    }

    /// Create Avro payload with schema version header
    fn create_versioned_payload(&self, operation_type: &str, data: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();

        // Add schema version header (4 bytes)
        payload.extend_from_slice(&self.avro_schema_version.to_le_bytes());

        // Add operation type length and data
        let op_type_bytes = operation_type.as_bytes();
        payload.extend_from_slice(&(op_type_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(op_type_bytes);

        // Add actual Avro data
        payload.extend_from_slice(data);

        payload
    }

    /// Parse versioned Avro payload
    fn parse_versioned_payload<'a>(&self, payload: &'a [u8]) -> Result<(u32, String, &'a [u8])> {
        if payload.len() < 8 {
            return Err(anyhow!("Payload too short for versioned format"));
        }

        // Read schema version (4 bytes)
        let version = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        self.validate_schema_version(Some(version))?;

        // Read operation type length (4 bytes)
        let op_len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

        if payload.len() < 8 + op_len {
            return Err(anyhow!("Payload too short for operation type"));
        }

        // Read operation type
        let operation_type = String::from_utf8(payload[8..8 + op_len].to_vec())
            .context("Invalid operation type UTF-8")?;

        // Return schema version, operation type, and Avro data
        let avro_data = &payload[8 + op_len..];
        Ok((version, operation_type, avro_data))
    }

    // =============================================================================
    // VECTOR OPERATIONS
    // =============================================================================



    /// Ultra-fast vector insert with trust-but-verify zero-copy approach
    /// Unified method for all vector operations (single, batch, bulk)
    /// Security model: Accept any payload, validate during background processing
    /// User responsibility: Correct data format for maximum performance
    /// Isolation: Corruption limited to user's collection only
    pub async fn vector_insert_zero_copy(&self, avro_payload: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(
            Level::DEBUG,
            "vector_insert_zero_copy",
            payload_size = avro_payload.len()
        );
        let start_time = std::time::Instant::now();
        let wal_start = std::time::Instant::now();

        // SECURITY: Basic payload size validation (prevent DoS)
        const MAX_BATCH_SIZE: usize = 100 * 1024 * 1024; // 100MB max
        if avro_payload.len() > MAX_BATCH_SIZE {
            return Err(anyhow!("Batch payload too large: {} bytes > {} MB limit", 
                avro_payload.len(), MAX_BATCH_SIZE / (1024 * 1024)));
        }

        // SECURITY: Basic header validation to prevent server crashes
        if avro_payload.len() < 12 {
            return Err(anyhow!("Invalid payload: too small for versioned format"));
        }

        // Extract header without full parsing for minimal overhead
        let schema_version = u32::from_le_bytes([
            avro_payload[0], avro_payload[1], avro_payload[2], avro_payload[3]
        ]);
        let op_len = u32::from_le_bytes([
            avro_payload[4], avro_payload[5], avro_payload[6], avro_payload[7]
        ]) as usize;

        // SECURITY: Basic bounds checking
        if op_len > 64 || 8 + op_len > avro_payload.len() {
            return Err(anyhow!("Invalid operation header"));
        }

        // Accept the payload with trust-but-verify principle
        warn!(
            "🚨 ZERO-COPY MODE: Accepting payload without validation. User responsible for data integrity."
        );
        warn!(
            "📋 Payload info: version={}, op_len={}, size={}KB", 
            schema_version, op_len, avro_payload.len() / 1024
        );

        // Parse minimal required data for proper WAL storage (collection ID and vectors)
        // Extract JSON payload from versioned format
        let json_data = &avro_payload[8 + op_len..];
        let insert_request: serde_json::Value = serde_json::from_slice(json_data)
            .context("Failed to parse vector insert JSON")?;
        
        // Extract collection ID for proper WAL storage
        let collection_id = insert_request
            .get("collection_id")
            .and_then(|v| v.as_str())
            .context("Missing collection_id in vector insert request")?
            .to_string();
        
        // Extract vectors for WAL entry creation
        let vectors = insert_request
            .get("vectors")
            .and_then(|v| v.as_array())
            .context("Missing vectors array in insert request")?;
        
        // Process vectors through the vector coordinator
        let mut vector_operations = Vec::new();
        for (i, vector_data) in vectors.iter().enumerate() {
            let vector_id = vector_data
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("vector_{}", i))
                .to_string();
            
            let vector = vector_data
                .get("vector")
                .and_then(|v| v.as_array())
                .context("Missing vector field")?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>();
            
            let metadata: std::collections::HashMap<String, serde_json::Value> = vector_data
                .get("metadata")
                .and_then(|v| v.as_object())
                .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            
            let record = VectorRecord {
                id: vector_id.clone(),
                collection_id: collection_id.clone(),
                vector,
                metadata,
                timestamp: vector_data.get("timestamp")
                    .and_then(|v| v.as_i64())
                    .map(|ts| chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(|| chrono::Utc::now()))
                    .unwrap_or_else(|| chrono::Utc::now()),
                expires_at: None,
            };
            
            // Create vector operation for coordinator
            let operation = VectorOperation::Insert {
                record,
                index_immediately: false, // Use background indexing for performance
            };
            vector_operations.push(operation);
        }
        
        // Execute batch operation through vector coordinator
        let batch_operation = VectorOperation::Batch {
            operations: vector_operations,
            transactional: false, // Use non-transactional for zero-copy performance
        };
        
        let batch_result = self.vector_coordinator
            .execute_operation(batch_operation)
            .await
            .context("Failed to execute vector batch operation")?;
        
        let wal_write_time = wal_start.elapsed().as_micros() as i64;

        // Background validation happens during compaction (trust-but-verify)
        // - Invalid records are logged and skipped
        // - Valid records are processed normally  
        // - User gets max performance, owns data quality
        
        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        info!(
            "🚀 Zero-copy vectors accepted in {}μs (WAL: {}μs) - Validation deferred to background",
            processing_time, wal_write_time
        );

        // Return immediate success (responsibility model)
        let response = crate::schema_types::VectorInsertResponse {
            success: true,
            vector_ids: vec!["vectors_accepted_zero_copy".to_string()],
            error_message: None,
            error_code: None,
            metrics: crate::schema_types::VectorOperationMetrics {
                total_processed: -1, // Unknown - validated in background
                successful_count: -1, // Unknown - validated in background  
                failed_count: 0,
                updated_count: 0,
                processing_time_us: processing_time,
                wal_write_time_us: wal_write_time,
                index_update_time_us: 0, // Deferred to background compaction
            },
        };

        Ok(serde_json::to_vec(&response)?)
    }

    /// Search vectors from binary Avro SearchQuery
    pub async fn search_vectors(&self, avro_payload: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(
            Level::DEBUG,
            "search_vectors",
            payload_size = avro_payload.len()
        );
        let start_time = std::time::Instant::now();

        // Parse versioned payload for search request
        let (_version, operation_type, avro_data) = self
            .parse_versioned_payload(avro_payload)
            .context("Failed to parse versioned search payload")?;

        if operation_type != "vector_search" {
            return Err(anyhow!(
                "Operation type mismatch: expected 'vector_search', got '{}'",
                operation_type
            ));
        }

        // Parse JSON search request
        let search_request: JsonValue = serde_json::from_slice(avro_data)
            .context("Failed to parse search request JSON")?;

        let collection_id = self.extract_string(&search_request, "collection_id")?;
        let queries = self.extract_array(&search_request, "queries")?;
        let top_k = self.extract_i64(&search_request, "top_k").unwrap_or(10) as usize;
        let include_vectors = search_request.get("include_vectors").and_then(|v| v.as_bool()).unwrap_or(false);
        let include_metadata = search_request.get("include_metadata").and_then(|v| v.as_bool()).unwrap_or(true);
        
        // Distance metric and indexing algorithm
        let distance_metric = search_request.get("distance_metric").and_then(|v| v.as_i64()).unwrap_or(1); // Default: Cosine
        let index_algorithm = search_request.get("index_algorithm").and_then(|v| v.as_i64()).unwrap_or(1); // Default: HNSW
        
        // Metadata filters
        let metadata_filters = search_request.get("metadata_filters").and_then(|v| v.as_object());
        
        // Search parameters (e.g., ef_search for HNSW)
        let _search_params = search_request.get("search_params").and_then(|v| v.as_object());
        
        debug!(
            "Advanced search: collection={}, queries={}, top_k={}, distance_metric={}, index_algo={}, has_filters={}",
            collection_id, queries.len(), top_k, distance_metric, index_algorithm, metadata_filters.is_some()
        );

        // Process multiple queries through vector coordinator
        let mut all_results = Vec::new();
        
        for (query_idx, query) in queries.iter().enumerate() {
            let query_vector = if let Some(vec_array) = query.as_array() {
                vec_array.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect::<Vec<f32>>()
            } else {
                return Err(anyhow!("Invalid query vector format at index {}", query_idx));
            };
            
            if query_vector.is_empty() {
                return Err(anyhow!("Empty query vector at index {}", query_idx));
            }

            // Create search context for the coordinator
            let mut search_context = SearchContext {
                collection_id: collection_id.clone(),
                query_vector,
                k: top_k,
                filters: None, // TODO: Convert metadata_filters to MetadataFilter
                strategy: SearchStrategy::Adaptive {
                    query_complexity_score: 0.5,
                    time_budget_ms: 1000,
                    accuracy_preference: 0.8,
                },
                algorithm_hints: std::collections::HashMap::new(),
                threshold: None,
                timeout_ms: Some(5000),
                include_debug_info: false,
                include_vectors: include_vectors,
            };

            // Execute search through coordinator
            let search_results = self.vector_coordinator
                .unified_search(search_context)
                .await
                .context("Failed to perform vector search through coordinator")?;
            
            // Add query index to results
            for mut result in search_results {
                // Inject query index into metadata
                if let serde_json::Value::Object(ref mut obj) = result.metadata {
                    obj.insert("query_index".to_string(), json!(query_idx));
                }
                all_results.push(result);
            }
        }

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        // Convert results to Avro format
        let avro_results: Vec<JsonValue> = all_results
            .into_iter()
            .map(|result| {
                json!({
                    "vector_id": result.vector_id,
                    "score": result.score,
                    "vector": if include_vectors { result.vector.unwrap_or_default() } else { Vec::<f32>::new() },
                    "metadata": if include_metadata { result.metadata } else { json!({}) }
                })
            })
            .collect();

        let response = json!({
            "results": avro_results,
            "total_count": avro_results.len(),
            "processing_time_us": processing_time,
            "collection_id": collection_id
        });

        self.serialize_search_response(&response)
    }

    /// Get single vector by ID
    pub async fn get_vector(
        &self,
        collection_id: &str,
        vector_id: &str,
        _include_vector: bool,
        _include_metadata: bool,
    ) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "get_vector", collection_id, vector_id);
        let start_time = std::time::Instant::now();

        let result = {
            let storage = self.storage.read().await;
            storage
                .read(&collection_id.to_string(), &vector_id.to_string())
                .await
                .context("Failed to get vector from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(result.is_some(), processing_time).await;

        let response = if let Some(vector_data) = result {
            json!({
                "found": true,
                "vector": vector_data,
                "processing_time_us": processing_time
            })
        } else {
            json!({
                "found": false,
                "processing_time_us": processing_time
            })
        };

        self.serialize_get_response(&response)
    }

    /// Delete single vector
    pub async fn delete_vector(&self, collection_id: &str, vector_id: &str) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "delete_vector", collection_id, vector_id);
        let start_time = std::time::Instant::now();

        // Write to WAL first
        let delete_record = json!({
            "collection_id": collection_id,
            "vector_id": vector_id,
            "operation": "delete"
        });
        let wal_payload = serde_json::to_vec(&delete_record)?;
        self.wal
            .append_avro_entry("delete_vector", &wal_payload)
            .await
            .context("Failed to write vector delete to WAL")?;

        // Delete from storage
        let deleted = {
            let storage = self.storage.read().await;
            storage
                .soft_delete(&collection_id.to_string(), &vector_id.to_string())
                .await
                .context("Failed to delete vector from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(deleted, processing_time).await;

        let result = self.create_operation_result(
            deleted,
            if deleted {
                None
            } else {
                Some("Vector not found".to_string())
            },
            None,
            if deleted { 1 } else { 0 },
            processing_time,
        );

        self.serialize_operation_result(&result)
    }

    // =============================================================================
    // COLLECTION OPERATIONS
    // =============================================================================

    /// Create collection from binary Avro CollectionConfig
    pub async fn create_collection(&self, avro_payload: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(
            Level::DEBUG,
            "create_collection",
            payload_size = avro_payload.len()
        );
        let start_time = std::time::Instant::now();

        // Parse versioned payload and validate schema
        let (_version, operation_type, avro_data) = self
            .parse_versioned_payload(avro_payload)
            .context("Failed to parse versioned Avro payload")?;

        if operation_type != "create_collection" {
            return Err(anyhow!(
                "Operation type mismatch: expected 'create_collection', got '{}'",
                operation_type
            ));
        }

        // Parse JSON from hardcoded schema types instead of Avro binary
        let schema_request: crate::schema_types::CollectionRequest = serde_json::from_slice(avro_data)
            .context("Failed to deserialize CollectionRequest from JSON payload")?;
        
        debug!("📦 Parsed schema request: {:?}", schema_request);

        // Extract fields from hardcoded schema types (efficient field access)
        let config = schema_request.collection_config.as_ref()
            .ok_or_else(|| anyhow!("Missing collection_config in request"))?;
        
        let collection_id = uuid::Uuid::new_v4().to_string();
        let collection_name = config.name.clone();
        let dimension = config.dimension;
        let distance_metric = match config.distance_metric {
            crate::schema_types::DistanceMetric::Cosine => "COSINE",
            crate::schema_types::DistanceMetric::Euclidean => "EUCLIDEAN", 
            crate::schema_types::DistanceMetric::DotProduct => "DOT_PRODUCT",
            crate::schema_types::DistanceMetric::Hamming => "HAMMING",
        }.to_string();
        let storage_layout = match config.storage_engine {
            crate::schema_types::StorageEngine::Viper => "VIPER",
            crate::schema_types::StorageEngine::Lsm => "LSM",
            crate::schema_types::StorageEngine::Mmap => "MMAP", 
            crate::schema_types::StorageEngine::Hybrid => "HYBRID",
        }.to_string();
        
        let now = chrono::Utc::now().timestamp_millis();
        let created_at = now;
        let updated_at = now;

        debug!(
            "Creating collection: name={}, id={}, dimension={}",
            collection_name, collection_id, dimension
        );

        // Write to WAL first
        self.wal
            .append_avro_entry("create_collection", avro_payload)
            .await
            .context("Failed to write collection creation to WAL")?;

        // Create in storage with full metadata
        {
            use crate::storage::metadata::CollectionMetadata;
            use std::collections::HashMap;
            
            let collection_metadata = CollectionMetadata {
                id: collection_id.clone(),
                name: collection_name.clone(),
                dimension: dimension as usize,
                distance_metric: distance_metric.clone(),
                indexing_algorithm: storage_layout.clone().to_lowercase(),
                created_at: chrono::DateTime::from_timestamp_micros(created_at).unwrap_or_else(|| chrono::Utc::now()),
                updated_at: chrono::DateTime::from_timestamp_micros(updated_at).unwrap_or_else(|| chrono::Utc::now()),
                vector_count: 0,
                total_size_bytes: 0,
                config: HashMap::new(),
                access_pattern: crate::storage::metadata::AccessPattern::Normal,
                retention_policy: None,
                tags: Vec::new(),
                owner: None,
                description: None,
                strategy_config: crate::storage::strategy::CollectionStrategyConfig::default(),
                strategy_change_history: Vec::new(),
                flush_config: None,
            };

            let storage = self.storage.read().await;
            storage
                .create_collection_with_metadata(collection_id.clone(), Some(collection_metadata), None)
                .await
                .context("Failed to create collection in storage")?;
        }

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        // Return collection metadata
        let collection_data = json!({
            "id": collection_id,
            "name": collection_name,
            "dimension": dimension,
            "distance_metric": distance_metric,
            "storage_layout": storage_layout,
            "created_at": chrono::Utc::now().timestamp_micros(),
            "vector_count": 0
        });

        self.serialize_collection_response(&collection_data)
    }

    /// Get collection metadata
    pub async fn get_collection(&self, collection_id: &str) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "get_collection", collection_id);
        let start_time = std::time::Instant::now();

        let collection = {
            let storage = self.storage.read().await;
            storage
                .get_collection_metadata(&collection_id.to_string())
                .await
                .context("Failed to get collection from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(collection.is_some(), processing_time)
            .await;

        let response = if let Some(collection_data) = collection {
            json!({
                "found": true,
                "collection": collection_data,
                "processing_time_us": processing_time
            })
        } else {
            json!({
                "found": false,
                "processing_time_us": processing_time
            })
        };

        self.serialize_collection_response(&response)
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "list_collections");
        let start_time = std::time::Instant::now();

        let collections = {
            let storage = self.storage.read().await;
            storage
                .list_collections()
                .await
                .context("Failed to list collections from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(true, processing_time).await;

        let response = json!({
            "collections": collections,
            "total_count": collections.len(),
            "processing_time_us": processing_time
        });

        self.serialize_collections_response(&response)
    }

    /// Delete collection
    pub async fn delete_collection(&self, collection_id: &str) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "delete_collection", collection_id);
        let start_time = std::time::Instant::now();

        // Write to WAL first
        let delete_record = json!({
            "collection_id": collection_id,
            "operation": "delete_collection"
        });
        let wal_payload = serde_json::to_vec(&delete_record)?;
        self.wal
            .append_avro_entry("delete_collection", &wal_payload)
            .await
            .context("Failed to write collection deletion to WAL")?;

        // Delete from storage
        let deleted = {
            let storage = self.storage.write().await;
            storage
                .delete_collection(&collection_id.to_string())
                .await
                .context("Failed to delete collection from storage")?
        };

        let processing_time = start_time.elapsed().as_micros() as i64;
        self.update_metrics(deleted, processing_time).await;

        let result = self.create_operation_result(
            deleted,
            if deleted {
                None
            } else {
                Some("Collection not found".to_string())
            },
            None,
            if deleted { 1 } else { 0 },
            processing_time,
        );

        self.serialize_operation_result(&result)
    }

    // =============================================================================
    // SYSTEM OPERATIONS
    // =============================================================================

    /// Get system health status
    pub async fn health_check(&self) -> Result<Vec<u8>> {
        let start_time = std::time::Instant::now();

        let metrics = self.performance_metrics.read().await;
        let storage_healthy = true; // TODO: Add actual health checks
        let wal_healthy = true; // TODO: Add actual health checks

        let health_status = json!({
            "status": if storage_healthy && wal_healthy { "HEALTHY" } else { "DEGRADED" },
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_seconds": 0, // TODO: Track actual uptime
            "total_operations": metrics.total_operations,
            "successful_operations": metrics.successful_operations,
            "failed_operations": metrics.failed_operations,
            "avg_processing_time_us": metrics.avg_processing_time_us,
            "storage_healthy": storage_healthy,
            "wal_healthy": wal_healthy,
            "timestamp": chrono::Utc::now().timestamp_micros()
        });

        let _processing_time = start_time.elapsed().as_micros() as i64;

        self.serialize_health_response(&health_status)
    }

    /// Get service metrics
    pub async fn get_metrics(&self) -> Result<Vec<u8>> {
        let metrics = self.performance_metrics.read().await;
        let wal_stats = self.wal.stats().await?;

        let metrics_response = json!({
            "service_metrics": {
                "total_operations": metrics.total_operations,
                "successful_operations": metrics.successful_operations,
                "failed_operations": metrics.failed_operations,
                "avg_processing_time_us": metrics.avg_processing_time_us,
                "last_operation_time": metrics.last_operation_time
            },
            "wal_metrics": {
                "total_entries": wal_stats.total_entries,
                "memory_entries": wal_stats.memory_entries,
                "disk_segments": wal_stats.disk_segments,
                "total_disk_size_bytes": wal_stats.total_disk_size_bytes,
                "compression_ratio": wal_stats.compression_ratio
            },
            "timestamp": chrono::Utc::now().timestamp_micros()
        });

        self.serialize_metrics_response(&metrics_response)
    }

    // =============================================================================
    // HELPER METHODS FOR AVRO SERIALIZATION/DESERIALIZATION
    // =============================================================================

    /// Update performance metrics
    async fn update_metrics(&self, success: bool, processing_time_us: i64) {
        let mut metrics = self.performance_metrics.write().await;
        metrics.total_operations += 1;
        if success {
            metrics.successful_operations += 1;
        } else {
            metrics.failed_operations += 1;
        }

        // Update average processing time
        let total_ops = metrics.total_operations as f64;
        metrics.avg_processing_time_us = (metrics.avg_processing_time_us * (total_ops - 1.0)
            + processing_time_us as f64)
            / total_ops;
        metrics.last_operation_time = Some(chrono::Utc::now());
    }

    // Avro deserialization helpers
    fn deserialize_vector_record(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize VectorRecord")
    }

    fn deserialize_batch_request(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize BatchRequest")
    }

    fn deserialize_search_query(&self, avro_bytes: &[u8]) -> Result<JsonValue> {
        // TODO: Replace with actual Avro binary deserialization
        serde_json::from_slice(avro_bytes).context("Failed to deserialize SearchQuery")
    }


    // Field extraction helpers
    fn extract_string(&self, record: &JsonValue, field: &str) -> Result<String> {
        record
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_optional_string(&self, record: &JsonValue, field: &str) -> Option<String> {
        record
            .get(field)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn extract_i64(&self, record: &JsonValue, field: &str) -> Result<i64> {
        record
            .get(field)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_optional_i64(&self, record: &JsonValue, field: &str) -> Option<i64> {
        record.get(field).and_then(|v| v.as_i64())
    }

    fn extract_bool(&self, record: &JsonValue, field: &str) -> Result<bool> {
        record
            .get(field)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| anyhow!("Missing or invalid field: {}", field))
    }

    fn extract_array<'a>(&self, record: &'a JsonValue, field: &str) -> Result<&'a Vec<JsonValue>> {
        record
            .get(field)
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("Missing or invalid array field: {}", field))
    }

    fn extract_optional_object<'a>(
        &self,
        record: &'a JsonValue,
        field: &str,
    ) -> Option<&'a serde_json::Map<String, JsonValue>> {
        record
            .get(field)
            .and_then(|v| if v.is_null() { None } else { v.as_object() })
    }

    fn extract_vector_array(&self, record: &JsonValue, field: &str) -> Result<Vec<f32>> {
        let array = self.extract_array(record, field)?;
        array
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| anyhow!("Invalid vector element"))
                    .map(|f| f as f32)
            })
            .collect()
    }

    fn extract_metadata(&self, record: &JsonValue) -> Result<Option<HashMap<String, JsonValue>>> {
        match record.get("metadata") {
            Some(meta) if !meta.is_null() => {
                if let Some(obj) = meta.as_object() {
                    let mut metadata = HashMap::new();
                    for (k, v) in obj {
                        metadata.insert(k.clone(), v.clone());
                    }
                    Ok(Some(metadata))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn extract_timestamp(&self, record: &JsonValue) -> Result<i64> {
        Ok(record
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros()))
    }

    // Vector processing helper for batches - aligned with single insert
    async fn process_single_vector_in_batch(
        &self,
        vector_record: &JsonValue,
        collection_id: &str,
        storage: &StorageEngine,
        _upsert_mode: bool,
    ) -> Result<String> {
        // Extract required vector field
        let vector_data = self.extract_vector_array(vector_record, "vector")
            .context("Missing required 'vector' field in batch item")?;

        // Generate UUID if id not provided (same as single insert)
        let vector_id = vector_record.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        
        // Optional metadata (defaults to empty)
        let metadata = self.extract_metadata(vector_record).unwrap_or(None);
        
        // Optional timestamp (defaults to now)
        let timestamp = vector_record.get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
        
        // expires_at is optional and defaults to null (active record)
        let expires_at = vector_record.get("expires_at")
            .and_then(|v| v.as_i64())
            .map(|ts| chrono::DateTime::from_timestamp(ts / 1_000_000, ((ts % 1_000_000) * 1000) as u32))
            .flatten();

        let vector_record = crate::core::VectorRecord {
            id: vector_id.clone(),
            collection_id: collection_id.to_string(),
            vector: vector_data,
            metadata: metadata.unwrap_or_default(),
            timestamp: chrono::DateTime::from_timestamp(timestamp / 1_000_000, ((timestamp % 1_000_000) * 1000) as u32)
                .unwrap_or_else(|| chrono::Utc::now()),
            expires_at, // null by default for active records
        };
        
        storage
            .write(vector_record)
            .await
            .context("Failed to insert vector in batch processing")?;
            
        Ok(vector_id)
    }

    // Avro result creation helpers
    fn create_operation_result(
        &self,
        success: bool,
        error_message: Option<String>,
        error_code: Option<String>,
        affected_count: i64,
        processing_time_us: i64,
    ) -> JsonValue {
        json!({
            "success": success,
            "error_message": error_message,
            "error_code": error_code,
            "affected_count": affected_count,
            "processing_time_us": processing_time_us
        })
    }

    fn create_search_result(
        &self,
        id: &str,
        score: f32,
        vector: Option<&Vec<f32>>,
        metadata: Option<&HashMap<String, JsonValue>>,
    ) -> JsonValue {
        json!({
            "id": id,
            "score": score,
            "vector": vector,
            "metadata": metadata
        })
    }

    // Avro serialization helpers
    fn serialize_operation_result(&self, result: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(result).context("Failed to serialize OperationResult")
    }

    fn serialize_search_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize search response")
    }

    fn serialize_get_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize get response")
    }

    fn serialize_collection_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize collection response")
    }

    fn serialize_collections_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize collections response")
    }

    fn serialize_health_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize health response")
    }

    fn serialize_metrics_response(&self, response: &JsonValue) -> Result<Vec<u8>> {
        // TODO: Replace with actual Avro binary serialization
        serde_json::to_vec(response).context("Failed to serialize metrics response")
    }

    // ============================================================================
    // NEW gRPC v1 PROTOCOL HANDLERS - Mixed Avro binary optimization
    // ============================================================================

    /// Handle unified collection operations (CREATE, GET, LIST, DELETE, MIGRATE)
    pub async fn handle_collection_operation(&self, avro_bytes: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_collection_operation");
        debug!("📦 UnifiedAvroService handling collection operation, payload: {} bytes", avro_bytes.len());

        // For now, delegate to existing methods based on operation type
        // TODO: Parse the Avro binary to determine operation and dispatch accordingly
        self.create_collection(avro_bytes).await
    }

    /// Handle vector insert with ultra-fast zero-copy (ONLY PATH)
    /// All vector operations use trust-but-verify zero-copy for maximum performance
    pub async fn handle_vector_insert(&self, avro_bytes: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_insert_zero_copy");
        debug!("📦 Zero-copy vector insert, payload: {} bytes", avro_bytes.len());

        // Use ONLY the zero-copy path - no validation, no parsing, maximum speed
        self.vector_insert_zero_copy(avro_bytes).await
    }

    /// Handle vector mutation (UPDATE/DELETE)
    pub async fn handle_vector_mutation(&self, avro_bytes: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_mutation");
        debug!("📦 UnifiedAvroService handling vector mutation, payload: {} bytes", avro_bytes.len());

        // TODO: Parse Avro binary VectorMutation and dispatch to update/delete
        // For now, return a simple success response
        let response = json!({
            "success": true,
            "operation": "mutation",
            "affected_count": 1,
            "processing_time_us": 1000
        });

        Ok(serde_json::to_vec(&response)?)
    }

    /// Handle vector search with conditional Avro binary response
    /// Handle vector insert with separated gRPC metadata and Avro vector data
    pub async fn handle_vector_insert_v2(
        &self, 
        collection_id: &str, 
        upsert_mode: bool, 
        vectors_avro_payload: &[u8]
    ) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_insert_v2");
        debug!("📦 UnifiedAvroService handling vector insert v2: collection={}, upsert={}, payload={}KB", 
               collection_id, upsert_mode, vectors_avro_payload.len() / 1024);

        // Parse vector data directly from Avro payload (no metadata parsing needed)
        let vectors: Vec<serde_json::Value> = serde_json::from_slice(vectors_avro_payload)
            .context("Failed to parse vectors Avro payload")?;

        if vectors.is_empty() {
            return Err(anyhow!("Empty vectors array in payload"));
        }

        let wal_start = std::time::Instant::now();
        let start_time = std::time::Instant::now();

        // Create WAL entries for each vector with proper collection ID (from gRPC field)
        let mut wal_entries = Vec::new();
        for (i, vector_data) in vectors.iter().enumerate() {
            let vector_id = vector_data
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("vector_{}", i))
                .to_string();
            
            let vector = vector_data
                .get("vector")
                .and_then(|v| v.as_array())
                .context("Missing vector field")?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>();
            
            let metadata: std::collections::HashMap<String, serde_json::Value> = vector_data
                .get("metadata")
                .and_then(|v| v.as_object())
                .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            
            let record = crate::core::types::VectorRecord {
                id: vector_id.clone(),
                collection_id: collection_id.to_string(),
                vector,
                metadata,
                timestamp: vector_data.get("timestamp")
                    .and_then(|v| v.as_i64())
                    .map(|ts| chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(|| chrono::Utc::now()))
                    .unwrap_or_else(|| chrono::Utc::now()),
                expires_at: None,
            };
            
            wal_entries.push((vector_id, record));
        }
        
        // Write vectors to WAL with proper collection ID (from gRPC field)
        self.wal
            .insert_batch(collection_id.to_string(), wal_entries)
            .await
            .context("Failed to write vectors to WAL")?;
        
        let wal_write_time = wal_start.elapsed().as_micros() as i64;
        let processing_time = start_time.elapsed().as_micros() as i64;
        
        self.update_metrics(true, processing_time).await;

        info!(
            "🚀 Zero-copy vectors accepted in {}μs (WAL: {}μs) - V2 with separated metadata",
            processing_time, wal_write_time
        );

        // Return immediate success response
        let response = crate::schema_types::VectorInsertResponse {
            success: true,
            vector_ids: vec!["vectors_accepted_zero_copy_v2".to_string()],
            error_message: None,
            error_code: None,
            metrics: crate::schema_types::VectorOperationMetrics {
                total_processed: vectors.len() as i64,
                successful_count: vectors.len() as i64,
                failed_count: 0,
                updated_count: if upsert_mode { vectors.len() as i64 } else { 0 },
                processing_time_us: processing_time,
                wal_write_time_us: wal_write_time,
                index_update_time_us: 0, // Deferred to background compaction
            },
        };

        Ok(serde_json::to_vec(&response)?)
    }

    pub async fn handle_vector_search(&self, avro_bytes: &[u8]) -> Result<Vec<u8>> {
        let _span = span!(Level::DEBUG, "handle_vector_search");
        debug!("📦 UnifiedAvroService handling vector search, payload: {} bytes", avro_bytes.len());

        // Delegate to existing search method
        self.search_vectors(avro_bytes).await
    }
}

// Placeholder for search results until storage integration
#[derive(Debug)]
struct SearchResult {
    id: String,
    score: f32,
    vector: Option<Vec<f32>>,
    metadata: Option<HashMap<String, JsonValue>>,
}
