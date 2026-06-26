//! Re-quantization Manager - Update quantization codebooks when data distribution changes
//!
//! This module implements intelligent re-quantization strategies that monitor data distribution
//! changes and automatically update PQ codebooks, scalar quantization thresholds, and binary
//! sketches to maintain optimal compression and search quality.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::RequantizationResult;
use proximadb_proto::proximadb_v1::StorageEngine as StorageEngineType;

/// Re-quantization manager coordinates codebook updates across storage engines
pub struct RequantizationManager {
    /// Configuration for re-quantization behavior
    config: RequantizationConfig,

    /// Current re-quantization state
    state: Arc<RwLock<RequantizationState>>,

    /// Performance metrics tracking
    #[allow(dead_code)]
    metrics: Arc<RequantizationMetrics>,

    /// Data distribution analyzers by collection (wrapped in RwLock for interior mutability)
    analyzers: Arc<RwLock<HashMap<String, Arc<RwLock<DataDistributionAnalyzer>>>>>,
}

/// Re-quantization configuration
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RequantizationConfig {
    /// Minimum quality degradation threshold to trigger re-quantization
    quality_degradation_threshold: f64,

    /// Maximum time between forced re-quantization checks
    max_analysis_interval: Duration,

    /// Minimum data volume change to consider re-quantization
    min_data_change_ratio: f64,

    /// Target compression ratio for quantization
    target_compression_ratio: f64,

    /// Number of samples to analyze for distribution changes
    analysis_sample_size: usize,
}

impl Default for RequantizationConfig {
    fn default() -> Self {
        Self {
            quality_degradation_threshold: 0.05, // 5% quality drop triggers re-quantization
            max_analysis_interval: Duration::from_secs(12 * 60 * 60), // 12 hours
            min_data_change_ratio: 0.20,         // 20% data change minimum
            target_compression_ratio: 0.25,      // 4:1 compression target
            analysis_sample_size: 10000,
        }
    }
}

/// Current re-quantization state
#[derive(Debug, Default)]
#[allow(dead_code)]
struct RequantizationState {
    /// Active re-quantization operations
    active_operations: HashMap<String, ActiveRequantization>,

    /// Last analysis time by collection
    last_analysis_time: HashMap<String, Instant>,

    /// Re-quantization statistics
    stats: RequantizationStatistics,

    /// Quality metrics by collection
    quality_metrics: HashMap<String, QuantizationQuality>,
}

/// Active re-quantization tracking
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ActiveRequantization {
    operation_id: String,
    collection_id: String,
    quantization_type: QuantizationType,
    started_at: Instant,
    estimated_completion: Instant,
    current_phase: RequantizationPhase,
}

/// Types of quantization operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationType {
    /// Product Quantization codebook updates
    ProductQuantization,
    /// Scalar quantization threshold updates
    ScalarQuantization,
    /// Binary sketch updates
    BinaryQuantization,
    /// Full re-quantization (all types)
    FullRequantization,
}

/// Re-quantization execution phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequantizationPhase {
    /// Analyzing data distribution
    Analysis,
    /// Computing new codebooks
    CodebookGeneration,
    /// Re-encoding existing data
    DataReencoding,
    /// Updating indices
    IndexUpdate,
    /// Finalizing and cleanup
    Finalization,
}

/// Re-quantization performance statistics
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
struct RequantizationStatistics {
    total_requantizations: u64,
    successful_requantizations: u64,
    failed_requantizations: u64,
    total_vectors_requantized: u64,
    average_quality_improvement: f64,
    average_requantization_time: Duration,
}

/// Quantization quality metrics
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct QuantizationQuality {
    current_quality_score: f64,
    baseline_quality_score: f64,
    compression_ratio: f64,
    last_updated: Instant,
    degradation_trend: QualityTrend,
}

/// Quality trend tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum QualityTrend {
    Improving,
    Stable,
    Degrading,
    RapidDegradation,
}

/// Data distribution analyzer for monitoring quantization effectiveness
#[derive(Debug)]
pub struct DataDistributionAnalyzer {
    collection_id: String,
    last_analysis: Option<DataDistributionSnapshot>,
    analysis_history: Vec<DataDistributionSnapshot>,
    config: RequantizationConfig,
}

/// Snapshot of data distribution characteristics
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DataDistributionSnapshot {
    timestamp: Instant,
    vector_count: usize,
    dimensionality: usize,
    mean_vector: Vec<f32>,
    variance_vector: Vec<f32>,
    quantization_error: f64,
    clustering_entropy: f64,
}

/// Re-quantization metrics tracking
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct RequantizationMetrics {
    /// Performance counters
    counters: Arc<RwLock<RequantizationStatistics>>,
}

impl RequantizationManager {
    /// Create new re-quantization manager
    pub fn new() -> Result<Self> {
        info!("🔄 Initializing RequantizationManager");

        Ok(Self {
            config: RequantizationConfig::default(),
            state: Arc::new(RwLock::new(RequantizationState::default())),
            metrics: Arc::new(RequantizationMetrics::default()),
            analyzers: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Analyze collection and determine if re-quantization is needed
    pub async fn analyze_collection(
        &self,
        collection_id: &str,
        engine_type: StorageEngineType,
    ) -> Result<bool> {
        debug!(
            "🔍 Analyzing collection {} for re-quantization (engine: {:?})",
            collection_id, engine_type
        );

        // Get or create analyzer for this collection
        let analyzer = {
            let mut analyzers = self.analyzers.write().await;
            analyzers
                .entry(collection_id.to_string())
                .or_insert_with(|| {
                    Arc::new(RwLock::new(DataDistributionAnalyzer::new(
                        collection_id.to_string(),
                        self.config.clone(),
                    )))
                })
                .clone()
        };

        // Perform data distribution analysis
        let current_snapshot = self
            .collect_distribution_snapshot(collection_id, engine_type)
            .await?;
        let needs_requantization = analyzer
            .write()
            .await
            .analyze_distribution_change(&current_snapshot)
            .await?;

        // Update analysis time
        {
            let mut state = self.state.write().await;
            state
                .last_analysis_time
                .insert(collection_id.to_string(), Instant::now());
        }

        if needs_requantization {
            info!(
                "📊 Collection {} needs re-quantization based on distribution analysis",
                collection_id
            );
        } else {
            debug!(
                "📊 Collection {} quantization is still optimal",
                collection_id
            );
        }

        Ok(needs_requantization)
    }

    /// Execute re-quantization for a collection
    pub async fn execute_requantization(
        &self,
        collection_id: &str,
        quantization_type: QuantizationType,
        engine_type: StorageEngineType,
    ) -> Result<RequantizationResult> {
        info!(
            "🔄 Executing {:?} re-quantization for collection: {} (engine: {:?})",
            quantization_type, collection_id, engine_type
        );

        let start_time = Instant::now();
        let operation_id = format!(
            "requant_{}_{}_{}",
            collection_id,
            format!("{:?}", quantization_type).to_lowercase(),
            chrono::Utc::now().timestamp_millis()
        );

        // Record active operation
        {
            let mut state = self.state.write().await;
            state.active_operations.insert(
                operation_id.clone(),
                ActiveRequantization {
                    operation_id: operation_id.clone(),
                    collection_id: collection_id.to_string(),
                    quantization_type,
                    started_at: start_time,
                    estimated_completion: start_time + Duration::from_secs(300), // 5 minutes estimate
                    current_phase: RequantizationPhase::Analysis,
                },
            );
        }

        // Execute re-quantization phases
        let result = self
            .execute_requantization_phases(
                collection_id,
                quantization_type,
                engine_type,
                &operation_id,
            )
            .await?;

        // Update metrics and clean up
        let duration = start_time.elapsed();
        {
            let mut state = self.state.write().await;
            state.active_operations.remove(&operation_id);

            state.stats.total_requantizations += 1;
            state.stats.successful_requantizations += 1;
            state.stats.total_vectors_requantized += result.codebooks_updated.len() as u64;
        }

        info!(
            "✅ Re-quantization completed for collection: {} in {:?} (quality improvement: {:.2}%)",
            collection_id,
            duration,
            result.quality_improvement * 100.0
        );

        Ok(result)
    }

    /// Check if collection needs re-quantization based on automatic triggers
    pub async fn needs_requantization(
        &self,
        collection_id: &str,
        _engine_type: StorageEngineType,
    ) -> bool {
        let state = self.state.read().await;

        // Check time-based triggers
        if let Some(last_time) = state.last_analysis_time.get(collection_id) {
            if last_time.elapsed() > self.config.max_analysis_interval {
                return true;
            }
        } else {
            // Never analyzed = no prior quantization exists, nothing to re-quantize
            return false;
        }

        // Check quality degradation
        if let Some(quality) = state.quality_metrics.get(collection_id) {
            let quality_degradation = (quality.baseline_quality_score
                - quality.current_quality_score)
                / quality.baseline_quality_score;

            if quality_degradation > self.config.quality_degradation_threshold {
                return true;
            }

            if matches!(quality.degradation_trend, QualityTrend::RapidDegradation) {
                return true;
            }
        }

        false
    }

    /// Get current re-quantization status for monitoring
    pub async fn get_requantization_status(&self) -> RequantizationStatus {
        let state = self.state.read().await;

        RequantizationStatus {
            active_requantizations: state.active_operations.len(),
            total_requantizations_completed: state.stats.total_requantizations,
            average_quality_improvement: state.stats.average_quality_improvement,
            collections_monitored: state.quality_metrics.len(),
        }
    }

    // Private implementation methods

    async fn collect_distribution_snapshot(
        &self,
        collection_id: &str,
        _engine_type: StorageEngineType,
    ) -> Result<DataDistributionSnapshot> {
        debug!(
            "📊 Collecting data distribution snapshot for collection: {}",
            collection_id
        );

        // Deferred: Implement actual data sampling and analysis
        // 1. Sample vectors from storage
        // 2. Compute distribution statistics (mean, variance, entropy)
        // 3. Measure current quantization error
        // 4. Analyze clustering quality

        Ok(DataDistributionSnapshot {
            timestamp: Instant::now(),
            vector_count: 10000,             // Placeholder
            dimensionality: 384,             // Placeholder
            mean_vector: vec![0.0; 384],     // Placeholder
            variance_vector: vec![1.0; 384], // Placeholder
            quantization_error: 0.05,        // Placeholder
            clustering_entropy: 0.85,        // Placeholder
        })
    }

    async fn execute_requantization_phases(
        &self,
        collection_id: &str,
        quantization_type: QuantizationType,
        _engine_type: StorageEngineType,
        operation_id: &str,
    ) -> Result<RequantizationResult> {
        let mut codebooks_updated = Vec::new();
        let start_quality = 0.80; // Placeholder baseline

        // Phase 1: Analysis
        self.update_operation_phase(operation_id, RequantizationPhase::Analysis)
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await; // Simulate analysis time

        // Phase 2: Codebook Generation
        self.update_operation_phase(operation_id, RequantizationPhase::CodebookGeneration)
            .await;
        match quantization_type {
            QuantizationType::ProductQuantization => {
                codebooks_updated.push("pq_codebook_updated".to_string());
            }
            QuantizationType::ScalarQuantization => {
                codebooks_updated.push("scalar_thresholds_updated".to_string());
            }
            QuantizationType::BinaryQuantization => {
                codebooks_updated.push("binary_sketches_updated".to_string());
            }
            QuantizationType::FullRequantization => {
                codebooks_updated.extend([
                    "pq_codebook_updated".to_string(),
                    "scalar_thresholds_updated".to_string(),
                    "binary_sketches_updated".to_string(),
                ]);
            }
        }

        // Phase 3: Data Re-encoding
        self.update_operation_phase(operation_id, RequantizationPhase::DataReencoding)
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await; // Simulate re-encoding time

        // Phase 4: Index Update
        self.update_operation_phase(operation_id, RequantizationPhase::IndexUpdate)
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await; // Simulate index update time

        // Phase 5: Finalization
        self.update_operation_phase(operation_id, RequantizationPhase::Finalization)
            .await;

        let end_quality = 0.90; // Placeholder improved quality
        let quality_improvement = (end_quality - start_quality) / start_quality;

        Ok(RequantizationResult {
            collection_id: collection_id.to_string(),
            codebooks_updated,
            quality_improvement,
            duration: Duration::from_secs(2), // Placeholder
        })
    }

    async fn update_operation_phase(&self, operation_id: &str, phase: RequantizationPhase) {
        let mut state = self.state.write().await;
        if let Some(operation) = state.active_operations.get_mut(operation_id) {
            operation.current_phase = phase;
            debug!(
                "🔄 Re-quantization {} entered phase: {:?}",
                operation_id, phase
            );
        }
    }
}

impl DataDistributionAnalyzer {
    fn new(collection_id: String, config: RequantizationConfig) -> Self {
        Self {
            collection_id,
            last_analysis: None,
            analysis_history: Vec::new(),
            config,
        }
    }

    async fn analyze_distribution_change(
        &mut self,
        snapshot: &DataDistributionSnapshot,
    ) -> Result<bool> {
        // Store current analysis
        if let Some(ref last) = self.last_analysis {
            self.analysis_history.push(last.clone());
        }
        self.last_analysis = Some(snapshot.clone());

        // Keep only recent history
        if self.analysis_history.len() > 10 {
            self.analysis_history.remove(0);
        }

        // Analyze changes if we have previous data
        if let Some(previous) = self.analysis_history.last() {
            let quality_change = (snapshot.quantization_error - previous.quantization_error).abs();
            let entropy_change = (snapshot.clustering_entropy - previous.clustering_entropy).abs();
            let data_change_ratio = (snapshot.vector_count as f64 - previous.vector_count as f64)
                .abs()
                / previous.vector_count as f64;

            // Determine if re-quantization is needed
            let needs_requantization = quality_change > self.config.quality_degradation_threshold ||
                entropy_change > 0.1 || // Significant clustering change
                data_change_ratio > self.config.min_data_change_ratio;

            debug!(
                "📊 Distribution analysis for {}: quality_change={:.4}, entropy_change={:.4}, data_change_ratio={:.4}, needs_requantization={}",
                self.collection_id,
                quality_change,
                entropy_change,
                data_change_ratio,
                needs_requantization
            );

            Ok(needs_requantization)
        } else {
            // First analysis, no re-quantization needed yet
            Ok(false)
        }
    }
}

/// Current re-quantization status for monitoring
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequantizationStatus {
    pub active_requantizations: usize,
    pub total_requantizations_completed: u64,
    pub average_quality_improvement: f64,
    pub collections_monitored: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_requantization_manager_creation() {
        let manager = RequantizationManager::new().unwrap();
        let status = manager.get_requantization_status().await;

        assert_eq!(status.active_requantizations, 0);
        assert_eq!(status.total_requantizations_completed, 0);
    }

    #[tokio::test]
    async fn test_collection_analysis() {
        let manager = RequantizationManager::new().unwrap();

        // Test analysis without existing data
        assert!(
            !manager
                .needs_requantization("test_collection", StorageEngineType::Sst)
                .await
        );
    }

    #[tokio::test]
    async fn test_data_distribution_analyzer() {
        let config = RequantizationConfig::default();
        let mut analyzer = DataDistributionAnalyzer::new("test_collection".to_string(), config);

        let snapshot = DataDistributionSnapshot {
            timestamp: Instant::now(),
            vector_count: 1000,
            dimensionality: 128,
            mean_vector: vec![0.0; 128],
            variance_vector: vec![1.0; 128],
            quantization_error: 0.05,
            clustering_entropy: 0.85,
        };

        // First analysis should not trigger re-quantization
        let needs_requant = analyzer
            .analyze_distribution_change(&snapshot)
            .await
            .unwrap();
        assert!(!needs_requant);
    }
}

// ---------------------------------------------------------------------------
// PR 10 — PQ Codebook lifecycle for embedding-precision rollout (Q12)
// ---------------------------------------------------------------------------
//
// Spec: docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc §"PQ
// Codebook Lifecycle (Q12)".
//
// Each PQ codebook is tagged with the precision epoch it was trained
// under. When xCatalog bumps `current_precision_epoch` on a collection,
// the migration job walks every codebook for that collection and
// transitions Fresh → Stale. Stale codebooks keep serving candidate
// generation (rerank-against-canonical guarantees correctness); the
// background retraining task atomic-swaps Stale → Retraining → Fresh
// once new centroids land.

use proximadb_records::EmbeddingScalarType;

/// Status of a PQ codebook in the precision-migration lifecycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CodebookStatus {
    /// Built from the current canonical precision. Safe for derived
    /// ranking. Default for any newly trained codebook.
    #[default]
    Fresh,
    /// Canonical precision changed since training. Safe for lossy
    /// candidate generation when callers rerank against canonical;
    /// scheduled for retraining.
    Stale {
        /// Epoch the canonical precision moved to (the "new" epoch the
        /// codebook needs to be retrained against).
        since_epoch: u64,
    },
    /// A retraining job has claimed this codebook and is currently
    /// computing new centroids. The job swaps back to `Fresh` (with a
    /// fresh `built_at_epoch` + `built_from_precision`) on success.
    Retraining,
}

impl CodebookStatus {
    /// Whether this codebook is allowed to serve derived candidate
    /// generation. LLD §Q12 contract: Fresh and Stale both serve
    /// candidates; only Retraining is held out (its centroids are mid-
    /// swap and reads should fall back to the previous Fresh snapshot).
    pub fn can_serve_candidates(self) -> bool {
        matches!(self, Self::Fresh | Self::Stale { .. })
    }

    /// Whether the background retraining job should pick this codebook
    /// up. Only `Stale` is up for grabs; Fresh and Retraining are no-ops.
    pub fn needs_retraining(self) -> bool {
        matches!(self, Self::Stale { .. })
    }
}

/// PQ codebook envelope with the LLD-locked precision tagging.
///
/// `centroids` is an opaque byte payload owned by the actual quantizer
/// implementation (the lifecycle module doesn't care about the centroid
/// data layout — only the routing tags).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PqCodebook {
    pub codebook_id: String,
    pub collection_id: String,
    pub subvector_count: u16,
    pub bits_per_subvector: u8,
    /// Precision the source vectors carried when this codebook was
    /// trained. Used by the migration job to decide which codebooks need
    /// retraining when a collection's canonical precision changes.
    pub built_from_precision: EmbeddingScalarType,
    /// Epoch the source vectors were tagged with at training time.
    pub built_at_epoch: u64,
    /// Lifecycle state. Defaults to `Fresh` for newly trained codebooks.
    #[serde(default)]
    pub status: CodebookStatus,
    /// Opaque centroid payload (typically `subvector_count * 2^bits *
    /// dim_per_subvector * sizeof(fp32)` bytes). The lifecycle module
    /// never touches these bytes.
    pub centroids: Vec<u8>,
}

impl PqCodebook {
    /// Construct a newly-trained codebook in `Fresh` status.
    pub fn new_fresh(
        codebook_id: impl Into<String>,
        collection_id: impl Into<String>,
        subvector_count: u16,
        bits_per_subvector: u8,
        built_from_precision: EmbeddingScalarType,
        built_at_epoch: u64,
        centroids: Vec<u8>,
    ) -> Self {
        Self {
            codebook_id: codebook_id.into(),
            collection_id: collection_id.into(),
            subvector_count,
            bits_per_subvector,
            built_from_precision,
            built_at_epoch,
            status: CodebookStatus::Fresh,
            centroids,
        }
    }

    /// Mark this codebook stale because the collection's canonical
    /// precision (or precision epoch) moved forward.
    ///
    /// Returns `true` if the status actually changed, `false` if the
    /// codebook was already Stale at this or a later epoch. The bool
    /// lets the migration job count exactly how many codebooks it
    /// flipped per epoch bump (for the `migration_progress_ratio`
    /// metric).
    ///
    /// Refuses to overwrite a `Retraining` status: if a retraining job
    /// is mid-flight when the next epoch bump happens, the new epoch
    /// gets recorded on the retraining job's swap-back path (the job
    /// re-checks the current epoch before promoting back to `Fresh`).
    pub fn mark_stale_on_epoch_bump(&mut self, new_epoch: u64) -> bool {
        match self.status {
            CodebookStatus::Fresh => {
                self.status = CodebookStatus::Stale {
                    since_epoch: new_epoch,
                };
                true
            }
            CodebookStatus::Stale { since_epoch } if new_epoch > since_epoch => {
                self.status = CodebookStatus::Stale {
                    since_epoch: new_epoch,
                };
                true
            }
            CodebookStatus::Stale { .. } | CodebookStatus::Retraining => false,
        }
    }

    /// Background retraining job claim. Transitions `Stale → Retraining`
    /// atomically; returns `true` on successful claim. Idempotent: if
    /// the codebook is already `Retraining`, returns `false` so callers
    /// don't double-claim.
    pub fn try_claim_for_retraining(&mut self) -> bool {
        if matches!(self.status, CodebookStatus::Stale { .. }) {
            self.status = CodebookStatus::Retraining;
            true
        } else {
            false
        }
    }

    /// Retraining job completion. Atomically swaps in fresh centroids +
    /// the new precision tag + epoch. Requires the codebook to be in
    /// `Retraining` state — calling this on a Fresh/Stale codebook is a
    /// programming error and returns `false` without mutation.
    pub fn finish_retraining(
        &mut self,
        new_centroids: Vec<u8>,
        new_precision: EmbeddingScalarType,
        new_epoch: u64,
    ) -> bool {
        if !matches!(self.status, CodebookStatus::Retraining) {
            return false;
        }
        self.centroids = new_centroids;
        self.built_from_precision = new_precision;
        self.built_at_epoch = new_epoch;
        self.status = CodebookStatus::Fresh;
        true
    }
}

#[cfg(test)]
mod codebook_lifecycle_tests {
    use super::*;

    fn sample_codebook() -> PqCodebook {
        PqCodebook::new_fresh(
            "cb_001",
            "collection_a",
            8,
            8,
            EmbeddingScalarType::Fp32,
            1,
            vec![0u8; 64],
        )
    }

    #[test]
    fn default_status_is_fresh() {
        assert_eq!(CodebookStatus::default(), CodebookStatus::Fresh);
    }

    #[test]
    fn fresh_and_stale_serve_candidates_retraining_does_not() {
        assert!(CodebookStatus::Fresh.can_serve_candidates());
        assert!(CodebookStatus::Stale { since_epoch: 5 }.can_serve_candidates());
        assert!(!CodebookStatus::Retraining.can_serve_candidates());
    }

    #[test]
    fn only_stale_needs_retraining() {
        assert!(!CodebookStatus::Fresh.needs_retraining());
        assert!(CodebookStatus::Stale { since_epoch: 5 }.needs_retraining());
        assert!(!CodebookStatus::Retraining.needs_retraining());
    }

    #[test]
    fn new_fresh_constructor_sets_lld_fields() {
        let cb = sample_codebook();
        assert_eq!(cb.codebook_id, "cb_001");
        assert_eq!(cb.collection_id, "collection_a");
        assert_eq!(cb.subvector_count, 8);
        assert_eq!(cb.bits_per_subvector, 8);
        assert_eq!(cb.built_from_precision, EmbeddingScalarType::Fp32);
        assert_eq!(cb.built_at_epoch, 1);
        assert_eq!(cb.status, CodebookStatus::Fresh);
    }

    #[test]
    fn mark_stale_transitions_fresh_to_stale() {
        let mut cb = sample_codebook();
        assert!(cb.mark_stale_on_epoch_bump(2));
        assert_eq!(cb.status, CodebookStatus::Stale { since_epoch: 2 });
    }

    #[test]
    fn mark_stale_bumps_since_epoch_when_new_epoch_is_higher() {
        let mut cb = sample_codebook();
        cb.mark_stale_on_epoch_bump(2);
        assert!(cb.mark_stale_on_epoch_bump(5));
        assert_eq!(cb.status, CodebookStatus::Stale { since_epoch: 5 });
    }

    #[test]
    fn mark_stale_noop_when_already_stale_at_same_or_earlier_epoch() {
        let mut cb = sample_codebook();
        cb.mark_stale_on_epoch_bump(5);
        assert!(!cb.mark_stale_on_epoch_bump(5), "same epoch is a no-op");
        assert!(!cb.mark_stale_on_epoch_bump(3), "earlier epoch is a no-op");
        assert_eq!(cb.status, CodebookStatus::Stale { since_epoch: 5 });
    }

    #[test]
    fn mark_stale_refuses_to_overwrite_retraining() {
        // Concurrent epoch bump while a retraining job is mid-flight:
        // the job's swap-back path handles the new epoch; the migration
        // job's mark_stale must NOT clobber the in-flight job's state.
        let mut cb = sample_codebook();
        cb.mark_stale_on_epoch_bump(2);
        assert!(cb.try_claim_for_retraining());
        assert_eq!(cb.status, CodebookStatus::Retraining);
        assert!(
            !cb.mark_stale_on_epoch_bump(3),
            "must not overwrite Retraining"
        );
        assert_eq!(cb.status, CodebookStatus::Retraining);
    }

    #[test]
    fn try_claim_only_picks_up_stale_codebooks() {
        let mut cb = sample_codebook();
        assert!(!cb.try_claim_for_retraining(), "Fresh is not claimable");
        cb.mark_stale_on_epoch_bump(2);
        assert!(cb.try_claim_for_retraining());
        assert!(
            !cb.try_claim_for_retraining(),
            "Retraining double-claim refused"
        );
    }

    #[test]
    fn finish_retraining_swaps_centroids_precision_and_epoch_atomically() {
        let mut cb = sample_codebook();
        cb.mark_stale_on_epoch_bump(2);
        cb.try_claim_for_retraining();
        let new_centroids = vec![0xAA; 64];
        assert!(cb.finish_retraining(new_centroids.clone(), EmbeddingScalarType::Fp16, 2));
        assert_eq!(cb.status, CodebookStatus::Fresh);
        assert_eq!(cb.built_from_precision, EmbeddingScalarType::Fp16);
        assert_eq!(cb.built_at_epoch, 2);
        assert_eq!(cb.centroids, new_centroids);
    }

    #[test]
    fn finish_retraining_refuses_when_not_retraining() {
        // Programming-error guard: calling finish on a Fresh/Stale
        // codebook returns false without mutation.
        let mut cb = sample_codebook();
        assert!(
            !cb.finish_retraining(vec![0u8; 64], EmbeddingScalarType::Fp16, 2),
            "Fresh codebook must reject finish_retraining"
        );
        assert_eq!(cb.status, CodebookStatus::Fresh);
        cb.mark_stale_on_epoch_bump(2);
        assert!(
            !cb.finish_retraining(vec![0u8; 64], EmbeddingScalarType::Fp16, 2),
            "Stale (un-claimed) codebook must reject finish_retraining"
        );
        assert!(matches!(cb.status, CodebookStatus::Stale { .. }));
    }

    #[test]
    fn codebook_serde_round_trips_each_status() {
        let mut cb = sample_codebook();
        // Fresh round-trip
        let json = serde_json::to_string(&cb).unwrap();
        let back: PqCodebook = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cb);
        // Stale round-trip
        cb.mark_stale_on_epoch_bump(5);
        let json = serde_json::to_string(&cb).unwrap();
        let back: PqCodebook = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cb);
        // Retraining round-trip
        cb.try_claim_for_retraining();
        let json = serde_json::to_string(&cb).unwrap();
        let back: PqCodebook = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cb);
    }

    #[test]
    fn full_lifecycle_walk_from_fresh_to_stale_to_retraining_to_fresh() {
        // The full LLD §Q12 happy path: epoch bumps, migration marks
        // stale, retrainer claims + finishes, codebook is back to Fresh
        // at the new epoch + precision.
        let mut cb = sample_codebook();
        assert_eq!(cb.status, CodebookStatus::Fresh);
        assert_eq!(cb.built_at_epoch, 1);

        // Catalog bumps current_precision_epoch from 1 -> 2 (and
        // canonical precision from fp32 to fp16).
        assert!(cb.mark_stale_on_epoch_bump(2));
        assert!(cb.status.can_serve_candidates(), "stale still serves");

        // Retraining job picks it up.
        assert!(cb.try_claim_for_retraining());
        assert!(!cb.status.can_serve_candidates(), "retraining is held out");

        // Job finishes — codebook is at the new epoch + precision.
        let new_centroids = vec![0xFF; 64];
        assert!(cb.finish_retraining(new_centroids.clone(), EmbeddingScalarType::Fp16, 2));
        assert_eq!(cb.status, CodebookStatus::Fresh);
        assert_eq!(cb.built_at_epoch, 2);
        assert_eq!(cb.built_from_precision, EmbeddingScalarType::Fp16);
        assert_eq!(cb.centroids, new_centroids);
        assert!(cb.status.can_serve_candidates());
    }
}
