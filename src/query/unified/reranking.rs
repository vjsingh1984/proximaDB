//! Cross-Modal Reranking
//!
//! Advanced reranking strategies for multi-modal query results that consider:
//! - Semantic similarity across modalities (vector, document, graph)
//! - Context-aware scoring based on query intent
//! - Diversity optimization for varied results
//! - Explanation generation for transparency

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use super::ast::DataModel;
use super::{QueryResult, UnifiedRecord};

/// Reranking strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    /// Enable cross-modal semantic similarity reranking
    pub semantic_rerank: bool,
    /// Enable diversity optimization
    pub diversity_optimization: bool,
    /// Diversity weight (0.0 to 1.0)
    pub diversity_weight: f64,
    /// Maximum marginal relevance lambda (0.0 = max diversity, 1.0 = max relevance)
    pub mmr_lambda: f64,
    /// Enable context-aware scoring
    pub context_aware: bool,
    /// Model weights per data model for final scoring
    pub model_weights: HashMap<DataModel, f64>,
    /// Generate explanations for reranking decisions
    pub generate_explanations: bool,
    /// Top-k results to consider for reranking
    pub rerank_top_k: usize,
}

impl Default for RerankConfig {
    fn default() -> Self {
        let mut model_weights = HashMap::new();
        model_weights.insert(DataModel::Vector, 1.0);
        model_weights.insert(DataModel::Document, 0.8);
        model_weights.insert(DataModel::Graph, 0.9);
        model_weights.insert(DataModel::Observability, 0.7);

        Self {
            semantic_rerank: true,
            diversity_optimization: true,
            diversity_weight: 0.3,
            mmr_lambda: 0.7,
            context_aware: true,
            model_weights,
            generate_explanations: false,
            rerank_top_k: 100,
        }
    }
}

/// Query context for context-aware reranking
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Original query text (if available)
    pub query_text: Option<String>,
    /// Query embedding (if available)
    pub query_embedding: Option<Vec<f32>>,
    /// User preferences/history (optional)
    pub user_preferences: HashMap<String, f64>,
    /// Intent classification (e.g., "navigational", "informational", "transactional")
    pub intent: Option<QueryIntent>,
    /// Temporal context (recency preference)
    pub temporal_preference: TemporalPreference,
    /// Required data models (user explicitly requested these)
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

/// Query intent classification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryIntent {
    /// Looking for a specific item
    Navigational,
    /// Seeking information/exploration
    Informational,
    /// Action-oriented (e.g., purchase, update)
    Transactional,
    /// Similarity search
    SimilaritySearch,
    /// Relationship exploration (graph-centric)
    RelationshipExploration,
    /// Analytics/aggregation
    Analytical,
}

/// Temporal preference for recency
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TemporalPreference {
    /// Prefer recent results
    Recent,
    /// Prefer older/established results
    Historical,
    /// No temporal preference
    Neutral,
    /// Custom decay function
    Custom { half_life_hours: f64 },
}

/// Explanation for a reranking decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankExplanation {
    /// Record ID
    pub record_id: String,
    /// Original rank before reranking
    pub original_rank: usize,
    /// New rank after reranking
    pub new_rank: usize,
    /// Score components
    pub score_components: Vec<ScoreComponent>,
    /// Human-readable explanation
    pub explanation_text: String,
    /// Confidence in the reranking (0.0 to 1.0)
    pub confidence: f64,
}

/// Component of a reranking score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreComponent {
    /// Component name
    pub name: String,
    /// Component value
    pub value: f64,
    /// Weight applied
    pub weight: f64,
    /// Contribution to final score
    pub contribution: f64,
}

/// Reranked result with explanations
#[derive(Debug, Clone)]
pub struct RerankedResult {
    /// Reranked records
    pub records: Vec<UnifiedRecord>,
    /// Explanations per record (if enabled)
    pub explanations: Vec<RerankExplanation>,
    /// Overall reranking quality score
    pub quality_score: f64,
    /// Diversity score of the result set
    pub diversity_score: f64,
}

/// Cross-Modal Reranker
pub struct CrossModalReranker {
    config: RerankConfig,
}

impl CrossModalReranker {
    /// Create a new cross-modal reranker
    pub fn new(config: RerankConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn default_reranker() -> Self {
        Self::new(RerankConfig::default())
    }

    /// Rerank query results using cross-modal signals
    pub fn rerank(&self, result: QueryResult, context: &QueryContext) -> Result<RerankedResult> {
        if result.records.is_empty() {
            return Ok(RerankedResult {
                records: vec![],
                explanations: vec![],
                quality_score: 1.0,
                diversity_score: 1.0,
            });
        }

        // Limit to top-k for reranking
        let records_to_rerank: Vec<UnifiedRecord> = result
            .records
            .into_iter()
            .take(self.config.rerank_top_k)
            .collect();

        debug!("Reranking {} records", records_to_rerank.len());

        // Step 1: Compute base scores
        let mut scored_records = self.compute_base_scores(&records_to_rerank, context)?;

        // Step 2: Apply semantic reranking if enabled
        if self.config.semantic_rerank {
            scored_records = self.apply_semantic_reranking(scored_records, context)?;
        }

        // Step 3: Apply context-aware scoring if enabled
        if self.config.context_aware {
            scored_records = self.apply_context_aware_scoring(scored_records, context)?;
        }

        // Step 4: Apply diversity optimization if enabled
        if self.config.diversity_optimization {
            scored_records = self.apply_mmr_diversity(scored_records)?;
        }

        // Step 5: Generate explanations if enabled
        let explanations = if self.config.generate_explanations {
            self.generate_explanations(&records_to_rerank, &scored_records)?
        } else {
            vec![]
        };

        // Step 6: Sort by final score and extract records
        scored_records.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let records: Vec<UnifiedRecord> = scored_records.into_iter().map(|sr| sr.record).collect();

        // Compute quality and diversity scores
        let diversity_score = self.compute_diversity_score(&records);
        let quality_score = self.compute_quality_score(&records);

        Ok(RerankedResult {
            records,
            explanations,
            quality_score,
            diversity_score,
        })
    }

    /// Compute base scores for all records
    fn compute_base_scores(
        &self,
        records: &[UnifiedRecord],
        _context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        let scored: Vec<ScoredRecord> = records
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                let model_weight = self
                    .config
                    .model_weights
                    .get(&record.source_model)
                    .copied()
                    .unwrap_or(1.0);

                let base_score = record.score.unwrap_or(0.5);
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

    /// Apply semantic similarity reranking
    fn apply_semantic_reranking(
        &self,
        mut records: Vec<ScoredRecord>,
        context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        // If we have query embedding, use it for semantic scoring
        if let Some(query_embedding) = &context.query_embedding {
            for record in &mut records {
                // Try to get record embedding from metadata
                let semantic_score = if let Some(embedding) = self.extract_embedding(&record.record)
                {
                    self.compute_cosine_similarity(query_embedding, &embedding)
                } else {
                    // Fall back to text-based similarity if available
                    if let (Some(query_text), Some(record_text)) = (
                        &context.query_text,
                        record.record.data.get("content").and_then(|v| v.as_str()),
                    ) {
                        self.compute_text_similarity(query_text, record_text)
                    } else {
                        0.5 // Neutral score if no semantic information available
                    }
                };

                record.semantic_score = semantic_score;
                record.score_components.push(ScoreComponent {
                    name: "semantic_score".to_string(),
                    value: semantic_score,
                    weight: 0.3,
                    contribution: semantic_score * 0.3,
                });

                // Blend semantic score with base score
                record.final_score = record.final_score * 0.7 + semantic_score * 0.3;
            }
        }

        Ok(records)
    }

    /// Apply context-aware scoring based on query intent
    fn apply_context_aware_scoring(
        &self,
        mut records: Vec<ScoredRecord>,
        context: &QueryContext,
    ) -> Result<Vec<ScoredRecord>> {
        for record in &mut records {
            let mut context_score = 0.0;
            let mut components = vec![];

            // Apply intent-based scoring
            if let Some(intent) = &context.intent {
                let intent_boost = match intent {
                    QueryIntent::SimilaritySearch => {
                        // Boost vector results for similarity search
                        if record.record.source_model == DataModel::Vector {
                            0.2
                        } else {
                            0.0
                        }
                    }
                    QueryIntent::RelationshipExploration => {
                        // Boost graph results for relationship queries
                        if record.record.source_model == DataModel::Graph {
                            0.2
                        } else {
                            0.0
                        }
                    }
                    QueryIntent::Navigational => {
                        // Boost exact matches
                        if record.base_score > 0.9 { 0.15 } else { 0.0 }
                    }
                    QueryIntent::Informational => {
                        // Slight diversity boost for exploration
                        0.05
                    }
                    QueryIntent::Analytical => {
                        // Boost observability data
                        if record.record.source_model == DataModel::Observability {
                            0.15
                        } else {
                            0.0
                        }
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

            // Apply temporal preference
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

            // Apply required models boost
            if context
                .required_models
                .contains(&record.record.source_model)
            {
                context_score += 0.1;
                components.push(ScoreComponent {
                    name: "required_model_boost".to_string(),
                    value: 0.1,
                    weight: 1.0,
                    contribution: 0.1,
                });
            }

            record.context_score = context_score;
            record.score_components.extend(components);
            record.final_score += context_score;
        }

        Ok(records)
    }

    /// Apply Maximum Marginal Relevance (MMR) for diversity
    fn apply_mmr_diversity(&self, records: Vec<ScoredRecord>) -> Result<Vec<ScoredRecord>> {
        if records.len() <= 1 {
            return Ok(records);
        }

        let lambda = self.config.mmr_lambda;
        let mut selected: Vec<ScoredRecord> = vec![];
        let mut remaining: Vec<ScoredRecord> = records;

        // Select first record (highest score)
        remaining.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(first) = remaining.first().cloned() {
            selected.push(first);
            remaining.remove(0);
        }

        // Iteratively select remaining records using MMR
        while !remaining.is_empty() {
            let mut best_idx = 0;
            let mut best_mmr = f64::NEG_INFINITY;

            for (idx, candidate) in remaining.iter().enumerate() {
                // Relevance term
                let relevance = candidate.final_score;

                // Diversity term: max similarity to any selected record
                let max_similarity = selected
                    .iter()
                    .map(|s| self.compute_record_similarity(&candidate.record, &s.record))
                    .fold(0.0_f64, f64::max);

                // MMR score: lambda * relevance - (1 - lambda) * max_similarity
                let mmr_score = lambda * relevance - (1.0 - lambda) * max_similarity;

                if mmr_score > best_mmr {
                    best_mmr = mmr_score;
                    best_idx = idx;
                }
            }

            // Add diversity penalty to the selected record
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

        // Update final scores with diversity penalty
        for record in &mut selected {
            record.final_score -= record.diversity_penalty * self.config.diversity_weight;
        }

        trace!("MMR diversity applied to {} records", selected.len());

        Ok(selected)
    }

    /// Generate explanations for reranking decisions
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

    // Helper methods

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
        // Simple Jaccard similarity based on word overlap
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
                // Exponential decay with 24-hour half-life
                (-age_hours / 24.0).exp() * 0.1
            }
            TemporalPreference::Historical => {
                // Boost older content
                (1.0 - (-age_hours / 720.0).exp()) * 0.1 // 30-day half-life
            }
            TemporalPreference::Neutral => 0.0,
            TemporalPreference::Custom { half_life_hours } => {
                let decay = (-age_hours / half_life_hours).exp();
                decay * 0.1
            }
        }
    }

    fn compute_record_similarity(&self, a: &UnifiedRecord, b: &UnifiedRecord) -> f64 {
        let mut similarity = 0.0;

        // Model similarity (same model = more similar)
        if a.source_model == b.source_model {
            similarity += 0.3;
        }

        // Metadata overlap
        let keys_a: HashSet<&String> = a.metadata.keys().collect();
        let keys_b: HashSet<&String> = b.metadata.keys().collect();
        let key_overlap = keys_a.intersection(&keys_b).count() as f64;
        let key_union = keys_a.union(&keys_b).count() as f64;
        if key_union > 0.0 {
            similarity += 0.3 * (key_overlap / key_union);
        }

        // Score similarity
        if let (Some(score_a), Some(score_b)) = (a.score, b.score) {
            similarity += 0.4 * (1.0 - (score_a - score_b).abs());
        }

        similarity.min(1.0)
    }

    fn compute_diversity_score(&self, records: &[UnifiedRecord]) -> f64 {
        if records.len() <= 1 {
            return 1.0;
        }

        // Count unique source models
        let models: HashSet<&DataModel> = records.iter().map(|r| &r.source_model).collect();
        let model_diversity = models.len() as f64 / 4.0; // Max 4 models

        // Compute average pairwise dissimilarity
        let mut total_dissimilarity = 0.0;
        let mut count = 0;

        for i in 0..records.len() {
            for j in (i + 1)..records.len() {
                total_dissimilarity +=
                    1.0 - self.compute_record_similarity(&records[i], &records[j]);
                count += 1;
            }
        }

        let avg_dissimilarity = if count > 0 {
            total_dissimilarity / count as f64
        } else {
            0.0
        };

        (model_diversity * 0.5 + avg_dissimilarity * 0.5).min(1.0)
    }

    fn compute_quality_score(&self, records: &[UnifiedRecord]) -> f64 {
        if records.is_empty() {
            return 1.0;
        }

        // Average normalized score
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
        // Higher confidence when more score components agree
        let positive_components = scored
            .score_components
            .iter()
            .filter(|c| c.contribution > 0.0)
            .count();
        let total_components = scored.score_components.len();

        if total_components > 0 {
            positive_components as f64 / total_components as f64
        } else {
            0.5
        }
    }
}

/// Internal scored record for reranking
#[derive(Debug, Clone)]
struct ScoredRecord {
    record: UnifiedRecord,
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
    fn test_rerank_preserves_records() {
        let reranker = CrossModalReranker::default_reranker();
        let records = vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.8, DataModel::Document),
            make_record("c", 0.7, DataModel::Graph),
        ];

        let result = QueryResult {
            records,
            total_count: Some(3),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();
        assert_eq!(reranked.records.len(), 3);
    }

    #[test]
    fn test_rerank_with_intent() {
        let reranker = CrossModalReranker::default_reranker();
        let records = vec![
            make_record("vector", 0.8, DataModel::Vector),
            make_record("graph", 0.85, DataModel::Graph),
        ];

        let result = QueryResult {
            records,
            total_count: Some(2),
            metrics: Default::default(),
        };

        // With similarity search intent, vector results should be boosted
        let context = QueryContext {
            intent: Some(QueryIntent::SimilaritySearch),
            ..Default::default()
        };

        let reranked = reranker.rerank(result, &context).unwrap();
        assert_eq!(reranked.records.len(), 2);
    }

    #[test]
    fn test_mmr_diversity() {
        let config = RerankConfig {
            diversity_optimization: true,
            mmr_lambda: 0.5, // Equal weight to relevance and diversity
            ..Default::default()
        };

        let reranker = CrossModalReranker::new(config);

        // Create records with varying similarity
        let records = vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.85, DataModel::Vector), // Same model as a
            make_record("c", 0.8, DataModel::Graph),   // Different model
        ];

        let result = QueryResult {
            records,
            total_count: Some(3),
            metrics: Default::default(),
        };

        let reranked = reranker.rerank(result, &QueryContext::default()).unwrap();

        // Diversity optimization should mix different source models
        assert_eq!(reranked.records.len(), 3);
        assert!(reranked.diversity_score > 0.0);
    }

    #[test]
    fn test_explanation_generation() {
        let config = RerankConfig {
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
            assert!(!explanation.explanation_text.is_empty());
            assert!(explanation.confidence >= 0.0 && explanation.confidence <= 1.0);
        }
    }

    #[test]
    fn test_temporal_preference_recent() {
        let reranker = CrossModalReranker::default_reranker();

        // Create records with timestamps
        let mut recent_record = make_record("recent", 0.7, DataModel::Document);
        recent_record.data = serde_json::json!({
            "id": "recent",
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i64
        });

        let mut old_record = make_record("old", 0.8, DataModel::Document);
        old_record.data = serde_json::json!({
            "id": "old",
            "timestamp": 0_i64 // Very old
        });

        let result = QueryResult {
            records: vec![old_record, recent_record],
            total_count: Some(2),
            metrics: Default::default(),
        };

        let context = QueryContext {
            temporal_preference: TemporalPreference::Recent,
            ..Default::default()
        };

        let reranked = reranker.rerank(result, &context).unwrap();
        assert_eq!(reranked.records.len(), 2);
    }

    #[test]
    fn test_cosine_similarity() {
        let reranker = CrossModalReranker::default_reranker();

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((reranker.compute_cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(reranker.compute_cosine_similarity(&a, &c).abs() < 0.001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((reranker.compute_cosine_similarity(&a, &d) + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_text_similarity() {
        let reranker = CrossModalReranker::default_reranker();

        let a = "hello world test";
        let b = "hello world example";
        let similarity = reranker.compute_text_similarity(a, b);
        assert!(similarity > 0.0 && similarity < 1.0);

        let c = "completely different text";
        let low_sim = reranker.compute_text_similarity(a, c);
        assert!(low_sim < similarity);
    }
}
