// Document aggregation engine
//
// Implements MongoDB-like aggregation pipeline for document collections:
// - Match stage: Filter documents
// - Group stage: GROUP BY with aggregation operations (COUNT, SUM, AVG, MIN, MAX, etc.)
// - Project stage: Field projection
// - Sort stage: Sorting results
// - Limit/Skip stages: Pagination
// - Lookup stage: Left outer join with another collection
// - Unwind stage: Array expansion
// - Full-text search scoring
//
// TD-106 Group B / Slice 5: the internal working set is the canonical
// `ProximaTree` (NF² property tree of `ProximaValue` leaves). The legacy proto
// `SqlObject` survives only at the three outermost edges:
//   * input edge   — `execute()` lifts `DocumentRecord.document` once via
//     `sql_object_to_proxima_tree` (removed in Group C when the record carries props);
//   * output edge  — `execute()` lowers the final tree to `SqlObject` via
//     `proxima_tree_to_sql_object` (until callers migrate to canonical results);
//   * filter edge  — `process_match` rebuilds a temp `DocumentRecord` for the
//     still-`SqlObject`-typed `FilterEvaluator`;
//   * lookup edge  — `process_lookup` bridges to the still-`SqlObject` cross-collection
//     `LookupFetcher` join boundary.
// No per-stage conversion happens between these edges.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use jsonpath_rust::JsonPathQuery;
use proximadb_data_model::ProximaValue;
use proximadb_records::conversions::json_to_proxima;
use proximadb_records::{ProximaTree, ProximaTreeNode};
use serde_json::Value as JsonValue;
use tracing::debug;

use crate::core::search::sql_value_filter::proxima_tree_to_json_map;
use crate::proto::proximadb_v1::{
    Aggregation, AggregationStage, AggregationType, DocumentFilter, GroupStage, LimitStage,
    LookupStage, MatchStage, ProjectStage, SkipStage, SortOrder, SortStage, SqlObject, UnwindStage,
    aggregation_stage::Stage,
};

#[cfg(test)]
use crate::proto::proximadb_v1::SortField;

use super::DocumentRecord;
use super::aggregation_extensions::{LookupConfig, LookupFetcher, execute_lookup};
use super::canonical_adapter::{proxima_tree_to_sql_object, sql_object_to_proxima_tree};
use super::query::filter::FilterEvaluator;

/// Aggregation pipeline executor
pub struct AggregationExecutor {
    filter_evaluator: FilterEvaluator,
}

impl AggregationExecutor {
    /// Create a new aggregation executor
    pub fn new() -> Self {
        Self {
            filter_evaluator: FilterEvaluator::new(),
        }
    }

    /// Execute an aggregation pipeline on a set of documents.
    ///
    /// The pipeline runs over the canonical `ProximaTree` working set; conversion
    /// happens only at the input edge (here) and the output edge (the final lower
    /// to `SqlObject` for the public result contract).
    pub fn execute(
        &self,
        documents: Vec<DocumentRecord>,
        filter: Option<&DocumentFilter>,
        pipeline: &[AggregationStage],
    ) -> Result<Vec<SqlObject>> {
        // Start with filtered documents if filter is provided. Slice 6: the record
        // carries the canonical `props` tree, so the input edge is a clone (no
        // per-record SqlObject conversion).
        let mut working_set: Vec<ProximaTree> = if let Some(f) = filter {
            documents
                .into_iter()
                .filter(|doc| self.filter_evaluator.evaluate(f, doc))
                .map(|doc| doc.props)
                .collect()
        } else {
            documents.into_iter().map(|doc| doc.props).collect()
        };

        debug!(
            "Aggregation starting with {} documents, {} pipeline stages",
            working_set.len(),
            pipeline.len()
        );

        // Process each pipeline stage
        for (stage_idx, stage) in pipeline.iter().enumerate() {
            working_set = self.process_stage(&working_set, stage, stage_idx)?;
            debug!("After stage {}: {} documents", stage_idx, working_set.len());
        }

        // Output edge: lower the canonical working set back to the proto row shape.
        Ok(working_set.iter().map(proxima_tree_to_sql_object).collect())
    }

    /// Process a single pipeline stage (public for external use with lookups).
    ///
    /// Operates entirely on the canonical `ProximaTree` working set.
    pub fn process_stage(
        &self,
        documents: &[ProximaTree],
        stage: &AggregationStage,
        stage_idx: usize,
    ) -> Result<Vec<ProximaTree>> {
        match &stage.stage {
            Some(Stage::Match(match_stage)) => self.process_match(documents, match_stage),
            Some(Stage::Group(group_stage)) => self.process_group(documents, group_stage),
            Some(Stage::Project(project_stage)) => self.process_project(documents, project_stage),
            Some(Stage::Sort(sort_stage)) => self.process_sort(documents, sort_stage),
            Some(Stage::Limit(limit_stage)) => self.process_limit(documents, limit_stage),
            Some(Stage::Skip(skip_stage)) => self.process_skip(documents, skip_stage),
            Some(Stage::Lookup(_lookup_stage)) => {
                // For now, return an error - lookup requires a fetcher callback
                // In production, this would be passed through the executor context
                Err(anyhow!(
                    "Lookup stage requires document fetcher - use DocumentService::aggregate_with_lookup instead"
                ))
            }
            Some(Stage::Unwind(unwind_stage)) => self.process_unwind(documents, unwind_stage),
            None => Err(anyhow!("Empty stage at index {}", stage_idx)),
        }
    }

    // =========================================================================
    // MATCH STAGE
    // =========================================================================

    /// Process a match (filter) stage
    fn process_match(
        &self,
        documents: &[ProximaTree],
        match_stage: &MatchStage,
    ) -> Result<Vec<ProximaTree>> {
        let filter = match &match_stage.filter {
            Some(f) => f,
            None => return Ok(documents.to_vec()),
        };

        Ok(documents
            .iter()
            .filter(|doc| {
                // The filter reads the canonical `props` tree; the temp record
                // carries the working tree directly (TD-106 Slice 7).
                let record = DocumentRecord {
                    id: String::new(),
                    props: (*doc).clone(),
                    version: 0,
                    collection_id: String::new(),
                    updated_at_ns: 0,
                    schema_id: None,
                    document_type: None,
                };
                self.filter_evaluator.evaluate(filter, &record)
            })
            .cloned()
            .collect())
    }

    // =========================================================================
    // GROUP STAGE - GROUP BY with aggregation operations
    // =========================================================================

    /// Process a group stage (GROUP BY with aggregations)
    fn process_group(
        &self,
        documents: &[ProximaTree],
        group_stage: &GroupStage,
    ) -> Result<Vec<ProximaTree>> {
        let group_key = &group_stage.key;

        // Group documents by key
        let mut groups: HashMap<String, Vec<&ProximaTree>> = HashMap::new();

        for doc in documents {
            let key_value = if group_key == "_id" || group_key.is_empty() {
                // Group all documents together
                "_all".to_string()
            } else {
                // Extract key value using JSON path
                self.extract_group_key(doc, group_key)
            };

            groups.entry(key_value).or_default().push(doc);
        }

        // Compute aggregations for each group. The working set is canonical
        // (`ProximaTree`/`ProximaValue`) end-to-end — TD-106 Slice 5.
        let mut results = Vec::with_capacity(groups.len());
        for (key, group_docs) in groups {
            let mut result_doc = ProximaTree::new();

            // Add the group key to result (as _id field, MongoDB style)
            if group_key != "_id" && !group_key.is_empty() {
                result_doc.insert(
                    "_id".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String(key.clone())),
                );
            }

            for agg in &group_stage.aggregations {
                let agg_value = self.compute_aggregation(&group_docs, agg)?;
                result_doc.insert(agg.output_field.clone(), ProximaTreeNode::Value(agg_value));
            }

            results.push(result_doc);
        }

        Ok(results)
    }

    /// Extract group key from document
    fn extract_group_key(&self, doc: &ProximaTree, path: &str) -> String {
        let json_doc = self.tree_to_json(doc);
        let normalized_path = self.normalize_path(path);

        match json_doc.path(&normalized_path) {
            Ok(result) => match &result {
                JsonValue::Array(arr) if arr.len() == 1 => self.json_value_to_string(&arr[0]),
                JsonValue::Array(arr) if arr.is_empty() => "_null".to_string(),
                JsonValue::Null => "_null".to_string(),
                _ => self.json_value_to_string(&result),
            },
            Err(_) => "_null".to_string(),
        }
    }

    /// Compute a single aggregation over a group of documents.
    ///
    /// The accumulator kernel produces canonical `ProximaValue` (TD-106 Slice 3).
    fn compute_aggregation(
        &self,
        docs: &[&ProximaTree],
        agg: &Aggregation,
    ) -> Result<ProximaValue> {
        let agg_type =
            AggregationType::try_from(agg.r#type).unwrap_or(AggregationType::Unspecified);
        let path = &agg.input_path;

        match agg_type {
            AggregationType::Count => self.agg_count(docs, path),
            AggregationType::Sum => self.agg_sum(docs, path),
            AggregationType::Avg => self.agg_avg(docs, path),
            AggregationType::Min => self.agg_min(docs, path),
            AggregationType::Max => self.agg_max(docs, path),
            AggregationType::First => self.agg_first(docs, path),
            AggregationType::Last => self.agg_last(docs, path),
            AggregationType::Push => self.agg_push(docs, path),
            AggregationType::AddToSet => self.agg_add_to_set(docs, path),
            AggregationType::Unspecified => Err(anyhow!("Unspecified aggregation type")),
        }
    }

    /// COUNT aggregation - counts documents (or non-null values if path specified)
    fn agg_count(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let count = if path.is_empty() || path == "*" {
            // Count all documents
            docs.len() as i64
        } else {
            // Count documents with non-null values at path
            docs.iter()
                .filter(|doc| self.extract_value(doc, path).is_some())
                .count() as i64
        };

        Ok(ProximaValue::Int64(count))
    }

    /// SUM aggregation
    fn agg_sum(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let mut sum = 0.0;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                sum += val;
            }
        }

        Ok(ProximaValue::Float64(sum))
    }

    /// AVG aggregation
    fn agg_avg(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let mut sum = 0.0;
        let mut count = 0;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                sum += val;
                count += 1;
            }
        }

        let avg = if count > 0 { sum / count as f64 } else { 0.0 };

        Ok(ProximaValue::Float64(avg))
    }

    /// MIN aggregation
    fn agg_min(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let mut min_val: Option<f64> = None;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                min_val = Some(min_val.map_or(val, |m| m.min(val)));
            }
        }

        Ok(min_val.map_or(ProximaValue::Null, ProximaValue::Float64))
    }

    /// MAX aggregation
    fn agg_max(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let mut max_val: Option<f64> = None;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                max_val = Some(max_val.map_or(val, |m| m.max(val)));
            }
        }

        Ok(max_val.map_or(ProximaValue::Null, ProximaValue::Float64))
    }

    /// FIRST aggregation - returns first value in group
    fn agg_first(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        for doc in docs {
            if let Some(val) = self.extract_value(doc, path) {
                return Ok(val);
            }
        }

        Ok(ProximaValue::Null)
    }

    /// LAST aggregation - returns last value in group
    fn agg_last(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        for doc in docs.iter().rev() {
            if let Some(val) = self.extract_value(doc, path) {
                return Ok(val);
            }
        }

        Ok(ProximaValue::Null)
    }

    /// PUSH aggregation - collects all values into an array
    fn agg_push(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        let values: Vec<ProximaValue> = docs
            .iter()
            .filter_map(|doc| self.extract_value(doc, path))
            .collect();

        Ok(ProximaValue::Array(values))
    }

    /// ADD_TO_SET aggregation - collects unique values into an array
    fn agg_add_to_set(&self, docs: &[&ProximaTree], path: &str) -> Result<ProximaValue> {
        // Dedup on the canonical values (first-seen ordering preserved).
        let mut seen: Vec<ProximaValue> = Vec::new();

        for doc in docs {
            if let Some(val) = self.extract_value(doc, path)
                && !seen.contains(&val)
            {
                seen.push(val);
            }
        }

        Ok(ProximaValue::Array(seen))
    }

    // =========================================================================
    // PROJECT STAGE
    // =========================================================================

    /// Process a project stage
    fn process_project(
        &self,
        documents: &[ProximaTree],
        project_stage: &ProjectStage,
    ) -> Result<Vec<ProximaTree>> {
        Ok(documents
            .iter()
            .map(|doc| self.project_document(doc, project_stage))
            .collect())
    }

    /// Project a single document
    fn project_document(&self, doc: &ProximaTree, project_stage: &ProjectStage) -> ProximaTree {
        let mut result = ProximaTree::new();

        // Handle field inclusion/exclusion
        let has_inclusions = project_stage.fields.values().any(|&v| v);
        let has_exclusions = project_stage.fields.values().any(|&v| !v);

        if has_inclusions {
            // Include mode: only include specified fields
            for (field, &include) in &project_stage.fields {
                if include && let Some(node) = doc.get(field) {
                    result.insert(field.clone(), node.clone());
                }
            }
        } else if has_exclusions {
            // Exclude mode: include all except specified fields
            for (key, node) in doc {
                if !project_stage.fields.get(key).copied().unwrap_or(true) {
                    continue; // Skip excluded fields
                }
                result.insert(key.clone(), node.clone());
            }
        } else {
            // No field specification - keep all fields
            result = doc.clone();
        }

        // Handle computed fields (canonical `ProximaValue` end-to-end).
        for (output_field, expression) in &project_stage.computed {
            if let Some(val) = self.evaluate_expression(doc, expression) {
                result.insert(output_field.clone(), ProximaTreeNode::Value(val));
            }
        }

        result
    }

    /// Evaluate a computed field expression
    fn evaluate_expression(&self, doc: &ProximaTree, expression: &str) -> Option<ProximaValue> {
        // Simple implementation: treat expression as a JSON path
        self.extract_value(doc, expression)
    }

    // =========================================================================
    // SORT STAGE
    // =========================================================================

    /// Process a sort stage
    fn process_sort(
        &self,
        documents: &[ProximaTree],
        sort_stage: &SortStage,
    ) -> Result<Vec<ProximaTree>> {
        let mut sorted = documents.to_vec();

        sorted.sort_by(|a, b| {
            for field in &sort_stage.fields {
                let order = SortOrder::try_from(field.order).unwrap_or(SortOrder::Asc);
                let cmp = self.compare_by_path(a, b, &field.path);
                let cmp = match order {
                    SortOrder::Desc => cmp.reverse(),
                    _ => cmp,
                };
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(sorted)
    }

    /// Compare two documents by a JSON path (canonical `ProximaValue` ordering).
    fn compare_by_path(&self, a: &ProximaTree, b: &ProximaTree, path: &str) -> std::cmp::Ordering {
        let val_a = self.extract_value(a, path);
        let val_b = self.extract_value(b, path);

        match (val_a, val_b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => self.compare_proxima_values(&a, &b),
        }
    }

    /// Compare two `ProximaValue` instances for ordering (cross-type numeric aware).
    fn compare_proxima_values(&self, a: &ProximaValue, b: &ProximaValue) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (a, b) {
            (ProximaValue::Null, ProximaValue::Null) => Ordering::Equal,
            (ProximaValue::Null, _) => Ordering::Less,
            (_, ProximaValue::Null) => Ordering::Greater,

            (ProximaValue::Boolean(va), ProximaValue::Boolean(vb)) => va.cmp(vb),

            (ProximaValue::Int64(va), ProximaValue::Int64(vb)) => va.cmp(vb),

            (ProximaValue::Float64(va), ProximaValue::Float64(vb)) => {
                va.partial_cmp(vb).unwrap_or(Ordering::Equal)
            }

            // Cross-type numeric comparison
            (ProximaValue::Int64(va), ProximaValue::Float64(vb)) => {
                (*va as f64).partial_cmp(vb).unwrap_or(Ordering::Equal)
            }
            (ProximaValue::Float64(va), ProximaValue::Int64(vb)) => {
                va.partial_cmp(&(*vb as f64)).unwrap_or(Ordering::Equal)
            }

            (ProximaValue::String(va), ProximaValue::String(vb)) => va.cmp(vb),

            _ => Ordering::Equal,
        }
    }

    // =========================================================================
    // LIMIT/SKIP STAGES
    // =========================================================================

    /// Process a limit stage
    fn process_limit(
        &self,
        documents: &[ProximaTree],
        limit_stage: &LimitStage,
    ) -> Result<Vec<ProximaTree>> {
        Ok(documents
            .iter()
            .take(limit_stage.limit as usize)
            .cloned()
            .collect())
    }

    /// Process a skip stage
    fn process_skip(
        &self,
        documents: &[ProximaTree],
        skip_stage: &SkipStage,
    ) -> Result<Vec<ProximaTree>> {
        Ok(documents
            .iter()
            .skip(skip_stage.skip as usize)
            .cloned()
            .collect())
    }

    // =========================================================================
    // UNWIND STAGE
    // =========================================================================

    /// Process an unwind stage (expands arrays into multiple documents)
    fn process_unwind(
        &self,
        documents: &[ProximaTree],
        unwind_stage: &UnwindStage,
    ) -> Result<Vec<ProximaTree>> {
        let path = &unwind_stage.path;
        let preserve_null = unwind_stage.preserve_null;

        let mut results = Vec::new();

        for doc in documents {
            // Unwind shapes the canonical array via `set_path_value` directly on
            // the `ProximaTree` working set — no proto detour.
            let array_val = self.extract_value(doc, path);

            match array_val {
                Some(ProximaValue::Array(arr)) => {
                    if arr.is_empty() && preserve_null {
                        // Preserve document with null array field
                        let mut unwound = doc.clone();
                        self.set_path_value(&mut unwound, path, ProximaValue::Null);
                        results.push(unwound);
                    } else {
                        // Create one document per array element
                        for elem in arr {
                            let mut unwound = doc.clone();
                            self.set_path_value(&mut unwound, path, elem);
                            results.push(unwound);
                        }
                    }
                }
                Some(_) => {
                    // Field exists but is not an array - keep document as-is
                    results.push(doc.clone());
                }
                None => {
                    if preserve_null {
                        results.push(doc.clone());
                    }
                    // Otherwise skip documents without the field
                }
            }
        }

        Ok(results)
    }

    // =========================================================================
    // LOOKUP STAGE - Left outer join with another collection
    // =========================================================================

    /// Process a lookup stage (requires a fetcher callback).
    ///
    /// Lookup edge: the cross-collection `LookupFetcher` join boundary still
    /// produces/consumes proto `SqlObject` (it queries the document store, which
    /// is `SqlObject`-backed until Group C). Convert the canonical working set to
    /// `SqlObject` for the join and lower the results back to `ProximaTree`.
    pub fn process_lookup(
        &self,
        documents: &[ProximaTree],
        lookup_stage: &LookupStage,
        fetcher: &dyn LookupFetcher,
    ) -> Result<Vec<ProximaTree>> {
        let config = LookupConfig {
            from_collection: lookup_stage.from_collection.clone(),
            local_field: lookup_stage.local_field.clone(),
            foreign_field: lookup_stage.foreign_field.clone(),
            output_field: lookup_stage.as_field.clone(),
        };

        let sql_docs: Vec<SqlObject> = documents.iter().map(proxima_tree_to_sql_object).collect();
        let joined = execute_lookup(&sql_docs, &config, fetcher)?;
        Ok(joined.iter().map(sql_object_to_proxima_tree).collect())
    }

    /// Check if a document matches a filter (helper for pipeline processing).
    ///
    /// Slice 7a: the filter reads the record's canonical `props` tree directly.
    pub fn matches_filter(&self, doc: &DocumentRecord, filter: &DocumentFilter) -> bool {
        self.filter_evaluator.evaluate(filter, doc)
    }

    /// Set a value at a path in a document (simplified for top-level paths)
    fn set_path_value(&self, doc: &mut ProximaTree, path: &str, value: ProximaValue) {
        // Handle simple paths (no nested objects for now)
        let field_name = path
            .trim_start_matches('$')
            .trim_start_matches('.')
            .split('.')
            .next()
            .unwrap_or(path);

        doc.insert(field_name.to_string(), ProximaTreeNode::Value(value));
    }

    // =========================================================================
    // FULL-TEXT SEARCH SCORING
    // =========================================================================

    /// Calculate full-text search scores for documents
    ///
    /// This provides basic TF-IDF-like scoring for text matching.
    /// For production use, integrate with Tantivy index.
    pub fn calculate_fulltext_scores(
        &self,
        documents: &[DocumentRecord],
        query_terms: &[String],
        text_paths: &[String],
    ) -> Vec<(DocumentRecord, f32)> {
        let total_docs = documents.len() as f32;

        // Slice 6: the record carries the canonical `props` tree directly.
        let trees: Vec<ProximaTree> = documents.iter().map(|doc| doc.props.clone()).collect();

        // Calculate document frequency for each term
        let mut doc_frequencies: HashMap<String, usize> = HashMap::new();
        for term in query_terms {
            let term_lower = term.to_lowercase();
            let count = trees
                .iter()
                .filter(|tree| self.document_contains_term(tree, &term_lower, text_paths))
                .count();
            doc_frequencies.insert(term_lower, count);
        }

        // Score each document
        documents
            .iter()
            .zip(trees.iter())
            .map(|(doc, tree)| {
                let mut score = 0.0f32;

                for term in query_terms {
                    let term_lower = term.to_lowercase();
                    let tf = self.calculate_term_frequency(tree, &term_lower, text_paths);

                    // IDF calculation: log(N / (df + 1)) + 1
                    let df = *doc_frequencies.get(&term_lower).unwrap_or(&0) as f32;
                    let idf = if df > 0.0 {
                        (total_docs / (df + 1.0)).ln() + 1.0
                    } else {
                        0.0
                    };

                    score += tf * idf;
                }

                (doc.clone(), score)
            })
            .collect()
    }

    /// Check if a document contains a term in any of the specified paths
    fn document_contains_term(&self, doc: &ProximaTree, term: &str, paths: &[String]) -> bool {
        for path in paths {
            if let Some(text) = self.extract_text_value(doc, path)
                && text.to_lowercase().contains(term)
            {
                return true;
            }
        }
        false
    }

    /// Calculate term frequency in a document
    fn calculate_term_frequency(&self, doc: &ProximaTree, term: &str, paths: &[String]) -> f32 {
        let mut total_count = 0;
        let mut total_words = 0;

        for path in paths {
            if let Some(text) = self.extract_text_value(doc, path) {
                let words: Vec<&str> = text.split_whitespace().collect();
                total_words += words.len();
                total_count += words.iter().filter(|w| w.to_lowercase() == term).count();
            }
        }

        if total_words > 0 {
            total_count as f32 / total_words as f32
        } else {
            0.0
        }
    }

    /// Extract text value from a path
    fn extract_text_value(&self, doc: &ProximaTree, path: &str) -> Option<String> {
        self.extract_value(doc, path).and_then(|val| match val {
            ProximaValue::String(s) => Some(s),
            _ => None,
        })
    }

    // =========================================================================
    // HELPER METHODS
    // =========================================================================

    /// Extract a value from a document using JSON path
    fn extract_value(&self, doc: &ProximaTree, path: &str) -> Option<ProximaValue> {
        let json_doc = self.tree_to_json(doc);
        let normalized_path = self.normalize_path(path);

        match json_doc.path(&normalized_path) {
            Ok(result) => match &result {
                JsonValue::Array(arr) if arr.len() == 1 => {
                    if arr[0].is_null() {
                        None
                    } else {
                        Some(json_to_proxima(&arr[0]))
                    }
                }
                JsonValue::Array(arr) if arr.is_empty() => None,
                JsonValue::Null => None,
                _ => Some(json_to_proxima(&result)),
            },
            Err(_) => None,
        }
    }

    /// Extract a numeric value from a document using JSON path
    fn extract_numeric_value(&self, doc: &ProximaTree, path: &str) -> Option<f64> {
        self.extract_value(doc, path).and_then(|val| match val {
            ProximaValue::Int64(i) => Some(i as f64),
            ProximaValue::Float64(f) => Some(f),
            _ => None,
        })
    }

    /// Normalize a JSON path expression
    fn normalize_path(&self, path: &str) -> String {
        if path.starts_with("$.") || path.starts_with('$') {
            path.to_string()
        } else {
            format!("$.{}", path)
        }
    }

    /// Render a canonical `ProximaTree` to `serde_json::Value` for the jsonpath
    /// engine. Reuses the shared `proxima_tree_to_json_map` bridge.
    fn tree_to_json(&self, tree: &ProximaTree) -> JsonValue {
        JsonValue::Object(proxima_tree_to_json_map(tree).into_iter().collect())
    }

    /// Convert a JSON value to string representation for grouping
    fn json_value_to_string(&self, value: &JsonValue) -> String {
        match value {
            JsonValue::Null => "_null".to_string(),
            JsonValue::Bool(b) => b.to_string(),
            JsonValue::Number(n) => n.to_string(),
            JsonValue::String(s) => s.clone(),
            JsonValue::Array(_) => value.to_string(),
            JsonValue::Object(_) => value.to_string(),
        }
    }
}

impl Default for AggregationExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{SqlValue, sql_value::Value as SqlValueVariant};

    /// Build a canonical `ProximaTree` working-set document from leaf values.
    fn tree_doc(fields: Vec<(&str, ProximaValue)>) -> ProximaTree {
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), ProximaTreeNode::Value(v)))
            .collect()
    }

    fn pv_int(i: i64) -> ProximaValue {
        ProximaValue::Int64(i)
    }

    fn pv_str(s: &str) -> ProximaValue {
        ProximaValue::String(s.to_string())
    }

    /// Read an `Int64` leaf out of a result tree (results stay canonical now).
    fn tree_int(doc: &ProximaTree, key: &str) -> Option<i64> {
        match doc.get(key) {
            Some(ProximaTreeNode::Value(ProximaValue::Int64(i))) => Some(*i),
            _ => None,
        }
    }

    /// Read a `Float64` leaf out of a result tree.
    fn tree_float(doc: &ProximaTree, key: &str) -> Option<f64> {
        match doc.get(key) {
            Some(ProximaTreeNode::Value(ProximaValue::Float64(f))) => Some(*f),
            _ => None,
        }
    }

    /// Read a `String` leaf out of a result tree.
    fn tree_string<'a>(doc: &'a ProximaTree, key: &str) -> Option<&'a str> {
        match doc.get(key) {
            Some(ProximaTreeNode::Value(ProximaValue::String(s))) => Some(s.as_str()),
            _ => None,
        }
    }

    // The fulltext test exercises the `DocumentRecord` input edge, which still
    // carries a proto `SqlObject` until Group C — build that shape here.
    fn sql_string(s: &str) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::StringValue(s.to_string())),
        }
    }

    fn sql_doc(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    #[test]
    fn test_aggregation_count() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("name", pv_str("Alice")), ("age", pv_int(30))]),
            tree_doc(vec![("name", pv_str("Bob")), ("age", pv_int(25))]),
            tree_doc(vec![("name", pv_str("Charlie")), ("age", pv_int(35))]),
        ];

        let agg = Aggregation {
            output_field: "count".to_string(),
            r#type: AggregationType::Count as i32,
            input_path: "*".to_string(),
        };

        let doc_refs: Vec<&ProximaTree> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("COUNT aggregation should succeed");

        if let ProximaValue::Int64(count) = result {
            assert_eq!(count, 3);
        } else {
            panic!("Expected Int64");
        }
    }

    #[test]
    fn test_aggregation_sum() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("amount", pv_int(100))]),
            tree_doc(vec![("amount", pv_int(200))]),
            tree_doc(vec![("amount", pv_int(300))]),
        ];

        let agg = Aggregation {
            output_field: "total".to_string(),
            r#type: AggregationType::Sum as i32,
            input_path: "amount".to_string(),
        };

        let doc_refs: Vec<&ProximaTree> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("SUM aggregation should succeed");

        if let ProximaValue::Float64(sum) = result {
            assert!((sum - 600.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Float64");
        }
    }

    #[test]
    fn test_aggregation_avg() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("score", pv_int(80))]),
            tree_doc(vec![("score", pv_int(90))]),
            tree_doc(vec![("score", pv_int(100))]),
        ];

        let agg = Aggregation {
            output_field: "avg_score".to_string(),
            r#type: AggregationType::Avg as i32,
            input_path: "score".to_string(),
        };

        let doc_refs: Vec<&ProximaTree> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("AVG aggregation should succeed");

        if let ProximaValue::Float64(avg) = result {
            assert!((avg - 90.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Float64");
        }
    }

    #[test]
    fn test_aggregation_min_max() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("value", pv_int(5))]),
            tree_doc(vec![("value", pv_int(3))]),
            tree_doc(vec![("value", pv_int(8))]),
        ];

        let doc_refs: Vec<&ProximaTree> = docs.iter().collect();

        // Test MIN
        let min_agg = Aggregation {
            output_field: "min_val".to_string(),
            r#type: AggregationType::Min as i32,
            input_path: "value".to_string(),
        };
        let min_result = executor
            .compute_aggregation(&doc_refs, &min_agg)
            .expect("MIN aggregation should succeed");
        if let ProximaValue::Float64(min) = min_result {
            assert!((min - 3.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Float64 for MIN");
        }

        // Test MAX
        let max_agg = Aggregation {
            output_field: "max_val".to_string(),
            r#type: AggregationType::Max as i32,
            input_path: "value".to_string(),
        };
        let max_result = executor
            .compute_aggregation(&doc_refs, &max_agg)
            .expect("MAX aggregation should succeed");
        if let ProximaValue::Float64(max) = max_result {
            assert!((max - 8.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Float64 for MAX");
        }
    }

    #[test]
    fn test_group_stage() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("category", pv_str("A")), ("value", pv_int(10))]),
            tree_doc(vec![("category", pv_str("B")), ("value", pv_int(20))]),
            tree_doc(vec![("category", pv_str("A")), ("value", pv_int(30))]),
        ];

        let group_stage = GroupStage {
            key: "category".to_string(),
            aggregations: vec![
                Aggregation {
                    output_field: "count".to_string(),
                    r#type: AggregationType::Count as i32,
                    input_path: "*".to_string(),
                },
                Aggregation {
                    output_field: "total".to_string(),
                    r#type: AggregationType::Sum as i32,
                    input_path: "value".to_string(),
                },
            ],
        };

        let results = executor
            .process_group(&docs, &group_stage)
            .expect("GROUP stage should succeed");

        assert_eq!(results.len(), 2); // Two categories: A and B

        // Find results for category A
        let cat_a = results
            .iter()
            .find(|r| tree_string(r, "_id") == Some("A"))
            .expect("Should find category A");

        // Category A should have count=2, total=40
        assert_eq!(tree_int(cat_a, "count"), Some(2));
        let total = tree_float(cat_a, "total").expect("total should be Float64");
        assert!((total - 40.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_sort_stage() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            tree_doc(vec![("value", pv_int(30))]),
            tree_doc(vec![("value", pv_int(10))]),
            tree_doc(vec![("value", pv_int(20))]),
        ];

        let sort_stage = SortStage {
            fields: vec![SortField {
                path: "value".to_string(),
                order: SortOrder::Asc as i32,
            }],
        };

        let sorted = executor
            .process_sort(&docs, &sort_stage)
            .expect("SORT stage should succeed");

        // Should be sorted: 10, 20, 30
        let values: Vec<i64> = sorted.iter().filter_map(|d| tree_int(d, "value")).collect();

        assert_eq!(values, vec![10, 20, 30]);
    }

    #[test]
    fn test_limit_skip_stages() {
        let executor = AggregationExecutor::new();

        let docs: Vec<ProximaTree> = (1..=10)
            .map(|i| tree_doc(vec![("value", pv_int(i))]))
            .collect();

        // Test SKIP
        let skip_stage = SkipStage { skip: 3 };
        let skipped = executor
            .process_skip(&docs, &skip_stage)
            .expect("SKIP stage should succeed");
        assert_eq!(skipped.len(), 7);

        // Test LIMIT
        let limit_stage = LimitStage { limit: 5 };
        let limited = executor
            .process_limit(&docs, &limit_stage)
            .expect("LIMIT stage should succeed");
        assert_eq!(limited.len(), 5);

        // Test SKIP + LIMIT (pagination)
        let skipped_then_limited = executor
            .process_limit(&skipped, &limit_stage)
            .expect("LIMIT stage (after SKIP) should succeed");
        assert_eq!(skipped_then_limited.len(), 5);
    }

    #[test]
    fn test_fulltext_scoring() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            DocumentRecord::new(
                "1".to_string(),
                sql_doc(vec![
                    ("title", sql_string("The quick brown fox")),
                    ("body", sql_string("A fox is a quick animal")),
                ]),
                "test".to_string(),
            ),
            DocumentRecord::new(
                "2".to_string(),
                sql_doc(vec![
                    ("title", sql_string("The lazy dog")),
                    ("body", sql_string("Dogs are friendly")),
                ]),
                "test".to_string(),
            ),
        ];

        let query_terms = vec!["fox".to_string(), "quick".to_string()];
        let text_paths = vec!["title".to_string(), "body".to_string()];

        let scored = executor.calculate_fulltext_scores(&docs, &query_terms, &text_paths);

        // Document 1 should have higher score (contains "fox" and "quick")
        assert!(scored[0].1 > scored[1].1);
    }
}
