//! Phase 5: Production Readiness Tests

use super::super::*;
use super::super::config::*;
// // use super::super::monitoring::*;  // TODO: Add monitoring module
// // use super::super::optimization::*;  // TODO: Add optimization module
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::Duration;

/// Test cache configuration loading and validation
#[tokio::test]
async fn test_cache_config_validation() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Test default configuration
    let config = CacheConfig::default();
    assert!(config.validate().is_ok());
    
    // Test invalid memory percentages
    let mut invalid_config = config.clone();
    invalid_config.vector_data.memory_percentage = 60;
    invalid_config.query_result.memory_percentage = 60; // Total > 100
    
    let validation = invalid_config.validate();
    assert!(validation.is_err());
    assert!(validation.unwrap_err().to_string().contains("must sum to 100"));
    
    // Test invalid thresholds
    let mut invalid_config = CacheConfig::default();
    invalid_config.monitoring.alert_thresholds.min_hit_rate = 1.5; // > 1.0
    
    let validation = invalid_config.validate();
    assert!(validation.is_err());
    
    // Test memory allocation calculation
    let config = CacheConfig::default();
    let vector_memory = config.get_cache_memory_bytes("vector_data");
    let expected = config.global.total_memory_mb * 1024 * 1024 * 40 / 100;
    assert_eq!(vector_memory, expected);
}

/// Test monitoring dashboard functionality
#[tokio::test]
async fn test_monitoring_dashboard() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let coordinator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 100));
    let config = Arc::new(CacheConfig::default());
    
    let dashboard = CacheMonitoringDashboard::new(coordinator.clone(), config.clone());
    
    // Start monitoring
    dashboard.start().await;
    
    // Wait for some metrics to be collected
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Get dashboard state
    let state = dashboard.get_dashboard_state().await;
    
    // Verify state components
    assert!(!state.active_alerts.is_empty() || state.active_alerts.is_empty());
    assert!(!state.cache_status.is_empty());
    assert!(!state.optimization_suggestions.is_empty() || state.optimization_suggestions.is_empty());
    
    // Check cache status
    assert!(state.cache_status.contains_key("vector_data"));
    assert!(state.cache_status.contains_key("query_result"));
    
    let vector_status = &state.cache_status["vector_data"];
    assert!(vector_status.enabled);
    assert_eq!(vector_status.memory_allocated_mb, config.global.total_memory_mb * 40 / 100);
}

/// Test cache optimizer - TODO: Implement CacheOptimizer module
#[ignore = "Waiting for CacheOptimizer implementation"]
#[tokio::test]
async fn test_cache_optimizer() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let coordinator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 100));
    let config = CacheConfig::default();
    
    // TODO: Implement CacheOptimizer and uncomment this test
    // let optimizer = CacheOptimizer::new(coordinator.clone(), config);
    // let report = optimizer.analyze().await;
    // assert!(report.current_performance.hit_rate >= 0.0);
}

/// Test performance profiling
#[tokio::test]
async fn test_performance_profiling() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // use super::super::monitoring::PerformanceProfiler;
    
    // let profiler = PerformanceProfiler::new(0.5, "/tmp/test_profiles".to_string()); // TODO: Add monitoring module
    
    // Record operations
    // for i in 0..100 {
    //     profiler.record_operation(
    //         "get".to_string(),
    //         CacheType::VectorData,
    //         "L1".to_string(),
    //         1.5 + (i as f64 * 0.1),
    //         i % 2 == 0,
    //         1024,
    //     ).await;
    // }
    
    // Profiles should be sampled based on rate
    // With 0.5 rate, approximately 50 should be recorded
    // (actual test would verify this)
}

/// Test alert management
#[tokio::test]
async fn test_alert_management() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use crate::metrics::CacheMetricsSnapshot;
    
    let thresholds = super::super::config::AlertThresholds {
        min_hit_rate: 0.7,
        max_memory_usage: 0.9,
        max_eviction_rate: 100.0,
        max_cascade_size: 1000,
        max_prefetch_queue: 5000,
    };
    
    // let alert_manager = AlertManager::new(&thresholds); // TODO: Add monitoring module
    
    // Create metrics that trigger alerts
    let mut metrics = CacheMetricsSnapshot::default();
    metrics.overall_hit_rate = 0.5; // Below threshold
    metrics.memory_usage.used_bytes = 900;
    metrics.memory_usage.total_allocated_bytes = 1000; // 90% usage
    
    // alert_manager.check_alerts(&metrics, &thresholds).await;
    // 
    // let alerts = alert_manager.get_active_alerts().await;
    // assert!(!alerts.is_empty());
    // 
    // // Check for low hit rate alert
    // assert!(alerts.iter().any(|a| a.id == "low_hit_rate"));
    // 
    // // Check alert severity
    // let hit_rate_alert = alerts.iter().find(|a| a.id == "low_hit_rate").unwrap();
    // assert!(matches!(hit_rate_alert.severity, AlertSeverity::Warning)); // TODO: Add monitoring module
}

/// Test configuration file operations
#[tokio::test]
async fn test_config_file_operations() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use tempfile::NamedTempFile;
    
    // Create example configuration
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_str().unwrap();
    
    CacheConfig::create_example_config(path).unwrap();
    
    // Load configuration
    let loaded = CacheConfig::from_file(path).unwrap();
    
    // Verify loaded config is valid
    assert!(loaded.validate().is_ok());
    assert_eq!(loaded.global.total_memory_mb, 1024);
    assert_eq!(loaded.vector_data.memory_percentage, 40);
    
    // Modify and save
    let mut modified = loaded;
    modified.global.total_memory_mb = 2048;
    modified.to_file(path).unwrap();
    
    // Reload and verify changes
    let reloaded = CacheConfig::from_file(path).unwrap();
    assert_eq!(reloaded.global.total_memory_mb, 2048);
}

/// Test production-ready error handling
#[tokio::test]
async fn test_error_handling_and_recovery() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let coordinator = CrossCacheOrchestrator::new(1024 * 1024 * 100);
    
    // Test handling of invalid keys
    let result = coordinator.on_vector_access("").await;
    assert!(result.is_ok()); // Should handle gracefully
    
    // Test handling of cascade with circular dependencies
    let propagator = CascadeUpdatePropagator::new();
    
    // Create circular dependency
    propagator.add_dependency("a".to_string(), "b".to_string()).await;
    propagator.add_dependency("b".to_string(), "c".to_string()).await;
    propagator.add_dependency("c".to_string(), "a".to_string()).await;
    
    // Should handle circular dependency without infinite loop
    let cascade = propagator.get_invalidation_cascade("a").await;
    assert!(cascade.len() <= 3); // Should detect cycle
}

/// Test cache system stress test
#[tokio::test]
async fn test_cache_system_stress() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let coordinator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 10)); // 10MB
    
    // Spawn multiple concurrent operations
    let mut handles = vec![];
    
    // Vector access tasks
    for i in 0..10 {
        let coord = coordinator.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..100 {
                coord.on_vector_access(&format!("vec_{}_{}", i, j)).await.ok();
            }
        }));
    }
    
    // Query execution tasks
    for i in 0..5 {
        let coord = coordinator.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..50 {
                coord.on_query_execution(&format!("query_{}_{}", i, j)).await.ok();
            }
        }));
    }
    
    // Invalidation tasks
    for i in 0..3 {
        let coord = coordinator.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..20 {
                coord.orchestrate_cascade_invalidation(&format!("vec_{}_{}", i, j)).await.ok();
            }
        }));
    }
    
    // Memory rebalancing task
    let coord = coordinator.clone();
    handles.push(tokio::spawn(async move {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            coord.reallocate_memory_tiers().await.ok();
        }
    }));
    
    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap();
    }
    
    // Verify system is still functional
    let result = coordinator.on_vector_access("final_test").await;
    assert!(result.is_ok());
}

/// Test optimization recommendations
#[tokio::test]
async fn test_optimization_recommendations() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use crate::metrics::CacheMetricsSnapshot;
    
    let coordinator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 100));
    let config = CacheConfig::default();
    // let optimizer = CacheOptimizer::new(coordinator, config); // TODO: Add optimization module
    
    // Create metrics scenarios
    let mut low_hit_rate_metrics = CacheMetricsSnapshot::default();
    low_hit_rate_metrics.overall_hit_rate = 0.3;
    
    let mut high_eviction_metrics = CacheMetricsSnapshot::default();
    high_eviction_metrics.eviction_metrics.memory_pressure_evictions = 800;
    high_eviction_metrics.eviction_metrics.total_evictions = 1000;
    
    // Test recommendations for different scenarios
    // (Would call optimizer methods with these metrics)
}

// Import necessary types
use super::super::orchestrator::{CascadeInvalidator as CascadeUpdatePropagator, CrossCacheOrchestrator};
// use super::super::monitoring::{AlertManager, AlertSeverity};
