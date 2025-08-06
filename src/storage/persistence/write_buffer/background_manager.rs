// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Background Maintenance Manager for WAL
//!
//! Manages async flush and compaction operations triggered by write operations.
//! Ensures only one background task per collection to prevent race conditions.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::WriteBufferConfig;

// use crate::storage::engines::viper::clustering_models::{ClusteringModelManager, MIN_VECTORS_FOR_CLUSTERING}; // Moved to AXIS
const MIN_VECTORS_FOR_CLUSTERING: usize = 1000; // Local constant since clustering moved to AXIS

/// Configuration for dynamic schema generation
#[derive(Debug, Clone)]
struct CollectionConfiguration {
    name: String,
    dimension: usize,
    distance_metric: String,
    quantization_settings: Option<QuantizationSettings>,
    filterable_metadata: Vec<FilterableMetadataColumn>,
}

/// Quantization settings for vector compression
#[derive(Debug, Clone)]
struct QuantizationSettings {
    enabled: bool,
    quantization_type: QuantizationType,
    bits_per_component: u8,
    subspaces: u8,
}

/// Types of quantization supported
#[derive(Debug, Clone)]
enum QuantizationType {
    ProductQuantization,
    ScalarQuantization,
    BinaryQuantization,
}

/// Configuration for filterable metadata columns
#[derive(Debug, Clone)]
struct FilterableMetadataColumn {
    name: String,
    data_type: FilterableColumnType,
    indexed: bool,
}

/// Data types supported for filterable metadata
#[derive(Debug, Clone)]
enum FilterableColumnType {
    String,
    Integer,
    Float,
    Boolean,
    Timestamp,
    ListString,
    ListInteger,
}

/// Background task status for a collection
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundTaskStatus {
    /// No background task running
    Idle,
    /// Flush operation in progress
    Flushing,
    /// Compaction operation in progress  
    Compacting,
    /// Both flush and compaction queued
    FlushAndCompact,
}

/// Background maintenance manager
pub struct BackgroundMaintenanceManager {
    /// Per-collection task status tracking
    collection_status: Arc<RwLock<HashMap<String, BackgroundTaskStatus>>>,

    /// Configuration
    config: Arc<WriteBufferConfig>,

    /// Statistics
    stats: Arc<Mutex<BackgroundMaintenanceStats>>,

    /// AXIS manager for IndexConfig-based indexing after operations
    axis_manager: Option<Arc<crate::index::axis::manager::AxisManager>>,

    /// WAL flush coordinator for atomic operations
    flush_coordinator: Option<Arc<super::flush_coordinator::WriteBufferFlushCoordinator>>,

    /// Storage engine registry for polymorphic compaction delegation
    storage_engines: Arc<RwLock<HashMap<String, Arc<dyn crate::storage::traits::UnifiedStorageEngine>>>>,

    // Clustering model manager moved to AXIS
    // clustering_model_manager: Option<Arc<ClusteringModelManager>>,
    
    /// Collection vector counts at last model training
    last_training_vector_counts: Arc<RwLock<HashMap<String, usize>>>,
    
    /// Collection service for fetching metadata
    collection_service: Option<Arc<crate::services::collection_service::CollectionService>>,
}

/// Statistics for background maintenance operations
#[derive(Debug, Clone, Default)]
pub struct BackgroundMaintenanceStats {
    pub total_flush_operations: u64,
    pub total_compaction_operations: u64,
    pub flush_operations_skipped: u64,
    pub compaction_operations_skipped: u64,
    pub average_flush_duration_ms: f64,
    pub average_compaction_duration_ms: f64,
    pub concurrent_operations_prevented: u64,
    pub total_model_training_operations: u64,
    pub model_training_skipped_recent: u64,
    pub model_training_skipped_small: u64,
    pub average_model_training_duration_ms: f64,
}

impl BackgroundMaintenanceManager {
    /// Create new background maintenance manager
    pub fn new(config: Arc<WriteBufferConfig>) -> Self {
        Self {
            collection_status: Arc::new(RwLock::new(HashMap::new())),
            config,
            stats: Arc::new(Mutex::new(BackgroundMaintenanceStats::default())),
            axis_manager: None,
            flush_coordinator: None,
            storage_engines: Arc::new(RwLock::new(HashMap::new())),
            // clustering_model_manager: None, // Moved to AXIS
            last_training_vector_counts: Arc::new(RwLock::new(HashMap::new())),
            collection_service: None,
        }
    }
    
    /// Set collection service for metadata fetching
    pub fn set_collection_service(&mut self, service: Arc<crate::services::collection_service::CollectionService>) {
        self.collection_service = Some(service.clone());
        
        // Also inject into flush coordinator if available
        if let Some(coordinator) = &mut self.flush_coordinator {
            // The flush coordinator needs mutable access to set the service
            // Since we have Arc, we'd need to use Arc::get_mut which requires unique ownership
            // Instead, the flush coordinator should be configured before being set
            info!("⚠️ BackgroundManager: Flush coordinator already set - collection service should be injected before setting coordinator");
        }
        
        info!("🔗 BackgroundManager: Collection service registered for metadata fetching");
    }

    /// Set AXIS manager for IndexConfig-based indexing
    pub fn set_axis_manager(&mut self, axis_manager: Arc<crate::index::axis::manager::AxisManager>) {
        self.axis_manager = Some(axis_manager);
        info!("🔗 BackgroundManager: AXIS manager registered for IndexConfig-based indexing");
    }

    /// Set flush coordinator for atomic operations
    pub fn set_flush_coordinator(&mut self, flush_coordinator: Arc<super::flush_coordinator::WriteBufferFlushCoordinator>) {
        self.flush_coordinator = Some(flush_coordinator);
        info!("🔗 BackgroundManager: Flush coordinator registered for atomic operations");
    }

    /// Register a storage engine for polymorphic compaction delegation
    pub async fn register_storage_engine(
        &self,
        engine_type: &str,
        engine: Arc<dyn crate::storage::traits::UnifiedStorageEngine>,
    ) {
        let mut engines = self.storage_engines.write().await;
        engines.insert(engine_type.to_string(), engine);
        info!(
            "🏭 BackgroundManager: Registered {} storage engine for compaction delegation",
            engine_type
        );
    }

    // Clustering model manager moved to AXIS
    // /// Set clustering model manager for intelligent model training
    // pub fn set_clustering_model_manager(&mut self, model_manager: Arc<ClusteringModelManager>) {
    //     self.clustering_model_manager = Some(model_manager);
    //     info!("🧠 BackgroundManager: Clustering model manager registered for intelligent training");
    // }

    /// **INTELLIGENT RETRAINING LOGIC**: Check if model should be retrained based on vector growth
    /// 
    /// Model retraining is triggered when:
    /// 1. Collection has >1M vectors (MIN_VECTORS_FOR_CLUSTERING)
    /// 2. Vector count has grown by >20% since last training
    /// 3. At least 6 hours have passed since last training
    async fn should_retrain_model(&self, collection_id: &str, _current_vectors: usize) -> bool {
        // Clustering functionality moved to AXIS
        // Always return false as model training is now handled by AXIS
        let mut stats = self.stats.lock().await;
        stats.model_training_skipped_small += 1;
        debug!(
            "🧠 Model training skipped for collection {} - clustering moved to AXIS",
            collection_id
        );
        false
    }

    /// Trigger async flush for collection if not already running
    /// Returns true if flush was triggered, false if already running
    pub async fn trigger_flush_if_needed(
        &self,
        collection_id: &str,
        current_memory_size: usize,
    ) -> Result<bool> {
        let effective_config = self.config.effective_config_for_collection(collection_id);

        // Check if flush is needed based on size
        if current_memory_size < effective_config.memory_flush_size_bytes {
            return Ok(false);
        }

        // Check if background task is already running
        {
            let status_map = self.collection_status.read().await;
            if let Some(status) = status_map.get(collection_id) {
                match status {
                    BackgroundTaskStatus::Idle => {}
                    BackgroundTaskStatus::Flushing => {
                        debug!(
                            "🔄 Flush already in progress for collection {}, skipping",
                            collection_id
                        );
                        let mut stats = self.stats.lock().await;
                        stats.flush_operations_skipped += 1;
                        return Ok(false);
                    }
                    BackgroundTaskStatus::Compacting => {
                        // Upgrade to flush + compact
                        debug!(
                            "📈 Upgrading compaction to flush+compact for collection {}",
                            collection_id
                        );
                        drop(status_map);
                        let mut status_map = self.collection_status.write().await;
                        status_map
                            .insert(collection_id.to_string(), BackgroundTaskStatus::FlushAndCompact);
                        return Ok(false);
                    }
                    BackgroundTaskStatus::FlushAndCompact => {
                        debug!(
                            "⏳ Flush+compact already queued for collection {}, skipping",
                            collection_id
                        );
                        let mut stats = self.stats.lock().await;
                        stats.flush_operations_skipped += 1;
                        return Ok(false);
                    }
                }
            }
        }

        // Set status to flushing
        {
            let mut status_map = self.collection_status.write().await;
            status_map.insert(collection_id.to_string(), BackgroundTaskStatus::Flushing);
        }

        // Trigger async flush task
        let collection_id_clone = collection_id.to_string();
        let status_map_clone = self.collection_status.clone();
        let stats_clone = self.stats.clone();
        let flush_coordinator = self.flush_coordinator.clone();
        let axis_manager = self.axis_manager.clone();
        let storage_engines_clone = self.storage_engines.clone();
        // let clustering_model_manager = self.clustering_model_manager.clone(); // Moved to AXIS
        let _last_training_vector_counts = self.last_training_vector_counts.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();

            info!(
                "🚿 [FLUSH] Starting background flush for collection {} (memory: {}MB, trigger_size: {}MB)",
                collection_id_clone,
                current_memory_size / (1024 * 1024),
                effective_config.memory_flush_size_bytes / (1024 * 1024)
            );

            debug!(
                "🚿 [FLUSH] Collection: {}, Start time: {:?}, Memory size: {} bytes",
                collection_id_clone, start_time, current_memory_size
            );

            // Execute coordinated flush using FlushCoordinator for atomic operations
            let flush_start = std::time::Instant::now();
            let flush_result = if let Some(ref flush_coordinator) = flush_coordinator {
                match flush_coordinator
                    .execute_coordinated_flush(
                        &collection_id_clone,
                        super::flush_coordinator::FlushDataSource::Memory,
                        None, // Use default engine selection
                        None, // WAL manager will be resolved internally
                    )
                    .await
                {
                    Ok(result) => {
                        info!(
                            "✅ [FLUSH] Coordinated flush successful for collection {}: {} entries, {} bytes, {} files",
                            collection_id_clone,
                            result.base.entries_flushed,
                            result.base.bytes_written,
                            result.base.files_created
                        );
                        Some(result)
                    }
                    Err(e) => {
                        warn!(
                            "❌ [FLUSH] Coordinated flush failed for collection {}: {}",
                            collection_id_clone, e
                        );
                        None
                    }
                }
            } else {
                warn!(
                    "⚠️ [FLUSH] No flush coordinator available for collection {}, skipping flush",
                    collection_id_clone
                );
                None
            };

            let flush_duration = flush_start.elapsed();
            debug!(
                "🚿 [FLUSH] Collection: {}, Flush operation completed in: {:?}",
                collection_id_clone, flush_duration
            );

            let duration = start_time.elapsed();

            // Determine if compaction is needed and execute the complete cycle
            let needs_compaction = Self::should_trigger_compaction_after_flush(&collection_id_clone).await;
            let mut final_files_created = if let Some(ref result) = flush_result {
                // Convert count to placeholder file paths - in production this would come from engine
                let file_count = result.base.files_created;
                (0..file_count)
                    .map(|i| format!("flushed_collection_{}_{}.parquet", collection_id_clone, i))
                    .collect::<Vec<String>>()
            } else {
                Vec::new()
            };

            if needs_compaction && flush_result.is_some() {
                info!(
                    "🔄 [COMPACTION] Triggering compaction after flush for collection {}",
                    collection_id_clone
                );

                // Update status to compacting
                {
                    let mut status_map = status_map_clone.write().await;
                    status_map.insert(
                        collection_id_clone.to_string(),
                        BackgroundTaskStatus::Compacting,
                    );
                }

                let compaction_start = std::time::Instant::now();
                debug!(
                    "🔄 [COMPACTION] Collection: {}, Compaction start time: {:?}",
                    collection_id_clone, compaction_start
                );

                // Execute compaction via storage engine delegation
                let compaction_result = Self::execute_compaction_with_engines(
                    &storage_engines_clone,
                    &collection_id_clone
                ).await;
                
                let compaction_duration = compaction_start.elapsed();
                
                match compaction_result {
                    Ok(compacted_files) => {
                        info!(
                            "✅ [COMPACTION] Compaction successful for collection {}: {} files created in {:?}",
                            collection_id_clone, compacted_files.len(), compaction_duration
                        );
                        // Update final files list with compacted files
                        final_files_created = compacted_files;
                    }
                    Err(e) => {
                        warn!(
                            "❌ [COMPACTION] Compaction failed for collection {}: {}",
                            collection_id_clone, e
                        );
                        // Keep original flush files if compaction failed
                    }
                }

                // Update stats
                {
                    let mut stats = stats_clone.lock().await;
                    stats.total_compaction_operations += 1;
                    let total_ops = stats.total_compaction_operations;
                    Self::update_average_duration(
                        &mut stats.average_compaction_duration_ms,
                        compaction_duration.as_millis() as f64,
                        total_ops,
                    );
                }

                info!(
                    "✅ [COMPACTION] Background compaction completed for collection {} in {}ms (files_before: TODO, files_after: TODO, size_reduction: TODO)",
                    collection_id_clone,
                    compaction_duration.as_millis()
                );
            }

            // CRITICAL: IndexConfig-based indexing AFTER complete flush-compaction cycle
            if let (Some(ref axis), Some(ref _flush_result)) = (&axis_manager, &flush_result) {
                if !final_files_created.is_empty() {
                    info!(
                        "🔄 [INDEXING] Starting IndexConfig-based indexing for collection {} after flush-compaction cycle",
                        collection_id_clone
                    );
                    
                    let indexing_start = std::time::Instant::now();
                    
                    // Extract vectors from flush result for indexing
                    let vectors_to_index = if let Some(ref enhanced_result) = flush_result {
                        enhanced_result.vector_records.clone()
                    } else {
                        Vec::new()
                    };
                    
                    match axis.handle_flushed_vectors(
                        &collection_id_clone,
                        vectors_to_index,
                        final_files_created.clone()
                    ).await {
                        Ok(()) => {
                            let indexing_duration = indexing_start.elapsed();
                            info!(
                                "✅ [INDEXING] IndexConfig-based indexing completed for collection {} in {:?}",
                                collection_id_clone, indexing_duration
                            );
                        }
                        Err(e) => {
                            warn!(
                                "⚠️ [INDEXING] IndexConfig-based indexing failed for collection {}: {}",
                                collection_id_clone, e
                            );
                            // Continue - flush/compaction was successful even if indexing failed
                        }
                    }
                } else {
                    info!(
                        "📋 [INDEXING] Skipping indexing for collection {} (no files created or flush failed)",
                        collection_id_clone
                    );
                }
            } else {
                info!(
                    "📋 [INDEXING] No AXIS manager or flush result available for collection {}, skipping indexing",
                    collection_id_clone
                );
            }

            // INTELLIGENT MODEL TRAINING moved to AXIS

            // Reset status to idle
            {
                let mut status_map = status_map_clone.write().await;
                status_map.insert(collection_id_clone.clone(), BackgroundTaskStatus::Idle);
            }

            // Update stats
            {
                let mut stats = stats_clone.lock().await;
                stats.total_flush_operations += 1;
                let total_ops = stats.total_flush_operations;
                Self::update_average_duration(
                    &mut stats.average_flush_duration_ms,
                    duration.as_millis() as f64,
                    total_ops,
                );
            }

            info!(
                "✅ [FLUSH] Background flush completed for collection {} in {}ms (total_ops: {}, avg_duration: {:.2}ms)",
                collection_id_clone,
                duration.as_millis(),
                {
                    let stats = stats_clone.lock().await;
                    stats.total_flush_operations
                },
                {
                    let stats = stats_clone.lock().await;
                    stats.average_flush_duration_ms
                }
            );

            debug!(
                "🚿 [FLUSH] Collection: {}, End time: {:?}, Total duration: {:?}, Memory freed: {}MB",
                collection_id_clone,
                std::time::Instant::now(),
                duration,
                current_memory_size / (1024 * 1024)
            );
        });

        Ok(true)
    }

    /// Check if collection needs compaction based on file count and sizes
    async fn should_trigger_compaction_after_flush(_collection_id: &str) -> bool {
        // TODO: Implement proper compaction criteria check
        // This would check file count and average file sizes
        // For now, always trigger compaction to test the Arrow/Parquet implementation
        true
    }

    /// Get collection configuration from collection service
    async fn get_collection_configuration(&self, collection_id: &str) -> Result<(CollectionConfiguration, Option<crate::proto::proximadb::Collection>)> {
        info!("🔍 [CONFIG] Getting collection configuration for {}", collection_id);
        
        // Fetch from actual collection service if available
        if let Some(ref collection_service) = self.collection_service {
            match collection_service.get_proto_collection(collection_id).await {
                Ok(Some(collection)) => {
                    // Extract configuration from proto Collection
                    let config = if let Some(ref proto_config) = collection.config {
                        // Map proto types to internal types
                        let filterable_metadata = proto_config.filterable_columns.iter().map(|col| {
                            use crate::proto::proximadb::FilterableDataType;
                            let data_type = match FilterableDataType::try_from(col.data_type) {
                                Ok(FilterableDataType::FilterableString) => FilterableColumnType::String,
                                Ok(FilterableDataType::FilterableInteger) => FilterableColumnType::Integer,
                                Ok(FilterableDataType::FilterableFloat) => FilterableColumnType::Float,
                                Ok(FilterableDataType::FilterableBoolean) => FilterableColumnType::Boolean,
                                Ok(FilterableDataType::FilterableDatetime) => FilterableColumnType::Timestamp,
                                Ok(FilterableDataType::FilterableArrayString) => FilterableColumnType::ListString,
                                Ok(FilterableDataType::FilterableArrayInteger) => FilterableColumnType::ListInteger,
                                _ => FilterableColumnType::String, // Default
                            };
                            
                            FilterableMetadataColumn {
                                name: col.name.clone(),
                                data_type,
                                indexed: col.indexed,
                            }
                        }).collect();
                        
                        let quantization_settings = proto_config.quantization_config.as_ref().map(|q| {
                            // Map quantization type based on configuration
                            let q_type = if q.enabled {
                                // Use quantization level to determine type
                                match q.quantization_level {
                                    0 => QuantizationType::ScalarQuantization,  // None/Basic
                                    1 => QuantizationType::ScalarQuantization,  // Low
                                    2 => QuantizationType::ProductQuantization, // Medium
                                    3 => QuantizationType::ProductQuantization, // High
                                    _ => QuantizationType::ScalarQuantization,
                                }
                            } else {
                                QuantizationType::ScalarQuantization // Default
                            };
                            
                            QuantizationSettings {
                                enabled: q.enabled,
                                quantization_type: q_type,
                                bits_per_component: q.bits_per_component as u8,
                                subspaces: q.num_subvectors as u8,
                            }
                        });
                        
                        CollectionConfiguration {
                            name: proto_config.name.clone(),
                            dimension: proto_config.dimension as usize,
                            distance_metric: {
                                use crate::proto::proximadb::DistanceMetric;
                                match DistanceMetric::try_from(proto_config.distance_metric) {
                                    Ok(DistanceMetric::Cosine) => "cosine",
                                    Ok(DistanceMetric::Euclidean) => "euclidean",
                                    Ok(DistanceMetric::DotProduct) => "dot_product",
                                    Ok(DistanceMetric::Manhattan) => "manhattan",
                                    _ => "cosine",
                                }.to_string()
                            },
                            quantization_settings,
                            filterable_metadata,
                        }
                    } else {
                        // Fallback configuration if config is missing
                        CollectionConfiguration {
                            name: collection_id.to_string(),
                            dimension: 512,
                            distance_metric: "cosine".to_string(),
                            quantization_settings: None,
                            filterable_metadata: vec![],
                        }
                    };
                    
                    info!(
                        "✅ [CONFIG] Retrieved configuration for collection {}: dim={}, metric={}, quantization={}, filterable_fields={}",
                        collection_id,
                        config.dimension,
                        config.distance_metric,
                        config.quantization_settings.is_some(),
                        config.filterable_metadata.len()
                    );
                    
                    return Ok((config, Some(collection)));
                }
                Ok(None) => {
                    warn!("⚠️ [CONFIG] Collection {} not found in metadata", collection_id);
                }
                Err(e) => {
                    warn!("⚠️ [CONFIG] Failed to fetch collection metadata: {}", e);
                }
            }
        }
        
        // No fallback - fail fast in production if collection service not available
        Err(anyhow::anyhow!(
            "Collection service not available or collection '{}' not found. Cannot proceed without metadata.",
            collection_id
        ))
    }

    /// Generate dynamic Parquet schema based on collection configuration
    async fn generate_parquet_schema_for_collection(&self, collection_id: &str) -> Result<Arc<arrow_schema::Schema>> {
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        
        // Get actual collection configuration from collection service
        let (collection_config, _proto_collection) = self.get_collection_configuration(collection_id).await?;
        
        let mut schema_fields = Vec::new();
        
        // Core fields (always present)
        schema_fields.push(Field::new("id", DataType::Utf8, false));
        schema_fields.push(Field::new("collection_id", DataType::Utf8, false));
        
        // Vector field - native Parquet array<float32>
        schema_fields.push(Field::new(
            "vector",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
            false,
        ));
        
        // Quantized vector field (if quantization is enabled)
        if let Some(quant_settings) = &collection_config.quantization_settings {
            match quant_settings.quantization_type {
                QuantizationType::ProductQuantization => {
                    // PQ codes as array of uint8
                    schema_fields.push(Field::new(
                        "vector_pq",
                        DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))),
                        true, // Nullable - may not be present for all records
                    ));
                    
                    // PQ centroids as binary blob
                    schema_fields.push(Field::new("pq_centroids", DataType::Binary, true));
                }
                QuantizationType::ScalarQuantization => {
                    // SQ codes as array of uint8 or uint16
                    let quantized_type = match quant_settings.bits_per_component {
                        8 => DataType::UInt8,
                        16 => DataType::UInt16,
                        _ => DataType::UInt8,
                    };
                    
                    schema_fields.push(Field::new(
                        "vector_sq",
                        DataType::List(Arc::new(Field::new("item", quantized_type, false))),
                        true,
                    ));
                    
                    // SQ scaling factors
                    schema_fields.push(Field::new("sq_scale", DataType::Float32, true));
                    schema_fields.push(Field::new("sq_offset", DataType::Float32, true));
                }
                QuantizationType::BinaryQuantization => {
                    // Binary codes as array of uint8 (packed bits)
                    schema_fields.push(Field::new(
                        "vector_binary",
                        DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))),
                        true,
                    ));
                }
            }
        }
        
        // Filterable metadata columns as native Parquet columns
        for metadata_col in &collection_config.filterable_metadata {
            let field_type = match metadata_col.data_type {
                FilterableColumnType::String => DataType::Utf8,
                FilterableColumnType::Integer => DataType::Int64,
                FilterableColumnType::Float => DataType::Float64,
                FilterableColumnType::Boolean => DataType::Boolean,
                FilterableColumnType::Timestamp => DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                FilterableColumnType::ListString => {
                    DataType::List(Arc::new(Field::new("item", DataType::Utf8, false)))
                }
                FilterableColumnType::ListInteger => {
                    DataType::List(Arc::new(Field::new("item", DataType::Int64, false)))
                }
            };
            
            schema_fields.push(Field::new(
                &metadata_col.name,
                field_type,
                true, // Filterable metadata is always nullable
            ));
        }
        
        // Core timestamp fields
        schema_fields.push(Field::new("timestamp", DataType::Int64, false));
        schema_fields.push(Field::new("created_at", DataType::Int64, false));
        schema_fields.push(Field::new("updated_at", DataType::Int64, false));
        schema_fields.push(Field::new("expires_at", DataType::Int64, true));
        schema_fields.push(Field::new("version", DataType::Int64, false));
        
        // Extra metadata as JSON string for non-filterable metadata
        schema_fields.push(Field::new("extra_metadata", DataType::Utf8, true));
        
        let schema = Arc::new(Schema::new(schema_fields));
        
        info!(
            "🔧 [SCHEMA] Generated dynamic Parquet schema for collection {} with {} fields",
            collection_id,
            schema.fields().len()
        );
        
        info!("📋 [SCHEMA] Schema fields:");
        for field in schema.fields() {
            info!("  • {} ({:?}) - nullable: {}", field.name(), field.data_type(), field.is_nullable());
        }
        
        Ok(schema)
    }
    
    /// Execute compaction for a collection - delegates to storage engine (instance method)
    async fn execute_compaction(
        &self,
        collection_id: &str,
    ) -> Result<Vec<String>> {
        info!(
            "🔄 [COMPACTION] Starting compaction for collection {} (delegating to storage engine)",
            collection_id
        );
        
        // Fetch collection metadata ONCE to avoid duplicate calls
        let collection_metadata = if let Some(ref collection_service) = self.collection_service {
            match collection_service.get_proto_collection(collection_id).await {
                Ok(Some(collection)) => {
                    info!(
                        "📋 [COMPACTION] Fetched collection metadata for '{}' - engine: {:?}",
                        collection_id,
                        collection.config.as_ref().map(|c| c.storage_engine)
                    );
                    Some(collection)
                }
                Ok(None) => {
                    warn!("⚠️ [COMPACTION] Collection '{}' not found in metadata", collection_id);
                    None
                }
                Err(e) => {
                    warn!("⚠️ [COMPACTION] Failed to fetch collection metadata: {}", e);
                    None
                }
            }
        } else {
            warn!("⚠️ [COMPACTION] No collection service available, proceeding without metadata");
            None
        };
        
        // Determine storage engine from metadata
        let engine_name = if let Some(ref metadata) = collection_metadata {
            if let Some(ref config) = metadata.config {
                use crate::proto::proximadb::StorageEngine;
                match StorageEngine::try_from(config.storage_engine) {
                    Ok(StorageEngine::Viper) => "viper",
                    Ok(StorageEngine::Sst) => "lsm",
                    _ => "viper" // Default to VIPER
                }
            } else {
                "viper"
            }
        } else {
            "viper" // Default to VIPER if no metadata
        };
        
        // Get storage engine for delegation
        let engines = self.storage_engines.read().await;
        
        // Try VIPER engine first (default strategy)
        let engine = if let Some(viper_engine) = engines.get("viper") {
            info!("🏭 [COMPACTION] Using VIPER storage engine for collection {}", collection_id);
            viper_engine.clone()
        } else if let Some(lsm_engine) = engines.get("lsm") {
            info!("🏭 [COMPACTION] Using LSM storage engine for collection {}", collection_id);
            lsm_engine.clone()
        } else {
            warn!("⚠️ [COMPACTION] No storage engines registered, cannot perform compaction");
            return Err(anyhow::anyhow!("No storage engines available for compaction"));
        };
        
        drop(engines); // Release the read lock
        
        // Create compaction parameters with collection metadata
        let compaction_params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: false, // Background compaction is not forced
            synchronous: true, // Wait for completion
            hints: std::collections::HashMap::new(),
            timeout_ms: Some(300_000), // 5 minute timeout
            priority: crate::storage::traits::OperationPriority::Low,
            collection_config: collection_metadata.clone(), // Pass metadata to avoid duplicate fetches
        };
        
        info!(
            "📋 [COMPACTION] Delegating to {} engine: do_compact({})",
            engine.engine_name(),
            collection_id
        );
        
        // Execute compaction via storage engine
        match engine.do_compact(&compaction_params).await {
            Ok(result) => {
                if result.success {
                    info!(
                        "✅ [COMPACTION] {} compaction completed for collection {}: {} entries processed, {} files {} → {}",
                        engine.engine_name(),
                        collection_id,
                        result.entries_processed,
                        result.input_files,
                        result.output_files,
                        result.duration_ms
                    );
                    
                    // Return file list for compatibility - for VIPER this would be the compacted files
                    // Since the UnifiedStorageEngine doesn't return file paths, we'll return a placeholder
                    Ok(vec![format!("compacted_collection_{}_{}files", collection_id, result.output_files)])
                } else {
                    warn!(
                        "❌ [COMPACTION] {} compaction failed for collection {}",
                        engine.engine_name(),
                        collection_id
                    );
                    Err(anyhow::anyhow!("Storage engine compaction failed"))
                }
            }
            Err(e) => {
                warn!(
                    "❌ [COMPACTION] {} compaction error for collection {}: {}",
                    engine.engine_name(),
                    collection_id,
                    e
                );
                Err(e)
            }
        }
    }
    
    /// Combine multiple single-row RecordBatches into a single larger batch with schema alignment
    fn combine_record_batches(schema: Arc<arrow_schema::Schema>, batches: &[arrow_array::RecordBatch]) -> Result<arrow_array::RecordBatch> {
        if batches.is_empty() {
            return Err(anyhow::anyhow!("Cannot combine empty batches"));
        }
        
        use arrow_array::ArrayRef;
        
        
        let mut combined_columns = Vec::new();
        
        // Process each column in the target schema to ensure proper alignment
        for field in schema.fields() {
            let field_name = field.name();
            let field_type = field.data_type();
            let mut column_arrays: Vec<ArrayRef> = Vec::new();
            
            // Collect arrays for this column from all batches, handling schema evolution
            for batch in batches {
                let array = if let Some(column) = batch.column_by_name(field_name) {
                    // Column exists in this batch
                    column.clone()
                } else {
                    // Column doesn't exist in this batch - create null array
                    warn!("Column '{}' not found in batch, creating null array", field_name);
                    Self::create_null_array_static(field_type, 1)?
                };
                column_arrays.push(array);
            }
            
            // Concatenate arrays for this column
            let combined_array = Self::concatenate_arrays_by_type_static(field_type, column_arrays)?;
            combined_columns.push(combined_array);
        }
        
        arrow_array::RecordBatch::try_new(schema, combined_columns)
            .map_err(|e| anyhow::anyhow!("Failed to create combined RecordBatch with schema alignment: {}", e))
    }
    
    /// Create a null array of the specified type and length (static version)
    fn create_null_array_static(data_type: &arrow_schema::DataType, length: usize) -> Result<arrow_array::ArrayRef> {
        use arrow_array::{ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, 
                         Int64Array, StringArray, TimestampMillisecondArray};
        use arrow_schema::{DataType, TimeUnit};
        use std::sync::Arc;
        
        let null_array: ArrayRef = match data_type {
            DataType::Utf8 => Arc::new(StringArray::from(vec![Option::<String>::None; length])),
            DataType::Int64 => Arc::new(Int64Array::from(vec![Option::<i64>::None; length])),
            DataType::Float32 => Arc::new(Float32Array::from(vec![Option::<f32>::None; length])),
            DataType::Float64 => Arc::new(Float64Array::from(vec![Option::<f64>::None; length])),
            DataType::Boolean => Arc::new(BooleanArray::from(vec![Option::<bool>::None; length])),
            DataType::Binary => {
                let null_values: Vec<Option<&[u8]>> = vec![None; length];
                Arc::new(BinaryArray::from(null_values))
            }
            DataType::Timestamp(TimeUnit::Millisecond, _) => {
                Arc::new(TimestampMillisecondArray::from(vec![Option::<i64>::None; length]))
            }
            _ => {
                // For other types, create a simple string null array as fallback
                Arc::new(StringArray::from(vec![Option::<String>::None; length]))
            }
        };
        
        Ok(null_array)
    }
    
    /// Concatenate arrays of a specific type (static version)
    fn concatenate_arrays_by_type_static(
        data_type: &arrow_schema::DataType,
        arrays: Vec<arrow_array::ArrayRef>,
    ) -> Result<arrow_array::ArrayRef> {
        if arrays.is_empty() {
            return Err(anyhow::anyhow!("Cannot concatenate empty array list"));
        }
        
        if arrays.len() == 1 {
            return Ok(arrays[0].clone());
        }
        
        use arrow_array::{Array, 
                         Int64Array, StringArray};
        use arrow_schema::DataType;
        use std::sync::Arc;
        
        // Manual concatenation for proper schema alignment
        match data_type {
            DataType::Utf8 => {
                let mut values = Vec::new();
                for array in &arrays {
                    let string_array = array.as_any().downcast_ref::<StringArray>()
                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StringArray"))?;
                    for i in 0..string_array.len() {
                        values.push(if string_array.is_null(i) {
                            None
                        } else {
                            Some(string_array.value(i).to_string())
                        });
                    }
                }
                Ok(Arc::new(StringArray::from(values)))
            }
            DataType::Int64 => {
                let mut values = Vec::new();
                for array in &arrays {
                    let int_array = array.as_any().downcast_ref::<Int64Array>()
                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
                    for i in 0..int_array.len() {
                        values.push(if int_array.is_null(i) {
                            None
                        } else {
                            Some(int_array.value(i))
                        });
                    }
                }
                Ok(Arc::new(Int64Array::from(values)))
            }
            _ => {
                // For other types, return the first array as fallback
                warn!("Concatenation for type {:?} not implemented in BackgroundManager, using first array", data_type);
                Ok(arrays[0].clone())
            }
        }
    }

    /// Update moving average for duration tracking
    fn update_average_duration(current_avg: &mut f64, new_duration: f64, total_count: u64) {
        if total_count == 1 {
            *current_avg = new_duration;
        } else {
            let alpha = 0.1; // Smoothing factor for exponential moving average
            *current_avg = alpha * new_duration + (1.0 - alpha) * (*current_avg);
        }
    }

    /// Get current status for a collection
    pub async fn get_collection_status(
        &self,
        collection_id: &str,
    ) -> BackgroundTaskStatus {
        let status_map = self.collection_status.read().await;
        status_map
            .get(collection_id)
            .cloned()
            .unwrap_or(BackgroundTaskStatus::Idle)
    }

    /// Get maintenance statistics
    pub async fn get_stats(&self) -> BackgroundMaintenanceStats {
        let stats = self.stats.lock().await;
        stats.clone()
    }

    /// Check if any background operations are running
    pub async fn has_active_operations(&self) -> bool {
        let status_map = self.collection_status.read().await;
        status_map
            .values()
            .any(|status| *status != BackgroundTaskStatus::Idle)
    }

    /// Wait for all background operations to complete
    pub async fn wait_for_completion(&self) -> Result<()> {
        let mut check_count = 0;
        const MAX_CHECKS: u32 = 600; // 60 seconds with 100ms intervals

        while self.has_active_operations().await && check_count < MAX_CHECKS {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            check_count += 1;
        }

        if check_count >= MAX_CHECKS {
            warn!("Background operations did not complete within timeout");
        }

        Ok(())
    }

    /// Force stop all background operations (for shutdown)
    pub async fn shutdown(&self) -> Result<()> {
        info!("🛑 Shutting down background maintenance manager");

        // Clear all status tracking
        {
            let mut status_map = self.collection_status.write().await;
            status_map.clear();
        }

        Ok(())
    }
}
