//! Result Fusion Strategies
//!
//! Combines results from multiple data model queries into a unified result set.
//! Supports various fusion strategies: intersection, union, ranked fusion, etc.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use tracing::debug;

use super::ast::DataModel;
use super::{QueryMetrics, QueryResult, UnifiedRecord};

/// Strategy for fusing results from multiple query components
#[derive(Debug, Clone)]
pub enum FusionStrategy {
    /// Only return records that appear in ALL component results (AND logic)
    Intersection,
    /// Return records that appear in ANY component result (OR logic)
    Union,
    /// Return records from the first component, filtering by other components
    FirstWithFilter,
    /// Weighted ranking combining scores from all components
    RankedFusion {
        /// Weights per data model (default 1.0 if not specified)
        weights: HashMap<DataModel, f64>,
        /// Whether to normalize scores before fusion
        normalize: bool,
    },
    /// Reciprocal Rank Fusion (RRF) - robust to different score scales
    ReciprocalRankFusion {
        /// RRF constant (typically 60)
        k: u32,
    },
    /// Custom fusion using a provided function
    Custom(String), // Stores function name, actual logic in executor
}

impl Default for FusionStrategy {
    fn default() -> Self {
        Self::Intersection
    }
}

/// Result fuser that combines sub-query results
pub struct ResultFuser {
    /// Default fusion strategy
    default_strategy: FusionStrategy,
}

impl ResultFuser {
    /// Create a new result fuser with a default strategy
    pub fn new(default_strategy: FusionStrategy) -> Self {
        Self { default_strategy }
    }

    /// Fuse results from multiple sub-queries
    pub fn fuse(
        &self,
        sub_results: Vec<SubQueryResult>,
        strategy: &FusionStrategy,
    ) -> Result<QueryResult> {
        if sub_results.is_empty() {
            return Ok(QueryResult {
                records: Vec::new(),
                total_count: Some(0),
                metrics: QueryMetrics::default(),
            });
        }

        // Single result - no fusion needed
        if sub_results.len() == 1 {
            return Ok(self.convert_single_result(sub_results.into_iter().next().unwrap()));
        }

        let fused_records = match strategy {
            FusionStrategy::Intersection => self.fuse_intersection(&sub_results)?,
            FusionStrategy::Union => self.fuse_union(&sub_results)?,
            FusionStrategy::FirstWithFilter => self.fuse_first_with_filter(&sub_results)?,
            FusionStrategy::RankedFusion { weights, normalize } => {
                self.fuse_ranked(&sub_results, weights, *normalize)?
            }
            FusionStrategy::ReciprocalRankFusion { k } => {
                self.fuse_rrf(&sub_results, *k)?
            }
            FusionStrategy::Custom(name) => {
                return Err(anyhow!("Custom fusion '{}' not implemented", name));
            }
        };

        // Aggregate metrics
        let metrics = self.aggregate_metrics(&sub_results);

        Ok(QueryResult {
            records: fused_records,
            total_count: None, // Not always available after fusion
            metrics,
        })
    }

    /// Intersection fusion - only records appearing in all results
    fn fuse_intersection(&self, sub_results: &[SubQueryResult]) -> Result<Vec<UnifiedRecord>> {
        if sub_results.is_empty() {
            return Ok(Vec::new());
        }

        // Start with IDs from first result
        let mut common_ids: HashSet<String> = sub_results[0].records
            .iter()
            .map(|r| r.id.clone())
            .collect();

        // Intersect with other results
        for result in sub_results.iter().skip(1) {
            let result_ids: HashSet<String> = result.records
                .iter()
                .map(|r| r.id.clone())
                .collect();
            common_ids = common_ids.intersection(&result_ids).cloned().collect();
        }

        debug!("Intersection found {} common records", common_ids.len());

        // Build merged records for common IDs
        let mut merged_records: HashMap<String, UnifiedRecord> = HashMap::new();

        for result in sub_results {
            for record in &result.records {
                if common_ids.contains(&record.id) {
                    merged_records.entry(record.id.clone())
                        .and_modify(|existing| {
                            // Merge data and metadata
                            self.merge_record_data(existing, record);
                        })
                        .or_insert_with(|| record.clone());
                }
            }
        }

        // Sort by score if available
        let mut records: Vec<UnifiedRecord> = merged_records.into_values().collect();
        records.sort_by(|a, b| {
            b.score.unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(records)
    }

    /// Union fusion - all unique records from all results
    fn fuse_union(&self, sub_results: &[SubQueryResult]) -> Result<Vec<UnifiedRecord>> {
        let mut all_records: HashMap<String, UnifiedRecord> = HashMap::new();

        for result in sub_results {
            for record in &result.records {
                all_records.entry(record.id.clone())
                    .and_modify(|existing| {
                        self.merge_record_data(existing, record);
                        // Take higher score
                        if let (Some(new_score), Some(old_score)) = (record.score, existing.score) {
                            if new_score > old_score {
                                existing.score = Some(new_score);
                            }
                        } else if record.score.is_some() {
                            existing.score = record.score;
                        }
                    })
                    .or_insert_with(|| record.clone());
            }
        }

        debug!("Union produced {} unique records", all_records.len());

        let mut records: Vec<UnifiedRecord> = all_records.into_values().collect();
        records.sort_by(|a, b| {
            b.score.unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(records)
    }

    /// First with filter - take first result, filter by IDs in others
    fn fuse_first_with_filter(&self, sub_results: &[SubQueryResult]) -> Result<Vec<UnifiedRecord>> {
        if sub_results.is_empty() {
            return Ok(Vec::new());
        }

        let first_result = &sub_results[0];

        if sub_results.len() == 1 {
            return Ok(first_result.records.clone());
        }

        // Get IDs from filter results
        let filter_ids: HashSet<String> = sub_results.iter()
            .skip(1)
            .flat_map(|r| r.records.iter().map(|rec| rec.id.clone()))
            .collect();

        // Filter first result by these IDs
        let filtered: Vec<UnifiedRecord> = first_result.records
            .iter()
            .filter(|r| filter_ids.contains(&r.id))
            .cloned()
            .collect();

        debug!("First-with-filter produced {} records", filtered.len());

        Ok(filtered)
    }

    /// Ranked fusion with optional weights and normalization
    fn fuse_ranked(
        &self,
        sub_results: &[SubQueryResult],
        weights: &HashMap<DataModel, f64>,
        normalize: bool,
    ) -> Result<Vec<UnifiedRecord>> {
        let mut score_map: HashMap<String, (UnifiedRecord, f64)> = HashMap::new();

        for result in sub_results {
            let weight = weights.get(&result.source_model).copied().unwrap_or(1.0);

            // Normalize scores within this result if requested
            let (min_score, max_score) = if normalize {
                let scores: Vec<f64> = result.records.iter()
                    .filter_map(|r| r.score)
                    .collect();
                if scores.is_empty() {
                    (0.0, 1.0)
                } else {
                    let min = scores.iter().cloned().fold(f64::INFINITY, f64::min);
                    let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    if (max - min).abs() < 1e-10 {
                        (0.0, 1.0)
                    } else {
                        (min, max)
                    }
                }
            } else {
                (0.0, 1.0)
            };

            for record in &result.records {
                let raw_score = record.score.unwrap_or(0.0);
                let normalized_score = if normalize && (max_score - min_score).abs() > 1e-10 {
                    (raw_score - min_score) / (max_score - min_score)
                } else {
                    raw_score
                };
                let weighted_score = normalized_score * weight;

                score_map.entry(record.id.clone())
                    .and_modify(|(existing, total_score)| {
                        *total_score += weighted_score;
                        self.merge_record_data(existing, record);
                    })
                    .or_insert_with(|| (record.clone(), weighted_score));
            }
        }

        // Update scores and sort
        let mut records: Vec<UnifiedRecord> = score_map.into_iter()
            .map(|(_, (mut record, score))| {
                record.score = Some(score);
                record
            })
            .collect();

        records.sort_by(|a, b| {
            b.score.unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!("Ranked fusion produced {} records", records.len());

        Ok(records)
    }

    /// Reciprocal Rank Fusion - robust to different score scales
    fn fuse_rrf(&self, sub_results: &[SubQueryResult], k: u32) -> Result<Vec<UnifiedRecord>> {
        let mut rrf_scores: HashMap<String, (UnifiedRecord, f64)> = HashMap::new();

        for result in sub_results {
            // Sort by score to get ranks
            let mut ranked: Vec<&UnifiedRecord> = result.records.iter().collect();
            ranked.sort_by(|a, b| {
                b.score.unwrap_or(0.0)
                    .partial_cmp(&a.score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            for (rank, record) in ranked.iter().enumerate() {
                // RRF score: 1 / (k + rank)
                let rrf_score = 1.0 / (k as f64 + rank as f64 + 1.0);

                rrf_scores.entry(record.id.clone())
                    .and_modify(|(existing, total_score)| {
                        *total_score += rrf_score;
                        self.merge_record_data(existing, record);
                    })
                    .or_insert_with(|| ((*record).clone(), rrf_score));
            }
        }

        // Update scores and sort
        let mut records: Vec<UnifiedRecord> = rrf_scores.into_iter()
            .map(|(_, (mut record, score))| {
                record.score = Some(score);
                record
            })
            .collect();

        records.sort_by(|a, b| {
            b.score.unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!("RRF fusion (k={}) produced {} records", k, records.len());

        Ok(records)
    }

    /// Merge data from one record into another
    fn merge_record_data(&self, target: &mut UnifiedRecord, source: &UnifiedRecord) {
        // Merge JSON data (source values override if present)
        if let (Some(target_obj), Some(source_obj)) = (
            target.data.as_object_mut(),
            source.data.as_object(),
        ) {
            for (key, value) in source_obj {
                target_obj.insert(key.clone(), value.clone());
            }
        }

        // Merge metadata
        for (key, value) in &source.metadata {
            target.metadata.entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    /// Convert a single sub-query result to a unified QueryResult
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

    /// Aggregate metrics from all sub-query results
    fn aggregate_metrics(&self, sub_results: &[SubQueryResult]) -> QueryMetrics {
        let mut metrics = QueryMetrics::default();

        for result in sub_results {
            metrics.sub_query_times.push((result.source_model.clone(), result.execution_time_us));
            metrics.records_scanned += result.records_scanned;
        }

        // Total time is max of parallel executions
        metrics.total_time_us = sub_results.iter()
            .map(|r| r.execution_time_us)
            .max()
            .unwrap_or(0);

        metrics
    }
}

/// Result from a single sub-query
#[derive(Debug, Clone)]
pub struct SubQueryResult {
    /// Source data model
    pub source_model: DataModel,
    /// Result records
    pub records: Vec<UnifiedRecord>,
    /// Total count (if available)
    pub total_count: Option<u64>,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Records scanned
    pub records_scanned: u64,
    /// Records returned
    pub records_returned: u64,
}

impl SubQueryResult {
    /// Create a new empty sub-query result
    pub fn empty(model: DataModel) -> Self {
        Self {
            source_model: model,
            records: Vec::new(),
            total_count: Some(0),
            execution_time_us: 0,
            records_scanned: 0,
            records_returned: 0,
        }
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
    fn test_intersection_fusion() {
        let fuser = ResultFuser::new(FusionStrategy::Intersection);

        let result1 = make_sub_result(DataModel::Vector, vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.8, DataModel::Vector),
            make_record("c", 0.7, DataModel::Vector),
        ]);

        let result2 = make_sub_result(DataModel::Document, vec![
            make_record("b", 0.85, DataModel::Document),
            make_record("c", 0.75, DataModel::Document),
            make_record("d", 0.65, DataModel::Document),
        ]);

        let fused = fuser.fuse(vec![result1, result2], &FusionStrategy::Intersection).unwrap();

        assert_eq!(fused.records.len(), 2); // b and c
        assert!(fused.records.iter().any(|r| r.id == "b"));
        assert!(fused.records.iter().any(|r| r.id == "c"));
        assert!(!fused.records.iter().any(|r| r.id == "a"));
        assert!(!fused.records.iter().any(|r| r.id == "d"));
    }

    #[test]
    fn test_union_fusion() {
        let fuser = ResultFuser::new(FusionStrategy::Union);

        let result1 = make_sub_result(DataModel::Vector, vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.8, DataModel::Vector),
        ]);

        let result2 = make_sub_result(DataModel::Document, vec![
            make_record("c", 0.85, DataModel::Document),
            make_record("b", 0.95, DataModel::Document), // Higher score for b
        ]);

        let fused = fuser.fuse(vec![result1, result2], &FusionStrategy::Union).unwrap();

        assert_eq!(fused.records.len(), 3); // a, b, c

        // b should have the higher score (0.95)
        let b_record = fused.records.iter().find(|r| r.id == "b").unwrap();
        assert!((b_record.score.unwrap() - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_rrf_fusion() {
        let fuser = ResultFuser::new(FusionStrategy::ReciprocalRankFusion { k: 60 });

        let result1 = make_sub_result(DataModel::Vector, vec![
            make_record("a", 0.9, DataModel::Vector),
            make_record("b", 0.8, DataModel::Vector),
        ]);

        let result2 = make_sub_result(DataModel::Document, vec![
            make_record("b", 0.85, DataModel::Document),
            make_record("a", 0.75, DataModel::Document),
        ]);

        let fused = fuser.fuse(
            vec![result1, result2],
            &FusionStrategy::ReciprocalRankFusion { k: 60 }
        ).unwrap();

        assert_eq!(fused.records.len(), 2);

        // Both a and b appear in both lists, so they should have similar RRF scores
        // but the exact ranking depends on positions
        let a_record = fused.records.iter().find(|r| r.id == "a").unwrap();
        let b_record = fused.records.iter().find(|r| r.id == "b").unwrap();

        // Both should have positive RRF scores
        assert!(a_record.score.unwrap() > 0.0);
        assert!(b_record.score.unwrap() > 0.0);
    }

    #[test]
    fn test_empty_results() {
        let fuser = ResultFuser::new(FusionStrategy::Intersection);
        let fused = fuser.fuse(vec![], &FusionStrategy::Intersection).unwrap();
        assert!(fused.records.is_empty());
    }

    #[test]
    fn test_single_result() {
        let fuser = ResultFuser::new(FusionStrategy::Intersection);

        let result = make_sub_result(DataModel::Vector, vec![
            make_record("a", 0.9, DataModel::Vector),
        ]);

        let fused = fuser.fuse(vec![result], &FusionStrategy::Intersection).unwrap();

        assert_eq!(fused.records.len(), 1);
        assert_eq!(fused.records[0].id, "a");
    }
}
