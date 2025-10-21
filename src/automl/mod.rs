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
    pub enabled: bool,
    pub active_optimizations: usize,
    pub total_optimizations: u64,
    pub average_improvement: f64,
    pub last_optimization: Option<chrono::DateTime<chrono::Utc>>,
}

/// AutoML Framework metrics
#[derive(Debug, Clone)]
pub struct AutoMLMetrics {
    pub predictions_made: u64,
    pub predictions_accurate: u64,
    pub optimizations_successful: u64,
    pub optimizations_failed: u64,
    pub average_optimization_time_ms: f64,
    pub total_runtime_saved_ms: u64,
}

/// Main AutoML coordinator
pub struct AutoMLCoordinator {
    service: Arc<AutoMLService>,
    workload_analyzer: Arc<WorkloadAnalyzer>,
    performance_predictor: Arc<PerformancePredictor>,
    optimization_pipeline: Arc<OptimizationPipeline>,
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
