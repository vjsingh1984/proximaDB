// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Optimization Pipeline for AutoML

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::info;

use super::prediction::{FeatureVector, PerformancePredictor};
use super::service::AutoMLConfig;
use super::workload::WorkloadPattern;

/// Optimization goals
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OptimizationGoal {
    /// Minimize query latency
    MinimizeLatency,
    /// Maximize throughput
    MaximizeThroughput,
    /// Minimize memory usage
    MinimizeMemory,
    /// Maximize accuracy/recall
    MaximizeAccuracy,
    /// Balance between latency and throughput
    Balanced,
    /// Custom goal with weighted objectives
    Custom(ObjectiveWeights),
}

/// Weighted objectives for custom optimization
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveWeights {
    pub latency: f64,
    pub throughput: f64,
    pub memory: f64,
    pub accuracy: f64,
}

impl Default for ObjectiveWeights {
    fn default() -> Self {
        Self {
            latency: 0.25,
            throughput: 0.25,
            memory: 0.25,
            accuracy: 0.25,
        }
    }
}

/// Configuration space for optimization
#[derive(Debug, Clone)]
pub struct ConfigurationSpace {
    /// Index configurations
    pub index_configs: Vec<IndexConfiguration>,
    /// Quantization configurations
    pub quantization_configs: Vec<QuantizationConfiguration>,
    /// Engine configurations
    pub engine_configs: Vec<EngineConfiguration>,
    /// Cache configurations
    pub cache_configs: Vec<CacheConfiguration>,
}

/// Index configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfiguration {
    pub algorithm: String,
    pub parameters: HashMap<String, f64>,
}

/// Quantization configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfiguration {
    pub level: String,
    pub codebook_size: usize,
    pub training_samples: usize,
}

/// Engine configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfiguration {
    pub engine_type: String,
    pub flush_threshold_mb: f64,
    pub compaction_interval_secs: u64,
}

/// Cache configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfiguration {
    pub cache_size_mb: f64,
    pub eviction_policy: String,
    pub prefetch_enabled: bool,
}

/// Optimization pipeline
pub struct OptimizationPipeline {
    /// Configuration
    #[allow(dead_code)]
    config: AutoMLConfig,
    /// Performance predictor
    predictor: Arc<PerformancePredictor>,
    /// Current configurations by collection
    current_configs: Arc<RwLock<HashMap<String, Configuration>>>,
    /// Optimization history
    history: Arc<RwLock<Vec<OptimizationRun>>>,
    /// Pipeline state
    state: Arc<RwLock<PipelineState>>,
}

/// Complete configuration for a collection
#[derive(Debug, Clone)]
pub struct Configuration {
    pub index: IndexConfiguration,
    pub quantization: QuantizationConfiguration,
    pub engine: EngineConfiguration,
    pub cache: CacheConfiguration,
}

/// Optimization run record
#[derive(Debug, Clone)]
pub struct OptimizationRun {
    pub collection_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub goal: OptimizationGoal,
    pub iterations: usize,
    pub best_config: Configuration,
    pub improvement: f64,
    pub duration_ms: u64,
}

/// Pipeline state
#[derive(Debug, Clone)]
pub enum PipelineState {
    Idle,
    Running,
    Stopped,
}

/// Optimization strategy
#[derive(Debug, Clone)]
pub enum OptimizationStrategy {
    /// Grid search through all combinations
    GridSearch,
    /// Random search with budget
    RandomSearch { budget: usize },
    /// Bayesian optimization
    BayesianOptimization { n_iterations: usize },
    /// Genetic algorithm
    GeneticAlgorithm {
        population_size: usize,
        generations: usize,
    },
}

impl OptimizationPipeline {
    /// Create a new optimization pipeline
    pub async fn new(config: AutoMLConfig) -> Result<Self> {
        let predictor = Arc::new(PerformancePredictor::new().await?);

        Ok(Self {
            config,
            predictor,
            current_configs: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
            state: Arc::new(RwLock::new(PipelineState::Idle)),
        })
    }

    /// Start the optimization pipeline
    pub async fn start(&self) -> Result<()> {
        let mut state = self.state.write().await;
        *state = PipelineState::Running;

        info!("Optimization pipeline started");
        Ok(())
    }

    /// Stop the optimization pipeline
    pub async fn stop(&self) -> Result<()> {
        let mut state = self.state.write().await;
        *state = PipelineState::Stopped;

        info!("Optimization pipeline stopped");
        Ok(())
    }

    /// Optimize configuration for a collection
    pub async fn optimize(
        &self,
        collection_id: &str,
        goal: OptimizationGoal,
        strategy: OptimizationStrategy,
        workload: WorkloadPattern,
    ) -> Result<Configuration> {
        let start_time = Instant::now();

        // Check if pipeline is running
        let state = self.state.read().await;
        if !matches!(*state, PipelineState::Running) {
            return Err(anyhow::anyhow!("Optimization pipeline is not running"));
        }
        drop(state);

        // Generate configuration space based on workload
        let config_space = self.generate_config_space(&workload)?;

        // Run optimization based on strategy
        let best_config = match strategy {
            OptimizationStrategy::GridSearch => {
                self.grid_search(collection_id, &goal, &config_space)
                    .await?
            }
            OptimizationStrategy::RandomSearch { budget } => {
                self.random_search(collection_id, &goal, &config_space, budget)
                    .await?
            }
            OptimizationStrategy::BayesianOptimization { n_iterations } => {
                self.bayesian_optimization(collection_id, &goal, &config_space, n_iterations)
                    .await?
            }
            OptimizationStrategy::GeneticAlgorithm {
                population_size,
                generations,
            } => {
                self.genetic_algorithm(
                    collection_id,
                    &goal,
                    &config_space,
                    population_size,
                    generations,
                )
                .await?
            }
        };

        // Calculate improvement
        let improvement = self
            .calculate_improvement(collection_id, &best_config)
            .await?;

        // Record optimization run
        let run = OptimizationRun {
            collection_id: collection_id.to_string(),
            timestamp: chrono::Utc::now(),
            goal,
            iterations: 0, // Would be set by strategy
            best_config: best_config.clone(),
            improvement,
            duration_ms: start_time.elapsed().as_millis() as u64,
        };

        let mut history = self.history.write().await;
        history.push(run);

        // Update current configuration
        let mut configs = self.current_configs.write().await;
        configs.insert(collection_id.to_string(), best_config.clone());

        Ok(best_config)
    }

    /// Generate configuration space based on workload
    fn generate_config_space(&self, workload: &WorkloadPattern) -> Result<ConfigurationSpace> {
        let index_configs = match workload {
            WorkloadPattern::ReadHeavy => vec![
                IndexConfiguration {
                    algorithm: "HNSW".to_string(),
                    parameters: [
                        ("M".to_string(), 16.0),
                        ("ef_construction".to_string(), 200.0),
                    ]
                    .into_iter()
                    .collect(),
                },
                IndexConfiguration {
                    algorithm: "IVF".to_string(),
                    parameters: [("nlist".to_string(), 100.0), ("nprobe".to_string(), 10.0)]
                        .into_iter()
                        .collect(),
                },
            ],
            WorkloadPattern::WriteHeavy => vec![IndexConfiguration {
                algorithm: "LSH".to_string(),
                parameters: [
                    ("n_tables".to_string(), 8.0),
                    ("n_projections".to_string(), 128.0),
                ]
                .into_iter()
                .collect(),
            }],
            _ => vec![IndexConfiguration {
                algorithm: "HNSW".to_string(),
                parameters: [
                    ("M".to_string(), 12.0),
                    ("ef_construction".to_string(), 150.0),
                ]
                .into_iter()
                .collect(),
            }],
        };

        let quantization_configs = vec![
            QuantizationConfiguration {
                level: "None".to_string(),
                codebook_size: 0,
                training_samples: 0,
            },
            QuantizationConfiguration {
                level: "INT8".to_string(),
                codebook_size: 256,
                training_samples: 10000,
            },
            QuantizationConfiguration {
                level: "PQ8".to_string(),
                codebook_size: 256,
                training_samples: 100000,
            },
        ];

        let engine_configs = vec![
            EngineConfiguration {
                engine_type: "SST".to_string(),
                flush_threshold_mb: 64.0,
                compaction_interval_secs: 300,
            },
            EngineConfiguration {
                engine_type: "VIPER".to_string(),
                flush_threshold_mb: 128.0,
                compaction_interval_secs: 600,
            },
            EngineConfiguration {
                engine_type: "NOVA".to_string(),
                flush_threshold_mb: 96.0,
                compaction_interval_secs: 450,
            },
        ];

        let cache_configs = vec![
            CacheConfiguration {
                cache_size_mb: 512.0,
                eviction_policy: "LRU".to_string(),
                prefetch_enabled: false,
            },
            CacheConfiguration {
                cache_size_mb: 1024.0,
                eviction_policy: "LFU".to_string(),
                prefetch_enabled: true,
            },
        ];

        Ok(ConfigurationSpace {
            index_configs,
            quantization_configs,
            engine_configs,
            cache_configs,
        })
    }

    /// Grid search optimization
    async fn grid_search(
        &self,
        collection_id: &str,
        goal: &OptimizationGoal,
        config_space: &ConfigurationSpace,
    ) -> Result<Configuration> {
        let mut best_config = None;
        let mut best_score = f64::NEG_INFINITY;

        for index_config in &config_space.index_configs {
            for quant_config in &config_space.quantization_configs {
                for engine_config in &config_space.engine_configs {
                    for cache_config in &config_space.cache_configs {
                        let config = Configuration {
                            index: index_config.clone(),
                            quantization: quant_config.clone(),
                            engine: engine_config.clone(),
                            cache: cache_config.clone(),
                        };

                        let score = self
                            .evaluate_configuration(collection_id, &config, goal)
                            .await?;

                        if score > best_score {
                            best_score = score;
                            best_config = Some(config);
                        }
                    }
                }
            }
        }

        best_config.ok_or_else(|| anyhow::anyhow!("No valid configuration found"))
    }

    /// Random search optimization
    async fn random_search(
        &self,
        collection_id: &str,
        goal: &OptimizationGoal,
        config_space: &ConfigurationSpace,
        budget: usize,
    ) -> Result<Configuration> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        let mut best_config = None;
        let mut best_score = f64::NEG_INFINITY;

        for _ in 0..budget {
            let config = Configuration {
                index: config_space.index_configs.choose(&mut rng).unwrap().clone(),
                quantization: config_space
                    .quantization_configs
                    .choose(&mut rng)
                    .unwrap()
                    .clone(),
                engine: config_space
                    .engine_configs
                    .choose(&mut rng)
                    .unwrap()
                    .clone(),
                cache: config_space.cache_configs.choose(&mut rng).unwrap().clone(),
            };

            let score = self
                .evaluate_configuration(collection_id, &config, goal)
                .await?;

            if score > best_score {
                best_score = score;
                best_config = Some(config);
            }
        }

        best_config.ok_or_else(|| anyhow::anyhow!("No valid configuration found"))
    }

    /// Bayesian optimization
    async fn bayesian_optimization(
        &self,
        collection_id: &str,
        goal: &OptimizationGoal,
        config_space: &ConfigurationSpace,
        n_iterations: usize,
    ) -> Result<Configuration> {
        // Simplified Bayesian optimization
        // In production, this would use Gaussian processes and acquisition functions

        let mut observations = Vec::new();

        for i in 0..n_iterations {
            // Select next configuration using acquisition function
            let config = if i < 5 {
                // Initial random exploration
                self.sample_random_config(config_space)?
            } else {
                // Use acquisition function (simplified)
                self.select_next_config(&observations, config_space)?
            };

            let score = self
                .evaluate_configuration(collection_id, &config, goal)
                .await?;
            observations.push((config, score));
        }

        // Return best observed configuration
        observations
            .into_iter()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(config, _)| config)
            .ok_or_else(|| anyhow::anyhow!("No valid configuration found"))
    }

    /// Genetic algorithm optimization
    async fn genetic_algorithm(
        &self,
        collection_id: &str,
        goal: &OptimizationGoal,
        config_space: &ConfigurationSpace,
        population_size: usize,
        generations: usize,
    ) -> Result<Configuration> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        // Initialize population
        let mut population: Vec<(Configuration, f64)> = Vec::new();
        for _ in 0..population_size {
            let config = self.sample_random_config(config_space)?;
            let score = self
                .evaluate_configuration(collection_id, &config, goal)
                .await?;
            population.push((config, score));
        }

        // Evolution loop
        for _ in 0..generations {
            // Selection (tournament)
            let mut new_population = Vec::new();

            while new_population.len() < population_size {
                // Tournament selection
                let parent1 = population.choose(&mut rng).unwrap();
                let parent2 = population.choose(&mut rng).unwrap();

                // Crossover
                let child = self.crossover(&parent1.0, &parent2.0, config_space)?;

                // Mutation
                let mutated = self.mutate(&child, config_space)?;

                // Evaluate
                let score = self
                    .evaluate_configuration(collection_id, &mutated, goal)
                    .await?;
                new_population.push((mutated, score));
            }

            // Replace population
            population = new_population;
        }

        // Return best individual
        population
            .into_iter()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(config, _)| config)
            .ok_or_else(|| anyhow::anyhow!("No valid configuration found"))
    }

    /// Sample a random configuration
    fn sample_random_config(&self, config_space: &ConfigurationSpace) -> Result<Configuration> {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        Ok(Configuration {
            index: config_space.index_configs.choose(&mut rng).unwrap().clone(),
            quantization: config_space
                .quantization_configs
                .choose(&mut rng)
                .unwrap()
                .clone(),
            engine: config_space
                .engine_configs
                .choose(&mut rng)
                .unwrap()
                .clone(),
            cache: config_space.cache_configs.choose(&mut rng).unwrap().clone(),
        })
    }

    /// Select next configuration for Bayesian optimization
    fn select_next_config(
        &self,
        _observations: &[(Configuration, f64)],
        config_space: &ConfigurationSpace,
    ) -> Result<Configuration> {
        // Simplified - would use acquisition function
        self.sample_random_config(config_space)
    }

    /// Crossover two configurations
    fn crossover(
        &self,
        parent1: &Configuration,
        parent2: &Configuration,
        _config_space: &ConfigurationSpace,
    ) -> Result<Configuration> {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        Ok(Configuration {
            index: if rng.gen_bool(0.5) {
                parent1.index.clone()
            } else {
                parent2.index.clone()
            },
            quantization: if rng.gen_bool(0.5) {
                parent1.quantization.clone()
            } else {
                parent2.quantization.clone()
            },
            engine: if rng.gen_bool(0.5) {
                parent1.engine.clone()
            } else {
                parent2.engine.clone()
            },
            cache: if rng.gen_bool(0.5) {
                parent1.cache.clone()
            } else {
                parent2.cache.clone()
            },
        })
    }

    /// Mutate a configuration
    fn mutate(
        &self,
        config: &Configuration,
        config_space: &ConfigurationSpace,
    ) -> Result<Configuration> {
        use rand::Rng;
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();

        let mut mutated = config.clone();

        if rng.gen_bool(0.1) {
            mutated.index = config_space.index_configs.choose(&mut rng).unwrap().clone();
        }
        if rng.gen_bool(0.1) {
            mutated.quantization = config_space
                .quantization_configs
                .choose(&mut rng)
                .unwrap()
                .clone();
        }
        if rng.gen_bool(0.1) {
            mutated.engine = config_space
                .engine_configs
                .choose(&mut rng)
                .unwrap()
                .clone();
        }
        if rng.gen_bool(0.1) {
            mutated.cache = config_space.cache_configs.choose(&mut rng).unwrap().clone();
        }

        Ok(mutated)
    }

    /// Evaluate a configuration
    async fn evaluate_configuration(
        &self,
        collection_id: &str,
        config: &Configuration,
        goal: &OptimizationGoal,
    ) -> Result<f64> {
        // Create feature vector from configuration
        let features = self.config_to_features(config)?;

        // Predict performance
        let prediction = self
            .predictor
            .predict(collection_id, &features)
            .await
            .unwrap_or_else(|_| super::prediction::PredictionResult {
                value: 100.0, // Default value
                confidence: 0.0,
                model_type: "default".to_string(),
            });

        // Calculate score based on goal
        let score = match goal {
            OptimizationGoal::MinimizeLatency => -prediction.value,
            OptimizationGoal::MaximizeThroughput => prediction.value,
            OptimizationGoal::MinimizeMemory => -prediction.value,
            OptimizationGoal::MaximizeAccuracy => prediction.value,
            OptimizationGoal::Balanced => prediction.value / 2.0,
            OptimizationGoal::Custom(weights) => {
                // Weighted combination
                weights.latency * (-prediction.value) + weights.throughput * prediction.value
            }
        };

        Ok(score)
    }

    /// Convert configuration to feature vector
    fn config_to_features(&self, config: &Configuration) -> Result<FeatureVector> {
        let index_encoding = match config.index.algorithm.as_str() {
            "HNSW" => 1.0,
            "IVF" => 2.0,
            "LSH" => 3.0,
            _ => 0.0,
        };

        let quant_encoding = match config.quantization.level.as_str() {
            "None" => 0.0,
            "Binary" => 1.0,
            "INT8" => 2.0,
            "PQ4" => 3.0,
            "PQ8" => 4.0,
            "PQ16" => 5.0,
            "PQ32" => 6.0,
            _ => 0.0,
        };

        Ok(FeatureVector {
            vector_count: 10000.0,   // Default
            vector_dimension: 128.0, // Default
            sparsity: 0.1,           // Default
            read_ratio: 0.8,         // Default
            write_ratio: 0.2,        // Default
            query_complexity: 1.0,   // Default
            batch_size: 100.0,       // Default
            index_type: index_encoding,
            quantization_level: quant_encoding,
            cache_size_mb: config.cache.cache_size_mb,
            cpu_cores: 4.0, // Default
            memory_gb: 8.0, // Default
            disk_type: 1.0, // SSD default
        })
    }

    /// Calculate improvement from optimization
    async fn calculate_improvement(
        &self,
        _collection_id: &str,
        _config: &Configuration,
    ) -> Result<f64> {
        // Placeholder - would compare with baseline
        Ok(15.0) // 15% improvement
    }

    /// Get current configuration for a collection
    pub async fn get_current_config(&self, collection_id: &str) -> Option<Configuration> {
        let configs = self.current_configs.read().await;
        configs.get(collection_id).cloned()
    }

    /// Get optimization history
    pub async fn get_history(&self) -> Vec<OptimizationRun> {
        let history = self.history.read().await;
        history.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_grid_search() {
        let config = AutoMLConfig::default();
        let pipeline = OptimizationPipeline::new(config).await.unwrap();
        pipeline.start().await.unwrap();

        let config_space = ConfigurationSpace {
            index_configs: vec![IndexConfiguration {
                algorithm: "HNSW".to_string(),
                parameters: HashMap::new(),
            }],
            quantization_configs: vec![QuantizationConfiguration {
                level: "None".to_string(),
                codebook_size: 0,
                training_samples: 0,
            }],
            engine_configs: vec![EngineConfiguration {
                engine_type: "SST".to_string(),
                flush_threshold_mb: 64.0,
                compaction_interval_secs: 300,
            }],
            cache_configs: vec![CacheConfiguration {
                cache_size_mb: 512.0,
                eviction_policy: "LRU".to_string(),
                prefetch_enabled: false,
            }],
        };

        let best = pipeline
            .grid_search(
                "test_collection",
                &OptimizationGoal::MinimizeLatency,
                &config_space,
            )
            .await
            .unwrap();

        assert_eq!(best.index.algorithm, "HNSW");
    }
}
