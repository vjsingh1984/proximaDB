//! Learned fusion for the extracted query runtime.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use anyhow::{Result, anyhow};
use proximadb_data_model::DataModel;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, trace};

use crate::fusion::{SubQueryResult, aggregate_metrics, compute_rrf_scores, merge_record_data};
use crate::results::{QueryMetrics, QueryResult, UnifiedRecord};

fn lock_poisoned<T>(context: &str, err: PoisonError<T>) -> anyhow::Error {
    anyhow!("Lock poisoning in {}: {}", context, err)
}

/// Configuration for learned fusion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedFusionConfig {
    /// Model type to use.
    pub model_type: FusionModelType,
    /// Number of features to extract.
    pub num_features: usize,
    /// Learning rate for online updates.
    pub learning_rate: f64,
    /// Regularization strength (L2).
    pub regularization: f64,
    /// Whether to collect training data.
    pub collect_training_data: bool,
    /// Maximum training samples to keep.
    pub max_training_samples: usize,
    /// Minimum samples before model training.
    pub min_samples_for_training: usize,
    /// Whether to enable online learning.
    pub enable_online_learning: bool,
    /// Update frequency for online learning.
    pub online_update_frequency: usize,
}

impl Default for LearnedFusionConfig {
    fn default() -> Self {
        Self {
            model_type: FusionModelType::GradientBoosting,
            num_features: 32,
            learning_rate: 0.01,
            regularization: 0.001,
            collect_training_data: true,
            max_training_samples: 10000,
            min_samples_for_training: 100,
            enable_online_learning: true,
            online_update_frequency: 100,
        }
    }
}

/// Model type for learned fusion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FusionModelType {
    /// Linear model with learned weights.
    Linear,
    /// Gradient boosting (LightGBM-style).
    GradientBoosting,
    /// Simple neural network.
    NeuralNetwork { hidden_sizes: Vec<usize> },
    /// Ensemble of models.
    Ensemble,
}

/// Feature extraction for fusion learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionFeatures {
    /// Query-level features.
    pub query_features: Vec<f64>,
    /// Per-model result features.
    pub model_features: HashMap<DataModel, Vec<f64>>,
    /// Per-record features.
    pub record_features: HashMap<String, Vec<f64>>,
    /// Interaction features between models.
    pub interaction_features: Vec<f64>,
}

impl FusionFeatures {
    /// Create empty features with specified dimensions.
    pub fn new(num_features: usize) -> Self {
        Self {
            query_features: vec![0.0; num_features],
            model_features: HashMap::new(),
            record_features: HashMap::new(),
            interaction_features: vec![0.0; num_features],
        }
    }

    /// Convert to flat feature vector for model input.
    pub fn to_flat_vector(&self) -> Vec<f64> {
        let mut features = Vec::new();
        features.extend(&self.query_features);

        for model in [
            DataModel::Vector,
            DataModel::Document,
            DataModel::Graph,
            DataModel::Observability,
        ] {
            if let Some(mf) = self.model_features.get(&model) {
                features.extend(mf);
            } else {
                features.extend(vec![0.0; self.query_features.len()]);
            }
        }

        features.extend(&self.interaction_features);
        features
    }
}

/// Training sample for learned fusion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSample {
    /// Input features.
    pub features: FusionFeatures,
    /// Target scores for records.
    pub target_scores: HashMap<String, f64>,
    /// User feedback.
    pub feedback: Option<FeedbackSignal>,
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// User feedback signal for learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FeedbackSignal {
    /// User clicked on result.
    Click { record_id: String, position: usize },
    /// User rated the results.
    Rating { score: f64 },
    /// User refined query.
    QueryRefinement,
    /// Explicit relevance judgment.
    RelevanceJudgment { record_id: String, relevant: bool },
}

/// Learned fusion model abstraction.
pub trait FusionModel: Send + Sync {
    /// Predict fusion scores for records.
    fn predict(&self, features: &FusionFeatures, record_ids: &[String]) -> Result<Vec<f64>>;

    /// Update model with new training sample.
    fn update(&mut self, sample: &TrainingSample) -> Result<()>;

    /// Train model on batch of samples.
    fn train_batch(&mut self, samples: &[TrainingSample]) -> Result<TrainingMetrics>;

    /// Get model weights.
    fn get_weights(&self) -> Option<Vec<f64>>;

    /// Save model to bytes.
    fn save(&self) -> Result<Vec<u8>>;

    /// Load model from bytes.
    fn load(&mut self, data: &[u8]) -> Result<()>;
}

/// Training metrics.
#[derive(Debug, Clone, Default)]
pub struct TrainingMetrics {
    /// Number of samples used.
    pub num_samples: usize,
    /// Training loss.
    pub loss: f64,
    /// Validation loss.
    pub validation_loss: Option<f64>,
    /// Training time in milliseconds.
    pub training_time_ms: u64,
    /// Number of iterations.
    pub iterations: usize,
}

/// Linear fusion model.
pub struct LinearFusionModel {
    weights: Vec<f64>,
    bias: f64,
    learning_rate: f64,
    regularization: f64,
}

impl LinearFusionModel {
    pub fn new(num_features: usize, learning_rate: f64, regularization: f64) -> Self {
        let scale = (2.0 / num_features as f64).sqrt();
        let weights: Vec<f64> = (0..num_features)
            .map(|i| (i as f64 * 0.1).sin() * scale)
            .collect();

        Self {
            weights,
            bias: 0.0,
            learning_rate,
            regularization,
        }
    }

    fn forward(&self, features: &[f64]) -> f64 {
        let mut score = self.bias;
        for (w, f) in self.weights.iter().zip(features.iter()) {
            score += w * f;
        }
        1.0 / (1.0 + (-score).exp())
    }
}

impl FusionModel for LinearFusionModel {
    fn predict(&self, features: &FusionFeatures, record_ids: &[String]) -> Result<Vec<f64>> {
        let flat_features = features.to_flat_vector();
        let base_score = self.forward(&flat_features);

        let scores: Vec<f64> = record_ids
            .iter()
            .map(|id| {
                if let Some(record_features) = features.record_features.get(id) {
                    let record_score = self.forward(record_features);
                    (base_score + record_score) / 2.0
                } else {
                    base_score
                }
            })
            .collect();

        Ok(scores)
    }

    fn update(&mut self, sample: &TrainingSample) -> Result<()> {
        let flat_features = sample.features.to_flat_vector();

        if flat_features.len() > self.weights.len() {
            self.weights.resize(flat_features.len(), 0.0);
        }

        let predicted = self.forward(&flat_features);
        let target = if sample.target_scores.is_empty() {
            0.5
        } else {
            sample.target_scores.values().sum::<f64>() / sample.target_scores.len() as f64
        };

        let error = predicted - target;
        let gradient_scale = error * predicted * (1.0 - predicted);

        for (i, w) in self.weights.iter_mut().enumerate() {
            if i < flat_features.len() {
                let gradient = gradient_scale * flat_features[i] + self.regularization * *w;
                *w -= self.learning_rate * gradient;
            }
        }

        self.bias -= self.learning_rate * gradient_scale;
        Ok(())
    }

    fn train_batch(&mut self, samples: &[TrainingSample]) -> Result<TrainingMetrics> {
        let start = Instant::now();
        let mut total_loss = 0.0;

        for sample in samples {
            let flat_features = sample.features.to_flat_vector();

            if flat_features.len() > self.weights.len() {
                self.weights.resize(flat_features.len(), 0.0);
            }

            let predicted = self.forward(&flat_features);
            let target = if sample.target_scores.is_empty() {
                0.5
            } else {
                sample.target_scores.values().sum::<f64>() / sample.target_scores.len() as f64
            };

            let loss = -target * predicted.ln() - (1.0 - target) * (1.0 - predicted).ln();
            total_loss += loss;
            self.update(sample)?;
        }

        Ok(TrainingMetrics {
            num_samples: samples.len(),
            loss: total_loss / samples.len() as f64,
            validation_loss: None,
            training_time_ms: start.elapsed().as_millis() as u64,
            iterations: 1,
        })
    }

    fn get_weights(&self) -> Option<Vec<f64>> {
        Some(self.weights.clone())
    }

    fn save(&self) -> Result<Vec<u8>> {
        let data = bincode::serialize(&(
            &self.weights,
            self.bias,
            self.learning_rate,
            self.regularization,
        ))
        .map_err(|e| anyhow!("Failed to serialize model: {}", e))?;
        Ok(data)
    }

    fn load(&mut self, data: &[u8]) -> Result<()> {
        let (weights, bias, lr, reg): (Vec<f64>, f64, f64, f64) = bincode::deserialize(data)
            .map_err(|e| anyhow!("Failed to deserialize model: {}", e))?;
        self.weights = weights;
        self.bias = bias;
        self.learning_rate = lr;
        self.regularization = reg;
        Ok(())
    }
}

/// Gradient boosting fusion model.
pub struct GradientBoostingModel {
    trees: Vec<DecisionStump>,
    learning_rate: f64,
    max_trees: usize,
    feature_importance: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DecisionStump {
    feature_index: usize,
    threshold: f64,
    left_value: f64,
    right_value: f64,
    weight: f64,
}

impl DecisionStump {
    fn predict(&self, features: &[f64]) -> f64 {
        if features.get(self.feature_index).copied().unwrap_or(0.0) <= self.threshold {
            self.left_value * self.weight
        } else {
            self.right_value * self.weight
        }
    }
}

impl GradientBoostingModel {
    pub fn new(num_features: usize, learning_rate: f64, max_trees: usize) -> Self {
        Self {
            trees: Vec::new(),
            learning_rate,
            max_trees,
            feature_importance: vec![0.0; num_features],
        }
    }

    fn forward(&self, features: &[f64]) -> f64 {
        let mut score = 0.5;
        for tree in &self.trees {
            score += tree.predict(features);
        }
        score.clamp(0.0, 1.0)
    }

    fn fit_stump(&self, samples: &[(&[f64], f64)]) -> Option<DecisionStump> {
        if samples.is_empty() {
            return None;
        }

        let num_features = samples[0].0.len();
        if num_features == 0 {
            return None;
        }

        let mut best_stump: Option<DecisionStump> = None;
        let mut best_loss = f64::MAX;

        for feature_idx in 0..num_features {
            let mut values: Vec<(f64, f64)> = samples
                .iter()
                .map(|(f, t)| (f.get(feature_idx).copied().unwrap_or(0.0), *t))
                .collect();
            values.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            for (i, window) in values.windows(2).enumerate() {
                let threshold = (window[0].0 + window[1].0) / 2.0;

                let (left_sum, left_count) = values
                    .iter()
                    .take(i + 1)
                    .fold((0.0, 0), |(s, c), (_, t)| (s + t, c + 1));
                let (right_sum, right_count) = values
                    .iter()
                    .skip(i + 1)
                    .fold((0.0, 0), |(s, c), (_, t)| (s + t, c + 1));

                let left_value = if left_count > 0 {
                    left_sum / left_count as f64
                } else {
                    0.0
                };
                let right_value = if right_count > 0 {
                    right_sum / right_count as f64
                } else {
                    0.0
                };

                let mut loss = 0.0;
                for (f, target) in samples {
                    let pred = if f.get(feature_idx).copied().unwrap_or(0.0) <= threshold {
                        left_value
                    } else {
                        right_value
                    };
                    loss += (pred - target).powi(2);
                }

                if loss < best_loss {
                    best_loss = loss;
                    best_stump = Some(DecisionStump {
                        feature_index: feature_idx,
                        threshold,
                        left_value,
                        right_value,
                        weight: self.learning_rate,
                    });
                }
            }
        }

        best_stump
    }
}

impl FusionModel for GradientBoostingModel {
    fn predict(&self, features: &FusionFeatures, record_ids: &[String]) -> Result<Vec<f64>> {
        let flat_features = features.to_flat_vector();
        let base_score = self.forward(&flat_features);

        let scores: Vec<f64> = record_ids
            .iter()
            .map(|id| {
                if let Some(record_features) = features.record_features.get(id) {
                    let record_score = self.forward(record_features);
                    (base_score + record_score) / 2.0
                } else {
                    base_score
                }
            })
            .collect();

        Ok(scores)
    }

    fn update(&mut self, _sample: &TrainingSample) -> Result<()> {
        Ok(())
    }

    fn train_batch(&mut self, samples: &[TrainingSample]) -> Result<TrainingMetrics> {
        let start = Instant::now();

        if samples.is_empty() {
            return Ok(TrainingMetrics::default());
        }

        let feature_vecs: Vec<Vec<f64>> = samples
            .iter()
            .map(|s| s.features.to_flat_vector())
            .collect();

        let targets: Vec<f64> = samples
            .iter()
            .map(|s| {
                if s.target_scores.is_empty() {
                    0.5
                } else {
                    s.target_scores.values().sum::<f64>() / s.target_scores.len() as f64
                }
            })
            .collect();

        let mut residuals: Vec<f64> = targets
            .iter()
            .zip(feature_vecs.iter())
            .map(|(t, f)| t - self.forward(f))
            .collect();

        let mut iterations = 0;
        let mut total_loss = 0.0;

        while self.trees.len() < self.max_trees {
            let samples_with_residuals: Vec<(&[f64], f64)> = feature_vecs
                .iter()
                .zip(residuals.iter())
                .map(|(f, r)| (f.as_slice(), *r))
                .collect();

            if let Some(stump) = self.fit_stump(&samples_with_residuals) {
                if stump.feature_index < self.feature_importance.len() {
                    self.feature_importance[stump.feature_index] += 1.0;
                }

                for (i, features) in feature_vecs.iter().enumerate() {
                    residuals[i] -= stump.predict(features);
                }

                self.trees.push(stump);
                iterations += 1;

                let mse: f64 =
                    residuals.iter().map(|r| r.powi(2)).sum::<f64>() / residuals.len() as f64;
                total_loss = mse;

                if mse < 1e-6 {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(TrainingMetrics {
            num_samples: samples.len(),
            loss: total_loss,
            validation_loss: None,
            training_time_ms: start.elapsed().as_millis() as u64,
            iterations,
        })
    }

    fn get_weights(&self) -> Option<Vec<f64>> {
        let total: f64 = self.feature_importance.iter().sum();
        if total > 0.0 {
            Some(self.feature_importance.iter().map(|f| f / total).collect())
        } else {
            None
        }
    }

    fn save(&self) -> Result<Vec<u8>> {
        let data = bincode::serialize(&(
            &self.trees,
            self.learning_rate,
            self.max_trees,
            &self.feature_importance,
        ))
        .map_err(|e| anyhow!("Failed to serialize model: {}", e))?;
        Ok(data)
    }

    fn load(&mut self, data: &[u8]) -> Result<()> {
        let (trees, lr, max_trees, importance): (Vec<DecisionStump>, f64, usize, Vec<f64>) =
            bincode::deserialize(data)
                .map_err(|e| anyhow!("Failed to deserialize model: {}", e))?;
        self.trees = trees;
        self.learning_rate = lr;
        self.max_trees = max_trees;
        self.feature_importance = importance;
        Ok(())
    }
}

/// Learned fusion engine.
pub struct LearnedFusion {
    config: LearnedFusionConfig,
    model: Arc<RwLock<Box<dyn FusionModel>>>,
    training_buffer: Arc<RwLock<Vec<TrainingSample>>>,
    query_count: Arc<RwLock<usize>>,
    feature_extractor: FeatureExtractor,
    is_trained: Arc<RwLock<bool>>,
}

impl LearnedFusion {
    /// Create a new learned fusion engine.
    pub fn new(config: LearnedFusionConfig) -> Self {
        let model: Box<dyn FusionModel> = match &config.model_type {
            FusionModelType::Linear => Box::new(LinearFusionModel::new(
                config.num_features * 5,
                config.learning_rate,
                config.regularization,
            )),
            FusionModelType::GradientBoosting => Box::new(GradientBoostingModel::new(
                config.num_features * 5,
                config.learning_rate,
                100,
            )),
            FusionModelType::NeuralNetwork { .. } => Box::new(LinearFusionModel::new(
                config.num_features * 5,
                config.learning_rate,
                config.regularization,
            )),
            FusionModelType::Ensemble => Box::new(GradientBoostingModel::new(
                config.num_features * 5,
                config.learning_rate,
                100,
            )),
        };

        Self {
            config: config.clone(),
            model: Arc::new(RwLock::new(model)),
            training_buffer: Arc::new(RwLock::new(Vec::new())),
            query_count: Arc::new(RwLock::new(0)),
            feature_extractor: FeatureExtractor::new(config.num_features),
            is_trained: Arc::new(RwLock::new(false)),
        }
    }

    /// Fuse results using the learned model.
    pub fn fuse(&self, sub_results: Vec<SubQueryResult>) -> Result<QueryResult> {
        if sub_results.is_empty() {
            return Ok(QueryResult {
                records: Vec::new(),
                total_count: Some(0),
                metrics: QueryMetrics::default(),
            });
        }

        if sub_results.len() == 1 {
            let result = sub_results
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("Expected single result but found none"))?;
            return Ok(self.convert_single_result(result));
        }

        let features = self.feature_extractor.extract(&sub_results);

        let mut all_records: HashMap<String, UnifiedRecord> = HashMap::new();
        for result in &sub_results {
            for record in &result.records {
                all_records
                    .entry(record.id.clone())
                    .and_modify(|existing| {
                        self.merge_record_data(existing, record);
                    })
                    .or_insert_with(|| record.clone());
            }
        }

        let record_ids: Vec<String> = all_records.keys().cloned().collect();

        let is_trained = *self
            .is_trained
            .read()
            .map_err(|e| lock_poisoned("is_trained", e))?;
        let scores = if is_trained {
            let model = self.model.read().map_err(|e| lock_poisoned("model", e))?;
            model.predict(&features, &record_ids)?
        } else {
            self.fallback_rrf_scores(&sub_results, &record_ids)
        };

        let mut records: Vec<UnifiedRecord> = record_ids
            .into_iter()
            .zip(scores.into_iter())
            .map(|(id, score)| {
                let mut record = all_records
                    .remove(&id)
                    .ok_or_else(|| anyhow!("Record ID {} not found in all_records", id))?;
                record.score = Some(score);
                Ok(record)
            })
            .collect::<Result<Vec<_>>>()?;

        records.sort_by(|a, b| {
            b.score
                .unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let metrics = self.aggregate_metrics(&sub_results);

        if self.config.enable_online_learning {
            let mut count = self
                .query_count
                .write()
                .map_err(|e| lock_poisoned("query_count", e))?;
            *count += 1;

            if *count % self.config.online_update_frequency == 0 {
                drop(count);
                self.maybe_train()?;
            }
        }

        debug!("Learned fusion produced {} records", records.len());

        Ok(QueryResult {
            records,
            total_count: None,
            metrics,
        })
    }

    /// Add training sample from user feedback.
    pub fn add_training_sample(&self, sample: TrainingSample) -> Result<()> {
        if !self.config.collect_training_data {
            return Ok(());
        }

        let mut buffer = self
            .training_buffer
            .write()
            .map_err(|e| lock_poisoned("training_buffer", e))?;

        if buffer.len() >= self.config.max_training_samples {
            buffer.remove(0);
        }

        buffer.push(sample);
        trace!("Added training sample, buffer size: {}", buffer.len());
        Ok(())
    }

    /// Record user feedback for learning.
    pub fn record_feedback(
        &self,
        features: FusionFeatures,
        feedback: FeedbackSignal,
    ) -> Result<()> {
        let target_scores = match &feedback {
            FeedbackSignal::Click {
                record_id,
                position,
            } => {
                let mut scores = HashMap::new();
                scores.insert(record_id.clone(), 1.0 / (*position as f64 + 1.0));
                scores
            }
            FeedbackSignal::Rating { score: _ } => HashMap::new(),
            FeedbackSignal::QueryRefinement => HashMap::new(),
            FeedbackSignal::RelevanceJudgment {
                record_id,
                relevant,
            } => {
                let mut scores = HashMap::new();
                scores.insert(record_id.clone(), if *relevant { 1.0 } else { 0.0 });
                scores
            }
        };

        let sample = TrainingSample {
            features,
            target_scores,
            feedback: Some(feedback),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| anyhow!("SystemTime error: {}", e))?
                .as_millis() as u64,
        };

        self.add_training_sample(sample)
    }

    /// Train the model on accumulated samples.
    pub fn train(&self) -> Result<TrainingMetrics> {
        let buffer = self
            .training_buffer
            .read()
            .map_err(|e| lock_poisoned("training_buffer", e))?;

        if buffer.len() < self.config.min_samples_for_training {
            return Err(anyhow!(
                "Not enough training samples: {} < {}",
                buffer.len(),
                self.config.min_samples_for_training
            ));
        }

        let samples: Vec<TrainingSample> = buffer.clone();
        drop(buffer);

        let mut model = self.model.write().map_err(|e| lock_poisoned("model", e))?;
        let metrics = model.train_batch(&samples)?;

        *self
            .is_trained
            .write()
            .map_err(|e| lock_poisoned("is_trained", e))? = true;

        info!(
            "Trained fusion model on {} samples, loss: {:.4}",
            metrics.num_samples, metrics.loss
        );

        Ok(metrics)
    }

    fn maybe_train(&self) -> Result<()> {
        let buffer = self
            .training_buffer
            .read()
            .map_err(|e| lock_poisoned("training_buffer", e))?;
        if buffer.len() >= self.config.min_samples_for_training {
            drop(buffer);
            let _ = self.train();
        }
        Ok(())
    }

    /// Get feature importance from the model.
    pub fn get_feature_importance(&self) -> Option<Vec<f64>> {
        let model = self.model.read().ok()?;
        model.get_weights()
    }

    /// Save model state.
    pub fn save_model(&self) -> Result<Vec<u8>> {
        let model = self.model.read().map_err(|e| lock_poisoned("model", e))?;
        model.save()
    }

    /// Load model state.
    pub fn load_model(&self, data: &[u8]) -> Result<()> {
        let mut model = self.model.write().map_err(|e| lock_poisoned("model", e))?;
        model.load(data)?;
        *self
            .is_trained
            .write()
            .map_err(|e| lock_poisoned("is_trained", e))? = true;
        Ok(())
    }

    /// Get training buffer size.
    pub fn training_buffer_size(&self) -> usize {
        self.training_buffer
            .read()
            .map_err(|e| lock_poisoned("training_buffer", e))
            .ok()
            .map_or(0, |b| b.len())
    }

    /// Check if model is trained.
    pub fn is_trained(&self) -> bool {
        self.is_trained.read().ok().is_some_and(|t| *t)
    }

    fn fallback_rrf_scores(
        &self,
        sub_results: &[SubQueryResult],
        record_ids: &[String],
    ) -> Vec<f64> {
        compute_rrf_scores(sub_results, record_ids, 60)
    }

    fn convert_single_result(&self, result: SubQueryResult) -> QueryResult {
        QueryResult {
            records: result.records,
            total_count: result.total_count,
            metrics: QueryMetrics {
                total_time_us: result.execution_time_us,
                sub_query_times: vec![(result.source_model, result.execution_time_us)],
                records_scanned: result.records_scanned,
                records_returned: result.records_returned,
                cache_hit_rate: 0.0,
            },
        }
    }

    fn merge_record_data(&self, target: &mut UnifiedRecord, source: &UnifiedRecord) {
        merge_record_data(target, source);
    }

    fn aggregate_metrics(&self, sub_results: &[SubQueryResult]) -> QueryMetrics {
        aggregate_metrics(sub_results)
    }
}

/// Feature extractor for fusion learning.
pub struct FeatureExtractor {
    num_features: usize,
}

impl FeatureExtractor {
    pub fn new(num_features: usize) -> Self {
        Self { num_features }
    }

    /// Extract features from sub-query results.
    pub fn extract(&self, sub_results: &[SubQueryResult]) -> FusionFeatures {
        let mut features = FusionFeatures::new(self.num_features);
        features.query_features = self.extract_query_features(sub_results);

        for result in sub_results {
            let model_features = self.extract_model_features(result);
            features
                .model_features
                .insert(result.source_model, model_features);
        }

        features.record_features = self.extract_record_features(sub_results);
        features.interaction_features = self.extract_interaction_features(sub_results);
        features
    }

    fn extract_query_features(&self, sub_results: &[SubQueryResult]) -> Vec<f64> {
        let mut features = vec![0.0; self.num_features];

        features[0] = sub_results.len() as f64 / 4.0;

        let total_results: usize = sub_results.iter().map(|r| r.records.len()).sum();
        features[1] = (total_results as f64).ln().max(0.0) / 10.0;

        if !sub_results.is_empty() {
            features[2] = (total_results as f64 / sub_results.len() as f64) / 100.0;
        }

        let all_scores: Vec<f64> = sub_results
            .iter()
            .flat_map(|r| r.records.iter())
            .filter_map(|rec| rec.score)
            .collect();

        if !all_scores.is_empty() {
            let mean = all_scores.iter().sum::<f64>() / all_scores.len() as f64;
            let variance = all_scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>()
                / all_scores.len() as f64;
            let std_dev = variance.sqrt();

            features[3] = mean;
            features[4] = std_dev;
            features[5] = *all_scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0);
            features[6] = *all_scores
                .iter()
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0);
        }

        let total_time: u64 = sub_results.iter().map(|r| r.execution_time_us).sum();
        features[7] = (total_time as f64).ln().max(0.0) / 20.0;
        features
    }

    fn extract_model_features(&self, result: &SubQueryResult) -> Vec<f64> {
        let mut features = vec![0.0; self.num_features];

        features[0] = (result.records.len() as f64).ln().max(0.0) / 10.0;

        let scores: Vec<f64> = result.records.iter().filter_map(|r| r.score).collect();
        if !scores.is_empty() {
            let mean = scores.iter().sum::<f64>() / scores.len() as f64;
            let variance =
                scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / scores.len() as f64;

            features[1] = mean;
            features[2] = variance.sqrt();
            features[3] = *scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0);
            features[4] = *scores
                .iter()
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(&0.0);
        }

        features[5] = (result.execution_time_us as f64).ln().max(0.0) / 20.0;

        if result.records_scanned > 0 {
            features[6] = result.records_returned as f64 / result.records_scanned as f64;
        }

        features[7] = match result.source_model {
            DataModel::Vector => 0.25,
            DataModel::Document => 0.50,
            DataModel::Graph => 0.75,
            DataModel::Observability
            | DataModel::TimeSeries
            | DataModel::Relational
            | DataModel::Event => 1.0,
        };

        features
    }

    fn extract_record_features(&self, sub_results: &[SubQueryResult]) -> HashMap<String, Vec<f64>> {
        let mut record_features = HashMap::new();

        for result in sub_results {
            for (rank, record) in result.records.iter().enumerate() {
                let features = record_features
                    .entry(record.id.clone())
                    .or_insert_with(|| vec![0.0; self.num_features]);

                features[0] += 1.0 / (rank as f64 + 1.0);
                if let Some(score) = record.score {
                    features[1] += score;
                }
                features[2] += 1.0;

                let model_idx = match result.source_model {
                    DataModel::Vector => 3,
                    DataModel::Document => 4,
                    DataModel::Graph => 5,
                    DataModel::Observability
                    | DataModel::TimeSeries
                    | DataModel::Relational
                    | DataModel::Event => 6,
                };
                if model_idx < self.num_features {
                    features[model_idx] = 1.0;
                }
            }
        }

        for features in record_features.values_mut() {
            let appearances = features[2].max(1.0);
            features[0] /= appearances;
            features[1] /= appearances;
            features[2] /= sub_results.len() as f64;
        }

        record_features
    }

    fn extract_interaction_features(&self, sub_results: &[SubQueryResult]) -> Vec<f64> {
        let mut features = vec![0.0; self.num_features];

        let mut id_sets: Vec<HashSet<String>> = Vec::new();
        for result in sub_results {
            let ids: HashSet<String> = result.records.iter().map(|r| r.id.clone()).collect();
            id_sets.push(ids);
        }

        let mut overlap_sum = 0.0;
        let mut pair_count = 0;

        for (i, id_set_i) in id_sets.iter().enumerate() {
            for id_set_j in id_sets.iter().skip(i + 1) {
                let intersection: HashSet<_> = id_set_i.intersection(id_set_j).collect();
                let union: HashSet<_> = id_set_i.union(id_set_j).collect();

                if !union.is_empty() {
                    overlap_sum += intersection.len() as f64 / union.len() as f64;
                }
                pair_count += 1;
            }
        }

        features[0] = if pair_count > 0 {
            overlap_sum / pair_count as f64
        } else {
            0.0
        };

        if sub_results.len() >= 2 {
            let scores1: HashMap<String, f64> = sub_results[0]
                .records
                .iter()
                .filter_map(|r| r.score.map(|s| (r.id.clone(), s)))
                .collect();
            let scores2: HashMap<String, f64> = sub_results[1]
                .records
                .iter()
                .filter_map(|r| r.score.map(|s| (r.id.clone(), s)))
                .collect();

            let common_ids: Vec<&String> = scores1
                .keys()
                .filter(|id| scores2.contains_key(*id))
                .collect();

            if common_ids.len() >= 2 {
                let mean1: f64 = common_ids
                    .iter()
                    .filter_map(|id| scores1.get(*id))
                    .sum::<f64>()
                    / common_ids.len() as f64;
                let mean2: f64 = common_ids
                    .iter()
                    .filter_map(|id| scores2.get(*id))
                    .sum::<f64>()
                    / common_ids.len() as f64;

                let mut cov = 0.0;
                let mut var1 = 0.0;
                let mut var2 = 0.0;

                for id in &common_ids {
                    if let (Some(&s1), Some(&s2)) = (scores1.get(*id), scores2.get(*id)) {
                        cov += (s1 - mean1) * (s2 - mean2);
                        var1 += (s1 - mean1).powi(2);
                        var2 += (s2 - mean2).powi(2);
                    }
                }

                if var1 > 0.0 && var2 > 0.0 {
                    features[1] = cov / (var1.sqrt() * var2.sqrt());
                }
            }
        }

        features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, score: f64, model: DataModel) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: model,
            data: serde_json::json!({"id": id}),
            score: Some(score),
            metadata: HashMap::new(),
        }
    }

    fn make_sub_result(model: DataModel, records: Vec<UnifiedRecord>) -> SubQueryResult {
        SubQueryResult {
            source_model: model,
            records_returned: records.len() as u64,
            records,
            total_count: None,
            execution_time_us: 100,
            records_scanned: 100,
        }
    }

    #[test]
    fn test_linear_model_creation() {
        let model = LinearFusionModel::new(32, 0.01, 0.001);
        assert_eq!(model.weights.len(), 32);
    }

    #[test]
    fn test_linear_model_predict() {
        let model = LinearFusionModel::new(32, 0.01, 0.001);
        let features = FusionFeatures::new(32);
        let scores = model
            .predict(&features, &["a".to_string(), "b".to_string()])
            .expect("Failed to predict scores");

        assert_eq!(scores.len(), 2);
        for score in scores {
            assert!((0.0..=1.0).contains(&score));
        }
    }

    #[test]
    fn test_feature_extraction() {
        let extractor = FeatureExtractor::new(32);

        let result1 = make_sub_result(
            DataModel::Vector,
            vec![
                make_record("a", 0.9, DataModel::Vector),
                make_record("b", 0.8, DataModel::Vector),
            ],
        );

        let result2 = make_sub_result(
            DataModel::Document,
            vec![
                make_record("b", 0.85, DataModel::Document),
                make_record("c", 0.75, DataModel::Document),
            ],
        );

        let features = extractor.extract(&[result1, result2]);

        assert_eq!(features.query_features.len(), 32);
        assert!(features.model_features.contains_key(&DataModel::Vector));
        assert!(features.model_features.contains_key(&DataModel::Document));
        assert!(features.record_features.contains_key("a"));
        assert!(features.record_features.contains_key("b"));
        assert!(features.record_features.contains_key("c"));
    }

    #[test]
    fn test_learned_fusion_fallback() {
        let config = LearnedFusionConfig::default();
        let fusion = LearnedFusion::new(config);

        let result1 = make_sub_result(
            DataModel::Vector,
            vec![
                make_record("a", 0.9, DataModel::Vector),
                make_record("b", 0.8, DataModel::Vector),
            ],
        );

        let result2 = make_sub_result(
            DataModel::Document,
            vec![
                make_record("b", 0.85, DataModel::Document),
                make_record("c", 0.75, DataModel::Document),
            ],
        );

        let fused = fusion
            .fuse(vec![result1, result2])
            .expect("Failed to fuse results");

        assert_eq!(fused.records.len(), 3);
        let b_record = fused
            .records
            .iter()
            .find(|r| r.id == "b")
            .expect("Record 'b' should be present in fused results");
        let a_record = fused
            .records
            .iter()
            .find(|r| r.id == "a")
            .expect("Record 'a' should be present in fused results");
        assert!(b_record.score > a_record.score);
    }

    #[test]
    fn test_training_sample_collection() {
        let config = LearnedFusionConfig {
            collect_training_data: true,
            max_training_samples: 100,
            ..Default::default()
        };
        let fusion = LearnedFusion::new(config);

        let features = FusionFeatures::new(32);
        let mut target_scores = HashMap::new();
        target_scores.insert("a".to_string(), 1.0);

        let sample = TrainingSample {
            features,
            target_scores,
            feedback: None,
            timestamp_ms: 0,
        };

        fusion
            .add_training_sample(sample)
            .expect("Failed to add training sample");

        assert_eq!(fusion.training_buffer_size(), 1);
    }

    #[test]
    fn test_feedback_recording() {
        let config = LearnedFusionConfig::default();
        let fusion = LearnedFusion::new(config);

        let features = FusionFeatures::new(32);
        let feedback = FeedbackSignal::Click {
            record_id: "a".to_string(),
            position: 0,
        };

        fusion
            .record_feedback(features, feedback)
            .expect("Failed to record feedback");

        assert_eq!(fusion.training_buffer_size(), 1);
    }

    #[test]
    fn test_model_save_load() {
        let config = LearnedFusionConfig {
            model_type: FusionModelType::Linear,
            ..Default::default()
        };
        let fusion = LearnedFusion::new(config.clone());

        let data = fusion.save_model().expect("Failed to save model");

        let fusion2 = LearnedFusion::new(config);
        fusion2.load_model(&data).expect("Failed to load model");

        assert!(fusion2.is_trained());
    }

    #[test]
    fn test_linear_model_training() {
        let mut model = LinearFusionModel::new(160, 0.1, 0.001);
        let mut samples = Vec::new();

        for i in 0..10 {
            let mut features = FusionFeatures::new(32);
            features.query_features[0] = i as f64 / 10.0;
            let mut target_scores = HashMap::new();
            target_scores.insert("a".to_string(), i as f64 / 10.0);
            samples.push(TrainingSample {
                features,
                target_scores,
                feedback: None,
                timestamp_ms: i,
            });
        }

        let metrics = model
            .train_batch(&samples)
            .expect("Training should succeed");
        assert_eq!(metrics.num_samples, 10);
    }
}
