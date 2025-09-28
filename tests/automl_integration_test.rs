// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration tests for AutoML Framework

use proximadb::automl::{
    AutoMLCoordinator, AutoMLConfig, AutoMLService,
    WorkloadAnalyzer, WorkloadPattern,
    OptimizationPipeline, OptimizationGoal,
    HyperparameterTuner, TuningConfig,
};
use proximadb::automl::prediction::{
    PerformancePredictor, FeatureVector, TrainingSample, TargetMetric,
};
use proximadb::automl::optimization::{
    OptimizationStrategy,
};
use proximadb::automl::tuning::{
    TuningAlgorithm, ProximaDBHyperparameters, ParameterValue,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_automl_coordinator_lifecycle() {
    let config = AutoMLConfig {
        enabled: true,
        min_data_points: 10, // Low for testing
        optimization_interval_secs: 1, // Fast for testing
        min_improvement_threshold: 1.0,
        max_concurrent_optimizations: 2,
        enable_workload_prediction: true,
        enable_hyperparameter_tuning: true,
        enable_auto_indexing: true,
        enable_quantization_optimization: true,
    };

    let coordinator = AutoMLCoordinator::new(config).await.unwrap();

    // Start the coordinator
    coordinator.start().await.unwrap();

    // Let it run for a bit
    sleep(Duration::from_millis(100)).await;

    // Check status
    let status = coordinator.get_status().await;
    assert!(status.enabled);

    // Stop the coordinator
    coordinator.stop().await.unwrap();

    // Verify stopped
    let status = coordinator.get_status().await;
    assert!(!status.enabled);
}

#[tokio::test]
async fn test_workload_pattern_detection() {
    let analyzer = WorkloadAnalyzer::new().await.unwrap();

    // Simulate read-heavy workload
    for i in 0..20 {
        analyzer.record_metric("test_collection", "reads_per_sec", 1000.0 + i as f64).await.unwrap();
        analyzer.record_metric("test_collection", "writes_per_sec", 10.0).await.unwrap();
        analyzer.record_metric("test_collection", "query_latency_ms", 5.0).await.unwrap();
    }

    let pattern = analyzer.analyze_workload("test_collection").await.unwrap();
    assert_eq!(pattern, WorkloadPattern::ReadHeavy);

    // Test statistics
    let stats = analyzer.get_statistics("test_collection").await.unwrap();
    assert!(stats.avg_reads_per_sec > 1000.0);
    assert!(stats.read_write_ratio > 100.0);
}

#[tokio::test]
async fn test_workload_pattern_transitions() {
    let analyzer = WorkloadAnalyzer::new().await.unwrap();

    // Start with write-heavy pattern
    for _ in 0..15 {
        analyzer.record_metric("test_collection", "reads_per_sec", 10.0).await.unwrap();
        analyzer.record_metric("test_collection", "writes_per_sec", 1000.0).await.unwrap();
        analyzer.record_metric("test_collection", "query_latency_ms", 20.0).await.unwrap();
    }

    let pattern = analyzer.analyze_workload("test_collection").await.unwrap();
    assert_eq!(pattern, WorkloadPattern::WriteHeavy);

    // Transition to balanced
    for _ in 0..15 {
        analyzer.record_metric("test_collection", "reads_per_sec", 500.0).await.unwrap();
        analyzer.record_metric("test_collection", "writes_per_sec", 500.0).await.unwrap();
        analyzer.record_metric("test_collection", "query_latency_ms", 10.0).await.unwrap();
    }

    let pattern = analyzer.analyze_workload("test_collection").await.unwrap();
    assert_eq!(pattern, WorkloadPattern::Balanced);
}

#[tokio::test]
async fn test_performance_prediction() {
    let predictor = PerformancePredictor::new().await.unwrap();

    // Create training samples
    for i in 0..100 {
        let features = FeatureVector::from_characteristics(
            1000 * (i + 1),  // vector_count
            128,             // dimension
            0.1,             // sparsity
            2.0 + (i as f64 * 0.1), // read_write_ratio
        );

        let sample = TrainingSample {
            features,
            target: TargetMetric::QueryLatency(50.0 + i as f64),
            timestamp: chrono::Utc::now(),
        };

        predictor.add_training_sample(sample).await.unwrap();
    }

    // Train model
    predictor.train_model("test_collection", "linear").await.unwrap();

    // Make prediction
    let test_features = FeatureVector::from_characteristics(5000, 128, 0.1, 3.0);
    let prediction = predictor.predict("test_collection", &test_features).await.unwrap();

    assert!(prediction.value > 0.0);
    assert_eq!(prediction.model_type, "LinearRegression");
}

#[tokio::test]
async fn test_optimization_pipeline_grid_search() {
    let config = AutoMLConfig::default();
    let pipeline = OptimizationPipeline::new(config).await.unwrap();

    pipeline.start().await.unwrap();

    // Simple objective function for testing
    let objective = |config: HashMap<String, f64>| -> f64 {
        // Prefer lower values of param1
        config.get("param1").unwrap_or(&100.0) * -1.0
    };

    let best_config = pipeline.optimize(
        "test_collection",
        OptimizationGoal::MinimizeLatency,
        OptimizationStrategy::GridSearch,
        WorkloadPattern::ReadHeavy,
    ).await.unwrap();

    assert_eq!(best_config.engine.engine_type, "SST"); // Default for read-heavy

    pipeline.stop().await.unwrap();
}

#[tokio::test]
async fn test_optimization_pipeline_random_search() {
    let config = AutoMLConfig::default();
    let pipeline = OptimizationPipeline::new(config).await.unwrap();

    pipeline.start().await.unwrap();

    let best_config = pipeline.optimize(
        "test_collection",
        OptimizationGoal::MaximizeThroughput,
        OptimizationStrategy::RandomSearch { budget: 10 },
        WorkloadPattern::WriteHeavy,
    ).await.unwrap();

    // Should have selected configuration for write-heavy workload
    assert!(best_config.index.algorithm == "LSH" || best_config.index.algorithm == "HNSW");

    pipeline.stop().await.unwrap();
}

#[tokio::test]
async fn test_hyperparameter_tuning_tpe() {
    let tuner = HyperparameterTuner::new(TuningConfig {
        max_trials: 10,
        timeout_per_trial: 1,
        early_stopping_patience: 5,
        min_improvement: 0.01,
        parallel_trials: false,
        max_parallel_trials: 1,
    }).await.unwrap();

    // Add HNSW parameters
    for param in ProximaDBHyperparameters::hnsw_params() {
        tuner.add_parameter(param).await.unwrap();
    }

    // Simple objective - prefer lower M values
    let objective = |params: HashMap<String, ParameterValue>| async move {
        let m_value = match params.get("M") {
            Some(ParameterValue::Integer(v)) => *v as f64,
            _ => 32.0,
        };
        Ok(100.0 - m_value) // Higher score for lower M
    };

    let best_params = tuner.with_algorithm(TuningAlgorithm::TPE)
        .tune(objective)
        .await
        .unwrap();

    assert!(best_params.contains_key("M"));
    assert!(best_params.contains_key("ef_construction"));
}

#[tokio::test]
async fn test_hyperparameter_tuning_grid() {
    let tuner = HyperparameterTuner::new(TuningConfig {
        max_trials: 100, // Grid search ignores this
        timeout_per_trial: 1,
        early_stopping_patience: 5,
        min_improvement: 0.01,
        parallel_trials: false,
        max_parallel_trials: 1,
    }).await.unwrap();

    // Add quantization parameters
    for param in ProximaDBHyperparameters::quantization_params() {
        tuner.add_parameter(param).await.unwrap();
    }

    // Objective - prefer PQ8
    let objective = |params: HashMap<String, ParameterValue>| async move {
        let quant_level = match params.get("quantization_level") {
            Some(ParameterValue::String(s)) => s.clone(),
            _ => "None".to_string(),
        };

        match quant_level.as_str() {
            "PQ8" => Ok(100.0),
            "PQ16" => Ok(80.0),
            "INT8" => Ok(60.0),
            _ => Ok(10.0),
        }
    };

    let best_params = tuner.with_algorithm(TuningAlgorithm::Grid)
        .tune(objective)
        .await
        .unwrap();

    // Should select PQ8 as it has highest score
    match best_params.get("quantization_level") {
        Some(ParameterValue::String(s)) => {
            assert_eq!(s, "PQ8");
        }
        _ => panic!("Expected quantization_level to be selected"),
    }
}

#[tokio::test]
async fn test_automl_service_optimization_cycle() {
    let config = AutoMLConfig {
        enabled: true,
        min_data_points: 10,
        optimization_interval_secs: 1,
        min_improvement_threshold: 1.0,
        max_concurrent_optimizations: 2,
        enable_workload_prediction: true,
        enable_hyperparameter_tuning: true,
        enable_auto_indexing: true,
        enable_quantization_optimization: true,
    };

    let service = AutoMLService::new(config).await.unwrap();

    // Start service
    service.start().await.unwrap();

    // Let it run one optimization cycle
    sleep(Duration::from_secs(2)).await;

    // Stop service
    service.stop().await.unwrap();
}

#[tokio::test]
async fn test_optimization_goals() {
    let config = AutoMLConfig::default();
    let pipeline = OptimizationPipeline::new(config).await.unwrap();

    pipeline.start().await.unwrap();

    // Test different optimization goals
    let goals = vec![
        OptimizationGoal::MinimizeLatency,
        OptimizationGoal::MaximizeThroughput,
        OptimizationGoal::MinimizeMemory,
        OptimizationGoal::MaximizeAccuracy,
        OptimizationGoal::Balanced,
    ];

    for goal in goals {
        let config = pipeline.optimize(
            "test_collection",
            goal.clone(),
            OptimizationStrategy::RandomSearch { budget: 5 },
            WorkloadPattern::Mixed,
        ).await.unwrap();

        // Verify we got a configuration
        assert!(!config.index.algorithm.is_empty());
        assert!(!config.engine.engine_type.is_empty());
    }

    pipeline.stop().await.unwrap();
}

#[tokio::test]
async fn test_workload_prediction() {
    let analyzer = WorkloadAnalyzer::new().await.unwrap();

    // Create a trend - increasing reads
    for i in 0..20 {
        let reads = 100.0 + (i as f64 * 10.0);
        analyzer.record_metric("test_collection", "reads_per_sec", reads).await.unwrap();
        analyzer.record_metric("test_collection", "writes_per_sec", 50.0).await.unwrap();
    }

    // Current pattern
    let current = analyzer.analyze_workload("test_collection").await.unwrap();

    // Predict future (1 hour)
    let future = analyzer.predict_pattern("test_collection", 3600).await.unwrap();

    // With increasing reads, should predict read-heavy pattern
    assert_eq!(future, WorkloadPattern::ReadHeavy);
}

#[tokio::test]
async fn test_optimization_history() {
    let config = AutoMLConfig::default();
    let pipeline = OptimizationPipeline::new(config).await.unwrap();

    pipeline.start().await.unwrap();

    // Run multiple optimizations
    for i in 0..3 {
        let _ = pipeline.optimize(
            &format!("collection_{}", i),
            OptimizationGoal::Balanced,
            OptimizationStrategy::RandomSearch { budget: 3 },
            WorkloadPattern::Mixed,
        ).await.unwrap();
    }

    // Check history
    let history = pipeline.get_history().await;
    assert_eq!(history.len(), 3);

    for run in history {
        assert!(run.improvement >= 0.0);
        assert!(run.duration_ms > 0);
    }

    pipeline.stop().await.unwrap();
}

#[tokio::test]
async fn test_concurrent_optimizations() {
    let config = AutoMLConfig {
        enabled: true,
        min_data_points: 10,
        optimization_interval_secs: 300,
        min_improvement_threshold: 1.0,
        max_concurrent_optimizations: 3,
        enable_workload_prediction: true,
        enable_hyperparameter_tuning: true,
        enable_auto_indexing: true,
        enable_quantization_optimization: true,
    };

    let pipeline = Arc::new(OptimizationPipeline::new(config).await.unwrap());
    pipeline.start().await.unwrap();

    // Launch concurrent optimizations
    let mut handles = vec![];

    for i in 0..3 {
        let pipeline_clone = pipeline.clone();
        let handle = tokio::spawn(async move {
            pipeline_clone.optimize(
                &format!("collection_{}", i),
                OptimizationGoal::MinimizeLatency,
                OptimizationStrategy::RandomSearch { budget: 5 },
                WorkloadPattern::Mixed,
            ).await
        });
        handles.push(handle);
    }

    // Wait for all to complete
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    pipeline.stop().await.unwrap();
}

#[cfg(test)]
mod test_utils {
    use super::*;

    /// Helper to create sample workload data
    pub async fn create_sample_workload(
        analyzer: &WorkloadAnalyzer,
        collection_id: &str,
        pattern: WorkloadPattern,
        points: usize,
    ) {
        let (reads, writes) = match pattern {
            WorkloadPattern::ReadHeavy => (1000.0, 50.0),
            WorkloadPattern::WriteHeavy => (50.0, 1000.0),
            WorkloadPattern::Balanced => (500.0, 500.0),
            _ => (100.0, 100.0),
        };

        for _ in 0..points {
            analyzer.record_metric(collection_id, "reads_per_sec", reads).await.unwrap();
            analyzer.record_metric(collection_id, "writes_per_sec", writes).await.unwrap();
            analyzer.record_metric(collection_id, "query_latency_ms", 10.0).await.unwrap();
        }
    }
}