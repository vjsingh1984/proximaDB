// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Storage Engine Implementation
//!
//! ⚠️  DEPRECATED: This file has been consolidated into the unified vector storage system.
//! 🔄  MIGRATED TO: `src/storage/vector/engines/viper_core.rs`
//! 📅  DEPRECATION DATE: Phase 4.1 consolidation
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

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::ttl::TTLCleanupService;
use super::types::*;
use super::ViperConfig;
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::core::CompressionAlgorithm;

/// Calculate sparsity ratio for a vector (standalone function)
fn calculate_sparsity_ratio(vector: &[f32]) -> f32 {
    let zero_count = vector.iter().filter(|&&x| x == 0.0).count();
    zero_count as f32 / vector.len() as f32
}

/// VIPER Storage Engine with ML-driven clustering and Parquet optimization
pub struct ViperStorageEngine {
    /// Configuration
    config: ViperConfig,

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

    /// TTL cleanup service
    ttl_service: Option<Arc<RwLock<super::ttl::TTLCleanupService>>>,

    /// Atomic operations factory for flush/compaction
    atomic_operations: Arc<super::atomic_operations::AtomicOperationsFactory>,
}

/// DEPRECATED: Collection metadata in VIPER
/// ⚠️  This struct is OBSOLETE - use crate::core::avro_unified::Collection instead
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
    pub model_data: Vec<u8>,       // Serialized model (e.g., ONNX)
    pub training_timestamp: DateTime<Utc>,
    pub prediction_stats: PredictionStats,
}

/// Feature importance model for column selection optimization
#[derive(Debug, Clone)]
pub struct FeatureImportanceModel {
    pub model_id: String,
    pub importance_scores: Vec<f32>, // Score per dimension (0.0 to 1.0)
    pub top_k_features: Vec<usize>,  // Most important dimension indices
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
    file_url: String,
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
        dimension_key_column: String, // "dim_key"
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
    pub total_flushes: u64,
    pub total_compactions: u64,
    pub total_vectors_flushed: u64,
}

impl ViperStorageEngine {
    /// Create a new VIPER storage engine with unified WAL
    pub async fn new(config: ViperConfig, filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        let compaction_manager =
            Arc::new(super::compaction::ViperCompactionEngine::new(config.clone()).await?);

        let writer_pool = Arc::new(ParquetWriterPool::new(8)); // Default to 8 threads

        let partitions = Arc::new(RwLock::new(HashMap::new()));

        // Initialize TTL service if enabled
        let ttl_service = if config.ttl_config.enabled {
            let mut service = TTLCleanupService::new(
                config.ttl_config.clone(),
                filesystem.clone(),
                partitions.clone(),
            );
            service.start().await?;
            Some(Arc::new(RwLock::new(service)))
        } else {
            None
        };

        // Initialize atomic operations factory
        let atomic_operations = Arc::new(super::atomic_operations::AtomicOperationsFactory::new(
            filesystem.clone(),
        ));

        Ok(Self {
            config,
            collections: Arc::new(RwLock::new(HashMap::new())),
            clusters: Arc::new(RwLock::new(HashMap::new())),
            partitions,
            ml_models: Arc::new(RwLock::new(HashMap::new())),
            feature_models: Arc::new(RwLock::new(HashMap::new())),
            compaction_manager,
            writer_pool,
            stats: Arc::new(RwLock::new(ViperStorageStats::default())),
            ttl_service,
            atomic_operations,
        })
    }

    /// Recover VIPER state from unified WAL
    pub async fn recover_from_wal(&self) -> Result<()> {
        // WAL recovery is now handled by the WAL manager internally
        // VIPER will rebuild its indexes and clustering from the standard WAL operations
        tracing::info!("VIPER recovery delegated to WAL manager");
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
            return Err(anyhow::anyhow!(
                "Collection already exists: {}",
                collection_id
            ));
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
            self.insert_vectors_batch(collection_id.clone(), vectors)
                .await?;
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
        tracing::info!(
            "🔹 VIPER: insert_vector called for collection: {}, vector_id: {}",
            collection_id,
            vector_record.id
        );

        // Get collection metadata or create if new
        let collection_meta = self
            .get_or_create_collection_metadata(collection_id)
            .await?;
        tracing::debug!("📄 VIPER: Got collection metadata for {}", collection_id);

        // Predict cluster assignment using ML model
        let cluster_prediction = self
            .predict_cluster(collection_id, &vector_record.vector)
            .await?;
        tracing::debug!(
            "🎯 VIPER: Predicted cluster {} for vector {}",
            cluster_prediction.cluster_id,
            vector_record.id
        );

        // Determine storage format (sparse vs dense) based on sparsity
        let storage_format = self
            .determine_format(&vector_record.vector, &collection_meta)
            .await?;
        tracing::info!(
            "💾 VIPER: Storage format for vector {}: {:?}",
            vector_record.id,
            storage_format
        );

        // Store vector ID before any moves
        let vector_id = vector_record.id.clone();

        // Write to WAL first for durability using new interface
        // Note: WAL operations are handled by unified engine, not storage engine

        // Create VIPER vector entry based on hybrid storage architecture
        let viper_vector = match storage_format {
            VectorStorageFormat::Sparse => {
                // For sparse vectors: separate metadata Parquet + KV storage
                let dimensions = self.vector_to_sparse_pairs(&vector_record.vector);
                let sparsity_ratio =
                    1.0 - (dimensions.len() as f32 / vector_record.vector.len() as f32);

                let metadata = SparseVectorMetadata::new(
                    vector_record.id.clone(),
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
                return Err(anyhow::anyhow!(
                    "Auto format should have been resolved by determine_format"
                ));
            }
        };

        // Write to appropriate hybrid storage (Parquet + KV for sparse, Parquet for dense)
        tracing::info!("📝 VIPER: Writing vector {} to hybrid storage", vector_id);
        self.write_to_hybrid_storage(collection_id, viper_vector, &cluster_prediction)
            .await?;
        tracing::info!(
            "✅ VIPER: Successfully wrote vector {} to storage",
            vector_id
        );

        // Update collection statistics
        tracing::debug!("📊 VIPER: Updating collection stats for {}", collection_id);
        self.update_collection_stats_single(collection_id, &cluster_prediction)
            .await?;

        tracing::info!(
            "🎉 VIPER: Completed insert_vector for {} in collection {}",
            vector_id,
            collection_id
        );

        Ok(())
    }

    /// Insert a batch of vectors with ML-guided clustering
    pub async fn insert_vectors_batch(
        &self,
        collection_id: CollectionId,
        vectors: Vec<VectorRecord>,
    ) -> Result<Vec<ClusterPrediction>> {
        tracing::info!(
            "📦 VIPER: insert_vectors_batch called for collection: {}, count: {}",
            collection_id,
            vectors.len()
        );
        let _collection = self.get_collection_metadata(&collection_id).await?;

        // Get cluster predictions for the batch
        let predictions = self.predict_clusters(&collection_id, &vectors).await?;

        // Group vectors by predicted clusters for efficient I/O
        let mut cluster_batches: HashMap<ClusterId, Vec<(VectorRecord, ClusterPrediction)>> =
            HashMap::new();

        for (vector, prediction) in vectors.into_iter().zip(predictions.iter().cloned()) {
            cluster_batches
                .entry(prediction.cluster_id)
                .or_insert_with(Vec::new)
                .push((vector, prediction));
        }

        // Process each cluster batch and log to WAL
        for (cluster_id, batch) in cluster_batches {
            // Log operations to unified WAL first
            for (_vector, _prediction) in &batch {
                // Note: WAL operations are handled by unified engine
            }

            // Then perform the actual insertion using hybrid storage
            self.insert_cluster_batch_hybrid(collection_id.clone(), cluster_id, batch)
                .await?;
        }

        // Update collection statistics
        self.update_collection_stats(&collection_id).await?;

        // Trigger background compaction if needed
        self.compaction_manager
            .schedule_compaction_if_needed(&collection_id)
            .await?;

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
            SearchStrategy::Progressive {
                tier_result_thresholds,
            } => {
                self.progressive_tier_search(
                    context,
                    cluster_hints,
                    important_features,
                    tier_result_thresholds,
                )
                .await?
            }

            SearchStrategy::ClusterPruned {
                max_clusters,
                confidence_threshold,
            } => {
                self.cluster_pruned_search(
                    context,
                    cluster_hints,
                    important_features,
                    max_clusters,
                    confidence_threshold,
                )
                .await?
            }

            SearchStrategy::Exhaustive => {
                self.exhaustive_search(context, important_features).await?
            }

            SearchStrategy::Adaptive {
                query_complexity_score,
                time_budget_ms,
            } => {
                self.adaptive_search(
                    context,
                    query_complexity_score,
                    time_budget_ms,
                    cluster_hints,
                    important_features,
                )
                .await?
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
            self.heuristic_cluster_assignment(collection_id, vectors)
                .await
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
            let weighted_features: Vec<f32> = vector
                .vector
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
                    features_used: model
                        .feature_weights
                        .iter()
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
        vector1
            .iter()
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
        let partition_id = self
            .get_or_create_partition(&collection_id, cluster_id)
            .await?;

        // Convert to VIPER vectors with optimal storage format
        let collection = self.get_collection_metadata(&collection_id).await?;
        let viper_vectors = self.convert_to_viper_vectors(batch, &collection).await?;

        // Write to Parquet using optimized columnar layout
        self.write_vectors_to_parquet(&partition_id, viper_vectors)
            .await?;

        // Update cluster and partition metadata
        self.update_cluster_metadata(cluster_id, &collection_id)
            .await?;

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
            if value != 0.0 {
                // Only store non-zero values
                dimensions.insert(idx as u32, value);
            }
        }

        let metadata = SparseVectorMetadata {
            id: vector.id.clone(),
            metadata: serde_json::to_value(vector.metadata)?,
            cluster_id,
            dimension_count: dimensions.len() as u32,
            sparsity_ratio: self.calculate_sparsity(&vector.vector),
            timestamp: chrono::DateTime::from_timestamp_millis(vector.timestamp).unwrap_or_else(|| Utc::now()),
            expires_at: vector.expires_at.map(|ts| chrono::DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now())),
        };
        let dimensions_vec: Vec<(u32, f32)> = dimensions.into_iter().collect();
        let data = SparseVectorData {
            id: vector.id,
            dimensions: dimensions_vec,
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
            timestamp: chrono::DateTime::from_timestamp_millis(vector.timestamp).unwrap_or_else(|| Utc::now()),
            expires_at: vector.expires_at.map(|ts| chrono::DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now())),
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

        let writer = writers.entry(partition_id.clone()).or_insert_with(|| {
            ParquetWriter::new(
                partition_id.clone(),
                self.get_partition_url(partition_id),
                self.create_optimized_schema(&vectors),
                self.config.row_group_size,
                CompressionAlgorithm::Snappy,
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
    async fn get_collection_metadata(
        &self,
        collection_id: &CollectionId,
    ) -> Result<CollectionMetadata> {
        let collections = self.collections.read().await;
        collections
            .get(collection_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))
    }

    async fn get_or_create_partition(
        &self,
        collection_id: &CollectionId,
        cluster_id: ClusterId,
    ) -> Result<PartitionId> {
        // Implementation for partition management
        Ok(format!("{}_{}", collection_id, cluster_id))
    }

    fn get_partition_url(&self, partition_id: &PartitionId) -> String {
        format!("viper/partitions/{}", partition_id)
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
            let has_sparse = vectors
                .iter()
                .any(|v| matches!(v, ViperVector::Sparse { .. }));
            let has_dense = vectors
                .iter()
                .any(|v| matches!(v, ViperVector::Dense { .. }));

            match (has_sparse, has_dense) {
                (true, false) => VectorColumns::Sparse {
                    metadata_table_ref: Some("metadata_index".to_string()),
                    dimension_key_column: "dim_key".to_string(),
                    dimension_value_column: "dim_value".to_string(),
                },
                (false, true) => {
                    // Determine dimension from first dense vector
                    let dimension = vectors
                        .iter()
                        .find_map(|v| match v {
                            ViperVector::Dense { record } => Some(record.vector.len()),
                            _ => None,
                        })
                        .unwrap_or(384);

                    VectorColumns::Dense {
                        dimension_columns: (0..dimension).map(|i| format!("dim_{}", i)).collect(),
                    }
                }
                (true, true) => {
                    // Hybrid format
                    let dimension = vectors
                        .iter()
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
                }
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
    async fn heuristic_cluster_assignment(
        &self,
        _collection_id: &CollectionId,
        _vectors: &[VectorRecord],
    ) -> Result<Vec<ClusterPrediction>> {
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

    async fn predict_query_clusters(
        &self,
        _context: &ViperSearchContext,
    ) -> Result<Vec<ClusterId>> {
        // Predict which clusters to search for query
        Ok(Vec::new())
    }

    async fn progressive_tier_search(
        &self,
        _context: ViperSearchContext,
        _cluster_hints: Option<Vec<ClusterId>>,
        _important_features: Vec<usize>,
        _tier_result_thresholds: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }

    async fn cluster_pruned_search(
        &self,
        _context: ViperSearchContext,
        _cluster_hints: Option<Vec<ClusterId>>,
        _important_features: Vec<usize>,
        _max_clusters: usize,
        _confidence_threshold: f32,
    ) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }

    async fn exhaustive_search(
        &self,
        _context: ViperSearchContext,
        _important_features: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }

    async fn adaptive_search(
        &self,
        _context: ViperSearchContext,
        _query_complexity_score: f32,
        _time_budget_ms: u64,
        _cluster_hints: Option<Vec<ClusterId>>,
        _important_features: Vec<usize>,
    ) -> Result<Vec<ViperSearchResult>> {
        Ok(Vec::new())
    }

    async fn update_search_stats(&self, _results: &[ViperSearchResult]) -> Result<()> {
        Ok(())
    }

    async fn update_cluster_metadata(
        &self,
        _cluster_id: ClusterId,
        _collection_id: &CollectionId,
    ) -> Result<()> {
        Ok(())
    }

    // Hybrid storage implementation methods

    /// Convert dense vector to sparse (dimension, value) pairs
    fn vector_to_sparse_pairs(&self, vector: &[f32]) -> Vec<(u32, f32)> {
        vector
            .iter()
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
        let partition_id = self
            .get_or_create_partition(collection_id, cluster_prediction.cluster_id)
            .await?;
        let partition_url = self.get_partition_url(&partition_id);

        // Note: Directory creation handled by filesystem API

        match viper_vector {
            ViperVector::Dense { record } => {
                // Write dense vector to Parquet with ID/metadata columns first
                self.write_dense_vector_to_parquet(&partition_url, record)
                    .await?;
            }
            ViperVector::Sparse { metadata, data } => {
                // Write sparse vector to separate metadata Parquet + KV storage
                self.write_sparse_metadata_to_parquet(&partition_url, metadata)
                    .await?;
                self.write_sparse_data_to_kv(&partition_url, data).await?;
            }
        }

        Ok(())
    }

    /// Write dense vector to Parquet with optimized schema (ID and metadata first)
    async fn write_dense_vector_to_parquet(
        &self,
        partition_url: &str,
        record: DenseVectorRecord,
    ) -> Result<()> {
        let dense_parquet_url = format!("{}/dense_vectors.parquet", partition_url);

        // Create Parquet schema with ID and metadata columns first for efficient lookups
        // Schema: [id, metadata, cluster_id, vector_dimensions..., timestamp]
        // This allows efficient ID-based filtering and metadata queries

        // For now, placeholder implementation
        // Real implementation would use arrow/parquet crates to write optimized columnar data
        println!(
            "Writing dense vector {} to {}",
            record.id, dense_parquet_url
        );

        Ok(())
    }

    /// Write sparse vector metadata to separate Parquet table for efficient filtering
    async fn write_sparse_metadata_to_parquet(
        &self,
        partition_url: &str,
        metadata: SparseVectorMetadata,
    ) -> Result<()> {
        let metadata_parquet_url = format!("{}/sparse_metadata.parquet", partition_url);

        // Create metadata Parquet schema: [id, metadata, cluster_id, dimension_count, sparsity_ratio, timestamp]
        // This enables fast filtering by ID, metadata, cluster, and sparsity before accessing KV data

        // Placeholder implementation
        println!(
            "Writing sparse metadata {} to {}",
            metadata.id, metadata_parquet_url
        );

        Ok(())
    }

    /// Write sparse vector data to KV storage (can be RocksDB, LMDB, or custom format)
    async fn write_sparse_data_to_kv(
        &self,
        partition_url: &str,
        data: SparseVectorData,
    ) -> Result<()> {
        let kv_url = format!("{}/vectors.kv", partition_url);

        // Store sparse vector as key-value: vector_id -> [(dim_index, value), ...]
        // This format avoids zero-padding and provides excellent compression for sparse data

        // Placeholder implementation - would use actual KV store
        println!(
            "Writing sparse vector data {} with {} dimensions to {}",
            data.id,
            data.dimensions.len(),
            kv_url
        );

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
            let storage_format = self
                .determine_format(&vector_record.vector, &collection_meta)
                .await?;

            // Create VIPER vector based on format
            let viper_vector = match storage_format {
                VectorStorageFormat::Sparse => {
                    let dimensions = self.vector_to_sparse_pairs(&vector_record.vector);
                    let sparsity_ratio =
                        1.0 - (dimensions.len() as f32 / vector_record.vector.len() as f32);

                    let metadata = SparseVectorMetadata::new(
                        vector_record.id.clone(),
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
                    return Err(anyhow::anyhow!(
                        "Auto format should be resolved before batch insert"
                    ));
                }
            };

            // Write to hybrid storage
            self.write_to_hybrid_storage(&collection_id, viper_vector, &cluster_prediction)
                .await?;
        }

        Ok(())
    }

    /// Get or create collection metadata
    async fn get_or_create_collection_metadata(
        &self,
        collection_id: &CollectionId,
    ) -> Result<CollectionMetadata> {
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
    async fn predict_cluster(
        &self,
        _collection_id: &CollectionId,
        _vector: &[f32],
    ) -> Result<ClusterPrediction> {
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
    async fn determine_format(
        &self,
        vector: &[f32],
        collection_meta: &CollectionMetadata,
    ) -> Result<VectorStorageFormat> {
        let sparsity = self.calculate_sparsity(vector);
        let threshold = collection_meta.compression_stats.optimal_sparsity_threshold;

        if sparsity > threshold {
            Ok(VectorStorageFormat::Sparse)
        } else {
            Ok(VectorStorageFormat::Dense)
        }
    }

    /// Update collection statistics for single vector insert
    async fn update_collection_stats_single(
        &self,
        collection_id: &CollectionId,
        _cluster_prediction: &ClusterPrediction,
    ) -> Result<()> {
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
        _storage_url: &str,
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
        _storage_url: &str,
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
        _storage_url: &str,
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
        storage_url: &str,
    ) -> Result<()> {
        let mut partitions = self.partitions.write().await;
        partitions.insert(
            partition_id.clone(),
            PartitionMetadata {
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
                storage_url: storage_url.to_string(),
                parquet_files: Vec::new(),
                bloom_filter: None,
                created_at: Utc::now(),
                last_modified: Utc::now(),
            },
        );
        Ok(())
    }

    /// Apply compaction operation from WAL during recovery
    async fn apply_compaction_operation(
        &self,
        _collection_id: String,
        _operation_id: String,
        _input_partitions: Vec<PartitionId>,
        _output_partitions: Vec<PartitionId>,
        _storage_url: &str,
    ) -> Result<()> {
        // TODO: Implement compaction recovery logic
        Ok(())
    }

    /// Apply tier migration from WAL during recovery
    async fn apply_tier_migration(
        &self,
        _collection_id: String,
        partition_id: PartitionId,
        _from_storage_url: &str,
        to_storage_url: &str,
        _migration_reason: String,
    ) -> Result<()> {
        let mut partitions = self.partitions.write().await;
        if let Some(partition) = partitions.get_mut(&partition_id) {
            partition.storage_url = to_storage_url.to_string();
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

    /// Get TTL cleanup statistics
    pub async fn get_ttl_stats(&self) -> Result<Option<super::ttl::TTLStats>> {
        if let Some(ttl_service) = &self.ttl_service {
            let service = ttl_service.read().await;
            Ok(Some(service.get_stats().await))
        } else {
            Ok(None)
        }
    }

    /// Force TTL cleanup (for admin/testing purposes)
    pub async fn force_ttl_cleanup(&self) -> Result<Option<super::ttl::CleanupResult>> {
        if let Some(ttl_service) = &self.ttl_service {
            let service = ttl_service.read().await;
            Ok(Some(service.force_cleanup().await?))
        } else {
            Ok(None)
        }
    }

    /// VIPER Phase 5: Two-phase search with WAL + Storage integration
    pub async fn search_two_phase(
        &self,
        collection_id: &CollectionId,
        query_vector: Vec<f32>,
        k: usize,
        filters: Option<HashMap<String, serde_json::Value>>,
        threshold: Option<f32>,
    ) -> Result<Vec<super::types::ViperSearchResult>> {
        tracing::info!(
            "🚀 VIPER two-phase search initiated for collection: {}",
            collection_id
        );

        // Create search engine if needed
        let search_engine =
            super::search_engine::ViperProgressiveSearchEngine::new(self.config.clone()).await?;

        // Build search context
        let search_context = super::types::ViperSearchContext {
            collection_id: collection_id.clone(),
            query_vector,
            k,
            threshold,
            filters,
            cluster_hints: None,
            search_strategy: super::types::SearchStrategy::Adaptive {
                query_complexity_score: 0.5,
                time_budget_ms: 1000,
            },
            max_storage_locations: Some(10),
        };

        // Execute two-phase search with WAL integration
        let results = search_engine.search_two_phase(search_context).await?;

        tracing::info!(
            "✅ VIPER two-phase search completed: {} results returned",
            results.len()
        );

        Ok(results)
    }

    /// VIPER search without WAL integration (storage-only)
    pub async fn search_storage_only(
        &self,
        collection_id: &CollectionId,
        query_vector: Vec<f32>,
        k: usize,
        filters: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<Vec<super::types::ViperSearchResult>> {
        tracing::debug!(
            "🗄️ VIPER storage-only search for collection: {}",
            collection_id
        );

        let search_engine =
            super::search_engine::ViperProgressiveSearchEngine::new(self.config.clone()).await?;

        let search_context = super::types::ViperSearchContext {
            collection_id: collection_id.clone(),
            query_vector,
            k,
            threshold: None,
            filters,
            cluster_hints: None,
            search_strategy: super::types::SearchStrategy::Progressive {
                tier_result_thresholds: vec![k * 2, k * 5, k * 10],
            },
            max_storage_locations: Some(5),
        };

        // Use existing progressive search (no WAL)
        let results = search_engine.search(search_context, None, vec![]).await?;

        tracing::debug!(
            "💾 Storage-only search completed: {} results",
            results.len()
        );

        Ok(results)
    }

    /// Get search statistics from the VIPER engine
    pub async fn get_search_stats(&self) -> Result<super::search_engine::SearchStats> {
        // For now, return default stats
        // Future: integrate with actual search engine statistics
        Ok(super::search_engine::SearchStats::default())
    }

    /// VIPER Phase 6: Build HNSW index for a collection
    pub async fn build_hnsw_index(&self, collection_id: &CollectionId) -> Result<()> {
        tracing::info!("🔨 Building HNSW index for collection: {}", collection_id);

        // Gather vectors from storage for index building
        let vectors = self.load_collection_vectors(collection_id).await?;

        if vectors.is_empty() {
            tracing::warn!(
                "⚠️ No vectors found for collection {}, skipping index build",
                collection_id
            );
            return Ok(());
        }

        // Create search engine and build index
        let search_engine =
            super::search_engine::ViperProgressiveSearchEngine::new(self.config.clone()).await?;

        // Default to cosine distance for now
        // Future: make this configurable per collection
        let distance_metric = crate::compute::distance::DistanceMetric::Cosine;

        search_engine
            .build_hnsw_index(collection_id, vectors, distance_metric)
            .await?;

        tracing::info!(
            "✅ HNSW index built successfully for collection {}",
            collection_id
        );

        Ok(())
    }

    /// VIPER Phase 6: Search with HNSW and metadata filtering
    pub async fn search_with_hnsw(
        &self,
        collection_id: &CollectionId,
        query_vector: Vec<f32>,
        k: usize,
        filters: Option<HashMap<String, serde_json::Value>>,
        threshold: Option<f32>,
    ) -> Result<Vec<super::types::ViperSearchResult>> {
        tracing::info!(
            "🚀 VIPER HNSW search initiated for collection: {}",
            collection_id
        );

        let search_engine =
            super::search_engine::ViperProgressiveSearchEngine::new(self.config.clone()).await?;

        let search_context = super::types::ViperSearchContext {
            collection_id: collection_id.clone(),
            query_vector,
            k,
            threshold,
            filters,
            cluster_hints: None,
            search_strategy: super::types::SearchStrategy::Adaptive {
                query_complexity_score: 0.5,
                time_budget_ms: 500, // Faster for HNSW
            },
            max_storage_locations: Some(5),
        };

        // Use filtered HNSW search
        let results = search_engine
            .search_filtered_with_hnsw(search_context)
            .await?;

        tracing::info!(
            "✅ VIPER HNSW search completed: {} results returned",
            results.len()
        );

        Ok(results)
    }

    /// Load vectors from storage for index building
    async fn load_collection_vectors(
        &self,
        _collection_id: &CollectionId,
    ) -> Result<Vec<(String, Vec<f32>, serde_json::Value)>> {
        // This would load vectors from Parquet storage
        // For now, return empty vector as placeholder
        tracing::warn!("⚠️ Vector loading from storage not yet implemented");
        Ok(Vec::new())
    }

    /// Trigger HNSW index rebuild after compaction
    pub async fn rebuild_hnsw_after_compaction(&self, collection_id: &CollectionId) -> Result<()> {
        tracing::info!(
            "🔄 Rebuilding HNSW index after compaction for collection: {}",
            collection_id
        );

        // In a real implementation:
        // 1. Lock the collection for writes
        // 2. Build new index in background
        // 3. Atomically swap indexes
        // 4. Unlock collection

        self.build_hnsw_index(collection_id).await?;

        Ok(())
    }

    /// Perform atomic flush using staging directory
    pub async fn atomic_flush_collection(
        &self,
        collection_id: &CollectionId,
        records: Vec<VectorRecord>,
        wal_entries: Vec<String>,
    ) -> Result<String> {
        tracing::info!(
            "🚀 VIPER FLUSH: Starting atomic flush for collection {} with {} records",
            collection_id,
            records.len()
        );
        tracing::debug!(
            "📃 VIPER FLUSH: WAL entries to clean: {}",
            wal_entries.len()
        );

        let records_count = records.len();
        let start_time = std::time::Instant::now();

        tracing::debug!("🔧 VIPER FLUSH: Creating atomic flusher");
        let flusher = self.atomic_operations.create_flusher();

        tracing::info!("💾 VIPER FLUSH: Executing atomic flush operation");
        let result = flusher
            .atomic_flush(collection_id, records, wal_entries)
            .await?;

        let flush_duration = start_time.elapsed();
        tracing::info!(
            "✅ VIPER FLUSH: Completed flush in {:?}, output: {}",
            flush_duration,
            result
        );

        // Update collection statistics
        tracing::debug!("📊 VIPER FLUSH: Updating statistics");
        let mut stats = self.stats.write().await;
        stats.total_flushes += 1;
        stats.total_vectors_flushed += records_count as u64;

        tracing::info!(
            "🎉 VIPER FLUSH: Successfully flushed {} records to {}",
            records_count,
            result
        );

        Ok(result)
    }

    /// Perform atomic compaction using staging directory
    pub async fn atomic_compact_collection(
        &self,
        collection_id: &CollectionId,
        source_files: Vec<String>,
    ) -> Result<String> {
        tracing::info!(
            "🔄 VIPER COMPACTION: Starting atomic compaction for collection {} with {} files",
            collection_id,
            source_files.len()
        );
        tracing::debug!("📂 VIPER COMPACTION: Source files: {:?}", source_files);

        let start_time = std::time::Instant::now();

        tracing::debug!("🔧 VIPER COMPACTION: Creating atomic compactor");
        let compactor = self.atomic_operations.create_compactor();
        tracing::info!("💾 VIPER COMPACTION: Executing atomic compaction");
        let result = compactor
            .atomic_compact(collection_id, source_files)
            .await?;

        let compaction_duration = start_time.elapsed();
        tracing::info!(
            "✅ VIPER COMPACTION: Completed compaction in {:?}, output: {}",
            compaction_duration,
            result
        );

        // Rebuild HNSW index after successful compaction
        tracing::info!("🏗️ VIPER COMPACTION: Rebuilding HNSW index after compaction");
        self.rebuild_hnsw_after_compaction(collection_id).await?;
        tracing::info!("✅ VIPER COMPACTION: HNSW index rebuilt successfully");

        // Update collection statistics
        tracing::debug!("📊 VIPER COMPACTION: Updating statistics");
        let mut stats = self.stats.write().await;
        stats.total_compactions += 1;

        tracing::info!(
            "🎉 VIPER COMPACTION: Successfully compacted collection {} to {}",
            collection_id,
            result
        );

        Ok(result)
    }

    /// Get collection read lock for queries
    pub async fn acquire_read_lock(
        &self,
        collection_id: &CollectionId,
    ) -> Result<super::atomic_operations::ReadLockGuard> {
        self.atomic_operations
            .lock_manager()
            .acquire_read_lock(collection_id)
            .await
    }

    /// Check if collection is available for writes (no exclusive locks)
    pub async fn is_available_for_writes(&self, _collection_id: &CollectionId) -> bool {
        // WAL and memtable are always available for writes during flush/compaction
        // Only the storage files are locked
        true
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
        file_url: String,
        schema: ParquetSchema,
        row_group_size: usize,
        compression: CompressionAlgorithm,
    ) -> Self {
        Self {
            partition_id,
            file_url,
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
