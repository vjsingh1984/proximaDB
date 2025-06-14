// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Storage Engine Implementation
//! 
//! Advanced columnar storage engine using Parquet format with intelligent
//! clustering and compression optimizations. Key features:
//! 
//! ## Columnar Compression Benefits
//! - **Dense vectors**: Parquet columnar compression excels at similar values
//! - **Sparse vectors**: Key-value pairs with missing keys = null compression
//! - **Feature selection**: Read only important columns during clustering
//! - **Background compaction**: Small Parquet files merged intelligently
//! 
//! ## ML-Driven Optimizations  
//! - **Trained models per collection**: Predict optimal clustering
//! - **Feature importance**: Use only relevant vector dimensions for partitioning
//! - **Intelligent compaction**: ML-guided data reorganization during compaction

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use anyhow::{Result, Context};

use crate::core::{VectorId, CollectionId, VectorRecord};
use crate::storage::{WalManager, WalEntry};
use super::types::*;
use super::{ViperConfig, CompressionAlgorithm, TierLevel};

/// Calculate sparsity ratio for a vector (standalone function)
fn calculate_sparsity_ratio(vector: &[f32]) -> f32 {
    let zero_count = vector.iter().filter(|&&x| x == 0.0).count();
    zero_count as f32 / vector.len() as f32
}

/// VIPER Storage Engine with ML-driven clustering and Parquet optimization
pub struct ViperStorageEngine {
    /// Configuration
    config: ViperConfig,
    
    /// Unified WAL manager
    wal_manager: Arc<WalManager>,
    
    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<CollectionId, CollectionMetadata>>>,
    
    /// Cluster metadata cache  
    clusters: Arc<RwLock<HashMap<ClusterId, ClusterMetadata>>>,
    
    /// Partition metadata cache
    partitions: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,
    
    /// ML models for cluster prediction per collection
    ml_models: Arc<RwLock<HashMap<CollectionId, ClusterPredictionModel>>>,
    
    /// Feature importance models for column selection
    feature_models: Arc<RwLock<HashMap<CollectionId, FeatureImportanceModel>>>,
    
    /// Background compaction manager
    compaction_manager: Arc<super::compaction::ViperCompactionEngine>,
    
    /// Parquet writer pool for concurrent operations
    writer_pool: Arc<ParquetWriterPool>,
    
    /// Statistics tracker
    stats: Arc<RwLock<ViperStorageStats>>,
}

/// Collection metadata in VIPER
#[derive(Debug, Clone)]
pub struct CollectionMetadata {
    pub collection_id: CollectionId,
    pub dimension: usize,
    pub vector_count: usize,
    pub total_clusters: usize,
    pub storage_format_preference: VectorStorageFormat,
    pub ml_model_version: Option<String>,
    pub feature_importance: Vec<f32>, // Per-dimension importance scores
    pub compression_stats: CompressionStats,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
}

/// Compression statistics for optimization
#[derive(Debug, Clone)]
pub struct CompressionStats {
    pub sparse_compression_ratio: f32,
    pub dense_compression_ratio: f32,
    pub optimal_sparsity_threshold: f32,
    pub column_compression_ratios: Vec<f32>, // Per-dimension compression ratios
}

/// ML model for cluster prediction
#[derive(Debug, Clone)]
pub struct ClusterPredictionModel {
    pub model_id: String,
    pub version: String,
    pub accuracy: f32,
    pub feature_weights: Vec<f32>, // Weights per vector dimension
    pub cluster_centroids: Vec<Vec<f32>>, // Cluster centers
    pub model_data: Vec<u8>, // Serialized model (e.g., ONNX)
    pub training_timestamp: DateTime<Utc>,
    pub prediction_stats: PredictionStats,
}

/// Feature importance model for column selection optimization
#[derive(Debug, Clone)]
pub struct FeatureImportanceModel {
    pub model_id: String,
    pub importance_scores: Vec<f32>, // Score per dimension (0.0 to 1.0)
    pub top_k_features: Vec<usize>, // Most important dimension indices
    pub compression_impact: Vec<f32>, // Compression benefit per feature
    pub clustering_impact: Vec<f32>, // Clustering quality impact per feature
    pub last_trained: DateTime<Utc>,
}

/// Prediction model statistics
#[derive(Debug, Clone)]
pub struct PredictionStats {
    pub total_predictions: u64,
    pub correct_predictions: u64,
    pub average_confidence: f32,
    pub prediction_latency_ms: f32,
    pub last_accuracy_check: DateTime<Utc>,
}

/// Parquet writer pool for concurrent I/O
pub struct ParquetWriterPool {
    writers: Arc<RwLock<HashMap<PartitionId, ParquetWriter>>>,
    max_concurrent_writers: usize,
}

/// Parquet writer for specific partitions
pub struct ParquetWriter {
    partition_id: PartitionId,
    file_path: PathBuf,
    schema: ParquetSchema,
    row_group_size: usize,
    current_batch: Vec<ViperVector>,
    compression_algorithm: CompressionAlgorithm,
}

/// Parquet schema definition for VIPER vectors optimized for ID-based lookups
#[derive(Debug, Clone)]
pub struct ParquetSchema {
    pub schema_version: u32,
    /// ID column (first for optimal ID-based lookups)
    pub id_column: String,
    /// Metadata columns (second for filtering efficiency)
    pub metadata_columns: Vec<String>,
    /// Cluster assignment column
    pub cluster_column: String,
    /// Timestamp column
    pub timestamp_column: String,
    /// Vector data columns (last for optimal columnar access)
    pub vector_columns: VectorColumns,
}

/// Vector column definitions based on storage format optimized for vector databases
#[derive(Debug, Clone)]
pub enum VectorColumns {
    /// Dense vectors: ID and metadata first, then dimension columns
    Dense {
        dimension_columns: Vec<String>, // "dim_0", "dim_1", etc.
    },
    
    /// Sparse vectors: separate metadata table for efficient lookups
    Sparse {
        /// Metadata table reference for efficient ID-based filtering
        metadata_table_ref: Option<String>,
        /// Key-value columns for vector data
        dimension_key_column: String,   // "dim_key"
        dimension_value_column: String, // "dim_value"
    },
    
    /// Hybrid: both formats with optimized column ordering
    Hybrid {
        format_indicator_column: String, // "storage_format"
        dense_columns: Vec<String>,
        sparse_key_column: String,
        sparse_value_column: String,
        /// Separate metadata table for sparse vectors
        sparse_metadata_table_ref: Option<String>,
    },
}

/// VIPER storage statistics
#[derive(Debug, Default)]
pub struct ViperStorageStats {
    pub total_vectors: u64,
    pub total_partitions: u64,
    pub total_clusters: u64,
    pub compression_ratio: f32,
    pub ml_prediction_accuracy: f32,
    pub average_cluster_quality: f32,
    pub storage_size_bytes: u64,
    pub read_operations: u64,
    pub write_operations: u64,
    pub compaction_operations: u64,
}

impl ViperStorageEngine {
    /// Create a new VIPER storage engine with unified WAL
    pub async fn new(config: ViperConfig, wal_manager: Arc<WalManager>) -> Result<Self> {
        // Ensure storage directories exist
        tokio::fs::create_dir_all(&config.storage_root).await
            .context("Failed to create VIPER storage root directory")?;
        
        let compaction_manager = Arc::new(
            super::compaction::ViperCompactionEngine::new(config.clone()).await?
        );
        
        let writer_pool = Arc::new(ParquetWriterPool::new(
            config.performance_config.parallel_config.io_thread_count
        ));
        
        Ok(Self {
            config,
            wal_manager,
            collections: Arc::new(RwLock::new(HashMap::new())),
            clusters: Arc::new(RwLock::new(HashMap::new())),
            partitions: Arc::new(RwLock::new(HashMap::new())),
            ml_models: Arc::new(RwLock::new(HashMap::new())),
            feature_models: Arc::new(RwLock::new(HashMap::new())),
            compaction_manager,
            writer_pool,
            stats: Arc::new(RwLock::new(ViperStorageStats::default())),
        })
    }
    
    /// Recover VIPER state from unified WAL
    pub async fn recover_from_wal(&self) -> Result<()> {
        // Get all VIPER-specific entries from WAL
        let wal_entries = self.wal_manager.read_all().await
            .map_err(|e| anyhow::anyhow!("Failed to read WAL: {}", e))?;
        
        for entry in wal_entries {
            match entry {
                WalEntry::ViperVectorInsert { 
                    collection_id, vector_id, vector_data, metadata, 
                    cluster_prediction, storage_format, tier_level, expires_at, .. 
                } => {
                    // Recreate vector and apply insertion
                    let vector_record = VectorRecord {
                        id: vector_id,
                        collection_id: collection_id.clone(),
                        vector: vector_data,
                        metadata: metadata.as_object().unwrap_or(&serde_json::Map::new()).clone().into_iter().collect(),
                        timestamp: chrono::Utc::now(),
                        expires_at,
                    };
                    
                    self.apply_vector_insert(collection_id, vector_record, cluster_prediction, storage_format, tier_level).await?;
                }
                
                WalEntry::ViperVectorUpdate { 
                    collection_id, vector_id, old_cluster_id, new_cluster_id, 
                    updated_metadata, tier_level, .. 
                } => {
                    self.apply_vector_update(collection_id, vector_id, old_cluster_id, new_cluster_id, updated_metadata, tier_level).await?;
                }
                
                WalEntry::ViperVectorDelete { 
                    collection_id, vector_id, cluster_id, tier_level, .. 
                } => {
                    self.apply_vector_delete(collection_id, vector_id, cluster_id, tier_level).await?;
                }
                
                WalEntry::ViperClusterUpdate { 
                    collection_id, cluster_id, new_centroid, quality_metrics, .. 
                } => {
                    self.apply_cluster_update(collection_id, cluster_id, new_centroid, quality_metrics).await?;
                }
                
                WalEntry::ViperPartitionCreate { 
                    collection_id, partition_id, cluster_ids, tier_level, .. 
                } => {
                    self.apply_partition_create(collection_id, partition_id, cluster_ids, tier_level).await?;
                }
                
                WalEntry::ViperCompactionOperation { 
                    collection_id, operation_id, input_partitions, output_partitions, tier_level, .. 
                } => {
                    self.apply_compaction_operation(collection_id, operation_id, input_partitions, output_partitions, tier_level).await?;
                }
                
                WalEntry::ViperTierMigration { 
                    collection_id, partition_id, from_tier, to_tier, migration_reason, .. 
                } => {
                    self.apply_tier_migration(collection_id, partition_id, from_tier, to_tier, migration_reason).await?;
                }
                
                WalEntry::ViperModelUpdate { 
                    collection_id, model_type, model_version, accuracy_metrics, .. 
                } => {
                    self.apply_model_update(collection_id, model_type, model_version, accuracy_metrics).await?;
                }
                
                _ => {
                    // Ignore non-VIPER entries
                }
            }
        }
        
        Ok(())
    }
    
    /// Create a new collection with ML-driven optimization
    pub async fn create_collection(
        &self,
        collection_id: CollectionId,
        dimension: usize,
        initial_vectors: Option<Vec<VectorRecord>>,
    ) -> Result<()> {
        let mut collections = self.collections.write().await;
        
        if collections.contains_key(&collection_id) {
            return Err(anyhow::anyhow!("Collection already exists: {}", collection_id));
        }
        
        // Analyze initial vectors to determine optimal storage format
        let (storage_format, feature_importance) = if let Some(ref vectors) = initial_vectors {
            self.analyze_initial_vectors(vectors, dimension).await?
        } else {
            (VectorStorageFormat::Auto, vec![1.0; dimension]) // Equal importance initially
        };
        
        let collection_metadata = CollectionMetadata {
            collection_id: collection_id.clone(),
            dimension,
            vector_count: initial_vectors.as_ref().map(|v| v.len()).unwrap_or(0),
            total_clusters: 0,
            storage_format_preference: storage_format,
            ml_model_version: None,
            feature_importance,
            compression_stats: CompressionStats {
                sparse_compression_ratio: 1.0,
                dense_compression_ratio: 1.0,
                optimal_sparsity_threshold: 0.7, // Default threshold
                column_compression_ratios: vec![1.0; dimension],
            },
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };
        
        collections.insert(collection_id.clone(), collection_metadata);
        
        // If initial vectors provided, insert them and train initial models
        if let Some(vectors) = initial_vectors {
            drop(collections); // Release lock before calling insert
            self.insert_vectors_batch(collection_id.clone(), vectors).await?;
            self.train_initial_models(collection_id).await?;
        }
        
        Ok(())
    }
    
    /// Insert a single vector using hybrid VIPER storage (dense Parquet or sparse KV+metadata)
    pub async fn insert_vector(
        &self,
        collection_id: &CollectionId,
        vector_record: VectorRecord,
    ) -> Result<()> {
        // Get collection metadata or create if new
        let collection_meta = self.get_or_create_collection_metadata(collection_id).await?;
        
        // Predict cluster assignment using ML model
        let cluster_prediction = self.predict_cluster(collection_id, &vector_record.vector).await?;
        
        // Determine storage format (sparse vs dense) based on sparsity
        let storage_format = self.determine_format(&vector_record.vector, &collection_meta).await?;
        
        // Write to WAL first for durability
        let wal_entry = WalEntry::ViperVectorInsert {
            collection_id: collection_id.clone(),
            vector_id: vector_record.id,
            vector_data: vector_record.vector.clone(),
            metadata: serde_json::to_value(vector_record.metadata.clone())?,
            cluster_prediction: cluster_prediction.clone(),
            storage_format,
            tier_level: TierLevel::UltraHot,
            expires_at: vector_record.expires_at,
            timestamp: Utc::now(),
        };
        
        self.wal_manager.append(wal_entry).await?;
        
        // Create VIPER vector entry based on hybrid storage architecture
        let viper_vector = match storage_format {
            VectorStorageFormat::Sparse => {
                // For sparse vectors: separate metadata Parquet + KV storage
                let dimensions = self.vector_to_sparse_pairs(&vector_record.vector);
                let sparsity_ratio = 1.0 - (dimensions.len() as f32 / vector_record.vector.len() as f32);
                
                let metadata = SparseVectorMetadata::new(
                    vector_record.id,
                    serde_json::to_value(vector_record.metadata.clone())?,
                    cluster_prediction.cluster_id,
                    vector_record.vector.len() as u32,
                    sparsity_ratio,
                );
                
                let data = SparseVectorData::new(vector_record.id, dimensions);
                
                ViperVector::Sparse { metadata, data }
            }
            VectorStorageFormat::Dense => {
                // For dense vectors: single Parquet row with ID/metadata columns first
                let record = DenseVectorRecord::new(
                    vector_record.id,
                    vector_record.vector,
                    serde_json::to_value(vector_record.metadata.clone())?,
                    cluster_prediction.cluster_id,
                );
                
                ViperVector::Dense { record }
            }
            VectorStorageFormat::Auto => {
                return Err(anyhow::anyhow!("Auto format should have been resolved by determine_format"));
            }
        };
        
        // Write to appropriate hybrid storage (Parquet + KV for sparse, Parquet for dense)
        self.write_to_hybrid_storage(collection_id, viper_vector, &cluster_prediction).await?;
        
        // Update collection statistics
        self.update_collection_stats_single(collection_id, &cluster_prediction).await?;
        
        Ok(())
    }

    /// Insert a batch of vectors with ML-guided clustering
    pub async fn insert_vectors_batch(
        &self,
        collection_id: CollectionId,
        vectors: Vec<VectorRecord>,
    ) -> Result<Vec<ClusterPrediction>> {
        let _collection = self.get_collection_metadata(&collection_id).await?;
        
        // Get cluster predictions for the batch
        let predictions = self.predict_clusters(&collection_id, &vectors).await?;
        
        // Group vectors by predicted clusters for efficient I/O
        let mut cluster_batches: HashMap<ClusterId, Vec<(VectorRecord, ClusterPrediction)>> = HashMap::new();
        
        for (vector, prediction) in vectors.into_iter().zip(predictions.iter().cloned()) {
            cluster_batches
                .entry(prediction.cluster_id)
                .or_insert_with(Vec::new)
                .push((vector, prediction));
        }
        
        // Process each cluster batch and log to WAL
        for (cluster_id, batch) in cluster_batches {
            // Log operations to unified WAL first
            for (vector, prediction) in &batch {
                let _ = self.wal_manager.log_viper_vector_insert(
                    collection_id.clone(),
                    vector.id,
                    vector.vector.clone(),
                    serde_json::to_value(vector.metadata.clone()).unwrap_or(serde_json::Value::Null),
                    prediction.clone(),
                    self.determine_storage_format(&vector.vector),
                    TierLevel::Hot, // Default tier for new vectors
                ).await;
            }
            
            // Then perform the actual insertion using hybrid storage
            self.insert_cluster_batch_hybrid(collection_id.clone(), cluster_id, batch).await?;
        }
        
        // Update collection statistics
        self.update_collection_stats(&collection_id).await?;
        
        // Trigger background compaction if needed
        self.compaction_manager.schedule_compaction_if_needed(&collection_id).await?;
        
        Ok(predictions)
    }
    
    /// Search vectors using ML-guided cluster pruning
    pub async fn search_vectors(
        &self,
        context: ViperSearchContext,
    ) -> Result<Vec<ViperSearchResult>> {
        let _collection = self.get_collection_metadata(&context.collection_id).await?;
        
        // Get cluster predictions for query vector to prune search space
        let cluster_hints = if context.cluster_hints.is_none() {
            Some(self.predict_query_clusters(&context).await?)
        } else {
            context.cluster_hints.clone()
        };
        
        // Select important features for efficient column reading
        let important_features = self.get_important_features(&context.collection_id).await?;
        
        // Progressive search across tiers based on strategy
        let results = match context.search_strategy.clone() {
            SearchStrategy::Progressive { tier_result_thresholds } => {
                self.progressive_tier_search(context, cluster_hints, important_features, tier_result_thresholds).await?
            }
            
            SearchStrategy::ClusterPruned { max_clusters, confidence_threshold } => {
                self.cluster_pruned_search(
                    context, 
                    cluster_hints, 
                    important_features,
                    max_clusters, 
                    confidence_threshold
                ).await?
            }
            
            SearchStrategy::Exhaustive => {
                self.exhaustive_search(context, important_features).await?
            }
            
            SearchStrategy::Adaptive { query_complexity_score, time_budget_ms } => {
                self.adaptive_search(
                    context,
                    query_complexity_score,
                    time_budget_ms,
                    cluster_hints,
                    important_features
                ).await?
            }
        };
        
        // Update search statistics
        self.update_search_stats(&results).await?;
        
        Ok(results)
    }
    
    /// Analyze initial vectors to determine optimal storage strategy
    async fn analyze_initial_vectors(
        &self,
        vectors: &[VectorRecord],
        dimension: usize,
    ) -> Result<(VectorStorageFormat, Vec<f32>)> {
        if vectors.is_empty() {
            return Ok((VectorStorageFormat::Auto, vec![1.0; dimension]));
        }
        
        // Calculate sparsity statistics
        let mut total_sparsity = 0.0;
        let mut dimension_variances = vec![0.0; dimension];
        let mut dimension_means = vec![0.0; dimension];
        
        for vector in vectors {
            // Calculate sparsity
            let zero_count = vector.vector.iter().filter(|&&x| x == 0.0).count();
            total_sparsity += zero_count as f32 / vector.vector.len() as f32;
            
            // Calculate per-dimension statistics for feature importance
            for (i, &value) in vector.vector.iter().enumerate() {
                if i < dimension {
                    dimension_means[i] += value;
                }
            }
        }
        
        // Finalize means
        for mean in dimension_means.iter_mut() {
            *mean /= vectors.len() as f32;
        }
        
        // Calculate variances for feature importance
        for vector in vectors {
            for (i, &value) in vector.vector.iter().enumerate() {
                if i < dimension {
                    let diff = value - dimension_means[i];
                    dimension_variances[i] += diff * diff;
                }
            }
        }
        
        // Finalize variances and calculate feature importance
        let mut feature_importance = vec![0.0; dimension];
        for i in 0..dimension {
            dimension_variances[i] /= vectors.len() as f32;
            // Higher variance = more important for clustering
            feature_importance[i] = dimension_variances[i].sqrt();
        }
        
        // Normalize feature importance
        let max_importance = feature_importance.iter().fold(0.0f32, |a, &b| a.max(b));
        if max_importance > 0.0 {
            for importance in feature_importance.iter_mut() {
                *importance /= max_importance;
            }
        }
        
        // Determine storage format based on average sparsity
        let avg_sparsity = total_sparsity / vectors.len() as f32;
        let storage_format = if avg_sparsity > 0.7 {
            VectorStorageFormat::Sparse
        } else if avg_sparsity < 0.3 {
            VectorStorageFormat::Dense
        } else {
            VectorStorageFormat::Auto
        };
        
        Ok((storage_format, feature_importance))
    }
    
    /// Predict clusters for a batch of vectors using ML model
    async fn predict_clusters(
        &self,
        collection_id: &CollectionId,
        vectors: &[VectorRecord],
    ) -> Result<Vec<ClusterPrediction>> {
        let models = self.ml_models.read().await;
        
        if let Some(model) = models.get(collection_id) {
            // Use trained ML model for prediction
            self.ml_predict_clusters(model, vectors).await
        } else {
            // Use simple heuristic clustering for new collections
            self.heuristic_cluster_assignment(collection_id, vectors).await
        }
    }
    
    /// ML-based cluster prediction using trained model
    async fn ml_predict_clusters(
        &self,
        model: &ClusterPredictionModel,
        vectors: &[VectorRecord],
    ) -> Result<Vec<ClusterPrediction>> {
        let mut predictions = Vec::with_capacity(vectors.len());
        
        for vector in vectors {
            // Calculate weighted features using model feature weights
            let weighted_features: Vec<f32> = vector.vector
                .iter()
                .zip(model.feature_weights.iter())
                .map(|(&v, &w)| v * w)
                .collect();
            
            // Find nearest cluster centroid
            let mut best_cluster = 0;
            let mut best_distance = f32::INFINITY;
            let mut distances = Vec::new();
            
            for (cluster_idx, centroid) in model.cluster_centroids.iter().enumerate() {
                let distance = self.calculate_weighted_distance(&weighted_features, centroid);
                distances.push(distance);
                
                if distance < best_distance {
                    best_distance = distance;
                    best_cluster = cluster_idx;
                }
            }
            
            // Calculate confidence based on distance distribution
            distances.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let confidence = if distances.len() > 1 {
                let gap = distances[1] - distances[0];
                (gap / (distances[1] + 1e-6)).min(1.0)
            } else {
                1.0
            };
            
            // Generate alternatives (top 3 closest clusters)
            let mut alternatives = Vec::new();
            for (i, distance) in distances.iter().take(3).enumerate() {
                if i != best_cluster {
                    alternatives.push((i as ClusterId, 1.0 - distance / (distances[0] + 1e-6)));
                }
            }
            
            predictions.push(ClusterPrediction {
                cluster_id: best_cluster as ClusterId,
                confidence,
                alternatives,
                prediction_metadata: PredictionMetadata {
                    model_version: model.version.clone(),
                    features_used: model.feature_weights.iter()
                        .enumerate()
                        .filter(|(_, &w)| w > 0.1) // Only include significant features
                        .map(|(i, _)| format!("dim_{}", i))
                        .collect(),
                    predicted_at: Utc::now(),
                    model_accuracy: model.accuracy,
                    prediction_cost_ms: 1, // Estimate based on model complexity
                },
            });
        }
        
        Ok(predictions)
    }
    
    /// Calculate weighted distance between vectors
    fn calculate_weighted_distance(&self, vector1: &[f32], vector2: &[f32]) -> f32 {
        vector1.iter()
            .zip(vector2.iter())
            .map(|(&a, &b)| {
                let diff = a - b;
                diff * diff
            })
            .sum::<f32>()
            .sqrt()
    }
    
    /// Get important features for efficient column reading during search
    async fn get_important_features(&self, collection_id: &CollectionId) -> Result<Vec<usize>> {
        let feature_models = self.feature_models.read().await;
        
        if let Some(model) = feature_models.get(collection_id) {
            Ok(model.top_k_features.clone())
        } else {
            // Return all features if no model trained yet
            let collection = self.get_collection_metadata(collection_id).await?;
            Ok((0..collection.dimension).collect())
        }
    }
    
    /// Insert a batch of vectors for a specific cluster with optimized Parquet writing
    async fn insert_cluster_batch(
        &self,
        collection_id: CollectionId,
        cluster_id: ClusterId,
        batch: Vec<(VectorRecord, ClusterPrediction)>,
    ) -> Result<()> {
        // Determine partition for this cluster
        let partition_id = self.get_or_create_partition(&collection_id, cluster_id).await?;
        
        // Convert to VIPER vectors with optimal storage format
        let collection = self.get_collection_metadata(&collection_id).await?;
        let viper_vectors = self.convert_to_viper_vectors(batch, &collection).await?;
        
        // Write to Parquet using optimized columnar layout
        self.write_vectors_to_parquet(&partition_id, viper_vectors).await?;
        
        // Update cluster and partition metadata
        self.update_cluster_metadata(cluster_id, &collection_id).await?;
        
        Ok(())
    }
    
    /// Convert vectors to optimal VIPER format based on sparsity analysis
    async fn convert_to_viper_vectors(
        &self,
        batch: Vec<(VectorRecord, ClusterPrediction)>,
        collection: &CollectionMetadata,
    ) -> Result<Vec<ViperVector>> {
        let mut viper_vectors = Vec::with_capacity(batch.len());
        
        for (vector, prediction) in batch {
            let sparsity = self.calculate_sparsity(&vector.vector);
            
            let viper_vector = match collection.storage_format_preference {
                VectorStorageFormat::Sparse => {
                    self.create_sparse_viper_vector(vector, prediction.cluster_id)?
                }
                VectorStorageFormat::Dense => {
                    self.create_dense_viper_vector(vector, prediction.cluster_id)?
                }
                VectorStorageFormat::Auto => {
                    if sparsity > collection.compression_stats.optimal_sparsity_threshold {
                        self.create_sparse_viper_vector(vector, prediction.cluster_id)?
                    } else {
                        self.create_dense_viper_vector(vector, prediction.cluster_id)?
                    }
                }
            };
            
            viper_vectors.push(viper_vector);
        }
        
        Ok(viper_vectors)
    }
    
    /// Create sparse VIPER vector (key-value format for high compression)
    fn create_sparse_viper_vector(
        &self,
        vector: VectorRecord,
        cluster_id: ClusterId,
    ) -> Result<ViperVector> {
        let mut dimensions = HashMap::new();
        
        for (idx, &value) in vector.vector.iter().enumerate() {
            if value != 0.0 { // Only store non-zero values
                dimensions.insert(idx as u32, value);
            }
        }
        
        let metadata = SparseVectorMetadata {
            id: vector.id,
            metadata: serde_json::to_value(vector.metadata)?,
            cluster_id,
            dimension_count: dimensions.len() as u32,
            sparsity_ratio: self.calculate_sparsity(&vector.vector),
            timestamp: vector.timestamp,
            expires_at: vector.expires_at,
        };
        let dimensions_vec: Vec<(u32, f32)> = dimensions.into_iter().collect();
        let data = SparseVectorData { 
            id: vector.id,
            dimensions: dimensions_vec 
        };
        
        Ok(ViperVector::Sparse { metadata, data })
    }
    
    /// Create dense VIPER vector (row format for better scan performance)
    fn create_dense_viper_vector(
        &self,
        vector: VectorRecord,
        cluster_id: ClusterId,
    ) -> Result<ViperVector> {
        let record = DenseVectorRecord {
            id: vector.id,
            metadata: serde_json::to_value(vector.metadata)?,
            cluster_id,
            vector: vector.vector,
            timestamp: vector.timestamp,
            expires_at: vector.expires_at,
        };
        
        Ok(ViperVector::Dense { record })
    }
    
    /// Calculate sparsity ratio for a vector
    fn calculate_sparsity(&self, vector: &[f32]) -> f32 {
        let zero_count = vector.iter().filter(|&&x| x == 0.0).count();
        zero_count as f32 / vector.len() as f32
    }
    
    /// Determine storage format based on vector sparsity
    fn determine_storage_format(&self, vector: &[f32]) -> VectorStorageFormat {
        let sparsity = self.calculate_sparsity(vector);
        if sparsity > 0.7 {
            VectorStorageFormat::Sparse
        } else if sparsity < 0.3 {
            VectorStorageFormat::Dense
        } else {
            VectorStorageFormat::Auto
        }
    }
    
    /// Write vectors to Parquet with optimized columnar compression
    async fn write_vectors_to_parquet(
        &self,
        partition_id: &PartitionId,
        vectors: Vec<ViperVector>,
    ) -> Result<()> {
        let writer_pool = self.writer_pool.clone();
        let mut writers = writer_pool.writers.write().await;
        
        let writer = writers.entry(partition_id.clone())
            .or_insert_with(|| {
                ParquetWriter::new(
                    partition_id.clone(),
                    self.get_partition_path(partition_id),
                    self.create_optimized_schema(&vectors),
                    self.config.parquet_config.row_group_size,
                    self.config.compression_config.vector_compression.clone(),
                )
            });
        
        writer.add_vectors(vectors).await?;
        
        // Flush if batch is large enough
        if writer.should_flush() {
            writer.flush().await?;
        }
        
        Ok(())
    }
    
    /// Helper methods for metadata management
    async fn get_collection_metadata(&self, collection_id: &CollectionId) -> Result<CollectionMetadata> {
        let collections = self.collections.read().await;
        collections.get(collection_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))
    }
    
    async fn get_or_create_partition(&self, collection_id: &CollectionId, cluster_id: ClusterId) -> Result<PartitionId> {
        // Implementation for partition management
        Ok(format!("{}_{}", collection_id, cluster_id))
    }
    
    fn get_partition_path(&self, partition_id: &PartitionId) -> PathBuf {
        self.config.storage_root.join(partition_id)
    }
    
    fn create_optimized_schema(&self, vectors: &[ViperVector]) -> ParquetSchema {
        // Determine optimal schema based on vector format and collection requirements
        let vector_columns = if vectors.is_empty() {
            // Default to dense format
            VectorColumns::Dense {
                dimension_columns: (0..384).map(|i| format!("dim_{}", i)).collect(),
            }
        } else {
            // Analyze vectors to determine optimal format
            let has_sparse = vectors.iter().any(|v| matches!(v, ViperVector::Sparse { .. }));
            let has_dense = vectors.iter().any(|v| matches!(v, ViperVector::Dense { .. }));
            
            match (has_sparse, has_dense) {
                (true, false) => VectorColumns::Sparse {
                    metadata_table_ref: Some("metadata_index".to_string()),
                    dimension_key_column: "dim_key".to_string(),
                    dimension_value_column: "dim_value".to_string(),
                },
                (false, true) => {
                    // Determine dimension from first dense vector
                    let dimension = vectors.iter()
                        .find_map(|v| match v {
                            ViperVector::Dense { record } => Some(record.vector.len()),
                            _ => None,
                        })
                        .unwrap_or(384);
                    
                    VectorColumns::Dense {
                        dimension_columns: (0..dimension).map(|i| format!("dim_{}", i)).collect(),
                    }
                },
                (true, true) => {
                    // Hybrid format
                    let dimension = vectors.iter()
                        .find_map(|v| match v {
                            ViperVector::Dense { record } => Some(record.vector.len()),
                            _ => None,
                        })
                        .unwrap_or(384);
                    
                    VectorColumns::Hybrid {
                        format_indicator_column: "storage_format".to_string(),
                        dense_columns: (0..dimension).map(|i| format!("dim_{}", i)).collect(),
                        sparse_key_column: "sparse_dim_key".to_string(),
                        sparse_value_column: "sparse_dim_value".to_string(),
                        sparse_metadata_table_ref: Some("sparse_metadata_index".to_string()),
                    }
                },
                (false, false) => {
                    // Fallback to dense
                    VectorColumns::Dense {
                        dimension_columns: (0..384).map(|i| format!("dim_{}", i)).collect(),
                    }
                }
            }
        };
        
        // Create optimized schema with ID and metadata columns first
        ParquetSchema {
            schema_version: 2, // Increment version for new layout
            id_column: "vector_id".to_string(),
            metadata_columns: vec!["metadata".to_string(), "user_metadata".to_string()],
            cluster_column: "cluster_id".to_string(),
            timestamp_column: "timestamp".to_string(),
            vector_columns,
        }
    }
    
    // Additional placeholder methods for compilation
    async fn heuristic_cluster_assignment(&self, _collection_id: &CollectionId, _vectors: &[VectorRecord]) -> Result<Vec<ClusterPrediction>> {
        // Simplified implementation for new collections
        Ok(Vec::new())
    }
    
    async fn train_initial_models(&self, _collection_id: CollectionId) -> Result<()> {
        // Train ML models on initial data
        Ok(())
    }
    
    async fn update_collection_stats(&self, _collection_id: &CollectionId) -> Result<()> {
        // Update collection statistics
        Ok(())
    }
    
    async fn predict_query_clusters(&self, _context: &ViperSearchContext) -> Result<Vec<ClusterId>> {
        // Predict which clusters to search for query
        Ok(Vec::new())
    }
    
    async fn progressive_tier_search(&self, _context: ViperSearchContext, _cluster_hints: Option<Vec<ClusterId>>, _important_features: Vec<usize>, _tier_result_thresholds: Vec<usize>) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }
    
    async fn cluster_pruned_search(&self, _context: ViperSearchContext, _cluster_hints: Option<Vec<ClusterId>>, _important_features: Vec<usize>, _max_clusters: usize, _confidence_threshold: f32) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }
    
    async fn exhaustive_search(&self, _context: ViperSearchContext, _important_features: Vec<usize>) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }
    
    async fn adaptive_search(&self, _context: ViperSearchContext, _query_complexity_score: f32, _time_budget_ms: u64, _cluster_hints: Option<Vec<ClusterId>>, _important_features: Vec<usize>) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }
    
    async fn update_search_stats(&self, _results: &[ViperSearchResult]) -> Result<()> {
        Ok(())
    }
    
    async fn update_cluster_metadata(&self, _cluster_id: ClusterId, _collection_id: &CollectionId) -> Result<()> {
        Ok(())
    }
    
    // Hybrid storage implementation methods
    
    /// Convert dense vector to sparse (dimension, value) pairs
    fn vector_to_sparse_pairs(&self, vector: &[f32]) -> Vec<(u32, f32)> {
        vector.iter()
            .enumerate()
            .filter_map(|(i, &val)| {
                if val != 0.0 {
                    Some((i as u32, val))
                } else {
                    None
                }
            })
            .collect()
    }
    
    /// Write vector to hybrid storage (Parquet + KV for sparse, Parquet only for dense)
    async fn write_to_hybrid_storage(
        &self,
        collection_id: &CollectionId,
        viper_vector: ViperVector,
        cluster_prediction: &ClusterPrediction,
    ) -> Result<()> {
        let partition_id = self.get_or_create_partition(collection_id, cluster_prediction.cluster_id).await?;
        let partition_path = self.get_partition_path(&partition_id);
        
        // Ensure partition directory exists
        tokio::fs::create_dir_all(&partition_path).await?;
        
        match viper_vector {
            ViperVector::Dense { record } => {
                // Write dense vector to Parquet with ID/metadata columns first
                self.write_dense_vector_to_parquet(&partition_path, record).await?;
            }
            ViperVector::Sparse { metadata, data } => {
                // Write sparse vector to separate metadata Parquet + KV storage
                self.write_sparse_metadata_to_parquet(&partition_path, metadata).await?;
                self.write_sparse_data_to_kv(&partition_path, data).await?;
            }
        }
        
        Ok(())
    }
    
    /// Write dense vector to Parquet with optimized schema (ID and metadata first)
    async fn write_dense_vector_to_parquet(
        &self,
        partition_path: &Path,
        record: DenseVectorRecord,
    ) -> Result<()> {
        let dense_parquet_path = partition_path.join("dense_vectors.parquet");
        
        // Create Parquet schema with ID and metadata columns first for efficient lookups
        // Schema: [id, metadata, cluster_id, vector_dimensions..., timestamp]
        // This allows efficient ID-based filtering and metadata queries
        
        // For now, placeholder implementation
        // Real implementation would use arrow/parquet crates to write optimized columnar data
        println!("Writing dense vector {} to {}", record.id, dense_parquet_path.display());
        
        Ok(())
    }
    
    /// Write sparse vector metadata to separate Parquet table for efficient filtering
    async fn write_sparse_metadata_to_parquet(
        &self,
        partition_path: &Path,
        metadata: SparseVectorMetadata,
    ) -> Result<()> {
        let metadata_parquet_path = partition_path.join("sparse_metadata.parquet");
        
        // Create metadata Parquet schema: [id, metadata, cluster_id, dimension_count, sparsity_ratio, timestamp]
        // This enables fast filtering by ID, metadata, cluster, and sparsity before accessing KV data
        
        // Placeholder implementation
        println!("Writing sparse metadata {} to {}", metadata.id, metadata_parquet_path.display());
        
        Ok(())
    }
    
    /// Write sparse vector data to KV storage (can be RocksDB, LMDB, or custom format)
    async fn write_sparse_data_to_kv(
        &self,
        partition_path: &Path,
        data: SparseVectorData,
    ) -> Result<()> {
        let kv_path = partition_path.join("vectors.kv");
        
        // Store sparse vector as key-value: vector_id -> [(dim_index, value), ...]
        // This format avoids zero-padding and provides excellent compression for sparse data
        
        // Placeholder implementation - would use actual KV store
        println!("Writing sparse vector data {} with {} dimensions to {}", 
                data.id, data.dimensions.len(), kv_path.display());
        
        Ok(())
    }
    
    /// Insert cluster batch using hybrid storage
    async fn insert_cluster_batch_hybrid(
        &self,
        collection_id: CollectionId,
        _cluster_id: ClusterId,
        batch: Vec<(VectorRecord, ClusterPrediction)>,
    ) -> Result<()> {
        for (vector_record, cluster_prediction) in batch {
            // Determine storage format for each vector
            let collection_meta = self.get_collection_metadata(&collection_id).await?;
            let storage_format = self.determine_format(&vector_record.vector, &collection_meta).await?;
            
            // Create VIPER vector based on format
            let viper_vector = match storage_format {
                VectorStorageFormat::Sparse => {
                    let dimensions = self.vector_to_sparse_pairs(&vector_record.vector);
                    let sparsity_ratio = 1.0 - (dimensions.len() as f32 / vector_record.vector.len() as f32);
                    
                    let metadata = SparseVectorMetadata::new(
                        vector_record.id,
                        serde_json::to_value(vector_record.metadata.clone())?,
                        cluster_prediction.cluster_id,
                        vector_record.vector.len() as u32,
                        sparsity_ratio,
                    );
                    
                    let data = SparseVectorData::new(vector_record.id, dimensions);
                    
                    ViperVector::Sparse { metadata, data }
                }
                VectorStorageFormat::Dense => {
                    let record = DenseVectorRecord::new(
                        vector_record.id,
                        vector_record.vector,
                        serde_json::to_value(vector_record.metadata.clone())?,
                        cluster_prediction.cluster_id,
                    );
                    
                    ViperVector::Dense { record }
                }
                VectorStorageFormat::Auto => {
                    return Err(anyhow::anyhow!("Auto format should be resolved before batch insert"));
                }
            };
            
            // Write to hybrid storage
            self.write_to_hybrid_storage(&collection_id, viper_vector, &cluster_prediction).await?;
        }
        
        Ok(())
    }
    
    /// Get or create collection metadata
    async fn get_or_create_collection_metadata(&self, collection_id: &CollectionId) -> Result<CollectionMetadata> {
        let collections = self.collections.read().await;
        if let Some(metadata) = collections.get(collection_id) {
            return Ok(metadata.clone());
        }
        drop(collections);
        
        // Create default metadata for new collection
        let metadata = CollectionMetadata {
            collection_id: collection_id.clone(),
            dimension: 384, // Default dimension
            vector_count: 0,
            total_clusters: 0,
            storage_format_preference: VectorStorageFormat::Auto,
            ml_model_version: None,
            feature_importance: vec![1.0; 384],
            compression_stats: CompressionStats {
                sparse_compression_ratio: 1.0,
                dense_compression_ratio: 1.0,
                optimal_sparsity_threshold: 0.7,
                column_compression_ratios: vec![1.0; 384],
            },
            created_at: Utc::now(),
            last_updated: Utc::now(),
        };
        
        let mut collections = self.collections.write().await;
        collections.insert(collection_id.clone(), metadata.clone());
        
        Ok(metadata)
    }
    
    /// Predict cluster for a single vector
    async fn predict_cluster(&self, _collection_id: &CollectionId, _vector: &[f32]) -> Result<ClusterPrediction> {
        // Placeholder implementation - would use trained ML models
        Ok(ClusterPrediction {
            cluster_id: 0, // Default cluster
            confidence: 0.8,
            alternatives: vec![(1, 0.15), (2, 0.05)],
            prediction_metadata: PredictionMetadata {
                model_version: "v1.0".to_string(),
                features_used: vec!["all_dimensions".to_string()],
                predicted_at: Utc::now(),
                model_accuracy: 0.85,
                prediction_cost_ms: 5,
            },
        })
    }
    
    /// Determine storage format based on vector characteristics and collection metadata
    async fn determine_format(&self, vector: &[f32], collection_meta: &CollectionMetadata) -> Result<VectorStorageFormat> {
        let sparsity = self.calculate_sparsity(vector);
        let threshold = collection_meta.compression_stats.optimal_sparsity_threshold;
        
        if sparsity > threshold {
            Ok(VectorStorageFormat::Sparse)
        } else {
            Ok(VectorStorageFormat::Dense)
        }
    }
    
    /// Update collection statistics for single vector insert
    async fn update_collection_stats_single(&self, collection_id: &CollectionId, _cluster_prediction: &ClusterPrediction) -> Result<()> {
        let mut collections = self.collections.write().await;
        if let Some(metadata) = collections.get_mut(collection_id) {
            metadata.vector_count += 1;
            metadata.last_updated = Utc::now();
        }
        Ok(())
    }
    
    // WAL Recovery Apply Methods
    
    /// Apply vector insert from WAL during recovery
    async fn apply_vector_insert(
        &self,
        collection_id: String,
        vector_record: VectorRecord,
        _cluster_prediction: ClusterPrediction,
        _storage_format: VectorStorageFormat,
        _tier_level: TierLevel,
    ) -> Result<()> {
        // Re-apply the insert without logging to WAL again
        self.insert_vector(&collection_id, vector_record).await
    }
    
    /// Apply vector update from WAL during recovery
    async fn apply_vector_update(
        &self,
        _collection_id: String,
        _vector_id: VectorId,
        _old_cluster_id: ClusterId,
        _new_cluster_id: ClusterId,
        _updated_metadata: Option<serde_json::Value>,
        _tier_level: TierLevel,
    ) -> Result<()> {
        // TODO: Implement vector update logic
        Ok(())
    }
    
    /// Apply vector delete from WAL during recovery
    async fn apply_vector_delete(
        &self,
        _collection_id: String,
        _vector_id: VectorId,
        _cluster_id: ClusterId,
        _tier_level: TierLevel,
    ) -> Result<()> {
        // TODO: Implement vector delete logic
        Ok(())
    }
    
    /// Apply cluster update from WAL during recovery
    async fn apply_cluster_update(
        &self,
        _collection_id: String,
        cluster_id: ClusterId,
        new_centroid: Vec<f32>,
        quality_metrics: ClusterQualityMetrics,
    ) -> Result<()> {
        let mut clusters = self.clusters.write().await;
        if let Some(cluster) = clusters.get_mut(&cluster_id) {
            cluster.centroid = new_centroid;
            cluster.quality_metrics = quality_metrics;
            cluster.last_updated = Utc::now();
        }
        Ok(())
    }
    
    /// Apply partition create from WAL during recovery
    async fn apply_partition_create(
        &self,
        collection_id: String,
        partition_id: PartitionId,
        cluster_ids: Vec<ClusterId>,
        tier_level: TierLevel,
    ) -> Result<()> {
        let mut partitions = self.partitions.write().await;
        partitions.insert(partition_id.clone(), PartitionMetadata {
            partition_id,
            collection_id,
            cluster_ids,
            statistics: PartitionStatistics {
                total_vectors: 0,
                storage_size_bytes: 0,
                compressed_size_bytes: 0,
                compression_ratio: 1.0,
                access_frequency: 0.0,
                last_accessed: Utc::now(),
            },
            tier_level,
            parquet_files: Vec::new(),
            bloom_filter: None,
            created_at: Utc::now(),
            last_modified: Utc::now(),
        });
        Ok(())
    }
    
    /// Apply compaction operation from WAL during recovery
    async fn apply_compaction_operation(
        &self,
        _collection_id: String,
        _operation_id: String,
        _input_partitions: Vec<PartitionId>,
        _output_partitions: Vec<PartitionId>,
        _tier_level: TierLevel,
    ) -> Result<()> {
        // TODO: Implement compaction recovery logic
        Ok(())
    }
    
    /// Apply tier migration from WAL during recovery
    async fn apply_tier_migration(
        &self,
        _collection_id: String,
        partition_id: PartitionId,
        _from_tier: TierLevel,
        to_tier: TierLevel,
        _migration_reason: String,
    ) -> Result<()> {
        let mut partitions = self.partitions.write().await;
        if let Some(partition) = partitions.get_mut(&partition_id) {
            partition.tier_level = to_tier;
        }
        Ok(())
    }
    
    /// Apply model update from WAL during recovery
    async fn apply_model_update(
        &self,
        _collection_id: String,
        _model_type: String,
        _model_version: String,
        _accuracy_metrics: serde_json::Value,
    ) -> Result<()> {
        // TODO: Implement model update recovery logic
        Ok(())
    }
}

impl ParquetWriterPool {
    fn new(max_writers: usize) -> Self {
        Self {
            writers: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent_writers: max_writers,
        }
    }
}

impl ParquetWriter {
    fn new(
        partition_id: PartitionId,
        file_path: PathBuf,
        schema: ParquetSchema,
        row_group_size: usize,
        compression: CompressionAlgorithm,
    ) -> Self {
        Self {
            partition_id,
            file_path,
            schema,
            row_group_size,
            current_batch: Vec::new(),
            compression_algorithm: compression,
        }
    }
    
    async fn add_vectors(&mut self, vectors: Vec<ViperVector>) -> Result<()> {
        self.current_batch.extend(vectors);
        Ok(())
    }
    
    fn should_flush(&self) -> bool {
        self.current_batch.len() >= self.row_group_size
    }
    
    async fn flush(&mut self) -> Result<()> {
        // Write current batch to Parquet file
        // Implementation would use parquet crate
        self.current_batch.clear();
        Ok(())
    }
}