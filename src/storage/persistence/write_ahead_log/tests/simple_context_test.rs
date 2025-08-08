//! Simple test for BackgroundFlushContext optimization

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::background_flush_context::{
        BackgroundFlushContext, StorageEngineType, CompressionConfig, OperationPriority
    };
    use crate::compute::distance_computation::DistanceMetric;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_background_flush_context_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: BackgroundFlushContext creation and validation");
        
        let context = BackgroundFlushContext {
            collection_id: "test_collection".to_string(),
            storage_engine: StorageEngineType::Viper,
            base_location: "file:///tmp/test".to_string(),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine,
            compression_config: CompressionConfig::default(),
            filterable_columns: Vec::new(),
            quantization_config: None,
            batch_size_hint: Some(1000),
            priority: OperationPriority::Normal,
            timeout_ms: Some(60_000),
            extra_metadata: HashMap::new(),
        };
        
        // Validate context fields
        assert_eq!(context.collection_id, "test_collection");
        assert_eq!(context.storage_engine, StorageEngineType::Viper);
        assert_eq!(context.dimension, 384);
        assert_eq!(context.distance_metric, DistanceMetric::Cosine);
        assert_eq!(context.engine_name(), "viper");
        
        // Test SST engine as well
        let sst_context = BackgroundFlushContext {
            collection_id: "sst_collection".to_string(),
            storage_engine: StorageEngineType::Sst,
            base_location: "file:///tmp/test".to_string(),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine,
            compression_config: CompressionConfig::default(),
            filterable_columns: Vec::new(),
            quantization_config: None,
            batch_size_hint: Some(500),
            priority: OperationPriority::Normal,
            timeout_ms: Some(60_000),
            extra_metadata: HashMap::new(),
        };
        assert_eq!(sst_context.engine_name(), "sst");
        
        println!("✅ BackgroundFlushContext creation test passed");
    }

    #[tokio::test]
    async fn test_context_performance_settings() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        println!("🧪 TEST: Context performance configuration");
        
        let viper_context = BackgroundFlushContext {
            collection_id: "perf_test".to_string(),
            storage_engine: StorageEngineType::Viper,
            base_location: "file:///tmp/test".to_string(),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine,
            compression_config: CompressionConfig::default(),
            filterable_columns: Vec::new(),
            quantization_config: None,
            batch_size_hint: Some(1000),
            priority: OperationPriority::Normal,
            timeout_ms: Some(60_000),
            extra_metadata: HashMap::new(),
        };
        
        // Test dimension-based optimizations
        assert_eq!(viper_context.dimension, 384);
        
        // Test batch size hint calculation
        assert!(viper_context.batch_size_hint.is_some());
        let batch_size = viper_context.batch_size_hint.unwrap();
        assert!(batch_size > 0 && batch_size <= 10000);
        
        // Test row group size optimization for VIPER
        let row_group_size = viper_context.row_group_size();
        assert!(row_group_size >= 1000 && row_group_size <= 50000);
        
        // Test flush threshold optimization
        let flush_threshold = viper_context.flush_threshold();
        assert!(flush_threshold >= 10000 && flush_threshold <= 100000);
        
        println!("✅ Context performance configuration test passed");
    }
}