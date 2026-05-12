//! Progressive search pipeline for vector operations.
//!
//! Implements multi-stage search with progressive quantization:
//! Binary filtering → INT8 approximation → PQ ranking → Full precision

/// Configuration for the progressive search pipeline.
#[derive(Debug, Clone)]
pub struct ProgressiveSearchPipeline {
    /// Enable progressive search
    pub enabled: bool,
    /// Custom recall targets for each stage
    pub recalls: Vec<f32>,
    /// Stage names for logging/explain
    pub stage_names: Vec<String>,
}

impl Default for ProgressiveSearchPipeline {
    fn default() -> Self {
        Self {
            enabled: true,
            recalls: vec![0.95, 0.90, 0.85], // Binary, INT8, PQ
            stage_names: vec!["binary".into(), "int8".into(), "pq".into(), "full".into()],
        }
    }
}

/// Build default progressive stages configuration.
pub fn default_progressive_stages() -> Vec<String> {
    vec!["binary".into(), "int8".into(), "pq".into(), "full".into()]
}

impl ProgressiveSearchPipeline {
    /// Create a new progressive pipeline with custom configuration.
    pub fn new(recalls: Vec<f32>) -> Self {
        Self {
            enabled: true,
            recalls,
            ..Default::default()
        }
    }

    /// Disable progressive search (use full precision only).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Calculate candidate count for a given stage.
    ///
    /// # Arguments
    ///
    /// * `final_k` - Target top-k result count
    /// * `stage_index` - Current stage index (0-based)
    ///
    /// # Returns
    ///
    /// Number of candidates to retrieve for this stage
    pub fn stage_candidates(&self, final_k: usize, stage_index: usize) -> usize {
        if !self.enabled || stage_index >= self.recalls.len() {
            return final_k;
        }

        let recall_product: f32 = self.recalls[..=stage_index].iter().product();
        let calculated = ((final_k as f32) / recall_product).ceil() as usize;
        calculated.max(final_k)
    }

    /// Get total number of stages including final full-precision stage.
    pub fn total_stages(&self) -> usize {
        if self.enabled {
            self.stage_names.len()
        } else {
            1
        }
    }

    /// Get stage name for logging.
    pub fn stage_name(&self, index: usize) -> Option<&str> {
        self.stage_names.get(index).map(|s| s.as_str())
    }
}

/// Stage result from progressive search pipeline.
#[derive(Debug, Clone)]
pub struct StageResult {
    /// Stage name
    pub stage: String,
    /// Candidates retrieved
    pub candidates: usize,
    /// Time taken in milliseconds
    pub duration_ms: u64,
    /// Estimated recall for this stage
    pub estimated_recall: Option<f32>,
}

impl StageResult {
    /// Create a new stage result.
    pub fn new(stage: String, candidates: usize, duration_ms: u64) -> Self {
        Self {
            stage,
            candidates,
            duration_ms,
            estimated_recall: None,
        }
    }

    /// Set estimated recall for this stage.
    pub fn with_recall(mut self, recall: f32) -> Self {
        self.estimated_recall = Some(recall);
        self
    }
}

/// Results from executing the progressive search pipeline.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Final results
    pub results: Vec<crate::proto::proximadb_v1::VectorRecord>,
    /// Results from each intermediate stage
    pub stages: Vec<StageResult>,
    /// Total pipeline execution time in milliseconds
    pub total_duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progressive_pipeline_default() {
        let pipeline = ProgressiveSearchPipeline::default();

        assert!(pipeline.enabled);
        assert_eq!(pipeline.recalls.len(), 3);
        assert_eq!(pipeline.total_stages(), 4);
    }

    #[test]
    fn test_stage_candidates() {
        let pipeline = ProgressiveSearchPipeline::default();
        let k = 10;

        // Stage 0: 10 / 0.95 ≈ 11
        let stage0 = pipeline.stage_candidates(k, 0);
        assert!(stage0 >= k);

        // Stage 1: 10 / (0.95 * 0.90) ≈ 12
        let stage1 = pipeline.stage_candidates(k, 1);
        assert!(stage1 >= stage0);
    }

    #[test]
    fn test_disabled_pipeline() {
        let pipeline = ProgressiveSearchPipeline::disabled();

        assert!(!pipeline.enabled);
        assert_eq!(pipeline.total_stages(), 1);
        assert_eq!(pipeline.stage_candidates(100, 0), 100);
    }

    #[test]
    fn test_stage_result() {
        let result = StageResult::new("binary".to_string(), 1000, 50).with_recall(0.95);

        assert_eq!(result.stage, "binary");
        assert_eq!(result.candidates, 1000);
        assert_eq!(result.duration_ms, 50);
        assert_eq!(result.estimated_recall, Some(0.95));
    }
}
