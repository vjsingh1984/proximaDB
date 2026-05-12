//! Pure cross-model record join helpers for the shared query runtime.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use proximadb_multimodel_query::{
    BlockBatchConfig, ComponentDependency, DataModel, JoinType, SemanticJoinMode,
};

use crate::{SubQueryResult, UnifiedRecord};

/// Result of a join operation.
#[derive(Debug, Clone)]
pub struct JoinResult {
    /// Records that matched the join condition.
    pub matched: Vec<UnifiedRecord>,
    /// Records from the left side that did not match.
    pub unmatched_left: Vec<UnifiedRecord>,
    /// Whether the join found any matches.
    pub has_matches: bool,
}

impl JoinResult {
    /// Create an empty join result.
    pub fn empty() -> Self {
        Self {
            matched: Vec::new(),
            unmatched_left: Vec::new(),
            has_matches: false,
        }
    }

    /// Convert to `SubQueryResult` based on join type.
    pub fn to_subquery_result(
        self,
        source_model: DataModel,
        join_type: &JoinType,
    ) -> SubQueryResult {
        let records = match join_type {
            JoinType::Inner | JoinType::Semi | JoinType::Semantic { .. } => self.matched,
            JoinType::LeftOuter => {
                let mut all = self.matched;
                all.extend(self.unmatched_left);
                all
            }
            JoinType::Anti => self.unmatched_left,
        };

        let count = records.len() as u64;
        SubQueryResult {
            source_model,
            records_returned: count,
            records,
            total_count: Some(count),
            execution_time_us: 0,
            records_scanned: count,
        }
    }
}

/// Narrow join-execution seam for orchestration helpers.
#[async_trait]
pub trait JoinExecutionService: Send + Sync {
    /// Execute a join for one dependency edge.
    async fn execute_join(
        &self,
        left: &[UnifiedRecord],
        right: &[UnifiedRecord],
        dependency: &ComponentDependency,
    ) -> JoinResult;
}

/// Narrow similarity seam for vector-based semantic joins.
pub trait RecordSimilarityEngine {
    /// Return similarity in the range `[0.0, 1.0]`, where higher is better.
    fn similarity(&self, left: &[f32], right: &[f32]) -> f32;
}

/// Narrow seam for the platform-bound block-batch semantic join path.
#[async_trait]
pub trait BlockBatchSemanticJoinService: Send + Sync {
    /// Execute the block-batch semantic join path for an already-selected mode.
    async fn execute_block_batch_join(
        &self,
        left: &[UnifiedRecord],
        right: &[UnifiedRecord],
        join_field: &str,
        top_k: u32,
        config: &BlockBatchConfig,
    ) -> JoinResult;
}

/// Execute a structured join between two result sets.
pub fn execute_exact_join(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    dependency: &ComponentDependency,
) -> JoinResult {
    let join_field = &dependency.join_field;
    let right_index: HashMap<String, Vec<&UnifiedRecord>> = build_join_index(right, join_field);

    let mut matched = Vec::new();
    let mut unmatched_left = Vec::new();

    for left_record in left {
        let left_values = extract_join_values(left_record, join_field);

        let mut found_match = false;
        for left_value in &left_values {
            if let Some(right_records) = right_index.get(left_value) {
                found_match = true;

                match dependency.join_type {
                    JoinType::Inner | JoinType::LeftOuter => {
                        for right_record in right_records {
                            let combined = merge_records(left_record, right_record, join_field);
                            matched.push(combined);
                        }
                    }
                    JoinType::Semi => {
                        matched.push(left_record.clone());
                        break;
                    }
                    JoinType::Anti | JoinType::Semantic { .. } => {}
                }
            }
        }

        if !found_match {
            match dependency.join_type {
                JoinType::LeftOuter => {
                    unmatched_left.push(left_record.clone());
                }
                JoinType::Anti => {
                    matched.push(left_record.clone());
                }
                _ => {}
            }
        }
    }

    let has_matches = !matched.is_empty();
    JoinResult {
        matched,
        unmatched_left,
        has_matches,
    }
}

/// Execute a join using extracted query-runtime dispatch plus caller-supplied
/// similarity and block-batch services.
pub async fn execute_join_with_services<S, B>(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    dependency: &ComponentDependency,
    similarity_engine: &S,
    block_batch_service: &B,
) -> JoinResult
where
    S: RecordSimilarityEngine + ?Sized,
    B: BlockBatchSemanticJoinService + ?Sized,
{
    let join_field = &dependency.join_field;
    match &dependency.join_type {
        JoinType::Semantic {
            threshold,
            top_k,
            mode,
        } => {
            dispatch_semantic_join_with_service(
                left,
                right,
                join_field,
                *threshold,
                *top_k,
                mode,
                similarity_engine,
                block_batch_service,
            )
            .await
        }
        _ => execute_exact_join(left, right, dependency),
    }
}

/// Filter records by IDs from a prior component.
pub fn filter_by_ids(
    records: &[UnifiedRecord],
    prior_ids: &[String],
    include: bool,
) -> Vec<UnifiedRecord> {
    let id_set: HashSet<&str> = prior_ids.iter().map(|s| s.as_str()).collect();

    records
        .iter()
        .filter(|r| {
            let in_set = id_set.contains(r.id.as_str());
            if include { in_set } else { !in_set }
        })
        .cloned()
        .collect()
}

/// Extract a vector from a record for semantic joins.
pub fn extract_record_vector(record: &UnifiedRecord, field: &str) -> Option<Vec<f32>> {
    if let Some(val) = record.data.get(field)
        && let Some(arr) = val.as_array()
    {
        let vec: Vec<f32> = arr
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();
        if !vec.is_empty() {
            return Some(vec);
        }
    }

    if let Some(val) = record.metadata.get(field) {
        let vec: Vec<f32> = val
            .split(',')
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .collect();
        if !vec.is_empty() {
            return Some(vec);
        }
    }

    None
}

/// Resolve a component's record IDs from prior results, if available.
pub fn resolve_component_record_ids(
    component_idx: usize,
    context: Option<&HashMap<usize, &SubQueryResult>>,
) -> Vec<String> {
    let Some(ctx) = context else {
        return Vec::new();
    };

    let Some(prior_result) = ctx.get(&component_idx) else {
        return Vec::new();
    };

    prior_result.records.iter().map(|r| r.id.clone()).collect()
}

/// Execute multiple joins in sequence for components with multiple dependencies.
pub async fn execute_multi_join_with<J>(
    component_result: &SubQueryResult,
    dependencies: &[ComponentDependency],
    prior_results: &HashMap<usize, &SubQueryResult>,
    join_service: &J,
) -> SubQueryResult
where
    J: JoinExecutionService + ?Sized,
{
    if dependencies.is_empty() {
        return component_result.clone();
    }

    let mut current_records = component_result.records.clone();
    let source_model = component_result.source_model;

    for dep in dependencies {
        if let Some(prior) = prior_results.get(&dep.component_index) {
            let join_result = join_service
                .execute_join(&current_records, &prior.records, dep)
                .await;
            current_records = join_result
                .to_subquery_result(source_model, &dep.join_type)
                .records;
        }
    }

    let count = current_records.len() as u64;
    SubQueryResult {
        source_model,
        records_returned: count,
        records: current_records,
        total_count: Some(count),
        execution_time_us: component_result.execution_time_us,
        records_scanned: component_result.records_scanned,
    }
}

/// Execute a semantic join using a caller-supplied similarity engine.
pub fn execute_semantic_join_with<S>(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    join_field: &str,
    threshold: f32,
    top_k: u32,
    similarity_engine: &S,
) -> JoinResult
where
    S: RecordSimilarityEngine + ?Sized,
{
    let mut matched = Vec::new();
    let mut unmatched_left = Vec::new();
    let mut has_matches = false;

    let right_vectors: Vec<(usize, Vec<f32>)> = right
        .iter()
        .enumerate()
        .filter_map(|(i, r)| extract_record_vector(r, join_field).map(|v| (i, v)))
        .collect();

    if right_vectors.is_empty() {
        return JoinResult {
            matched: Vec::new(),
            unmatched_left: left.to_vec(),
            has_matches: false,
        };
    }

    for left_record in left {
        if let Some(left_vec) = extract_record_vector(left_record, join_field) {
            let mut matches: Vec<(f32, &UnifiedRecord)> = Vec::new();

            for (right_idx, right_vec) in &right_vectors {
                let similarity = similarity_engine.similarity(&left_vec, right_vec);
                if similarity >= threshold {
                    matches.push((similarity, &right[*right_idx]));
                }
            }

            if !matches.is_empty() {
                has_matches = true;
                matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

                for (_, right_record) in matches.iter().take(top_k as usize) {
                    matched.push(merge_semantic_records(left_record, right_record));
                }
            } else {
                unmatched_left.push(left_record.clone());
            }
        } else {
            unmatched_left.push(left_record.clone());
        }
    }

    JoinResult {
        matched,
        unmatched_left,
        has_matches,
    }
}

/// Dispatch a semantic join to cosine or block-batch mode behind extracted traits.
#[allow(
    clippy::too_many_arguments,
    reason = "public dispatcher keeps mode, engines, and join parameters explicit"
)]
pub async fn dispatch_semantic_join_with_service<S, B>(
    left: &[UnifiedRecord],
    right: &[UnifiedRecord],
    join_field: &str,
    threshold: f32,
    top_k: u32,
    mode: &SemanticJoinMode,
    similarity_engine: &S,
    block_batch_service: &B,
) -> JoinResult
where
    S: RecordSimilarityEngine + ?Sized,
    B: BlockBatchSemanticJoinService + ?Sized,
{
    match mode {
        SemanticJoinMode::Cosine => {
            execute_semantic_join_with(left, right, join_field, threshold, top_k, similarity_engine)
        }
        SemanticJoinMode::LlmBlockBatch(config) => {
            block_batch_service
                .execute_block_batch_join(left, right, join_field, top_k, config)
                .await
        }
    }
}

/// Build a hash index of records by join field value.
pub fn build_join_index<'a>(
    records: &'a [UnifiedRecord],
    join_field: &str,
) -> HashMap<String, Vec<&'a UnifiedRecord>> {
    let mut index: HashMap<String, Vec<&UnifiedRecord>> = HashMap::new();

    for record in records {
        let values = extract_join_values(record, join_field);
        for value in values {
            index.entry(value).or_default().push(record);
        }
    }

    index
}

/// Extract join field value(s) from a record.
pub fn extract_join_values(record: &UnifiedRecord, join_field: &str) -> Vec<String> {
    let mut values = Vec::new();

    if join_field == "id" {
        values.push(record.id.clone());
        return values;
    }

    if let Some(val) = extract_from_json(&record.data, join_field) {
        values.push(val);
        return values;
    }

    if let Some(val) = record.metadata.get(join_field) {
        values.push(val.clone());
        return values;
    }

    if join_field.contains('.') {
        let parts: Vec<&str> = join_field.split('.').collect();
        if let Some(val) = extract_nested_json(&record.data, &parts) {
            values.push(val);
        }
    }

    values
}

/// Merge two records from different models into one.
pub fn merge_records(
    left: &UnifiedRecord,
    right: &UnifiedRecord,
    join_field: &str,
) -> UnifiedRecord {
    let mut merged_data = serde_json::Map::new();

    let left_key = format!("{}", left.source_model);
    merged_data.insert(left_key, left.data.clone());

    let right_key = format!("{}", right.source_model);
    merged_data.insert(right_key, right.data.clone());

    merged_data.insert(
        "join_field".to_string(),
        serde_json::Value::String(join_field.to_string()),
    );

    let mut merged_metadata = left.metadata.clone();
    for (k, v) in &right.metadata {
        merged_metadata
            .entry(format!("{}_{}", right.source_model, k))
            .or_insert_with(|| v.clone());
    }

    merged_metadata.insert("right_id".to_string(), right.id.clone());

    let merged_score = match (left.score, right.score) {
        (Some(l), Some(r)) => Some((l + r) / 2.0),
        (Some(s), None) | (None, Some(s)) => Some(s),
        (None, None) => None,
    };

    UnifiedRecord {
        id: left.id.clone(),
        source_model: left.source_model,
        data: serde_json::Value::Object(merged_data),
        score: merged_score,
        metadata: merged_metadata,
    }
}

fn extract_from_json(data: &serde_json::Value, field: &str) -> Option<String> {
    match data.get(field) {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        Some(serde_json::Value::Array(arr)) => arr.first().and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }),
        _ => None,
    }
}

fn extract_nested_json(data: &serde_json::Value, path_parts: &[&str]) -> Option<String> {
    if path_parts.is_empty() {
        return None;
    }

    let mut current = data;
    for part in path_parts {
        current = current.get(*part)?;
    }

    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn merge_semantic_records(left: &UnifiedRecord, right: &UnifiedRecord) -> UnifiedRecord {
    let mut joined = left.clone();
    if let Some(obj) = joined.data.as_object_mut()
        && let Some(right_obj) = right.data.as_object()
    {
        for (k, v) in right_obj {
            if !obj.contains_key(k) {
                obj.insert(k.clone(), v.clone());
            } else {
                obj.insert(format!("right_{}", k), v.clone());
            }
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_multimodel_query::{BlockBatchConfig, DataModel, JoinType, SemanticJoinMode};
    fn make_record(id: &str, model: DataModel, data: serde_json::Value) -> UnifiedRecord {
        UnifiedRecord {
            id: id.to_string(),
            source_model: model,
            data,
            score: Some(1.0),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn extract_join_values_supports_id_json_and_nested_paths() {
        let record = make_record(
            "r1",
            DataModel::Document,
            serde_json::json!({
                "user_id": "u1",
                "metadata": { "customer_id": "c1" }
            }),
        );

        assert_eq!(extract_join_values(&record, "id"), vec!["r1".to_string()]);
        assert_eq!(
            extract_join_values(&record, "user_id"),
            vec!["u1".to_string()]
        );
        assert_eq!(
            extract_join_values(&record, "metadata.customer_id"),
            vec!["c1".to_string()]
        );
    }

    #[test]
    fn merge_records_combines_model_data() {
        let left = make_record(
            "l1",
            DataModel::Document,
            serde_json::json!({"name": "left"}),
        );
        let right = make_record("r1", DataModel::Vector, serde_json::json!({"score": 0.9}));

        let merged = merge_records(&left, &right, "id");
        assert_eq!(merged.id, "l1");
        assert!(merged.data.get("document").is_some());
        assert!(merged.data.get("vector").is_some());
        assert_eq!(
            merged.metadata.get("right_id").map(String::as_str),
            Some("r1")
        );
    }

    #[test]
    fn exact_join_matches_by_field() {
        let left = vec![make_record(
            "l1",
            DataModel::Document,
            serde_json::json!({"user_id": "u1"}),
        )];
        let right = vec![make_record(
            "r1",
            DataModel::Vector,
            serde_json::json!({"user_id": "u1"}),
        )];
        let dep = ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::Inner,
        };

        let result = execute_exact_join(&left, &right, &dep);
        assert!(result.has_matches);
        assert_eq!(result.matched.len(), 1);
    }

    struct ExactJoinExecutor;

    #[async_trait]
    impl JoinExecutionService for ExactJoinExecutor {
        async fn execute_join(
            &self,
            left: &[UnifiedRecord],
            right: &[UnifiedRecord],
            dependency: &ComponentDependency,
        ) -> JoinResult {
            execute_exact_join(left, right, dependency)
        }
    }

    #[test]
    fn filter_by_ids_supports_include_and_exclude() {
        let records = vec![
            make_record("a", DataModel::Document, serde_json::json!({})),
            make_record("b", DataModel::Document, serde_json::json!({})),
        ];
        let ids = vec!["a".to_string()];

        assert_eq!(filter_by_ids(&records, &ids, true).len(), 1);
        assert_eq!(filter_by_ids(&records, &ids, false).len(), 1);
    }

    struct DotLikeSimilarity;

    impl RecordSimilarityEngine for DotLikeSimilarity {
        fn similarity(&self, left: &[f32], right: &[f32]) -> f32 {
            left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
        }
    }

    struct MockBlockBatchService;

    #[async_trait]
    impl BlockBatchSemanticJoinService for MockBlockBatchService {
        async fn execute_block_batch_join(
            &self,
            left: &[UnifiedRecord],
            _right: &[UnifiedRecord],
            _join_field: &str,
            _top_k: u32,
            _config: &BlockBatchConfig,
        ) -> JoinResult {
            JoinResult {
                matched: left.to_vec(),
                unmatched_left: Vec::new(),
                has_matches: true,
            }
        }
    }

    #[test]
    fn resolve_component_record_ids_returns_ids_or_empty() {
        let result = SubQueryResult {
            source_model: DataModel::Document,
            records: vec![make_record("a", DataModel::Document, serde_json::json!({}))],
            total_count: Some(1),
            execution_time_us: 0,
            records_scanned: 1,
            records_returned: 1,
        };
        let mut context = HashMap::new();
        context.insert(2usize, &result);

        assert_eq!(
            resolve_component_record_ids(2, Some(&context)),
            vec!["a".to_string()]
        );
        assert!(resolve_component_record_ids(3, Some(&context)).is_empty());
        assert!(resolve_component_record_ids(2, None).is_empty());
    }

    #[tokio::test]
    async fn execute_multi_join_with_sequences_dependencies() {
        let vector_result = SubQueryResult {
            source_model: DataModel::Vector,
            records: vec![make_record(
                "v1",
                DataModel::Vector,
                serde_json::json!({"user_id": "u1"}),
            )],
            total_count: Some(1),
            execution_time_us: 1,
            records_scanned: 1,
            records_returned: 1,
        };
        let doc_result = SubQueryResult {
            source_model: DataModel::Document,
            records: vec![make_record(
                "d1",
                DataModel::Document,
                serde_json::json!({"user_id": "u1"}),
            )],
            total_count: Some(1),
            execution_time_us: 2,
            records_scanned: 1,
            records_returned: 1,
        };
        let dependencies = vec![ComponentDependency {
            component_index: 0,
            join_field: "user_id".to_string(),
            join_type: JoinType::Inner,
        }];
        let mut prior_results = HashMap::new();
        prior_results.insert(0usize, &vector_result);

        let joined = execute_multi_join_with(
            &doc_result,
            &dependencies,
            &prior_results,
            &ExactJoinExecutor,
        )
        .await;

        assert_eq!(joined.records.len(), 1);
        assert_eq!(joined.records_returned, 1);
    }

    #[test]
    fn semantic_join_with_matches_and_merges_ranked_records() {
        let left = vec![make_record(
            "user1",
            DataModel::Document,
            serde_json::json!({"name": "Alice", "vec": [1.0, 0.0, 0.0]}),
        )];
        let right = vec![
            make_record(
                "prod1",
                DataModel::Vector,
                serde_json::json!({"product": "Red Apple", "vec": [0.9, 0.1, 0.0]}),
            ),
            make_record(
                "prod2",
                DataModel::Vector,
                serde_json::json!({"product": "Green Broccoli", "vec": [0.1, 0.9, 0.0]}),
            ),
        ];

        let result = execute_semantic_join_with(&left, &right, "vec", 0.5, 1, &DotLikeSimilarity);
        assert!(result.has_matches);
        assert_eq!(result.matched.len(), 1);
        assert_eq!(result.matched[0].id, "user1");
        assert_eq!(result.matched[0].data["product"], "Red Apple");
    }

    #[test]
    fn extract_record_vector_supports_json_and_metadata() {
        let json_record = make_record(
            "a",
            DataModel::Vector,
            serde_json::json!({"vec": [1.0, 2.0]}),
        );
        assert_eq!(
            extract_record_vector(&json_record, "vec"),
            Some(vec![1.0, 2.0])
        );

        let mut metadata_record = make_record("b", DataModel::Vector, serde_json::json!({}));
        metadata_record
            .metadata
            .insert("vec".to_string(), "3.0, 4.0".to_string());
        assert_eq!(
            extract_record_vector(&metadata_record, "vec"),
            Some(vec![3.0, 4.0])
        );
    }

    #[tokio::test]
    async fn execute_join_with_services_routes_semantic_modes() {
        let left = vec![make_record(
            "user1",
            DataModel::Document,
            serde_json::json!({"vec": [1.0, 0.0]}),
        )];
        let right = vec![make_record(
            "prod1",
            DataModel::Vector,
            serde_json::json!({"vec": [1.0, 0.0]}),
        )];

        let cosine = execute_join_with_services(
            &left,
            &right,
            &ComponentDependency {
                component_index: 0,
                join_field: "vec".to_string(),
                join_type: JoinType::Semantic {
                    threshold: 0.5,
                    top_k: 1,
                    mode: SemanticJoinMode::Cosine,
                },
            },
            &DotLikeSimilarity,
            &MockBlockBatchService,
        )
        .await;
        assert!(cosine.has_matches);

        let block_batch = execute_join_with_services(
            &left,
            &right,
            &ComponentDependency {
                component_index: 0,
                join_field: "vec".to_string(),
                join_type: JoinType::Semantic {
                    threshold: 0.5,
                    top_k: 1,
                    mode: SemanticJoinMode::LlmBlockBatch(BlockBatchConfig::default()),
                },
            },
            &DotLikeSimilarity,
            &MockBlockBatchService,
        )
        .await;
        assert_eq!(block_batch.matched.len(), 1);
        assert_eq!(block_batch.matched[0].id, "user1");
    }
}
