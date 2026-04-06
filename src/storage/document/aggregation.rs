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

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use jsonpath_rust::JsonPathQuery;
use serde_json::Value as JsonValue;
use tracing::debug;

use crate::proto::proximadb_v1::{
    Aggregation, AggregationStage, AggregationType, DocumentFilter, GroupStage, LimitStage,
    LookupStage, MatchStage, ProjectStage, SkipStage, SortOrder, SortStage, SqlArray, SqlObject,
    SqlValue, UnwindStage, aggregation_stage::Stage, sql_value::Value as SqlValueVariant,
};

#[cfg(test)]
use crate::proto::proximadb_v1::SortField;

use super::DocumentRecord;
use super::aggregation_extensions::{LookupConfig, LookupFetcher, execute_lookup};
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

    /// Execute an aggregation pipeline on a set of documents
    pub fn execute(
        &self,
        documents: Vec<DocumentRecord>,
        filter: Option<&DocumentFilter>,
        pipeline: &[AggregationStage],
    ) -> Result<Vec<SqlObject>> {
        // Start with filtered documents if filter is provided
        let mut working_set: Vec<SqlObject> = if let Some(f) = filter {
            documents
                .into_iter()
                .filter(|doc| self.filter_evaluator.evaluate(f, doc))
                .map(|doc| doc.document)
                .collect()
        } else {
            documents.into_iter().map(|doc| doc.document).collect()
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

        Ok(working_set)
    }

    /// Process a single pipeline stage (public for external use with lookups)
    pub fn process_stage(
        &self,
        documents: &[SqlObject],
        stage: &AggregationStage,
        stage_idx: usize,
    ) -> Result<Vec<SqlObject>> {
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
        documents: &[SqlObject],
        match_stage: &MatchStage,
    ) -> Result<Vec<SqlObject>> {
        let filter = match &match_stage.filter {
            Some(f) => f,
            None => return Ok(documents.to_vec()),
        };

        Ok(documents
            .iter()
            .filter(|doc| {
                // Create a temporary DocumentRecord for filter evaluation
                let record = DocumentRecord {
                    id: String::new(),
                    document: (*doc).clone(),
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
        documents: &[SqlObject],
        group_stage: &GroupStage,
    ) -> Result<Vec<SqlObject>> {
        let group_key = &group_stage.key;

        // Group documents by key
        let mut groups: HashMap<String, Vec<&SqlObject>> = HashMap::new();

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

        // Compute aggregations for each group
        let mut results = Vec::with_capacity(groups.len());
        for (key, group_docs) in groups {
            let mut result_doc = SqlObject {
                fields: HashMap::new(),
            };

            // Add the group key to result (as _id field, MongoDB style)
            if group_key != "_id" && !group_key.is_empty() {
                result_doc.fields.insert(
                    "_id".to_string(),
                    SqlValue {
                        value: Some(SqlValueVariant::StringValue(key.clone())),
                    },
                );
            }

            // Compute each aggregation
            for agg in &group_stage.aggregations {
                let agg_value = self.compute_aggregation(&group_docs, agg)?;
                result_doc
                    .fields
                    .insert(agg.output_field.clone(), agg_value);
            }

            results.push(result_doc);
        }

        Ok(results)
    }

    /// Extract group key from document
    fn extract_group_key(&self, doc: &SqlObject, path: &str) -> String {
        let json_doc = self.sql_object_to_json(doc);
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

    /// Compute a single aggregation over a group of documents
    fn compute_aggregation(&self, docs: &[&SqlObject], agg: &Aggregation) -> Result<SqlValue> {
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
    fn agg_count(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let count = if path.is_empty() || path == "*" {
            // Count all documents
            docs.len() as i64
        } else {
            // Count documents with non-null values at path
            docs.iter()
                .filter(|doc| self.extract_value(doc, path).is_some())
                .count() as i64
        };

        Ok(SqlValue {
            value: Some(SqlValueVariant::Int64Value(count)),
        })
    }

    /// SUM aggregation
    fn agg_sum(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let mut sum = 0.0;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                sum += val;
            }
        }

        Ok(SqlValue {
            value: Some(SqlValueVariant::NumberValue(sum)),
        })
    }

    /// AVG aggregation
    fn agg_avg(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let mut sum = 0.0;
        let mut count = 0;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                sum += val;
                count += 1;
            }
        }

        let avg = if count > 0 { sum / count as f64 } else { 0.0 };

        Ok(SqlValue {
            value: Some(SqlValueVariant::NumberValue(avg)),
        })
    }

    /// MIN aggregation
    fn agg_min(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let mut min_val: Option<f64> = None;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                min_val = Some(min_val.map_or(val, |m| m.min(val)));
            }
        }

        Ok(SqlValue {
            value: min_val.map(SqlValueVariant::NumberValue),
        })
    }

    /// MAX aggregation
    fn agg_max(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let mut max_val: Option<f64> = None;

        for doc in docs {
            if let Some(val) = self.extract_numeric_value(doc, path) {
                max_val = Some(max_val.map_or(val, |m| m.max(val)));
            }
        }

        Ok(SqlValue {
            value: max_val.map(SqlValueVariant::NumberValue),
        })
    }

    /// FIRST aggregation - returns first value in group
    fn agg_first(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        for doc in docs {
            if let Some(val) = self.extract_value(doc, path) {
                return Ok(val);
            }
        }

        Ok(SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        })
    }

    /// LAST aggregation - returns last value in group
    fn agg_last(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        for doc in docs.iter().rev() {
            if let Some(val) = self.extract_value(doc, path) {
                return Ok(val);
            }
        }

        Ok(SqlValue {
            value: Some(SqlValueVariant::NullValue(0)),
        })
    }

    /// PUSH aggregation - collects all values into an array
    fn agg_push(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let values: Vec<SqlValue> = docs
            .iter()
            .filter_map(|doc| self.extract_value(doc, path))
            .collect();

        Ok(SqlValue {
            value: Some(SqlValueVariant::ArrayValue(SqlArray { values })),
        })
    }

    /// ADD_TO_SET aggregation - collects unique values into an array
    fn agg_add_to_set(&self, docs: &[&SqlObject], path: &str) -> Result<SqlValue> {
        let mut seen: Vec<SqlValue> = Vec::new();

        for doc in docs {
            if let Some(val) = self.extract_value(doc, path) {
                // Check if we've already seen this value
                let is_duplicate = seen.iter().any(|v| self.sql_values_equal(v, &val));
                if !is_duplicate {
                    seen.push(val);
                }
            }
        }

        Ok(SqlValue {
            value: Some(SqlValueVariant::ArrayValue(SqlArray { values: seen })),
        })
    }

    // =========================================================================
    // PROJECT STAGE
    // =========================================================================

    /// Process a project stage
    fn process_project(
        &self,
        documents: &[SqlObject],
        project_stage: &ProjectStage,
    ) -> Result<Vec<SqlObject>> {
        Ok(documents
            .iter()
            .map(|doc| self.project_document(doc, project_stage))
            .collect())
    }

    /// Project a single document
    fn project_document(&self, doc: &SqlObject, project_stage: &ProjectStage) -> SqlObject {
        let mut result = SqlObject {
            fields: HashMap::new(),
        };

        // Handle field inclusion/exclusion
        let has_inclusions = project_stage.fields.values().any(|&v| v);
        let has_exclusions = project_stage.fields.values().any(|&v| !v);

        if has_inclusions {
            // Include mode: only include specified fields
            for (field, &include) in &project_stage.fields {
                if include && let Some(val) = doc.fields.get(field) {
                    result.fields.insert(field.clone(), val.clone());
                }
            }
        } else if has_exclusions {
            // Exclude mode: include all except specified fields
            for (key, val) in &doc.fields {
                if !project_stage.fields.get(key).copied().unwrap_or(true) {
                    continue; // Skip excluded fields
                }
                result.fields.insert(key.clone(), val.clone());
            }
        } else {
            // No field specification - keep all fields
            result.fields = doc.fields.clone();
        }

        // Handle computed fields
        for (output_field, expression) in &project_stage.computed {
            if let Some(val) = self.evaluate_expression(doc, expression) {
                result.fields.insert(output_field.clone(), val);
            }
        }

        result
    }

    /// Evaluate a computed field expression
    fn evaluate_expression(&self, doc: &SqlObject, expression: &str) -> Option<SqlValue> {
        // Simple implementation: treat expression as a JSON path
        self.extract_value(doc, expression)
    }

    // =========================================================================
    // SORT STAGE
    // =========================================================================

    /// Process a sort stage
    fn process_sort(
        &self,
        documents: &[SqlObject],
        sort_stage: &SortStage,
    ) -> Result<Vec<SqlObject>> {
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

    /// Compare two documents by a JSON path
    fn compare_by_path(&self, a: &SqlObject, b: &SqlObject, path: &str) -> std::cmp::Ordering {
        let val_a = self.extract_value(a, path);
        let val_b = self.extract_value(b, path);

        match (val_a, val_b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => self.compare_sql_values(&a, &b),
        }
    }

    /// Compare two SqlValue instances for ordering
    fn compare_sql_values(&self, a: &SqlValue, b: &SqlValue) -> std::cmp::Ordering {
        match (&a.value, &b.value) {
            (Some(SqlValueVariant::NullValue(_)), Some(SqlValueVariant::NullValue(_))) => {
                std::cmp::Ordering::Equal
            }
            (Some(SqlValueVariant::NullValue(_)), _) => std::cmp::Ordering::Less,
            (_, Some(SqlValueVariant::NullValue(_))) => std::cmp::Ordering::Greater,

            (Some(SqlValueVariant::BoolValue(va)), Some(SqlValueVariant::BoolValue(vb))) => {
                va.cmp(vb)
            }

            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                va.cmp(vb)
            }

            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                va.partial_cmp(vb).unwrap_or(std::cmp::Ordering::Equal)
            }

            // Cross-type numeric comparison
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::NumberValue(vb))) => (*va
                as f64)
                .partial_cmp(vb)
                .unwrap_or(std::cmp::Ordering::Equal),
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::Int64Value(vb))) => va
                .partial_cmp(&(*vb as f64))
                .unwrap_or(std::cmp::Ordering::Equal),

            (Some(SqlValueVariant::StringValue(va)), Some(SqlValueVariant::StringValue(vb))) => {
                va.cmp(vb)
            }

            _ => std::cmp::Ordering::Equal,
        }
    }

    // =========================================================================
    // LIMIT/SKIP STAGES
    // =========================================================================

    /// Process a limit stage
    fn process_limit(
        &self,
        documents: &[SqlObject],
        limit_stage: &LimitStage,
    ) -> Result<Vec<SqlObject>> {
        Ok(documents
            .iter()
            .take(limit_stage.limit as usize)
            .cloned()
            .collect())
    }

    /// Process a skip stage
    fn process_skip(
        &self,
        documents: &[SqlObject],
        skip_stage: &SkipStage,
    ) -> Result<Vec<SqlObject>> {
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
        documents: &[SqlObject],
        unwind_stage: &UnwindStage,
    ) -> Result<Vec<SqlObject>> {
        let path = &unwind_stage.path;
        let preserve_null = unwind_stage.preserve_null;

        let mut results = Vec::new();

        for doc in documents {
            let array_val = self.extract_value(doc, path);

            match array_val {
                Some(SqlValue {
                    value: Some(SqlValueVariant::ArrayValue(arr)),
                }) => {
                    if arr.values.is_empty() && preserve_null {
                        // Preserve document with null array field
                        let mut unwound = doc.clone();
                        self.set_path_value(
                            &mut unwound,
                            path,
                            SqlValue {
                                value: Some(SqlValueVariant::NullValue(0)),
                            },
                        );
                        results.push(unwound);
                    } else {
                        // Create one document per array element
                        for elem in arr.values {
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

    /// Process a lookup stage (requires a fetcher callback)
    pub fn process_lookup(
        &self,
        documents: &[SqlObject],
        lookup_stage: &LookupStage,
        fetcher: &dyn LookupFetcher,
    ) -> Result<Vec<SqlObject>> {
        let config = LookupConfig {
            from_collection: lookup_stage.from_collection.clone(),
            local_field: lookup_stage.local_field.clone(),
            foreign_field: lookup_stage.foreign_field.clone(),
            output_field: lookup_stage.as_field.clone(),
        };

        execute_lookup(documents, &config, fetcher)
    }

    /// Check if a document matches a filter (helper for pipeline processing)
    pub fn matches_filter(&self, doc: &DocumentRecord, filter: &DocumentFilter) -> bool {
        // Create a temporary DocumentRecord for filter evaluation
        let record = DocumentRecord {
            id: String::new(),
            document: doc.document.clone(),
            version: 0,
            collection_id: String::new(),
            updated_at_ns: 0,
            schema_id: None,
            document_type: None,
        };
        self.filter_evaluator.evaluate(filter, &record)
    }

    /// Set a value at a path in a document (simplified for top-level paths)
    fn set_path_value(&self, doc: &mut SqlObject, path: &str, value: SqlValue) {
        // Handle simple paths (no nested objects for now)
        let field_name = path
            .trim_start_matches('$')
            .trim_start_matches('.')
            .split('.')
            .next()
            .unwrap_or(path);

        doc.fields.insert(field_name.to_string(), value);
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

        // Calculate document frequency for each term
        let mut doc_frequencies: HashMap<String, usize> = HashMap::new();
        for term in query_terms {
            let term_lower = term.to_lowercase();
            let count = documents
                .iter()
                .filter(|doc| self.document_contains_term(&doc.document, &term_lower, text_paths))
                .count();
            doc_frequencies.insert(term_lower, count);
        }

        // Score each document
        documents
            .iter()
            .map(|doc| {
                let mut score = 0.0f32;

                for term in query_terms {
                    let term_lower = term.to_lowercase();
                    let tf = self.calculate_term_frequency(&doc.document, &term_lower, text_paths);

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
    fn document_contains_term(&self, doc: &SqlObject, term: &str, paths: &[String]) -> bool {
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
    fn calculate_term_frequency(&self, doc: &SqlObject, term: &str, paths: &[String]) -> f32 {
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
    fn extract_text_value(&self, doc: &SqlObject, path: &str) -> Option<String> {
        self.extract_value(doc, path).and_then(|val| {
            if let Some(SqlValueVariant::StringValue(s)) = val.value {
                Some(s)
            } else {
                None
            }
        })
    }

    // =========================================================================
    // HELPER METHODS
    // =========================================================================

    /// Extract a value from a document using JSON path
    fn extract_value(&self, doc: &SqlObject, path: &str) -> Option<SqlValue> {
        let json_doc = self.sql_object_to_json(doc);
        let normalized_path = self.normalize_path(path);

        match json_doc.path(&normalized_path) {
            Ok(result) => match &result {
                JsonValue::Array(arr) if arr.len() == 1 => {
                    if arr[0].is_null() {
                        None
                    } else {
                        self.json_to_sql_value(&arr[0])
                    }
                }
                JsonValue::Array(arr) if arr.is_empty() => None,
                JsonValue::Null => None,
                _ => self.json_to_sql_value(&result),
            },
            Err(_) => None,
        }
    }

    /// Extract a numeric value from a document using JSON path
    fn extract_numeric_value(&self, doc: &SqlObject, path: &str) -> Option<f64> {
        self.extract_value(doc, path)
            .and_then(|val| match val.value {
                Some(SqlValueVariant::Int64Value(i)) => Some(i as f64),
                Some(SqlValueVariant::NumberValue(f)) => Some(f),
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

    /// Convert SqlObject to serde_json::Value
    fn sql_object_to_json(&self, obj: &SqlObject) -> JsonValue {
        let mut map = serde_json::Map::new();
        for (key, value) in &obj.fields {
            if let Some(json_val) = self.sql_value_to_json(value) {
                map.insert(key.clone(), json_val);
            }
        }
        JsonValue::Object(map)
    }

    /// Convert SqlValue to serde_json::Value
    fn sql_value_to_json(&self, value: &SqlValue) -> Option<JsonValue> {
        match &value.value {
            Some(SqlValueVariant::NullValue(_)) => Some(JsonValue::Null),
            Some(SqlValueVariant::BoolValue(b)) => Some(JsonValue::Bool(*b)),
            Some(SqlValueVariant::Int64Value(i)) => Some(JsonValue::Number((*i).into())),
            Some(SqlValueVariant::NumberValue(f)) => {
                serde_json::Number::from_f64(*f).map(JsonValue::Number)
            }
            Some(SqlValueVariant::StringValue(s)) => Some(JsonValue::String(s.clone())),
            Some(SqlValueVariant::BytesValue(b)) => {
                let hex_str: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                Some(JsonValue::String(format!("0x{}", hex_str)))
            }
            Some(SqlValueVariant::ArrayValue(arr)) => {
                let json_arr: Vec<JsonValue> = arr
                    .values
                    .iter()
                    .filter_map(|v| self.sql_value_to_json(v))
                    .collect();
                Some(JsonValue::Array(json_arr))
            }
            Some(SqlValueVariant::ObjectValue(obj)) => Some(self.sql_object_to_json(obj)),
            None => None,
        }
    }

    /// Convert serde_json::Value to SqlValue
    fn json_to_sql_value(&self, value: &JsonValue) -> Option<SqlValue> {
        match value {
            JsonValue::Null => Some(SqlValue {
                value: Some(SqlValueVariant::NullValue(0)),
            }),
            JsonValue::Bool(b) => Some(SqlValue {
                value: Some(SqlValueVariant::BoolValue(*b)),
            }),
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(SqlValue {
                        value: Some(SqlValueVariant::Int64Value(i)),
                    })
                } else {
                    n.as_f64().map(|f| SqlValue {
                        value: Some(SqlValueVariant::NumberValue(f)),
                    })
                }
            }
            JsonValue::String(s) => Some(SqlValue {
                value: Some(SqlValueVariant::StringValue(s.clone())),
            }),
            JsonValue::Array(arr) => {
                let sql_values: Vec<SqlValue> = arr
                    .iter()
                    .filter_map(|v| self.json_to_sql_value(v))
                    .collect();
                Some(SqlValue {
                    value: Some(SqlValueVariant::ArrayValue(SqlArray { values: sql_values })),
                })
            }
            JsonValue::Object(obj) => {
                let mut fields = std::collections::HashMap::new();
                for (k, v) in obj {
                    if let Some(sql_val) = self.json_to_sql_value(v) {
                        fields.insert(k.clone(), sql_val);
                    }
                }
                Some(SqlValue {
                    value: Some(SqlValueVariant::ObjectValue(SqlObject { fields })),
                })
            }
        }
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

    /// Compare two SqlValue instances for equality
    fn sql_values_equal(&self, a: &SqlValue, b: &SqlValue) -> bool {
        match (&a.value, &b.value) {
            (Some(SqlValueVariant::NullValue(_)), Some(SqlValueVariant::NullValue(_))) => true,
            (Some(SqlValueVariant::BoolValue(va)), Some(SqlValueVariant::BoolValue(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                (va - vb).abs() < f64::EPSILON
            }
            (Some(SqlValueVariant::StringValue(va)), Some(SqlValueVariant::StringValue(vb))) => {
                va == vb
            }
            (Some(SqlValueVariant::BytesValue(va)), Some(SqlValueVariant::BytesValue(vb))) => {
                va == vb
            }
            // Cross-type numeric comparison
            (Some(SqlValueVariant::Int64Value(va)), Some(SqlValueVariant::NumberValue(vb))) => {
                (*va as f64 - vb).abs() < f64::EPSILON
            }
            (Some(SqlValueVariant::NumberValue(va)), Some(SqlValueVariant::Int64Value(vb))) => {
                (va - *vb as f64).abs() < f64::EPSILON
            }
            _ => false,
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

    fn create_test_doc(fields: Vec<(&str, SqlValue)>) -> SqlObject {
        SqlObject {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    fn int_value(i: i64) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::Int64Value(i)),
        }
    }

    fn string_value(s: &str) -> SqlValue {
        SqlValue {
            value: Some(SqlValueVariant::StringValue(s.to_string())),
        }
    }

    #[test]
    fn test_aggregation_count() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![
                ("name", string_value("Alice")),
                ("age", int_value(30)),
            ]),
            create_test_doc(vec![("name", string_value("Bob")), ("age", int_value(25))]),
            create_test_doc(vec![
                ("name", string_value("Charlie")),
                ("age", int_value(35)),
            ]),
        ];

        let agg = Aggregation {
            output_field: "count".to_string(),
            r#type: AggregationType::Count as i32,
            input_path: "*".to_string(),
        };

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("COUNT aggregation should succeed");

        if let Some(SqlValueVariant::Int64Value(count)) = result.value {
            assert_eq!(count, 3);
        } else {
            panic!("Expected Int64Value");
        }
    }

    #[test]
    fn test_aggregation_sum() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![("amount", int_value(100))]),
            create_test_doc(vec![("amount", int_value(200))]),
            create_test_doc(vec![("amount", int_value(300))]),
        ];

        let agg = Aggregation {
            output_field: "total".to_string(),
            r#type: AggregationType::Sum as i32,
            input_path: "amount".to_string(),
        };

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("SUM aggregation should succeed");

        if let Some(SqlValueVariant::NumberValue(sum)) = result.value {
            assert!((sum - 600.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue");
        }
    }

    #[test]
    fn test_aggregation_avg() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![("score", int_value(80))]),
            create_test_doc(vec![("score", int_value(90))]),
            create_test_doc(vec![("score", int_value(100))]),
        ];

        let agg = Aggregation {
            output_field: "avg_score".to_string(),
            r#type: AggregationType::Avg as i32,
            input_path: "score".to_string(),
        };

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();
        let result = executor
            .compute_aggregation(&doc_refs, &agg)
            .expect("AVG aggregation should succeed");

        if let Some(SqlValueVariant::NumberValue(avg)) = result.value {
            assert!((avg - 90.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue");
        }
    }

    #[test]
    fn test_aggregation_min_max() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![("value", int_value(5))]),
            create_test_doc(vec![("value", int_value(3))]),
            create_test_doc(vec![("value", int_value(8))]),
        ];

        let doc_refs: Vec<&SqlObject> = docs.iter().collect();

        // Test MIN
        let min_agg = Aggregation {
            output_field: "min_val".to_string(),
            r#type: AggregationType::Min as i32,
            input_path: "value".to_string(),
        };
        let min_result = executor
            .compute_aggregation(&doc_refs, &min_agg)
            .expect("MIN aggregation should succeed");
        if let Some(SqlValueVariant::NumberValue(min)) = min_result.value {
            assert!((min - 3.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue for MIN");
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
        if let Some(SqlValueVariant::NumberValue(max)) = max_result.value {
            assert!((max - 8.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected NumberValue for MAX");
        }
    }

    #[test]
    fn test_group_stage() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![
                ("category", string_value("A")),
                ("value", int_value(10)),
            ]),
            create_test_doc(vec![
                ("category", string_value("B")),
                ("value", int_value(20)),
            ]),
            create_test_doc(vec![
                ("category", string_value("A")),
                ("value", int_value(30)),
            ]),
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
            .find(|r| {
                r.fields.get("_id").and_then(|v| match &v.value {
                    Some(SqlValueVariant::StringValue(s)) => Some(s.as_str()),
                    _ => None,
                }) == Some("A")
            })
            .expect("Should find category A");

        // Category A should have count=2, total=40
        if let Some(SqlValueVariant::Int64Value(count)) =
            cat_a.fields.get("count").and_then(|v| v.value.as_ref())
        {
            assert_eq!(*count, 2);
        }
        if let Some(SqlValueVariant::NumberValue(total)) =
            cat_a.fields.get("total").and_then(|v| v.value.as_ref())
        {
            assert!((*total - 40.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn test_sort_stage() {
        let executor = AggregationExecutor::new();

        let docs = vec![
            create_test_doc(vec![("value", int_value(30))]),
            create_test_doc(vec![("value", int_value(10))]),
            create_test_doc(vec![("value", int_value(20))]),
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
        let values: Vec<i64> = sorted
            .iter()
            .filter_map(|d| {
                d.fields.get("value").and_then(|v| match &v.value {
                    Some(SqlValueVariant::Int64Value(i)) => Some(*i),
                    _ => None,
                })
            })
            .collect();

        assert_eq!(values, vec![10, 20, 30]);
    }

    #[test]
    fn test_limit_skip_stages() {
        let executor = AggregationExecutor::new();

        let docs: Vec<SqlObject> = (1..=10)
            .map(|i| create_test_doc(vec![("value", int_value(i))]))
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
                create_test_doc(vec![
                    ("title", string_value("The quick brown fox")),
                    ("body", string_value("A fox is a quick animal")),
                ]),
                "test".to_string(),
            ),
            DocumentRecord::new(
                "2".to_string(),
                create_test_doc(vec![
                    ("title", string_value("The lazy dog")),
                    ("body", string_value("Dogs are friendly")),
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
