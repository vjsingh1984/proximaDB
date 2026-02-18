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

use super::types::{ProximaScheme, TypeId};

/// Data domain identifier for context-aware selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    General,
}

impl Default for DataDomain {
    fn default() -> Self {
        DataDomain::General
    }
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
}
