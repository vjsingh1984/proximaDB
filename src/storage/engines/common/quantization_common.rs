// Universal Quantization Infrastructure
// Shared quantization capabilities across all storage engines

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::compute::distance_computation::DistanceMetric;

/// Universal quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalQuantizationConfig {
    /// Enable quantization
    pub enabled: bool,
    
    /// Progressive quantization stages
    pub stages: Vec<ProgressiveQuantizationStage>,
    
    /// Hardware-specific optimizations
    pub hardware_optimizations: HardwareQuantizationConfig,
    
    /// Memory management
    pub memory_config: QuantizationMemoryConfig,
    
    /// Quality vs performance trade-offs
    pub quality_config: QuantizationQualityConfig,
    
    /// Engine-specific overrides
    pub engine_overrides: HashMap<String, serde_json::Value>,
}

/// Progressive quantization stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressiveQuantizationStage {
    /// Stage name
    pub name: String,
    
    /// Quantization level
    pub level: UniversalQuantizationLevel,
    
    /// When to use this stage
    pub usage_threshold: QuantizationThreshold,
    
    /// Stage-specific configuration
    pub config: QuantizationStageConfig,
    
    /// Expected compression ratio
    pub expected_compression_ratio: f32,
    
    /// Expected quality retention
    pub expected_quality_retention: f32,
}

/// Universal quantization levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UniversalQuantizationLevel {
    /// No quantization (full precision)
    None,
    
    /// Binary quantization (1 bit per dimension)
    Binary {
        threshold_strategy: BinaryThresholdStrategy,
    },
    
    /// 8-bit integer quantization
    Int8 {
        scale_strategy: ScaleStrategy,
        zero_point_strategy: ZeroPointStrategy,
    },
    
    /// 4-bit integer quantization
    Int4 {
        scale_strategy: ScaleStrategy,
        zero_point_strategy: ZeroPointStrategy,
    },
    
    /// Product Quantization
    ProductQuantization {
        segments: u8,
        bits_per_segment: u8,
        codebook_strategy: CodebookStrategy,
    },
    
    /// Scalar Quantization
    ScalarQuantization {
        bits: u8,
        range_strategy: RangeStrategy,
    },
    
    /// Hierarchical quantization
    Hierarchical {
        levels: Vec<UniversalQuantizationLevel>,
        selection_strategy: HierarchicalStrategy,
    },
    
    /// Adaptive quantization based on data
    Adaptive {
        min_level: Box<UniversalQuantizationLevel>,
        max_level: Box<UniversalQuantizationLevel>,
        adaptation_criteria: AdaptationCriteria,
    },
    
    /// Custom quantization method
    Custom {
        method_name: String,
        parameters: HashMap<String, serde_json::Value>,
    },
}

/// Binary quantization threshold strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BinaryThresholdStrategy {
    /// Use zero as threshold
    Zero,
    
    /// Use median as threshold
    Median,
    
    /// Use mean as threshold
    Mean,
    
    /// Use percentile as threshold
    Percentile(f32),
    
    /// Optimize threshold for distance preservation
    DistanceOptimized,
}

/// Scale calculation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScaleStrategy {
    /// Min-max scaling
    MinMax,
    
    /// Standard deviation scaling
    StandardDeviation,
    
    /// Percentile-based scaling
    Percentile { lower: f32, upper: f32 },
    
    /// Dynamic range scaling
    DynamicRange,
    
    /// Distance-preserving scaling
    DistancePreserving,
}

/// Zero point calculation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ZeroPointStrategy {
    /// Use minimum value
    Minimum,
    
    /// Use mean value
    Mean,
    
    /// Use median value
    Median,
    
    /// Optimize for uniformity
    Uniform,
    
    /// Optimize for distance preservation
    DistanceOptimized,
}

/// Codebook generation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CodebookStrategy {
    /// K-means clustering
    KMeans {
        max_iterations: u32,
        tolerance: f32,
    },
    
    /// Random initialization
    Random {
        seed: Option<u64>,
    },
    
    /// Uniform distribution
    Uniform,
    
    /// Principal component analysis based
    PCA {
        components: u8,
    },
    
    /// Learned codebook from training data
    Learned {
        training_vectors: usize,
    },
}

/// Range calculation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RangeStrategy {
    /// Global min-max
    Global,
    
    /// Per-dimension min-max
    PerDimension,
    
    /// Percentile-based range
    Percentile { lower: f32, upper: f32 },
    
    /// Robust range (outlier-resistant)
    Robust,
}

/// Hierarchical quantization strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HierarchicalStrategy {
    /// Quality-based selection
    Quality,
    
    /// Performance-based selection
    Performance,
    
    /// Memory-based selection
    Memory,
    
    /// Adaptive selection
    Adaptive,
}

/// Adaptation criteria for adaptive quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptationCriteria {
    /// Data distribution metrics
    pub distribution_metrics: Vec<DistributionMetric>,
    
    /// Performance thresholds
    pub performance_thresholds: PerformanceThresholds,
    
    /// Quality thresholds
    pub quality_thresholds: QualityThresholds,
    
    /// Adaptation frequency
    pub adaptation_frequency: AdaptationFrequency,
}

/// Distribution metrics for adaptation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DistributionMetric {
    /// Standard deviation
    StandardDeviation,
    
    /// Skewness
    Skewness,
    
    /// Kurtosis
    Kurtosis,
    
    /// Entropy
    Entropy,
    
    /// Range
    Range,
    
    /// Outlier ratio
    OutlierRatio,
}

/// Performance thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceThresholds {
    /// Maximum acceptable latency (ms)
    pub max_latency_ms: f64,
    
    /// Minimum acceptable throughput (ops/sec)
    pub min_throughput: f64,
    
    /// Maximum memory usage (bytes)
    pub max_memory_bytes: u64,
    
    /// Maximum CPU usage (%)
    pub max_cpu_percent: f32,
}

/// Quality thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityThresholds {
    /// Minimum acceptable recall
    pub min_recall: f32,
    
    /// Maximum acceptable error rate
    pub max_error_rate: f32,
    
    /// Minimum distance preservation
    pub min_distance_preservation: f32,
    
    /// Maximum quantization noise
    pub max_quantization_noise: f32,
}

/// Adaptation frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdaptationFrequency {
    /// Continuous adaptation
    Continuous,
    
    /// Periodic adaptation
    Periodic { interval_ms: u64 },
    
    /// Threshold-based adaptation
    ThresholdBased { metric_threshold: f64 },
    
    /// Manual adaptation only
    Manual,
}

/// Quantization threshold conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationThreshold {
    /// Dataset size thresholds
    pub dataset_size: Option<DatasetSizeThreshold>,
    
    /// Memory pressure thresholds
    pub memory_pressure: Option<MemoryPressureThreshold>,
    
    /// Performance requirements
    pub performance_requirements: Option<PerformanceRequirement>,
    
    /// Quality requirements
    pub quality_requirements: Option<QualityRequirement>,
}

/// Dataset size thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetSizeThreshold {
    /// Minimum number of vectors
    pub min_vectors: u64,
    
    /// Maximum number of vectors
    pub max_vectors: Option<u64>,
    
    /// Minimum dimension
    pub min_dimension: usize,
    
    /// Maximum dimension
    pub max_dimension: Option<usize>,
}

/// Memory pressure thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPressureThreshold {
    /// Available memory threshold (%)
    pub available_memory_percent: f32,
    
    /// Memory usage growth rate threshold
    pub growth_rate_threshold: f32,
    
    /// Cache miss rate threshold
    pub cache_miss_rate_threshold: f32,
}

/// Performance requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceRequirement {
    /// Target latency (ms)
    pub target_latency_ms: f64,
    
    /// Target throughput (ops/sec)
    pub target_throughput: f64,
    
    /// Maximum CPU usage (%)
    pub max_cpu_percent: f32,
}

/// Quality requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRequirement {
    /// Target recall
    pub target_recall: f32,
    
    /// Maximum error rate
    pub max_error_rate: f32,
    
    /// Distance preservation requirement
    pub distance_preservation: f32,
}

/// Quantization stage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStageConfig {
    /// Stage-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    
    /// Hardware optimizations for this stage
    pub hardware_optimizations: StageHardwareConfig,
    
    /// Memory management for this stage
    pub memory_management: StageMemoryConfig,
    
    /// Validation configuration
    pub validation: StageValidationConfig,
}

/// Hardware quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareQuantizationConfig {
    /// SIMD optimizations
    pub simd_config: SIMDQuantizationConfig,
    
    /// GPU optimizations
    pub gpu_config: GPUQuantizationConfig,
    
    /// CPU-specific optimizations
    pub cpu_config: CPUQuantizationConfig,
    
    /// Memory optimizations
    pub memory_config: HardwareMemoryConfig,
}

/// SIMD quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SIMDQuantizationConfig {
    /// Enable SIMD operations
    pub enabled: bool,
    
    /// SIMD instruction set preference
    pub instruction_set_preference: Vec<SIMDInstructionSet>,
    
    /// Vector width optimization
    pub vector_width_optimization: bool,
    
    /// Alignment requirements
    pub alignment_bytes: usize,
}

/// SIMD instruction sets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SIMDInstructionSet {
    AVX512,
    AVX2,
    AVX,
    SSE42,
    SSE2,
    NEON,
    Auto,
}

/// GPU quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GPUQuantizationConfig {
    /// Enable GPU acceleration
    pub enabled: bool,
    
    /// GPU memory allocation strategy
    pub memory_strategy: GPUMemoryStrategy,
    
    /// Batch size for GPU operations
    pub batch_size: usize,
    
    /// GPU kernel selection
    pub kernel_preference: Vec<GPUKernel>,
}

/// GPU memory strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GPUMemoryStrategy {
    /// Keep data on GPU
    Persistent,
    
    /// Transfer as needed
    OnDemand,
    
    /// Hybrid approach
    Hybrid,
    
    /// Automatic management
    Automatic,
}

/// GPU kernels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GPUKernel {
    CUDA,
    OpenCL,
    Vulkan,
    Metal,
    Auto,
}

/// CPU quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CPUQuantizationConfig {
    /// Threading configuration
    pub threading: ThreadingConfig,
    
    /// Cache optimization
    pub cache_optimization: CacheOptimizationConfig,
    
    /// Instruction optimization
    pub instruction_optimization: InstructionOptimizationConfig,
}

/// Threading configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadingConfig {
    /// Number of threads
    pub thread_count: Option<usize>,
    
    /// Thread affinity
    pub thread_affinity: bool,
    
    /// Work stealing enabled
    pub work_stealing: bool,
}

/// Cache optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheOptimizationConfig {
    /// Cache-friendly data layout
    pub cache_friendly_layout: bool,
    
    /// Prefetch strategy
    pub prefetch_strategy: PrefetchStrategy,
    
    /// Cache line size optimization
    pub cache_line_optimization: bool,
}

/// Prefetch strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrefetchStrategy {
    None,
    Software,
    Hardware,
    Adaptive,
}

/// Instruction optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionOptimizationConfig {
    /// Loop unrolling
    pub loop_unrolling: bool,
    
    /// Vectorization hints
    pub vectorization_hints: bool,
    
    /// Branch prediction optimization
    pub branch_prediction_optimization: bool,
}

/// Hardware memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareMemoryConfig {
    /// Memory alignment
    pub alignment_bytes: usize,
    
    /// NUMA awareness
    pub numa_aware: bool,
    
    /// Memory prefetch
    pub prefetch_enabled: bool,
    
    /// Memory pool usage
    pub use_memory_pool: bool,
}

/// Stage hardware configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageHardwareConfig {
    /// Preferred hardware
    pub preferred_hardware: Vec<HardwarePreference>,
    
    /// Fallback options
    pub fallback_options: Vec<HardwarePreference>,
    
    /// Hardware-specific parameters
    pub hardware_parameters: HashMap<String, serde_json::Value>,
}

/// Hardware preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HardwarePreference {
    CPU,
    GPU,
    SIMD,
    Auto,
}

/// Stage memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageMemoryConfig {
    /// Memory allocation strategy
    pub allocation_strategy: MemoryAllocationStrategy,
    
    /// Buffer reuse
    pub buffer_reuse: bool,
    
    /// Memory limits
    pub memory_limits: MemoryLimits,
}

/// Memory allocation strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryAllocationStrategy {
    Preallocated,
    OnDemand,
    Pooled,
    Adaptive,
}

/// Memory limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLimits {
    /// Maximum memory per stage (bytes)
    pub max_memory_per_stage: u64,
    
    /// Maximum total memory (bytes)
    pub max_total_memory: u64,
    
    /// Memory growth rate limit
    pub max_growth_rate: f32,
}

/// Stage validation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageValidationConfig {
    /// Enable validation
    pub enabled: bool,
    
    /// Validation frequency
    pub frequency: ValidationFrequency,
    
    /// Validation metrics
    pub metrics: Vec<ValidationMetric>,
    
    /// Error handling
    pub error_handling: ValidationErrorHandling,
}

/// Validation frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidationFrequency {
    Always,
    Periodic { interval_ms: u64 },
    Sampling { rate: f32 },
    Never,
}

/// Validation metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidationMetric {
    Accuracy,
    Recall,
    Precision,
    DistancePreservation,
    CompressionRatio,
    PerformanceRegression,
}

/// Validation error handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidationErrorHandling {
    Fail,
    Warn,
    Ignore,
    Fallback,
}

/// Quantization memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationMemoryConfig {
    /// Memory pool configuration
    pub memory_pool: MemoryPoolConfig,
    
    /// Buffer management
    pub buffer_management: BufferManagementConfig,
    
    /// Garbage collection
    pub garbage_collection: GarbageCollectionConfig,
}

/// Memory pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPoolConfig {
    /// Enable memory pooling
    pub enabled: bool,
    
    /// Initial pool size (bytes)
    pub initial_size: u64,
    
    /// Maximum pool size (bytes)
    pub max_size: u64,
    
    /// Growth strategy
    pub growth_strategy: PoolGrowthStrategy,
}

/// Pool growth strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PoolGrowthStrategy {
    Fixed,
    Linear { increment: u64 },
    Exponential { factor: f32 },
    Adaptive,
}

/// Buffer management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferManagementConfig {
    /// Buffer reuse enabled
    pub reuse_enabled: bool,
    
    /// Maximum buffer age
    pub max_buffer_age_ms: u64,
    
    /// Buffer size limits
    pub size_limits: BufferSizeLimits,
}

/// Buffer size limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferSizeLimits {
    /// Minimum buffer size (bytes)
    pub min_size: u64,
    
    /// Maximum buffer size (bytes)
    pub max_size: u64,
    
    /// Preferred buffer size (bytes)
    pub preferred_size: u64,
}

/// Garbage collection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GarbageCollectionConfig {
    /// Enable garbage collection
    pub enabled: bool,
    
    /// Collection frequency
    pub frequency: GarbageCollectionFrequency,
}

/// Garbage collection frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GarbageCollectionFrequency {
    Immediate,
    Periodic { interval_ms: u64 },
    MemoryPressure { threshold: f32 },
    Manual,
}

/// Garbage collection strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GarbageCollectionStrategy {
    MarkAndSweep,
    Generational,
    Incremental,
    Concurrent,
}

/// Quantization quality configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationQualityConfig {
    /// Quality vs performance trade-off
    pub quality_performance_balance: f32, // 0.0 = performance, 1.0 = quality
    
    /// Quality metrics
    pub quality_metrics: Vec<QualityMetric>,
    
    /// Quality monitoring
    pub quality_monitoring: QualityMonitoringConfig,
    
    /// Quality assurance
    pub quality_assurance: QualityAssuranceConfig,
}

/// Quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityMetric {
    MeanSquaredError,
    SignalToNoiseRatio,
    PeakSignalToNoiseRatio,
    StructuralSimilarity,
    PerceptualDistance,
    InformationLoss,
}

/// Quality monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMonitoringConfig {
    /// Enable monitoring
    pub enabled: bool,
    
    /// Monitoring frequency
    pub frequency: MonitoringFrequency,
    
    /// Monitoring thresholds
    pub thresholds: QualityThresholds,
    
    /// Alerting configuration
    pub alerting: AlertingConfig,
}

/// Monitoring frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitoringFrequency {
    Continuous,
    Periodic { interval_ms: u64 },
    Sampling { rate: f32 },
    OnDemand,
}

/// Alerting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertingConfig {
    /// Enable alerting
    pub enabled: bool,
    
    /// Alert thresholds
    pub thresholds: HashMap<String, f64>,
    
    /// Alert destinations
    pub destinations: Vec<AlertDestination>,
}

/// Alert destinations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertDestination {
    Log,
    Email { address: String },
    Webhook { url: String },
    Metrics { system: String },
}

/// Quality assurance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityAssuranceConfig {
    /// Enable quality assurance
    pub enabled: bool,
    
    /// Testing configuration
    pub testing: QualityTestingConfig,
    
    /// Validation configuration
    pub validation: QualityValidationConfig,
}

/// Quality testing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityTestingConfig {
    /// Test suite enabled
    pub enabled: bool,
    
    /// Test data configuration
    pub test_data: TestDataConfig,
    
    /// Test frequency
    pub frequency: TestFrequency,
}

/// Test data configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDataConfig {
    /// Use synthetic test data
    pub use_synthetic: bool,
    
    /// Use real data samples
    pub use_real_samples: bool,
    
    /// Test data size
    pub test_data_size: usize,
    
    /// Test data diversity
    pub diversity_requirements: DiversityRequirements,
}

/// Diversity requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiversityRequirements {
    /// Dimension ranges
    pub dimension_ranges: Vec<(usize, usize)>,
    
    /// Distribution types
    pub distribution_types: Vec<DistributionType>,
    
    /// Noise levels
    pub noise_levels: Vec<f32>,
}

/// Distribution types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DistributionType {
    Uniform,
    Normal,
    Exponential,
    Clustered,
    Sparse,
    Dense,
}

/// Test frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TestFrequency {
    OnChange,
    Periodic { interval_ms: u64 },
    OnDemand,
    Continuous,
}

/// Quality validation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityValidationConfig {
    /// Validation enabled
    pub enabled: bool,
    
    /// Validation criteria
    pub criteria: Vec<ValidationCriterion>,
    
    /// Validation actions
    pub actions: ValidationActions,
}

/// Validation criterion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCriterion {
    /// Metric name
    pub metric: String,
    
    /// Threshold value
    pub threshold: f64,
    
    /// Comparison operator
    pub operator: ComparisonOperator,
    
    /// Severity level
    pub severity: SeverityLevel,
}

/// Comparison operators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComparisonOperator {
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Equal,
    NotEqual,
}

/// Severity levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SeverityLevel {
    Info,
    Warning,
    Error,
    Critical,
}

/// Validation actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationActions {
    /// Action on validation failure
    pub on_failure: ValidationAction,
    
    /// Action on validation warning
    pub on_warning: ValidationAction,
    
    /// Action on validation success
    pub on_success: ValidationAction,
}

/// Validation actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidationAction {
    Continue,
    Warn,
    Fail,
    Fallback,
    Retry,
    Ignore,
}

/// Universal quantization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalQuantizationStats {
    /// Overall statistics
    pub overall: OverallQuantizationStats,
    
    /// Per-stage statistics
    pub per_stage: HashMap<String, StageQuantizationStats>,
    
    /// Hardware utilization
    pub hardware_utilization: HardwareUtilizationStats,
    
    /// Quality metrics
    pub quality_metrics: QualityMetricsStats,
    
    /// Performance metrics
    pub performance_metrics: PerformanceMetricsStats,
}

/// Overall quantization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallQuantizationStats {
    /// Total vectors quantized
    pub total_vectors_quantized: u64,
    
    /// Total compression ratio achieved
    pub total_compression_ratio: f32,
    
    /// Total memory saved (bytes)
    pub total_memory_saved: u64,
    
    /// Average quantization time (ms)
    pub avg_quantization_time_ms: f64,
    
    /// Error rates
    pub error_rates: ErrorRates,
}

/// Error rates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRates {
    /// Quantization error rate
    pub quantization_error_rate: f64,
    
    /// Distance preservation error
    pub distance_preservation_error: f64,
    
    /// Reconstruction error
    pub reconstruction_error: f64,
}

/// Stage quantization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageQuantizationStats {
    /// Vectors processed in this stage
    pub vectors_processed: u64,
    
    /// Compression ratio for this stage
    pub compression_ratio: f32,
    
    /// Quality retention for this stage
    pub quality_retention: f32,
    
    /// Processing time for this stage
    pub processing_time_ms: f64,
    
    /// Memory usage for this stage
    pub memory_usage_bytes: u64,
}

/// Hardware utilization statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareUtilizationStats {
    /// CPU utilization
    pub cpu_utilization: f32,
    
    /// GPU utilization
    pub gpu_utilization: Option<f32>,
    
    /// Memory utilization
    pub memory_utilization: f32,
    
    /// SIMD utilization
    pub simd_utilization: f32,
}

/// Quality metrics statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityMetricsStats {
    /// Mean squared error
    pub mean_squared_error: f64,
    
    /// Signal to noise ratio
    pub signal_to_noise_ratio: f64,
    
    /// Distance preservation ratio
    pub distance_preservation_ratio: f64,
    
    /// Information retention ratio
    pub information_retention_ratio: f64,
}

/// Performance metrics statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetricsStats {
    /// Throughput (vectors/second)
    pub throughput: f64,
    
    /// Latency (ms)
    pub latency_ms: f64,
    
    /// Memory bandwidth utilization
    pub memory_bandwidth_utilization: f32,
    
    /// Cache hit rate
    pub cache_hit_rate: f32,
}

/// Quantization capabilities
#[derive(Debug, Clone)]
pub struct QuantizationCapabilities {
    /// Supported quantization levels
    pub supported_levels: Vec<UniversalQuantizationLevel>,
    
    /// Hardware capabilities
    pub hardware_capabilities: Arc<HardwareCapabilities>,
    
    /// Performance characteristics
    pub performance_characteristics: PerformanceCharacteristics,
    
    /// Quality characteristics
    pub quality_characteristics: QualityCharacteristics,
}

/// Performance characteristics
#[derive(Debug, Clone)]
pub struct PerformanceCharacteristics {
    /// Maximum throughput (vectors/second)
    pub max_throughput: f64,
    
    /// Minimum latency (ms)
    pub min_latency_ms: f64,
    
    /// Memory efficiency
    pub memory_efficiency: f32,
    
    /// Computational complexity
    pub computational_complexity: ComputationalComplexity,
}

/// Computational complexity
#[derive(Debug, Clone)]
pub enum ComputationalComplexity {
    Constant,
    Linear,
    Logarithmic,
    Quadratic,
    Exponential,
}

/// Quality characteristics
#[derive(Debug, Clone)]
pub struct QualityCharacteristics {
    /// Best achievable compression ratio
    pub max_compression_ratio: f32,
    
    /// Minimum quality retention
    pub min_quality_retention: f32,
    
    /// Distance preservation capability
    pub distance_preservation_capability: f32,
    
    /// Noise characteristics
    pub noise_characteristics: NoiseCharacteristics,
}

/// Noise characteristics
#[derive(Debug, Clone)]
pub struct NoiseCharacteristics {
    /// Quantization noise level
    pub quantization_noise_level: f32,
    
    /// Noise distribution type
    pub noise_distribution: NoiseDistribution,
    
    /// Noise frequency characteristics
    pub noise_frequency: NoiseFrequency,
}

/// Noise distribution
#[derive(Debug, Clone)]
pub enum NoiseDistribution {
    Uniform,
    Gaussian,
    Laplacian,
    Exponential,
}

/// Noise frequency
#[derive(Debug, Clone)]
pub enum NoiseFrequency {
    LowFrequency,
    MidFrequency,
    HighFrequency,
    Broadband,
}

impl Default for UniversalQuantizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stages: vec![
                ProgressiveQuantizationStage {
                    name: "binary".to_string(),
                    level: UniversalQuantizationLevel::Binary {
                        threshold_strategy: BinaryThresholdStrategy::Zero,
                    },
                    usage_threshold: QuantizationThreshold {
                        dataset_size: Some(DatasetSizeThreshold {
                            min_vectors: 1000,
                            max_vectors: None,
                            min_dimension: 64,
                            max_dimension: None,
                        }),
                        memory_pressure: None,
                        performance_requirements: None,
                        quality_requirements: None,
                    },
                    config: QuantizationStageConfig {
                        parameters: HashMap::new(),
                        hardware_optimizations: StageHardwareConfig {
                            preferred_hardware: vec![HardwarePreference::SIMD],
                            fallback_options: vec![HardwarePreference::CPU],
                            hardware_parameters: HashMap::new(),
                        },
                        memory_management: StageMemoryConfig {
                            allocation_strategy: MemoryAllocationStrategy::Pooled,
                            buffer_reuse: true,
                            memory_limits: MemoryLimits {
                                max_memory_per_stage: 1024 * 1024 * 1024, // 1GB
                                max_total_memory: 4 * 1024 * 1024 * 1024, // 4GB
                                max_growth_rate: 0.1,
                            },
                        },
                        validation: StageValidationConfig {
                            enabled: true,
                            frequency: ValidationFrequency::Sampling { rate: 0.01 },
                            metrics: vec![ValidationMetric::Accuracy, ValidationMetric::CompressionRatio],
                            error_handling: ValidationErrorHandling::Warn,
                        },
                    },
                    expected_compression_ratio: 32.0, // 32:1 compression
                    expected_quality_retention: 0.8,  // 80% quality retention
                },
                ProgressiveQuantizationStage {
                    name: "int8".to_string(),
                    level: UniversalQuantizationLevel::Int8 {
                        scale_strategy: ScaleStrategy::MinMax,
                        zero_point_strategy: ZeroPointStrategy::Minimum,
                    },
                    usage_threshold: QuantizationThreshold {
                        dataset_size: Some(DatasetSizeThreshold {
                            min_vectors: 100,
                            max_vectors: Some(10000),
                            min_dimension: 32,
                            max_dimension: Some(2048),
                        }),
                        memory_pressure: None,
                        performance_requirements: None,
                        quality_requirements: None,
                    },
                    config: QuantizationStageConfig {
                        parameters: HashMap::new(),
                        hardware_optimizations: StageHardwareConfig {
                            preferred_hardware: vec![HardwarePreference::SIMD, HardwarePreference::CPU],
                            fallback_options: vec![HardwarePreference::CPU],
                            hardware_parameters: HashMap::new(),
                        },
                        memory_management: StageMemoryConfig {
                            allocation_strategy: MemoryAllocationStrategy::Pooled,
                            buffer_reuse: true,
                            memory_limits: MemoryLimits {
                                max_memory_per_stage: 2 * 1024 * 1024 * 1024, // 2GB
                                max_total_memory: 8 * 1024 * 1024 * 1024, // 8GB
                                max_growth_rate: 0.15,
                            },
                        },
                        validation: StageValidationConfig {
                            enabled: true,
                            frequency: ValidationFrequency::Sampling { rate: 0.05 },
                            metrics: vec![
                                ValidationMetric::Accuracy,
                                ValidationMetric::DistancePreservation,
                                ValidationMetric::CompressionRatio,
                            ],
                            error_handling: ValidationErrorHandling::Warn,
                        },
                    },
                    expected_compression_ratio: 4.0, // 4:1 compression
                    expected_quality_retention: 0.95, // 95% quality retention
                },
            ],
            hardware_optimizations: HardwareQuantizationConfig::default(),
            memory_config: QuantizationMemoryConfig::default(),
            quality_config: QuantizationQualityConfig::default(),
            engine_overrides: HashMap::new(),
        }
    }
}

impl Default for HardwareQuantizationConfig {
    fn default() -> Self {
        Self {
            simd_config: SIMDQuantizationConfig {
                enabled: true,
                instruction_set_preference: vec![
                    SIMDInstructionSet::AVX512,
                    SIMDInstructionSet::AVX2,
                    SIMDInstructionSet::SSE42,
                ],
                vector_width_optimization: true,
                alignment_bytes: 32,
            },
            gpu_config: GPUQuantizationConfig {
                enabled: false, // Disabled by default, enabled when GPU detected
                memory_strategy: GPUMemoryStrategy::OnDemand,
                batch_size: 1024,
                kernel_preference: vec![GPUKernel::Auto],
            },
            cpu_config: CPUQuantizationConfig {
                threading: ThreadingConfig {
                    thread_count: None, // Auto-detect
                    thread_affinity: false,
                    work_stealing: true,
                },
                cache_optimization: CacheOptimizationConfig {
                    cache_friendly_layout: true,
                    prefetch_strategy: PrefetchStrategy::Adaptive,
                    cache_line_optimization: true,
                },
                instruction_optimization: InstructionOptimizationConfig {
                    loop_unrolling: true,
                    vectorization_hints: true,
                    branch_prediction_optimization: true,
                },
            },
            memory_config: HardwareMemoryConfig {
                alignment_bytes: 32,
                numa_aware: false,
                prefetch_enabled: true,
                use_memory_pool: true,
            },
        }
    }
}

impl Default for QuantizationMemoryConfig {
    fn default() -> Self {
        Self {
            memory_pool: MemoryPoolConfig {
                enabled: true,
                initial_size: 256 * 1024 * 1024, // 256MB
                max_size: 2 * 1024 * 1024 * 1024, // 2GB
                growth_strategy: PoolGrowthStrategy::Exponential { factor: 1.5 },
            },
            buffer_management: BufferManagementConfig {
                reuse_enabled: true,
                max_buffer_age_ms: 60000, // 1 minute
                size_limits: BufferSizeLimits {
                    min_size: 4096,        // 4KB
                    max_size: 64 * 1024 * 1024, // 64MB
                    preferred_size: 1024 * 1024, // 1MB
                },
            },
            garbage_collection: GarbageCollectionConfig {
                enabled: true,
                frequency: GarbageCollectionFrequency::MemoryPressure { threshold: 0.8 },
                // strategy removed -  GarbageCollectionStrategy::Incremental,
            },
        }
    }
}

impl Default for QuantizationQualityConfig {
    fn default() -> Self {
        Self {
            quality_performance_balance: 0.7, // Favor quality slightly
            quality_metrics: vec![
                QualityMetric::MeanSquaredError,
                QualityMetric::SignalToNoiseRatio,
                QualityMetric::StructuralSimilarity,
            ],
            quality_monitoring: QualityMonitoringConfig {
                enabled: true,
                frequency: MonitoringFrequency::Sampling { rate: 0.01 },
                thresholds: QualityThresholds {
                    min_recall: 0.8,
                    max_error_rate: 0.1,
                    min_distance_preservation: 0.85,
                    max_quantization_noise: 0.15,
                },
                alerting: AlertingConfig {
                    enabled: true,
                    thresholds: HashMap::from([
                        ("error_rate".to_string(), 0.05),
                        ("quality_degradation".to_string(), 0.2),
                    ]),
                    destinations: vec![AlertDestination::Log],
                },
            },
            quality_assurance: QualityAssuranceConfig {
                enabled: true,
                testing: QualityTestingConfig {
                    enabled: true,
                    test_data: TestDataConfig {
                        use_synthetic: true,
                        use_real_samples: true,
                        test_data_size: 1000,
                        diversity_requirements: DiversityRequirements {
                            dimension_ranges: vec![(64, 128), (256, 512), (768, 1024)],
                            distribution_types: vec![
                                DistributionType::Normal,
                                DistributionType::Uniform,
                                DistributionType::Clustered,
                            ],
                            noise_levels: vec![0.0, 0.1, 0.2],
                        },
                    },
                    frequency: TestFrequency::Periodic { interval_ms: 3600000 }, // 1 hour
                },
                validation: QualityValidationConfig {
                    enabled: true,
                    criteria: vec![
                        ValidationCriterion {
                            metric: "recall".to_string(),
                            threshold: 0.8,
                            operator: ComparisonOperator::GreaterThanOrEqual,
                            severity: SeverityLevel::Error,
                        },
                        ValidationCriterion {
                            metric: "compression_ratio".to_string(),
                            threshold: 2.0,
                            operator: ComparisonOperator::GreaterThanOrEqual,
                            severity: SeverityLevel::Warning,
                        },
                    ],
                    actions: ValidationActions {
                        on_failure: ValidationAction::Fallback,
                        on_warning: ValidationAction::Warn,
                        on_success: ValidationAction::Continue,
                    },
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_universal_quantization_config_creation() {
        let config = UniversalQuantizationConfig::default();
        
        assert!(config.enabled);
        assert_eq!(config.stages.len(), 2);
        assert_eq!(config.stages[0].name, "binary");
        assert_eq!(config.stages[1].name, "int8");
        assert!(config.hardware_optimizations.simd_config.enabled);
    }
    
    #[test]
    fn test_quantization_levels() {
        let binary_level = UniversalQuantizationLevel::Binary {
            threshold_strategy: BinaryThresholdStrategy::Zero,
        };
        
        let int8_level = UniversalQuantizationLevel::Int8 {
            scale_strategy: ScaleStrategy::MinMax,
            zero_point_strategy: ZeroPointStrategy::Minimum,
        };
        
        let pq_level = UniversalQuantizationLevel::ProductQuantization {
            segments: 16,
            bits_per_segment: 8,
            codebook_strategy: CodebookStrategy::KMeans {
                max_iterations: 100,
                tolerance: 0.001,
            },
        };
        
        assert!(matches!(binary_level, UniversalQuantizationLevel::Binary { .. }));
        assert!(matches!(int8_level, UniversalQuantizationLevel::Int8 { .. }));
        assert!(matches!(pq_level, UniversalQuantizationLevel::ProductQuantization { .. }));
    }
    
    #[test]
    fn test_hardware_quantization_config() {
        let hardware_config = HardwareQuantizationConfig::default();
        
        assert!(hardware_config.simd_config.enabled);
        assert!(!hardware_config.gpu_config.enabled); // Disabled by default
        assert!(hardware_config.cpu_config.threading.work_stealing);
        assert!(hardware_config.memory_config.use_memory_pool);
    }
    
    #[test]
    fn test_quality_configuration() {
        let quality_config = QuantizationQualityConfig::default();
        
        assert_eq!(quality_config.quality_performance_balance, 0.7);
        assert!(quality_config.quality_monitoring.enabled);
        assert!(quality_config.quality_assurance.enabled);
        assert_eq!(quality_config.quality_metrics.len(), 3);
    }
    
    #[test]
    fn test_memory_configuration() {
        let memory_config = QuantizationMemoryConfig::default();
        
        assert!(memory_config.memory_pool.enabled);
        assert_eq!(memory_config.memory_pool.initial_size, 256 * 1024 * 1024);
        assert!(memory_config.buffer_management.reuse_enabled);
        assert!(memory_config.garbage_collection.enabled);
    }
}