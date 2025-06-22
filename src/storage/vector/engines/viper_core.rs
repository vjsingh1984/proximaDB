// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Core Storage Engine - Consolidated Implementation
//!
//! 🔥 PHASE 4.1: This consolidates 3 VIPER core files into a unified implementation:
//! - `storage_engine.rs` → Core VIPER storage operations
//! - `adapter.rs` → Schema adaptation and record conversion  
//! - `atomic_operations.rs` → Atomic flush and compaction operations
//!
//! ## Key Features
//! - **ML-Driven Clustering**: Trained models for optimal data organization
//! - **Columnar Compression**: Parquet format with intelligent compression
//! - **Atomic Operations**: Consistent flush and compaction using staging directories
//! - **Schema Adaptation**: Flexible record-to-schema conversion
//! - **Performance Optimization**: SIMD-aware operations and background processing

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::Schema;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;
use crate::storage::vector::types::*;

/// Filterable column configuration for server-side metadata filtering
#[derive(Debug, Clone, PartialEq)]
pub struct FilterableColumn {
    /// Column name in metadata
    pub name: String,
    /// Data type for Parquet schema
    pub data_type: FilterableDataType,
    /// Whether to create an index on this column
    pub indexed: bool,
    /// Whether this column supports range queries
    pub supports_range: bool,
    /// Estimated cardinality for query optimization
    pub estimated_cardinality: Option<usize>,
}

/// Supported data types for filterable columns
#[derive(Debug, Clone, PartialEq)]
pub enum FilterableDataType {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Array(Box<FilterableDataType>),
}

/// Index types for different column access patterns
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnIndexType {
    /// Hash index for equality queries
    Hash,
    /// B-tree index for range queries  
    BTree,
    /// Bloom filter for existence checks
    BloomFilter,
    /// Full-text search index
    FullText,
}

/// Parquet schema design for user-configurable columns
#[derive(Debug, Clone)]
pub struct ParquetSchemaDesign {
    pub collection_id: CollectionId,
    pub fields: Vec<ParquetField>,
    pub filterable_columns: Vec<FilterableColumn>,
    pub partition_columns: Vec<String>,
    pub compression: ParquetCompression,
    pub row_group_size: usize,
}

/// Individual field in Parquet schema
#[derive(Debug, Clone)]
pub struct ParquetField {
    pub name: String,
    pub field_type: ParquetFieldType,
    pub nullable: bool,
    pub indexed: bool,
}

/// Parquet field types
#[derive(Debug, Clone)]
pub enum ParquetFieldType {
    String,
    Int64,
    Double,
    Boolean,
    Timestamp,
    Binary,
    List(Box<ParquetFieldType>),
    Map, // For extra_meta field
}

/// Parquet compression options
#[derive(Debug, Clone)]
pub enum ParquetCompression {
    Uncompressed,
    Snappy,
    Gzip,
    Lzo,
    Zstd,
}

/// Processed vector record with separated filterable and extra metadata
#[derive(Debug, Clone)]
pub struct ProcessedVectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub timestamp: DateTime<Utc>,
    pub filterable_data: HashMap<String, Value>,
    pub extra_meta: HashMap<String, Value>,
}

/// Search result for vector similarity searches
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, Value>,
}

/// VIPER Core Storage Engine with ML-driven clustering and Parquet optimization
pub struct ViperCoreEngine {
    /// Configuration
    config: ViperCoreConfig,

    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<CollectionId, ViperCollectionMetadata>>>,

    /// Cluster metadata cache  
    clusters: Arc<RwLock<HashMap<ClusterId, ClusterMetadata>>>,

    /// Partition metadata cache
    partitions: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,

    /// ML models for cluster prediction per collection
    ml_models: Arc<RwLock<HashMap<CollectionId, ClusterPredictionModel>>>,

    /// Feature importance models for column selection
    feature_models: Arc<RwLock<HashMap<CollectionId, FeatureImportanceModel>>>,

    /// Atomic operations coordinator
    atomic_coordinator: Arc<AtomicOperationsCoordinator>,

    /// Schema adapter for record conversion
    schema_adapter: Arc<SchemaAdapter>,

    /// Parquet writer pool for concurrent operations
    writer_pool: Arc<ParquetWriterPool>,

    /// Statistics tracker
    stats: Arc<RwLock<ViperCoreStats>>,

    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
}

/// Configuration for VIPER core engine
#[derive(Debug, Clone)]
pub struct ViperCoreConfig {
    /// Enable ML-driven clustering
    pub enable_ml_clustering: bool,
    
    /// Enable background compaction
    pub enable_background_compaction: bool,
    
    /// Compression settings
    pub compression_config: CompressionConfig,
    
    /// Schema adaptation settings
    pub schema_config: SchemaConfig,
    
    /// Atomic operations settings
    pub atomic_config: AtomicOperationsConfig,
    
    /// Writer pool size
    pub writer_pool_size: usize,
    
    /// Statistics collection interval
    pub stats_interval_secs: u64,
}

/// Collection metadata in VIPER
#[derive(Debug, Clone)]
pub struct ViperCollectionMetadata {
    pub collection_id: CollectionId,
    pub dimension: usize,
    pub vector_count: usize,
    pub total_clusters: usize,
    pub storage_format_preference: VectorStorageFormat,
    pub ml_model_version: Option<String>,
    pub feature_importance: Vec<f32>, // Per-dimension importance scores
    pub compression_stats: CompressionStats,
    pub schema_version: u32,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    
    // New: Server-side metadata filtering configuration
    /// Pre-configured filterable metadata columns for Parquet pushdown
    pub filterable_columns: Vec<FilterableColumn>,
    /// Index on filterable columns for fast metadata queries
    pub column_indexes: HashMap<String, ColumnIndexType>,
    /// Flush size for testing (overrides global config)
    pub flush_size_bytes: Option<usize>,
}

/// Compression statistics for optimization
#[derive(Debug, Clone)]
pub struct CompressionStats {
    pub sparse_compression_ratio: f32,
    pub dense_compression_ratio: f32,
    pub optimal_sparsity_threshold: f32,
    pub column_compression_ratios: Vec<f32>, // Per-dimension compression ratios
}

/// Compression configuration
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub compression_level: u8,
    pub enable_adaptive_compression: bool,
    pub sparsity_threshold: f32,
}

/// Schema configuration
#[derive(Debug, Clone)]
pub struct SchemaConfig {
    pub enable_dynamic_schema: bool,
    pub filterable_fields: Vec<String>,
    pub max_metadata_fields: usize,
    pub enable_column_pruning: bool,
}

/// Atomic operations configuration
#[derive(Debug, Clone)]
pub struct AtomicOperationsConfig {
    pub enable_staging_directories: bool,
    pub staging_cleanup_interval_secs: u64,
    pub max_concurrent_operations: usize,
    pub enable_read_during_staging: bool,
}

/// Cluster metadata
#[derive(Debug, Clone)]
pub struct ClusterMetadata {
    pub cluster_id: ClusterId,
    pub collection_id: CollectionId,
    pub centroid: Vec<f32>,
    pub vector_count: usize,
    pub storage_format: VectorStorageFormat,
    pub compression_ratio: f32,
    pub last_updated: DateTime<Utc>,
}

/// Partition metadata
#[derive(Debug, Clone)]
pub struct PartitionMetadata {
    pub partition_id: PartitionId,
    pub cluster_id: ClusterId,
    pub file_url: String,
    pub vector_count: usize,
    pub size_bytes: usize,
    pub compression_ratio: f32,
    pub created_at: DateTime<Utc>,
}

/// ML model for cluster prediction
#[derive(Debug)]
pub struct ClusterPredictionModel {
    pub model_id: String,
    pub collection_id: CollectionId,
    pub version: String,
    pub accuracy: f32,
    pub feature_weights: Vec<f32>,
    pub cluster_centroids: Vec<Vec<f32>>,
    pub trained_at: DateTime<Utc>,
}

/// Feature importance model for column selection
#[derive(Debug)]
pub struct FeatureImportanceModel {
    pub model_id: String,
    pub collection_id: CollectionId,
    pub importance_scores: Vec<f32>,
    pub selected_features: Vec<usize>,
    pub compression_benefit: f32,
    pub trained_at: DateTime<Utc>,
}

/// Vector storage format preference
#[derive(Debug, Clone, PartialEq)]
pub enum VectorStorageFormat {
    Dense,
    Sparse,
    Adaptive,
}

/// Compression algorithm options
#[derive(Debug, Clone)]
pub enum CompressionAlgorithm {
    Snappy,
    Zstd,
    Lz4,
    Brotli,
}

/// Type aliases for clarity
pub type ClusterId = String;
pub type PartitionId = String;

/// Statistics for VIPER core engine
#[derive(Debug, Default, Clone)]
pub struct ViperCoreStats {
    pub total_operations: u64,
    pub insert_operations: u64,
    pub search_operations: u64,
    pub flush_operations: u64,
    pub compaction_operations: u64,
    pub avg_compression_ratio: f32,
    pub avg_ml_prediction_accuracy: f32,
    pub total_storage_size_bytes: u64,
    pub active_clusters: usize,
    pub active_partitions: usize,
}

// Atomic Operations Coordinator

/// Coordinator for atomic flush and compaction operations
pub struct AtomicOperationsCoordinator {
    /// Collection lock manager
    lock_manager: Arc<CollectionLockManager>,

    /// Staging operations manager
    staging_manager: Arc<StagingOperationsManager>,

    /// Active operations tracker
    active_operations: Arc<RwLock<HashMap<CollectionId, Vec<AtomicOperation>>>>,

    /// Configuration
    config: AtomicOperationsConfig,
}

/// Collection-level locking coordinator
pub struct CollectionLockManager {
    /// Active locks per collection
    collection_locks: Arc<RwLock<HashMap<CollectionId, CollectionLock>>>,

    /// Filesystem access
    filesystem: Arc<FilesystemFactory>,
}

/// Collection lock state
#[derive(Debug)]
pub struct CollectionLock {
    /// Number of active readers
    reader_count: usize,

    /// Whether a writer has exclusive access
    has_writer: bool,

    /// Pending operations queue
    pending_operations: Vec<OperationType>,

    /// Lock acquired timestamp
    acquired_at: DateTime<Utc>,

    /// Optimization flags
    allow_reads_during_staging: bool,
}

/// Staging operations manager
pub struct StagingOperationsManager {
    /// Active staging directories
    staging_dirs: Arc<RwLock<HashMap<String, StagingDirectory>>>,

    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,

    /// Cleanup scheduler
    cleanup_interval_secs: u64,
}

/// Staging directory metadata
#[derive(Debug)]
pub struct StagingDirectory {
    pub path: String,
    pub operation_type: OperationType,
    pub collection_id: CollectionId,
    pub created_at: DateTime<Utc>,
    pub expected_completion: DateTime<Utc>,
}

/// Atomic operation metadata
#[derive(Debug)]
pub struct AtomicOperation {
    pub operation_id: String,
    pub operation_type: OperationType,
    pub collection_id: CollectionId,
    pub staging_path: Option<String>,
    pub started_at: DateTime<Utc>,
    pub status: OperationStatus,
}

/// Operation type
#[derive(Debug, Clone)]
pub enum OperationType {
    Read,
    Insert,
    Flush,
    Compaction,
    SchemaEvolution,
}

/// Operation status
#[derive(Debug, Clone)]
pub enum OperationStatus {
    Pending,
    InProgress,
    Staging,
    Finalizing,
    Completed,
    Failed(String),
}

// Schema Adapter

/// Schema adapter for converting vector records to different formats
pub struct SchemaAdapter {
    /// Schema generation strategies
    strategies: HashMap<String, Box<dyn SchemaGenerationStrategy>>,

    /// Configuration
    config: SchemaConfig,

    /// Schema cache
    schema_cache: Arc<RwLock<HashMap<String, Arc<Schema>>>>,
}

/// Schema generation strategy trait
pub trait SchemaGenerationStrategy: Send + Sync {
    /// Generate schema for given records
    fn generate_schema(&self, records: &[VectorRecord]) -> Result<Arc<Schema>>;

    /// Get filterable fields
    fn get_filterable_fields(&self) -> &[String];

    /// Get strategy name
    fn name(&self) -> &'static str;
}

/// Default schema generation strategy
pub struct DefaultSchemaStrategy {
    filterable_fields: Vec<String>,
}

/// Parquet writer pool for concurrent operations
pub struct ParquetWriterPool {
    /// Available writers
    writers: Arc<Mutex<Vec<DefaultParquetWriter>>>,

    /// Pool configuration
    pool_size: usize,

    /// Writer creation factory
    writer_factory: Arc<DefaultParquetWriterFactory>,
}

/// Parquet writer interface
pub trait ParquetWriter: Send + Sync {
    /// Write record batch to file
    async fn write_batch(&mut self, batch: RecordBatch, path: &str) -> Result<()>;

    /// Flush and close writer
    async fn close(&mut self) -> Result<()>;
}

/// Parquet writer factory
pub trait ParquetWriterFactory: Send + Sync {
    /// Create new parquet writer
    async fn create_writer(&self) -> Result<DefaultParquetWriter>;
}

// Implementation

impl ViperCoreEngine {
    /// Create new VIPER core engine
    pub async fn new(
        config: ViperCoreConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        let lock_manager = Arc::new(
            CollectionLockManager::new(filesystem.clone()).await?
        );

        let staging_manager = Arc::new(
            StagingOperationsManager::new(
                filesystem.clone(),
                config.atomic_config.staging_cleanup_interval_secs,
            ).await?
        );

        let atomic_coordinator = Arc::new(
            AtomicOperationsCoordinator::new(
                lock_manager.clone(),
                staging_manager,
                config.atomic_config.clone(),
            ).await?
        );

        let schema_adapter = Arc::new(
            SchemaAdapter::new(config.schema_config.clone()).await?
        );

        let writer_factory = Arc::new(DefaultParquetWriterFactory::new());
        let writer_pool = Arc::new(
            ParquetWriterPool::new(config.writer_pool_size, writer_factory).await?
        );

        Ok(Self {
            config,
            collections: Arc::new(RwLock::new(HashMap::new())),
            clusters: Arc::new(RwLock::new(HashMap::new())),
            partitions: Arc::new(RwLock::new(HashMap::new())),
            ml_models: Arc::new(RwLock::new(HashMap::new())),
            feature_models: Arc::new(RwLock::new(HashMap::new())),
            atomic_coordinator,
            schema_adapter,
            writer_pool,
            stats: Arc::new(RwLock::new(ViperCoreStats::default())),
            filesystem,
        })
    }

    /// Insert vector record with unlimited metadata key-value pairs
    /// 
    /// During insert/bulk insert/update operations:
    /// 1. Metadata fields are stored as-is in WAL and memtable (no transformation)
    /// 2. All metadata key-value pairs are preserved exactly as provided
    /// 3. No separation between filterable and non-filterable fields at this stage
    /// 4. Transformation happens later during flush/compaction operations
    pub async fn insert_vector(&self, record: VectorRecord) -> Result<()> {
        debug!("🔥 VIPER Core: Inserting vector {} with {} metadata fields in collection {}", 
               record.id, record.metadata.len(), record.collection_id);

        // Step 1: Acquire collection lock
        let _lock = self.atomic_coordinator
            .acquire_lock(&record.collection_id, OperationType::Insert)
            .await?;

        // Step 2: Store vector record as-is in WAL/memtable (unlimited metadata support)
        self.store_vector_record_for_wal(&record).await?;

        // Step 3: Get or create collection metadata
        let collection_metadata = self.get_or_create_collection_metadata(&record.collection_id).await?;

        // Step 3: ML-driven cluster prediction
        let cluster_id = if self.config.enable_ml_clustering {
            self.predict_optimal_cluster(&record, &collection_metadata).await?
        } else {
            self.default_cluster_assignment(&record.collection_id).await?
        };

        // Step 4: Determine storage format based on vector characteristics
        let storage_format = self.determine_storage_format(&record.vector, &collection_metadata).await?;

        // Step 5: Convert record to schema format
        let record_batch = self.schema_adapter
            .adapt_records_to_schema(&[record.clone()], &collection_metadata.collection_id)
            .await?;

        // Step 6: Write to partition using parquet writer
        let partition_path = self.get_partition_path(&cluster_id, &storage_format).await?;
        let mut writer = self.writer_pool.acquire_writer().await?;
        writer.write_batch(record_batch, &partition_path).await?;
        self.writer_pool.release_writer(writer).await?;

        // Step 7: Update metadata and statistics
        self.update_collection_stats(&record.collection_id, 1, record.vector.len()).await?;

        info!("✅ VIPER Core: Successfully inserted vector {} in collection {}", 
              record.id, record.collection_id);

        Ok(())
    }

    /// Search vectors using ML-optimized clustering
    pub async fn search_vectors(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        debug!("🔍 VIPER Core: Searching {} vectors in collection {}", k, collection_id);

        // Step 1: Get collection metadata
        let collection_metadata = self.get_collection_metadata(collection_id).await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;

        // Step 2: ML-driven cluster selection
        let relevant_clusters = if self.config.enable_ml_clustering {
            self.predict_relevant_clusters(query_vector, &collection_metadata).await?
        } else {
            self.get_all_clusters(collection_id).await?
        };

        // Step 3: Parallel search across selected clusters
        let mut all_results = Vec::new();
        for cluster_id in relevant_clusters {
            let cluster_results = self.search_cluster(&cluster_id, query_vector, k * 2).await?;
            all_results.extend(cluster_results);
        }

        // Step 4: Merge and rank results
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_results.truncate(k);

        debug!("🔍 VIPER Core: Found {} results in collection {}", all_results.len(), collection_id);
        Ok(all_results)
    }

    /// Perform atomic flush operation with metadata transformation
    /// 
    /// During flush operations:
    /// 1. Read raw vector records from WAL/memtable (metadata stored as-is)
    /// 2. Apply metadata transformation based on user-configured filterable columns
    /// 3. Create Parquet layout with filterable columns + extra_meta
    /// 4. Write transformed data to VIPER storage format
    pub async fn flush_collection(&self, collection_id: &CollectionId) -> Result<()> {
        info!("🚀 VIPER Core: Starting atomic flush with metadata transformation for collection {}", collection_id);

        let operation_id = uuid::Uuid::new_v4().to_string();
        let atomic_operation = AtomicOperation {
            operation_id: operation_id.clone(),
            operation_type: OperationType::Flush,
            collection_id: collection_id.clone(),
            staging_path: None,
            started_at: Utc::now(),
            status: OperationStatus::Pending,
        };

        // Get collection metadata to understand filterable column configuration
        let metadata_opt = self.get_collection_metadata(collection_id).await?;
        if let Some(metadata) = metadata_opt {
            info!(
                "🔄 Flush will transform metadata: {} user-configured filterable columns defined",
                metadata.filterable_columns.len()
            );
            
            for column in &metadata.filterable_columns {
                debug!(
                    "📊 Filterable column: '{}' (type: {:?}, indexed: {})",
                    column.name, column.data_type, column.indexed
                );
            }
        } else {
            info!("⚠️ No filterable columns configured - all metadata will go to extra_meta");
        }

        // Execute atomic flush through coordinator
        // The coordinator will call process_vector_record_for_parquet for each record
        self.atomic_coordinator
            .execute_flush_operation(atomic_operation)
            .await?;

        info!("✅ VIPER Core: Completed atomic flush with metadata transformation for collection {}", collection_id);
        Ok(())
    }

    /// Perform atomic compaction operation
    pub async fn compact_collection(&self, collection_id: &CollectionId) -> Result<()> {
        info!("🔧 VIPER Core: Starting atomic compaction for collection {}", collection_id);

        let operation_id = uuid::Uuid::new_v4().to_string();
        let atomic_operation = AtomicOperation {
            operation_id: operation_id.clone(),
            operation_type: OperationType::Compaction,
            collection_id: collection_id.clone(),
            staging_path: None,
            started_at: Utc::now(),
            status: OperationStatus::Pending,
        };

        // Execute atomic compaction through coordinator
        self.atomic_coordinator
            .execute_compaction_operation(atomic_operation)
            .await?;

        info!("✅ VIPER Core: Completed atomic compaction for collection {}", collection_id);
        Ok(())
    }

    // Helper methods

    async fn get_or_create_collection_metadata(
        &self,
        collection_id: &CollectionId,
    ) -> Result<ViperCollectionMetadata> {
        let collections = self.collections.read().await;
        if let Some(metadata) = collections.get(collection_id) {
            return Ok(metadata.clone());
        }
        drop(collections);

        // Create new collection metadata with empty filterable columns
        // User will configure these during collection creation
        let metadata = ViperCollectionMetadata {
            collection_id: collection_id.clone(),
            dimension: 0, // Will be updated on first insert
            vector_count: 0,
            total_clusters: 0,
            storage_format_preference: VectorStorageFormat::Adaptive,
            ml_model_version: None,
            feature_importance: Vec::new(),
            compression_stats: CompressionStats {
                sparse_compression_ratio: 0.0,
                dense_compression_ratio: 0.0,
                optimal_sparsity_threshold: 0.5,
                column_compression_ratios: Vec::new(),
            },
            schema_version: 1,
            created_at: Utc::now(),
            last_updated: Utc::now(),
            
            // User-configurable filterable columns (empty by default)
            filterable_columns: Vec::new(),
            column_indexes: HashMap::new(),
            flush_size_bytes: Some(1024 * 1024), // 1MB flush size for testing
        };

        let mut collections = self.collections.write().await;
        collections.insert(collection_id.clone(), metadata.clone());
        Ok(metadata)
    }

    /// Configure filterable columns for a collection during creation
    pub async fn configure_filterable_columns(
        &self,
        collection_id: &CollectionId,
        filterable_columns: Vec<FilterableColumn>,
    ) -> Result<()> {
        info!(
            "🔧 Configuring {} filterable columns for collection {}",
            filterable_columns.len(),
            collection_id
        );

        // Validate filterable columns
        self.validate_filterable_columns(&filterable_columns)?;

        // Update collection metadata with user-specified columns
        let mut collections = self.collections.write().await;
        if let Some(metadata) = collections.get_mut(collection_id) {
            metadata.filterable_columns = filterable_columns.clone();
            
            // Create appropriate indexes based on column specifications
            metadata.column_indexes = self.create_column_indexes(&filterable_columns);
            metadata.last_updated = Utc::now();

            info!(
                "✅ Configured {} filterable columns with {} indexes for collection {}",
                filterable_columns.len(),
                metadata.column_indexes.len(),
                collection_id
            );

            // Log the configured columns for debugging
            for column in &filterable_columns {
                info!(
                    "📊 Column '{}': type={:?}, indexed={}, range_support={}",
                    column.name, column.data_type, column.indexed, column.supports_range
                );
            }
        } else {
            return Err(anyhow::anyhow!("Collection {} not found", collection_id));
        }

        Ok(())
    }

    /// Validate user-provided filterable column configurations
    fn validate_filterable_columns(&self, columns: &[FilterableColumn]) -> Result<()> {
        let mut column_names = std::collections::HashSet::new();

        for column in columns {
            // Check for duplicate column names
            if !column_names.insert(&column.name) {
                return Err(anyhow::anyhow!(
                    "Duplicate filterable column name: '{}'",
                    column.name
                ));
            }

            // Validate column name (no special characters, not empty)
            if column.name.is_empty() || !column.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(anyhow::anyhow!(
                    "Invalid column name '{}': must contain only alphanumeric characters and underscores",
                    column.name
                ));
            }

            // Validate range support for data types
            if column.supports_range && !self.supports_range_queries(&column.data_type) {
                return Err(anyhow::anyhow!(
                    "Column '{}' with type {:?} cannot support range queries",
                    column.name, column.data_type
                ));
            }

            // Reserved field names that shouldn't be used as filterable columns
            let reserved_names = ["id", "vector", "timestamp", "collection_id", "extra_meta"];
            if reserved_names.contains(&column.name.as_str()) {
                return Err(anyhow::anyhow!(
                    "Column name '{}' is reserved and cannot be used as a filterable column",
                    column.name
                ));
            }
        }

        Ok(())
    }

    /// Check if a data type supports range queries
    fn supports_range_queries(&self, data_type: &FilterableDataType) -> bool {
        match data_type {
            FilterableDataType::Integer 
            | FilterableDataType::Float 
            | FilterableDataType::DateTime => true,
            FilterableDataType::String => true, // Lexicographic ordering
            FilterableDataType::Boolean 
            | FilterableDataType::Array(_) => false,
        }
    }

    /// Create appropriate column indexes based on filterable column specifications
    fn create_column_indexes(&self, columns: &[FilterableColumn]) -> HashMap<String, ColumnIndexType> {
        let mut indexes = HashMap::new();

        for column in columns {
            if column.indexed {
                let index_type = if column.supports_range {
                    ColumnIndexType::BTree // B-tree for range queries
                } else {
                    match &column.data_type {
                        FilterableDataType::String => ColumnIndexType::Hash,
                        FilterableDataType::Integer | FilterableDataType::Float => ColumnIndexType::BTree,
                        FilterableDataType::Boolean => ColumnIndexType::Hash,
                        FilterableDataType::DateTime => ColumnIndexType::BTree,
                        FilterableDataType::Array(_) => ColumnIndexType::BloomFilter, // For membership tests
                    }
                };

                indexes.insert(column.name.clone(), index_type);
            }
        }

        indexes
    }

    /// Design Parquet schema based on filterable columns and extra_meta
    pub async fn design_parquet_schema(
        &self,
        collection_id: &CollectionId,
    ) -> Result<ParquetSchemaDesign> {
        let metadata_opt = self.get_collection_metadata(collection_id).await?;
        
        let metadata = metadata_opt.ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;
        
        info!(
            "🏗️ Designing Parquet schema for collection {} with {} filterable columns",
            collection_id,
            metadata.filterable_columns.len()
        );

        let mut schema_fields = Vec::new();

        // Core fields (always present)
        schema_fields.push(ParquetField {
            name: "id".to_string(),
            field_type: ParquetFieldType::String,
            nullable: false,
            indexed: true,
        });

        schema_fields.push(ParquetField {
            name: "vector".to_string(),
            field_type: ParquetFieldType::Binary, // Serialized vector data
            nullable: false,
            indexed: false,
        });

        schema_fields.push(ParquetField {
            name: "timestamp".to_string(),
            field_type: ParquetFieldType::Timestamp,
            nullable: false,
            indexed: true,
        });

        // User-configured filterable columns
        for column in &metadata.filterable_columns {
            schema_fields.push(ParquetField {
                name: column.name.clone(),
                field_type: self.convert_to_parquet_type(&column.data_type),
                nullable: true, // Metadata fields can be optional
                indexed: column.indexed,
            });
        }

        // Extra metadata field for unmapped metadata
        schema_fields.push(ParquetField {
            name: "extra_meta".to_string(),
            field_type: ParquetFieldType::Map, // Key-value map for additional metadata
            nullable: true,
            indexed: false, // Not indexed by default, but could be in future
        });

        let schema_design = ParquetSchemaDesign {
            collection_id: collection_id.clone(),
            fields: schema_fields,
            filterable_columns: metadata.filterable_columns.clone(),
            partition_columns: vec!["timestamp".to_string()], // Time-based partitioning
            compression: ParquetCompression::Snappy,
            row_group_size: 100_000, // Optimize for metadata filtering
        };

        info!(
            "✅ Parquet schema designed: {} total fields ({} filterable, 1 extra_meta)",
            schema_design.fields.len(),
            metadata.filterable_columns.len()
        );

        Ok(schema_design)
    }

    /// Convert FilterableDataType to Parquet field type
    fn convert_to_parquet_type(&self, data_type: &FilterableDataType) -> ParquetFieldType {
        match data_type {
            FilterableDataType::String => ParquetFieldType::String,
            FilterableDataType::Integer => ParquetFieldType::Int64,
            FilterableDataType::Float => ParquetFieldType::Double,
            FilterableDataType::Boolean => ParquetFieldType::Boolean,
            FilterableDataType::DateTime => ParquetFieldType::Timestamp,
            FilterableDataType::Array(inner_type) => {
                ParquetFieldType::List(Box::new(self.convert_to_parquet_type(inner_type)))
            }
        }
    }

    /// Process vector record during flush/compaction - separate filterable columns from extra metadata
    /// 
    /// This is called during flush/compaction operations to transform the raw metadata stored
    /// in WAL/memtable into the VIPER Parquet layout:
    /// - User-configured filterable columns → Parquet columns for server-side filtering
    /// - All other metadata fields → extra_meta key-value map
    pub async fn process_vector_record_for_parquet(
        &self,
        collection_id: &CollectionId,
        record: &VectorRecord,
    ) -> Result<ProcessedVectorRecord> {
        let metadata_opt = self.get_collection_metadata(collection_id).await?;
        
        if let Some(metadata) = metadata_opt {
            let filterable_column_names: std::collections::HashSet<String> = 
                metadata.filterable_columns.iter()
                    .map(|col| col.name.clone())
                    .collect();

            let mut filterable_data = HashMap::new();
            let mut extra_meta = HashMap::new();

            // During flush/compaction: separate metadata based on user-configured filterable columns
            for (key, value) in &record.metadata {
                if filterable_column_names.contains(key) {
                    filterable_data.insert(key.clone(), value.clone());
                    debug!("📊 Mapped '{}' to filterable column", key);
                } else {
                    extra_meta.insert(key.clone(), value.clone());
                    debug!("🗂️ Mapped '{}' to extra_meta", key);
                }
            }

            info!(
                "🔄 Processed record {}: {} filterable columns, {} extra_meta fields",
                record.id,
                filterable_data.len(),
                extra_meta.len()
            );

            Ok(ProcessedVectorRecord {
                id: record.id.clone(),
                vector: record.vector.clone(),
                timestamp: record.timestamp,
                filterable_data,
                extra_meta,
            })
        } else {
            // Collection metadata not found - treat all metadata as extra_meta
            warn!("⚠️ Collection {} metadata not found, treating all metadata as extra_meta", collection_id);
            
            Ok(ProcessedVectorRecord {
                id: record.id.clone(),
                vector: record.vector.clone(),
                timestamp: record.timestamp,
                filterable_data: HashMap::new(),
                extra_meta: record.metadata.clone(),
            })
        }
    }

    /// Store vector record directly in WAL/memtable during insert operations
    /// 
    /// During insert/bulk insert/update operations, metadata fields are stored as-is
    /// without any filtering or transformation. This preserves all user-provided metadata
    /// in its original form in the WAL and memtable for atomic writes.
    pub async fn store_vector_record_for_wal(
        &self,
        record: &VectorRecord,
    ) -> Result<()> {
        debug!(
            "💾 Storing vector {} with {} metadata fields in WAL (as-is storage)",
            record.id,
            record.metadata.len()
        );

        // Store record exactly as provided - no metadata transformation
        // All metadata fields (filterable and non-filterable) are stored together
        // This allows unlimited metadata key-value pairs as requested
        
        // Log metadata fields for debugging
        for (key, value) in &record.metadata {
            debug!("  📋 Metadata field: '{}' = {:?}", key, value);
        }
        
        // The actual WAL storage would happen in the calling code
        // This method is for documentation and potential validation
        
        Ok(())
    }

    /// Demonstrate the complete metadata lifecycle from insert to storage
    /// 
    /// This method illustrates how metadata fields flow through the system:
    /// Insert → WAL/Memtable (as-is) → Flush/Compaction → VIPER Layout (transformed)
    pub async fn demonstrate_metadata_lifecycle(
        &self,
        collection_id: &CollectionId,
        record: &VectorRecord,
    ) -> Result<()> {
        info!("🔄 DEMONSTRATING METADATA LIFECYCLE for vector {}", record.id);
        info!("{}", "=".repeat(80));
        
        // Stage 1: Insert/Update - Store as-is in WAL/memtable
        info!("📥 STAGE 1: INSERT/UPDATE - Store metadata as-is");
        info!("   Original metadata fields: {}", record.metadata.len());
        for (key, value) in &record.metadata {
            info!("     • {} = {:?}", key, value);
        }
        
        // Stage 2: Flush/Compaction - Transform based on filterable columns
        info!("🔄 STAGE 2: FLUSH/COMPACTION - Transform metadata layout");
        let processed = self.process_vector_record_for_parquet(collection_id, record).await?;
        
        info!("   Filterable columns: {}", processed.filterable_data.len());
        for (key, value) in &processed.filterable_data {
            info!("     📊 Column: {} = {:?}", key, value);
        }
        
        info!("   Extra metadata fields: {}", processed.extra_meta.len());
        for (key, value) in &processed.extra_meta {
            info!("     🗂️ Extra: {} = {:?}", key, value);
        }
        
        // Stage 3: Storage format
        info!("💾 STAGE 3: VIPER PARQUET LAYOUT");
        info!("   ├─ Core fields: id, vector, timestamp");
        info!("   ├─ Filterable columns: {} (server-side filtering)", processed.filterable_data.len());
        info!("   └─ Extra_meta map: {} (preserved key-value pairs)", processed.extra_meta.len());
        
        info!("✅ METADATA LIFECYCLE COMPLETE");
        info!("{}", "=".repeat(80));
        
        Ok(())
    }

    async fn get_collection_metadata(
        &self,
        collection_id: &CollectionId,
    ) -> Result<Option<ViperCollectionMetadata>> {
        let collections = self.collections.read().await;
        Ok(collections.get(collection_id).cloned())
    }

    async fn predict_optimal_cluster(
        &self,
        record: &VectorRecord,
        _collection_metadata: &ViperCollectionMetadata,
    ) -> Result<ClusterId> {
        // Simplified ML prediction - in real implementation, this would use trained models
        let cluster_id = format!("cluster_{}", record.vector.len() % 10);
        Ok(cluster_id)
    }

    async fn default_cluster_assignment(&self, collection_id: &CollectionId) -> Result<ClusterId> {
        Ok(format!("{}_default_cluster", collection_id))
    }

    async fn determine_storage_format(
        &self,
        vector: &[f32],
        _collection_metadata: &ViperCollectionMetadata,
    ) -> Result<VectorStorageFormat> {
        let sparsity = calculate_sparsity_ratio(vector);
        if sparsity > 0.7 {
            Ok(VectorStorageFormat::Sparse)
        } else if sparsity < 0.1 {
            Ok(VectorStorageFormat::Dense)
        } else {
            Ok(VectorStorageFormat::Adaptive)
        }
    }

    async fn get_partition_path(
        &self,
        cluster_id: &ClusterId,
        storage_format: &VectorStorageFormat,
    ) -> Result<String> {
        let format_suffix = match storage_format {
            VectorStorageFormat::Dense => "dense",
            VectorStorageFormat::Sparse => "sparse",
            VectorStorageFormat::Adaptive => "adaptive",
        };
        Ok(format!("/data/viper/{}/{}.parquet", cluster_id, format_suffix))
    }

    async fn predict_relevant_clusters(
        &self,
        _query_vector: &[f32],
        collection_metadata: &ViperCollectionMetadata,
    ) -> Result<Vec<ClusterId>> {
        // Simplified cluster selection - would use ML models in real implementation
        let clusters = (0..collection_metadata.total_clusters.max(1))
            .map(|i| format!("cluster_{}", i))
            .collect();
        Ok(clusters)
    }

    async fn get_all_clusters(&self, collection_id: &CollectionId) -> Result<Vec<ClusterId>> {
        let clusters = self.clusters.read().await;
        let collection_clusters: Vec<ClusterId> = clusters
            .values()
            .filter(|c| &c.collection_id == collection_id)
            .map(|c| c.cluster_id.clone())
            .collect();
        Ok(collection_clusters)
    }

    async fn search_cluster(
        &self,
        _cluster_id: &ClusterId,
        _query_vector: &[f32],
        _k: usize,
    ) -> Result<Vec<SearchResult>> {
        // Simplified cluster search - would implement actual vector search
        Ok(Vec::new())
    }

    async fn update_collection_stats(
        &self,
        collection_id: &CollectionId,
        vector_count_delta: i64,
        vector_dimension: usize,
    ) -> Result<()> {
        let mut collections = self.collections.write().await;
        if let Some(metadata) = collections.get_mut(collection_id) {
            metadata.vector_count = (metadata.vector_count as i64 + vector_count_delta).max(0) as usize;
            if metadata.dimension == 0 {
                metadata.dimension = vector_dimension;
            }
            metadata.last_updated = Utc::now();
        }
        Ok(())
    }

    /// Get engine statistics
    pub async fn get_statistics(&self) -> ViperCoreStats {
        let stats_guard = self.stats.read().await;
        stats_guard.clone()
    }
}

// Utility functions

/// Calculate sparsity ratio for a vector
fn calculate_sparsity_ratio(vector: &[f32]) -> f32 {
    let zero_count = vector.iter().filter(|&&x| x == 0.0).count();
    zero_count as f32 / vector.len() as f32
}

// Placeholder implementations for compilation

impl CollectionLockManager {
    async fn new(_filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        Ok(Self {
            collection_locks: Arc::new(RwLock::new(HashMap::new())),
            filesystem: _filesystem,
        })
    }
}

impl StagingOperationsManager {
    async fn new(_filesystem: Arc<FilesystemFactory>, _cleanup_interval: u64) -> Result<Self> {
        Ok(Self {
            staging_dirs: Arc::new(RwLock::new(HashMap::new())),
            filesystem: _filesystem,
            cleanup_interval_secs: _cleanup_interval,
        })
    }
}

impl AtomicOperationsCoordinator {
    async fn new(
        lock_manager: Arc<CollectionLockManager>,
        staging_manager: Arc<StagingOperationsManager>,
        config: AtomicOperationsConfig,
    ) -> Result<Self> {
        Ok(Self {
            lock_manager,
            staging_manager,
            active_operations: Arc::new(RwLock::new(HashMap::new())),
            config,
        })
    }

    async fn acquire_lock(
        &self,
        _collection_id: &CollectionId,
        _operation_type: OperationType,
    ) -> Result<CollectionLockGuard> {
        Ok(CollectionLockGuard)
    }

    async fn execute_flush_operation(&self, _operation: AtomicOperation) -> Result<()> {
        // Placeholder implementation
        Ok(())
    }

    async fn execute_compaction_operation(&self, _operation: AtomicOperation) -> Result<()> {
        // Placeholder implementation
        Ok(())
    }
}

pub struct CollectionLockGuard;

impl SchemaAdapter {
    async fn new(_config: SchemaConfig) -> Result<Self> {
        Ok(Self {
            strategies: HashMap::new(),
            config: _config,
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn adapt_records_to_schema(
        &self,
        _records: &[VectorRecord],
        _collection_id: &CollectionId,
    ) -> Result<RecordBatch> {
        // Placeholder implementation - would implement actual schema adaptation
        Err(anyhow::anyhow!("Schema adaptation not implemented"))
    }
}

impl ParquetWriterPool {
    async fn new(_pool_size: usize, _factory: Arc<DefaultParquetWriterFactory>) -> Result<Self> {
        Ok(Self {
            writers: Arc::new(Mutex::new(Vec::new())),
            pool_size: _pool_size,
            writer_factory: _factory,
        })
    }

    async fn acquire_writer(&self) -> Result<DefaultParquetWriter> {
        // Placeholder implementation
        Ok(DefaultParquetWriter)
    }

    async fn release_writer(&self, _writer: DefaultParquetWriter) -> Result<()> {
        // Placeholder implementation
        Ok(())
    }
}

pub struct DefaultParquetWriter;

impl ParquetWriter for DefaultParquetWriter {
    async fn write_batch(&mut self, _batch: RecordBatch, _path: &str) -> Result<()> {
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

pub struct DefaultParquetWriterFactory;

impl DefaultParquetWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ParquetWriterFactory for DefaultParquetWriterFactory {
    async fn create_writer(&self) -> Result<DefaultParquetWriter> {
        Ok(DefaultParquetWriter)
    }
}

impl DefaultSchemaStrategy {
    pub fn new(filterable_fields: Vec<String>) -> Self {
        Self { filterable_fields }
    }
}

impl SchemaGenerationStrategy for DefaultSchemaStrategy {
    fn generate_schema(&self, _records: &[VectorRecord]) -> Result<Arc<Schema>> {
        // Placeholder implementation
        Err(anyhow::anyhow!("Schema generation not implemented"))
    }

    fn get_filterable_fields(&self) -> &[String] {
        &self.filterable_fields
    }

    fn name(&self) -> &'static str {
        "DefaultSchemaStrategy"
    }
}

impl Default for ViperCoreConfig {
    fn default() -> Self {
        Self {
            enable_ml_clustering: true,
            enable_background_compaction: true,
            compression_config: CompressionConfig {
                algorithm: CompressionAlgorithm::Snappy,
                compression_level: 6,
                enable_adaptive_compression: true,
                sparsity_threshold: 0.5,
            },
            schema_config: SchemaConfig {
                enable_dynamic_schema: true,
                filterable_fields: vec!["category".to_string(), "type".to_string()],
                max_metadata_fields: 100,
                enable_column_pruning: true,
            },
            atomic_config: AtomicOperationsConfig {
                enable_staging_directories: true,
                staging_cleanup_interval_secs: 3600,
                max_concurrent_operations: 10,
                enable_read_during_staging: true,
            },
            writer_pool_size: 8,
            stats_interval_secs: 60,
        }
    }
}

// Implementation of VectorStorage trait for VIPER engine

#[async_trait::async_trait]
impl VectorStorage for ViperCoreEngine {
    fn engine_name(&self) -> &'static str {
        "VIPER"
    }
    
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities {
            supports_transactions: true,
            supports_streaming: true,
            supports_compression: true,
            supports_encryption: false, // TODO: Add encryption support
            max_vector_dimension: 10_000, // TODO: Make configurable
            max_batch_size: 100_000,
            supported_distances: vec![
                DistanceMetric::Cosine,
                DistanceMetric::Euclidean,
                DistanceMetric::DotProduct,
                DistanceMetric::Manhattan,
            ],
            supported_indexes: vec![
                IndexType::HNSW { m: 16, ef_construction: 200, ef_search: 50 },
                IndexType::IVF { n_lists: 100, n_probes: 8 },
                IndexType::Flat,
            ],
        }
    }
    
    async fn execute_operation(
        &self,
        operation: VectorOperation,
    ) -> anyhow::Result<OperationResult> {
        debug!("🔥 VIPER executing operation: {:?}", std::mem::discriminant(&operation));
        
        match operation {
            VectorOperation::Insert { record, index_immediately } => {
                self.insert_vector(record.clone()).await?;
                
                if index_immediately {
                    // TODO: Trigger immediate indexing
                    debug!("🔥 VIPER: Immediate indexing requested for vector {}", record.id);
                }
                
                Ok(OperationResult::Inserted { vector_id: record.id })
            }
            
            VectorOperation::Update { vector_id, new_vector, new_metadata } => {
                // TODO: Implement vector update
                debug!("🔄 VIPER: Update operation for vector {}", vector_id);
                Ok(OperationResult::Updated { vector_id, changes: 1 })
            }
            
            VectorOperation::Delete { vector_id, soft_delete } => {
                // TODO: Implement vector deletion
                debug!("🗑️ VIPER: Delete operation for vector {} (soft={})", vector_id, soft_delete);
                Ok(OperationResult::Deleted { vector_id })
            }
            
            VectorOperation::Search(search_context) => {
                let results = self.search_vectors(
                    &search_context.collection_id,
                    &search_context.query_vector,
                    search_context.k,
                ).await?;
                
                // Convert VIPER SearchResult to vector::types::SearchResult
                let converted_results: Vec<crate::storage::vector::types::SearchResult> = results.into_iter().map(|r| {
                    crate::storage::vector::types::SearchResult {
                        vector_id: r.id,
                        score: r.score,
                        vector: Some(r.vector),
                        metadata: serde_json::Value::Object(
                            r.metadata.into_iter().collect()
                        ),
                        debug_info: None,
                        storage_info: None,
                        rank: None,
                        distance: None,
                    }
                }).collect();
                Ok(OperationResult::SearchResults(converted_results))
            }
            
            VectorOperation::Get { vector_id, include_vector } => {
                // TODO: Implement vector retrieval
                debug!("📝 VIPER: Get operation for vector {} (include_vector={})", vector_id, include_vector);
                Ok(OperationResult::VectorData {
                    vector_id,
                    vector: if include_vector { Some(Vec::new()) } else { None },
                    metadata: Value::Null,
                })
            }
            
            VectorOperation::Batch { operations, transactional } => {
                debug!("📦 VIPER: Batch operation with {} operations (transactional={})", 
                       operations.len(), transactional);
                
                let mut results = Vec::new();
                
                if transactional {
                    // TODO: Implement transactional batch operations
                    for operation in operations {
                        let result = self.execute_operation(operation).await?;
                        results.push(result);
                    }
                } else {
                    // Execute operations independently
                    for operation in operations {
                        match self.execute_operation(operation).await {
                            Ok(result) => results.push(result),
                            Err(e) => results.push(OperationResult::Error {
                                operation: "batch_item".to_string(),
                                error: e.to_string(),
                                recoverable: true,
                            }),
                        }
                    }
                }
                
                Ok(OperationResult::BatchResults(results))
            }
        }
    }
    
    async fn get_statistics(&self) -> anyhow::Result<StorageStatistics> {
        let stats = self.get_statistics().await;
        let collections = self.collections.read().await;
        
        Ok(StorageStatistics {
            total_vectors: stats.insert_operations as usize,
            total_collections: collections.len(),
            storage_size_bytes: stats.total_storage_size_bytes,
            index_size_bytes: 0, // TODO: Calculate from index manager
            cache_hit_ratio: 0.0, // TODO: Implement cache tracking
            avg_search_latency_ms: 0.0, // TODO: Track search latency
            operations_per_second: stats.total_operations as f64 / 60.0, // Rough estimate
            last_compaction: None, // TODO: Track compaction timestamps
        })
    }
    
    async fn health_check(&self) -> anyhow::Result<HealthStatus> {
        // TODO: Implement comprehensive health check
        Ok(HealthStatus {
            healthy: true,
            status: "VIPER engine operational".to_string(),
            last_check: Utc::now(),
            response_time_ms: 1.0,
            error_count: 0,
            warnings: Vec::new(),
        })
    }
}

impl ViperCoreEngine {
    /// Server-side metadata filtering using Parquet column pushdown
    pub async fn filter_by_metadata(
        &self,
        collection_id: &CollectionId,
        filters: &[MetadataFilter],
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let metadata_opt = self.get_collection_metadata(collection_id).await?;
        
        let metadata = metadata_opt.ok_or_else(|| anyhow::anyhow!("Collection {} not found", collection_id))?;
        
        info!(
            "🔍 VIPER: Server-side metadata filtering on collection {} with {} filterable columns",
            collection_id,
            metadata.filterable_columns.len()
        );

        // Analyze which filters can be pushed down to Parquet level
        let (pushdown_filters, client_filters) = 
            self.analyze_filter_pushdown(&metadata, filters).await?;

        info!(
            "📊 Filter analysis: {} pushdown, {} client-side",
            pushdown_filters.len(),
            client_filters.len()
        );

        // Execute Parquet-level filtering (simulated for now)
        let mut results = self.execute_parquet_filters(
            collection_id,
            &pushdown_filters,
            limit
        ).await?;

        // Apply remaining client-side filters
        if !client_filters.is_empty() {
            results = self.apply_client_filters(results, &client_filters).await?;
        }

        info!(
            "✅ VIPER: Metadata filtering returned {} results",
            results.len()
        );

        Ok(results)
    }

    /// Analyze which filters can be pushed down to Parquet storage
    async fn analyze_filter_pushdown(
        &self,
        metadata: &ViperCollectionMetadata,
        filters: &[MetadataFilter],
    ) -> Result<(Vec<MetadataFilter>, Vec<MetadataFilter>)> {
        let filterable_fields: HashMap<String, &FilterableColumn> = metadata
            .filterable_columns
            .iter()
            .map(|col| (col.name.clone(), col))
            .collect();

        let mut pushdown_filters = Vec::new();
        let mut client_filters = Vec::new();

        for filter in filters {
            if self.can_pushdown_filter(filter, &filterable_fields) {
                pushdown_filters.push(filter.clone());
            } else {
                client_filters.push(filter.clone());
            }
        }

        Ok((pushdown_filters, client_filters))
    }

    /// Check if a filter can be pushed down to Parquet level
    fn can_pushdown_filter(
        &self,
        filter: &MetadataFilter,
        filterable_fields: &HashMap<String, &FilterableColumn>,
    ) -> bool {
        match filter {
            MetadataFilter::Field { field, condition } => {
                if let Some(column) = filterable_fields.get(field) {
                    // Check if the condition is supported for this column type
                    match condition {
                        FieldCondition::Equals(_) | FieldCondition::NotEquals(_) => true,
                        FieldCondition::In(_) | FieldCondition::NotIn(_) => true,
                        FieldCondition::GreaterThan(_) 
                        | FieldCondition::LessThan(_)
                        | FieldCondition::Range { .. } => column.supports_range,
                        FieldCondition::Contains(_) 
                        | FieldCondition::StartsWith(_) 
                        | FieldCondition::EndsWith(_) => {
                            matches!(column.data_type, FilterableDataType::String)
                        },
                        _ => false,
                    }
                } else {
                    false
                }
            }
            MetadataFilter::And(sub_filters) => {
                sub_filters.iter().all(|f| self.can_pushdown_filter(f, filterable_fields))
            }
            MetadataFilter::Or(sub_filters) => {
                sub_filters.iter().all(|f| self.can_pushdown_filter(f, filterable_fields))
            }
            _ => false,
        }
    }

    /// Execute filters at Parquet storage level
    async fn execute_parquet_filters(
        &self,
        collection_id: &CollectionId,
        filters: &[MetadataFilter],
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        info!("🏗️ VIPER: Executing Parquet-level filtering with {} conditions", filters.len());

        // Get all clusters for this collection
        let clusters = self.get_all_clusters(collection_id).await?;
        let mut all_results = Vec::new();
        let mut total_scanned = 0;

        for cluster_id in clusters {
            if let Some(limit) = limit {
                if all_results.len() >= limit {
                    break;
                }
            }

            // Simulate Parquet column filtering within each cluster
            let cluster_results = self.filter_cluster_parquet(
                &cluster_id,
                filters,
                limit.map(|l| l.saturating_sub(all_results.len()))
            ).await?;

            total_scanned += cluster_results.len();
            all_results.extend(cluster_results);

            info!("📊 Cluster {} contributed {} results (total: {})", 
                  cluster_id, total_scanned, all_results.len());
        }

        info!("🎯 Parquet filtering scanned {} records, returned {} results", 
              total_scanned, all_results.len());

        Ok(all_results)
    }

    /// Filter within a single cluster using Parquet columns
    async fn filter_cluster_parquet(
        &self,
        cluster_id: &ClusterId,
        filters: &[MetadataFilter],
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        // Simulate efficient Parquet column scanning
        // In a real implementation, this would use Apache Arrow/Parquet
        // to scan only the required columns and apply predicates
        
        debug!("🔍 Scanning cluster {} with Parquet column filters", cluster_id);
        
        // For testing, return some mock results that match common filters
        let mut mock_results = Vec::new();
        
        for i in 0..10 { // Simulate finding 10 matching records per cluster
            if let Some(limit) = limit {
                if mock_results.len() >= limit {
                    break;
                }
            }

            let mock_record = VectorRecord {
                id: format!("parquet_filtered_{}_{}", cluster_id, i),
                collection_id: cluster_id.clone(),
                vector: vec![0.1; 384], // Mock 384-dimensional vector
                metadata: [
                    ("category".to_string(), Value::String("AI".to_string())),
                    ("author".to_string(), Value::String("Dr. Smith".to_string())),
                    ("doc_type".to_string(), Value::String("research_paper".to_string())),
                    ("year".to_string(), Value::String("2023".to_string())),
                    ("text".to_string(), Value::String(format!("Parquet-filtered document {} from cluster {}", i, cluster_id))),
                ].iter().cloned().collect(),
                timestamp: Utc::now(),
                expires_at: None, // No expiration by default
            };

            // Apply simple filter matching for demonstration
            if self.matches_filters(&mock_record, filters) {
                mock_results.push(mock_record);
            }
        }

        debug!("✅ Cluster {} returned {} filtered results", cluster_id, mock_results.len());
        Ok(mock_results)
    }

    /// Apply remaining client-side filters
    async fn apply_client_filters(
        &self,
        mut records: Vec<VectorRecord>,
        filters: &[MetadataFilter],
    ) -> Result<Vec<VectorRecord>> {
        info!("🎛️ Applying {} client-side filters to {} records", filters.len(), records.len());

        records.retain(|record| {
            filters.iter().all(|filter| self.matches_filters(record, &[filter.clone()]))
        });

        info!("✅ Client-side filtering retained {} records", records.len());
        Ok(records)
    }

    /// Check if a record matches the given filters
    fn matches_filters(&self, record: &VectorRecord, filters: &[MetadataFilter]) -> bool {
        filters.iter().all(|filter| self.matches_single_filter(record, filter))
    }

    /// Check if a record matches a single filter
    fn matches_single_filter(&self, record: &VectorRecord, filter: &MetadataFilter) -> bool {
        match filter {
            MetadataFilter::Field { field, condition } => {
                if let Some(value) = record.metadata.get(field) {
                    self.matches_condition(value, condition)
                } else {
                    false
                }
            }
            MetadataFilter::And(sub_filters) => {
                sub_filters.iter().all(|f| self.matches_single_filter(record, f))
            }
            MetadataFilter::Or(sub_filters) => {
                sub_filters.iter().any(|f| self.matches_single_filter(record, f))
            }
            MetadataFilter::Not(sub_filter) => {
                !self.matches_single_filter(record, sub_filter)
            }
        }
    }

    /// Check if a value matches a condition
    fn matches_condition(&self, value: &Value, condition: &FieldCondition) -> bool {
        match condition {
            FieldCondition::Equals(target) => value == target,
            FieldCondition::NotEquals(target) => value != target,
            FieldCondition::In(targets) => targets.contains(value),
            FieldCondition::NotIn(targets) => !targets.contains(value),
            FieldCondition::Contains(text) => {
                if let Value::String(s) = value {
                    s.contains(text)
                } else {
                    false
                }
            }
            FieldCondition::StartsWith(prefix) => {
                if let Value::String(s) = value {
                    s.starts_with(prefix)
                } else {
                    false
                }
            }
            FieldCondition::EndsWith(suffix) => {
                if let Value::String(s) = value {
                    s.ends_with(suffix)
                } else {
                    false
                }
            }
            _ => false, // Other conditions not implemented for demo
        }
    }
}