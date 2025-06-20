//! VIPER Factory Pattern Implementation
//!
//! This module provides factory patterns for creating appropriate schema strategies,
//! processors, and adapters based on collection characteristics and requirements.

use super::adapter::{
    CompressedVectorAdapter, TimePrecision, TimeSeriesAdapter, VectorCompressionType,
    VectorRecordSchemaAdapter,
};
use super::config::ViperSchemaBuilder;
use super::processor::{
    SimilarityVectorProcessor, TimeSeriesVectorProcessor, VectorProcessor, VectorRecordProcessor,
};
use super::schema::{LegacySchemaStrategy, SchemaGenerationStrategy, ViperSchemaStrategy};
use crate::schema_types::CollectionConfig;

/// Factory Pattern: Creates appropriate schema strategies
pub struct ViperSchemaFactory;

impl ViperSchemaFactory {
    /// Create default VIPER strategy for a collection
    pub fn create_strategy(
        collection_config: &CollectionConfig,
    ) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(ViperSchemaStrategy::new(collection_config))
    }

    /// Create strategy with custom builder configuration
    pub fn create_strategy_with_builder(
        builder: ViperSchemaBuilder,
        collection_config: &CollectionConfig,
    ) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(builder.build(collection_config))
    }

    /// Create legacy strategy for backward compatibility
    pub fn create_legacy_strategy(collection_id: String) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(LegacySchemaStrategy::new(collection_id))
    }

    /// Create strategy based on collection characteristics
    pub fn create_adaptive_strategy(
        collection_config: &CollectionConfig,
    ) -> Box<dyn SchemaGenerationStrategy> {
        // Auto-select strategy based on collection characteristics
        let has_filterable_fields = !collection_config.filterable_metadata_fields.is_empty();

        if has_filterable_fields {
            // Use VIPER strategy for collections with filterable metadata
            Box::new(ViperSchemaStrategy::new(collection_config))
        } else {
            // Use legacy strategy for simple collections
            Box::new(LegacySchemaStrategy::new(
                collection_config.name.clone(),
            ))
        }
    }
}

/// Factory for creating vector processors
pub struct VectorProcessorFactory;

impl VectorProcessorFactory {
    /// Create default vector processor
    pub fn create_processor<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
    ) -> Box<dyn VectorProcessor + 'a> {
        Box::new(VectorRecordProcessor::new(strategy))
    }

    /// Create time-series optimized processor
    pub fn create_time_series_processor<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        time_window_seconds: u64,
    ) -> Box<dyn VectorProcessor + 'a> {
        Box::new(TimeSeriesVectorProcessor::new(
            strategy,
            time_window_seconds,
        ))
    }

    /// Create similarity-optimized processor
    pub fn create_similarity_processor<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        cluster_threshold: f32,
    ) -> Box<dyn VectorProcessor + 'a> {
        Box::new(SimilarityVectorProcessor::new(strategy, cluster_threshold))
    }

    /// Create processor based on collection characteristics
    pub fn create_adaptive_processor<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        collection_config: &CollectionConfig,
    ) -> Box<dyn VectorProcessor + 'a> {
        // Auto-select processor based on collection characteristics
        let vector_count_estimate = 1000; // Could be derived from collection metadata

        if vector_count_estimate > 100_000 {
            // Use similarity processor for large collections
            Box::new(SimilarityVectorProcessor::new(strategy, 0.8))
        } else if collection_config
            .filterable_metadata_fields
            .iter()
            .any(|f| f.contains("timestamp") || f.contains("time"))
        {
            // Use time-series processor if time-related fields detected
            Box::new(TimeSeriesVectorProcessor::new(strategy, 3600)) // 1-hour windows
        } else {
            // Use default processor
            Box::new(VectorRecordProcessor::new(strategy))
        }
    }
}

/// Factory for creating record adapters
pub struct AdapterFactory;

impl AdapterFactory {
    /// Create default adapter
    pub fn create_adapter<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
    ) -> VectorRecordSchemaAdapter<'a> {
        VectorRecordSchemaAdapter::new(strategy)
    }

    /// Create time-series adapter
    pub fn create_time_series_adapter<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        time_precision: TimePrecision,
    ) -> TimeSeriesAdapter<'a> {
        TimeSeriesAdapter::new(strategy, time_precision)
    }

    /// Create compressed vector adapter
    pub fn create_compressed_adapter<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        compression_type: VectorCompressionType,
    ) -> CompressedVectorAdapter<'a> {
        CompressedVectorAdapter::new(strategy, compression_type)
    }

    /// Create adapter based on collection requirements
    pub fn create_adaptive_adapter<'a>(
        strategy: &'a dyn SchemaGenerationStrategy,
        collection_config: &CollectionConfig,
    ) -> VectorRecordSchemaAdapter<'a> {
        // Could implement adaptive logic based on collection size, dimension, etc.
        // For now, return default adapter
        VectorRecordSchemaAdapter::new(strategy)
    }
}

/// Complete factory for creating entire VIPER processing pipelines
pub struct ViperPipelineFactory;

impl ViperPipelineFactory {
    /// Create a complete processing pipeline for a collection
    pub fn create_pipeline(
        collection_config: &CollectionConfig,
    ) -> Box<dyn SchemaGenerationStrategy> {
        ViperSchemaFactory::create_adaptive_strategy(collection_config)
    }

    /// Create an optimized pipeline for high-throughput scenarios
    pub fn create_high_throughput_pipeline(
        collection_config: &CollectionConfig,
    ) -> Box<dyn SchemaGenerationStrategy> {
        ViperSchemaFactory::create_strategy(collection_config)
    }

    /// Create a pipeline optimized for time-series data
    pub fn create_time_series_pipeline(
        collection_config: &CollectionConfig,
        _time_window_seconds: u64,
    ) -> Box<dyn SchemaGenerationStrategy> {
        ViperSchemaFactory::create_strategy(collection_config)
    }
}
