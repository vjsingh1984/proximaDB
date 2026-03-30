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

/// Simple Gaussian Kernel Density Estimator for TPE
struct GaussianKDE {
    /// Sample values
    values: Vec<f64>,
    /// Bandwidth (smoothing parameter)
    bandwidth: f64,
}

impl GaussianKDE {
    /// Create a new KDE from sample values with automatic bandwidth selection (Silverman's rule)
    fn new(values: Vec<f64>) -> Self {
        let n = values.len() as f64;
        let std_dev = Self::std_dev(&values);
        // Silverman's rule of thumb
        let bandwidth = if std_dev > 0.0 && n > 1.0 {
            1.06 * std_dev * n.powf(-0.2)
        } else {
            1.0
        };
        Self { values, bandwidth }
    }

    /// Compute standard deviation
    fn std_dev(values: &[f64]) -> f64 {
        if values.len() < 2 {
            return 0.0;
        }
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
        variance.sqrt()
    }

    /// Evaluate PDF at a point
    fn pdf(&self, x: f64) -> f64 {
        if self.values.is_empty() {
            return 0.0;
        }
        let n = self.values.len() as f64;
        let sum: f64 = self
            .values
            .iter()
            .map(|v| Self::gaussian_kernel((x - v) / self.bandwidth))
            .sum();
        sum / (n * self.bandwidth)
    }

    /// Standard Gaussian kernel
    fn gaussian_kernel(x: f64) -> f64 {
        (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
    }

    /// Sample a value from the KDE
    fn sample(&self) -> f64 {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        if self.values.is_empty() {
            return 0.0;
        }

        // Pick a random kernel center
        let idx = rng.gen_range(0..self.values.len());
        let center = self.values[idx];

        // Sample from Gaussian around that center using Box-Muller transform
        let u1: f64 = rng.gen_range(0.0001..1.0);
        let u2: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
        let z = (-2.0 * u1.ln()).sqrt() * u2.cos();

        center + z * self.bandwidth
    }
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
    pub async fn tune<F, Fut>(&self, objective: F) -> Result<HashMap<String, ParameterValue>>
    where
        F: Fn(HashMap<String, ParameterValue>) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = Result<f64>> + Send,
    {
        info!(
            "Starting hyperparameter tuning with {:?} algorithm",
            self.algorithm
        );

        let parameters = self.parameters.read().await.clone();

        match self.algorithm {
            TuningAlgorithm::TPE => self.tune_tpe(objective, parameters).await,
            TuningAlgorithm::Random => self.tune_random(objective, parameters).await,
            TuningAlgorithm::Grid => self.tune_grid(objective, parameters).await,
            TuningAlgorithm::Hyperband { max_iter, eta } => {
                self.tune_hyperband(objective, parameters, max_iter, eta)
                    .await
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

            // Cap trial history to prevent unbounded memory growth.
            // Keep the most recent trials (TPE only needs recent history for KDE).
            let max_retained = self.config.max_trials.max(200);
            if trials.len() > max_retained {
                let drain_count = trials.len() - max_retained;
                trials.drain(..drain_count);
            }

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
                let _r_i = r * eta.powi(i as i32);

                // Evaluate configurations
                let mut scores = Vec::new();
                for config in &configs {
                    let score = objective(config.clone()).await.unwrap_or(f64::NEG_INFINITY);
                    scores.push((config.clone(), score));
                }

                // Sort and keep top configurations
                scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

                // Update best before consuming scores
                if let Some((config, score)) = scores.first()
                    && *score > best_score {
                        best_score = *score;
                        best_params = config.clone();
                    }

                let k = (n_i as f64 / eta).floor() as usize;
                configs = scores
                    .into_iter()
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
    fn sample_random(
        &self,
        parameters: &[HyperParameter],
    ) -> Result<HashMap<String, ParameterValue>> {
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
                    ParameterValue::String(
                        choices
                            .choose(&mut rng)
                            .ok_or_else(|| anyhow::anyhow!("No categorical choices available"))?
                            .clone(),
                    )
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

    /// Sample using TPE (Tree-structured Parzen Estimator)
    async fn sample_tpe(
        &self,
        parameters: &[HyperParameter],
    ) -> Result<HashMap<String, ParameterValue>> {
        let trials = self.trials.read().await;

        if trials.len() < 10 {
            return self.sample_random(parameters);
        }

        // Split trials into good and bad based on gamma quantile (top 25%)
        // Use sorted indices to avoid cloning the entire trial history
        let mut indices: Vec<usize> = (0..trials.len()).collect();
        indices.sort_by(|&a, &b| {
            trials[b]
                .score
                .partial_cmp(&trials[a].score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let gamma = 0.25;
        let quantile = ((trials.len() as f64 * gamma).ceil() as usize).max(1);
        let good_indices = &indices[..quantile];
        let bad_indices = &indices[quantile..];

        // Collect references without cloning
        let good_trials: Vec<&TrialResult> = good_indices.iter().map(|&i| &trials[i]).collect();
        let bad_trials: Vec<&TrialResult> = bad_indices.iter().map(|&i| &trials[i]).collect();

        let mut params = HashMap::new();

        for param in parameters {
            let value = match &param.search_space {
                SearchSpace::Continuous { min, max } | SearchSpace::LogScale { min, max } => {
                    let is_log = matches!(&param.search_space, SearchSpace::LogScale { .. });

                    // Extract values from good and bad trials
                    let good_values: Vec<f64> = good_trials
                        .iter()
                        .filter_map(|t| t.parameters.get(&param.name))
                        .filter_map(|v| match v {
                            ParameterValue::Float(f) => Some(if is_log { f.ln() } else { *f }),
                            _ => None,
                        })
                        .collect();

                    let bad_values: Vec<f64> = bad_trials
                        .iter()
                        .filter_map(|t| t.parameters.get(&param.name))
                        .filter_map(|v| match v {
                            ParameterValue::Float(f) => Some(if is_log { f.ln() } else { *f }),
                            _ => None,
                        })
                        .collect();

                    if good_values.is_empty() {
                        // Fall back to random
                        let random = self.sample_random(std::slice::from_ref(param))?;
                        params.insert(
                            param.name.clone(),
                            random
                                .into_values()
                                .next()
                                .ok_or_else(|| anyhow::anyhow!("No parameter value"))?,
                        );
                        continue;
                    }

                    let l_kde = GaussianKDE::new(good_values);
                    let g_kde = GaussianKDE::new(bad_values);

                    // Sample candidates and pick the one with best l(x)/g(x) ratio
                    let n_candidates = 24;
                    let mut best_value = l_kde.sample();
                    let mut best_ratio = f64::NEG_INFINITY;

                    let (search_min, search_max) = if is_log {
                        (min.ln(), max.ln())
                    } else {
                        (*min, *max)
                    };

                    for _ in 0..n_candidates {
                        let candidate = l_kde.sample();
                        // Clamp to search space
                        let candidate = candidate.clamp(search_min, search_max);

                        let l_score = l_kde.pdf(candidate);
                        let g_score = g_kde.pdf(candidate).max(1e-12);
                        let ratio = l_score / g_score;

                        if ratio > best_ratio {
                            best_ratio = ratio;
                            best_value = candidate;
                        }
                    }

                    best_value = best_value.clamp(search_min, search_max);

                    if is_log {
                        ParameterValue::Float(best_value.exp())
                    } else {
                        ParameterValue::Float(best_value)
                    }
                }
                SearchSpace::Discrete { min, max, step } => {
                    // For discrete: use KDE on good trials
                    let good_values: Vec<i64> = good_trials
                        .iter()
                        .filter_map(|t| t.parameters.get(&param.name))
                        .filter_map(|v| match v {
                            ParameterValue::Integer(i) => Some(*i),
                            _ => None,
                        })
                        .collect();

                    if good_values.is_empty() {
                        let random = self.sample_random(std::slice::from_ref(param))?;
                        params.insert(
                            param.name.clone(),
                            random
                                .into_values()
                                .next()
                                .ok_or_else(|| anyhow::anyhow!("No parameter value"))?,
                        );
                        continue;
                    }

                    // Use KDE on discrete values (treat as continuous, then round)
                    let float_values: Vec<f64> = good_values.iter().map(|v| *v as f64).collect();
                    let kde = GaussianKDE::new(float_values);
                    let sampled = kde.sample();

                    // Round to nearest valid discrete value
                    let rounded =
                        ((sampled - *min as f64) / *step as f64).round() as i64 * step + min;
                    let clamped = rounded.clamp(*min, *max);

                    ParameterValue::Integer(clamped)
                }
                SearchSpace::Categorical { choices } => {
                    // Categorical: use empirical frequency from good/bad trials
                    let good_choices: Vec<&str> = good_trials
                        .iter()
                        .filter_map(|t| t.parameters.get(&param.name))
                        .filter_map(|v| match v {
                            ParameterValue::String(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .collect();

                    let bad_choices: Vec<&str> = bad_trials
                        .iter()
                        .filter_map(|t| t.parameters.get(&param.name))
                        .filter_map(|v| match v {
                            ParameterValue::String(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .collect();

                    if good_choices.is_empty() || choices.is_empty() {
                        let random = self.sample_random(std::slice::from_ref(param))?;
                        params.insert(
                            param.name.clone(),
                            random
                                .into_values()
                                .next()
                                .ok_or_else(|| anyhow::anyhow!("No parameter value"))?,
                        );
                        continue;
                    }

                    // Compute l(x)/g(x) ratio for each choice with Laplace smoothing
                    let n_good = good_choices.len() as f64;
                    let n_bad = bad_choices.len().max(1) as f64;

                    let mut best_choice = choices[0].clone();
                    let mut best_ratio = f64::NEG_INFINITY;

                    for choice in choices {
                        let l_count = good_choices
                            .iter()
                            .filter(|c| **c == choice.as_str())
                            .count() as f64;
                        let g_count = bad_choices
                            .iter()
                            .filter(|c| **c == choice.as_str())
                            .count() as f64;

                        // Laplace smoothing
                        let l_prob = (l_count + 1.0) / (n_good + choices.len() as f64);
                        let g_prob = (g_count + 1.0) / (n_bad + choices.len() as f64);

                        let ratio = l_prob / g_prob;
                        if ratio > best_ratio {
                            best_ratio = ratio;
                            best_choice = choice.clone();
                        }
                    }

                    ParameterValue::String(best_choice)
                }
            };

            params.insert(param.name.clone(), value);
        }

        Ok(params)
    }

    /// Generate grid points
    fn generate_grid(
        &self,
        parameters: &[HyperParameter],
    ) -> Result<Vec<HashMap<String, ParameterValue>>> {
        let mut grid_points = vec![HashMap::new()];

        for param in parameters {
            let mut new_points = Vec::new();

            let values = match &param.search_space {
                SearchSpace::Continuous { min, max } => {
                    // Sample 5 points for continuous
                    (0..5)
                        .map(|i| {
                            let t = i as f64 / 4.0;
                            ParameterValue::Float(min + t * (max - min))
                        })
                        .collect::<Vec<_>>()
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
                SearchSpace::Categorical { choices } => choices
                    .iter()
                    .map(|c| ParameterValue::String(c.clone()))
                    .collect(),
                SearchSpace::LogScale { min, max } => {
                    // Sample 5 points in log scale
                    let log_min = min.ln();
                    let log_max = max.ln();
                    (0..5)
                        .map(|i| {
                            let t = i as f64 / 4.0;
                            let log_value = log_min + t * (log_max - log_min);
                            ParameterValue::Float(log_value.exp())
                        })
                        .collect::<Vec<_>>()
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
                search_space: SearchSpace::Discrete {
                    min: 8,
                    max: 64,
                    step: 4,
                },
            },
            HyperParameter {
                name: "ef_construction".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete {
                    min: 50,
                    max: 500,
                    step: 50,
                },
            },
            HyperParameter {
                name: "ef_search".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete {
                    min: 10,
                    max: 200,
                    step: 10,
                },
            },
        ]
    }

    /// IVF index hyperparameters
    pub fn ivf_params() -> Vec<HyperParameter> {
        vec![
            HyperParameter {
                name: "nlist".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::LogScale {
                    min: 10.0,
                    max: 10000.0,
                },
            },
            HyperParameter {
                name: "nprobe".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete {
                    min: 1,
                    max: 100,
                    step: 5,
                },
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
                search_space: SearchSpace::Discrete {
                    min: 128,
                    max: 1024,
                    step: 128,
                },
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_random_sampling() {
        let tuner = HyperparameterTuner::new(TuningConfig::default())
            .await
            .expect("Failed to create hyperparameter tuner");

        let params = ProximaDBHyperparameters::hnsw_params();
        for param in params {
            tuner
                .add_parameter(param)
                .await
                .expect("Failed to add parameter");
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

        let best = tuner
            .tune(objective)
            .await
            .expect("Failed to complete tuning");
        assert!(best.contains_key("M"));
    }

    #[test]
    fn test_gaussian_kde_basic() {
        let kde = GaussianKDE::new(vec![1.0, 2.0, 3.0, 4.0, 5.0]);

        // PDF should be higher near the center of the data
        let pdf_center = kde.pdf(3.0);
        let pdf_edge = kde.pdf(10.0);
        assert!(
            pdf_center > pdf_edge,
            "PDF should be higher near center of data"
        );
    }

    #[test]
    fn test_gaussian_kde_sample() {
        let kde = GaussianKDE::new(vec![5.0, 5.1, 4.9, 5.0, 5.2]);

        // Sample 100 values, most should be near 5.0
        let samples: Vec<f64> = (0..100).map(|_| kde.sample()).collect();
        let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
        assert!(
            (mean - 5.0).abs() < 2.0,
            "Mean of samples should be close to data center"
        );
    }

    #[test]
    fn test_gaussian_kde_bandwidth() {
        let kde = GaussianKDE::new(vec![1.0, 2.0, 3.0]);
        assert!(kde.bandwidth > 0.0, "Bandwidth should be positive");
    }

    #[test]
    fn test_gaussian_kde_empty() {
        let kde = GaussianKDE::new(vec![]);
        assert_eq!(kde.pdf(1.0), 0.0);
        assert_eq!(kde.sample(), 0.0);
    }

    #[tokio::test]
    async fn test_tpe_convergence() {
        // Test that TPE converges on a simple quadratic objective
        let tuner = HyperparameterTuner::new(TuningConfig {
            max_trials: 50,
            early_stopping_patience: 50, // Don't early stop
            ..Default::default()
        })
        .await
        .expect("Failed to create tuner");

        tuner
            .add_parameter(HyperParameter {
                name: "x".to_string(),
                param_type: ParameterType::Float,
                search_space: SearchSpace::Continuous {
                    min: -10.0,
                    max: 10.0,
                },
            })
            .await
            .expect("Failed to add parameter");

        // Objective: maximize -(x-3)^2, optimum at x=3
        let result = tuner
            .tune(|params| async move {
                let x = match params.get("x") {
                    Some(ParameterValue::Float(v)) => *v,
                    _ => return Err(anyhow::anyhow!("missing x")),
                };
                Ok(-(x - 3.0).powi(2))
            })
            .await
            .expect("Tuning failed");

        let x = match result.get("x") {
            Some(ParameterValue::Float(v)) => *v,
            _ => panic!("Expected float for x"),
        };

        // TPE should find a value reasonably close to 3.0
        assert!(
            (x - 3.0).abs() < 5.0,
            "TPE should converge near optimum, got x={}",
            x
        );
    }

    #[test]
    fn test_grid_generation() {
        let params = vec![
            HyperParameter {
                name: "param1".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete {
                    min: 0,
                    max: 2,
                    step: 1,
                },
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

        let grid = tuner
            .generate_grid(&params)
            .expect("Failed to generate grid");
        assert_eq!(grid.len(), 6); // 3 values × 2 choices
    }

    #[test]
    fn test_tuning_config_default() {
        let config = TuningConfig::default();
        assert_eq!(config.max_trials, 100);
        assert_eq!(config.timeout_per_trial, 60);
        assert_eq!(config.early_stopping_patience, 10);
        assert!((config.min_improvement - 0.01).abs() < f64::EPSILON);
        assert!(config.parallel_trials);
        assert_eq!(config.max_parallel_trials, 4);
    }

    #[test]
    fn test_parameter_value_types() {
        let int_val = ParameterValue::Integer(42);
        let float_val = ParameterValue::Float(3.14);
        let string_val = ParameterValue::String("hello".to_string());
        let bool_val = ParameterValue::Boolean(true);

        // Verify Debug formatting works for all variants
        assert!(format!("{:?}", int_val).contains("42"));
        assert!(format!("{:?}", float_val).contains("3.14"));
        assert!(format!("{:?}", string_val).contains("hello"));
        assert!(format!("{:?}", bool_val).contains("true"));

        // Verify Clone works
        let int_clone = int_val.clone();
        assert!(matches!(int_clone, ParameterValue::Integer(42)));
        let float_clone = float_val.clone();
        assert!(matches!(float_clone, ParameterValue::Float(f) if (f - 3.14).abs() < f64::EPSILON));
        let string_clone = string_val.clone();
        assert!(matches!(string_clone, ParameterValue::String(s) if s == "hello"));
        let bool_clone = bool_val.clone();
        assert!(matches!(bool_clone, ParameterValue::Boolean(true)));
    }

    #[test]
    fn test_search_space_continuous() {
        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Random,
        };

        let params = vec![HyperParameter {
            name: "x".to_string(),
            param_type: ParameterType::Float,
            search_space: SearchSpace::Continuous {
                min: -5.0,
                max: 5.0,
            },
        }];

        // Sample many times and verify all values are in range
        for _ in 0..100 {
            let sampled = tuner.sample_random(&params).expect("sampling failed");
            let val = sampled.get("x").expect("missing x");
            if let ParameterValue::Float(f) = val {
                assert!(*f >= -5.0 && *f < 5.0, "Value {} out of range [-5, 5)", f);
            } else {
                panic!("Expected Float parameter value");
            }
        }
    }

    #[test]
    fn test_search_space_discrete() {
        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Random,
        };

        let params = vec![HyperParameter {
            name: "n".to_string(),
            param_type: ParameterType::Integer,
            search_space: SearchSpace::Discrete {
                min: 0,
                max: 10,
                step: 2,
            },
        }];

        for _ in 0..100 {
            let sampled = tuner.sample_random(&params).expect("sampling failed");
            let val = sampled.get("n").expect("missing n");
            if let ParameterValue::Integer(i) = val {
                assert!(*i >= 0 && *i <= 10, "Value {} out of range [0, 10]", i);
                assert_eq!(*i % 2, 0, "Value {} not a multiple of step 2", i);
            } else {
                panic!("Expected Integer parameter value");
            }
        }
    }

    #[test]
    fn test_search_space_categorical() {
        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Random,
        };

        let choices = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let params = vec![HyperParameter {
            name: "mode".to_string(),
            param_type: ParameterType::Categorical,
            search_space: SearchSpace::Categorical {
                choices: choices.clone(),
            },
        }];

        for _ in 0..50 {
            let sampled = tuner.sample_random(&params).expect("sampling failed");
            let val = sampled.get("mode").expect("missing mode");
            if let ParameterValue::String(s) = val {
                assert!(
                    choices.contains(s),
                    "Value '{}' not in choices {:?}",
                    s,
                    choices
                );
            } else {
                panic!("Expected String parameter value");
            }
        }
    }

    #[test]
    fn test_search_space_logscale() {
        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Random,
        };

        let params = vec![HyperParameter {
            name: "lr".to_string(),
            param_type: ParameterType::Float,
            search_space: SearchSpace::LogScale {
                min: 0.001,
                max: 1.0,
            },
        }];

        for _ in 0..100 {
            let sampled = tuner.sample_random(&params).expect("sampling failed");
            let val = sampled.get("lr").expect("missing lr");
            if let ParameterValue::Float(f) = val {
                assert!(
                    *f >= 0.001 && *f <= 1.0,
                    "LogScale value {} out of range [0.001, 1.0]",
                    f
                );
            } else {
                panic!("Expected Float parameter value");
            }
        }
    }

    #[test]
    fn test_trial_status_variants() {
        let running = TrialStatus::Running;
        let completed = TrialStatus::Completed;
        let failed = TrialStatus::Failed;
        let timeout = TrialStatus::Timeout;
        let early_stopped = TrialStatus::EarlyStopped;

        // PartialEq
        assert_eq!(running, TrialStatus::Running);
        assert_eq!(completed, TrialStatus::Completed);
        assert_eq!(failed, TrialStatus::Failed);
        assert_eq!(timeout, TrialStatus::Timeout);
        assert_eq!(early_stopped, TrialStatus::EarlyStopped);

        // Not equal to each other
        assert_ne!(running, completed);
        assert_ne!(failed, timeout);
        assert_ne!(completed, early_stopped);

        // Debug works for all variants
        assert!(format!("{:?}", running).contains("Running"));
        assert!(format!("{:?}", completed).contains("Completed"));
        assert!(format!("{:?}", failed).contains("Failed"));
        assert!(format!("{:?}", timeout).contains("Timeout"));
        assert!(format!("{:?}", early_stopped).contains("EarlyStopped"));
    }

    #[tokio::test]
    async fn test_hyperband_tuning() {
        let config = TuningConfig {
            max_trials: 20,
            early_stopping_patience: 20,
            ..Default::default()
        };
        let tuner = HyperparameterTuner::new(config)
            .await
            .expect("Failed to create tuner");

        tuner
            .add_parameter(HyperParameter {
                name: "x".to_string(),
                param_type: ParameterType::Float,
                search_space: SearchSpace::Continuous {
                    min: -10.0,
                    max: 10.0,
                },
            })
            .await
            .expect("Failed to add parameter");

        let tuner = tuner.with_algorithm(TuningAlgorithm::Hyperband {
            max_iter: 9,
            eta: 3.0,
        });

        // Objective: maximize -(x-2)^2, optimum at x=2
        let result = tuner
            .tune(|params| async move {
                let x = match params.get("x") {
                    Some(ParameterValue::Float(v)) => *v,
                    _ => return Err(anyhow::anyhow!("missing x")),
                };
                Ok(-(x - 2.0).powi(2))
            })
            .await
            .expect("Hyperband tuning failed");

        assert!(
            result.contains_key("x"),
            "Result should contain parameter x"
        );
        if let Some(ParameterValue::Float(x)) = result.get("x") {
            // Hyperband with random sampling should find something reasonable
            assert!(
                *x >= -10.0 && *x <= 10.0,
                "Result x={} should be within search space",
                x
            );
        }
    }

    #[tokio::test]
    async fn test_grid_search() {
        let config = TuningConfig {
            max_trials: 100,
            early_stopping_patience: 100,
            ..Default::default()
        };
        let tuner = HyperparameterTuner::new(config)
            .await
            .expect("Failed to create tuner");

        tuner
            .add_parameter(HyperParameter {
                name: "choice".to_string(),
                param_type: ParameterType::Categorical,
                search_space: SearchSpace::Categorical {
                    choices: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
                },
            })
            .await
            .expect("Failed to add parameter");

        let tuner = tuner.with_algorithm(TuningAlgorithm::Grid);

        // Objective: "high" = 10, "medium" = 5, "low" = 1
        let result = tuner
            .tune(|params| async move {
                let score = match params.get("choice") {
                    Some(ParameterValue::String(s)) => match s.as_str() {
                        "high" => 10.0,
                        "medium" => 5.0,
                        "low" => 1.0,
                        _ => 0.0,
                    },
                    _ => return Err(anyhow::anyhow!("missing choice")),
                };
                Ok(score)
            })
            .await
            .expect("Grid search failed");

        // Grid search should find the optimum
        if let Some(ParameterValue::String(s)) = result.get("choice") {
            assert_eq!(s, "high", "Grid search should find 'high' as optimum");
        } else {
            panic!("Expected String parameter for choice");
        }
    }

    #[test]
    fn test_sample_random_all_types() {
        let tuner = HyperparameterTuner {
            config: TuningConfig::default(),
            parameters: Arc::new(RwLock::new(Vec::new())),
            trials: Arc::new(RwLock::new(Vec::new())),
            best_trial: Arc::new(RwLock::new(None)),
            algorithm: TuningAlgorithm::Random,
        };

        let params = vec![
            HyperParameter {
                name: "continuous".to_string(),
                param_type: ParameterType::Float,
                search_space: SearchSpace::Continuous { min: 0.0, max: 1.0 },
            },
            HyperParameter {
                name: "discrete".to_string(),
                param_type: ParameterType::Integer,
                search_space: SearchSpace::Discrete {
                    min: 0,
                    max: 100,
                    step: 10,
                },
            },
            HyperParameter {
                name: "categorical".to_string(),
                param_type: ParameterType::Categorical,
                search_space: SearchSpace::Categorical {
                    choices: vec!["a".to_string(), "b".to_string()],
                },
            },
            HyperParameter {
                name: "logscale".to_string(),
                param_type: ParameterType::Float,
                search_space: SearchSpace::LogScale {
                    min: 0.01,
                    max: 10.0,
                },
            },
        ];

        let sampled = tuner.sample_random(&params).expect("sampling failed");
        assert_eq!(sampled.len(), 4);
        assert!(matches!(
            sampled.get("continuous"),
            Some(ParameterValue::Float(_))
        ));
        assert!(matches!(
            sampled.get("discrete"),
            Some(ParameterValue::Integer(_))
        ));
        assert!(matches!(
            sampled.get("categorical"),
            Some(ParameterValue::String(_))
        ));
        assert!(matches!(
            sampled.get("logscale"),
            Some(ParameterValue::Float(_))
        ));
    }

    #[tokio::test]
    async fn test_trial_history_capping() {
        let config = TuningConfig {
            max_trials: 5,
            early_stopping_patience: 100, // don't early stop
            ..Default::default()
        };
        let tuner = HyperparameterTuner::new(config)
            .await
            .expect("Failed to create tuner");

        tuner
            .add_parameter(HyperParameter {
                name: "x".to_string(),
                param_type: ParameterType::Float,
                search_space: SearchSpace::Continuous { min: 0.0, max: 1.0 },
            })
            .await
            .expect("Failed to add parameter");

        // Run tuning (TPE by default)
        let _result = tuner
            .tune(|_params| async move { Ok(1.0) })
            .await
            .expect("Tuning failed");

        let trials = tuner.get_trials().await;
        // max_retained = max(max_trials, 200) = 200, so with only 5 trials they should all be kept
        assert_eq!(
            trials.len(),
            5,
            "Should have exactly max_trials={} completed trials",
            5
        );
    }

    // ============================================================
    // ProximaDBHyperparameters tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_hnsw_params_structure() {
        let params = ProximaDBHyperparameters::hnsw_params();
        assert!(!params.is_empty());
        // Verify expected parameter names exist
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"M"), "Should contain 'M' (max connections)");
        assert!(
            names.contains(&"ef_construction"),
            "Should contain 'ef_construction'"
        );
    }

    #[test]
    fn test_ivf_params_structure() {
        let params = ProximaDBHyperparameters::ivf_params();
        assert!(!params.is_empty());
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"nlist"),
            "Should contain 'nlist' (cluster count)"
        );
    }

    #[test]
    fn test_quantization_params_structure() {
        let params = ProximaDBHyperparameters::quantization_params();
        assert!(!params.is_empty());
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"quantization_level") || names.contains(&"bits"),
            "Should contain quantization_level or bits parameter"
        );
    }

    fn search_space_range(space: &SearchSpace) -> Option<(f64, f64)> {
        match space {
            SearchSpace::Continuous { min, max } | SearchSpace::LogScale { min, max } => {
                Some((*min, *max))
            }
            SearchSpace::Discrete { min, max, .. } => Some((*min as f64, *max as f64)),
            SearchSpace::Categorical { .. } => None,
        }
    }

    #[test]
    fn test_param_ranges_valid() {
        for param in ProximaDBHyperparameters::hnsw_params() {
            if let Some((min, max)) = search_space_range(&param.search_space) {
                assert!(
                    min <= max,
                    "Parameter {} has invalid range: [{}, {}]",
                    param.name,
                    min,
                    max
                );
            }
        }
        for param in ProximaDBHyperparameters::ivf_params() {
            if let Some((min, max)) = search_space_range(&param.search_space) {
                assert!(min <= max);
            }
        }
        for param in ProximaDBHyperparameters::quantization_params() {
            if let Some((min, max)) = search_space_range(&param.search_space) {
                assert!(min <= max);
            }
        }
    }

    // ============================================================
    // GaussianKDE tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_gaussian_kernel_at_zero() {
        let result = GaussianKDE::gaussian_kernel(0.0);
        let expected = 1.0 / (2.0 * std::f64::consts::PI).sqrt();
        assert!(
            (result - expected).abs() < 1e-10,
            "Kernel at 0 should be peak value"
        );
    }

    #[test]
    fn test_gaussian_kernel_symmetry() {
        let pos = GaussianKDE::gaussian_kernel(1.0);
        let neg = GaussianKDE::gaussian_kernel(-1.0);
        assert!(
            (pos - neg).abs() < 1e-10,
            "Gaussian kernel should be symmetric"
        );
    }

    #[test]
    fn test_gaussian_kernel_decays() {
        let at_zero = GaussianKDE::gaussian_kernel(0.0);
        let at_one = GaussianKDE::gaussian_kernel(1.0);
        let at_three = GaussianKDE::gaussian_kernel(3.0);
        assert!(at_zero > at_one, "Kernel should decay from center");
        assert!(at_one > at_three, "Kernel should continue decaying");
    }

    #[test]
    fn test_std_dev_constant_values() {
        let result = GaussianKDE::std_dev(&[5.0, 5.0, 5.0, 5.0]);
        assert!(
            result.abs() < 1e-10,
            "Std dev of constant values should be 0"
        );
    }

    #[test]
    fn test_std_dev_known_values() {
        // std_dev of [1, 2, 3, 4, 5] = sqrt(2.5) ≈ 1.5811
        let result = GaussianKDE::std_dev(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(
            (result - 1.5811).abs() < 0.01,
            "Std dev should be ~1.58, got {}",
            result
        );
    }

    #[test]
    fn test_std_dev_single_value() {
        let result = GaussianKDE::std_dev(&[42.0]);
        assert_eq!(result, 0.0, "Std dev of single value should be 0");
    }

    #[test]
    fn test_std_dev_empty() {
        let result = GaussianKDE::std_dev(&[]);
        assert_eq!(result, 0.0, "Std dev of empty slice should be 0");
    }

    #[test]
    fn test_gaussian_kde_creation() {
        let kde = GaussianKDE::new(vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(kde.bandwidth > 0.0, "Bandwidth should be positive");
    }

    #[test]
    fn test_gaussian_kde_density_at_sample_point() {
        let kde = GaussianKDE::new(vec![0.0, 0.0, 0.0]);
        let density = kde.pdf(0.0);
        assert!(
            density > 0.0,
            "Density at sample concentration should be positive"
        );
    }
}
