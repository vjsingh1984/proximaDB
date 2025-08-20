// Universal Search Modes and Capabilities
// Shared search abstractions across all storage engines

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::compute::distance_computation::DistanceMetric;

/// Placeholder for metadata filtering - use crate::query::unified_query_optimizer::UnifiedMetadataFilter instead
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilter {
    pub placeholder: bool,
}

/// Universal search mode that works across all storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UniversalSearchMode {
    /// Vector similarity search
    VectorSimilarity {
        query_vector: Vec<f32>,
        top_k: usize,
        distance_metric: DistanceMetric,
        search_params: SimilaritySearchParams,
    },
    
    /// ID-based lookup
    IdLookup {
        ids: Vec<String>,
        include_vectors: bool,
        include_metadata: bool,
    },
    
    /// Range-based search
    RangeSearch {
        query_vector: Vec<f32>,
        distance_threshold: f32,
        max_results: Option<usize>,
        distance_metric: DistanceMetric,
    },
    
    /// Filtered similarity search
    FilteredSimilarity {
        query_vector: Vec<f32>,
        top_k: usize,
        distance_metric: DistanceMetric,
        metadata_filter: MetadataFilter,
        search_params: SimilaritySearchParams,
    },
    
    /// Hybrid search (vector + text/metadata)
    HybridSearch {
        vector_query: Option<Vec<f32>>,
        text_query: Option<String>,
        metadata_filter: Option<MetadataFilter>,
        fusion_params: HybridFusionParams,
        top_k: usize,
    },
    
    /// Multi-vector search
    MultiVector {
        query_vectors: Vec<Vec<f32>>,
        combination_strategy: MultiVectorStrategy,
        top_k: usize,
        distance_metric: DistanceMetric,
    },
    
    /// Approximate nearest neighbors
    ApproximateNN {
        query_vector: Vec<f32>,
        top_k: usize,
        distance_metric: DistanceMetric,
        approximation_params: ApproximationParams,
    },
    
    /// Batch search
    BatchSearch {
        queries: Vec<SearchQuery>,
        batch_params: BatchSearchParams,
    },
    
    /// Progressive search with refinement
    ProgressiveSearch {
        query_vector: Vec<f32>,
        top_k: usize,
        distance_metric: DistanceMetric,
        progressive_params: ProgressiveSearchParams,
    },
    
    /// Custom search mode
    Custom {
        mode_name: String,
        parameters: HashMap<String, serde_json::Value>,
    },
}

/// Similarity search parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilaritySearchParams {
    /// Search quality vs speed trade-off
    pub quality_speed_balance: f32, // 0.0 = speed, 1.0 = quality
    
    /// Enable early termination
    pub enable_early_termination: bool,
    
    /// Search timeout (ms)
    pub timeout_ms: Option<u64>,
    
    /// Search hints
    pub search_hints: Vec<SearchHint>,
    
    /// Result diversification
    pub diversification: Option<DiversificationParams>,
    
    /// Search parallelization
    pub parallelization: ParallelizationParams,
}

/// Search hints for optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchHint {
    /// Expected result distribution
    ExpectedResultDistribution {
        clustered: bool,
        uniform: bool,
        sparse: bool,
    },
    
    /// Query characteristics
    QueryCharacteristics {
        query_selectivity: f32,
        query_complexity: QueryComplexity,
    },
    
    /// Data characteristics
    DataCharacteristics {
        data_distribution: DataDistribution,
        dimensionality: DimensionalityHint,
    },
    
    /// Performance preferences
    PerformancePreference {
        prefer_accuracy: bool,
        prefer_speed: bool,
        prefer_memory_efficiency: bool,
    },
    
    /// Resource constraints
    ResourceConstraints {
        max_memory_mb: Option<u64>,
        max_cpu_cores: Option<usize>,
        max_latency_ms: Option<u64>,
    },
}

/// Query complexity levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryComplexity {
    Simple,
    Moderate,
    Complex,
    VeryComplex,
}

/// Data distribution types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataDistribution {
    Uniform,
    Clustered,
    Sparse,
    Dense,
    Hierarchical,
    Unknown,
}

/// Dimensionality hints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DimensionalityHint {
    Low,      // < 100 dimensions
    Medium,   // 100-1000 dimensions
    High,     // 1000-10000 dimensions
    VeryHigh, // > 10000 dimensions
}

/// Result diversification parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiversificationParams {
    /// Enable diversification
    pub enabled: bool,
    
    /// Diversification strategy
    
    /// Diversity threshold
    pub diversity_threshold: f32,
    
    /// Maximum diversity loss
    pub max_diversity_loss: f32,
}

/// Diversification strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiversificationStrategy {
    /// Maximum marginal relevance
    MaximalMarginalRelevance {
        lambda: f32, // Balance between relevance and diversity
    },
    
    /// Clustering-based diversification
    ClusteringBased {
        num_clusters: usize,
        cluster_method: ClusteringMethod,
    },
    
    /// Distance-based diversification
    DistanceBased {
        min_similarity: f32,
        distance_metric: DistanceMetric,
    },
    
    /// Feature-based diversification
    FeatureBased {
        feature_weights: Vec<f32>,
        diversity_features: Vec<String>,
    },
}

/// Clustering methods for diversification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusteringMethod {
    KMeans,
    Hierarchical,
    DBSCAN,
    SpectralClustering,
}

/// Parallelization parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelizationParams {
    /// Enable parallel search
    pub enabled: bool,
    
    /// Number of parallel workers
    pub num_workers: Option<usize>,
    
    /// Work distribution strategy
    pub distribution_strategy: WorkDistributionStrategy,
    
    /// Result aggregation strategy
    pub aggregation_strategy: ResultAggregationStrategy,
}

/// Work distribution strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkDistributionStrategy {
    /// Static partitioning
    Static {
        partition_size: usize,
    },
    
    /// Dynamic work stealing
    DynamicWorkStealing {
        steal_threshold: usize,
    },
    
    /// Query decomposition
    QueryDecomposition {
        decomposition_strategy: String,
    },
    
    /// Data partitioning
    DataPartitioning {
        partition_strategy: String,
    },
}

/// Result aggregation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResultAggregationStrategy {
    /// Merge and sort
    MergeSort,
    
    /// Priority queue based
    PriorityQueue,
    
    /// Streaming aggregation
    Streaming,
    
    /// Two-phase aggregation
    TwoPhase,
}

/// Hybrid fusion parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridFusionParams {
    /// Fusion strategy
    
    /// Component weights
    pub weights: FusionWeights,
    
    /// Normalization method
    pub normalization: ScoreNormalization,
    
    /// Fusion post-processing
    pub post_processing: Option<PostProcessingParams>,
}

/// Fusion strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FusionStrategy {
    /// Linear combination
    LinearCombination,
    
    /// Reciprocal rank fusion
    ReciprocalRankFusion {
        k: f32, // RRF parameter
    },
    
    /// Borda count
    BordaCount,
    
    /// Weighted combination
    WeightedCombination {
        combination_function: CombinationFunction,
    },
    
    /// Machine learning based fusion
    MLFusion {
        model_type: String,
        model_parameters: HashMap<String, serde_json::Value>,
    },
}

/// Combination functions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CombinationFunction {
    Sum,
    Product,
    Maximum,
    Minimum,
    Average,
    WeightedAverage,
    Custom(String),
}

/// Fusion weights
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionWeights {
    /// Vector search weight
    pub vector_weight: f32,
    
    /// Text search weight
    pub text_weight: f32,
    
    /// Metadata filter weight
    pub metadata_weight: f32,
    
    /// Dynamic weight adjustment
    pub dynamic_adjustment: bool,
}

/// Score normalization methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScoreNormalization {
    /// No normalization
    None,
    
    /// Min-max normalization
    MinMax,
    
    /// Z-score normalization
    ZScore,
    
    /// Rank normalization
    Rank,
    
    /// Sigmoid normalization
    Sigmoid {
        alpha: f32,
        beta: f32,
    },
}

/// Post-processing parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessingParams {
    /// Re-ranking enabled
    pub reranking_enabled: bool,
    
    /// Re-ranking strategy
    pub reranking_strategy: Option<RerankingStrategy>,
    
    /// Result filtering
    pub result_filtering: Option<ResultFilteringParams>,
    
    /// Score adjustment
    pub score_adjustment: Option<ScoreAdjustmentParams>,
}

/// Re-ranking strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RerankingStrategy {
    /// Machine learning based re-ranking
    MLReranking {
        model_type: String,
        features: Vec<String>,
    },
    
    /// Rule-based re-ranking
    RuleBased {
        rules: Vec<RerankingRule>,
    },
    
    /// Distance-based re-ranking
    DistanceBased {
        distance_metric: DistanceMetric,
        reference_vector: Vec<f32>,
    },
    
    /// Multi-criteria re-ranking
    MultiCriteria {
        criteria: Vec<RerankingCriterion>,
        aggregation: CriteriaAggregation,
    },
}

/// Re-ranking rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankingRule {
    /// Rule condition
    pub condition: String,
    
    /// Score adjustment
    pub score_adjustment: f32,
    
    /// Rule priority
    pub priority: u32,
}

/// Re-ranking criterion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankingCriterion {
    /// Criterion name
    pub name: String,
    
    /// Criterion weight
    pub weight: f32,
    
    /// Criterion function
    pub function: String,
}

/// Criteria aggregation methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CriteriaAggregation {
    WeightedSum,
    WeightedProduct,
    Voting,
    Ranking,
}

/// Result filtering parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultFilteringParams {
    /// Score threshold
    pub score_threshold: Option<f32>,
    
    /// Distance threshold
    pub distance_threshold: Option<f32>,
    
    /// Duplicate removal
    pub remove_duplicates: bool,
    
    /// Custom filters
    pub custom_filters: Vec<CustomFilter>,
}

/// Custom filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFilter {
    /// Filter name
    pub name: String,
    
    /// Filter parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Filter priority
    pub priority: u32,
}

/// Score adjustment parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreAdjustmentParams {
    /// Boost factors
    pub boost_factors: HashMap<String, f32>,
    
    /// Penalty factors
    pub penalty_factors: HashMap<String, f32>,
    
    /// Adjustment strategy
    pub strategy: ScoreAdjustmentStrategy,
}

/// Score adjustment strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScoreAdjustmentStrategy {
    Additive,
    Multiplicative,
    Exponential,
    Logarithmic,
    Custom(String),
}

/// Multi-vector combination strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MultiVectorStrategy {
    /// Average all query vectors
    Average,
    
    /// Use maximum similarity
    Maximum,
    
    /// Use minimum similarity
    Minimum,
    
    /// Weighted combination
    Weighted {
        weights: Vec<f32>,
    },
    
    /// Sequential refinement
    Sequential {
        refinement_strategy: String,
    },
    
    /// Ensemble combination
    Ensemble {
        combination_method: String,
    },
}

/// Approximation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproximationParams {
    /// Approximation quality
    pub quality: ApproximationQuality,
    
    /// Search algorithm preferences
    pub algorithm_preferences: Vec<ApproximationAlgorithm>,
    
    /// Approximation constraints
    pub constraints: ApproximationConstraints,
    
    /// Refinement settings
    pub refinement: Option<RefinementParams>,
}

/// Approximation quality levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApproximationQuality {
    Fast,      // Low accuracy, high speed
    Balanced,  // Balanced accuracy and speed
    Accurate,  // High accuracy, lower speed
    Custom {
        accuracy_threshold: f32,
        speed_requirement: f32,
    },
}

/// Approximation algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApproximationAlgorithm {
    LSH,       // Locality Sensitive Hashing
    RandomProjection,
    ProductQuantization,
    IVF,       // Inverted File
    HNSW,      // Hierarchical Navigable Small World
    NSG,       // Navigating Spreading-out Graph
    FAISS,     // Facebook AI Similarity Search
    Annoy,     // Approximate Nearest Neighbors Oh Yeah
    Custom(String),
}

/// Approximation constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproximationConstraints {
    /// Maximum search time (ms)
    pub max_search_time_ms: Option<u64>,
    
    /// Maximum memory usage (MB)
    pub max_memory_mb: Option<u64>,
    
    /// Minimum recall requirement
    pub min_recall: Option<f32>,
    
    /// Maximum distance error
    pub max_distance_error: Option<f32>,
}

/// Refinement parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinementParams {
    /// Enable refinement
    pub enabled: bool,
    
    /// Refinement strategy
    
    /// Refinement budget
    pub refinement_budget: RefinementBudget,
}

/// Refinement strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefinementStrategy {
    /// Re-ranking with exact distances
    ExactReranking {
        rerank_factor: f32,
    },
    
    /// Iterative refinement
    Iterative {
        max_iterations: u32,
        convergence_threshold: f32,
    },
    
    /// Multi-stage refinement
    MultiStage {
        stages: Vec<RefinementStage>,
    },
}

/// Refinement stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinementStage {
    /// Stage name
    pub name: String,
    
    /// Algorithm for this stage
    pub algorithm: String,
    
    /// Stage parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Candidate reduction factor
    pub reduction_factor: f32,
}

/// Refinement budget
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RefinementBudget {
    /// Time-based budget
    Time {
        max_time_ms: u64,
    },
    
    /// Computation-based budget
    Computation {
        max_distance_computations: u64,
    },
    
    /// Memory-based budget
    Memory {
        max_memory_mb: u64,
    },
    
    /// Adaptive budget
    Adaptive {
        initial_budget: u64,
        adjustment_factor: f32,
    },
}

/// Search query for batch operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Query identifier
    pub query_id: String,
    
    /// Search mode for this query
    pub search_mode: UniversalSearchMode,
    
    /// Query-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Query priority
    pub priority: QueryPriority,
}

/// Query priority levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Batch search parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchParams {
    /// Batch processing strategy
    pub processing_strategy: BatchProcessingStrategy,
    
    /// Result collection strategy
    pub result_collection: ResultCollectionStrategy,
    
    /// Error handling strategy
    pub error_handling: BatchErrorHandling,
    
    /// Resource management
    pub resource_management: BatchResourceManagement,
}

/// Batch processing strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BatchProcessingStrategy {
    /// Sequential processing
    Sequential,
    
    /// Parallel processing
    Parallel {
        max_parallelism: usize,
    },
    
    /// Priority-based processing
    PriorityBased {
        priority_queue_size: usize,
    },
    
    /// Adaptive processing
    Adaptive {
        adaptation_strategy: String,
    },
}

/// Result collection strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResultCollectionStrategy {
    /// Collect all results at once
    Bulk,
    
    /// Stream results as they complete
    Streaming,
    
    /// Collect by priority
    Priority,
    
    /// Collect by completion order
    CompletionOrder,
}

/// Batch error handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BatchErrorHandling {
    /// Fail fast on first error
    FailFast,
    
    /// Continue on errors
    ContinueOnError,
    
    /// Retry on errors
    RetryOnError {
        max_retries: u32,
        retry_delay_ms: u64,
    },
    
    /// Partial failure tolerance
    PartialFailure {
        max_failure_rate: f32,
    },
}

/// Batch resource management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResourceManagement {
    /// Memory management
    pub memory_management: BatchMemoryManagement,
    
    /// CPU management
    pub cpu_management: BatchCPUManagement,
    
    /// I/O management
    pub io_management: BatchIOManagement,
}

/// Batch memory management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchMemoryManagement {
    /// Maximum memory per batch
    pub max_memory_per_batch: Option<u64>,
    
    /// Memory cleanup strategy
    pub cleanup_strategy: MemoryCleanupStrategy,
    
    /// Memory monitoring
    pub monitoring_enabled: bool,
}

/// Memory cleanup strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryCleanupStrategy {
    Immediate,
    Deferred,
    Periodic { interval_ms: u64 },
    PressureBased { threshold: f32 },
}

/// Batch CPU management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCPUManagement {
    /// CPU quota per batch
    pub cpu_quota_percent: Option<f32>,
    
    /// Thread pool management
    pub thread_pool_management: ThreadPoolManagement,
    
    /// CPU monitoring
    pub monitoring_enabled: bool,
}

/// Thread pool management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThreadPoolManagement {
    Static { pool_size: usize },
    Dynamic { min_size: usize, max_size: usize },
    Adaptive { adaptation_strategy: String },
}

/// Batch I/O management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchIOManagement {
    /// I/O bandwidth limit
    pub bandwidth_limit_mbps: Option<f64>,
    
    /// I/O scheduling
    pub scheduling_strategy: IOSchedulingStrategy,
    
    /// I/O monitoring
    pub monitoring_enabled: bool,
}

/// I/O scheduling strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IOSchedulingStrategy {
    FIFO,
    Priority,
    Deadline,
    FairShare,
}

/// Progressive search parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveSearchParams {
    /// Progressive stages
    pub stages: Vec<ProgressiveStage>,
    
    /// Stage transition criteria
    pub transition_criteria: StageTransitionCriteria,
    
    /// Early termination conditions
    pub early_termination: EarlyTerminationConditions,
    
    /// Result refinement
    pub result_refinement: ProgressiveRefinement,
}

/// Progressive search stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveStage {
    /// Stage name
    pub name: String,
    
    /// Stage search algorithm
    pub algorithm: ProgressiveAlgorithm,
    
    /// Stage parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Candidate count for this stage
    pub candidate_count: usize,
    
    /// Quality threshold for this stage
    pub quality_threshold: f32,
}

/// Progressive algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressiveAlgorithm {
    BinaryFilter,
    QuantizedSearch,
    ApproximateNN,
    ExactSearch,
    Custom(String),
}

/// Stage transition criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageTransitionCriteria {
    /// Quality-based transition
    pub quality_based: Option<QualityTransition>,
    
    /// Time-based transition
    pub time_based: Option<TimeTransition>,
    
    /// Result-based transition
    pub result_based: Option<ResultTransition>,
}

/// Quality transition criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityTransition {
    /// Quality improvement threshold
    pub improvement_threshold: f32,
    
    /// Quality plateau detection
    pub plateau_detection: bool,
    
    /// Quality target
    pub quality_target: Option<f32>,
}

/// Time transition criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeTransition {
    /// Maximum time per stage
    pub max_time_per_stage_ms: u64,
    
    /// Total time budget
    pub total_time_budget_ms: u64,
    
    /// Time allocation strategy
    pub allocation_strategy: TimeAllocationStrategy,
}

/// Time allocation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimeAllocationStrategy {
    Equal,
    Weighted { weights: Vec<f32> },
    Adaptive,
    PriorityBased,
}

/// Result transition criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultTransition {
    /// Minimum results per stage
    pub min_results_per_stage: usize,
    
    /// Result quality threshold
    pub result_quality_threshold: f32,
    
    /// Convergence detection
    pub convergence_detection: bool,
}

/// Early termination conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarlyTerminationConditions {
    /// Quality satisfaction
    pub quality_satisfied: Option<f32>,
    
    /// Time limit exceeded
    pub time_limit_ms: Option<u64>,
    
    /// Result count sufficient
    pub sufficient_results: Option<usize>,
    
    /// Custom termination conditions
    pub custom_conditions: Vec<CustomTerminationCondition>,
}

/// Custom termination condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTerminationCondition {
    /// Condition name
    pub name: String,
    
    /// Condition parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Condition priority
    pub priority: u32,
}

/// Progressive result refinement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveRefinement {
    /// Enable cross-stage refinement
    pub cross_stage_refinement: bool,
    
    /// Refinement strategy
    
    /// Refinement budget
    pub budget: RefinementBudget,
}

/// Progressive refinement strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressiveRefinementStrategy {
    CumulativeRefinement,
    StagewiseRefinement,
    AdaptiveRefinement,
    HybridRefinement,
}

/// Search capabilities description
#[derive(Debug, Clone)]
pub struct SearchCapabilities {
    /// Supported search modes
    pub supported_modes: Vec<String>,
    
    /// Distance metrics supported
    pub supported_distance_metrics: Vec<DistanceMetric>,
    
    /// Maximum vector dimension
    pub max_dimension: usize,
    
    /// Maximum result count
    pub max_results: usize,
    
    /// Approximate search support
    pub approximate_search: bool,
    
    /// Parallel search support
    pub parallel_search: bool,
    
    /// Batch search support
    pub batch_search: bool,
    
    /// Progressive search support
    pub progressive_search: bool,
    
    /// Hybrid search support
    pub hybrid_search: bool,
}

/// Search optimizations available
#[derive(Debug, Clone)]
pub struct SearchOptimizations {
    /// Hardware optimizations
    pub hardware_optimizations: Vec<HardwareOptimization>,
    
    /// Algorithm optimizations
    pub algorithm_optimizations: Vec<AlgorithmOptimization>,
    
    /// Index optimizations
    pub index_optimizations: Vec<IndexOptimization>,
    
    /// Cache optimizations
    pub cache_optimizations: Vec<CacheOptimization>,
}

/// Hardware optimizations
#[derive(Debug, Clone)]
pub enum HardwareOptimization {
    SIMD,
    GPU,
    MultiCore,
    VectorInstructions,
    MemoryPrefetch,
}

/// Algorithm optimizations
#[derive(Debug, Clone)]
pub enum AlgorithmOptimization {
    EarlyTermination,
    PruningStrategies,
    ApproximateAlgorithms,
    HierarchicalSearch,
    QuantizedSearch,
}

/// Index optimizations
#[derive(Debug, Clone)]
pub enum IndexOptimization {
    BloomFilters,
    InvertedIndexes,
    LSH,
    TreeIndexes,
    GraphIndexes,
}

/// Cache optimizations
#[derive(Debug, Clone)]
pub enum CacheOptimization {
    ResultCaching,
    IndexCaching,
    DataCaching,
    ComputationCaching,
    PrefetchCaching,
}

/// Search candidate result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRecord {
    /// Record identifier
    pub id: String,
    
    /// Distance/similarity score
    pub similarity: f32,
    
    /// Optional vector data
    pub vector: Option<Vec<f32>>,
    
    /// Optional metadata
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    
    /// Search context information
    pub search_context: Option<SearchContext>,
}

/// Search context information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchContext {
    /// Search stage that found this candidate
    pub search_stage: Option<String>,
    
    /// Approximation quality
    pub approximation_quality: Option<f32>,
    
    /// Computation cost
    pub computation_cost: Option<f32>,
    
    /// Additional context
    pub additional_context: HashMap<String, serde_json::Value>,
}

/// Search candidate for progressive refinement
#[derive(Debug, Clone)]
pub struct SearchCandidate {
    /// Candidate record
    pub record: CandidateRecord,
    
    /// Refinement history
    pub refinement_history: Vec<RefinementStep>,
    
    /// Candidate state
    pub state: CandidateState,
}

/// Refinement step
#[derive(Debug, Clone)]
pub struct RefinementStep {
    /// Step name
    pub step_name: String,
    
    /// Previous score
    pub previous_score: f32,
    
    /// New score
    pub new_score: f32,
    
    /// Step timestamp
    pub timestamp: std::time::Instant,
    
    /// Step metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Candidate state
#[derive(Debug, Clone)]
pub enum CandidateState {
    Initial,
    Refined,
    Final,
    Discarded,
}

/// Progressive search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveSearchResult {
    /// Final candidates
    pub candidates: Vec<CandidateRecord>,
    
    /// Search statistics
    pub statistics: ProgressiveSearchStatistics,
    
    /// Stage results
    pub stage_results: Vec<StageResult>,
    
    /// Overall quality metrics
    pub quality_metrics: QualityMetrics,
}

/// Progressive search statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveSearchStatistics {
    /// Total search time
    pub total_time_ms: u64,
    
    /// Time per stage
    pub stage_times_ms: Vec<u64>,
    
    /// Total candidates examined
    pub total_candidates_examined: u64,
    
    /// Candidates per stage
    pub candidates_per_stage: Vec<u64>,
    
    /// Distance computations
    pub distance_computations: u64,
    
    /// Early termination occurred
    pub early_termination: bool,
}

/// Stage result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// Stage name
    pub stage_name: String,
    
    /// Stage candidates
    pub candidates: Vec<CandidateRecord>,
    
    /// Stage time
    pub time_ms: u64,
    
    /// Stage quality
    pub quality: f32,
    
    /// Stage metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetrics {
    /// Overall quality score
    pub overall_quality: f32,
    
    /// Precision estimate
    pub precision: Option<f32>,
    
    /// Recall estimate
    pub recall: Option<f32>,
    
    /// Distance accuracy
    pub distance_accuracy: Option<f32>,
    
    /// Result diversity
    pub result_diversity: Option<f32>,
}

impl Default for SimilaritySearchParams {
    fn default() -> Self {
        Self {
            quality_speed_balance: 0.7, // Slightly favor quality
            enable_early_termination: true,
            timeout_ms: None,
            search_hints: Vec::new(),
            diversification: None,
            parallelization: ParallelizationParams {
                enabled: true,
                num_workers: None, // Auto-detect
                distribution_strategy: WorkDistributionStrategy::DynamicWorkStealing {
                    steal_threshold: 100,
                },
                aggregation_strategy: ResultAggregationStrategy::MergeSort,
            },
        }
    }
}

impl Default for ApproximationParams {
    fn default() -> Self {
        Self {
            quality: ApproximationQuality::Balanced,
            algorithm_preferences: vec![
                ApproximationAlgorithm::HNSW,
                ApproximationAlgorithm::IVF,
                ApproximationAlgorithm::LSH,
            ],
            constraints: ApproximationConstraints {
                max_search_time_ms: Some(100),
                max_memory_mb: Some(1024),
                min_recall: Some(0.9),
                max_distance_error: Some(0.1),
            },
            refinement: Some(RefinementParams {
                enabled: true,
                strategy: RefinementStrategy::ExactReranking {
                    rerank_factor: 2.0,
                },
                refinement_budget: RefinementBudget::Time {
                    max_time_ms: 50,
                },
            }),
        }
    }
}

impl Default for BatchSearchParams {
    fn default() -> Self {
        Self {
            processing_strategy: BatchProcessingStrategy::Parallel {
                max_parallelism: 4,
            },
            result_collection: ResultCollectionStrategy::CompletionOrder,
            error_handling: BatchErrorHandling::ContinueOnError,
            resource_management: BatchResourceManagement {
                memory_management: BatchMemoryManagement {
                    max_memory_per_batch: Some(1024 * 1024 * 1024), // 1GB
                    cleanup_strategy: MemoryCleanupStrategy::Immediate,
                    monitoring_enabled: true,
                },
                cpu_management: BatchCPUManagement {
                    cpu_quota_percent: Some(80.0),
                    thread_pool_management: ThreadPoolManagement::Dynamic {
                        min_size: 2,
                        max_size: 16,
                    },
                    monitoring_enabled: true,
                },
                io_management: BatchIOManagement {
                    bandwidth_limit_mbps: None,
                    scheduling_strategy: IOSchedulingStrategy::FairShare,
                    monitoring_enabled: true,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_universal_search_mode_creation() {
        let search_mode = UniversalSearchMode::VectorSimilarity {
            query_vector: vec![1.0, 2.0, 3.0],
            top_k: 10,
            distance_metric: DistanceMetric::Cosine,
            search_params: SimilaritySearchParams::default(),
        };
        
        assert!(matches!(search_mode, UniversalSearchMode::VectorSimilarity { .. }));
    }
    
    #[test]
    fn test_search_parameters() {
        let params = SimilaritySearchParams::default();
        
        assert_eq!(params.quality_speed_balance, 0.7);
        assert!(params.enable_early_termination);
        assert!(params.parallelization.enabled);
    }
    
    #[test]
    fn test_approximation_parameters() {
        let approx_params = ApproximationParams::default();
        
        assert!(matches!(approx_params.quality, ApproximationQuality::Balanced));
        assert_eq!(approx_params.algorithm_preferences.len(), 3);
        assert!(approx_params.refinement.is_some());
    }
    
    #[test]
    fn test_batch_search_parameters() {
        let batch_params = BatchSearchParams::default();
        
        assert!(matches!(
            batch_params.processing_strategy,
            BatchProcessingStrategy::Parallel { .. }
        ));
        assert!(matches!(
            batch_params.error_handling,
            BatchErrorHandling::ContinueOnError
        ));
        assert!(batch_params.resource_management.memory_management.monitoring_enabled);
    }
    
    #[test]
    fn test_search_capabilities() {
        let capabilities = SearchCapabilities {
            supported_modes: vec![
                "VectorSimilarity".to_string(),
                "IdLookup".to_string(),
                "FilteredSimilarity".to_string(),
            ],
            supported_distance_metrics: vec![
                DistanceMetric::Cosine,
                DistanceMetric::Euclidean,
                DistanceMetric::DotProduct,
            ],
            max_dimension: 4096,
            max_results: 10000,
            approximate_search: true,
            parallel_search: true,
            batch_search: true,
            progressive_search: true,
            hybrid_search: true,
        };
        
        assert_eq!(capabilities.supported_modes.len(), 3);
        assert_eq!(capabilities.supported_distance_metrics.len(), 3);
        assert!(capabilities.approximate_search);
        assert!(capabilities.progressive_search);
    }
}