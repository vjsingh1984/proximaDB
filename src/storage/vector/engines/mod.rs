// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Storage Engines
//!
//! This module provides unified interfaces for different vector storage engines.
//! Each engine focuses on specific use cases and performance characteristics.

pub mod viper_core;
pub mod viper_pipeline;
pub mod viper_factory;
pub mod viper_utilities;

pub use viper_core::{
    ViperCoreEngine, ViperCoreConfig, ViperCollectionMetadata,
    CompressionStats, CompressionConfig, SchemaConfig, AtomicOperationsConfig,
    VectorStorageFormat, CompressionAlgorithm, ViperCoreStats,
};

pub use viper_pipeline::{
    ViperPipeline, ViperPipelineConfig, ProcessingConfig, FlushingConfig,
    VectorRecordProcessor, ParquetFlusher, CompactionEngine, FlushResult,
    CompactionTask, CompactionType, CompactionPriority, SortingStrategy,
    ViperPipelineStats,
};

pub use viper_factory::{
    ViperFactory, ViperConfiguration, ViperConfigurationBuilder, ViperComponents,
    ViperStorageConfig, ViperSchemaConfig, ViperProcessingConfig, TTLConfig,
    OptimizationConfig, SchemaGenerationStrategy, VectorProcessor,
    StrategySelectionMode, BatchProcessingConfig, CollectionConfig,
};

pub use viper_utilities::{
    ViperUtilities, ViperUtilitiesConfig, PerformanceStatsCollector, TTLCleanupService,
    StagingOperationsCoordinator, DataPartitioner, CompressionOptimizer,
    OperationStatsCollector, OperationMetrics, GlobalViperStats, TTLStats,
    CompressionRecommendation, PerformanceReport, PartitionedData,
};