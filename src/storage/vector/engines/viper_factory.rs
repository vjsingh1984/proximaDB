// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Factory and Configuration - Consolidated Implementation
//!
//! 🔥 PHASE 4.3: This consolidates 3 VIPER configuration files into a unified implementation:
//! - `config.rs` → VIPER configuration structures and builders
//! - `factory.rs` → Factory patterns for creating strategies and processors
//! - `schema.rs` → Schema generation strategies with dynamic field support
//!
//! ## Key Features
//! - **Builder Pattern**: Flexible configuration with sensible defaults
//! - **Factory Pattern**: Automatic strategy selection based on collection characteristics
//! - **Strategy Pattern**: Pluggable schema generation and processing strategies
//! - **Adaptive Selection**: ML-driven optimization based on data patterns

use anyhow::Result;
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::{CollectionId, VectorRecord};
use crate::storage::vector::types::*;

/// VIPER Factory - Main entry point for creating VIPER components
pub struct ViperFactory {
    /// Default configuration
    default_config: ViperConfiguration,
    
    /// Strategy registry
    strategy_registry: HashMap<String, Box<dyn SchemaStrategyFactory>>,
    
    /// Processor registry
    processor_registry: HashMap<String, Box<dyn ProcessorFactory>>,
    
    /// Configuration builder cache
    builder_cache: HashMap<String, ViperConfigurationBuilder>,
}

/// Complete VIPER configuration
#[derive(Debug, Clone)]
pub struct ViperConfiguration {
    /// Core storage configuration
    pub storage_config: ViperStorageConfig,
    
    /// Schema generation configuration
    pub schema_config: ViperSchemaConfig,
    
    /// Processing configuration
    pub processing_config: ViperProcessingConfig,
    
    /// TTL configuration
    pub ttl_config: TTLConfig,
    
    /// Compaction configuration
    pub compaction_config: CompactionConfig,
    
    /// Performance optimization settings
    pub optimization_config: OptimizationConfig,
}

/// VIPER storage configuration
#[derive(Debug, Clone)]
pub struct ViperStorageConfig {
    /// Enable VIPER clustering
    pub enable_clustering: bool,
    
    /// Number of clusters for partitioning (0 = auto-detect)
    pub cluster_count: usize,
    
    /// Minimum vectors per partition
    pub min_vectors_per_partition: usize,
    
    /// Maximum vectors per partition
    pub max_vectors_per_partition: usize,
    
    /// Enable dictionary encoding for vector IDs
    pub enable_dictionary_encoding: bool,
    
    /// Target compression ratio (0.0 = no compression, 1.0 = maximum)
    pub target_compression_ratio: f64,
    
    /// Parquet block size in bytes
    pub parquet_block_size: usize,
    
    /// Row group size for Parquet files
    pub row_group_size: usize,
    
    /// Enable column statistics
    pub enable_column_stats: bool,
    
    /// Enable bloom filters for string columns
    pub enable_bloom_filters: bool,
}

/// VIPER schema configuration
#[derive(Debug, Clone)]
pub struct ViperSchemaConfig {
    /// Enable dynamic schema generation
    pub enable_dynamic_schema: bool,
    
    /// Maximum filterable metadata fields
    pub max_filterable_fields: usize,
    
    /// Enable TTL fields in schema
    pub enable_ttl_fields: bool,
    
    /// Enable extra metadata map
    pub enable_extra_metadata: bool,
    
    /// Schema version
    pub schema_version: u32,
    
    /// Enable schema evolution
    pub enable_schema_evolution: bool,
}

/// VIPER processing configuration
#[derive(Debug, Clone)]
pub struct ViperProcessingConfig {
    /// Enable preprocessing optimizations
    pub enable_preprocessing: bool,
    
    /// Enable postprocessing optimizations
    pub enable_postprocessing: bool,
    
    /// Processing strategy selection mode
    pub strategy_selection: StrategySelectionMode,
    
    /// Batch processing configuration
    pub batch_config: BatchProcessingConfig,
    
    /// Enable ML-driven optimizations
    pub enable_ml_optimizations: bool,
}

/// TTL (Time-To-Live) configuration
#[derive(Debug, Clone)]
pub struct TTLConfig {
    /// Enable TTL functionality
    pub enabled: bool,
    
    /// Default TTL duration for vectors
    pub default_ttl: Option<Duration>,
    
    /// Background cleanup interval
    pub cleanup_interval: Duration,
    
    /// Maximum number of expired vectors to clean per batch
    pub max_cleanup_batch_size: usize,
    
    /// Enable TTL-based Parquet file filtering during reads
    pub enable_file_level_filtering: bool,
    
    /// Minimum expiration age before file deletion
    pub min_expiration_age: Duration,
}

/// Compaction configuration
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    pub enabled: bool,
    
    /// Minimum number of files to trigger compaction
    pub min_files_for_compaction: usize,
    
    /// Maximum average file size for compaction trigger (KB)
    pub max_avg_file_size_kb: usize,
    
    /// Maximum files to compact in a single operation
    pub max_files_per_compaction: usize,
    
    /// Target file size after compaction (MB)
    pub target_file_size_mb: usize,
    
    /// Enable ML-guided compaction
    pub enable_ml_guidance: bool,
    
    /// Compaction priority scheduling
    pub priority_scheduling: bool,
}

/// Performance optimization configuration
#[derive(Debug, Clone)]
pub struct OptimizationConfig {
    /// Enable adaptive clustering
    pub enable_adaptive_clustering: bool,
    
    /// Enable compression optimization
    pub enable_compression_optimization: bool,
    
    /// Enable feature importance analysis
    pub enable_feature_importance: bool,
    
    /// Enable access pattern learning
    pub enable_access_pattern_learning: bool,
    
    /// Optimization update interval
    pub optimization_interval_secs: u64,
}

/// Strategy selection mode
#[derive(Debug, Clone)]
pub enum StrategySelectionMode {
    /// Automatic selection based on collection characteristics
    Adaptive,
    
    /// Fixed strategy selection
    Fixed(String),
    
    /// ML-driven strategy selection
    MLGuided,
    
    /// User-specified strategy
    UserDefined(String),
}

/// Batch processing configuration
#[derive(Debug, Clone)]
pub struct BatchProcessingConfig {
    /// Default batch size
    pub default_batch_size: usize,
    
    /// Maximum batch size
    pub max_batch_size: usize,
    
    /// Enable dynamic batch sizing
    pub enable_dynamic_sizing: bool,
    
    /// Batch timeout in milliseconds
    pub batch_timeout_ms: u64,
}

/// Configuration builder with fluent interface
#[derive(Debug, Clone)]
pub struct ViperConfigurationBuilder {
    config: ViperConfiguration,
}

/// Schema generation strategy trait
pub trait SchemaGenerationStrategy: Send + Sync {
    /// Generate schema for records
    fn generate_schema(&self) -> Result<Arc<Schema>>;
    
    /// Get filterable fields
    fn get_filterable_fields(&self) -> &[String];
    
    /// Get collection ID
    fn get_collection_id(&self) -> &CollectionId;
    
    /// Check if TTL is supported
    fn supports_ttl(&self) -> bool;
    
    /// Get schema version
    fn get_version(&self) -> u32;
    
    /// Get strategy name
    fn name(&self) -> &'static str;
}

/// Schema strategy factory trait
pub trait SchemaStrategyFactory: Send + Sync {
    /// Create strategy instance
    fn create_strategy(&self, config: &ViperSchemaConfig, collection_id: &CollectionId) -> Box<dyn SchemaGenerationStrategy>;
    
    /// Get factory name
    fn name(&self) -> &'static str;
}

/// Processor factory trait
pub trait ProcessorFactory: Send + Sync {
    /// Create processor instance
    fn create_processor(&self, config: &ViperProcessingConfig) -> Box<dyn VectorProcessor>;
    
    /// Get factory name
    fn name(&self) -> &'static str;
}

/// Vector processor trait
#[async_trait::async_trait]
pub trait VectorProcessor: Send + Sync {
    /// Process vector records
    async fn process_records(&self, records: Vec<VectorRecord>) -> Result<ProcessedBatch>;
    
    /// Get processor name
    fn name(&self) -> &'static str;
}

/// Processed batch result
#[derive(Debug)]
pub struct ProcessedBatch {
    pub records: Vec<VectorRecord>,
    pub schema: Arc<Schema>,
    pub statistics: ProcessingStatistics,
}

/// Processing statistics
#[derive(Debug, Default)]
pub struct ProcessingStatistics {
    pub records_processed: usize,
    pub processing_time_ms: u64,
    pub optimization_applied: Vec<String>,
    pub compression_ratio: f32,
}

// Concrete Strategy Implementations

/// VIPER schema strategy
pub struct ViperSchemaStrategy {
    collection_id: CollectionId,
    config: ViperSchemaConfig,
    filterable_fields: Vec<String>,
}

/// Legacy schema strategy for backward compatibility
pub struct LegacySchemaStrategy {
    collection_id: CollectionId,
    config: ViperSchemaConfig,
}

/// Time-series optimized schema strategy
pub struct TimeSeriesSchemaStrategy {
    collection_id: CollectionId,
    config: ViperSchemaConfig,
    time_precision: TimePrecision,
}

/// Time precision for time-series data
#[derive(Debug, Clone)]
pub enum TimePrecision {
    Millisecond,
    Microsecond,
    Nanosecond,
}

// Concrete Processor Implementations

/// Standard vector record processor
pub struct StandardVectorProcessor {
    config: ViperProcessingConfig,
}

/// Time-series optimized processor
pub struct TimeSeriesVectorProcessor {
    config: ViperProcessingConfig,
    time_window_seconds: u64,
}

/// Similarity-based processor for clustering
pub struct SimilarityVectorProcessor {
    config: ViperProcessingConfig,
    cluster_threshold: f32,
}

// Factory Implementations

impl ViperFactory {
    /// Create new VIPER factory with default configuration
    pub fn new() -> Self {
        let mut factory = Self {
            default_config: ViperConfiguration::default(),
            strategy_registry: HashMap::new(),
            processor_registry: HashMap::new(),
            builder_cache: HashMap::new(),
        };
        
        factory.register_default_strategies();
        factory.register_default_processors();
        factory
    }
    
    /// Create VIPER components for a collection
    pub fn create_for_collection(
        &self,
        collection_id: &CollectionId,
        collection_config: Option<&CollectionConfig>,
    ) -> Result<ViperComponents> {
        info!("🏭 VIPER Factory: Creating components for collection {}", collection_id);
        
        // Step 1: Build configuration based on collection characteristics
        let config = if let Some(collection_config) = collection_config {
            self.build_adaptive_configuration(collection_config)?
        } else {
            self.default_config.clone()
        };
        
        // Step 2: Select and create schema strategy
        let strategy_name = self.select_schema_strategy(&config, collection_config);
        let schema_strategy = self.create_schema_strategy(&strategy_name, &config, collection_id)?;
        
        // Step 3: Select and create processor
        let processor_name = self.select_processor_strategy(&config, collection_config);
        let processor = self.create_processor(&processor_name, &config)?;
        
        // Step 4: Create configuration builder for runtime adjustments
        let builder = ViperConfigurationBuilder::from_config(config.clone());
        
        debug!("✅ VIPER Factory: Created components with strategy '{}' and processor '{}'", 
               strategy_name, processor_name);
        
        Ok(ViperComponents {
            configuration: config,
            schema_strategy,
            processor,
            builder,
        })
    }
    
    /// Register custom schema strategy
    pub fn register_schema_strategy(&mut self, name: String, factory: Box<dyn SchemaStrategyFactory>) {
        self.strategy_registry.insert(name, factory);
    }
    
    /// Register custom processor
    pub fn register_processor(&mut self, name: String, factory: Box<dyn ProcessorFactory>) {
        self.processor_registry.insert(name, factory);
    }
    
    fn register_default_strategies(&mut self) {
        self.strategy_registry.insert("viper".to_string(), Box::new(ViperSchemaStrategyFactory));
        self.strategy_registry.insert("legacy".to_string(), Box::new(LegacySchemaStrategyFactory));
        self.strategy_registry.insert("timeseries".to_string(), Box::new(TimeSeriesSchemaStrategyFactory));
    }
    
    fn register_default_processors(&mut self) {
        self.processor_registry.insert("standard".to_string(), Box::new(StandardProcessorFactory));
        self.processor_registry.insert("timeseries".to_string(), Box::new(TimeSeriesProcessorFactory));
        self.processor_registry.insert("similarity".to_string(), Box::new(SimilarityProcessorFactory));
    }
    
    fn build_adaptive_configuration(&self, collection_config: &CollectionConfig) -> Result<ViperConfiguration> {
        let builder = ViperConfigurationBuilder::new()
            .with_clustering_enabled(!collection_config.filterable_metadata_fields.is_empty())
            .with_ttl_enabled(true)
            .with_compression_optimization(true)
            .with_ml_optimizations(true);
        
        Ok(builder.build())
    }
    
    fn select_schema_strategy(&self, _config: &ViperConfiguration, collection_config: Option<&CollectionConfig>) -> String {
        if let Some(config) = collection_config {
            if !config.filterable_metadata_fields.is_empty() {
                "viper".to_string()
            } else {
                "legacy".to_string()
            }
        } else {
            "viper".to_string()
        }
    }
    
    fn select_processor_strategy(&self, config: &ViperConfiguration, _collection_config: Option<&CollectionConfig>) -> String {
        match &config.processing_config.strategy_selection {
            StrategySelectionMode::Adaptive => "standard".to_string(),
            StrategySelectionMode::Fixed(name) => name.clone(),
            StrategySelectionMode::MLGuided => "similarity".to_string(),
            StrategySelectionMode::UserDefined(name) => name.clone(),
        }
    }
    
    fn create_schema_strategy(
        &self,
        strategy_name: &str,
        config: &ViperConfiguration,
        collection_id: &CollectionId,
    ) -> Result<Box<dyn SchemaGenerationStrategy>> {
        let factory = self.strategy_registry.get(strategy_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown schema strategy: {}", strategy_name))?;
        
        Ok(factory.create_strategy(&config.schema_config, collection_id))
    }
    
    fn create_processor(
        &self,
        processor_name: &str,
        config: &ViperConfiguration,
    ) -> Result<Box<dyn VectorProcessor>> {
        let factory = self.processor_registry.get(processor_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown processor: {}", processor_name))?;
        
        Ok(factory.create_processor(&config.processing_config))
    }
}

/// Complete VIPER components for a collection
pub struct ViperComponents {
    pub configuration: ViperConfiguration,
    pub schema_strategy: Box<dyn SchemaGenerationStrategy>,
    pub processor: Box<dyn VectorProcessor>,
    pub builder: ViperConfigurationBuilder,
}

/// Collection configuration (simplified)
#[derive(Debug, Clone)]
pub struct CollectionConfig {
    pub name: String,
    pub filterable_metadata_fields: Vec<String>,
    pub vector_dimension: Option<usize>,
    pub estimated_size: Option<usize>,
}

impl ViperConfigurationBuilder {
    /// Create new builder with defaults
    pub fn new() -> Self {
        Self {
            config: ViperConfiguration::default(),
        }
    }
    
    /// Create builder from existing configuration
    pub fn from_config(config: ViperConfiguration) -> Self {
        Self { config }
    }
    
    /// Enable/disable clustering
    pub fn with_clustering_enabled(mut self, enabled: bool) -> Self {
        self.config.storage_config.enable_clustering = enabled;
        self
    }
    
    /// Set cluster count
    pub fn with_cluster_count(mut self, count: usize) -> Self {
        self.config.storage_config.cluster_count = count;
        self
    }
    
    /// Enable/disable TTL
    pub fn with_ttl_enabled(mut self, enabled: bool) -> Self {
        self.config.ttl_config.enabled = enabled;
        self.config.schema_config.enable_ttl_fields = enabled;
        self
    }
    
    /// Set compression ratio target
    pub fn with_compression_ratio(mut self, ratio: f64) -> Self {
        self.config.storage_config.target_compression_ratio = ratio;
        self
    }
    
    /// Enable compression optimization
    pub fn with_compression_optimization(mut self, enabled: bool) -> Self {
        self.config.optimization_config.enable_compression_optimization = enabled;
        self
    }
    
    /// Enable ML optimizations
    pub fn with_ml_optimizations(mut self, enabled: bool) -> Self {
        self.config.processing_config.enable_ml_optimizations = enabled;
        self.config.optimization_config.enable_adaptive_clustering = enabled;
        self.config.optimization_config.enable_feature_importance = enabled;
        self
    }
    
    /// Set strategy selection mode
    pub fn with_strategy_selection(mut self, mode: StrategySelectionMode) -> Self {
        self.config.processing_config.strategy_selection = mode;
        self
    }
    
    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.config.processing_config.batch_config.default_batch_size = size;
        self
    }
    
    /// Build final configuration
    pub fn build(self) -> ViperConfiguration {
        self.config
    }
}

// Schema Strategy Implementations

impl ViperSchemaStrategy {
    pub fn new(collection_id: CollectionId, config: ViperSchemaConfig, filterable_fields: Vec<String>) -> Self {
        Self {
            collection_id,
            config,
            filterable_fields,
        }
    }
}

impl SchemaGenerationStrategy for ViperSchemaStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>> {
        let mut fields = Vec::new();
        
        // Core required fields
        fields.push(Field::new("id", DataType::Utf8, false));
        fields.push(Field::new(
            "vectors",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
            false,
        ));
        
        // Dynamic filterable metadata columns
        for field_name in &self.filterable_fields {
            fields.push(Field::new(
                &format!("meta_{}", field_name),
                DataType::Utf8,
                true,
            ));
        }
        
        // Extra metadata as Map type
        if self.config.enable_extra_metadata {
            let map_field = Field::new(
                "extra_meta",
                DataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        DataType::Struct(
                            vec![
                                Field::new("key", DataType::Utf8, false),
                                Field::new("value", DataType::Utf8, true),
                            ].into(),
                        ),
                        false,
                    )),
                    false,
                ),
                true,
            );
            fields.push(map_field);
        }
        
        // TTL fields
        if self.config.enable_ttl_fields {
            fields.push(Field::new(
                "expires_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            ));
        }
        
        // Timestamps
        fields.push(Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ));
        fields.push(Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ));
        
        Ok(Arc::new(Schema::new(fields)))
    }
    
    fn get_filterable_fields(&self) -> &[String] {
        &self.filterable_fields
    }
    
    fn get_collection_id(&self) -> &CollectionId {
        &self.collection_id
    }
    
    fn supports_ttl(&self) -> bool {
        self.config.enable_ttl_fields
    }
    
    fn get_version(&self) -> u32 {
        self.config.schema_version
    }
    
    fn name(&self) -> &'static str {
        "ViperSchemaStrategy"
    }
}

impl LegacySchemaStrategy {
    pub fn new(collection_id: CollectionId, config: ViperSchemaConfig) -> Self {
        Self { collection_id, config }
    }
}

impl SchemaGenerationStrategy for LegacySchemaStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>> {
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vectors",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
                false,
            ),
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
        ];
        
        Ok(Arc::new(Schema::new(fields)))
    }
    
    fn get_filterable_fields(&self) -> &[String] {
        &[]
    }
    
    fn get_collection_id(&self) -> &CollectionId {
        &self.collection_id
    }
    
    fn supports_ttl(&self) -> bool {
        false
    }
    
    fn get_version(&self) -> u32 {
        1
    }
    
    fn name(&self) -> &'static str {
        "LegacySchemaStrategy"
    }
}

// Processor Implementations

impl StandardVectorProcessor {
    pub fn new(config: ViperProcessingConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]\nimpl VectorProcessor for StandardVectorProcessor {
    async fn process_records(&self, mut records: Vec<VectorRecord>) -> Result<ProcessedBatch> {
        let start_time = std::time::Instant::now();
        
        // Standard processing: sort by timestamp
        if self.config.enable_preprocessing {
            records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        }
        
        let processing_time = start_time.elapsed().as_millis() as u64;
        
        // Generate schema (placeholder)
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
        ]));
        
        Ok(ProcessedBatch {
            records,
            schema,
            statistics: ProcessingStatistics {
                records_processed: records.len(),
                processing_time_ms: processing_time,
                optimization_applied: vec!["timestamp_sorting".to_string()],
                compression_ratio: 1.0,
            },
        })
    }
    
    fn name(&self) -> &'static str {
        "StandardVectorProcessor"
    }
}

// Factory Implementations

pub struct ViperSchemaStrategyFactory;
pub struct LegacySchemaStrategyFactory;
pub struct TimeSeriesSchemaStrategyFactory;

pub struct StandardProcessorFactory;
pub struct TimeSeriesProcessorFactory;
pub struct SimilarityProcessorFactory;

impl SchemaStrategyFactory for ViperSchemaStrategyFactory {
    fn create_strategy(&self, config: &ViperSchemaConfig, collection_id: &CollectionId) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(ViperSchemaStrategy::new(
            collection_id.clone(),
            config.clone(),
            vec!["category".to_string(), "type".to_string()], // Default filterable fields
        ))
    }
    
    fn name(&self) -> &'static str {
        "ViperSchemaStrategyFactory"
    }
}

impl SchemaStrategyFactory for LegacySchemaStrategyFactory {
    fn create_strategy(&self, config: &ViperSchemaConfig, collection_id: &CollectionId) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(LegacySchemaStrategy::new(collection_id.clone(), config.clone()))
    }
    
    fn name(&self) -> &'static str {
        "LegacySchemaStrategyFactory"
    }
}

impl SchemaStrategyFactory for TimeSeriesSchemaStrategyFactory {
    fn create_strategy(&self, config: &ViperSchemaConfig, collection_id: &CollectionId) -> Box<dyn SchemaGenerationStrategy> {
        // Placeholder implementation
        Box::new(LegacySchemaStrategy::new(collection_id.clone(), config.clone()))
    }
    
    fn name(&self) -> &'static str {
        "TimeSeriesSchemaStrategyFactory"
    }
}

impl ProcessorFactory for StandardProcessorFactory {
    fn create_processor(&self, config: &ViperProcessingConfig) -> Box<dyn VectorProcessor> {
        Box::new(StandardVectorProcessor::new(config.clone()))
    }
    
    fn name(&self) -> &'static str {
        "StandardProcessorFactory"
    }
}

impl ProcessorFactory for TimeSeriesProcessorFactory {
    fn create_processor(&self, config: &ViperProcessingConfig) -> Box<dyn VectorProcessor> {
        // Placeholder implementation
        Box::new(StandardVectorProcessor::new(config.clone()))
    }
    
    fn name(&self) -> &'static str {
        "TimeSeriesProcessorFactory"
    }
}

impl ProcessorFactory for SimilarityProcessorFactory {
    fn create_processor(&self, config: &ViperProcessingConfig) -> Box<dyn VectorProcessor> {
        // Placeholder implementation
        Box::new(StandardVectorProcessor::new(config.clone()))
    }
    
    fn name(&self) -> &'static str {
        "SimilarityProcessorFactory"
    }
}

// Default implementations

impl Default for ViperConfiguration {
    fn default() -> Self {
        Self {
            storage_config: ViperStorageConfig::default(),
            schema_config: ViperSchemaConfig::default(),
            processing_config: ViperProcessingConfig::default(),
            ttl_config: TTLConfig::default(),
            compaction_config: CompactionConfig::default(),
            optimization_config: OptimizationConfig::default(),
        }
    }
}

impl Default for ViperStorageConfig {
    fn default() -> Self {
        Self {
            enable_clustering: true,
            cluster_count: 0, // Auto-detect
            min_vectors_per_partition: 1000,
            max_vectors_per_partition: 100000,
            enable_dictionary_encoding: true,
            target_compression_ratio: 0.7,
            parquet_block_size: 1024 * 1024, // 1MB
            row_group_size: 1000000,
            enable_column_stats: true,
            enable_bloom_filters: true,
        }
    }
}

impl Default for ViperSchemaConfig {
    fn default() -> Self {
        Self {
            enable_dynamic_schema: true,
            max_filterable_fields: 16,
            enable_ttl_fields: true,
            enable_extra_metadata: true,
            schema_version: 1,
            enable_schema_evolution: true,
        }
    }
}

impl Default for ViperProcessingConfig {
    fn default() -> Self {
        Self {
            enable_preprocessing: true,
            enable_postprocessing: true,
            strategy_selection: StrategySelectionMode::Adaptive,
            batch_config: BatchProcessingConfig::default(),
            enable_ml_optimizations: true,
        }
    }
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl: None,
            cleanup_interval: Duration::hours(1),
            max_cleanup_batch_size: 10000,
            enable_file_level_filtering: true,
            min_expiration_age: Duration::hours(24),
        }
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_files_for_compaction: 2,
            max_avg_file_size_kb: 16 * 1024, // 16MB
            max_files_per_compaction: 5,
            target_file_size_mb: 64,
            enable_ml_guidance: true,
            priority_scheduling: true,
        }
    }
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            enable_adaptive_clustering: true,
            enable_compression_optimization: true,
            enable_feature_importance: true,
            enable_access_pattern_learning: true,
            optimization_interval_secs: 3600, // 1 hour
        }
    }
}

impl Default for BatchProcessingConfig {
    fn default() -> Self {
        Self {
            default_batch_size: 1000,
            max_batch_size: 10000,
            enable_dynamic_sizing: true,
            batch_timeout_ms: 5000,
        }
    }
}