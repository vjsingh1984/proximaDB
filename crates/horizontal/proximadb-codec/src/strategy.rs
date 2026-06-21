//! Codec Selection Strategy Trait (ISP Compliant)
//!
//! Provides a trait-based abstraction for codec selection, replacing hardcoded heuristics.
//! Different strategies can be plugged in based on data domain and use case.
//!
//! ## Design Goals:
//!
//! 1. **Interface Segregation**: Each strategy implements only its selection logic
//! 2. **Domain-Specific**: Strategies optimized for different data types (ML, time series, sparse)
//! 3. **Type-Aware**: Strategies can make type-specific decisions (F32, I64, etc.)
//! 4. **Extensible**: New strategies can be added without modifying existing code
//!
//! ## Available Strategies:
//!
//! - `IntegerAnalysisStrategy`: Default strategy based on data pattern analysis (current behavior)
//! - `MlEmbeddingStrategy`: Optimized for ML embeddings (prefers Raw for F32)
//! - `TimeSeriesStrategy`: Optimized for time series data (prefers DoubleDelta, Gorilla)
//! - `SparseDataStrategy`: Optimized for sparse vectors (prefers SparseCOO, SparseBitmap)
//!
//! ## Usage:
//!
//! ```rust,ignore
//! let strategy = IntegerAnalysisStrategy::default();
//! let context = SelectionContext::for_ml_embeddings(TypeId::F32);
//! let analysis = DataAnalysis::from_f32_values(&values);
//!
//! let scheme = strategy.select(&analysis, &context);
//! ```

use serde::{Deserialize, Serialize};

use super::types::{ProximaScheme, TypeId};

/// Semantic authority mode for data being encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AuthorityMode {
    /// Canonical ProximaRecord payload; exact reconstruction is required.
    #[default]
    CanonicalRecord,
    /// Rebuildable projection derived from canonical records.
    RebuildableProjection,
    /// External asset is declared authoritative by catalog policy.
    ExternalAuthoritative,
}

/// Loss policy accepted for this encoding decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LossPolicy {
    /// Only byte-exact reconstruction is allowed.
    #[default]
    ExactOnly,
    /// The encoded data is a projection and may be approximate if cataloged.
    ProjectionMayBeLossy,
    /// The source is external-authoritative and follows its own precision contract.
    ExternalAuthoritative,
}

/// Workload profile used for CPU-vs-I/O tradeoffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WorkloadProfile {
    /// Point-heavy OLTP access.
    Oltp,
    /// Scan-heavy analytical access.
    Olap,
    /// Mixed point and scan access.
    #[default]
    Htap,
    /// ANN/rerank access over vector-bearing records.
    AnnRerank,
    /// Graph traversal access.
    GraphTraversal,
    /// Document/path scan access.
    DocumentScan,
}

/// Storage specialization for the column or projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StorageSpecialization {
    /// General record/table data.
    #[default]
    General,
    /// PAX row-family stripe.
    PaxStripe,
    /// Vector exact payload.
    VectorExact,
    /// Lossy vector projection/index payload.
    VectorProjection,
    /// Graph topology arrays such as CSR/CSC.
    GraphTopology,
    /// JSON/document structural stripe.
    JsonStructural,
    /// Time-series or observability stripe.
    TimeSeries,
}

/// Expected access temperature for a block or stripe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AccessTemperature {
    /// Frequently read or latency-sensitive data.
    Hot,
    /// Mixed access.
    #[default]
    Warm,
    /// Infrequently read data where CPU can be traded for I/O.
    Cold,
}

/// Random access granularity required by readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RandomAccessGranularity {
    /// Entire stripe/block decode is acceptable.
    #[default]
    Stripe,
    /// Row group decode is required.
    RowGroup,
    /// Individual value access is expected.
    Value,
}

/// Block access pattern for PAX stripe encoding decisions.
///
/// Tells the codec whether a column value is being written for a single-row
/// OLTP path, a full column stripe in an OLAP block, or the combined PAX layout
/// where both row directory and column stripes are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlockContext {
    /// Single-row OLTP point write — optimise for fast encode/decode latency.
    OltpRow,
    /// Full column stripe in an OLAP-only block — optimise for compression ratio.
    OlapStripe,
    /// Column stripe in a PAX block — balance latency and compression.
    #[default]
    PaxStripe,
}

/// Data domain identifier for context-aware selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DataDomain {
    /// Machine learning embeddings (typically F32, high entropy)
    MlEmbeddings,
    /// Time series data (typically sequential, compressible)
    TimeSeries,
    /// Sparse vectors (many zeros)
    Sparse,
    /// Metadata columns (mixed types, variable patterns)
    Metadata,
    /// General purpose (no specific optimization)
    #[default]
    General,
}

/// High-level modality of a column or projection payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColumnModality {
    /// Generic scalar column.
    #[default]
    Scalar,
    /// Exact vector payload.
    VectorExact,
    /// Vector projection/index payload.
    VectorProjection,
    /// Graph adjacency/topology payload.
    GraphTopology,
    /// JSON/document path or structural payload.
    JsonStructural,
    /// Time-series value or timestamp payload.
    TimeSeries,
    /// Metadata/system column.
    Metadata,
}

/// Physical ordering already applied or intended for the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PhysicalOrdering {
    /// No useful order known.
    #[default]
    None,
    /// Ordered by primary key or row id.
    PrimaryKey,
    /// Ordered by time.
    Time,
    /// Ordered by value or by a correlated value.
    Value,
    /// Ordered by reduced vector spatial key.
    VectorSpatial,
    /// Ordered by graph vertex remapping.
    GraphVertex,
    /// Ordered by JSON shape/path dictionary.
    JsonShape,
}

/// Coarse sortedness/correlation hint for a stripe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sortedness {
    /// No ordering guarantee.
    #[default]
    Unsorted,
    /// Non-decreasing order.
    Sorted,
    /// Values are clustered or serially correlated but not strictly sorted.
    Correlated,
}

/// Scope for dictionaries and learned/base parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DictionaryScope {
    /// No dictionary or external parameter scope.
    #[default]
    None,
    /// Parameters are local to the current stripe/block.
    Block,
    /// Parameters span a segment.
    Segment,
    /// Parameters are catalog-managed and versioned.
    Catalog,
}

/// Identifier for correlated columns/stripes that should be optimized together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationGroupId(pub u32);

/// Vector-specific layout hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorLayoutHint {
    /// Embedding dimension, if known.
    pub dimension: Option<u16>,
    /// Model identifier or cataloged embedding profile.
    pub model_id: Option<String>,
    /// Physical ordering family.
    pub ordering: PhysicalOrdering,
    /// Whether exact f32/f64 payload reconstruction is required.
    pub exact_payload: bool,
}

impl Default for VectorLayoutHint {
    fn default() -> Self {
        Self {
            dimension: None,
            model_id: None,
            ordering: PhysicalOrdering::None,
            exact_payload: true,
        }
    }
}

/// Graph-specific layout hint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GraphLayoutHint {
    /// Edge label or topology family, if known.
    pub edge_label: Option<String>,
    /// Whether neighbor lists are sorted.
    pub neighbor_sorted: bool,
    /// Whether vertex ids were remapped for locality.
    pub vertex_remapped: bool,
}

/// JSON/document-specific layout hint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JsonLayoutHint {
    /// Stable shape id for common structural variants.
    pub shape_id: Option<u32>,
    /// JSON path dictionary id.
    pub path_dictionary_id: Option<u32>,
    /// Whether the column is a typed leaf stripe instead of the full props blob.
    pub typed_leaf: bool,
}

/// Layout hints supplied to codec selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutHints {
    /// Column/projection modality.
    pub modality: ColumnModality,
    /// Physical ordering applied to the values.
    pub physical_ordering: PhysicalOrdering,
    /// Optional correlation group.
    pub correlation_group: Option<CorrelationGroupId>,
    /// Vector-specific hints.
    pub vector_layout: Option<VectorLayoutHint>,
    /// Graph-specific hints.
    pub graph_layout: Option<GraphLayoutHint>,
    /// JSON-specific hints.
    pub json_layout: Option<JsonLayoutHint>,
    /// Dictionary/parameter scope.
    pub dictionary_scope: DictionaryScope,
    /// Sortedness/correlation of the physical order.
    pub sortedness: Sortedness,
}

impl Default for LayoutHints {
    fn default() -> Self {
        Self {
            modality: ColumnModality::Scalar,
            physical_ordering: PhysicalOrdering::None,
            correlation_group: None,
            vector_layout: None,
            graph_layout: None,
            json_layout: None,
            dictionary_scope: DictionaryScope::None,
            sortedness: Sortedness::Unsorted,
        }
    }
}

impl LayoutHints {
    /// No layout hints.
    pub fn none() -> Self {
        Self::default()
    }

    /// Values are physically sorted.
    pub fn value_sorted() -> Self {
        Self {
            physical_ordering: PhysicalOrdering::Value,
            sortedness: Sortedness::Sorted,
            ..Self::default()
        }
    }

    /// Values are physically co-located by vector spatial key.
    pub fn vector_spatial() -> Self {
        Self {
            modality: ColumnModality::VectorExact,
            physical_ordering: PhysicalOrdering::VectorSpatial,
            sortedness: Sortedness::Correlated,
            vector_layout: Some(VectorLayoutHint {
                ordering: PhysicalOrdering::VectorSpatial,
                exact_payload: true,
                ..VectorLayoutHint::default()
            }),
            ..Self::default()
        }
    }
}

/// Canonical profile for one codec-selection decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionProfile {
    /// Semantic authority mode.
    pub authority_mode: AuthorityMode,
    /// Loss policy.
    pub loss_policy: LossPolicy,
    /// Workload profile.
    pub workload_profile: WorkloadProfile,
    /// Storage specialization.
    pub storage_specialization: StorageSpecialization,
    /// Access temperature.
    pub hotness: AccessTemperature,
    /// Required random-access granularity.
    pub random_access_granularity: RandomAccessGranularity,
    /// Target compression ratio. Values are uncompressed/compressed.
    pub target_compression_ratio: Option<f32>,
    /// Target decode budget per value.
    pub target_decode_ns_per_value: Option<u64>,
    /// Maximum allowed encode CPU per block.
    pub max_encode_cpu_ms_per_block: Option<u64>,
    /// Block context.
    pub block_context: BlockContext,
}

impl Default for CompressionProfile {
    fn default() -> Self {
        Self {
            authority_mode: AuthorityMode::CanonicalRecord,
            loss_policy: LossPolicy::ExactOnly,
            workload_profile: WorkloadProfile::Htap,
            storage_specialization: StorageSpecialization::General,
            hotness: AccessTemperature::Warm,
            random_access_granularity: RandomAccessGranularity::Stripe,
            target_compression_ratio: None,
            target_decode_ns_per_value: None,
            max_encode_cpu_ms_per_block: None,
            block_context: BlockContext::PaxStripe,
        }
    }
}

impl CompressionProfile {
    /// Profile for latency-sensitive hot reads/writes.
    pub fn hot_oltp() -> Self {
        Self {
            workload_profile: WorkloadProfile::Oltp,
            hotness: AccessTemperature::Hot,
            random_access_granularity: RandomAccessGranularity::Value,
            block_context: BlockContext::OltpRow,
            ..Self::default()
        }
    }

    /// Profile for cold analytical stripes.
    pub fn cold_olap(target_compression_ratio: f32) -> Self {
        Self {
            workload_profile: WorkloadProfile::Olap,
            hotness: AccessTemperature::Cold,
            random_access_granularity: RandomAccessGranularity::Stripe,
            target_compression_ratio: Some(target_compression_ratio),
            block_context: BlockContext::OlapStripe,
            storage_specialization: StorageSpecialization::PaxStripe,
            ..Self::default()
        }
    }

    /// Build a profile from the legacy selection context.
    pub fn from_selection_context(context: &SelectionContext) -> Self {
        let block_context = context.block_context.unwrap_or(BlockContext::PaxStripe);
        let (workload_profile, hotness, random_access_granularity) = match block_context {
            BlockContext::OltpRow => (
                WorkloadProfile::Oltp,
                AccessTemperature::Hot,
                RandomAccessGranularity::Value,
            ),
            BlockContext::OlapStripe => (
                WorkloadProfile::Olap,
                AccessTemperature::Cold,
                RandomAccessGranularity::Stripe,
            ),
            BlockContext::PaxStripe => (
                WorkloadProfile::Htap,
                AccessTemperature::Warm,
                RandomAccessGranularity::RowGroup,
            ),
        };
        let storage_specialization = match context.domain {
            DataDomain::MlEmbeddings => StorageSpecialization::VectorExact,
            DataDomain::TimeSeries => StorageSpecialization::TimeSeries,
            DataDomain::Sparse | DataDomain::Metadata | DataDomain::General => {
                StorageSpecialization::PaxStripe
            }
        };

        Self {
            authority_mode: AuthorityMode::CanonicalRecord,
            loss_policy: if context.allow_lossy {
                LossPolicy::ProjectionMayBeLossy
            } else {
                LossPolicy::ExactOnly
            },
            workload_profile,
            storage_specialization,
            hotness,
            random_access_granularity,
            target_compression_ratio: context.target_compression,
            target_decode_ns_per_value: None,
            max_encode_cpu_ms_per_block: None,
            block_context,
        }
    }
}

/// Parameter metadata attached to a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecParameters {
    /// Scope for any external parameters.
    pub dictionary_scope: DictionaryScope,
    /// Stable layout parameter id, when cataloged.
    pub layout_parameter_id: Option<u64>,
    /// Stable codec parameter id, when cataloged.
    pub codec_parameter_id: Option<u64>,
}

impl Default for CodecParameters {
    fn default() -> Self {
        Self {
            dictionary_scope: DictionaryScope::None,
            layout_parameter_id: None,
            codec_parameter_id: None,
        }
    }
}

/// Why a codec candidate was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectionReason {
    /// Candidate is lossy but exact reconstruction was required.
    LossyRejected,
    /// Candidate is too CPU-heavy for the hot path.
    HotPathDecodeBudget,
    /// Candidate did not meet the configured compression threshold.
    CompressionTargetMiss,
    /// Candidate is incompatible with the declared modality/layout.
    LayoutMismatch,
}

/// Rejected codec candidate with reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectedCodecCandidate {
    /// Rejected scheme.
    pub scheme: ProximaScheme,
    /// Reason it was rejected.
    pub reason: RejectionReason,
    /// Estimated compression ratio, if known.
    pub expected_ratio: Option<f32>,
}

/// Explainable codec-selection result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodecDecision {
    /// Selected scheme.
    pub scheme: ProximaScheme,
    /// Selected parameter metadata.
    pub parameters: CodecParameters,
    /// Estimated compression ratio. Values are uncompressed/compressed.
    pub expected_ratio: f32,
    /// Estimated decode budget per value.
    pub expected_decode_ns_per_value: Option<u64>,
    /// Whether exact reconstruction is guaranteed for the input type.
    pub exact_reconstruction: bool,
    /// Rejected alternatives and reasons.
    pub rejected_candidates: Vec<RejectedCodecCandidate>,
}

/// Context for codec selection decisions
#[derive(Debug, Clone)]
pub struct SelectionContext {
    /// The data type being encoded
    pub data_type: TypeId,
    /// The domain of the data
    pub domain: DataDomain,
    /// Target compression ratio (None = optimize for speed)
    pub target_compression: Option<f32>,
    /// Whether lossy encoding is allowed
    pub allow_lossy: bool,
    /// Hint for expected value range (if known)
    pub expected_range: Option<(i64, i64)>,
    /// Whether data is already sorted
    pub is_sorted: bool,
    /// PAX block access pattern — drives latency-vs-compression trade-off.
    /// `None` means the codec is used outside a PAX block context.
    pub block_context: Option<BlockContext>,
}

impl SelectionContext {
    /// Create context for ML embeddings (F32 vectors)
    pub fn for_ml_embeddings(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::MlEmbeddings,
            target_compression: None, // Speed over compression
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: None,
        }
    }

    /// Create context for time series data
    pub fn for_time_series(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::TimeSeries,
            target_compression: Some(5.0), // Target 5x compression
            allow_lossy: false,
            expected_range: None,
            is_sorted: true, // Time series is typically sorted by time
            block_context: None,
        }
    }

    /// Create context for sparse data
    pub fn for_sparse(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::Sparse,
            target_compression: Some(10.0), // Sparse data should compress well
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: None,
        }
    }

    /// Create context for metadata columns
    pub fn for_metadata(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::Metadata,
            target_compression: Some(3.0),
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: None,
        }
    }

    /// Create default context for general purpose
    pub fn general(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::General,
            target_compression: None,
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: None,
        }
    }

    /// Create context for a PAX column stripe.
    ///
    /// Balances latency and compression. Use for the default `Pax` block mode.
    pub fn for_pax_stripe(data_type: TypeId, domain: DataDomain) -> Self {
        Self {
            data_type,
            domain,
            target_compression: Some(3.0),
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: Some(BlockContext::PaxStripe),
        }
    }

    /// Create context for a pure OLAP column stripe.
    ///
    /// Maximises compression — latency is less critical for batch scan paths.
    pub fn for_olap_stripe(data_type: TypeId, domain: DataDomain) -> Self {
        Self {
            data_type,
            domain,
            target_compression: Some(6.0),
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: Some(BlockContext::OlapStripe),
        }
    }

    /// Create context for a single OLTP row value.
    ///
    /// Minimises encode/decode latency; compression ratio is not a priority.
    pub fn for_oltp_row(data_type: TypeId) -> Self {
        Self {
            data_type,
            domain: DataDomain::General,
            target_compression: None,
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: Some(BlockContext::OltpRow),
        }
    }

    /// Set target compression ratio
    pub fn with_compression(mut self, ratio: f32) -> Self {
        self.target_compression = Some(ratio);
        self
    }

    /// Allow lossy encoding
    pub fn allow_lossy(mut self) -> Self {
        self.allow_lossy = true;
        self
    }

    /// Set expected value range
    pub fn with_range(mut self, min: i64, max: i64) -> Self {
        self.expected_range = Some((min, max));
        self
    }

    /// Mark data as sorted
    pub fn sorted(mut self) -> Self {
        self.is_sorted = true;
        self
    }

    /// Convert legacy context into the canonical compression profile.
    pub fn compression_profile(&self) -> CompressionProfile {
        CompressionProfile::from_selection_context(self)
    }

    /// Convert legacy context into layout hints.
    pub fn layout_hints(&self) -> LayoutHints {
        let modality = match self.domain {
            DataDomain::MlEmbeddings => ColumnModality::VectorExact,
            DataDomain::TimeSeries => ColumnModality::TimeSeries,
            DataDomain::Sparse | DataDomain::Metadata | DataDomain::General => {
                ColumnModality::Scalar
            }
        };
        let physical_ordering = if self.is_sorted {
            PhysicalOrdering::Value
        } else {
            PhysicalOrdering::None
        };
        let sortedness = if self.is_sorted {
            Sortedness::Sorted
        } else {
            Sortedness::Unsorted
        };

        LayoutHints {
            modality,
            physical_ordering,
            sortedness,
            ..LayoutHints::default()
        }
    }
}

/// Analysis of data patterns for informed selection
#[derive(Debug, Clone)]
pub struct DataAnalysis {
    /// Ratio of zero values (0.0 - 1.0)
    pub zero_ratio: f64,
    /// Ratio of unique values to total values (0.0 - 1.0)
    pub unique_ratio: f64,
    /// Score for sequential pattern (0.0 - 1.0)
    pub sequential_score: f64,
    /// Range of values (max - min)
    pub range: u64,
    /// Maximum bits needed to represent the range
    pub max_bits: u8,
    /// Score for constant data (1.0 if all same, 0.0 otherwise)
    pub constant_score: f64,
    /// Number of values analyzed
    pub count: usize,
    /// Minimum value (if applicable)
    pub min_value: Option<i64>,
    /// Maximum value (if applicable)
    pub max_value: Option<i64>,
}

impl DataAnalysis {
    /// Create an empty analysis
    pub fn empty() -> Self {
        Self {
            zero_ratio: 0.0,
            unique_ratio: 0.0,
            sequential_score: 0.0,
            range: 0,
            max_bits: 0,
            constant_score: 0.0,
            count: 0,
            min_value: None,
            max_value: None,
        }
    }

    /// Analyze I64 values
    pub fn from_i64_values(values: &[i64]) -> Self {
        if values.is_empty() {
            return Self::empty();
        }

        let len = values.len() as f64;

        // Count zeros
        let zero_count = values.iter().filter(|&&v| v == 0).count() as f64;
        let zero_ratio = zero_count / len;

        // Count unique values
        let mut unique = std::collections::HashSet::new();
        for &v in values {
            unique.insert(v);
        }
        let unique_ratio = unique.len() as f64 / len;

        // Check if constant
        let constant_score = if unique.len() == 1 { 1.0 } else { 0.0 };

        // Check sequential pattern
        let mut sequential_count = 0;
        for i in 1..values.len() {
            let diff = values[i].wrapping_sub(values[i - 1]);
            if diff.abs() <= 2 {
                sequential_count += 1;
            }
        }
        let sequential_score = if values.len() > 1 {
            sequential_count as f64 / (values.len() - 1) as f64
        } else {
            0.0
        };

        // Find range and max bits
        let min = values.iter().min().copied().unwrap_or(0);
        let max = values.iter().max().copied().unwrap_or(0);
        let range = (max - min) as u64;
        let max_bits = if range == 0 {
            1
        } else {
            64 - range.leading_zeros() as u8
        };

        Self {
            zero_ratio,
            unique_ratio,
            sequential_score,
            range,
            max_bits,
            constant_score,
            count: values.len(),
            min_value: Some(min),
            max_value: Some(max),
        }
    }

    /// Analyze I32 values
    pub fn from_i32_values(values: &[i32]) -> Self {
        let i64_values: Vec<i64> = values.iter().map(|&v| v as i64).collect();
        Self::from_i64_values(&i64_values)
    }

    /// Analyze F32 values (as bit patterns)
    pub fn from_f32_values(values: &[f32]) -> Self {
        let i64_values: Vec<i64> = values.iter().map(|&v| v.to_bits() as i64).collect();
        Self::from_i64_values(&i64_values)
    }

    /// Analyze F64 values (as bit patterns)
    pub fn from_f64_values(values: &[f64]) -> Self {
        let i64_values: Vec<i64> = values.iter().map(|&v| v.to_bits() as i64).collect();
        Self::from_i64_values(&i64_values)
    }

    /// Check if data is highly sparse (>70% zeros)
    pub fn is_sparse(&self) -> bool {
        self.zero_ratio > 0.70
    }

    /// Check if data is very sparse (>95% zeros)
    pub fn is_very_sparse(&self) -> bool {
        self.zero_ratio > 0.95
    }

    /// Check if data is constant (all same value)
    pub fn is_constant(&self) -> bool {
        self.constant_score > 0.9
    }

    /// Check if data is sequential
    pub fn is_sequential(&self) -> bool {
        self.sequential_score > 0.80
    }

    /// Check if data has low cardinality (<10% unique)
    pub fn is_low_cardinality(&self) -> bool {
        self.unique_ratio < 0.10
    }
}

fn type_bits(type_id: TypeId) -> u8 {
    (type_id.size_bytes() * 8) as u8
}

fn decode_estimate_ns_per_value(scheme: &ProximaScheme) -> u64 {
    match scheme {
        ProximaScheme::Raw => 1,
        ProximaScheme::Delta { .. } | ProximaScheme::BitPacked { .. } => 2,
        ProximaScheme::FrameOfReference { .. } | ProximaScheme::DoubleDelta { .. } => 3,
        ProximaScheme::Simple8b | ProximaScheme::VByte => 4,
        ProximaScheme::SparseBitmap | ProximaScheme::RunLength => 5,
        ProximaScheme::Dictionary | ProximaScheme::SparseCOO => 7,
        ProximaScheme::PForDelta { .. } | ProximaScheme::PForDoubleDelta { .. } => 8,
        ProximaScheme::Zigzag { .. } => 3,
        ProximaScheme::Gorilla => 9,
        // SQ8 decode is a single affine multiply-add per value (cheap, ~Raw+1).
        ProximaScheme::Sq8 => 2,
        // RaBitQ candidate scan is a sign-weighted add per dim (no multiply).
        ProximaScheme::RaBitQ => 2,
        ProximaScheme::Adaptive => 10,
    }
}

fn is_hot_path_heavy(scheme: &ProximaScheme) -> bool {
    matches!(
        scheme,
        ProximaScheme::Dictionary
            | ProximaScheme::SparseCOO
            | ProximaScheme::PForDelta { .. }
            | ProximaScheme::PForDoubleDelta { .. }
            | ProximaScheme::Gorilla
            | ProximaScheme::Adaptive
    )
}

fn is_high_value_fast_codec(scheme: &ProximaScheme, analysis: &DataAnalysis) -> bool {
    matches!(scheme, ProximaScheme::RunLength)
        || (analysis.is_sparse() && matches!(scheme, ProximaScheme::SparseBitmap))
        || (analysis.is_sequential()
            && matches!(
                scheme,
                ProximaScheme::Delta { .. } | ProximaScheme::DoubleDelta { .. }
            ))
}

fn lossless_fallback_for(
    type_id: TypeId,
    analysis: &DataAnalysis,
    hints: &LayoutHints,
) -> ProximaScheme {
    if hints.sortedness == Sortedness::Sorted || analysis.is_sequential() {
        ProximaScheme::Delta {
            base: analysis.min_value.unwrap_or(0),
        }
    } else if analysis.range > 0
        && analysis.range < (1u64 << 32)
        && !matches!(type_id, TypeId::F32 | TypeId::F64)
    {
        ProximaScheme::FrameOfReference {
            reference: analysis.min_value.unwrap_or(0),
            bits: analysis.max_bits.max(1),
        }
    } else {
        ProximaScheme::Raw
    }
}

fn ordering_preferred_scheme(
    analysis: &DataAnalysis,
    context: &SelectionContext,
    hints: &LayoutHints,
) -> Option<ProximaScheme> {
    match (hints.physical_ordering, hints.sortedness) {
        (PhysicalOrdering::Time, _) | (_, Sortedness::Sorted) if analysis.is_sequential() => {
            Some(ProximaScheme::DoubleDelta {
                first_value: analysis.min_value.unwrap_or(0),
                first_delta: 1,
            })
        }
        (PhysicalOrdering::Value | PhysicalOrdering::PrimaryKey | PhysicalOrdering::Time, _)
        | (_, Sortedness::Sorted) => Some(ProximaScheme::Delta {
            base: analysis.min_value.unwrap_or(0),
        }),
        (_, Sortedness::Correlated)
            if context.data_type != TypeId::F32 && context.data_type != TypeId::F64 =>
        {
            Some(ProximaScheme::FrameOfReference {
                reference: analysis.min_value.unwrap_or(0),
                bits: analysis.max_bits.max(1),
            })
        }
        _ => None,
    }
}

fn estimate_compression_ratio(
    scheme: &ProximaScheme,
    analysis: &DataAnalysis,
    type_id: TypeId,
) -> f32 {
    match scheme {
        ProximaScheme::Raw => 1.0,
        ProximaScheme::RunLength => {
            if analysis.is_constant() {
                analysis.count.clamp(1, 256) as f32
            } else {
                1.2
            }
        }
        ProximaScheme::SparseCOO => {
            if analysis.zero_ratio > 0.95 {
                8.0
            } else {
                1.4
            }
        }
        ProximaScheme::SparseBitmap => {
            if analysis.zero_ratio > 0.70 {
                3.0
            } else {
                1.2
            }
        }
        ProximaScheme::Dictionary => {
            if analysis.unique_ratio < 0.10 {
                (1.0 / analysis.unique_ratio.max(0.01) as f32).min(8.0)
            } else {
                1.1
            }
        }
        ProximaScheme::Delta { .. } => {
            if analysis.is_sequential() {
                2.0
            } else {
                1.2
            }
        }
        ProximaScheme::DoubleDelta { .. } => {
            if analysis.is_sequential() {
                3.0
            } else {
                1.3
            }
        }
        ProximaScheme::FrameOfReference { bits, .. } => {
            let full_bits = type_bits(type_id).max(1) as f32;
            (full_bits / (*bits).max(1) as f32).max(1.0)
        }
        ProximaScheme::BitPacked { bits } => {
            let full_bits = type_bits(type_id).max(1) as f32;
            (full_bits / (*bits).max(1) as f32).max(1.0)
        }
        ProximaScheme::PForDelta { majority_bits, .. } => {
            let full_bits = type_bits(type_id).max(1) as f32;
            (full_bits / (*majority_bits).max(1) as f32).max(1.0) * 1.2
        }
        ProximaScheme::PForDoubleDelta { .. } => 2.5,
        ProximaScheme::Simple8b => 1.8,
        ProximaScheme::VByte => 1.5,
        ProximaScheme::Zigzag { bits } => {
            let full_bits = type_bits(type_id).max(1) as f32;
            (full_bits / (*bits).max(1) as f32).max(1.0)
        }
        ProximaScheme::Gorilla => 2.0,
        // SQ8 stores 1 byte/value; ratio vs the source width (4.0 for f32).
        ProximaScheme::Sq8 => (type_bits(type_id).max(1) as f32 / 8.0).max(1.0),
        // RaBitQ stores ~1 bit/value; ratio vs source width (~32× for f32).
        ProximaScheme::RaBitQ => (type_bits(type_id).max(1) as f32).max(1.0),
        ProximaScheme::Adaptive => 1.0,
    }
}

/// Codec selection strategy trait
///
/// Different strategies can be implemented for different data domains.
/// Each strategy receives data analysis and context, and returns the
/// optimal encoding scheme.
pub trait CodecSelectionStrategy: Send + Sync {
    /// Strategy name for logging/debugging
    fn name(&self) -> &'static str;

    /// Select the optimal encoding scheme based on analysis and context
    fn select(&self, analysis: &DataAnalysis, context: &SelectionContext) -> ProximaScheme;

    /// Whether this strategy supports a given data type
    fn supports_type(&self, type_id: TypeId) -> bool {
        // Default: support all types
        match type_id {
            TypeId::F32 | TypeId::F64 | TypeId::I64 | TypeId::I32 | TypeId::U64 | TypeId::U32 => {
                true
            }
        }
    }

    /// Optional: estimate compression ratio for given scheme
    fn estimate_compression(&self, _scheme: &ProximaScheme, _analysis: &DataAnalysis) -> f32 {
        1.0 // Default: no compression estimate
    }
}

// ============================================================================
// Standard Strategy Implementations
// ============================================================================

/// Integer Analysis Strategy (Default)
///
/// Analyzes data patterns and selects scheme based on:
/// - Constant data → RunLength
/// - Sparse data → SparseCOO/SparseBitmap
/// - Low cardinality → Dictionary
/// - Sequential → DoubleDelta
/// - Small range → Simple8b/VByte
/// - Default → Delta
pub struct IntegerAnalysisStrategy;

impl Default for IntegerAnalysisStrategy {
    fn default() -> Self {
        Self
    }
}

impl CodecSelectionStrategy for IntegerAnalysisStrategy {
    fn name(&self) -> &'static str {
        "IntegerAnalysis"
    }

    fn select(&self, analysis: &DataAnalysis, _context: &SelectionContext) -> ProximaScheme {
        // Constant data → RunLength
        if analysis.is_constant() {
            return ProximaScheme::RunLength;
        }

        // Very sparse (>95% zeros) → SparseCOO
        if analysis.is_very_sparse() {
            return ProximaScheme::SparseCOO;
        }

        // Sparse (70-95% zeros) → SparseBitmap
        if analysis.is_sparse() {
            return ProximaScheme::SparseBitmap;
        }

        // Low cardinality (<10% unique) → Dictionary
        if analysis.is_low_cardinality() {
            return ProximaScheme::Dictionary;
        }

        // Sequential data → DoubleDelta
        if analysis.is_sequential() {
            let first_value = analysis.min_value.unwrap_or(0);
            return ProximaScheme::DoubleDelta {
                first_value,
                first_delta: 1, // Assume step of 1 for sequential
            };
        }

        // Small range values → Simple8b
        if analysis.max_bits <= 20 && analysis.range < 1_000_000 {
            return ProximaScheme::Simple8b;
        }

        // Small values → VByte
        if analysis.max_bits <= 14 {
            return ProximaScheme::VByte;
        }

        // Medium range → FrameOfReference
        if analysis.range < (1u64 << 32) {
            let reference = analysis.min_value.unwrap_or(0);
            let bits = analysis.max_bits;
            return ProximaScheme::FrameOfReference { reference, bits };
        }

        // Default: Delta encoding
        ProximaScheme::Delta { base: 0 }
    }
}

/// ML Embedding Strategy
///
/// Optimized for machine learning embeddings (F32 vectors):
/// - Prefers Raw encoding for F32 (preserves precision)
/// - Falls back to Delta for integer types
pub struct MlEmbeddingStrategy;

impl Default for MlEmbeddingStrategy {
    fn default() -> Self {
        Self
    }
}

impl CodecSelectionStrategy for MlEmbeddingStrategy {
    fn name(&self) -> &'static str {
        "MlEmbedding"
    }

    fn select(&self, _analysis: &DataAnalysis, context: &SelectionContext) -> ProximaScheme {
        // For F32/F64, use Raw encoding (no compression, preserves precision)
        match context.data_type {
            TypeId::F32 | TypeId::F64 => ProximaScheme::Raw,
            // For integer types in ML context (e.g., quantized embeddings), use Delta
            _ => ProximaScheme::Delta { base: 0 },
        }
    }

    fn supports_type(&self, _type_id: TypeId) -> bool {
        // Primarily for floating point, but supports all
        true
    }
}

/// Time Series Strategy
///
/// Optimized for time series data:
/// - Sequential data → DoubleDelta (best for timestamps)
/// - Float data → Gorilla encoding (if available)
/// - Default → Delta
pub struct TimeSeriesStrategy;

impl Default for TimeSeriesStrategy {
    fn default() -> Self {
        Self
    }
}

impl CodecSelectionStrategy for TimeSeriesStrategy {
    fn name(&self) -> &'static str {
        "TimeSeries"
    }

    fn select(&self, analysis: &DataAnalysis, context: &SelectionContext) -> ProximaScheme {
        // Time series is typically sequential
        if analysis.is_sequential() || context.is_sorted {
            let first_value = analysis.min_value.unwrap_or(0);
            return ProximaScheme::DoubleDelta {
                first_value,
                first_delta: 1,
            };
        }

        // For float time series, use Gorilla if available
        if matches!(context.data_type, TypeId::F32 | TypeId::F64) {
            return ProximaScheme::Gorilla;
        }

        // Default: Delta
        ProximaScheme::Delta { base: 0 }
    }
}

/// Sparse Data Strategy
///
/// Optimized for sparse vectors:
/// - Very sparse (>95%) → SparseCOO
/// - Sparse (70-95%) → SparseBitmap
/// - Else → Dictionary or Delta
pub struct SparseDataStrategy;

impl Default for SparseDataStrategy {
    fn default() -> Self {
        Self
    }
}

impl CodecSelectionStrategy for SparseDataStrategy {
    fn name(&self) -> &'static str {
        "SparseData"
    }

    fn select(&self, analysis: &DataAnalysis, _context: &SelectionContext) -> ProximaScheme {
        if analysis.is_very_sparse() {
            ProximaScheme::SparseCOO
        } else if analysis.is_sparse() {
            ProximaScheme::SparseBitmap
        } else if analysis.is_low_cardinality() {
            ProximaScheme::Dictionary
        } else {
            ProximaScheme::Delta { base: 0 }
        }
    }
}

/// Strategy registry for managing multiple strategies
pub struct StrategyRegistry {
    strategies: Vec<(DataDomain, Box<dyn CodecSelectionStrategy>)>,
    default_strategy: Box<dyn CodecSelectionStrategy>,
}

impl StrategyRegistry {
    /// Create a new registry with default strategy
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
            default_strategy: Box::new(IntegerAnalysisStrategy),
        }
    }

    /// Register a strategy for a specific domain
    pub fn register(
        mut self,
        domain: DataDomain,
        strategy: Box<dyn CodecSelectionStrategy>,
    ) -> Self {
        self.strategies.push((domain, strategy));
        self
    }

    /// Set the default strategy
    pub fn with_default(mut self, strategy: Box<dyn CodecSelectionStrategy>) -> Self {
        self.default_strategy = strategy;
        self
    }

    /// Get strategy for a given context
    pub fn get_strategy(&self, domain: DataDomain) -> &dyn CodecSelectionStrategy {
        for (d, s) in &self.strategies {
            if *d == domain {
                return s.as_ref();
            }
        }
        self.default_strategy.as_ref()
    }

    /// Select scheme using appropriate strategy
    pub fn select(&self, analysis: &DataAnalysis, context: &SelectionContext) -> ProximaScheme {
        let strategy = self.get_strategy(context.domain);
        strategy.select(analysis, context)
    }

    /// Select an explainable codec decision using the canonical compression profile.
    ///
    /// This is the forward-compatible selection entry point for PAX writers,
    /// graph/JSON/vector projections, and future EXPLAIN output. `select()` is
    /// retained as the legacy scheme-only API.
    pub fn select_decision(
        &self,
        analysis: &DataAnalysis,
        context: &SelectionContext,
        profile: &CompressionProfile,
        hints: &LayoutHints,
    ) -> CodecDecision {
        let mut rejected_candidates = Vec::new();
        let strategy = self.get_strategy(context.domain);
        let mut scheme = strategy.select(analysis, context);

        if !analysis.is_constant()
            && let Some(ordering_scheme) = ordering_preferred_scheme(analysis, context, hints)
            && ordering_scheme != scheme
        {
            rejected_candidates.push(RejectedCodecCandidate {
                scheme: scheme.clone(),
                reason: RejectionReason::LayoutMismatch,
                expected_ratio: Some(estimate_compression_ratio(
                    &scheme,
                    analysis,
                    context.data_type,
                )),
            });
            scheme = ordering_scheme;
        }

        if profile.loss_policy == LossPolicy::ExactOnly && scheme.is_lossy(context.data_type) {
            rejected_candidates.push(RejectedCodecCandidate {
                scheme: scheme.clone(),
                reason: RejectionReason::LossyRejected,
                expected_ratio: Some(estimate_compression_ratio(
                    &scheme,
                    analysis,
                    context.data_type,
                )),
            });
            scheme = lossless_fallback_for(context.data_type, analysis, hints);
        }

        if profile.hotness == AccessTemperature::Hot
            && is_hot_path_heavy(&scheme)
            && !is_high_value_fast_codec(&scheme, analysis)
        {
            rejected_candidates.push(RejectedCodecCandidate {
                scheme: scheme.clone(),
                reason: RejectionReason::HotPathDecodeBudget,
                expected_ratio: Some(estimate_compression_ratio(
                    &scheme,
                    analysis,
                    context.data_type,
                )),
            });
            scheme = ProximaScheme::Raw;
        }

        let mut expected_ratio = estimate_compression_ratio(&scheme, analysis, context.data_type);
        if let Some(target_ratio) = profile.target_compression_ratio
            && target_ratio > 1.0
            && expected_ratio < target_ratio
            && !matches!(scheme, ProximaScheme::Raw)
        {
            rejected_candidates.push(RejectedCodecCandidate {
                scheme: scheme.clone(),
                reason: RejectionReason::CompressionTargetMiss,
                expected_ratio: Some(expected_ratio),
            });
            scheme = ProximaScheme::Raw;
            expected_ratio = 1.0;
        }

        CodecDecision {
            exact_reconstruction: !scheme.is_lossy(context.data_type),
            expected_decode_ns_per_value: Some(decode_estimate_ns_per_value(&scheme)),
            parameters: CodecParameters {
                dictionary_scope: hints.dictionary_scope,
                ..CodecParameters::default()
            },
            scheme,
            expected_ratio,
            rejected_candidates,
        }
    }

    /// Select an explainable decision by deriving the profile and layout hints
    /// from the legacy `SelectionContext`.
    pub fn select_decision_from_context(
        &self,
        analysis: &DataAnalysis,
        context: &SelectionContext,
    ) -> CodecDecision {
        let profile = context.compression_profile();
        let hints = context.layout_hints();
        self.select_decision(analysis, context, &profile, &hints)
    }
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
            .register(DataDomain::MlEmbeddings, Box::new(MlEmbeddingStrategy))
            .register(DataDomain::TimeSeries, Box::new(TimeSeriesStrategy))
            .register(DataDomain::Sparse, Box::new(SparseDataStrategy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_analysis_empty() {
        let analysis = DataAnalysis::empty();
        assert_eq!(analysis.count, 0);
        assert!(!analysis.is_sparse());
        assert!(!analysis.is_constant());
    }

    #[test]
    fn test_data_analysis_constant() {
        let values = vec![42i64; 100];
        let analysis = DataAnalysis::from_i64_values(&values);

        assert!(analysis.is_constant());
        assert_eq!(analysis.unique_ratio, 0.01); // 1 unique out of 100
    }

    #[test]
    fn test_data_analysis_sparse() {
        let mut values = vec![0i64; 100];
        values[10] = 1;
        values[50] = 2;

        let analysis = DataAnalysis::from_i64_values(&values);
        assert!(analysis.is_very_sparse());
        assert!(analysis.zero_ratio > 0.95);
    }

    #[test]
    fn test_data_analysis_sequential() {
        let values: Vec<i64> = (0..100).collect();
        let analysis = DataAnalysis::from_i64_values(&values);

        assert!(analysis.is_sequential());
    }

    #[test]
    fn test_integer_analysis_strategy_constant() {
        let strategy = IntegerAnalysisStrategy;
        let values = vec![42i64; 100];
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);

        let scheme = strategy.select(&analysis, &context);
        assert!(matches!(scheme, ProximaScheme::RunLength));
    }

    #[test]
    fn test_integer_analysis_strategy_sparse() {
        let strategy = IntegerAnalysisStrategy;
        let mut values = vec![0i64; 100];
        values[10] = 1;

        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);

        let scheme = strategy.select(&analysis, &context);
        assert!(matches!(scheme, ProximaScheme::SparseCOO));
    }

    #[test]
    fn test_ml_embedding_strategy_f32() {
        let strategy = MlEmbeddingStrategy;
        let analysis = DataAnalysis::empty();
        let context = SelectionContext::for_ml_embeddings(TypeId::F32);

        let scheme = strategy.select(&analysis, &context);
        assert!(matches!(scheme, ProximaScheme::Raw));
    }

    #[test]
    fn test_time_series_strategy_sequential() {
        let strategy = TimeSeriesStrategy;
        let values: Vec<i64> = (0..100).collect();
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::for_time_series(TypeId::I64);

        let scheme = strategy.select(&analysis, &context);
        assert!(matches!(scheme, ProximaScheme::DoubleDelta { .. }));
    }

    #[test]
    fn test_strategy_registry() {
        let registry = StrategyRegistry::default();

        // ML embeddings should use MlEmbeddingStrategy
        let analysis = DataAnalysis::empty();
        let ml_context = SelectionContext::for_ml_embeddings(TypeId::F32);
        let scheme = registry.select(&analysis, &ml_context);
        assert!(matches!(scheme, ProximaScheme::Raw));

        // General should use IntegerAnalysisStrategy
        let general_context = SelectionContext::general(TypeId::I64);
        let sparse_values = vec![0i64; 100];
        let sparse_analysis = DataAnalysis::from_i64_values(&sparse_values);
        let scheme = registry.select(&sparse_analysis, &general_context);
        // Constant data (all zeros) -> RunLength
        assert!(matches!(scheme, ProximaScheme::RunLength));
    }

    #[test]
    fn test_selection_context_builders() {
        let ml = SelectionContext::for_ml_embeddings(TypeId::F32);
        assert_eq!(ml.domain, DataDomain::MlEmbeddings);
        assert!(!ml.allow_lossy);

        let ts = SelectionContext::for_time_series(TypeId::I64);
        assert_eq!(ts.domain, DataDomain::TimeSeries);
        assert!(ts.is_sorted);

        let sparse = SelectionContext::for_sparse(TypeId::I64);
        assert_eq!(sparse.domain, DataDomain::Sparse);
    }

    #[test]
    fn test_exact_only_rejects_lossy_float_codec() {
        let registry = StrategyRegistry::default();
        let values = vec![1.0f32, 2.5, 1.25, 4.75, 3.5];
        let analysis = DataAnalysis::from_f32_values(&values);
        let context = SelectionContext {
            data_type: TypeId::F32,
            domain: DataDomain::TimeSeries,
            target_compression: None,
            allow_lossy: false,
            expected_range: None,
            is_sorted: false,
            block_context: Some(BlockContext::PaxStripe),
        };
        let profile = CompressionProfile::default();
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::none());

        assert!(matches!(decision.scheme, ProximaScheme::Raw));
        assert!(decision.exact_reconstruction);
        assert!(decision.rejected_candidates.iter().any(|candidate| {
            matches!(candidate.scheme, ProximaScheme::Gorilla)
                && candidate.reason == RejectionReason::LossyRejected
        }));
    }

    #[test]
    fn test_hot_profile_prefers_raw_over_heavy_dictionary() {
        let registry = StrategyRegistry::default();
        let values: Vec<i64> = (0..200).map(|i| (i % 5) as i64).collect();
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);
        let profile = CompressionProfile::hot_oltp();
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::none());

        assert!(matches!(decision.scheme, ProximaScheme::Raw));
        assert!(decision.rejected_candidates.iter().any(|candidate| {
            matches!(candidate.scheme, ProximaScheme::Dictionary)
                && candidate.reason == RejectionReason::HotPathDecodeBudget
        }));
    }

    #[test]
    fn test_cold_profile_keeps_compressive_dictionary() {
        let registry = StrategyRegistry::default();
        let values: Vec<i64> = (0..200).map(|i| (i % 5) as i64).collect();
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);
        let profile = CompressionProfile::cold_olap(2.0);
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::none());

        assert!(matches!(decision.scheme, ProximaScheme::Dictionary));
        assert!(decision.expected_ratio >= 2.0);
        assert!(
            !decision
                .rejected_candidates
                .iter()
                .any(|candidate| candidate.reason == RejectionReason::HotPathDecodeBudget)
        );
    }

    #[test]
    fn test_value_ordering_prefers_delta_candidate() {
        let registry = StrategyRegistry::default();
        let values = vec![0i64, 10, 20, 30, 40, 50, 60, 70];
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);
        let profile = CompressionProfile::default();
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::value_sorted());

        assert!(matches!(decision.scheme, ProximaScheme::Delta { .. }));
        assert!(decision.rejected_candidates.iter().any(|candidate| {
            matches!(candidate.scheme, ProximaScheme::Simple8b)
                && candidate.reason == RejectionReason::LayoutMismatch
        }));
    }

    #[test]
    fn test_sorted_constant_decision_keeps_runlength() {
        let registry = StrategyRegistry::default();
        let values = vec![42i64; 128];
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64).sorted();
        let profile = CompressionProfile::default();
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::value_sorted());

        assert!(matches!(decision.scheme, ProximaScheme::RunLength));
        assert!(decision.rejected_candidates.is_empty());
    }

    #[test]
    fn test_unmet_compression_target_falls_back_to_raw() {
        let registry = StrategyRegistry::default();
        let values: Vec<i64> = (0..128).map(|i| i * 4).collect();
        let analysis = DataAnalysis::from_i64_values(&values);
        let context = SelectionContext::general(TypeId::I64);
        let profile = CompressionProfile::cold_olap(10.0);
        let decision =
            registry.select_decision(&analysis, &context, &profile, &LayoutHints::none());

        assert!(matches!(decision.scheme, ProximaScheme::Raw));
        assert_eq!(decision.expected_ratio, 1.0);
        assert!(
            decision
                .rejected_candidates
                .iter()
                .any(|candidate| candidate.reason == RejectionReason::CompressionTargetMiss)
        );
    }
}
