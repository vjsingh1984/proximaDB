# Vector Storage Integration Guide

## Overview

This guide provides comprehensive instructions for integrating with the new unified vector storage system in ProximaDB. The consolidation has transformed 21+ fragmented files into a clean, enterprise-grade architecture with 4 core modules.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     VectorStorageCoordinator                     │
│  (Central orchestration for all vector operations)               │
├─────────────────────┬───────────────────┬─────────────────────┤
│   UnifiedSearchEngine│ UnifiedIndexManager│    Storage Engines   │
│  ┌─────────────────┐│┌─────────────────┐│┌───────────────────┐│
│  │ - HNSW          ││ - Multi-index    ││ - ViperCoreEngine ││
│  │ - IVF           ││ - SIMD optimized ││ - LSM Engine      ││
│  │ - Flat          ││ - Auto-optimize  ││ - MMAP Engine     ││
│  │ - Cost-based    ││ - ML-driven      ││ - Cloud Storage   ││
│  └─────────────────┘└─────────────────┘└───────────────────┘│
├─────────────────────┴───────────────────┴─────────────────────┤
│                    Unified Type System                          │
│              (Single source of truth for all types)             │
└─────────────────────────────────────────────────────────────────┘
```

## Quick Start

### 1. Basic Vector Operations

```rust
use proximadb::storage::vector::{
    VectorStorageCoordinator, CoordinatorConfig,
    UnifiedSearchEngine, UnifiedIndexManager,
    VectorOperation, SearchContext
};

// Initialize the coordinator
let config = CoordinatorConfig::default();
let coordinator = VectorStorageCoordinator::new(
    Arc::new(search_engine),
    Arc::new(index_manager),
    config,
).await?;

// Register storage engines
coordinator.register_engine("viper", Box::new(viper_engine)).await?;
coordinator.register_engine("lsm", Box::new(lsm_engine)).await?;

// Insert vectors
let operation = VectorOperation::Insert {
    record: VectorRecord {
        id: "vec_001".to_string(),
        collection_id: "embeddings".to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata: serde_json::json!({"category": "document"}),
        timestamp: Utc::now(),
        expires_at: None,
    },
    options: Default::default(),
};

let result = coordinator.execute_operation(operation).await?;

// Search vectors
let search_context = SearchContext {
    collection_id: "embeddings".to_string(),
    query_vector: vec![0.1, 0.2, 0.3, 0.4],
    k: 10,
    strategy: SearchStrategy::Adaptive {
        query_complexity_score: 0.8,
        time_budget_ms: 100,
        accuracy_preference: 0.95,
    },
    filters: Some(MetadataFilter::field_equals("category", "document")),
    ..Default::default()
};

let results = coordinator.unified_search(search_context).await?;
```

### 2. VIPER Engine Setup

```rust
use proximadb::storage::vector::engines::{
    ViperFactory, ViperConfiguration, ViperConfigurationBuilder
};

// Create VIPER factory
let factory = ViperFactory::new();

// Build adaptive configuration
let config = ViperConfigurationBuilder::new()
    .with_clustering_enabled(true)
    .with_ml_optimizations(true)
    .with_compression_optimization(true)
    .with_ttl_enabled(true)
    .build();

// Create components for collection
let components = factory.create_for_collection(
    &collection_id,
    Some(&collection_config),
).await?;

// Use the created components
let core_engine = components.core_engine;
let pipeline = components.pipeline;
let utilities = components.utilities;
```

## Migration from Legacy System

### Before (Legacy Code)
```rust
// Multiple scattered imports
use crate::storage::viper::storage_engine::ViperStorageEngine;
use crate::storage::viper::search_engine::ViperSearchEngine;
use crate::storage::search_index::SearchIndexManager;
use crate::storage::viper::types::*;

// Separate initialization for each component
let storage = ViperStorageEngine::new(config).await?;
let search = ViperSearchEngine::new(search_config).await?;
let index = SearchIndexManager::new(data_dir);

// Inconsistent interfaces
storage.write(record).await?;
let results = search.search(query, k, filters).await?;
```

### After (Unified System)
```rust
// Single unified import
use proximadb::storage::vector::{
    VectorStorageCoordinator, VectorOperation, SearchContext
};

// Single coordinator initialization
let coordinator = VectorStorageCoordinator::new(
    search_engine,
    index_manager,
    config,
).await?;

// Consistent unified interface
coordinator.execute_operation(VectorOperation::Insert { record, options }).await?;
let results = coordinator.unified_search(search_context).await?;
```

## Advanced Features

### 1. Cross-Engine Search

```rust
// Configure cross-engine search
let search_context = SearchContext {
    collection_id: "embeddings".to_string(),
    query_vector: query_vec,
    k: 100,
    strategy: SearchStrategy::CrossEngine {
        merge_strategy: MergeStrategy::ScoreWeighted,
        max_engines: 3,
        timeout_ms: 1000,
    },
    ..Default::default()
};

// Coordinator automatically searches across all registered engines
let results = coordinator.unified_search(search_context).await?;
```

### 2. ML-Driven Optimization

```rust
use proximadb::storage::vector::engines::ViperUtilities;

// Get performance analytics
let analytics = coordinator.get_performance_analytics(
    Some(&collection_id)
).await?;

// ML-driven recommendations
for recommendation in analytics.recommendations {
    match recommendation.recommendation_type.as_str() {
        "index_rebuild" => {
            // Rebuild index with new parameters
            index_manager.rebuild_index(&collection_id).await?;
        }
        "compression_change" => {
            // Update compression algorithm
            let new_config = config.with_compression_algorithm(
                recommendation.suggested_algorithm
            );
        }
        _ => {}
    }
}
```

### 3. Background Services

```rust
// Initialize utilities with background services
let utilities_config = ViperUtilitiesConfig {
    enable_background_services: true,
    ttl_config: TTLConfig {
        enabled: true,
        cleanup_interval: Duration::hours(1),
        ..Default::default()
    },
    compression_config: CompressionConfig {
        enable_adaptive_compression: true,
        ..Default::default()
    },
    ..Default::default()
};

let mut utilities = ViperUtilities::new(utilities_config, filesystem).await?;

// Start all background services
utilities.start_services().await?;

// Services now running:
// - TTL cleanup (automatic expiration)
// - Performance monitoring
// - Compression optimization
// - Staging cleanup
```

### 4. Performance Monitoring

```rust
use proximadb::storage::vector::engines::OperationStatsCollector;

// Create operation collector
let mut collector = OperationStatsCollector::new(
    "batch_insert".to_string(),
    collection_id.clone(),
);

// Track operation phases
collector.start_operation();
collector.start_phase("validation");
// ... validation logic ...
collector.end_phase("validation");

collector.start_phase("processing");
// ... processing logic ...
collector.end_phase("processing");

// Finalize and record metrics
collector.set_records_processed(10000);
collector.set_compression_ratio(0.7);
let metrics = collector.finalize();

// Record to global statistics
utilities.record_operation(metrics).await?;

// Get performance report
let report = utilities.get_performance_stats(Some(&collection_id)).await?;
println!("Average latency: {:.2}ms", report.global_stats.avg_latency_ms);
println!("Compression ratio: {:.2}", report.global_stats.avg_compression_ratio);
```

## Configuration Reference

### CoordinatorConfig
```rust
pub struct CoordinatorConfig {
    /// Default storage engine name
    pub default_engine: String,              // "viper"
    
    /// Enable cross-engine operations
    pub enable_cross_engine: bool,           // true
    
    /// Enable operation caching
    pub enable_caching: bool,                // true
    
    /// Cache size in MB
    pub cache_size_mb: usize,                // 512
    
    /// Enable performance monitoring
    pub enable_monitoring: bool,             // true
    
    /// Routing strategy
    pub routing_strategy: RoutingStrategy,   // CollectionBased
}
```

### SearchStrategy Options
```rust
pub enum SearchStrategy {
    /// Simple k-NN search
    Simple {
        algorithm_hint: Option<String>,
    },
    
    /// Adaptive search with performance requirements
    Adaptive {
        query_complexity_score: f64,    // 0.0-1.0
        time_budget_ms: u64,           // milliseconds
        accuracy_preference: f64,       // 0.0-1.0
    },
    
    /// Cross-engine search
    CrossEngine {
        merge_strategy: MergeStrategy,
        max_engines: usize,
        timeout_ms: u64,
    },
    
    /// Tiered search across storage layers
    Tiered {
        tiers: Vec<StorageTier>,
        early_termination: bool,
    },
}
```

### VIPER Configuration
```rust
pub struct ViperConfiguration {
    /// Storage configuration
    pub storage_config: ViperStorageConfig {
        enable_clustering: bool,
        cluster_count: usize,           // 0 = auto-detect
        compression_ratio: f64,         // 0.0-1.0
        // ... more options
    },
    
    /// Schema configuration
    pub schema_config: ViperSchemaConfig {
        enable_dynamic_schema: bool,
        max_filterable_fields: usize,   // 16
        enable_ttl_fields: bool,
        // ... more options
    },
    
    /// Processing configuration
    pub processing_config: ViperProcessingConfig {
        enable_ml_optimizations: bool,
        batch_size: usize,
        strategy_selection: StrategySelectionMode,
        // ... more options
    },
}
```

## Best Practices

### 1. Engine Registration Order
```rust
// Register engines in priority order
coordinator.register_engine("viper", viper_engine).await?;    // Primary
coordinator.register_engine("lsm", lsm_engine).await?;        // Secondary
coordinator.register_engine("cloud", cloud_engine).await?;    // Fallback
```

### 2. Batch Operations
```rust
// Use batch operations for better performance
let batch_op = VectorOperation::Batch {
    operations: vec![
        BatchOperation::Insert(record1),
        BatchOperation::Insert(record2),
        // ... hundreds more
    ],
    options: BatchOptions {
        atomic: false,  // Allow partial success
        optimize_order: true,  // Reorder for performance
    },
};

coordinator.execute_operation(batch_op).await?;
```

### 3. Filter Optimization
```rust
// Use structured filters for better performance
let filter = MetadataFilter::and(vec![
    MetadataFilter::field_equals("category", "documents"),
    MetadataFilter::field_range("importance", 0.8, 1.0),
    MetadataFilter::field_in("tags", vec!["ml", "ai", "vectors"]),
]);
```

### 4. Resource Management
```rust
// Properly shutdown services
utilities.stop_services().await?;

// Graceful coordinator shutdown
coordinator.shutdown().await?;
```

## Troubleshooting

### Common Issues

1. **Performance Degradation**
   ```rust
   // Check performance analytics
   let analytics = coordinator.get_performance_analytics(None).await?;
   
   // Look for recommendations
   for rec in analytics.recommendations {
       println!("Issue: {} - Solution: {}", 
                rec.recommendation_type, 
                rec.description);
   }
   ```

2. **Memory Usage**
   ```rust
   // Monitor memory usage
   let stats = utilities.get_performance_stats(None).await?;
   println!("Memory usage: {} MB", 
            stats.global_stats.memory_usage_mb);
   
   // Adjust cache size if needed
   coordinator.update_config(|config| {
       config.cache_size_mb = 256;  // Reduce cache
   }).await?;
   ```

3. **Search Accuracy**
   ```rust
   // Adjust search strategy for accuracy
   let context = SearchContext {
       strategy: SearchStrategy::Adaptive {
           query_complexity_score: 0.9,
           time_budget_ms: 500,  // Allow more time
           accuracy_preference: 0.99,  // Prioritize accuracy
       },
       ..context
   };
   ```

## Performance Tuning

### 1. Collection-Specific Optimization
```rust
// Create collection-specific configuration
let collection_config = CollectionConfig {
    name: "high_performance_vectors".to_string(),
    filterable_metadata_fields: vec!["category", "importance"],
    vector_dimension: Some(768),
    estimated_size: Some(10_000_000),  // 10M vectors
};

// Factory creates optimized components
let components = factory.create_for_collection(
    &collection_id,
    Some(&collection_config),
).await?;
```

### 2. Compression Optimization
```rust
// Get compression recommendations
let compression_rec = utilities.optimize_compression(&collection_id).await?;

// Apply recommendations
let new_config = config.compression_config
    .with_algorithm(compression_rec.recommended_algorithm)
    .with_level(compression_rec.recommended_level);
```

### 3. Index Optimization
```rust
// Analyze index performance
let index_stats = index_manager.get_statistics(&collection_id).await?;

if index_stats.fragmentation_ratio > 0.3 {
    // Rebuild index to reduce fragmentation
    index_manager.rebuild_index(&collection_id).await?;
}

// Enable multi-index for large collections
if index_stats.total_vectors > 1_000_000 {
    let auxiliary_spec = IndexSpec::ivf_index("auxiliary", 1000, 100);
    index_manager.add_auxiliary_index(&collection_id, auxiliary_spec).await?;
}
```

## API Reference

See the [API Documentation](./API_REFERENCE.md) for complete reference.

## Support

For issues or questions:
- GitHub Issues: https://github.com/proximadb/proximadb/issues
- Documentation: https://docs.proximadb.com
- Community: https://discord.gg/proximadb