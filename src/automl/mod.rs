// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AutoML Framework for ProximaDB
//!
//! This module provides automated machine learning capabilities for:
//! - Workload pattern detection and prediction
//! - Automatic hyperparameter tuning
//! - Performance optimization pipelines
//! - Self-tuning database configurations

pub mod optimization;
pub mod prediction;
pub mod service;
pub mod tuning;
pub mod workload;

pub use optimization::{OptimizationGoal, OptimizationPipeline};
pub use prediction::{PerformancePredictor, PredictionModel};
pub use service::{AutoMLConfig, AutoMLService};
pub use tuning::{HyperparameterTuner, TuningConfig};
pub use workload::{WorkloadAnalyzer, WorkloadPattern};

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

/// AutoML Framework status
#[derive(Debug, Clone)]
pub struct AutoMLStatus {
    /// Whether the AutoML framework is currently active
    pub enabled: bool,
    /// Number of optimization tasks currently in progress
    pub active_optimizations: usize,
    /// Cumulative count of all optimization runs since startup
    pub total_optimizations: u64,
    /// Average performance improvement (%) across all completed optimizations
    pub average_improvement: f64,
    /// Timestamp of the most recent completed optimization, or `None` if none have run
    pub last_optimization: Option<chrono::DateTime<chrono::Utc>>,
}

/// AutoML Framework metrics
#[derive(Debug, Clone)]
pub struct AutoMLMetrics {
    /// Total number of performance predictions issued
    pub predictions_made: u64,
    /// Number of predictions that fell within the acceptable error margin
    pub predictions_accurate: u64,
    /// Count of optimization runs that produced a measurable improvement
    pub optimizations_successful: u64,
    /// Count of optimization runs that ended in error or produced no improvement
    pub optimizations_failed: u64,
    /// Mean wall-clock time (milliseconds) spent per optimization run
    pub average_optimization_time_ms: f64,
    /// Cumulative query runtime saved (milliseconds) across all successful optimizations
    pub total_runtime_saved_ms: u64,
}

/// Main AutoML coordinator
pub struct AutoMLCoordinator {
    service: Arc<AutoMLService>,
    workload_analyzer: Arc<WorkloadAnalyzer>,
    #[allow(dead_code)]
    performance_predictor: Arc<PerformancePredictor>,
    optimization_pipeline: Arc<OptimizationPipeline>,
    #[allow(dead_code)]
    hyperparameter_tuner: Arc<HyperparameterTuner>,
    status: Arc<RwLock<AutoMLStatus>>,
    metrics: Arc<RwLock<AutoMLMetrics>>,
}

impl AutoMLCoordinator {
    /// Create a new AutoML coordinator
    pub async fn new(config: AutoMLConfig) -> Result<Self> {
        let service = Arc::new(AutoMLService::new(config.clone()).await?);
        let workload_analyzer = Arc::new(WorkloadAnalyzer::new().await?);
        let performance_predictor = Arc::new(PerformancePredictor::new().await?);
        let optimization_pipeline = Arc::new(OptimizationPipeline::new(config.clone()).await?);
        let hyperparameter_tuner =
            Arc::new(HyperparameterTuner::new(TuningConfig::default()).await?);

        let status = Arc::new(RwLock::new(AutoMLStatus {
            enabled: true,
            active_optimizations: 0,
            total_optimizations: 0,
            average_improvement: 0.0,
            last_optimization: None,
        }));

        let metrics = Arc::new(RwLock::new(AutoMLMetrics {
            predictions_made: 0,
            predictions_accurate: 0,
            optimizations_successful: 0,
            optimizations_failed: 0,
            average_optimization_time_ms: 0.0,
            total_runtime_saved_ms: 0,
        }));

        Ok(Self {
            service,
            workload_analyzer,
            performance_predictor,
            optimization_pipeline,
            hyperparameter_tuner,
            status,
            metrics,
        })
    }

    /// Start the AutoML coordinator
    pub async fn start(&self) -> Result<()> {
        tracing::info!("Starting AutoML Coordinator");

        // Start background services
        self.service.start().await?;
        self.workload_analyzer.start_monitoring().await?;
        self.optimization_pipeline.start().await?;

        Ok(())
    }

    /// Stop the AutoML coordinator
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping AutoML Coordinator");

        let mut status = self.status.write().await;
        status.enabled = false;

        self.service.stop().await?;
        self.workload_analyzer.stop_monitoring().await?;
        self.optimization_pipeline.stop().await?;

        Ok(())
    }

    /// Get current status
    pub async fn get_status(&self) -> AutoMLStatus {
        self.status.read().await.clone()
    }

    /// Get metrics
    pub async fn get_metrics(&self) -> AutoMLMetrics {
        self.metrics.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_automl_config_defaults() {
        let config = AutoMLConfig::default();

        assert!(config.enabled);
        assert_eq!(config.min_data_points, 1000);
        assert_eq!(config.optimization_interval_secs, 300);
        assert!((config.min_improvement_threshold - 5.0).abs() < f64::EPSILON);
        assert_eq!(config.max_concurrent_optimizations, 4);
        assert!(config.enable_workload_prediction);
        assert!(config.enable_hyperparameter_tuning);
        assert!(config.enable_auto_indexing);
        assert!(config.enable_quantization_optimization);
    }

    #[test]
    fn test_optimization_strategy_variants() {
        // Verify all OptimizationStrategy variants can be constructed
        let grid = optimization::OptimizationStrategy::GridSearch;
        let random = optimization::OptimizationStrategy::RandomSearch { budget: 50 };
        let bayesian =
            optimization::OptimizationStrategy::BayesianOptimization { n_iterations: 20 };
        let genetic = optimization::OptimizationStrategy::GeneticAlgorithm {
            population_size: 100,
            generations: 50,
        };

        // Verify parameters are stored correctly
        assert!(matches!(
            grid,
            optimization::OptimizationStrategy::GridSearch
        ));

        match random {
            optimization::OptimizationStrategy::RandomSearch { budget } => {
                assert_eq!(budget, 50);
            }
            _ => panic!("Expected RandomSearch"),
        }

        match bayesian {
            optimization::OptimizationStrategy::BayesianOptimization { n_iterations } => {
                assert_eq!(n_iterations, 20);
            }
            _ => panic!("Expected BayesianOptimization"),
        }

        match genetic {
            optimization::OptimizationStrategy::GeneticAlgorithm {
                population_size,
                generations,
            } => {
                assert_eq!(population_size, 100);
                assert_eq!(generations, 50);
            }
            _ => panic!("Expected GeneticAlgorithm"),
        }

        // Verify all OptimizationGoal variants
        let goals = vec![
            OptimizationGoal::MinimizeLatency,
            OptimizationGoal::MaximizeThroughput,
            OptimizationGoal::MinimizeMemory,
            OptimizationGoal::MaximizeAccuracy,
            OptimizationGoal::Balanced,
            OptimizationGoal::Custom(optimization::ObjectiveWeights::default()),
        ];
        assert_eq!(goals.len(), 6, "Expected 6 optimization goal variants");

        // Verify default ObjectiveWeights
        let weights = optimization::ObjectiveWeights::default();
        assert!((weights.latency - 0.25).abs() < f64::EPSILON);
        assert!((weights.throughput - 0.25).abs() < f64::EPSILON);
        assert!((weights.memory - 0.25).abs() < f64::EPSILON);
        assert!((weights.accuracy - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_workload_prediction_config() {
        // Verify WorkloadPattern variants
        let patterns = vec![
            WorkloadPattern::ReadHeavy,
            WorkloadPattern::WriteHeavy,
            WorkloadPattern::Balanced,
            WorkloadPattern::BatchProcessing,
            WorkloadPattern::Streaming,
            WorkloadPattern::Analytics,
            WorkloadPattern::Mixed,
        ];
        assert_eq!(patterns.len(), 7, "Expected 7 workload pattern variants");

        // Verify PartialEq is implemented
        assert_eq!(WorkloadPattern::ReadHeavy, WorkloadPattern::ReadHeavy);
        assert_ne!(WorkloadPattern::ReadHeavy, WorkloadPattern::WriteHeavy);

        // Verify PredictionModel enum has the expected variant names (cannot construct
        // directly since model structs have private fields, so we verify via TargetMetric)
        let target_variants = vec![
            prediction::TargetMetric::QueryLatency(10.0),
            prediction::TargetMetric::Throughput(1000.0),
            prediction::TargetMetric::MemoryUsage(512.0),
            prediction::TargetMetric::IndexBuildTime(30.0),
        ];
        assert_eq!(
            target_variants.len(),
            4,
            "Expected 4 target metric variants"
        );

        // Verify FeatureVector construction from characteristics
        let features = prediction::FeatureVector::from_characteristics(
            10000, // vector_count
            128,   // dimension
            0.1,   // sparsity
            5.0,   // read_write_ratio
        );
        assert!((features.vector_count - 10000.0).abs() < f64::EPSILON);
        assert!((features.vector_dimension - 128.0).abs() < f64::EPSILON);
        assert!((features.sparsity - 0.1).abs() < 1e-6);
        // read_ratio = 5.0 / (1.0 + 5.0) = 5/6
        assert!((features.read_ratio - 5.0 / 6.0).abs() < 1e-6);
        // write_ratio = 1.0 / (1.0 + 5.0) = 1/6
        assert!((features.write_ratio - 1.0 / 6.0).abs() < 1e-6);

        // Verify to_array produces correct length
        let array = features.to_array();
        assert_eq!(array.len(), 13, "FeatureVector should produce 13 features");
    }

    #[test]
    fn test_tuning_parameter_bounds() {
        // Verify TuningConfig defaults
        let config = tuning::TuningConfig::default();
        assert_eq!(config.max_trials, 100);
        assert_eq!(config.timeout_per_trial, 60);
        assert_eq!(config.early_stopping_patience, 10);
        assert!((config.min_improvement - 0.01).abs() < f64::EPSILON);
        assert!(config.parallel_trials);
        assert_eq!(config.max_parallel_trials, 4);

        // Verify SearchSpace variants for parameter bounds
        let continuous = tuning::SearchSpace::Continuous { min: 0.0, max: 1.0 };
        match &continuous {
            tuning::SearchSpace::Continuous { min, max } => {
                assert!(*min < *max, "min should be less than max");
                assert!((*min - 0.0).abs() < f64::EPSILON);
                assert!((*max - 1.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Continuous"),
        }

        let discrete = tuning::SearchSpace::Discrete {
            min: 1,
            max: 100,
            step: 5,
        };
        match &discrete {
            tuning::SearchSpace::Discrete { min, max, step } => {
                assert!(*min < *max, "min should be less than max");
                assert_eq!(*min, 1);
                assert_eq!(*max, 100);
                assert_eq!(*step, 5);
            }
            _ => panic!("Expected Discrete"),
        }

        let categorical = tuning::SearchSpace::Categorical {
            choices: vec!["HNSW".to_string(), "IVF".to_string(), "LSH".to_string()],
        };
        match &categorical {
            tuning::SearchSpace::Categorical { choices } => {
                assert_eq!(choices.len(), 3);
                assert!(choices.contains(&"HNSW".to_string()));
            }
            _ => panic!("Expected Categorical"),
        }

        let logscale = tuning::SearchSpace::LogScale {
            min: 0.001,
            max: 1.0,
        };
        match &logscale {
            tuning::SearchSpace::LogScale { min, max } => {
                assert!(*min > 0.0, "LogScale min must be positive");
                assert!(*min < *max, "min should be less than max");
            }
            _ => panic!("Expected LogScale"),
        }

        // Verify HyperParameter construction
        let param = tuning::HyperParameter {
            name: "learning_rate".to_string(),
            param_type: tuning::ParameterType::Float,
            search_space: tuning::SearchSpace::LogScale {
                min: 1e-4,
                max: 0.1,
            },
        };
        assert_eq!(param.name, "learning_rate");
        assert!(matches!(param.param_type, tuning::ParameterType::Float));

        // Verify ParameterType variants
        let _integer = tuning::ParameterType::Integer;
        let _float = tuning::ParameterType::Float;
        let _categorical = tuning::ParameterType::Categorical;
        let _boolean = tuning::ParameterType::Boolean;

        // Verify ParameterValue variants
        let int_val = tuning::ParameterValue::Integer(42);
        let float_val = tuning::ParameterValue::Float(0.001);
        let str_val = tuning::ParameterValue::String("HNSW".to_string());
        let bool_val = tuning::ParameterValue::Boolean(true);

        match int_val {
            tuning::ParameterValue::Integer(v) => assert_eq!(v, 42),
            _ => panic!("Expected Integer"),
        }
        match float_val {
            tuning::ParameterValue::Float(v) => assert!((v - 0.001).abs() < f64::EPSILON),
            _ => panic!("Expected Float"),
        }
        match str_val {
            tuning::ParameterValue::String(v) => assert_eq!(v, "HNSW"),
            _ => panic!("Expected String"),
        }
        match bool_val {
            tuning::ParameterValue::Boolean(v) => assert!(v),
            _ => panic!("Expected Boolean"),
        }
    }
}
