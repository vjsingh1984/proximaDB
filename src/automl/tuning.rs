// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Hyperparameter Tuning Module

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Hyperparameter tuning configuration
#[derive(Debug, Clone)]
pub struct TuningConfig {
    /// Maximum number of trials
    pub max_trials: usize,
    /// Timeout per trial in seconds
    pub timeout_per_trial: u64,
    /// Early stopping patience
    pub early_stopping_patience: usize,
    /// Minimum improvement threshold
    pub min_improvement: f64,
    /// Enable parallel trials
    pub parallel_trials: bool,
    /// Maximum parallel trials
    pub max_parallel_trials: usize,
}

impl Default for TuningConfig {
    fn default() -> Self {
        Self {
            max_trials: 100,
            timeout_per_trial: 60,
            early_stopping_patience: 10,
            min_improvement: 0.01, // 1% improvement
            parallel_trials: true,
            max_parallel_trials: 4,
        }
    }
}

/// Hyperparameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperParameter {
    pub name: String,
    pub param_type: ParameterType,
    pub search_space: SearchSpace,
}

/// Parameter types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterType {
    Integer,
    Float,
    Categorical,
    Boolean,
}

/// Search space definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchSpace {
    /// Continuous range [min, max]
    Continuous { min: f64, max: f64 },
    /// Discrete range [min, max, step]
    Discrete { min: i64, max: i64, step: i64 },
    /// Categorical choices
    Categorical { choices: Vec<String> },
    /// Logarithmic scale [min, max]
    LogScale { min: f64, max: f64 },
}

/// Trial result
#[derive(Debug, Clone)]
pub struct TrialResult {
    pub trial_id: String,
    pub parameters: HashMap<String, ParameterValue>,
    pub score: f64,
    pub duration_ms: u64,
    pub status: TrialStatus,
}

/// Parameter value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterValue {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
}

/// Trial status
#[derive(Debug, Clone, PartialEq)]
pub enum TrialStatus {
    Running,
    Completed,
    Failed,
    Timeout,
    EarlyStopped,
}

/// Tuning algorithm
#[derive(Debug, Clone)]
pub enum TuningAlgorithm {
    /// Tree-structured Parzen Estimator
    TPE,
    /// Random search
    Random,
    /// Grid search
    Grid,
    /// Hyperband
    Hyperband { max_iter: usize, eta: f64 },
    /// Optuna-style algorithm
    Optuna,
}

/// Hyperparameter tuner
pub struct HyperparameterTuner {
    config: TuningConfig,
    parameters: Arc<RwLock<Vec<HyperParameter>>>,
    trials: Arc<RwLock<Vec<TrialResult>>>,
    best_trial: Arc<RwLock<Option<TrialResult>>>,
    algorithm: TuningAlgorithm,
}

impl HyperparameterTuner {
    /// Create a new hyperparameter tuner
    pub async fn new(config: TuningConfig) -> Result<Self> {
        Ok(Self {
            config,
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::TPE,
        })
    }

    /// Add a hyperparameter to tune
    pub async fn add_parameter(&self, param: HyperParameter) -> Result<()> {
        let mut parameters = self.parameters.write().await;
        parameters.push(param);
        Ok(())
    }

    /// Set tuning algorithm
    pub fn with_algorithm(mut self, algorithm: TuningAlgorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// Run tuning process
    pub async fn tune<F, Fut>(
        &self,
        objective: F,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        info!("Starting hyperparameter tuning with {:?} algorithm", self.algorithm);

        let parameters = self.parameters.read().await.clone();

        match self.algorithm {
            TuningAlgorithm::TPE => self.tune_tpe(objective, parameters).await,
            TuningAlgorithm::Random => self.tune_random(objective, parameters).await,
            TuningAlgorithm::Grid => self.tune_grid(objective, parameters).await,
            TuningAlgorithm::Hyperband { max_iter, eta } => {
                self.tune_hyperband(objective, parameters, max_iter, eta).await
            }
            TuningAlgorithm::Optuna => self.tune_optuna(objective, parameters).await,
        }
    }

    /// TPE tuning algorithm
    async fn tune_tpe<F, Fut>(
        &self,
        objective: F,
        parameters: Vec<HyperParameter>,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        let mut best_params = HashMap::new();
        let mut best_score = f64::NEG_INFINITY;
        let mut no_improvement_count = 0;

        for trial_num in 0..self.config.max_trials {
            // Sample parameters using TPE
            let params = if trial_num < 10 {
                // Random exploration for first trials
                self.sample_random(&parameters)?
            } else {
                // Use TPE for subsequent trials
                self.sample_tpe(&parameters).await?
            };

            // Evaluate objective
            let start = std::time::Instant::now();
            let score = match objective(params.clone()).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("Trial {} failed: {}", trial_num, e);
                    continue;
                }
            };
            let duration = start.elapsed().as_millis() as u64;

            // Record trial
            let trial = TrialResult {
                trial_id: format!("trial_{}", trial_num),
                parameters: params.clone(),
                score,
                duration_ms: duration,
                status: TrialStatus::Completed,
            };

            let mut trials = self.trials.write().await;
            trials.push(trial.clone());

            // Update best
            if score > best_score {
                best_score = score;
                best_params = params;
                no_improvement_count = 0;

                let mut best = self.best_trial.write().await;
                *best = Some(trial);
            } else {
                no_improvement_count += 1;
            }

            // Early stopping
            if no_improvement_count >= self.config.early_stopping_patience {
                info!("Early stopping after {} trials", trial_num + 1);
                break;
            }
        }

        Ok(best_params)
    }

    /// Random search tuning
    async fn tune_random<F, Fut>(
        &self,
        objective: F,
        parameters: Vec<HyperParameter>,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        let mut best_params = HashMap::new();
        let mut best_score = f64::NEG_INFINITY;

        for trial_num in 0..self.config.max_trials {
            let params = self.sample_random(&parameters)?;

            let score = match objective(params.clone()).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("Trial {} failed: {}", trial_num, e);
                    continue;
                }
            };

            if score > best_score {
                best_score = score;
                best_params = params;
            }
        }

        Ok(best_params)
    }

    /// Grid search tuning
    async fn tune_grid<F, Fut>(
        &self,
        objective: F,
        parameters: Vec<HyperParameter>,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        let grid_points = self.generate_grid(&parameters)?;
        let mut best_params = HashMap::new();
        let mut best_score = f64::NEG_INFINITY;

        for params in grid_points {
            let score = match objective(params.clone()).await {
                Ok(s) => s,
                Err(_) => continue,
            };

            if score > best_score {
                best_score = score;
                best_params = params;
            }
        }

        Ok(best_params)
    }

    /// Hyperband tuning algorithm
    async fn tune_hyperband<F, Fut>(
        &self,
        objective: F,
        parameters: Vec<HyperParameter>,
        max_iter: usize,
        eta: f64,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        // Simplified Hyperband implementation
        let mut best_params = HashMap::new();
        let mut best_score = f64::NEG_INFINITY;

        let s_max = (max_iter as f64).log(eta).floor() as usize;

        for s in (0..=s_max).rev() {
            let n = ((eta.powi(s as i32) * max_iter as f64) / (s + 1) as f64).ceil() as usize;
            let r = max_iter as f64 * eta.powi(-(s as i32));

            // Generate initial configurations
            let mut configs: Vec<HashMap<String, ParameterValue>> = Vec::new();
            for _ in 0..n {
                configs.push(self.sample_random(&parameters)?);
            }

            // Successive halving
            for i in 0..=s {
                let n_i = (n as f64 * eta.powi(-(i as i32))).floor() as usize;
                let r_i = r * eta.powi(i as i32);

                // Evaluate configurations
                let mut scores = Vec::new();
                for config in &configs {
                    let score = objective(config.clone()).await.unwrap_or(f64::NEG_INFINITY);
                    scores.push((config.clone(), score));
                }

                // Sort and keep top configurations
                scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

                // Update best before consuming scores
                if let Some((config, score)) = scores.first() {
                    if *score > best_score {
                        best_score = *score;
                        best_params = config.clone();
                    }
                }

                let k = (n_i as f64 / eta).floor() as usize;
                configs = scores.into_iter()
                    .take(k)
                    .map(|(config, _)| config)
                    .collect();
            }
        }

        Ok(best_params)
    }

    /// Optuna-style tuning
    async fn tune_optuna<F, Fut>(
        &self,
        objective: F,
        parameters: Vec<HyperParameter>,
    ) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        // Similar to TPE but with pruning
        self.tune_tpe(objective, parameters).await
    }

    /// Sample random parameters
    fn sample_random(&self, parameters: &[HyperParameter]) -> Result<HashMap<String, ParameterValue>> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut params = HashMap::new();

        for param in parameters {
            let value = match &param.search_space {
                SearchSpace::Continuous { min, max } => {
                    ParameterValue::Float(rng.gen_range(*min..*max))
                }
                SearchSpace::Discrete { min, max, step } => {
                    let n_steps = ((max - min) / step) as usize;
                    let step_idx = rng.gen_range(0..=n_steps);
                    ParameterValue::Integer(min + (step_idx as i64) * step)
                }
                SearchSpace::Categorical { choices } => {
                    use rand::seq::SliceRandom;
                    ParameterValue::String(choices.choose(&mut rng).unwrap().clone())
                }
                SearchSpace::LogScale { min, max } => {
                    let log_min = min.ln();
                    let log_max = max.ln();
                    let log_value = rng.gen_range(log_min..log_max);
                    ParameterValue::Float(log_value.exp())
                }
            };

            params.insert(param.name.clone(), value);
        }

        Ok(params)
    }

    /// Sample using TPE
    async fn sample_tpe(&self, parameters: &[HyperParameter]) -> Result<HashMap<String, ParameterValue>> {
        let trials = self.trials.read().await;

        if trials.len() < 10 {
            return self.sample_random(parameters);
        }

        // Split trials into good and bad based on quantile
        let mut sorted_trials = trials.clone();
        sorted_trials.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        let quantile = (trials.len() as f64 * 0.25) as usize;
        let good_trials = &sorted_trials[..quantile];
        let bad_trials = &sorted_trials[quantile..];

        // Sample from good distribution
        let mut params = HashMap::new();

        for param in parameters {
            // Get values from good trials
            let good_values: Vec<_> = good_trials.iter()
                .filter_map(|t| t.parameters.get(&param.name))
                .collect();

            if good_values.is_empty() {
                // Sample randomly if no good values
                let random_params = self.sample_random(&[param.clone()])?;
                params.insert(param.name.clone(), random_params.into_values().next().unwrap());
            } else {
                // Sample from good distribution (simplified)
                use rand::seq::SliceRandom;
                let mut rng = rand::thread_rng();
                let value = good_values.choose(&mut rng).unwrap();
                params.insert(param.name.clone(), (*value).clone());
            }
        }

        Ok(params)
    }

    /// Generate grid points
    fn generate_grid(&self, parameters: &[HyperParameter]) -> Result<Vec<HashMap<String, ParameterValue>>> {
        let mut grid_points = vec![HashMap::new()];

        for param in parameters {
            let mut new_points = Vec::new();

            let values = match &param.search_space {
                SearchSpace::Continuous { min, max } => {
                    // Sample 5 points for continuous
                    (0..5).map(|i| {
                        let t = i as f64 / 4.0;
                        ParameterValue::Float(min + t * (max - min))
                    }).collect::<Vec<_>>()
                }
                SearchSpace::Discrete { min, max, step } => {
                    let mut values = Vec::new();
                    let mut v = *min;
                    while v <= *max {
                        values.push(ParameterValue::Integer(v));
                        v += step;
                    }
                    values
                }
                SearchSpace::Categorical { choices } => {
                    choices.iter()
                        .map(|c| ParameterValue::String(c.clone()))
                        .collect()
                }
                SearchSpace::LogScale { min, max } => {
                    // Sample 5 points in log scale
                    let log_min = min.ln();
                    let log_max = max.ln();
                    (0..5).map(|i| {
                        let t = i as f64 / 4.0;
                        let log_value = log_min + t * (log_max - log_min);
                        ParameterValue::Float(log_value.exp())
                    }).collect::<Vec<_>>()
                }
            };

            for point in &grid_points {
                for value in &values {
                    let mut new_point = point.clone();
                    new_point.insert(param.name.clone(), value.clone());
                    new_points.push(new_point);
                }
            }

            grid_points = new_points;
        }

        Ok(grid_points)
    }

    /// Get best trial
    pub async fn get_best_trial(&self) -> Option<TrialResult> {
        let best = self.best_trial.read().await;
        best.clone()
    }

    /// Get all trials
    pub async fn get_trials(&self) -> Vec<TrialResult> {
        let trials = self.trials.read().await;
        trials.clone()
    }
}

/// ProximaDB-specific hyperparameter sets
pub struct ProximaDBHyperparameters;

impl ProximaDBHyperparameters {
    /// HNSW index hyperparameters
    pub fn hnsw_params() -> Vec<HyperParameter> {
        vec![
            HyperParameter {
                name: "M".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 8, max: 64, step: 4 },
            },
            HyperParameter {
                name: "ef_construction".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 50, max: 500, step: 50 },
            },
            HyperParameter {
                name: "ef_search".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 10, max: 200, step: 10 },
            },
        ]
    }

    /// IVF index hyperparameters
    pub fn ivf_params() -> Vec<HyperParameter> {
        vec![
            HyperParameter {
                name: "nlist".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::LogScale { min: 10.0, max: 10000.0 },
            },
            HyperParameter {
                name: "nprobe".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 1, max: 100, step: 5 },
            },
        ]
    }

    /// Quantization hyperparameters
    pub fn quantization_params() -> Vec<HyperParameter> {
        vec![
            HyperParameter {
                name: "quantization_level".to_string(),
                param_type: ParameterType::Categorical,
                search_space: SearchSpace::Categorical {
                    choices: vec![
                        "None".to_string(),
                        "Binary".to_string(),
                        "INT8".to_string(),
                        "PQ4".to_string(),
                        "PQ8".to_string(),
                        "PQ16".to_string(),
                    ],
                },
            },
            HyperParameter {
                name: "codebook_size".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 128, max: 1024, step: 128 },
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_random_sampling() {
        let tuner = HyperparameterTuner::new(TuningConfig::default()).await.unwrap();

        let params = ProximaDBHyperparameters::hnsw_params();
        for param in params {
            tuner.add_parameter(param).await.unwrap();
        }

        // Test objective function
        let objective = |params: HashMap<String, ParameterValue>| async move {
            // Simple scoring function
            let m = match params.get("M") {
                Some(ParameterValue::Integer(v)) => *v as f64,
                _ => 16.0,
            };
            Ok(100.0 - m) // Prefer smaller M
        };

        let best = tuner.tune(objective).await.unwrap();
        assert!(best.contains_key("M"));
    }

    #[test]
    fn test_grid_generation() {
        let params = vec![
            HyperParameter {
                name: "param1".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete { min: 0, max: 2, step: 1 },
            },
            HyperParameter {
                name: "param2".to_string(),
                param_type: ParameterType::Categorical,
                search_space: SearchSpace::Categorical {
                    choices: vec!["a".to_string(), "b".to_string()],
                },
            },
        ];

        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(params.clone())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Grid,
        };

        let grid = tuner.generate_grid(&params).unwrap();
        assert_eq!(grid.len(), 6); // 3 values × 2 choices
    }
}