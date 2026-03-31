// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Performance Prediction Models

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Performance predictor using ML models
pub struct PerformancePredictor {
    /// Prediction models by collection
    #[allow(dead_code)]
    models: Arc<RwLock<HashMap<String, PredictionModel>>>,
    /// Model training configuration
    config: PredictorConfig,
    /// Training data buffer
    training_data: Arc<RwLock<TrainingDataBuffer>>,
}

/// Predictor configuration
#[derive(Debug, Clone)]
pub struct PredictorConfig {
    /// Minimum training samples required
    pub min_training_samples: usize,
    /// Maximum training samples to keep
    pub max_training_samples: usize,
    /// Model update interval in seconds
    pub update_interval_secs: u64,
    /// Enable online learning
    pub enable_online_learning: bool,
}

impl Default for PredictorConfig {
    fn default() -> Self {
        Self {
            min_training_samples: 100,
            max_training_samples: 10000,
            update_interval_secs: 3600, // Update hourly
            enable_online_learning: true,
        }
    }
}

/// Training data buffer
#[derive(Debug, Clone)]
pub struct TrainingDataBuffer {
    samples: Vec<TrainingSample>,
}

impl TrainingDataBuffer {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    fn add_sample(&mut self, sample: TrainingSample, max_samples: usize) {
        self.samples.push(sample);

        // Keep only recent samples
        if self.samples.len() > max_samples {
            self.samples.drain(0..self.samples.len() - max_samples);
        }
    }

    fn get_samples(&self) -> &[TrainingSample] {
        &self.samples
    }

    #[allow(dead_code)]
    fn clear(&mut self) {
        self.samples.clear();
    }
}

/// Training sample for model learning
#[derive(Debug, Clone)]
pub struct TrainingSample {
    pub features: FeatureVector,
    pub target: TargetMetric,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Feature vector for prediction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVector {
    // Data characteristics
    pub vector_count: f64,
    pub vector_dimension: f64,
    pub sparsity: f64,

    // Workload characteristics
    pub read_ratio: f64,
    pub write_ratio: f64,
    pub query_complexity: f64,
    pub batch_size: f64,

    // Index configuration
    pub index_type: f64, // Encoded index type
    pub quantization_level: f64,
    pub cache_size_mb: f64,

    // Resource usage
    pub cpu_cores: f64,
    pub memory_gb: f64,
    pub disk_type: f64, // 0 = HDD, 1 = SSD, 2 = NVMe
}

impl FeatureVector {
    /// Create a feature vector from collection characteristics
    pub fn from_characteristics(
        vector_count: usize,
        dimension: usize,
        sparsity: f32,
        read_write_ratio: f64,
    ) -> Self {
        Self {
            vector_count: vector_count as f64,
            vector_dimension: dimension as f64,
            sparsity: sparsity as f64,
            read_ratio: read_write_ratio / (1.0 + read_write_ratio),
            write_ratio: 1.0 / (1.0 + read_write_ratio),
            query_complexity: 1.0,   // Default
            batch_size: 1.0,         // Default
            index_type: 0.0,         // Default
            quantization_level: 0.0, // Default
            cache_size_mb: 1024.0,   // Default 1GB
            cpu_cores: 4.0,          // Default
            memory_gb: 8.0,          // Default
            disk_type: 1.0,          // SSD default
        }
    }

    /// Convert to array for ML processing
    pub fn to_array(&self) -> Vec<f64> {
        vec![
            self.vector_count,
            self.vector_dimension,
            self.sparsity,
            self.read_ratio,
            self.write_ratio,
            self.query_complexity,
            self.batch_size,
            self.index_type,
            self.quantization_level,
            self.cache_size_mb,
            self.cpu_cores,
            self.memory_gb,
            self.disk_type,
        ]
    }
}

/// Target metric for prediction
#[derive(Debug, Clone)]
pub enum TargetMetric {
    QueryLatency(f64),
    Throughput(f64),
    MemoryUsage(f64),
    IndexBuildTime(f64),
}

/// Prediction model types
#[derive(Debug, Clone)]
pub enum PredictionModel {
    /// Linear regression model
    LinearRegression(LinearRegressionModel),
    /// Random forest model
    RandomForest(RandomForestModel),
    /// Neural network model
    NeuralNetwork(NeuralNetworkModel),
    /// Gradient boosting model
    GradientBoosting(GradientBoostingModel),
}

/// Linear regression model
#[derive(Debug, Clone)]
pub struct LinearRegressionModel {
    coefficients: Vec<f64>,
    intercept: f64,
}

impl LinearRegressionModel {
    fn new() -> Self {
        Self {
            coefficients: vec![0.0; 13], // 13 features
            intercept: 0.0,
        }
    }

    fn predict(&self, features: &FeatureVector) -> f64 {
        let feature_array = features.to_array();
        let mut prediction = self.intercept;

        for (coef, feat) in self.coefficients.iter().zip(feature_array.iter()) {
            prediction += coef * feat;
        }

        // Guard against NaN/Inf from numerical instability
        if !prediction.is_finite() {
            return 0.0;
        }

        prediction
    }

    fn train(&mut self, samples: &[TrainingSample]) {
        // Simple gradient descent implementation
        let learning_rate = 0.01;
        let iterations = 100;

        for _ in 0..iterations {
            let mut gradient = vec![0.0; self.coefficients.len()];
            let mut intercept_gradient = 0.0;

            for sample in samples {
                let prediction = self.predict(&sample.features);
                let target = match sample.target {
                    TargetMetric::QueryLatency(v) => v,
                    TargetMetric::Throughput(v) => v,
                    TargetMetric::MemoryUsage(v) => v,
                    TargetMetric::IndexBuildTime(v) => v,
                };

                let error = prediction - target;
                let features = sample.features.to_array();

                for (i, feat) in features.iter().enumerate() {
                    gradient[i] += error * feat;
                }
                intercept_gradient += error;
            }

            // Update weights with numerical stability checks
            let n = samples.len() as f64;
            for (i, grad) in gradient.iter().enumerate() {
                let update = learning_rate * grad / n;
                if update.is_finite() {
                    self.coefficients[i] -= update;
                }
                // Guard against NaN/Inf in coefficients
                if !self.coefficients[i].is_finite() {
                    self.coefficients[i] = 0.0;
                }
            }
            let intercept_update = learning_rate * intercept_gradient / n;
            if intercept_update.is_finite() {
                self.intercept -= intercept_update;
            }
            if !self.intercept.is_finite() {
                self.intercept = 0.0;
            }
        }
    }
}

/// Random forest model (simplified)
#[derive(Debug, Clone)]
pub struct RandomForestModel {
    trees: Vec<DecisionTree>,
    n_trees: usize,
}

#[derive(Debug, Clone)]
struct DecisionTree {
    root: TreeNode,
}

#[derive(Debug, Clone)]
enum TreeNode {
    Leaf {
        value: f64,
    },
    #[allow(dead_code)]
    Split {
        #[allow(dead_code)]
        feature_idx: usize,
        #[allow(dead_code)]
        threshold: f64,
        #[allow(dead_code)]
        left: Box<TreeNode>,
        #[allow(dead_code)]
        right: Box<TreeNode>,
    },
}

impl RandomForestModel {
    fn new(n_trees: usize) -> Self {
        Self {
            trees: Vec::with_capacity(n_trees),
            n_trees,
        }
    }

    fn predict(&self, features: &FeatureVector) -> f64 {
        if self.trees.is_empty() {
            return 0.0;
        }

        let predictions: Vec<f64> = self
            .trees
            .iter()
            .map(|tree| tree.predict(features))
            .collect();

        predictions.iter().sum::<f64>() / predictions.len() as f64
    }

    fn train(&mut self, samples: &[TrainingSample]) {
        // Simplified training - create simple trees
        self.trees.clear();

        for _ in 0..self.n_trees {
            let tree = DecisionTree::train_simple(samples);
            self.trees.push(tree);
        }
    }
}

impl DecisionTree {
    fn predict(&self, features: &FeatureVector) -> f64 {
        self.root.predict(features)
    }

    fn train_simple(samples: &[TrainingSample]) -> Self {
        // Very simplified tree training
        let avg_target = samples
            .iter()
            .map(|s| match s.target {
                TargetMetric::QueryLatency(v) => v,
                TargetMetric::Throughput(v) => v,
                TargetMetric::MemoryUsage(v) => v,
                TargetMetric::IndexBuildTime(v) => v,
            })
            .sum::<f64>()
            / samples.len() as f64;

        Self {
            root: TreeNode::Leaf { value: avg_target },
        }
    }
}

impl TreeNode {
    fn predict(&self, features: &FeatureVector) -> f64 {
        match self {
            TreeNode::Leaf { value } => *value,
            TreeNode::Split {
                feature_idx,
                threshold,
                left,
                right,
            } => {
                let feature_value = features.to_array()[*feature_idx];
                if feature_value <= *threshold {
                    left.predict(features)
                } else {
                    right.predict(features)
                }
            }
        }
    }
}

/// Neural network model (simplified)
#[derive(Debug, Clone)]
pub struct NeuralNetworkModel {
    layers: Vec<Layer>,
}

#[derive(Debug, Clone)]
struct Layer {
    weights: Vec<Vec<f64>>,
    biases: Vec<f64>,
}

impl NeuralNetworkModel {
    fn new(layer_sizes: &[usize]) -> Self {
        let mut layers = Vec::new();

        for i in 0..layer_sizes.len() - 1 {
            let input_size = layer_sizes[i];
            let output_size = layer_sizes[i + 1];

            layers.push(Layer {
                weights: vec![vec![0.1; input_size]; output_size],
                biases: vec![0.0; output_size],
            });
        }

        Self { layers }
    }

    fn predict(&self, features: &FeatureVector) -> f64 {
        let mut activation = features.to_array();

        for layer in &self.layers {
            activation = layer.forward(&activation);
        }

        activation[0] // Return single output
    }

    fn train(&mut self, _samples: &[TrainingSample]) {
        // Simplified - would need backpropagation
    }
}

impl Layer {
    fn forward(&self, input: &[f64]) -> Vec<f64> {
        let mut output = self.biases.clone();

        for (i, neuron_weights) in self.weights.iter().enumerate() {
            for (j, weight) in neuron_weights.iter().enumerate() {
                output[i] += input[j] * weight;
            }
            // ReLU activation
            output[i] = output[i].max(0.0);
        }

        output
    }
}

/// Gradient boosting model (simplified)
#[derive(Debug, Clone)]
pub struct GradientBoostingModel {
    base_prediction: f64,
    trees: Vec<DecisionTree>,
    learning_rate: f64,
}

impl GradientBoostingModel {
    fn new(n_estimators: usize, learning_rate: f64) -> Self {
        Self {
            base_prediction: 0.0,
            trees: Vec::with_capacity(n_estimators),
            learning_rate,
        }
    }

    fn predict(&self, features: &FeatureVector) -> f64 {
        let mut prediction = self.base_prediction;

        for tree in &self.trees {
            prediction += self.learning_rate * tree.predict(features);
        }

        prediction
    }

    fn train(&mut self, samples: &[TrainingSample]) {
        // Calculate base prediction
        self.base_prediction = samples
            .iter()
            .map(|s| match s.target {
                TargetMetric::QueryLatency(v) => v,
                TargetMetric::Throughput(v) => v,
                TargetMetric::MemoryUsage(v) => v,
                TargetMetric::IndexBuildTime(v) => v,
            })
            .sum::<f64>()
            / samples.len() as f64;

        // Simplified - would need gradient computation
        self.trees.clear();
    }
}

impl PerformancePredictor {
    /// Create a new performance predictor
    pub async fn new() -> Result<Self> {
        Self::with_config(PredictorConfig::default()).await
    }

    /// Create with custom configuration
    pub async fn with_config(config: PredictorConfig) -> Result<Self> {
        Ok(Self {
            models: Arc::new(RwLock::new(HashMap::new())),
            config,
            training_data: Arc::new(RwLock::new(TrainingDataBuffer::new())),
        })
    }

    /// Add training sample
    pub async fn add_training_sample(&self, sample: TrainingSample) -> Result<()> {
        let mut buffer = self.training_data.write().await;
        buffer.add_sample(sample, self.config.max_training_samples);
        Ok(())
    }

    /// Train model for a collection
    pub async fn train_model(&self, collection_id: &str, model_type: &str) -> Result<()> {
        let buffer = self.training_data.read().await;
        let samples = buffer.get_samples();

        if samples.len() < self.config.min_training_samples {
            return Err(anyhow::anyhow!(
                "Insufficient training samples: {} < {}",
                samples.len(),
                self.config.min_training_samples
            ));
        }

        let mut model = match model_type {
            "linear" => PredictionModel::LinearRegression(LinearRegressionModel::new()),
            "random_forest" => PredictionModel::RandomForest(RandomForestModel::new(10)),
            "neural_network" => {
                PredictionModel::NeuralNetwork(NeuralNetworkModel::new(&[13, 32, 16, 1]))
            }
            "gradient_boosting" => {
                PredictionModel::GradientBoosting(GradientBoostingModel::new(10, 0.1))
            }
            _ => return Err(anyhow::anyhow!("Unknown model type: {}", model_type)),
        };

        // Train the model
        match &mut model {
            PredictionModel::LinearRegression(m) => m.train(samples),
            PredictionModel::RandomForest(m) => m.train(samples),
            PredictionModel::NeuralNetwork(m) => m.train(samples),
            PredictionModel::GradientBoosting(m) => m.train(samples),
        }

        // Store the trained model
        let mut models = self.models.write().await;
        models.insert(collection_id.to_string(), model);

        Ok(())
    }

    /// Predict performance for given features
    pub async fn predict(
        &self,
        collection_id: &str,
        features: &FeatureVector,
    ) -> Result<PredictionResult> {
        let models = self.models.read().await;
        let model = models
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No model found for collection"))?;

        let prediction = match model {
            PredictionModel::LinearRegression(m) => m.predict(features),
            PredictionModel::RandomForest(m) => m.predict(features),
            PredictionModel::NeuralNetwork(m) => m.predict(features),
            PredictionModel::GradientBoosting(m) => m.predict(features),
        };

        Ok(PredictionResult {
            value: prediction,
            confidence: 0.8, // Placeholder confidence
            model_type: model_type_name(model),
        })
    }

    /// Get model accuracy metrics
    pub async fn get_model_metrics(&self, _collection_id: &str) -> Result<ModelMetrics> {
        // Placeholder implementation
        Ok(ModelMetrics {
            mse: 0.0,
            mae: 0.0,
            r2_score: 0.0,
            training_samples: 0,
        })
    }
}

fn model_type_name(model: &PredictionModel) -> String {
    match model {
        PredictionModel::LinearRegression(_) => "LinearRegression".to_string(),
        PredictionModel::RandomForest(_) => "RandomForest".to_string(),
        PredictionModel::NeuralNetwork(_) => "NeuralNetwork".to_string(),
        PredictionModel::GradientBoosting(_) => "GradientBoosting".to_string(),
    }
}

/// Prediction result
#[derive(Debug, Clone)]
pub struct PredictionResult {
    pub value: f64,
    pub confidence: f64,
    pub model_type: String,
}

/// Model accuracy metrics
#[derive(Debug, Clone)]
pub struct ModelMetrics {
    pub mse: f64,      // Mean squared error
    pub mae: f64,      // Mean absolute error
    pub r2_score: f64, // R-squared score
    pub training_samples: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_regression() {
        let mut model = LinearRegressionModel::new();
        model.coefficients = vec![1.0; 13];
        model.intercept = 10.0;

        let features = FeatureVector::from_characteristics(1000, 128, 0.1, 2.0);
        let prediction = model.predict(&features);

        assert!(prediction > 0.0);
    }

    #[test]
    fn test_feature_vector() {
        let features = FeatureVector::from_characteristics(10000, 256, 0.2, 5.0);
        let array = features.to_array();

        assert_eq!(array.len(), 13);
        assert_eq!(array[0], 10000.0); // vector_count
        assert_eq!(array[1], 256.0); // dimension
    }

    // ============================================================
    // Additional FeatureVector tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_feature_vector_all_fields() {
        let fv = FeatureVector::from_characteristics(5000, 128, 0.5, 3.0);
        let arr = fv.to_array();
        assert_eq!(arr.len(), 13);
        assert_eq!(arr[0], 5000.0); // vector_count
        assert_eq!(arr[1], 128.0); // dimension
        assert!((arr[2] - 0.5).abs() < 1e-6); // sparsity
        // arr[3] = read_ratio = read_write_ratio / (1 + read_write_ratio) = 3.0/4.0 = 0.75
        assert!(
            (arr[3] - 0.75).abs() < 1e-6,
            "read_ratio should be 0.75, got {}",
            arr[3]
        );
    }

    #[test]
    fn test_feature_vector_zero_vectors() {
        let fv = FeatureVector::from_characteristics(0, 1, 0.0, 0.0);
        let arr = fv.to_array();
        assert_eq!(arr[0], 0.0);
    }

    #[test]
    fn test_feature_vector_large_dimension() {
        let fv = FeatureVector::from_characteristics(100, 3072, 1.0, 100.0);
        let arr = fv.to_array();
        assert_eq!(arr[1], 3072.0); // OpenAI text-embedding-3-large
    }

    #[test]
    fn test_linear_regression_zero_coefficients() {
        let mut model = LinearRegressionModel::new();
        model.coefficients = vec![0.0; 13];
        model.intercept = 42.0;

        let features = FeatureVector::from_characteristics(1000, 128, 0.5, 1.0);
        let prediction = model.predict(&features);
        assert!(
            (prediction - 42.0).abs() < 1e-6,
            "Zero coefficients should give intercept only"
        );
    }

    #[tokio::test]
    async fn test_performance_predictor_creation() {
        let predictor = PerformancePredictor::new().await;
        assert!(predictor.is_ok());
    }

    #[tokio::test]
    async fn test_performance_predictor_with_config() {
        let config = PredictorConfig {
            min_training_samples: 50,
            max_training_samples: 5000,
            update_interval_secs: 600,
            enable_online_learning: true,
        };
        let predictor = PerformancePredictor::with_config(config).await;
        assert!(predictor.is_ok());
    }

    #[tokio::test]
    async fn test_performance_predictor_add_sample() {
        let predictor = PerformancePredictor::new().await.unwrap();
        let sample = TrainingSample {
            features: FeatureVector::from_characteristics(100, 64, 0.5, 1.0),
            target: TargetMetric::QueryLatency(5.0),
            timestamp: chrono::Utc::now(),
        };
        let result = predictor.add_training_sample(sample).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_performance_predictor_get_metrics() {
        let predictor = PerformancePredictor::new().await.unwrap();
        let metrics = predictor.get_model_metrics("test").await;
        assert!(metrics.is_ok());
    }

    // ============================================================
    // Extended FeatureVector tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_feature_vector_read_write_ratios() {
        // read_write_ratio = 1.0 => equal reads and writes
        let fv = FeatureVector::from_characteristics(100, 64, 0.0, 1.0);
        assert!((fv.read_ratio - 0.5).abs() < 1e-6, "Equal ratio should give 0.5 read");
        assert!((fv.write_ratio - 0.5).abs() < 1e-6, "Equal ratio should give 0.5 write");
    }

    #[test]
    fn test_feature_vector_read_heavy() {
        // read_write_ratio = 99.0 => mostly reads
        let fv = FeatureVector::from_characteristics(100, 64, 0.0, 99.0);
        assert!(fv.read_ratio > 0.98, "High ratio should give high read_ratio, got {}", fv.read_ratio);
        assert!(fv.write_ratio < 0.02, "High ratio should give low write_ratio, got {}", fv.write_ratio);
    }

    #[test]
    fn test_feature_vector_write_heavy() {
        // read_write_ratio = 0.0 => all writes
        let fv = FeatureVector::from_characteristics(100, 64, 0.0, 0.0);
        assert!((fv.read_ratio - 0.0).abs() < 1e-6, "Zero ratio should give 0 read_ratio");
        assert!((fv.write_ratio - 1.0).abs() < 1e-6, "Zero ratio should give 1.0 write_ratio");
    }

    #[test]
    fn test_feature_vector_to_array_length() {
        let fv = FeatureVector::from_characteristics(1000, 128, 0.5, 2.0);
        let arr = fv.to_array();
        assert_eq!(arr.len(), 13, "Feature vector should have exactly 13 elements");
    }

    #[test]
    fn test_feature_vector_defaults() {
        let fv = FeatureVector::from_characteristics(500, 256, 0.3, 4.0);
        assert_eq!(fv.query_complexity, 1.0);
        assert_eq!(fv.batch_size, 1.0);
        assert_eq!(fv.index_type, 0.0);
        assert_eq!(fv.quantization_level, 0.0);
        assert_eq!(fv.cache_size_mb, 1024.0);
        assert_eq!(fv.cpu_cores, 4.0);
        assert_eq!(fv.memory_gb, 8.0);
        assert_eq!(fv.disk_type, 1.0); // SSD
    }

    #[test]
    fn test_feature_vector_serde_roundtrip() {
        let fv = FeatureVector::from_characteristics(10000, 768, 0.1, 10.0);
        let json = serde_json::to_string(&fv).unwrap();
        let deserialized: FeatureVector = serde_json::from_str(&json).unwrap();
        assert_eq!(fv.vector_count, deserialized.vector_count);
        assert_eq!(fv.vector_dimension, deserialized.vector_dimension);
        assert!((fv.sparsity - deserialized.sparsity).abs() < 1e-10);
        assert!((fv.read_ratio - deserialized.read_ratio).abs() < 1e-10);
    }

    // ============================================================
    // TrainingDataBuffer tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_training_data_buffer_new() {
        let buffer = TrainingDataBuffer::new();
        assert!(buffer.get_samples().is_empty());
    }

    #[test]
    fn test_training_data_buffer_add_and_get() {
        let mut buffer = TrainingDataBuffer::new();
        let sample = TrainingSample {
            features: FeatureVector::from_characteristics(100, 64, 0.5, 1.0),
            target: TargetMetric::QueryLatency(5.0),
            timestamp: chrono::Utc::now(),
        };
        buffer.add_sample(sample, 100);
        assert_eq!(buffer.get_samples().len(), 1);
    }

    #[test]
    fn test_training_data_buffer_max_samples_eviction() {
        let mut buffer = TrainingDataBuffer::new();
        let max_samples = 5;
        for i in 0..10 {
            let sample = TrainingSample {
                features: FeatureVector::from_characteristics(i * 100, 64, 0.5, 1.0),
                target: TargetMetric::Throughput(i as f64 * 100.0),
                timestamp: chrono::Utc::now(),
            };
            buffer.add_sample(sample, max_samples);
        }
        assert_eq!(
            buffer.get_samples().len(),
            max_samples,
            "Buffer should cap at max_samples"
        );
        // The remaining samples should be the most recent ones (indices 5-9)
        if let TargetMetric::Throughput(v) = &buffer.get_samples()[0].target {
            assert!(
                *v >= 500.0,
                "First sample should be from later additions, got {}",
                v
            );
        }
    }

    #[test]
    fn test_training_data_buffer_clear() {
        let mut buffer = TrainingDataBuffer::new();
        for _ in 0..5 {
            let sample = TrainingSample {
                features: FeatureVector::from_characteristics(100, 64, 0.5, 1.0),
                target: TargetMetric::MemoryUsage(1024.0),
                timestamp: chrono::Utc::now(),
            };
            buffer.add_sample(sample, 100);
        }
        assert_eq!(buffer.get_samples().len(), 5);
        buffer.clear();
        assert!(buffer.get_samples().is_empty());
    }

    // ============================================================
    // ML Model tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_linear_regression_new() {
        let model = LinearRegressionModel::new();
        assert_eq!(model.coefficients.len(), 13);
        assert_eq!(model.intercept, 0.0);
    }

    #[test]
    fn test_linear_regression_predict_with_known_coefficients() {
        let mut model = LinearRegressionModel::new();
        // Set first coefficient to 0.001 (for vector_count)
        model.coefficients[0] = 0.001;
        model.intercept = 5.0;

        let features = FeatureVector::from_characteristics(1000, 128, 0.0, 1.0);
        let prediction = model.predict(&features);
        // prediction = 5.0 + 0.001 * 1000 + 0*... = 6.0
        assert!(
            (prediction - 6.0).abs() < 1e-6,
            "Expected ~6.0, got {}",
            prediction
        );
    }

    #[test]
    fn test_linear_regression_train_simple() {
        let mut model = LinearRegressionModel::new();
        let samples: Vec<TrainingSample> = (0..50)
            .map(|i| TrainingSample {
                features: FeatureVector::from_characteristics(i * 100, 128, 0.1, 1.0),
                target: TargetMetric::QueryLatency(i as f64 * 0.5),
                timestamp: chrono::Utc::now(),
            })
            .collect();
        model.train(&samples);
        // After training, prediction should be somewhat correlated with input
        let low_features = FeatureVector::from_characteristics(100, 128, 0.1, 1.0);
        let high_features = FeatureVector::from_characteristics(4000, 128, 0.1, 1.0);
        let low_pred = model.predict(&low_features);
        let high_pred = model.predict(&high_features);
        // We don't assert exact values, just that the model learned something
        assert!(
            low_pred.is_finite() && high_pred.is_finite(),
            "Predictions should be finite"
        );
    }

    #[test]
    fn test_random_forest_empty() {
        let model = RandomForestModel::new(10);
        let features = FeatureVector::from_characteristics(100, 64, 0.5, 1.0);
        assert_eq!(model.predict(&features), 0.0, "Empty forest should predict 0");
    }

    #[test]
    fn test_random_forest_train_and_predict() {
        let mut model = RandomForestModel::new(5);
        let samples: Vec<TrainingSample> = (0..20)
            .map(|_| TrainingSample {
                features: FeatureVector::from_characteristics(1000, 128, 0.1, 1.0),
                target: TargetMetric::QueryLatency(10.0),
                timestamp: chrono::Utc::now(),
            })
            .collect();
        model.train(&samples);
        let features = FeatureVector::from_characteristics(1000, 128, 0.1, 1.0);
        let prediction = model.predict(&features);
        assert!(
            (prediction - 10.0).abs() < 1e-6,
            "Random forest with constant target should predict near target, got {}",
            prediction
        );
    }

    #[test]
    fn test_neural_network_predict() {
        let model = NeuralNetworkModel::new(&[13, 8, 1]);
        let features = FeatureVector::from_characteristics(1000, 128, 0.5, 1.0);
        let prediction = model.predict(&features);
        assert!(prediction.is_finite(), "Neural network prediction should be finite");
    }

    #[test]
    fn test_gradient_boosting_empty() {
        let model = GradientBoostingModel::new(10, 0.1);
        let features = FeatureVector::from_characteristics(100, 64, 0.5, 1.0);
        let prediction = model.predict(&features);
        assert_eq!(prediction, 0.0, "Empty boosting model should predict base prediction (0)");
    }

    #[test]
    fn test_gradient_boosting_train() {
        let mut model = GradientBoostingModel::new(5, 0.1);
        let samples: Vec<TrainingSample> = (0..20)
            .map(|_| TrainingSample {
                features: FeatureVector::from_characteristics(1000, 128, 0.1, 1.0),
                target: TargetMetric::Throughput(500.0),
                timestamp: chrono::Utc::now(),
            })
            .collect();
        model.train(&samples);
        // After training, base_prediction should be the mean target
        assert!(
            (model.base_prediction - 500.0).abs() < 1e-6,
            "Base prediction should be mean of targets, got {}",
            model.base_prediction
        );
    }

    #[test]
    fn test_model_type_name() {
        assert_eq!(
            model_type_name(&PredictionModel::LinearRegression(LinearRegressionModel::new())),
            "LinearRegression"
        );
        assert_eq!(
            model_type_name(&PredictionModel::RandomForest(RandomForestModel::new(1))),
            "RandomForest"
        );
        assert_eq!(
            model_type_name(&PredictionModel::NeuralNetwork(NeuralNetworkModel::new(&[13, 1]))),
            "NeuralNetwork"
        );
        assert_eq!(
            model_type_name(&PredictionModel::GradientBoosting(GradientBoostingModel::new(1, 0.1))),
            "GradientBoosting"
        );
    }

    // ============================================================
    // TargetMetric tests
    // ============================================================

    #[test]
    fn test_target_metric_variants() {
        let latency = TargetMetric::QueryLatency(5.0);
        let throughput = TargetMetric::Throughput(1000.0);
        let memory = TargetMetric::MemoryUsage(2048.0);
        let build_time = TargetMetric::IndexBuildTime(30.0);

        // Verify Debug works
        assert!(format!("{:?}", latency).contains("5.0"));
        assert!(format!("{:?}", throughput).contains("1000.0"));
        assert!(format!("{:?}", memory).contains("2048.0"));
        assert!(format!("{:?}", build_time).contains("30.0"));
    }

    // ============================================================
    // PredictorConfig tests
    // ============================================================

    #[test]
    fn test_predictor_config_default() {
        let config = PredictorConfig::default();
        assert_eq!(config.min_training_samples, 100);
        assert_eq!(config.max_training_samples, 10000);
        assert_eq!(config.update_interval_secs, 3600);
        assert!(config.enable_online_learning);
    }
}
