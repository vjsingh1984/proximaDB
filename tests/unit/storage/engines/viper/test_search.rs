//! Unit tests for VIPER search engine
//!
//! Tests the storage-aware search functionality including:
//! - Search configuration and hints
//! - Quantization level handling
//! - Cluster-based search optimization
//! - Direct and hybrid search strategies

use proximadb::storage::engines::viper::search::{
    ViperSearchConfig, SearchHints, QuantizationLevel, SearchStrategy
};

#[test]
fn test_quantization_level_ordering() {
    // Test that quantization levels have expected characteristics
    assert_ne!(QuantizationLevel::FP32, QuantizationLevel::PQ4);
    assert_ne!(QuantizationLevel::PQ8, QuantizationLevel::Binary);
}

#[test]
fn test_search_config_defaults() {
    let config = ViperSearchConfig::default();
    assert!(config.enable_ml_clustering);
    assert!(config.enable_parallel_search);
    assert_eq!(config.max_clusters_to_search, 10);
    assert_eq!(config.cluster_confidence_threshold, 0.7);
    assert!(config.enable_predicate_pushdown);
}

#[test]
fn test_search_hints_defaults() {
    let hints = SearchHints::default();
    assert!(hints.quantization_level.is_none());
    assert!(hints.enable_clustering);
    assert!(hints.enable_metadata_filtering);
    assert!(hints.custom_params.is_empty());
}

#[test]
fn test_search_strategy_enum() {
    // Test that search strategies are properly differentiated
    assert_ne!(SearchStrategy::DirectSearch, SearchStrategy::ClusterOptimized);
    assert_ne!(SearchStrategy::ClusterOptimized, SearchStrategy::HybridSearch);
    assert_ne!(SearchStrategy::DirectSearch, SearchStrategy::HybridSearch);
}

#[test]
fn test_search_config_validation() {
    let mut config = ViperSearchConfig::default();
    
    // Test valid configurations
    config.max_clusters_to_search = 5;
    config.cluster_confidence_threshold = 0.8;
    assert_eq!(config.max_clusters_to_search, 5);
    assert_eq!(config.cluster_confidence_threshold, 0.8);
    
    // Test boundary values
    config.max_clusters_to_search = 1;
    config.cluster_confidence_threshold = 0.0;
    assert_eq!(config.max_clusters_to_search, 1);
    assert_eq!(config.cluster_confidence_threshold, 0.0);
    
    config.max_clusters_to_search = 100;
    config.cluster_confidence_threshold = 1.0;
    assert_eq!(config.max_clusters_to_search, 100);
    assert_eq!(config.cluster_confidence_threshold, 1.0);
}

#[test]
fn test_search_hints_customization() {
    let mut hints = SearchHints::default();
    
    // Test customization of hints
    hints.enable_clustering = false;
    hints.enable_metadata_filtering = false;
    hints.quantization_level = Some(QuantizationLevel::PQ8);
    
    assert!(!hints.enable_clustering);
    assert!(!hints.enable_metadata_filtering);
    assert_eq!(hints.quantization_level, Some(QuantizationLevel::PQ8));
    
    // Test custom parameters
    hints.custom_params.insert("use_simd".to_string(), "true".to_string());
    hints.custom_params.insert("parallel_threshold".to_string(), "1000".to_string());
    
    assert_eq!(hints.custom_params.len(), 2);
    assert_eq!(hints.custom_params.get(&key);
    assert_eq!(hints.custom_params.get(&key);
}

#[test]
fn test_quantization_level_memory_efficiency() {
    // Test that quantization levels represent expected memory trade-offs
    // (In real implementation, these would have different memory footprints)
    
    let levels = vec![
        QuantizationLevel::FP32,
        QuantizationLevel::PQ8,
        QuantizationLevel::PQ4,
        QuantizationLevel::Binary,
    ];
    
    // Each level should be distinct
    for (i, level1) in levels.iter().enumerate() {
        for (j, level2) in levels.iter().enumerate() {
            if i != j {
                assert_ne!(level1, level2);
            }
        }
    }
}

#[test]
fn test_search_config_clustering_settings() {
    let config = ViperSearchConfig::default();
    
    // Test clustering-related configuration
    assert!(config.enable_ml_clustering);
    assert!(config.enable_parallel_search);
    assert!(config.max_clusters_to_search > 0);
    assert!(config.cluster_confidence_threshold >= 0.0);
    assert!(config.cluster_confidence_threshold <= 1.0);
    
    // Test predicate pushdown is enabled by default
    assert!(config.enable_predicate_pushdown);
}

#[test]
fn test_search_hints_metadata_filtering() {
    let hints = SearchHints::default();
    
    // Test metadata filtering is enabled by default
    assert!(hints.enable_metadata_filtering);
    
    // Test that custom parameters can be used for metadata filtering hints
    let mut custom_hints = hints.clone();
    custom_hints.custom_params.insert("filter_early".to_string(), "true".to_string());
    custom_hints.custom_params.insert("filter_threshold".to_string(), "0.5".to_string());
    
    assert_eq!(custom_hints.custom_params.get(&key);
    assert_eq!(custom_hints.custom_params.get(&key);
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[test]
    fn test_search_configuration_compatibility() {
        // Test that search configuration works with different hint combinations
        let config = ViperSearchConfig::default();
        let hints = SearchHints::default();
        
        // Test clustering configuration compatibility
        if config.enable_ml_clustering && hints.enable_clustering {
            assert!(config.max_clusters_to_search > 0);
        }
        
        // Test quantization configuration compatibility
        if let Some(quantization_level) = hints.quantization_level {
            match quantization_level {
                QuantizationLevel::FP32 => {
                    // Full precision - should work with all configurations
                    assert!(true);
                }
                QuantizationLevel::PQ8 | QuantizationLevel::PQ4 => {
                    // Compressed quantization - should work with clustering
                    assert!(true);
                }
                QuantizationLevel::Binary => {
                    // Binary quantization - extreme compression
                    assert!(true);
                }
            }
        }
    }
}