// Universal Compression Infrastructure
// Shared compression capabilities across all storage engines

use std::collections::HashMap;

use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::metrics::compression::CompressionData;
use proximadb_compression::CompressionAlgorithm;

/// Universal compression configuration
#[derive(Debug, Clone)]
pub struct UniversalCompressionConfig {
    /// Enable compression
    pub enabled: bool,

    /// Primary compression algorithm
    pub primary_algorithm: CompressionAlgorithm,

    /// Fallback algorithms
    pub fallback_algorithms: Vec<CompressionAlgorithm>,

    /// Compression level (1-9, algorithm dependent)
    pub compression_level: u8,

    /// Adaptive compression settings
    pub adaptive_settings: AdaptiveCompressionSettings,

    /// Context-aware compression
    pub context_aware: ContextAwareCompressionConfig,

    /// Hardware optimizations
    pub hardware_optimizations: CompressionHardwareConfig,

    /// Performance tuning
    pub performance_config: CompressionPerformanceConfig,

    /// Quality settings
    pub quality_settings: CompressionQualitySettings,
}

/// Adaptive compression settings
#[derive(Debug, Clone)]
pub struct AdaptiveCompressionSettings {
    /// Enable adaptive compression
    pub enabled: bool,

    /// Adaptation strategies
    pub strategies: Vec<CompressionStrategy>,

    /// Adaptation criteria
    pub criteria: AdaptationCriteria,

    /// Minimum adaptation interval
    pub min_adaptation_interval_ms: u64,

    /// Maximum adaptation overhead
    pub max_adaptation_overhead_percent: f32,
}

/// Adaptive compression strategies
#[derive(Debug, Clone)]
pub enum AdaptiveStrategy {
    /// Data-driven selection based on content analysis
    DataDriven,
    /// Performance-driven selection based on benchmarks
    PerformanceDriven,
    /// Hardware-driven selection based on available capabilities
    HardwareDriven,
    /// Context-driven selection based on usage patterns
    ContextDriven,
}

/// Compression strategies
#[derive(Debug, Clone)]
pub enum CompressionStrategy {
    /// Optimize for speed
    Speed {
        target_latency_ms: f64,
        min_compression_ratio: f32,
    },

    /// Optimize for compression ratio
    Ratio {
        target_compression_ratio: f32,
        max_latency_ms: f64,
    },

    /// Balanced optimization
    Balanced {
        speed_weight: f32,
        ratio_weight: f32,
    },

    /// Memory-constrained optimization
    Memory {
        max_memory_usage_mb: u64,
        prefer_streaming: bool,
    },

    /// CPU-constrained optimization
    CPU {
        max_cpu_percent: f32,
        enable_hardware_acceleration: bool,
    },

    /// Network-optimized compression
    Network {
        bandwidth_mbps: f64,
        latency_ms: f64,
    },

    /// Storage-optimized compression
    Storage {
        storage_type: StorageType,
        io_pattern: IOPattern,
    },

    /// Custom strategy
    Custom {
        strategy_name: String,
        parameters: HashMap<String, serde_json::Value>,
    },
}

/// Storage types for optimization
#[derive(Debug, Clone)]
pub enum StorageType {
    SSD,
    HDD,
    NVMe,
    Cloud,
    Memory,
    Network,
}

/// I/O patterns
#[derive(Debug, Clone)]
pub enum IOPattern {
    Sequential,
    Random,
    Mixed,
    Burst,
    Streaming,
}

/// Adaptation criteria
#[derive(Debug, Clone)]
pub struct AdaptationCriteria {
    /// Data characteristics
    pub data_characteristics: DataCharacteristics,

    /// Performance thresholds
    pub performance_thresholds: PerformanceThresholds,

    /// Resource constraints
    pub resource_constraints: ResourceConstraints,

    /// Quality requirements
    pub quality_requirements: QualityRequirements,
}

/// Data characteristics for adaptation
#[derive(Debug, Clone)]
pub struct DataCharacteristics {
    /// Data entropy thresholds
    pub entropy_thresholds: EntropyThresholds,

    /// Data size thresholds
    pub size_thresholds: SizeThresholds,

    /// Data pattern recognition
    pub pattern_recognition: PatternRecognitionConfig,

    /// Data type hints
    pub data_type_hints: Vec<DataHint>,
}

/// Entropy thresholds
#[derive(Debug, Clone)]
pub struct EntropyThresholds {
    /// Low entropy threshold (highly compressible)
    pub low_entropy: f64,

    /// High entropy threshold (less compressible)
    pub high_entropy: f64,

    /// Entropy calculation method
    pub calculation_method: EntropyCalculationMethod,
}

/// Entropy calculation methods
#[derive(Debug, Clone)]
pub enum EntropyCalculationMethod {
    Shannon,
    Renyi { alpha: f64 },
    Tsallis { q: f64 },
    Approximate,
}

/// Size thresholds
#[derive(Debug, Clone)]
pub struct SizeThresholds {
    /// Small data threshold (bytes)
    pub small_data_threshold: u64,

    /// Large data threshold (bytes)
    pub large_data_threshold: u64,

    /// Block size considerations
    pub block_size_optimization: bool,
}

/// Pattern recognition configuration
#[derive(Debug, Clone)]
pub struct PatternRecognitionConfig {
    /// Enable pattern recognition
    pub enabled: bool,

    /// Pattern types to detect
    pub pattern_types: Vec<DataPattern>,

    /// Recognition accuracy threshold
    pub accuracy_threshold: f32,

    /// Pattern cache size
    pub pattern_cache_size: usize,
}

/// Data patterns
#[derive(Debug, Clone)]
pub enum DataPattern {
    Repetitive,
    Sequential,
    Sparse,
    Dense,
    Structured,
    Unstructured,
    Binary,
    Text,
    Numeric,
    Mixed,
}

/// Data type hints
#[derive(Debug, Clone)]
pub enum DataHint {
    Vector,
    Metadata,
    Index,
    Binary,
    Text,
    JSON,
    Parquet,
    Image,
    Audio,
    Unknown,
}

/// Performance thresholds
#[derive(Debug, Clone)]
pub struct PerformanceThresholds {
    /// Maximum compression latency (ms)
    pub max_compression_latency_ms: f64,

    /// Maximum decompression latency (ms)
    pub max_decompression_latency_ms: f64,

    /// Minimum throughput (MB/s)
    pub min_throughput_mbps: f64,

    /// Maximum CPU usage (%)
    pub max_cpu_usage_percent: f32,

    /// Maximum memory usage (MB)
    pub max_memory_usage_mb: u64,
}

/// Resource constraints
#[derive(Debug, Clone)]
pub struct ResourceConstraints {
    /// Memory constraints
    pub memory_constraints: MemoryConstraints,

    /// CPU constraints
    pub cpu_constraints: CPUConstraints,

    /// I/O constraints
    pub io_constraints: IOConstraints,

    /// Network constraints
    pub network_constraints: Option<NetworkConstraints>,
}

/// Memory constraints
#[derive(Debug, Clone)]
pub struct MemoryConstraints {
    /// Maximum working memory (bytes)
    pub max_working_memory: u64,

    /// Maximum buffer size (bytes)
    pub max_buffer_size: u64,

    /// Memory pressure threshold
    pub memory_pressure_threshold: f32,

    /// Enable memory mapping
    pub enable_memory_mapping: bool,
}

/// CPU constraints
#[derive(Debug, Clone)]
pub struct CPUConstraints {
    /// Maximum CPU cores to use
    pub max_cpu_cores: Option<usize>,

    /// Maximum CPU usage (%)
    pub max_cpu_usage_percent: f32,

    /// Enable hardware acceleration
    pub enable_hardware_acceleration: bool,

    /// Thread priority
    pub thread_priority: ThreadPriority,
}

/// Thread priority levels
#[derive(Debug, Clone)]
pub enum ThreadPriority {
    Low,
    Normal,
    High,
    RealTime,
}

/// I/O constraints
#[derive(Debug, Clone)]
pub struct IOConstraints {
    /// Maximum I/O bandwidth (MB/s)
    pub max_io_bandwidth_mbps: f64,

    /// I/O priority
    pub io_priority: IOPriority,

    /// Buffer I/O operations
    pub buffer_io: bool,

    /// Use direct I/O
    pub use_direct_io: bool,
}

/// I/O priority levels
#[derive(Debug, Clone)]
pub enum IOPriority {
    Low,
    Normal,
    High,
    RealTime,
}

/// Network constraints
#[derive(Debug, Clone)]
pub struct NetworkConstraints {
    /// Maximum network bandwidth (Mbps)
    pub max_bandwidth_mbps: f64,

    /// Network latency (ms)
    pub network_latency_ms: f64,

    /// Packet loss rate
    pub packet_loss_rate: f32,

    /// Enable compression for network transfer
    pub enable_network_compression: bool,
}

/// Quality requirements
#[derive(Debug, Clone)]
pub struct QualityRequirements {
    /// Minimum compression ratio
    pub min_compression_ratio: f32,

    /// Maximum quality loss (%)
    pub max_quality_loss_percent: f32,

    /// Lossless compression required
    pub require_lossless: bool,

    /// Error tolerance
    pub error_tolerance: ErrorTolerance,
}

/// Error tolerance levels
#[derive(Debug, Clone)]
pub enum ErrorTolerance {
    None,        // No errors allowed
    Low,         // Very low error rate
    Medium,      // Moderate error rate
    High,        // High error rate acceptable
    Custom(f64), // Custom error rate
}

/// Context-aware compression configuration
#[derive(Debug, Clone)]
pub struct ContextAwareCompressionConfig {
    /// Enable context-aware compression
    pub enabled: bool,

    /// Data type for context-aware compression
    pub data_type: CompressionData,

    /// Context types
    pub context_types: Vec<CompressionContext>,

    /// Context switching strategy
    pub switching_strategy: ContextSwitchingStrategy,

    /// Context learning configuration
    pub learning_config: ContextLearningConfig,
}

/// Compression contexts
#[derive(Debug, Clone)]
pub enum CompressionContext {
    /// Vector data compression
    VectorData {
        dimension: usize,
        // data_type removed -  VectorData,
        sparsity: f32,
    },

    /// Metadata compression
    Metadata {
        schema_type: MetadataSchemaType,
        cardinality: MetadataCardinality,
    },

    /// Index data compression
    IndexData { index_type: Index, density: f32 },

    /// Binary data compression
    BinaryData {
        data_format: BinaryDataFormat,
        structure: BinaryStructure,
    },

    /// Text data compression
    TextData {
        language: Option<String>,
        encoding: TextEncoding,
    },

    /// Mixed data compression
    MixedData {
        primary_type: String,
        secondary_types: Vec<String>,
    },
}

/// Vector data types
#[derive(Debug, Clone)]
pub enum VectorData {
    Float32,
    Float16,
    Int8,
    Binary,
    Quantized,
}

/// Metadata schema types
#[derive(Debug, Clone)]
pub enum MetadataSchemaType {
    JSON,
    StructuredJSON,
    KeyValue,
    Tabular,
    Hierarchical,
    FreeForm,
}

/// Metadata cardinality
#[derive(Debug, Clone)]
pub enum MetadataCardinality {
    Low,    // Few distinct values
    Medium, // Moderate distinct values
    High,   // Many distinct values
    Unique, // Mostly unique values
}

/// Index types for compression
#[derive(Debug, Clone)]
pub enum Index {
    BTree,
    Hash,
    Bitmap,
    Inverted,
    Spatial,
    FullText,
}

/// Binary data formats
#[derive(Debug, Clone)]
pub enum BinaryDataFormat {
    Raw,
    Structured,
    Compressed,
    Encrypted,
    Serialized,
}

/// Binary structure types
#[derive(Debug, Clone)]
pub enum BinaryStructure {
    FixedLength,
    VariableLength,
    HeaderBased,
    Delimited,
    Nested,
}

/// Text encodings
#[derive(Debug, Clone)]
pub enum TextEncoding {
    UTF8,
    UTF16,
    ASCII,
    Latin1,
    Unknown,
}

/// Context switching strategies
#[derive(Debug, Clone)]
pub enum ContextSwitchingStrategy {
    /// Automatic detection
    Automatic {
        detection_threshold: f32,
        min_switch_interval_ms: u64,
    },

    /// Manual context specification
    Manual,

    /// Hybrid approach
    Hybrid {
        auto_detection: bool,
        manual_override: bool,
    },

    /// Learning-based switching
    Learning {
        learning_rate: f32,
        adaptation_period: u64,
    },
}

/// Context learning configuration
#[derive(Debug, Clone)]
pub struct ContextLearningConfig {
    /// Enable learning
    pub enabled: bool,

    /// Learning algorithms
    pub algorithms: Vec<LearningAlgorithm>,

    /// Training data requirements
    pub training_requirements: TrainingRequirements,

    /// Model persistence
    pub model_persistence: ModelPersistenceConfig,
}

/// Learning algorithms
#[derive(Debug, Clone)]
pub enum LearningAlgorithm {
    /// Decision tree learning
    DecisionTree { max_depth: u32, min_samples: u32 },

    /// Neural network learning
    NeuralNetwork {
        hidden_layers: Vec<u32>,
        learning_rate: f32,
    },

    /// Ensemble methods
    Ensemble {
        base_learners: Vec<String>,
        combination_strategy: CombinationStrategy,
    },

    /// Reinforcement learning
    Reinforcement {
        exploration_rate: f32,
        discount_factor: f32,
    },
}

/// Combination strategies for ensembles
#[derive(Debug, Clone)]
pub enum CombinationStrategy {
    Voting,
    Averaging,
    Weighted,
    Stacking,
}

/// Training requirements
#[derive(Debug, Clone)]
pub struct TrainingRequirements {
    /// Minimum training samples
    pub min_training_samples: u64,

    /// Training data diversity
    pub diversity_requirements: DiversityRequirements,

    /// Training frequency
    pub training_frequency: TrainingFrequency,

    /// Validation requirements
    pub validation_requirements: ValidationRequirements,
}

/// Diversity requirements for training
#[derive(Debug, Clone)]
pub struct DiversityRequirements {
    /// Data type diversity
    pub data_types: Vec<String>,

    /// Size diversity
    pub size_ranges: Vec<(u64, u64)>,

    /// Pattern diversity
    pub pattern_types: Vec<String>,

    /// Context diversity
    pub context_types: Vec<String>,
}

/// Training frequency
#[derive(Debug, Clone)]
pub enum TrainingFrequency {
    Continuous,
    Periodic { interval_ms: u64 },
    OnDemand,
    Triggered { trigger_condition: String },
}

/// Validation requirements
#[derive(Debug, Clone)]
pub struct ValidationRequirements {
    /// Validation split ratio
    pub validation_split: f32,

    /// Cross-validation folds
    pub cross_validation_folds: u32,

    /// Performance metrics
    pub performance_metrics: Vec<String>,

    /// Minimum performance threshold
    pub min_performance_threshold: f64,
}

/// Model persistence configuration
#[derive(Debug, Clone)]
pub struct ModelPersistenceConfig {
    /// Enable model persistence
    pub enabled: bool,

    /// Model storage path
    pub storage_path: Option<String>,

    /// Model versioning
    pub versioning: ModelVersioningConfig,

    /// Model compression
    pub model_compression: bool,

    /// Checkpoint configuration
    pub checkpoint_config: CheckpointConfig,
}

/// Model versioning configuration
#[derive(Debug, Clone)]
pub struct ModelVersioningConfig {
    /// Enable versioning
    pub enabled: bool,

    /// Maximum versions to keep
    pub max_versions: u32,

    /// Version naming strategy
    pub naming_strategy: VersionNamingStrategy,
}

/// Version naming strategies
#[derive(Debug, Clone)]
pub enum VersionNamingStrategy {
    Timestamp,
    Sequential,
    Semantic,
    Hash,
}

/// Model versioning (alias for ModelVersioningConfig for backward compatibility)
pub type ModelVersioning = ModelVersioningConfig;

/// Checkpoint configuration
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Enable checkpoints
    pub enabled: bool,

    /// Checkpoint interval (ms)
    pub checkpoint_interval_ms: u64,

    /// Maximum checkpoints to keep
    pub max_checkpoints: u32,
}

/// Compression hardware configuration
#[derive(Debug, Clone)]
pub struct CompressionHardwareConfig {
    /// CPU optimizations
    pub cpu_optimizations: CPUOptimizations,

    /// GPU optimizations
    pub gpu_optimizations: GPUOptimizations,

    /// SIMD optimizations
    pub simd_optimizations: SIMDOptimizations,

    /// Hardware acceleration libraries
    pub acceleration_libraries: AccelerationLibraries,
}

/// CPU optimizations
#[derive(Debug, Clone)]
pub struct CPUOptimizations {
    /// Enable multithreading
    pub enable_multithreading: bool,

    /// Thread pool size
    pub thread_pool_size: Option<usize>,

    /// CPU instruction optimizations
    pub instruction_optimizations: Vec<CPUInstruction>,

    /// Cache optimization
    pub cache_optimization: CacheOptimization,
}

/// CPU instructions
#[derive(Debug, Clone)]
pub enum CPUInstruction {
    SSE2,
    SSE3,
    SSE41,
    SSE42,
    AVX,
    AVX2,
    AVX512,
    BMI1,
    BMI2,
    POPCNT,
    LZCNT,
}

/// Cache optimization
#[derive(Debug, Clone)]
pub struct CacheOptimization {
    /// Enable cache-friendly algorithms
    pub cache_friendly_algorithms: bool,

    /// Data prefetching
    pub data_prefetching: bool,

    /// Cache line alignment
    pub cache_line_alignment: bool,

    /// Memory access patterns optimization
    pub memory_access_optimization: bool,
}

/// GPU optimizations
#[derive(Debug, Clone)]
pub struct GPUOptimizations {
    /// Enable GPU acceleration
    pub enabled: bool,

    /// GPU compute libraries
    pub compute_libraries: Vec<GPUComputeLibrary>,

    /// Memory management
    pub memory_management: GPUMemoryManagement,

    /// Kernel optimization
    pub kernel_optimization: GPUKernelOptimization,
}

/// GPU compute libraries
#[derive(Debug, Clone)]
pub enum GPUComputeLibrary {
    CUDA,
    OpenCL,
    Vulkan,
    Metal,
    ROCm,
    DirectCompute,
}

/// GPU memory management
#[derive(Debug, Clone)]
pub struct GPUMemoryManagement {
    /// Memory allocation strategy
    pub allocation_strategy: GPUAllocationStrategy,

    /// Memory transfer optimization
    pub transfer_optimization: bool,

    /// Unified memory usage
    pub unified_memory: bool,
}

/// GPU allocation strategies
#[derive(Debug, Clone)]
pub enum GPUAllocationStrategy {
    Preallocated,
    OnDemand,
    Pooled,
    Streaming,
}

/// GPU kernel optimization
#[derive(Debug, Clone)]
pub struct GPUKernelOptimization {
    /// Occupancy optimization
    pub occupancy_optimization: bool,

    /// Register usage optimization
    pub register_optimization: bool,

    /// Shared memory optimization
    pub shared_memory_optimization: bool,

    /// Warp efficiency optimization
    pub warp_efficiency_optimization: bool,
}

/// SIMD optimizations
#[derive(Debug, Clone)]
pub struct SIMDOptimizations {
    /// Enable SIMD
    pub enabled: bool,

    /// Instruction set preferences
    pub instruction_sets: Vec<SIMDInstructionSet>,

    /// Vector width optimization
    pub vector_width_optimization: bool,

    /// Data alignment requirements
    pub alignment_requirements: AlignmentRequirements,
}

/// SIMD instruction sets
#[derive(Debug, Clone)]
pub enum SIMDInstructionSet {
    SSE,
    SSE2,
    SSE3,
    SSSE3,
    SSE41,
    SSE42,
    AVX,
    AVX2,
    AVX512F,
    AVX512BW,
    AVX512VL,
    NEON,
    Auto,
}

/// Alignment requirements
#[derive(Debug, Clone)]
pub struct AlignmentRequirements {
    /// Data alignment (bytes)
    pub data_alignment: usize,

    /// Memory alignment (bytes)
    pub memory_alignment: usize,

    /// Stack alignment (bytes)
    pub stack_alignment: usize,
}

/// Hardware acceleration libraries
#[derive(Debug, Clone)]
pub struct AccelerationLibraries {
    /// Intel libraries
    pub intel_libraries: Vec<IntelLibrary>,

    /// AMD libraries
    pub amd_libraries: Vec<AMDLibrary>,

    /// NVIDIA libraries
    pub nvidia_libraries: Vec<NVIDIALibrary>,

    /// ARM libraries
    pub arm_libraries: Vec<ARMLibrary>,
}

/// Intel acceleration libraries
#[derive(Debug, Clone)]
pub enum IntelLibrary {
    IPP,  // Intel Performance Primitives
    MKL,  // Math Kernel Library
    TBB,  // Threading Building Blocks
    DAAL, // Data Analytics Acceleration Library
}

/// AMD acceleration libraries
#[derive(Debug, Clone)]
pub enum AMDLibrary {
    BLIS, // BLAS-like Library Instantiation Software
    FFTW, // Fastest Fourier Transform in the West
    ROCm, // Radeon Open Compute
}

/// NVIDIA acceleration libraries
#[derive(Debug, Clone)]
pub enum NVIDIALibrary {
    CuBlas,
    CuFft,
    CuSparse,
    NPP, // NVIDIA Performance Primitives
    Thrust,
}

/// ARM acceleration libraries
#[derive(Debug, Clone)]
pub enum ARMLibrary {
    NEON,
    ComputeLibrary,
    CMSIS,
}

/// Compression performance configuration
#[derive(Debug, Clone)]
pub struct CompressionPerformanceConfig {
    /// Performance targets
    pub targets: PerformanceTargets,

    /// Monitoring configuration
    pub monitoring: PerformanceMonitoring,

    /// Optimization configuration
    pub optimization: PerformanceOptimization,

    /// Profiling configuration
    pub profiling: ProfilingConfig,
}

/// Performance targets
#[derive(Debug, Clone)]
pub struct PerformanceTargets {
    /// Target compression speed (MB/s)
    pub target_compression_speed: f64,

    /// Target decompression speed (MB/s)
    pub target_decompression_speed: f64,

    /// Target compression ratio
    pub target_compression_ratio: f32,

    /// Target latency (ms)
    pub target_latency_ms: f64,

    /// Target throughput (ops/s)
    pub target_throughput: f64,
}

/// Performance monitoring
#[derive(Debug, Clone)]
pub struct PerformanceMonitoring {
    /// Enable monitoring
    pub enabled: bool,

    /// Monitoring frequency
    pub frequency: MonitoringFrequency,

    /// Metrics to monitor
    pub metrics: Vec<PerformanceMetric>,

    /// Alert configuration
    pub alerting: AlertConfiguration,
}

/// Monitoring frequency
#[derive(Debug, Clone)]
pub enum MonitoringFrequency {
    Continuous,
    Periodic { interval_ms: u64 },
    OnOperation,
    OnDemand,
}

/// Performance metrics
#[derive(Debug, Clone)]
pub enum PerformanceMetric {
    CompressionSpeed,
    DecompressionSpeed,
    CompressionRatio,
    Latency,
    Throughput,
    CPUUsage,
    MemoryUsage,
    IOBandwidth,
    CacheHitRate,
    ErrorRate,
}

/// Alert configuration
#[derive(Debug, Clone)]
pub struct AlertConfiguration {
    /// Enable alerting
    pub enabled: bool,

    /// Alert thresholds
    pub thresholds: HashMap<String, f64>,

    /// Alert destinations
    pub destinations: Vec<AlertDestination>,

    /// Alert cooldown period
    pub cooldown_ms: u64,
}

/// Alert destinations
#[derive(Debug, Clone)]
pub enum AlertDestination {
    Log,
    Email { address: String },
    Webhook { url: String },
    Metrics { system: String },
    Console,
}

/// Performance optimization
#[derive(Debug, Clone)]
pub struct PerformanceOptimization {
    /// Enable automatic optimization
    pub auto_optimization: bool,

    /// Optimization strategies
    pub strategies: Vec<OptimizationStrategy>,

    /// Optimization frequency
    pub optimization_frequency: OptimizationFrequency,

    /// Optimization constraints
    pub constraints: OptimizationConstraints,
}

/// Optimization strategies
#[derive(Debug, Clone)]
pub enum OptimizationStrategy {
    AlgorithmSelection,
    ParameterTuning,
    HardwareAdaptation,
    DataAdaptation,
    ContextOptimization,
    Hybrid,
}

/// Optimization frequency
#[derive(Debug, Clone)]
pub enum OptimizationFrequency {
    Never,
    OnStartup,
    Periodic { interval_ms: u64 },
    OnPerformanceDegradation,
    OnContextChange,
    Adaptive,
}

/// Optimization constraints
#[derive(Debug, Clone)]
pub struct OptimizationConstraints {
    /// Maximum optimization time (ms)
    pub max_optimization_time_ms: u64,

    /// Maximum performance regression allowed
    pub max_regression_percent: f32,

    /// Minimum improvement threshold
    pub min_improvement_percent: f32,

    /// Resource limits during optimization
    pub resource_limits: ResourceLimits,
}

/// Resource limits
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum CPU usage during optimization (%)
    pub max_cpu_usage_percent: f32,

    /// Maximum memory usage during optimization (MB)
    pub max_memory_usage_mb: u64,

    /// Maximum I/O bandwidth during optimization (MB/s)
    pub max_io_bandwidth_mbps: f64,
}

/// Profiling configuration
#[derive(Debug, Clone)]
pub struct ProfilingConfig {
    /// Enable profiling
    pub enabled: bool,

    /// Profiling mode
    pub mode: ProfilingMode,

    /// Profiling metrics
    pub metrics: Vec<ProfilingMetric>,

    /// Profiling output
    pub output: ProfilingOutput,
}

/// Profiling modes
#[derive(Debug, Clone)]
pub enum ProfilingMode {
    Sampling { interval_ms: u64 },
    Instrumentation,
    Hybrid,
    Statistical,
}

/// Profiling metrics
#[derive(Debug, Clone)]
pub enum ProfilingMetric {
    CPUCycles,
    Instructions,
    CacheMisses,
    BranchMispredictions,
    MemoryAccess,
    IOOperations,
    FunctionCalls,
    ExecutionTime,
}

/// Profiling output
#[derive(Debug, Clone)]
pub struct ProfilingOutput {
    /// Output format
    pub format: ProfilingFormat,

    /// Output destination
    pub destination: ProfilingDestination,

    /// Output frequency
    pub frequency: ProfilingFrequency,
}

/// Profiling formats
#[derive(Debug, Clone)]
pub enum ProfilingFormat {
    JSON,
    CSV,
    Binary,
    FlameGraph,
    CallGraph,
    Custom { format_name: String },
}

/// Profiling destinations
#[derive(Debug, Clone)]
pub enum ProfilingDestination {
    File { path: String },
    Memory,
    Network { endpoint: String },
    Database { connection: String },
}

/// Profiling frequency
#[derive(Debug, Clone)]
pub enum ProfilingFrequency {
    OnDemand,
    Periodic { interval_ms: u64 },
    OnOperation,
    Continuous,
}

/// Compression quality settings
#[derive(Debug, Clone)]
pub struct CompressionQualitySettings {
    /// Quality vs speed trade-off
    pub quality_speed_balance: f32, // 0.0 = speed, 1.0 = quality

    /// Lossless compression preference
    pub prefer_lossless: bool,

    /// Quality metrics
    pub quality_metrics: Vec<QualityMetric>,

    /// Quality assurance
    pub quality_assurance: QualityAssurance,
}

/// Quality metrics
#[derive(Debug, Clone)]
pub enum QualityMetric {
    CompressionRatio,
    PSNR, // Peak Signal-to-Noise Ratio
    SSIM, // Structural Similarity Index
    MSE,  // Mean Squared Error
    RMSE, // Root Mean Squared Error
    InformationLoss,
    BitErrorRate,
    Custom { metric_name: String },
}

/// Quality assurance
#[derive(Debug, Clone)]
pub struct QualityAssurance {
    /// Enable quality assurance
    pub enabled: bool,

    /// Quality testing
    pub testing: QualityTesting,

    /// Quality validation
    pub validation: QualityValidation,

    /// Quality monitoring
    pub monitoring: QualityMonitoring,
}

/// Quality testing
#[derive(Debug, Clone)]
pub struct QualityTesting {
    /// Enable testing
    pub enabled: bool,

    /// Test data configuration
    pub test_data: TestDataConfiguration,

    /// Test frequency
    pub frequency: TestingFrequency,

    /// Test criteria
    pub criteria: Vec<TestCriterion>,
}

/// Test data configuration
#[derive(Debug, Clone)]
pub struct TestDataConfiguration {
    /// Use synthetic test data
    pub use_synthetic: bool,

    /// Use real data samples
    pub use_real_samples: bool,

    /// Test data sources
    pub data_sources: Vec<DataSource>,

    /// Test data size
    pub test_data_size: usize,
}

/// Data sources
#[derive(Debug, Clone)]
pub enum DataSource {
    File { path: String },
    Database { connection: String, query: String },
    Generator { generator_type: String },
    Memory { data_id: String },
    Network { endpoint: String },
}

/// Testing frequency
#[derive(Debug, Clone)]
pub enum TestingFrequency {
    OnStartup,
    Periodic { interval_ms: u64 },
    OnAlgorithmChange,
    OnDemand,
    Never,
}

/// Test criterion
#[derive(Debug, Clone)]
pub struct TestCriterion {
    /// Metric name
    pub metric: String,

    /// Expected value
    pub expected_value: f64,

    /// Tolerance
    pub tolerance: f64,

    /// Severity
    pub severity: TestSeverity,
}

/// Test severity levels
#[derive(Debug, Clone)]
pub enum TestSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Quality validation
#[derive(Debug, Clone)]
pub struct QualityValidation {
    /// Enable validation
    pub enabled: bool,

    /// Validation rules
    pub rules: Vec<ValidationRule>,

    /// Validation frequency
    pub frequency: ValidationFrequency,

    /// Validation actions
    pub actions: ValidationActions,
}

/// Validation rule
#[derive(Debug, Clone)]
pub struct ValidationRule {
    /// Rule name
    pub name: String,

    /// Rule condition
    pub condition: ValidationCondition,

    /// Action on violation
    pub action: ValidationAction,
}

/// Validation condition
#[derive(Debug, Clone)]
pub enum ValidationCondition {
    MetricThreshold {
        metric: String,
        threshold: f64,
        operator: ComparisonOperator,
    },
    CompressionRatioRange {
        min_ratio: f32,
        max_ratio: f32,
    },
    PerformanceRegression {
        baseline_metric: String,
        max_regression_percent: f32,
    },
    Custom {
        condition_name: String,
        parameters: HashMap<String, serde_json::Value>,
    },
}

/// Comparison operators
#[derive(Debug, Clone)]
pub enum ComparisonOperator {
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Equal,
    NotEqual,
}

/// Validation frequency
#[derive(Debug, Clone)]
pub enum ValidationFrequency {
    Always,
    Periodic { interval_ms: u64 },
    OnOperation,
    Sampling { rate: f32 },
    Never,
}

/// Validation actions
#[derive(Debug, Clone)]
pub struct ValidationActions {
    /// Action on validation failure
    pub on_failure: ValidationAction,

    /// Action on validation warning
    pub on_warning: ValidationAction,

    /// Action on validation success
    pub on_success: ValidationAction,
}

/// Validation action
#[derive(Debug, Clone)]
pub enum ValidationAction {
    Continue,
    Warn,
    Fail,
    Fallback {
        fallback_algorithm: CompressionAlgorithm,
    },
    Retry {
        max_retries: u32,
    },
    Abort,
}

/// Quality monitoring
#[derive(Debug, Clone)]
pub struct QualityMonitoring {
    /// Enable monitoring
    pub enabled: bool,

    /// Monitoring frequency
    pub frequency: MonitoringFrequency,

    /// Metrics to monitor
    pub metrics: Vec<QualityMetric>,

    /// Monitoring thresholds
    pub thresholds: HashMap<String, f64>,
}

/// Compression capabilities
#[derive(Debug, Clone)]
pub struct CompressionCapabilities {
    /// Supported algorithms
    pub supported_algorithms: Vec<CompressionAlgorithm>,

    /// Hardware capabilities
    pub hardware_capabilities: HardwareCapabilities,

    /// Performance characteristics
    pub performance_characteristics: HashMap<CompressionAlgorithm, PerformanceCharacteristics>,

    /// Quality characteristics
    pub quality_characteristics: HashMap<CompressionAlgorithm, QualityCharacteristics>,
}

/// Performance characteristics
#[derive(Debug, Clone)]
pub struct PerformanceCharacteristics {
    /// Compression speed (MB/s)
    pub compression_speed: f64,

    /// Decompression speed (MB/s)
    pub decompression_speed: f64,

    /// Memory usage (MB)
    pub memory_usage: f64,

    /// CPU usage (%)
    pub cpu_usage: f32,

    /// Latency (ms)
    pub latency: f64,
}

/// Quality characteristics
#[derive(Debug, Clone)]
pub struct QualityCharacteristics {
    /// Typical compression ratio
    pub typical_compression_ratio: f32,

    /// Maximum compression ratio
    pub max_compression_ratio: f32,

    /// Quality retention
    pub quality_retention: f32,

    /// Lossless capability
    pub lossless_capable: bool,
}

/// Compression statistics
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Overall statistics
    pub overall: OverallCompressionStats,

    /// Per-algorithm statistics
    pub per_algorithm: HashMap<String, AlgorithmCompressionStats>,

    /// Hardware utilization
    pub hardware_utilization: HardwareUtilizationStats,

    /// Quality metrics
    pub quality_metrics: QualityStats,

    /// Performance metrics
    pub performance_metrics: PerformanceStats,
}

/// Overall compression statistics
#[derive(Debug, Clone)]
pub struct OverallCompressionStats {
    /// Total bytes compressed
    pub total_bytes_compressed: u64,

    /// Total bytes decompressed
    pub total_bytes_decompressed: u64,

    /// Overall compression ratio
    pub overall_compression_ratio: f32,

    /// Total compression time (ms)
    pub total_compression_time_ms: u64,

    /// Total decompression time (ms)
    pub total_decompression_time_ms: u64,

    /// Error count
    pub error_count: u64,
}

/// Algorithm compression statistics
#[derive(Debug, Clone)]
pub struct AlgorithmCompressionStats {
    /// Bytes processed
    pub bytes_processed: u64,

    /// Compression ratio achieved
    pub compression_ratio: f32,

    /// Average compression time (ms)
    pub avg_compression_time_ms: f64,

    /// Average decompression time (ms)
    pub avg_decompression_time_ms: f64,

    /// Usage count
    pub usage_count: u64,

    /// Error count
    pub error_count: u64,
}

/// Hardware utilization statistics
#[derive(Debug, Clone)]
pub struct HardwareUtilizationStats {
    /// CPU utilization (%)
    pub cpu_utilization: f32,

    /// Memory utilization (%)
    pub memory_utilization: f32,

    /// GPU utilization (%) - if available
    pub gpu_utilization: Option<f32>,

    /// I/O bandwidth utilization (%)
    pub io_bandwidth_utilization: f32,
}

/// Quality statistics
#[derive(Debug, Clone)]
pub struct QualityStats {
    /// Average quality retention
    pub avg_quality_retention: f32,

    /// Quality variance
    pub quality_variance: f32,

    /// Lossless operation percentage
    pub lossless_percentage: f32,

    /// Quality test pass rate
    pub quality_test_pass_rate: f32,
}

/// Performance statistics
#[derive(Debug, Clone)]
pub struct PerformanceStats {
    /// Average compression speed (MB/s)
    pub avg_compression_speed: f64,

    /// Average decompression speed (MB/s)
    pub avg_decompression_speed: f64,

    /// Average latency (ms)
    pub avg_latency: f64,

    /// Throughput (ops/s)
    pub throughput: f64,

    /// Performance target achievement rate
    pub target_achievement_rate: f32,
}

impl Default for UniversalCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            primary_algorithm: CompressionAlgorithm::Zstd,
            fallback_algorithms: vec![CompressionAlgorithm::Lz4, CompressionAlgorithm::Snappy],
            compression_level: 3,
            adaptive_settings: AdaptiveCompressionSettings::default(),
            context_aware: ContextAwareCompressionConfig::default(),
            hardware_optimizations: CompressionHardwareConfig::default(),
            performance_config: CompressionPerformanceConfig::default(),
            quality_settings: CompressionQualitySettings::default(),
        }
    }
}

impl Default for AdaptiveCompressionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strategies: vec![CompressionStrategy::Balanced {
                speed_weight: 0.6,
                ratio_weight: 0.4,
            }],
            criteria: AdaptationCriteria {
                data_characteristics: DataCharacteristics {
                    entropy_thresholds: EntropyThresholds {
                        low_entropy: 2.0,
                        high_entropy: 7.0,
                        calculation_method: EntropyCalculationMethod::Shannon,
                    },
                    size_thresholds: SizeThresholds {
                        small_data_threshold: 1024,             // 1KB
                        large_data_threshold: 1024 * 1024 * 10, // 10MB
                        block_size_optimization: true,
                    },
                    pattern_recognition: PatternRecognitionConfig {
                        enabled: true,
                        pattern_types: vec![
                            DataPattern::Repetitive,
                            DataPattern::Sequential,
                            DataPattern::Sparse,
                        ],
                        accuracy_threshold: 0.8,
                        pattern_cache_size: 1000,
                    },
                    data_type_hints: vec![DataHint::Vector, DataHint::Metadata, DataHint::Index],
                },
                performance_thresholds: PerformanceThresholds {
                    max_compression_latency_ms: 100.0,
                    max_decompression_latency_ms: 50.0,
                    min_throughput_mbps: 100.0,
                    max_cpu_usage_percent: 80.0,
                    max_memory_usage_mb: 1024,
                },
                resource_constraints: ResourceConstraints {
                    memory_constraints: MemoryConstraints {
                        max_working_memory: 512 * 1024 * 1024, // 512MB
                        max_buffer_size: 64 * 1024 * 1024,     // 64MB
                        memory_pressure_threshold: 0.8,
                        enable_memory_mapping: true,
                    },
                    cpu_constraints: CPUConstraints {
                        max_cpu_cores: None,
                        max_cpu_usage_percent: 80.0,
                        enable_hardware_acceleration: true,
                        thread_priority: ThreadPriority::Normal,
                    },
                    io_constraints: IOConstraints {
                        max_io_bandwidth_mbps: 1000.0,
                        io_priority: IOPriority::Normal,
                        buffer_io: true,
                        use_direct_io: false,
                    },
                    network_constraints: None,
                },
                quality_requirements: QualityRequirements {
                    min_compression_ratio: 1.5,
                    max_quality_loss_percent: 5.0,
                    require_lossless: false,
                    error_tolerance: ErrorTolerance::Low,
                },
            },
            min_adaptation_interval_ms: 1000,
            max_adaptation_overhead_percent: 5.0,
        }
    }
}

impl Default for ContextAwareCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            data_type: crate::metrics::compression::CompressionData::Mixed,
            context_types: vec![
                CompressionContext::VectorData {
                    dimension: 768,
                    // data_type removed -  VectorDataType::Float32,
                    sparsity: 0.0,
                },
                CompressionContext::Metadata {
                    schema_type: MetadataSchemaType::JSON,
                    cardinality: MetadataCardinality::Medium,
                },
            ],
            switching_strategy: ContextSwitchingStrategy::Automatic {
                detection_threshold: 0.8,
                min_switch_interval_ms: 5000,
            },
            learning_config: ContextLearningConfig {
                enabled: false, // Disabled by default for simplicity
                algorithms: vec![],
                training_requirements: TrainingRequirements {
                    min_training_samples: 1000,
                    diversity_requirements: DiversityRequirements {
                        data_types: vec!["vector".to_string(), "metadata_info".to_string()],
                        size_ranges: vec![(1024, 1024 * 1024)],
                        pattern_types: vec!["structured".to_string(), "unstructured".to_string()],
                        context_types: vec!["vector".to_string(), "metadata_info".to_string()],
                    },
                    training_frequency: TrainingFrequency::Periodic {
                        interval_ms: 3600000,
                    },
                    validation_requirements: ValidationRequirements {
                        validation_split: 0.2,
                        cross_validation_folds: 5,
                        performance_metrics: vec![
                            "accuracy".to_string(),
                            "compression_ratio".to_string(),
                        ],
                        min_performance_threshold: 0.8,
                    },
                },
                model_persistence: ModelPersistenceConfig {
                    enabled: false,
                    storage_path: None,
                    versioning: ModelVersioningConfig {
                        enabled: false,
                        max_versions: 5,
                        naming_strategy: VersionNamingStrategy::Timestamp,
                    },
                    model_compression: true,
                    checkpoint_config: CheckpointConfig {
                        enabled: false,
                        checkpoint_interval_ms: 300000,
                        max_checkpoints: 5,
                    },
                },
            },
        }
    }
}

impl Default for CompressionHardwareConfig {
    fn default() -> Self {
        Self {
            cpu_optimizations: CPUOptimizations {
                enable_multithreading: true,
                thread_pool_size: None, // Auto-detect
                instruction_optimizations: vec![
                    CPUInstruction::SSE42,
                    CPUInstruction::AVX2,
                    CPUInstruction::AVX512,
                ],
                cache_optimization: CacheOptimization {
                    cache_friendly_algorithms: true,
                    data_prefetching: true,
                    cache_line_alignment: true,
                    memory_access_optimization: true,
                },
            },
            gpu_optimizations: GPUOptimizations {
                enabled: false, // Disabled by default
                compute_libraries: vec![GPUComputeLibrary::CUDA, GPUComputeLibrary::OpenCL],
                memory_management: GPUMemoryManagement {
                    allocation_strategy: GPUAllocationStrategy::OnDemand,
                    transfer_optimization: true,
                    unified_memory: false,
                },
                kernel_optimization: GPUKernelOptimization {
                    occupancy_optimization: true,
                    register_optimization: true,
                    shared_memory_optimization: true,
                    warp_efficiency_optimization: true,
                },
            },
            simd_optimizations: SIMDOptimizations {
                enabled: true,
                instruction_sets: vec![
                    SIMDInstructionSet::AVX512F,
                    SIMDInstructionSet::AVX2,
                    SIMDInstructionSet::SSE42,
                ],
                vector_width_optimization: true,
                alignment_requirements: AlignmentRequirements {
                    data_alignment: 32,
                    memory_alignment: 64,
                    stack_alignment: 16,
                },
            },
            acceleration_libraries: AccelerationLibraries {
                intel_libraries: vec![IntelLibrary::IPP, IntelLibrary::MKL],
                amd_libraries: vec![AMDLibrary::BLIS],
                nvidia_libraries: vec![NVIDIALibrary::CuBlas],
                arm_libraries: vec![ARMLibrary::NEON],
            },
        }
    }
}

impl Default for CompressionPerformanceConfig {
    fn default() -> Self {
        Self {
            targets: PerformanceTargets {
                target_compression_speed: 500.0,    // 500 MB/s
                target_decompression_speed: 1000.0, // 1000 MB/s
                target_compression_ratio: 3.0,      // 3:1 ratio
                target_latency_ms: 10.0,            // 10ms
                target_throughput: 1000.0,          // 1000 ops/s
            },
            monitoring: PerformanceMonitoring {
                enabled: true,
                frequency: MonitoringFrequency::Periodic { interval_ms: 10000 },
                metrics: vec![
                    PerformanceMetric::CompressionSpeed,
                    PerformanceMetric::DecompressionSpeed,
                    PerformanceMetric::CompressionRatio,
                    PerformanceMetric::Latency,
                ],
                alerting: AlertConfiguration {
                    enabled: true,
                    thresholds: HashMap::from([
                        ("compression_speed".to_string(), 100.0),
                        ("latency".to_string(), 50.0),
                    ]),
                    destinations: vec![AlertDestination::Log],
                    cooldown_ms: 60000,
                },
            },
            optimization: PerformanceOptimization {
                auto_optimization: true,
                strategies: vec![
                    OptimizationStrategy::AlgorithmSelection,
                    OptimizationStrategy::ParameterTuning,
                ],
                optimization_frequency: OptimizationFrequency::OnPerformanceDegradation,
                constraints: OptimizationConstraints {
                    max_optimization_time_ms: 5000,
                    max_regression_percent: 5.0,
                    min_improvement_percent: 10.0,
                    resource_limits: ResourceLimits {
                        max_cpu_usage_percent: 50.0,
                        max_memory_usage_mb: 256,
                        max_io_bandwidth_mbps: 100.0,
                    },
                },
            },
            profiling: ProfilingConfig {
                enabled: false, // Disabled by default for performance
                mode: ProfilingMode::Sampling { interval_ms: 1000 },
                metrics: vec![
                    ProfilingMetric::CPUCycles,
                    ProfilingMetric::Instructions,
                    ProfilingMetric::CacheMisses,
                ],
                output: ProfilingOutput {
                    format: ProfilingFormat::JSON,
                    destination: ProfilingDestination::File {
                        path: "/tmp/compression_profile.json".to_string(),
                    },
                    frequency: ProfilingFrequency::OnDemand,
                },
            },
        }
    }
}

impl Default for CompressionQualitySettings {
    fn default() -> Self {
        Self {
            quality_speed_balance: 0.6, // Slightly favor quality
            prefer_lossless: true,
            quality_metrics: vec![
                QualityMetric::CompressionRatio,
                QualityMetric::InformationLoss,
            ],
            quality_assurance: QualityAssurance {
                enabled: true,
                testing: QualityTesting {
                    enabled: true,
                    test_data: TestDataConfiguration {
                        use_synthetic: true,
                        use_real_samples: false,
                        data_sources: vec![],
                        test_data_size: 1000,
                    },
                    frequency: TestingFrequency::OnStartup,
                    criteria: vec![TestCriterion {
                        metric: "compression_ratio".to_string(),
                        expected_value: 2.0,
                        tolerance: 0.5,
                        severity: TestSeverity::Warning,
                    }],
                },
                validation: QualityValidation {
                    enabled: true,
                    rules: vec![ValidationRule {
                        name: "min_compression_ratio".to_string(),
                        condition: ValidationCondition::MetricThreshold {
                            metric: "compression_ratio".to_string(),
                            threshold: 1.5,
                            operator: ComparisonOperator::GreaterThanOrEqual,
                        },
                        action: ValidationAction::Warn,
                    }],
                    frequency: ValidationFrequency::Sampling { rate: 0.01 },
                    actions: ValidationActions {
                        on_failure: ValidationAction::Fallback {
                            fallback_algorithm: CompressionAlgorithm::Lz4,
                        },
                        on_warning: ValidationAction::Warn,
                        on_success: ValidationAction::Continue,
                    },
                },
                monitoring: QualityMonitoring {
                    enabled: true,
                    frequency: MonitoringFrequency::Periodic { interval_ms: 30000 },
                    metrics: vec![
                        QualityMetric::CompressionRatio,
                        QualityMetric::InformationLoss,
                    ],
                    thresholds: HashMap::from([
                        ("compression_ratio".to_string(), 1.5),
                        ("information_loss".to_string(), 0.1),
                    ]),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_universal_compression_config_creation() {
        let config = UniversalCompressionConfig::default();

        assert!(config.enabled);
        assert_eq!(config.primary_algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(config.compression_level, 3);
        assert!(config.adaptive_settings.enabled);
    }

    #[test]
    fn test_compression_strategies() {
        let speed_strategy = CompressionStrategy::Speed {
            target_latency_ms: 10.0,
            min_compression_ratio: 2.0,
        };

        let ratio_strategy = CompressionStrategy::Ratio {
            target_compression_ratio: 5.0,
            max_latency_ms: 100.0,
        };

        assert!(matches!(speed_strategy, CompressionStrategy::Speed { .. }));
        assert!(matches!(ratio_strategy, CompressionStrategy::Ratio { .. }));
    }

    #[test]
    fn test_hardware_optimization_config() {
        let hardware_config = CompressionHardwareConfig::default();

        assert!(hardware_config.cpu_optimizations.enable_multithreading);
        assert!(!hardware_config.gpu_optimizations.enabled);
        assert!(hardware_config.simd_optimizations.enabled);
        assert!(
            hardware_config
                .cpu_optimizations
                .cache_optimization
                .cache_friendly_algorithms
        );
    }

    #[test]
    fn test_context_aware_compression() {
        let context_config = ContextAwareCompressionConfig::default();

        assert!(context_config.enabled);
        assert_eq!(context_config.context_types.len(), 2);
        assert!(matches!(
            context_config.switching_strategy,
            ContextSwitchingStrategy::Automatic { .. }
        ));
    }

    #[test]
    fn test_performance_configuration() {
        let perf_config = CompressionPerformanceConfig::default();

        assert_eq!(perf_config.targets.target_compression_speed, 500.0);
        assert_eq!(perf_config.targets.target_decompression_speed, 1000.0);
        assert!(perf_config.monitoring.enabled);
        assert!(perf_config.optimization.auto_optimization);
    }

    #[test]
    fn test_quality_settings() {
        let quality_config = CompressionQualitySettings::default();

        assert_eq!(quality_config.quality_speed_balance, 0.6);
        assert!(quality_config.prefer_lossless);
        assert!(quality_config.quality_assurance.enabled);
        assert_eq!(quality_config.quality_metrics.len(), 2);
    }
}
