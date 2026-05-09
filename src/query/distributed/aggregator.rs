/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Result Aggregator
//!
//! Aggregates and merges results from local and remote query execution.

use std::collections::HashMap;

use anyhow::Result;
use tracing::debug;

use crate::core::error::VectorDBError;
use crate::query::unified::UnifiedRecord;
use crate::query::unified::ast::DataModel;
use crate::query::unified::fusion::SubQueryResult;

/// Strategy for aggregating results from multiple nodes
#[derive(Debug, Clone)]
pub enum AggregationStrategy {
    /// Merge all results (default for most queries)
    Merge(MergeConfig),
    /// Sum numeric values (for count aggregations)
    Sum,
    /// Average numeric values
    Average,
    /// Take top-K by score
    TopK(usize),
    /// Union with deduplication
    UnionDedup,
    /// Intersection (results must appear on all nodes)
    Intersection,
}

impl Default for AggregationStrategy {
    fn default() -> Self {
        AggregationStrategy::Merge(MergeConfig::default())
    }
}

/// Configuration for merge aggregation
#[derive(Debug, Clone)]
pub struct MergeConfig {
    /// Deduplicate by ID
    pub deduplicate: bool,
    /// Sort results after merge
    pub sort_by_score: bool,
    /// Maximum results after merge
    pub limit: Option<usize>,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            deduplicate: true,
            sort_by_score: true,
            limit: None,
        }
    }
}

/// Aggregator for distributed query results
pub struct ResultAggregator {
    strategy: AggregationStrategy,
}

impl ResultAggregator {
    /// Create a new result aggregator
    pub fn new(strategy: AggregationStrategy) -> Self {
        Self { strategy }
    }

    /// Aggregate local and remote results
    pub fn aggregate(
        &self,
        local_results: Vec<SubQueryResult>,
        remote_results: Vec<SubQueryResult>,
    ) -> Result<Vec<SubQueryResult>> {
        debug!(
            "Aggregating {} local and {} remote results",
            local_results.len(),
            remote_results.len()
        );

        // Combine all results
        let mut all_results = local_results;
        all_results.extend(remote_results);

        // Group by data model
        let mut by_model: HashMap<DataModel, Vec<SubQueryResult>> = HashMap::new();
        for result in all_results {
            let model = result.source_model;
            by_model.entry(model).or_default().push(result);
        }

        // Aggregate each model's results
        let mut aggregated = Vec::new();
        for (model, results) in by_model {
            let merged = self.aggregate_model_results(model, results)?;
            aggregated.push(merged);
        }

        Ok(aggregated)
    }

    /// Aggregate results for a single data model
    fn aggregate_model_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
    ) -> Result<SubQueryResult> {
        if results.is_empty() {
            return Ok(SubQueryResult::empty(model));
        }

        if results.len() == 1 {
            let result = results.into_iter().next().ok_or_else(|| {
                VectorDBError::Internal("Expected single result but found none".to_string())
            })?;
            return Ok(result);
        }

        match &self.strategy {
            AggregationStrategy::Merge(config) => self.merge_results(model, results, config),
            AggregationStrategy::Sum => self.sum_results(model, results),
            AggregationStrategy::Average => self.average_results(model, results),
            AggregationStrategy::TopK(k) => self.topk_results(model, results, *k),
            AggregationStrategy::UnionDedup => self.union_dedup_results(model, results),
            AggregationStrategy::Intersection => self.intersection_results(model, results),
        }
    }

    /// Merge results with optional deduplication and sorting
    fn merge_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
        config: &MergeConfig,
    ) -> Result<SubQueryResult> {
        let mut all_records: Vec<UnifiedRecord> =
            results.into_iter().flat_map(|r| r.records).collect();

        // Deduplicate by ID
        if config.deduplicate {
            let mut seen: HashMap<String, usize> = HashMap::new();
            let mut deduped: Vec<UnifiedRecord> = Vec::new();

            for record in all_records {
                if let Some(&existing_idx) = seen.get(&record.id) {
                    // Keep the one with higher score
                    if record.score > deduped[existing_idx].score {
                        deduped[existing_idx] = record;
                    }
                } else {
                    seen.insert(record.id.clone(), deduped.len());
                    deduped.push(record);
                }
            }

            all_records = deduped;
        }

        // Sort by score
        if config.sort_by_score {
            all_records.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Apply limit
        if let Some(limit) = config.limit {
            all_records.truncate(limit);
        }

        let count = all_records.len() as u64;
        Ok(SubQueryResult {
            source_model: model,
            records_returned: count,
            records: all_records,
            total_count: Some(count),
            execution_time_us: 0,
            records_scanned: count,
        })
    }

    /// Sum numeric values across results (for count aggregations)
    fn sum_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
    ) -> Result<SubQueryResult> {
        let total_count: u64 = results.iter().map(|r| r.records_returned).sum();
        let total_scanned: u64 = results.iter().map(|r| r.records_scanned).sum();

        // For sum, we create a single record with the total
        let sum_record = UnifiedRecord {
            id: "sum_result".to_string(),
            source_model: model,
            data: serde_json::json!({
                "total_count": total_count,
                "total_scanned": total_scanned,
            }),
            score: Some(total_count as f64),
            metadata: HashMap::new(),
        };

        Ok(SubQueryResult {
            source_model: model,
            records_returned: 1,
            records: vec![sum_record],
            total_count: Some(total_count),
            execution_time_us: 0,
            records_scanned: total_scanned,
        })
    }

    /// Average numeric values across results
    fn average_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
    ) -> Result<SubQueryResult> {
        let total_records: usize = results.iter().map(|r| r.records.len()).sum();
        let total_score: f64 = results
            .iter()
            .flat_map(|r| &r.records)
            .filter_map(|r| r.score)
            .sum();

        let average = if total_records > 0 {
            total_score / total_records as f64
        } else {
            0.0
        };

        let avg_record = UnifiedRecord {
            id: "avg_result".to_string(),
            source_model: model,
            data: serde_json::json!({
                "average": average,
                "count": total_records,
            }),
            score: Some(average),
            metadata: HashMap::new(),
        };

        Ok(SubQueryResult {
            source_model: model,
            records_returned: 1,
            records: vec![avg_record],
            total_count: Some(total_records as u64),
            execution_time_us: 0,
            records_scanned: total_records as u64,
        })
    }

    /// Get top-K results by score
    fn topk_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
        k: usize,
    ) -> Result<SubQueryResult> {
        let config = MergeConfig {
            deduplicate: true,
            sort_by_score: true,
            limit: Some(k),
        };
        self.merge_results(model, results, &config)
    }

    /// Union with deduplication
    fn union_dedup_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
    ) -> Result<SubQueryResult> {
        let config = MergeConfig {
            deduplicate: true,
            sort_by_score: false,
            limit: None,
        };
        self.merge_results(model, results, &config)
    }

    /// Intersection - keep only records that appear on all nodes
    fn intersection_results(
        &self,
        model: DataModel,
        results: Vec<SubQueryResult>,
    ) -> Result<SubQueryResult> {
        if results.is_empty() {
            return Ok(SubQueryResult::empty(model));
        }

        let num_sources = results.len();

        // Count occurrences of each ID
        let mut id_counts: HashMap<String, usize> = HashMap::new();
        let mut id_records: HashMap<String, UnifiedRecord> = HashMap::new();

        for result in results {
            for record in result.records {
                *id_counts.entry(record.id.clone()).or_insert(0) += 1;
                id_records.insert(record.id.clone(), record);
            }
        }

        // Keep only records that appear in all sources
        let intersection: Vec<UnifiedRecord> = id_counts
            .into_iter()
            .filter(|(_, count)| *count == num_sources)
            .filter_map(|(id, _)| id_records.remove(&id))
            .collect();

        let count = intersection.len() as u64;
        Ok(SubQueryResult {
            source_model: model,
            records_returned: count,
            records: intersection,
            total_count: Some(count),
            execution_time_us: 0,
            records_scanned: count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, score: Option<f64>) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: DataModel::Vector,
            data: serde_json::json!({}),
            score,
            metadata: HashMap::new(),
        }
    }

    fn make_result(records: Vec<UnifiedRecord>) -> SubQueryResult {
        let count = records.len() as u64;
        SubQueryResult {
            source_model: DataModel::Vector,
            records_returned: count,
            records,
            total_count: Some(count),
            execution_time_us: 0,
            records_scanned: count,
        }
    }

    #[test]
    fn test_aggregator_creation() {
        let aggregator = ResultAggregator::new(AggregationStrategy::default());
        assert!(matches!(aggregator.strategy, AggregationStrategy::Merge(_)));
    }

    #[test]
    fn test_merge_with_dedup() {
        let aggregator = ResultAggregator::new(AggregationStrategy::default());

        let local = vec![make_result(vec![
            make_record("a", Some(0.9)),
            make_record("b", Some(0.8)),
        ])];

        let remote = vec![make_result(vec![
            make_record("a", Some(0.85)), // Duplicate
            make_record("c", Some(0.7)),
        ])];

        let results = aggregator.aggregate(local, remote).unwrap();
        assert_eq!(results.len(), 1);

        let records = &results[0].records;
        assert_eq!(records.len(), 3); // a, b, c (deduplicated)

        // Should keep higher score for 'a'
        let a_record = records.iter().find(|r| r.id == "a").unwrap();
        assert_eq!(a_record.score, Some(0.9));
    }

    #[test]
    fn test_topk() {
        let aggregator = ResultAggregator::new(AggregationStrategy::TopK(2));

        let results = vec![
            make_result(vec![
                make_record("a", Some(0.9)),
                make_record("b", Some(0.5)),
            ]),
            make_result(vec![
                make_record("c", Some(0.8)),
                make_record("d", Some(0.3)),
            ]),
        ];

        let merged = aggregator.aggregate(results, Vec::new()).unwrap();
        assert_eq!(merged[0].records.len(), 2);
        assert_eq!(merged[0].records[0].id, "a"); // 0.9
        assert_eq!(merged[0].records[1].id, "c"); // 0.8
    }

    #[test]
    fn test_sum() {
        let aggregator = ResultAggregator::new(AggregationStrategy::Sum);

        let results = vec![
            SubQueryResult {
                source_model: DataModel::Vector,
                records_returned: 10,
                records: Vec::new(),
                total_count: Some(10),
                execution_time_us: 0,
                records_scanned: 100,
            },
            SubQueryResult {
                source_model: DataModel::Vector,
                records_returned: 15,
                records: Vec::new(),
                total_count: Some(15),
                execution_time_us: 0,
                records_scanned: 150,
            },
        ];

        let merged = aggregator.aggregate(results, Vec::new()).unwrap();
        assert_eq!(merged[0].total_count, Some(25));
        assert_eq!(merged[0].records_scanned, 250);
    }

    #[test]
    fn test_intersection() {
        let aggregator = ResultAggregator::new(AggregationStrategy::Intersection);

        let local = vec![make_result(vec![
            make_record("a", Some(0.9)),
            make_record("b", Some(0.8)),
            make_record("c", Some(0.7)),
        ])];

        let remote = vec![make_result(vec![
            make_record("a", Some(0.85)),
            make_record("c", Some(0.75)),
            make_record("d", Some(0.6)),
        ])];

        let results = aggregator.aggregate(local, remote).unwrap();
        let records = &results[0].records;

        // Only 'a' and 'c' appear in both
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.id == "a"));
        assert!(records.iter().any(|r| r.id == "c"));
    }

    #[test]
    fn test_empty_results() {
        let aggregator = ResultAggregator::new(AggregationStrategy::default());

        let results = aggregator.aggregate(Vec::new(), Vec::new()).unwrap();
        assert!(results.is_empty());
    }
}
