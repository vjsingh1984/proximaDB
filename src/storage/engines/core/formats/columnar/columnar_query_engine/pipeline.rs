//! Pull-based pipeline execution engine for columnar queries.
//!
//! Implements DuckDB-inspired pipeline-based execution where operators are chained
//! and data flows through them in `DataChunk`-sized morsels (default 2048 rows).
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────┐    ┌──────────────┐    ┌──────────────┐    ┌──────────────┐
//! │  ScanOperator │───▶│FilterOperator│───▶│ScoreOperator │───▶│ TopKOperator │
//! │  (Parquet I/O)│    │(Arrow preds) │    │(SIMD batch)  │    │(BoundedQueue)│
//! └───────────────┘    └──────────────┘    └──────────────┘    └──────────────┘
//!     next_chunk()  ───▶  next_chunk()  ───▶  next_chunk()  ───▶    drain()
//! ```
//!
//! Each operator implements `PipelineOperator::next_chunk()` which pulls data from
//! its input, processes it, and returns the next `DataChunk`. Pipeline breakers
//! (sort, aggregate) buffer all input before producing output.

use anyhow::Result;
use arrow::array::{Array, FixedSizeListArray, Float32Array, ListArray, StringArray};
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::trace;

use super::vectorized_executor::{DataChunk, vectorized_filter_batch};
use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::engines::core::formats::columnar::FilterCondition;

/// Default morsel size matching L2 cache line efficiency.
/// DuckDB uses 2048; we match that for comparable behavior.
pub const DEFAULT_MORSEL_SIZE: usize = 2048;

// ─── Operator Trait ──────────────────────────────────────────────────────────

/// A pipeline operator that processes DataChunks in a pull-based model.
/// Call `next_chunk()` repeatedly until it returns `None` (exhausted).
pub trait PipelineOperator: Send {
    /// Pull the next chunk of data through the pipeline.
    /// Returns `None` when all input is consumed.
    fn next_chunk(&mut self) -> Result<Option<DataChunk>>;

    /// Human-readable name for debugging and metrics.
    fn name(&self) -> &str;
}

// ─── Scan Operator ───────────────────────────────────────────────────────────

/// Source operator that reads RecordBatches from a pre-loaded batch list.
/// Row group pruning and file I/O happen before the pipeline starts;
/// this operator simply feeds already-loaded batches into the pipeline.
pub struct ScanOperator {
    batches: Vec<RecordBatch>,
    current_idx: usize,
}

impl ScanOperator {
    pub fn new(batches: Vec<RecordBatch>) -> Self {
        Self {
            batches,
            current_idx: 0,
        }
    }
}

impl PipelineOperator for ScanOperator {
    fn next_chunk(&mut self) -> Result<Option<DataChunk>> {
        if self.current_idx >= self.batches.len() {
            return Ok(None);
        }

        let batch = self.batches[self.current_idx].clone();
        self.current_idx += 1;

        trace!(
            "ScanOperator: emitting chunk with {} rows",
            batch.num_rows()
        );
        Ok(Some(DataChunk::new(batch)))
    }

    fn name(&self) -> &str {
        "Scan"
    }
}

// ─── Filter Operator ─────────────────────────────────────────────────────────

/// Vectorized predicate filter operator.
/// Evaluates filter conditions on entire Arrow arrays using compute kernels,
/// then applies selection bitmap (late materialization).
pub struct FilterOperator {
    input: Box<dyn PipelineOperator>,
    conditions: Vec<FilterCondition>,
}

impl FilterOperator {
    pub fn new(input: Box<dyn PipelineOperator>, conditions: Vec<FilterCondition>) -> Self {
        Self { input, conditions }
    }
}

impl PipelineOperator for FilterOperator {
    fn next_chunk(&mut self) -> Result<Option<DataChunk>> {
        loop {
            let chunk = match self.input.next_chunk()? {
                Some(c) => c,
                None => return Ok(None),
            };

            if self.conditions.is_empty() {
                return Ok(Some(chunk));
            }

            // Apply vectorized filtering and materialize
            let filtered = vectorized_filter_batch(chunk.materialize()?, &self.conditions)?;

            if filtered.num_rows() > 0 {
                trace!(
                    "FilterOperator: {} -> {} rows",
                    chunk.batch().num_rows(),
                    filtered.num_rows()
                );
                return Ok(Some(DataChunk::new(filtered)));
            }
            // All rows filtered out — pull next chunk
        }
    }

    fn name(&self) -> &str {
        "Filter"
    }
}

// ─── Score Operator ──────────────────────────────────────────────────────────

/// SIMD-accelerated batch distance scoring operator.
/// Extracts vectors from the DataChunk, computes distances against a query vector
/// using the SIMD engine, and attaches scores as a new column.
pub struct ScoreOperator {
    input: Box<dyn PipelineOperator>,
    query_vector: Vec<f32>,
    distance_engine: UnifiedDistanceCompute,
    distance_metric: DistanceMetric,
}

impl ScoreOperator {
    pub fn new(
        input: Box<dyn PipelineOperator>,
        query_vector: Vec<f32>,
        distance_metric: DistanceMetric,
    ) -> Self {
        let distance_engine = UnifiedDistanceCompute::new(distance_metric);
        Self {
            input,
            query_vector,
            distance_engine,
            distance_metric,
        }
    }

    /// Extract vectors from a batch and compute batch SIMD distances.
    /// Returns (batch, scores) where scores[i] is the similarity for row i.
    fn compute_scores(&self, batch: &RecordBatch) -> Result<Vec<f32>> {
        let num_rows = batch.num_rows();
        if num_rows == 0 {
            return Ok(Vec::new());
        }

        let vector_col = batch
            .column_by_name("vector")
            .or_else(|| batch.column_by_name("vector_fp32"));

        let vec_slices: Vec<Vec<f32>> = if let Some(col) = vector_col {
            if let Some(fixed_list) = col.as_any().downcast_ref::<FixedSizeListArray>() {
                let values = fixed_list
                    .values()
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Invalid vector type in FixedSizeList"))?;
                let dim = fixed_list.value_length() as usize;
                (0..num_rows)
                    .map(|i| {
                        let start = i * dim;
                        (start..start + dim).map(|idx| values.value(idx)).collect()
                    })
                    .collect()
            } else if let Some(list) = col.as_any().downcast_ref::<ListArray>() {
                (0..num_rows)
                    .map(|i| {
                        let arr = list.value(i);
                        #[expect(
                            clippy::expect_used,
                            reason = "schema enforces Float32 elements in this list column"
                        )]
                        let fa = arr
                            .as_any()
                            .downcast_ref::<Float32Array>()
                            .expect("List should contain Float32Array elements");
                        (0..fa.len()).map(|j| fa.value(j)).collect()
                    })
                    .collect()
            } else {
                return Ok(vec![0.0; num_rows]);
            }
        } else {
            return Ok(vec![0.0; num_rows]);
        };

        let refs: Vec<&[f32]> = vec_slices.iter().map(|v| v.as_slice()).collect();
        let results = self.distance_engine.batch_distance_pooled_simd(
            &self.query_vector,
            &refs,
            &self.distance_metric,
        );

        Ok(results.iter().map(|r| r.similarity_score).collect())
    }
}

impl PipelineOperator for ScoreOperator {
    fn next_chunk(&mut self) -> Result<Option<DataChunk>> {
        let chunk = match self.input.next_chunk()? {
            Some(c) => c,
            None => return Ok(None),
        };

        let batch = chunk.materialize()?;
        let scores = self.compute_scores(&batch)?;

        // Attach scores as a new Float32 column
        let score_array = Arc::new(Float32Array::from(scores)) as arrow::array::ArrayRef;

        let mut fields = batch.schema().fields().to_vec();
        fields.push(Arc::new(arrow::datatypes::Field::new(
            "_score",
            arrow::datatypes::DataType::Float32,
            false,
        )));
        let new_schema = Arc::new(arrow::datatypes::Schema::new(fields));

        let mut columns: Vec<arrow::array::ArrayRef> = batch.columns().to_vec();
        columns.push(score_array);

        let scored_batch = RecordBatch::try_new(new_schema, columns)
            .map_err(|e| anyhow::anyhow!("Failed to create scored batch: {}", e))?;

        trace!("ScoreOperator: scored {} rows", scored_batch.num_rows());

        Ok(Some(DataChunk::new(scored_batch)))
    }

    fn name(&self) -> &str {
        "Score"
    }
}

// ─── TopK Operator (Pipeline Breaker) ────────────────────────────────────────

/// Pipeline breaker that consumes all input, maintains a bounded priority queue,
/// and produces a single output chunk with the top-k results.
#[allow(dead_code)]
pub struct TopKOperator {
    input: Box<dyn PipelineOperator>,
    queue: BoundedPriorityQueue,
    top_k: usize,
    min_score: Option<f32>,
    needs_metadata: bool,
    exhausted: bool,
}

impl TopKOperator {
    pub fn new(
        input: Box<dyn PipelineOperator>,
        top_k: usize,
        min_score: Option<f32>,
        needs_metadata: bool,
    ) -> Self {
        Self {
            input,
            queue: BoundedPriorityQueue::new(top_k),
            top_k,
            min_score,
            needs_metadata,
            exhausted: false,
        }
    }

    /// Consume all input and return the top-k results
    pub fn drain(mut self) -> Result<Vec<OptimizedSearchRecord>> {
        // Pull all chunks through the pipeline
        while let Some(chunk) = self.next_chunk()? {
            let _ = chunk; // next_chunk already inserts into queue
        }

        Ok(self.queue.into_sorted_vec())
    }

    /// Extract an OptimizedSearchRecord from a scored batch row.
    fn extract_result(
        &self,
        batch: &RecordBatch,
        row_idx: usize,
        score: f32,
    ) -> Option<OptimizedSearchRecord> {
        let id_array = batch
            .column_by_name("id")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())?;

        let id = id_array.value(row_idx).to_string();

        // Extract vector
        let vector = batch
            .column_by_name("vector")
            .or_else(|| batch.column_by_name("vector_fp32"))
            .and_then(|col| {
                if let Some(fl) = col.as_any().downcast_ref::<FixedSizeListArray>() {
                    let values = fl.values().as_any().downcast_ref::<Float32Array>()?;
                    let dim = fl.value_length() as usize;
                    let start = row_idx * dim;
                    Some(Arc::new(
                        (start..start + dim)
                            .map(|i| values.value(i))
                            .collect::<Vec<f32>>(),
                    ))
                } else {
                    None
                }
            });

        let version = batch
            .column_by_name("version")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::Int64Array>())
            .map(|a| a.value(row_idx) as u32);

        let timestamp = batch
            .column_by_name("timestamp")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::Int64Array>())
            .map(|a| a.value(row_idx));

        Some(OptimizedSearchRecord {
            id: id.clone(),
            vector_id: Some(id),
            score,
            similarity: Some(score),
            vector,
            metadata: HashMap::new(), // Metadata extracted separately if needed
            debug_info: None,
            version,
            timestamp,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: vec![],
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
            ..Default::default()
        })
    }
}

impl PipelineOperator for TopKOperator {
    fn next_chunk(&mut self) -> Result<Option<DataChunk>> {
        if self.exhausted {
            return Ok(None);
        }

        // Consume all input chunks, inserting candidates into priority queue
        while let Some(chunk) = self.input.next_chunk()? {
            let batch = chunk.materialize()?;

            // Get the _score column
            let score_array = batch
                .column_by_name("_score")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            let scores = match score_array {
                Some(arr) => arr,
                None => continue,
            };

            for row_idx in 0..batch.num_rows() {
                let score = scores.value(row_idx);

                if let Some(min) = self.min_score
                    && score < min
                {
                    continue;
                }

                if !self.queue.would_accept(score) {
                    continue;
                }

                if let Some(record) = self.extract_result(&batch, row_idx, score) {
                    self.queue.try_insert(record);
                }
            }
        }

        self.exhausted = true;
        Ok(None) // TopK is a breaker — results obtained via drain()
    }

    fn name(&self) -> &str {
        "TopK"
    }
}

// ─── Pipeline Builder ────────────────────────────────────────────────────────

/// Builder for constructing query execution pipelines.
///
/// # Example
/// ```rust,ignore
/// let results = PipelineBuilder::new(batches)
///     .filter(conditions)
///     .score(query_vector, DistanceMetric::Cosine)
///     .top_k(10, None, false)
///     .execute()?;
/// ```
pub struct PipelineBuilder {
    operator: Box<dyn PipelineOperator>,
}

impl PipelineBuilder {
    /// Start a pipeline from pre-loaded RecordBatches
    pub fn new(batches: Vec<RecordBatch>) -> Self {
        Self {
            operator: Box::new(ScanOperator::new(batches)),
        }
    }

    /// Add vectorized predicate filtering
    pub fn filter(self, conditions: Vec<FilterCondition>) -> Self {
        if conditions.is_empty() {
            return self;
        }
        Self {
            operator: Box::new(FilterOperator::new(self.operator, conditions)),
        }
    }

    /// Add SIMD-accelerated distance scoring
    pub fn score(self, query_vector: Vec<f32>, metric: DistanceMetric) -> Self {
        Self {
            operator: Box::new(ScoreOperator::new(self.operator, query_vector, metric)),
        }
    }

    /// Add top-k selection (pipeline breaker) and execute
    pub fn top_k(
        self,
        k: usize,
        min_score: Option<f32>,
        needs_metadata: bool,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let topk_op = TopKOperator::new(self.operator, k, min_score, needs_metadata);
        topk_op.drain()
    }

    /// Execute pipeline and collect all output chunks (for non-TopK pipelines)
    pub fn collect(mut self) -> Result<Vec<RecordBatch>> {
        let mut result = Vec::new();
        while let Some(chunk) = self.operator.next_chunk()? {
            result.push(chunk.materialize()?);
        }
        Ok(result)
    }
}

// ─── Pipeline Statistics ─────────────────────────────────────────────────────

/// Statistics collected during pipeline execution
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    pub chunks_processed: usize,
    pub rows_in: usize,
    pub rows_out: usize,
    pub operator_name: String,
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};

    fn make_test_batch(num_rows: usize) -> RecordBatch {
        let dim = 4;
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    dim as i32,
                ),
                false,
            ),
            Field::new("category", DataType::Utf8, false),
        ]));

        let ids: Vec<String> = (0..num_rows).map(|i| format!("vec_{}", i)).collect();
        let id_array = StringArray::from(ids);

        // Create vectors with varying similarity to [1,0,0,0]
        let mut vector_values = Vec::with_capacity(num_rows * dim);
        for i in 0..num_rows {
            let angle = (i as f32) / (num_rows as f32) * std::f32::consts::PI;
            vector_values.push(angle.cos());
            vector_values.push(angle.sin());
            vector_values.push(0.0);
            vector_values.push(0.0);
        }
        let values_array = Float32Array::from(vector_values);
        let vector_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            dim as i32,
            Arc::new(values_array),
            None,
        )
        .unwrap();

        let categories: Vec<&str> = (0..num_rows)
            .map(|i| if i % 2 == 0 { "even" } else { "odd" })
            .collect();
        let cat_array = StringArray::from(categories);

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(vector_array),
                Arc::new(cat_array),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_scan_operator() {
        let batch = make_test_batch(10);
        let mut scan = ScanOperator::new(vec![batch]);

        let chunk = scan.next_chunk().unwrap();
        assert!(chunk.is_some());
        assert_eq!(chunk.unwrap().batch().num_rows(), 10);

        let chunk2 = scan.next_chunk().unwrap();
        assert!(chunk2.is_none());
    }

    #[test]
    fn test_filter_operator() {
        let batch = make_test_batch(10);
        let scan = ScanOperator::new(vec![batch]);

        let conditions = vec![FilterCondition::Equals(
            "category".to_string(),
            serde_json::json!("even"),
        )];

        let mut filter = FilterOperator::new(Box::new(scan), conditions);

        let chunk = filter.next_chunk().unwrap().unwrap();
        // 5 even rows out of 10
        assert_eq!(chunk.batch().num_rows(), 5);
    }

    #[test]
    fn test_score_operator() {
        let batch = make_test_batch(5);
        let scan = ScanOperator::new(vec![batch]);

        let query = vec![1.0, 0.0, 0.0, 0.0]; // unit vector along first axis
        let mut score_op = ScoreOperator::new(Box::new(scan), query, DistanceMetric::Cosine);

        let chunk = score_op.next_chunk().unwrap().unwrap();
        let scored_batch = chunk.batch();

        // Should have _score column
        assert!(scored_batch.column_by_name("_score").is_some());
        assert_eq!(scored_batch.num_rows(), 5);

        // First row (angle=0, cos=1) should have highest score
        let scores = scored_batch
            .column_by_name("_score")
            .unwrap()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        assert!(scores.value(0) > scores.value(4));
    }

    #[test]
    fn test_pipeline_builder_full() {
        let batch = make_test_batch(20);

        let results = PipelineBuilder::new(vec![batch])
            .filter(vec![FilterCondition::Equals(
                "category".to_string(),
                serde_json::json!("even"),
            )])
            .score(vec![1.0, 0.0, 0.0, 0.0], DistanceMetric::Cosine)
            .top_k(3, None, false)
            .unwrap();

        assert_eq!(results.len(), 3);
        // Results should be sorted by score descending
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }

    #[test]
    fn test_pipeline_collect() {
        let batch = make_test_batch(10);

        let batches = PipelineBuilder::new(vec![batch])
            .filter(vec![FilterCondition::Equals(
                "category".to_string(),
                serde_json::json!("odd"),
            )])
            .collect()
            .unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 5);
    }

    #[test]
    fn test_pipeline_empty_input() {
        let results = PipelineBuilder::new(vec![])
            .score(vec![1.0, 0.0], DistanceMetric::Cosine)
            .top_k(5, None, false)
            .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_pipeline_min_score_threshold() {
        let batch = make_test_batch(20);

        let results = PipelineBuilder::new(vec![batch])
            .score(vec![1.0, 0.0, 0.0, 0.0], DistanceMetric::Cosine)
            .top_k(100, Some(0.9), false)
            .unwrap();

        // Only vectors with similarity > 0.9 to [1,0,0,0]
        for r in &results {
            assert!(r.score >= 0.9, "Score {} below threshold 0.9", r.score);
        }
    }
}
