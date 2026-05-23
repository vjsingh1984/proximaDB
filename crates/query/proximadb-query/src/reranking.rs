//! Cross-modal reranking for the extracted query runtime.
//!
//! Advanced reranking strategies for multi-modal query results that consider:
//! - Semantic similarity across modalities
//! - Context-aware scoring based on query intent
//! - Diversity optimization for varied results
//! - Explanation generation for transparency

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use proximadb_data_model::DataModel;
// ScoreComponent now lives in proximadb-kernel so that both the root crate
// (src/core/search/results.rs) and this crate share the canonical definition.
// See roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md (R-0).
pub use proximadb_kernel::ScoreComponent;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use crate::results::{QueryResult, UnifiedRecord};

/// Reranking strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RerankConfig {
    /// Enable cross-modal reranking. Defaults to false so production ranking is
    /// not changed by heuristic policy unless explicitly configured.
    #[serde(default)]
    pub enabled: bool,
    /// Enable cross-modal semantic similarity reranking.
    #[serde(default)]
    pub semantic_rerank: bool,
    /// Enable diversity optimization.
    #[serde(default)]
    pub diversity_optimization: bool,
    /// Diversity weight (0.0 to 1.0).
    #[serde(default)]
    pub diversity_weight: f64,
    /// Maximum marginal relevance lambda (0.0 = max diversity, 1.0 = max relevance).
    #[serde(default)]
    pub mmr_lambda: f64,
    /// Enable context-aware scoring.
    #[serde(default)]
    pub context_aware: bool,
    /// Model weights per data model for final scoring.
    #[serde(default)]
    pub model_weights: ModelWeightConfig,
    /// Policy for records that do not carry a score.
    #[serde(default)]
    pub missing_score: MissingScorePolicy,
    /// Score to use when `missing_score = "configured"`.
    #[serde(default)]
    pub configured_missing_score: Option<f64>,
    /// Score to use when semantic rerank is enabled and no semantic signal exists.
    #[serde(default)]
    pub missing_semantic_score: f64,
    /// Existing rank score weight when semantic reranking is enabled.
    #[serde(default = "default_semantic_base_weight")]
    pub semantic_base_weight: f64,
    /// Semantic signal weight when semantic reranking is enabled.
    #[serde(default = "default_semantic_signal_weight")]
    pub semantic_signal_weight: f64,
    /// Query-intent boost policy.
    #[serde(default)]
    pub intent_boosts: IntentBoostConfig,
    /// Minimum base score for navigational high-confidence boosts.
    #[serde(default = "default_navigational_min_base_score")]
    pub navigational_min_base_score: f64,
    /// Temporal boost policy.
    #[serde(default)]
    pub temporal: TemporalBoostConfig,
    /// Similarity policy used by MMR diversity.
    #[serde(default)]
    pub diversity_similarity: DiversitySimilarityConfig,
    /// Generate explanations for reranking decisions.
    #[serde(default)]
    pub generate_explanations: bool,
    /// Weight of model diversity in aggregate diversity scoring.
    #[serde(default = "default_diversity_score_weight")]
    pub diversity_model_weight: f64,
    /// Weight of pairwise dissimilarity in aggregate diversity scoring.
    #[serde(default = "default_diversity_score_weight")]
    pub diversity_dissimilarity_weight: f64,
    /// Confidence used when no explanation components exist.
    #[serde(default = "default_explanation_empty_confidence")]
    pub explanation_empty_confidence: f64,
    /// Top-k results to consider for reranking.
    #[serde(default = "default_rerank_top_k")]
    pub rerank_top_k: usize,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            semantic_rerank: false,
            diversity_optimization: false,
            diversity_weight: 0.0,
            mmr_lambda: 1.0,
            context_aware: false,
            model_weights: ModelWeightConfig::default(),
            missing_score: MissingScorePolicy::Zero,
            configured_missing_score: None,
            missing_semantic_score: 0.0,
            semantic_base_weight: default_semantic_base_weight(),
            semantic_signal_weight: default_semantic_signal_weight(),
            intent_boosts: IntentBoostConfig::default(),
            navigational_min_base_score: default_navigational_min_base_score(),
            temporal: TemporalBoostConfig::default(),
            diversity_similarity: DiversitySimilarityConfig::default(),
            generate_explanations: false,
            diversity_model_weight: default_diversity_score_weight(),
            diversity_dissimilarity_weight: default_diversity_score_weight(),
            explanation_empty_confidence: default_explanation_empty_confidence(),
            rerank_top_k: default_rerank_top_k(),
        }
    }
}

fn default_rerank_top_k() -> usize {
    100
}

fn default_semantic_base_weight() -> f64 {
    0.7
}

fn default_semantic_signal_weight() -> f64 {
    0.3
}

fn default_navigational_min_base_score() -> f64 {
    0.9
}

fn default_diversity_score_weight() -> f64 {
    0.5
}

fn default_explanation_empty_confidence() -> f64 {
    0.5
}

/// Data-model weights for base score normalization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelWeightConfig {
    #[serde(default = "default_weight_one")]
    pub vector: f64,
    #[serde(default = "default_weight_one")]
    pub document: f64,
    #[serde(default = "default_weight_one")]
    pub graph: f64,
    #[serde(default = "default_weight_one")]
    pub observability: f64,
}

impl Default for ModelWeightConfig {
    fn default() -> Self {
        Self {
            vector: 1.0,
            document: 1.0,
            graph: 1.0,
            observability: 1.0,
        }
    }
}

impl ModelWeightConfig {
    fn weight_for(self, model: DataModel) -> f64 {
        match model {
            DataModel::Vector => self.vector,
            DataModel::Document => self.document,
            DataModel::Graph => self.graph,
            DataModel::Observability => self.observability,
            _ => 1.0,
        }
    }
}

fn default_weight_one() -> f64 {
    1.0
}

/// Policy for records without engine-provided scores.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingScorePolicy {
    /// Use 0.0 and make the absence explicit rather than inventing a neutral score.
    #[default]
    Zero,
    /// Preserve input order for missing-score records by assigning no positive contribution.
    Preserve,
    /// Use `configured_missing_score`.
    Configured,
}

/// Query-intent boosts. Defaults are neutral.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct IntentBoostConfig {
    #[serde(default)]
    pub similarity_vector: f64,
    #[serde(default)]
    pub relationship_graph: f64,
    #[serde(default)]
    pub navigational_high_confidence: f64,
    #[serde(default)]
    pub informational: f64,
    #[serde(default)]
    pub analytical_observability: f64,
    #[serde(default)]
    pub required_model: f64,
}

/// Temporal boost policy. Defaults are neutral.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TemporalBoostConfig {
    #[serde(default)]
    pub recent_weight: f64,
    #[serde(default)]
    pub historical_weight: f64,
    #[serde(default = "default_recent_half_life_hours")]
    pub recent_half_life_hours: f64,
    #[serde(default = "default_historical_half_life_hours")]
    pub historical_half_life_hours: f64,
}

impl Default for TemporalBoostConfig {
    fn default() -> Self {
        Self {
            recent_weight: 0.0,
            historical_weight: 0.0,
            recent_half_life_hours: default_recent_half_life_hours(),
            historical_half_life_hours: default_historical_half_life_hours(),
        }
    }
}

fn default_recent_half_life_hours() -> f64 {
    24.0
}

fn default_historical_half_life_hours() -> f64 {
    720.0
}

/// Similarity components used by MMR diversity. Defaults are neutral.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct DiversitySimilarityConfig {
    #[serde(default)]
    pub same_model_weight: f64,
    #[serde(default)]
    pub metadata_overlap_weight: f64,
    #[serde(default)]
    pub score_proximity_weight: f64,
}

impl RerankConfig {
    pub fn validate(&self) -> Result<()> {
        if self.rerank_top_k == 0 {
            anyhow::bail!("query.reranking.rerank_top_k must be greater than 0");
        }
        validate_unit_interval("query.reranking.diversity_weight", self.diversity_weight)?;
        validate_unit_interval("query.reranking.mmr_lambda", self.mmr_lambda)?;
        validate_unit_interval(
            "query.reranking.missing_semantic_score",
            self.missing_semantic_score,
        )?;
        validate_unit_interval(
            "query.reranking.semantic_base_weight",
            self.semantic_base_weight,
        )?;
        validate_unit_interval(
            "query.reranking.semantic_signal_weight",
            self.semantic_signal_weight,
        )?;
        validate_unit_interval(
            "query.reranking.navigational_min_base_score",
            self.navigational_min_base_score,
        )?;
        validate_unit_interval(
            "query.reranking.diversity_model_weight",
            self.diversity_model_weight,
        )?;
        validate_unit_interval(
            "query.reranking.diversity_dissimilarity_weight",
            self.diversity_dissimilarity_weight,
        )?;
        validate_unit_interval(
            "query.reranking.explanation_empty_confidence",
            self.explanation_empty_confidence,
        )?;
        if self.semantic_rerank && self.semantic_base_weight + self.semantic_signal_weight <= 0.0 {
            anyhow::bail!(
                "query.reranking semantic weights must have positive total when semantic_rerank is enabled"
            );
        }
        if self.missing_score == MissingScorePolicy::Configured {
            match self.configured_missing_score {
                Some(score) => {
                    validate_unit_interval("query.reranking.configured_missing_score", score)?
                }
                None => anyhow::bail!(
                    "query.reranking.configured_missing_score is required when missing_score = \"configured\""
                ),
            }
        }

        for (name, value) in [
            (
                "query.reranking.model_weights.vector",
                self.model_weights.vector,
            ),
            (
                "query.reranking.model_weights.document",
                self.model_weights.document,
            ),
            (
                "query.reranking.model_weights.graph",
                self.model_weights.graph,
            ),
            (
                "query.reranking.model_weights.observability",
                self.model_weights.observability,
            ),
            (
                "query.reranking.intent_boosts.similarity_vector",
                self.intent_boosts.similarity_vector,
            ),
            (
                "query.reranking.intent_boosts.relationship_graph",
                self.intent_boosts.relationship_graph,
            ),
            (
                "query.reranking.intent_boosts.navigational_high_confidence",
                self.intent_boosts.navigational_high_confidence,
            ),
            (
                "query.reranking.intent_boosts.informational",
                self.intent_boosts.informational,
            ),
            (
                "query.reranking.intent_boosts.analytical_observability",
                self.intent_boosts.analytical_observability,
            ),
            (
                "query.reranking.intent_boosts.required_model",
                self.intent_boosts.required_model,
            ),
            (
                "query.reranking.temporal.recent_weight",
                self.temporal.recent_weight,
            ),
            (
                "query.reranking.temporal.historical_weight",
                self.temporal.historical_weight,
            ),
            (
                "query.reranking.diversity_similarity.same_model_weight",
                self.diversity_similarity.same_model_weight,
            ),
            (
                "query.reranking.diversity_similarity.metadata_overlap_weight",
                self.diversity_similarity.metadata_overlap_weight,
            ),
            (
                "query.reranking.diversity_similarity.score_proximity_weight",
                self.diversity_similarity.score_proximity_weight,
            ),
        ] {
            validate_non_negative(name, value)?;
        }

        validate_positive(
            "query.reranking.temporal.recent_half_life_hours",
            self.temporal.recent_half_life_hours,
        )?;
        validate_positive(
            "query.reranking.temporal.historical_half_life_hours",
            self.temporal.historical_half_life_hours,
        )?;
        Ok(())
    }
}

fn validate_unit_interval(name: &str, value: f64) -> Result<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        anyhow::bail!("{name} must be finite and in [0.0, 1.0], got {value}")
    }
}

fn validate_non_negative(name: &str, value: f64) -> Result<()> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        anyhow::bail!("{name} must be finite and non-negative, got {value}")
    }
}

fn validate_positive(name: &str, value: f64) -> Result<()> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        anyhow::bail!("{name} must be finite and positive, got {value}")
    }
}

/// Query context for context-aware reranking.
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Original query text (if available).
    pub query_text: Option<String>,
    /// Query embedding (if available).
    pub query_embedding: Option<Vec<f32>>,
    /// User preferences/history (optional).
    pub user_preferences: HashMap<String, f64>,
    /// Intent classification.
    pub intent: Option<QueryIntent>,
    /// Temporal context (recency preference).
    pub temporal_preference: TemporalPreference,
    /// Required data models (user explicitly requested these).
    pub required_models: HashSet<DataModel>,
}

impl Default for QueryContext {
    fn default() -> Self {
        Self {
            query_text: None,
            query_embedding: None,
            user_preferences: HashMap::new(),
            intent: None,
            temporal_preference: TemporalPreference::Neutral,
            required_models: HashSet::new(),
        }
    }
}

/// Query intent classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryIntent {
    /// Looking for a specific item.
    Navigational,
    /// Seeking information/exploration.
    Informational,
    /// Action-oriented.
    Transactional,
    /// Similarity search.
    SimilaritySearch,
    /// Relationship exploration.
    RelationshipExploration,
    /// Analytics/aggregation.
    Analytical,
}

/// Temporal preference for recency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TemporalPreference {
    /// Prefer recent results.
    Recent,
    /// Prefer older/established results.
    Historical,
    /// No temporal preference.
    Neutral,
    /// Custom decay function.
    Custom { half_life_hours: f64 },
}

/// Explanation for a reranking decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankExplanation {
    /// Record ID.
    pub record_id: String,
    /// Original rank before reranking.
    pub original_rank: usize,
    /// New rank after reranking.
    pub new_rank: usize,
    /// Score components.
    pub score_components: Vec<ScoreComponent>,
    /// Human-readable explanation.
    pub explanation_text: String,
    /// Confidence in the reranking.
    pub confidence: f64,
}

/// Reranked result with explanations.
#[derive(Debug, Clone)]
pub struct RerankedResult {
    /// Reranked records.
    pub records: Vec<UnifiedRecord>,
    /// Explanations per record (if enabled).
    pub explanations: Vec<RerankExplanation>,
    /// Overall reranking quality score.
    pub quality_score: f64,
    /// Diversity score of the result set.
    pub diversity_score: f64,
}

/// Cross-modal reranker.
pub struct CrossModalReranker {
    config: RerankConfig,
}

impl CrossModalReranker {
    /// Create a new cross-modal reranker.
    pub fn new(config: RerankConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration.
    pub fn default_reranker() -> Self {
        Self::new(RerankConfig::default())
    }

    /// Rerank query results using cross-modal signals.
    pub fn rerank(&self, result: QueryResult, context: &QueryContext) -> Result<RerankedResult> {
        if result.records.is_empty() {
            return Ok(RerankedResult {
                records: vec![],
                explanations: vec![],
                quality_score: 1.0,
                diversity_score: 1.0,
            });
        }

        self.config.validate()?;

        if !self.config.enabled {
            let records = result.records;
            return Ok(RerankedResult {
                quality_score: self.compute_quality_score(&records),
                diversity_score: self.compute_diversity_score(&records),
                records,
                explanations: vec![],
            });
        }

        let records_to_rerank: Vec<UnifiedRecord> = result
            .records
            .into_iter()
            .take(self.config.rerank_top_k)
            .collect();

        debug!("Reranking {} records", records_to_rerank.len());

        let mut scored_records = self.compute_base_scores(&records_to_rerank, context)?;

        if self.config.semantic_rerank {
            scored_records = self.apply_semantic_reranking(scored_records, context)?;
        }

        if self.config.context_aware {
            scored_records = self.apply_context_aware_scoring(scored_records, context)?;
        }

        if self.config.diversity_optimization {
            scored_records = self.apply_mmr_diversity(scored_records)?;
        }

        let explanations = if self.config.generate_explanations {
            self.generate_explanations(&records_to_rerank, &scored_records)?
        } else {
            vec![]
        };

        scored_records.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let records: Vec<UnifiedRecord> = scored_records.into_iter().map(|sr| sr.record).collect();
        let diversity_score = self.compute_diversity_score(&records);
        let quality_score = self.compute_quality_score(&records);

        Ok(RerankedResult {
            records,
            explanations,
            quality_score,
            diversity_score,
        })
    }

    fn compute_base_scores(
        &self,
        records: &[UnifiedRecord],
        _context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        let scored: Vec<ScoredRecord> = records
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                let model_weight = self.config.model_weights.weight_for(record.source_model);
                let base_score = self.base_score(record.score);
                let weighted_score = base_score * model_weight;

                ScoredRecord {
                    record: record.clone(),
                    original_rank: idx,
                    base_score,
                    semantic_score: 0.0,
                    context_score: 0.0,
                    diversity_penalty: 0.0,
                    final_score: weighted_score,
                    score_components: vec![ScoreComponent {
                        name: "base_score".to_string(),
                        value: base_score,
                        weight: model_weight,
                        contribution: weighted_score,
                    }],
                }
            })
            .collect();

        Ok(scored)
    }

    fn apply_semantic_reranking(
        &self,
        mut records: Vec<ScoredRecord>,
        context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        if let Some(query_embedding) = &context.query_embedding {
            for record in &mut records {
                let semantic_score = if let Some(embedding) = self.extract_embedding(&record.record)
                {
                    self.compute_cosine_similarity(query_embedding, &embedding)
                } else if let (Some(query_text), Some(record_text)) = (
                    &context.query_text,
                    record.record.data.get("content").and_then(|v| v.as_str()),
                ) {
                    self.compute_text_similarity(query_text, record_text)
                } else {
                    self.config.missing_semantic_score
                };

                record.semantic_score = semantic_score;
                record.score_components.push(ScoreComponent {
                    name: "semantic_score".to_string(),
                    value: semantic_score,
                    weight: self.config.semantic_signal_weight,
                    contribution: semantic_score * self.config.semantic_signal_weight,
                });

                record.final_score = record.final_score * self.config.semantic_base_weight
                    + semantic_score * self.config.semantic_signal_weight;
            }
        }

        Ok(records)
    }

    fn apply_context_aware_scoring(
        &self,
        mut records: Vec<ScoredRecord>,
        context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        for record in &mut records {
            let mut context_score = 0.0;
            let mut components = vec![];

            if let Some(intent) = &context.intent {
                let intent_boost = match intent {
                    QueryIntent::SimilaritySearch
                        if record.record.source_model == DataModel::Vector =>
                    {
                        self.config.intent_boosts.similarity_vector
                    }
                    QueryIntent::RelationshipExploration
                        if record.record.source_model == DataModel::Graph =>
                    {
                        self.config.intent_boosts.relationship_graph
                    }
                    QueryIntent::Navigational
                        if record.base_score > self.config.navigational_min_base_score =>
                    {
                        self.config.intent_boosts.navigational_high_confidence
                    }
                    QueryIntent::Informational => self.config.intent_boosts.informational,
                    QueryIntent::Analytical
                        if record.record.source_model == DataModel::Observability =>
                    {
                        self.config.intent_boosts.analytical_observability
                    }
                    _ => 0.0,
                };

                context_score += intent_boost;
                components.push(ScoreComponent {
                    name: "intent_boost".to_string(),
                    value: intent_boost,
                    weight: 1.0,
                    contribution: intent_boost,
                });
            }

            if let Some(timestamp) = self.extract_timestamp(&record.record) {
                let temporal_boost =
                    self.compute_temporal_boost(timestamp, &context.temporal_preference);
                context_score += temporal_boost;
                components.push(ScoreComponent {
                    name: "temporal_boost".to_string(),
                    value: temporal_boost,
                    weight: 1.0,
                    contribution: temporal_boost,
                });
            }

            if context
                .required_models
                .contains(&record.record.source_model)
            {
                let required_model_boost = self.config.intent_boosts.required_model;
                context_score += required_model_boost;
                components.push(ScoreComponent {
                    name: "required_model_boost".to_string(),
                    value: required_model_boost,
                    weight: 1.0,
                    contribution: required_model_boost,
                });
            }

            record.context_score = context_score;
            record.score_components.extend(components);
            record.final_score += context_score;
        }

        Ok(records)
    }

    fn apply_mmr_diversity(&self, records: Vec<ScoredRecord>) -> Result<Vec<ScoredRecord>> {
        if records.len() <= 1 {
            return Ok(records);
        }

        let lambda = self.config.mmr_lambda;
        let mut selected: Vec<ScoredRecord> = vec![];
        let mut remaining: Vec<ScoredRecord> = records;

        remaining.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(first) = remaining.first().cloned() {
            selected.push(first);
            remaining.remove(0);
        }

        while !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_mmr = f64::NEG_INFINITY;

            for (idx, candidate) in remaining.iter().enumerate() {
                let relevance = candidate.final_score;
                let max_similarity = selected
                    .iter()
                    .map(|s| self.compute_record_similarity(&candidate.record, &s.record))
                    .fold(0.0_f64, f64::max);

                let mmr_score = lambda * relevance - (1.0 - lambda) * max_similarity;

                if mmr_score > best_mmr {
                    best_mmr = mmr_score;
                    best_idx = idx;
                }
            }

            let mut selected_record = remaining.remove(best_idx);
            let diversity_penalty =
                (1.0 - lambda) * (1.0 - best_mmr / selected_record.final_score).max(0.0);
            selected_record.diversity_penalty = diversity_penalty;
            selected_record.score_components.push(ScoreComponent {
                name: "diversity_adjustment".to_string(),
                value: -diversity_penalty,
                weight: 1.0,
                contribution: -diversity_penalty,
            });

            selected.push(selected_record);
        }

        for record in &mut selected {
            record.final_score -= record.diversity_penalty * self.config.diversity_weight;
        }

        trace!("MMR diversity applied to {} records", selected.len());

        Ok(selected)
    }

    fn generate_explanations(
        &self,
        original_records: &[UnifiedRecord],
        reranked: &[ScoredRecord],
    ) -> Result<Vec<RerankExplanation>> {
        let original_ranks: HashMap<String, usize> = original_records
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();

        let explanations: Vec<RerankExplanation> = reranked
            .iter()
            .enumerate()
            .map(|(new_rank, scored)| {
                let original_rank = original_ranks.get(&scored.record.id).copied().unwrap_or(0);
                let rank_change = original_rank as i64 - new_rank as i64;

                let explanation_text = if rank_change > 0 {
                    format!(
                        "Promoted {} positions due to: {}",
                        rank_change,
                        self.format_promotion_reasons(&scored.score_components)
                    )
                } else if rank_change < 0 {
                    format!(
                        "Demoted {} positions due to: {}",
                        -rank_change,
                        self.format_demotion_reasons(&scored.score_components)
                    )
                } else {
                    "Rank unchanged".to_string()
                };

                RerankExplanation {
                    record_id: scored.record.id.clone(),
                    original_rank,
                    new_rank,
                    score_components: scored.score_components.clone(),
                    explanation_text,
                    confidence: self.compute_explanation_confidence(scored),
                }
            })
            .collect();

        Ok(explanations)
    }

    fn extract_embedding(&self, record: &UnifiedRecord) -> Option<Vec<f32>> {
        record
            .data
            .get("embedding")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect()
            })
    }

    fn extract_timestamp(&self, record: &UnifiedRecord) -> Option<i64> {
        record
            .data
            .get("timestamp")
            .or_else(|| record.data.get("created_at"))
            .or_else(|| record.data.get("updated_at"))
            .and_then(|v| v.as_i64())
    }

    fn base_score(&self, score: Option<f64>) -> f64 {
        match (score, self.config.missing_score) {
            (Some(score), _) => score,
            (None, MissingScorePolicy::Configured) => {
                self.config.configured_missing_score.unwrap_or(0.0)
            }
            (None, MissingScorePolicy::Zero | MissingScorePolicy::Preserve) => 0.0,
        }
    }

    fn compute_cosine_similarity(&self, a: &[f32], b: &[f32]) -> f64 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f64 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as f64) * (*y as f64))
            .sum();
        let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();

        if norm_a > 0.0 && norm_b > 0.0 {
            dot / (norm_a * norm_b)
        } else {
            0.0
        }
    }

    fn compute_text_similarity(&self, text_a: &str, text_b: &str) -> f64 {
        let words_a: HashSet<&str> = text_a.split_whitespace().collect();
        let words_b: HashSet<&str> = text_b.split_whitespace().collect();

        let intersection = words_a.intersection(&words_b).count() as f64;
        let union = words_a.union(&words_b).count() as f64;

        if union > 0.0 {
            intersection / union
        } else {
            0.0
        }
    }

    fn compute_temporal_boost(&self, timestamp: i64, preference: &TemporalPreference) -> f64 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);

        let age_hours = (now_ns - timestamp) as f64 / (3600.0 * 1_000_000_000.0);

        match preference {
            TemporalPreference::Recent => {
                (-age_hours / self.config.temporal.recent_half_life_hours).exp()
                    * self.config.temporal.recent_weight
            }
            TemporalPreference::Historical => {
                (1.0 - (-age_hours / self.config.temporal.historical_half_life_hours).exp())
                    * self.config.temporal.historical_weight
            }
            TemporalPreference::Neutral => 0.0,
            TemporalPreference::Custom { half_life_hours } => {
                (-age_hours / half_life_hours).exp() * self.config.temporal.recent_weight
            }
        }
    }

    fn compute_record_similarity(&self, a: &UnifiedRecord, b: &UnifiedRecord) -> f64 {
        let mut similarity = 0.0;

        if a.source_model == b.source_model {
            similarity += self.config.diversity_similarity.same_model_weight;
        }

        let keys_a: HashSet<&String> = a.metadata.keys().collect();
        let keys_b: HashSet<&String> = b.metadata.keys().collect();
        let key_overlap = keys_a.intersection(&keys_b).count() as f64;
        let key_union = keys_a.union(&keys_b).count() as f64;
        if key_union > 0.0 {
            similarity += self.config.diversity_similarity.metadata_overlap_weight
                * (key_overlap / key_union);
        }

        if let (Some(score_a), Some(score_b)) = (a.score, b.score) {
            similarity += self.config.diversity_similarity.score_proximity_weight
                * (1.0 - (score_a - score_b).abs());
        }

        similarity.min(1.0)
    }

    fn compute_diversity_score(&self, records: &[UnifiedRecord]) -> f64 {
        if records.len() <= 1 {
            return 1.0;
        }

        let models: HashSet<&DataModel> = records.iter().map(|r| &r.source_model).collect();
        let model_diversity = models.len() as f64 / 4.0;

        let mut total_dissimilarity = 0.0;
        let mut count = 0;

        for (i, record_i) in records.iter().enumerate() {
            for record_j in records.iter().skip(i + 1) {
                total_dissimilarity += 1.0 - self.compute_record_similarity(record_i, record_j);
                count += 1;
            }
        }

        let avg_dissimilarity = if count > 0 {
            total_dissimilarity / count as f64
        } else {
            0.0
        };

        (model_diversity * self.config.diversity_model_weight
            + avg_dissimilarity * self.config.diversity_dissimilarity_weight)
            .min(1.0)
    }

    fn compute_quality_score(&self, records: &[UnifiedRecord]) -> f64 {
        if records.is_empty() {
            return 1.0;
        }

        let avg_score: f64 =
            records.iter().filter_map(|r| r.score).sum::<f64>() / records.len() as f64;

        avg_score.min(1.0)
    }

    fn format_promotion_reasons(&self, components: &[ScoreComponent]) -> String {
        components
            .iter()
            .filter(|c| c.contribution > 0.0)
            .map(|c| format!("{} (+{:.2})", c.name, c.contribution))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn format_demotion_reasons(&self, components: &[ScoreComponent]) -> String {
        components
            .iter()
            .filter(|c| c.contribution < 0.0)
            .map(|c| format!("{} ({:.2})", c.name, c.contribution))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn compute_explanation_confidence(&self, scored: &ScoredRecord) -> f64 {
        let positive_components = scored
            .score_components
            .iter()
            .filter(|c| c.contribution > 0.0)
            .count();
        let total_components = scored.score_components.len();

        if total_components > 0 {
            positive_components as f64 / total_components as f64
        } else {
            self.config.explanation_empty_confidence
        }
    }
}

#[derive(Debug, Clone)]
struct ScoredRecord {
    record: UnifiedRecord,
    #[allow(dead_code)]
    original_rank: usize,
    base_score: f64,
    semantic_score: f64,
    context_score: f64,
    diversity_penalty: f64,
    final_score: f64,
    score_components: Vec<ScoreComponent>,
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

    fn make_unscored_record(id: &str, model: DataModel) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: model,
            data: serde_json::json!({"id": id}),
            score: None,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_rerank_empty() {
        let reranker = CrossModalReranker::default_reranker();
        let result = QueryResult {
            records: vec![],
            total_count: Some(0),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();
        assert!(reranked.records.is_empty());
        assert_eq!(reranked.quality_score, 1.0);
    }

    #[test]
    fn test_default_reranker_is_disabled_and_preserves_order() {
        assert!(!RerankConfig::default().enabled);
        let reranker = CrossModalReranker::default_reranker();
        let records = vec![
            make_record("vector", 0.8, DataModel::Vector),
            make_record("graph", 0.85, DataModel::Graph),
            make_record("c", 0.7, DataModel::Graph),
        ];

        let result = QueryResult {
            records,
            total_count: Some(3),
            metrics: Default::default(),
        };

        let context = QueryContext {
            intent: Some(QueryIntent::SimilaritySearch),
            ..Default::default()
        };

        let reranked = reranker.rerank(result, &context).unwrap();
        assert_eq!(reranked.records.len(), 3);
        assert_eq!(reranked.records[0].id, "vector");
        assert_eq!(reranked.records[1].id, "graph");
        assert!(reranked.explanations.is_empty());
    }

    #[test]
    fn test_configured_similarity_intent_boost_changes_rank() {
        let config = RerankConfig {
            enabled: true,
            context_aware: true,
            intent_boosts: IntentBoostConfig {
                similarity_vector: 0.2,
                ..Default::default()
            },
            ..Default::default()
        };
        let reranker = CrossModalReranker::new(config);
        let records = vec![
            make_record("graph", 0.85, DataModel::Graph),
            make_record("vector", 0.8, DataModel::Vector),
        ];

        let result = QueryResult {
            records,
            total_count: Some(2),
            metrics: Default::default(),
        };

        let context = QueryContext {
            intent: Some(QueryIntent::SimilaritySearch),
            ..Default::default()
        };

        let reranked = reranker.rerank(result, &context).unwrap();
        assert_eq!(reranked.records.len(), 2);
        assert_eq!(reranked.records[0].id, "vector");
    }

    #[test]
    fn test_configured_missing_score_is_explicit() {
        let config = RerankConfig {
            enabled: true,
            missing_score: MissingScorePolicy::Configured,
            configured_missing_score: Some(0.95),
            ..Default::default()
        };
        let reranker = CrossModalReranker::new(config);
        let result = QueryResult {
            records: vec![
                make_record("scored", 0.5, DataModel::Document),
                make_unscored_record("configured_missing", DataModel::Vector),
            ],
            total_count: Some(2),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();
        assert_eq!(reranked.records[0].id, "configured_missing");
    }

    #[test]
    fn test_invalid_rerank_config_is_rejected() {
        let missing_score_config = RerankConfig {
            enabled: true,
            missing_score: MissingScorePolicy::Configured,
            configured_missing_score: None,
            ..Default::default()
        };
        assert!(missing_score_config.validate().is_err());

        let bad_mmr_config = RerankConfig {
            enabled: true,
            mmr_lambda: 1.5,
            ..Default::default()
        };
        assert!(bad_mmr_config.validate().is_err());
    }

    #[test]
    fn test_mmr_diversity() {
        let config = RerankConfig {
            enabled: true,
            diversity_optimization: true,
            diversity_weight: 0.5,
            mmr_lambda: 0.5,
            diversity_similarity: DiversitySimilarityConfig {
                same_model_weight: 0.3,
                metadata_overlap_weight: 0.3,
                score_proximity_weight: 0.4,
            },
            ..Default::default()
        };

        let reranker = CrossModalReranker::new(config);
        let records = vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.85, DataModel::Vector),
            make_record("c", 0.8, DataModel::Graph),
        ];

        let result = QueryResult {
            records,
            total_count: Some(3),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();
        assert_eq!(reranked.records.len(), 3);
        assert!(reranked.diversity_score > 0.0);
    }

    #[test]
    fn test_explanation_generation() {
        let config = RerankConfig {
            enabled: true,
            generate_explanations: true,
            ..Default::default()
        };

        let reranker = CrossModalReranker::new(config);
        let records = vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.8, DataModel::Document),
        ];

        let result = QueryResult {
            records,
            total_count: Some(2),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();
        assert_eq!(reranked.explanations.len(), 2);

        for explanation in &reranked.explanations {
            assert!(!explanation.record_id.is_empty());
            assert!(!explanation.explanation_text.is_empty());
            assert!((0.0..=1.0).contains(&explanation.confidence));
        }
    }
}
