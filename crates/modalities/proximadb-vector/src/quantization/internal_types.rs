//! Internal quantization types (Release 1 - no legacy compatibility)
//!
//! These types are used internally for quantization operations.
//! The proto QuantizationConfig is simplified for user-facing API.

use proximadb_quantization_types::{
    DurableQuantState, QuantizationLifecycle, QuantizationMethod, QuantizationType,
};
use serde::{Deserialize, Serialize};

/// Unified quantization level configuration
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnifiedQuantizationLevel {
    pub level_type: Option<QuantizationLevel>,
}

/// Quantization level types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QuantizationLevel {
    None(NoQuantization),
    Uniform(UniformQuantization),
    Pq(ProductQuantization),
    Scalar(ScalarQuantization),
    Binary(BinaryQuantization),
    Custom(CustomQuantization),
    /// TurboQuant — data-oblivious scalar quantizer (ADR-021).
    /// See `TURBOQUANT_LLD_2026_05_30.adoc` §"Locked Type Signatures".
    #[cfg(feature = "experimental-turboquant")]
    TurboQuant(TurboQuantization),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NoQuantization {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UniformQuantization {
    pub bits: i32,
    pub scale: Option<f32>,
    pub offset: Option<f32>,
}

impl Eq for UniformQuantization {}

impl std::hash::Hash for UniformQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
        // Hash the bits representation of the floats for consistency
        self.scale.map(|s| s.to_bits()).hash(state);
        self.offset.map(|o| o.to_bits()).hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ProductQuantization {
    pub bits_per_code: i32,
    pub num_subvectors: i32,
    pub codebook_id: Option<String>,
    pub adaptive_subvectors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScalarQuantization {
    pub bits: i32,
    pub scale: f32,
    pub offset: f32,
    pub clamp_values: bool,
}

impl Eq for ScalarQuantization {}

impl std::hash::Hash for ScalarQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bits.hash(state);
        self.scale.to_bits().hash(state);
        self.offset.to_bits().hash(state);
        self.clamp_values.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BinaryQuantization {
    pub threshold: Option<f32>,
    pub sign_based: bool,
}

impl Eq for BinaryQuantization {}

impl std::hash::Hash for BinaryQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.threshold.map(|t| t.to_bits()).hash(state);
        self.sign_based.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomQuantization {
    pub type_id: String,
    pub bits_per_element: i32,
    pub config: std::collections::HashMap<String, String>,
}

impl std::hash::Hash for CustomQuantization {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
        self.bits_per_element.hash(state);
        // Hash the config map in a deterministic way
        let mut sorted_config: Vec<_> = self.config.iter().collect();
        sorted_config.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in sorted_config {
            k.hash(state);
            v.hash(state);
        }
    }
}

/// TurboQuant configuration (ADR-021, TURBOQUANT_LLD_2026_05_30 §"Locked Type Signatures").
///
/// The three fields are everything a search path needs to dispatch:
/// - `bit_width` ∈ {2, 4} for P1; {2, 3, 4} after P10.
/// - `calibration_mode`: identity vs TQ+ per-coord. Frozen after first
///   qualifying batch when TqPlus is selected. See [`CalibrationMode`].
/// - `rotation_seed`: per-collection, derived via
///   `proximadb_quantization_types::derive_rotation_seed(collection_id)`
///   on first enablement, then persisted in xCatalog (P8).
///
/// The struct itself owns no buffers — codes, scales, and TQ+ calibration
/// vectors are persisted in the `.tq` file (LLD §3) and loaded into
/// `TurboQuantVectorData` (root crate) at search time.
#[cfg(feature = "experimental-turboquant")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TurboQuantization {
    /// Bit-width per coordinate. 2 or 4 in P1.
    pub bit_width: u8,
    /// Per-coord calibration mode. See foundation's `CalibrationMode`.
    pub calibration_mode: proximadb_quantization_types::CalibrationMode,
    /// Per-collection rotation seed. Derived from collection id at first
    /// enablement, then immutable for the collection's lifetime (until an
    /// epoch bump triggers a full re-encode).
    pub rotation_seed: u64,
}

#[cfg(feature = "experimental-turboquant")]
impl TurboQuantization {
    /// Construct a TurboQuant config with identity calibration. Use when
    /// the collection is too small for TQ+ or when explicitly disabling it.
    pub fn identity(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            bit_width,
            calibration_mode: proximadb_quantization_types::CalibrationMode::Identity,
            rotation_seed,
        }
    }

    /// Construct a TurboQuant config with TQ+ calibration. The calibration
    /// itself is fit lazily on the first qualifying batch (≥ 1000 vectors).
    pub fn tq_plus(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            bit_width,
            calibration_mode: proximadb_quantization_types::CalibrationMode::TqPlus,
            rotation_seed,
        }
    }
}

#[allow(non_upper_case_globals)]
impl UnifiedQuantizationLevel {
    /// Common quantization level constants for easy access
    /// Using PascalCase for API consistency with enum variant style
    pub const Binary: Self = Self {
        level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
            threshold: None,
            sign_based: false,
        })),
    };

    pub const Int8: Self = Self {
        level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
            bits: 8,
            scale: 1.0,
            offset: 0.0,
            clamp_values: true,
        })),
    };

    /// Create a PQ4 constant (requires runtime initialization due to parameter)
    pub const Pq4: Self = Self {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 4,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };

    /// Create a PQ8 constant (requires runtime initialization due to parameter)
    pub const Pq8: Self = Self {
        level_type: Some(QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 8, // Default value
            codebook_id: None,
            adaptive_subvectors: false,
        })),
    };

    /// Create a PQ8 configuration (common case)
    pub fn pq8(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 8,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }

    /// Create a PQ4 configuration (higher compression)
    pub fn pq4(num_subvectors: u8) -> Self {
        Self {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 4,
                num_subvectors: num_subvectors as i32,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        }
    }

    /// Create an INT8 scalar quantization
    pub fn int8() -> Self {
        Self {
            level_type: Some(QuantizationLevel::Scalar(ScalarQuantization {
                bits: 8,
                scale: 1.0,
                offset: 0.0,
                clamp_values: true,
            })),
        }
    }

    /// Create a binary quantization
    pub fn binary() -> Self {
        Self {
            level_type: Some(QuantizationLevel::Binary(BinaryQuantization {
                threshold: None,
                sign_based: false,
            })),
        }
    }

    /// Get the number of bits per element
    pub fn bits_per_element(&self) -> u32 {
        match &self.level_type {
            Some(QuantizationLevel::Pq(pq)) => pq.bits_per_code as u32,
            Some(QuantizationLevel::Scalar(sq)) => sq.bits as u32,
            Some(QuantizationLevel::Binary(_)) => 1,
            Some(QuantizationLevel::Uniform(uq)) => uq.bits as u32,
            Some(QuantizationLevel::Custom(cq)) => cq.bits_per_element as u32,
            #[cfg(feature = "experimental-turboquant")]
            Some(QuantizationLevel::TurboQuant(tq)) => tq.bit_width as u32,
            _ => 32, // Full precision
        }
    }

    /// Calculate bytes per vector based on quantization level
    pub fn bytes_per_vector(&self, dimension: usize) -> usize {
        match &self.level_type {
            Some(QuantizationLevel::Pq(pq)) => {
                let codes_per_vector = pq.num_subvectors as usize;
                let bytes_per_code = ((pq.bits_per_code + 7) / 8) as usize;
                codes_per_vector * bytes_per_code
            }
            Some(QuantizationLevel::Scalar(sq)) => dimension * ((sq.bits + 7) / 8) as usize,
            Some(QuantizationLevel::Binary(_)) => {
                dimension.div_ceil(8) // 1 bit per dimension
            }
            Some(QuantizationLevel::Uniform(uq)) => dimension * ((uq.bits + 7) / 8) as usize,
            Some(QuantizationLevel::Custom(cq)) => {
                dimension * ((cq.bits_per_element + 7) / 8) as usize
            }
            #[cfg(feature = "experimental-turboquant")]
            Some(QuantizationLevel::TurboQuant(tq)) => {
                // ceil(dim * bit_width / 8) — matches LLD §3 wire layout.
                // Plus 4 bytes per vector for the RaBitQ-style length-renorm
                // scale (stored separately in the .tq file but accounted for
                // here so callers get a single bytes-per-vector estimate).
                let bits_per_vec = dimension * tq.bit_width as usize;
                bits_per_vec.div_ceil(8) + 4
            }
            _ => dimension * 4, // Full FP32 precision
        }
    }

    /// Get compression ratio compared to FP32
    pub fn compression_ratio(&self, dimension: usize) -> f32 {
        let fp32_bytes = dimension * 4;
        let compressed_bytes = self.bytes_per_vector(dimension);
        fp32_bytes as f32 / compressed_bytes.max(1) as f32
    }

    /// Construct a TurboQuant configuration. Bit width must be 2 or 4 in P1
    /// (3-bit is deferred per LLD Q10).
    #[cfg(feature = "experimental-turboquant")]
    pub fn turboquant(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            level_type: Some(QuantizationLevel::TurboQuant(TurboQuantization::tq_plus(
                bit_width,
                rotation_seed,
            ))),
        }
    }

    /// Construct a TurboQuant configuration with identity calibration
    /// (no TQ+ per-coord adjustment).
    #[cfg(feature = "experimental-turboquant")]
    pub fn turboquant_identity(bit_width: u8, rotation_seed: u64) -> Self {
        Self {
            level_type: Some(QuantizationLevel::TurboQuant(TurboQuantization::identity(
                bit_width,
                rotation_seed,
            ))),
        }
    }
}

// ============================================================================
// QuantizationMethod trait impls
// (Phase A — Quantization Trait Convergence Plan)
// ============================================================================
//
// These impls bind every modality-side parameter struct to the foundation
// crate's `QuantizationMethod` trait. The trait is metadata-only — every
// method is `&self`, returns a small `Copy` value, and never touches the
// codebook cache or vector buffers. Routers, planners, EXPLAIN builders,
// and Prometheus label emitters call these methods instead of matching on
// the `QuantizationLevel` enum directly. See the plan document at
// `~/.claude/plans/dreamy-finding-clover.md` §"Design rationale" for the
// load-bearing reasoning, and ADR-021 §"Authority mode" for how lifecycle
// classification drives the read-time vs write-time routing decision.

impl QuantizationMethod for NoQuantization {
    fn quantization_type(&self) -> QuantizationType {
        QuantizationType::None
    }
    fn bit_width(&self) -> u8 {
        // FP32 baseline — the "no quantization" path is 32-bit float.
        32
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::Identity
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        None
    }
    fn metric_label(&self) -> &'static str {
        "none"
    }
}

impl QuantizationMethod for BinaryQuantization {
    fn quantization_type(&self) -> QuantizationType {
        QuantizationType::Binary
    }
    fn bit_width(&self) -> u8 {
        // Binary is always 1 bit per coord.
        1
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::WriteTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        // Binary quantization has no per-collection training state: the
        // threshold (when present) is a fixed config value, not durable
        // state requiring repair. Return `None` so xCatalog stores no row.
        None
    }
    fn metric_label(&self) -> &'static str {
        "binary"
    }
}

impl QuantizationMethod for ScalarQuantization {
    fn quantization_type(&self) -> QuantizationType {
        QuantizationType::Scalar
    }
    fn bit_width(&self) -> u8 {
        // Clamp to u8 — the on-disk schema uses i32 for legacy reasons but
        // values are always in {1..32}.
        self.bits.clamp(1, 32) as u8
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::WriteTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        // Scalar quantization carries scale/offset as parameters, not as
        // per-collection durable state. They live alongside the codes and
        // don't need a catalog row.
        None
    }
    fn metric_label(&self) -> &'static str {
        "scalar"
    }
}

impl QuantizationMethod for UniformQuantization {
    fn quantization_type(&self) -> QuantizationType {
        // Uniform quantization is a scalar variant with optional learned
        // scale/offset. From the router's perspective it's a scalar method.
        QuantizationType::Scalar
    }
    fn bit_width(&self) -> u8 {
        self.bits.clamp(1, 32) as u8
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::WriteTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        None
    }
    fn metric_label(&self) -> &'static str {
        "uniform"
    }
}

impl QuantizationMethod for ProductQuantization {
    fn quantization_type(&self) -> QuantizationType {
        QuantizationType::Product
    }
    fn bit_width(&self) -> u8 {
        self.bits_per_code.clamp(1, 32) as u8
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::WriteTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        // PQ collections persist their codebook id so a restart can rebind
        // codes to the right codebook. encoded_epoch defaults to 0 here;
        // xCatalog overlays the live epoch at hydration time.
        Some(DurableQuantState {
            seed_or_codebook_id: self.codebook_id.clone(),
            calibration: None,
            encoded_epoch: 0,
        })
    }
    fn metric_label(&self) -> &'static str {
        "product"
    }
}

impl QuantizationMethod for CustomQuantization {
    fn quantization_type(&self) -> QuantizationType {
        // Custom variants are tunneled through Scalar at the router level;
        // their `type_id` is opaque to the planner and only meaningful to
        // their own encode/score implementations.
        QuantizationType::Scalar
    }
    fn bit_width(&self) -> u8 {
        self.bits_per_element.clamp(1, 32) as u8
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        QuantizationLifecycle::WriteTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        // The type_id is the closest analogue to a codebook id — preserve
        // it so a restart can pick the right impl. No calibration tunnel.
        Some(DurableQuantState {
            seed_or_codebook_id: Some(self.type_id.clone()),
            calibration: None,
            encoded_epoch: 0,
        })
    }
    fn metric_label(&self) -> &'static str {
        "custom"
    }
}

#[cfg(feature = "experimental-turboquant")]
impl QuantizationMethod for TurboQuantization {
    fn quantization_type(&self) -> QuantizationType {
        QuantizationType::TurboQuant
    }
    fn bit_width(&self) -> u8 {
        self.bit_width
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        // Per ADR-021 §"Authority mode": TurboQuant codes are computed at
        // read-time from rotation_seed + frozen calibration; codes live in
        // the .tq sidecar, never in the columnar layout.
        QuantizationLifecycle::ReadTime
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        // rotation_seed + calibration_mode together let a restart
        // reconstruct the same encoding deterministically. encoded_epoch
        // is filled by the store at load time (xCatalog overlays the live
        // value at hydration — Phase E of the convergence plan).
        Some(DurableQuantState {
            seed_or_codebook_id: Some(format!("{:#x}", self.rotation_seed)),
            calibration: Some(self.calibration_mode.to_string()),
            encoded_epoch: 0,
        })
    }
    fn supports_candidate_mask(&self) -> bool {
        // TurboQuant's SIMD kernel consumes a packed u64 bitmap for the
        // block-skip fast path. Phase D AXIS adapters dispatch on this.
        true
    }
    fn metric_label(&self) -> &'static str {
        "turboquant"
    }
}

// ----------------------------------------------------------------------------
// Blanket impl on the enum + wrapper — what every router call site holds.
// ----------------------------------------------------------------------------

impl QuantizationMethod for QuantizationLevel {
    fn quantization_type(&self) -> QuantizationType {
        match self {
            Self::None(v) => v.quantization_type(),
            Self::Uniform(v) => v.quantization_type(),
            Self::Pq(v) => v.quantization_type(),
            Self::Scalar(v) => v.quantization_type(),
            Self::Binary(v) => v.quantization_type(),
            Self::Custom(v) => v.quantization_type(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.quantization_type(),
        }
    }
    fn bit_width(&self) -> u8 {
        match self {
            Self::None(v) => v.bit_width(),
            Self::Uniform(v) => v.bit_width(),
            Self::Pq(v) => v.bit_width(),
            Self::Scalar(v) => v.bit_width(),
            Self::Binary(v) => v.bit_width(),
            Self::Custom(v) => v.bit_width(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.bit_width(),
        }
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        match self {
            Self::None(v) => v.lifecycle(),
            Self::Uniform(v) => v.lifecycle(),
            Self::Pq(v) => v.lifecycle(),
            Self::Scalar(v) => v.lifecycle(),
            Self::Binary(v) => v.lifecycle(),
            Self::Custom(v) => v.lifecycle(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.lifecycle(),
        }
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        match self {
            Self::None(v) => v.durable_state(),
            Self::Uniform(v) => v.durable_state(),
            Self::Pq(v) => v.durable_state(),
            Self::Scalar(v) => v.durable_state(),
            Self::Binary(v) => v.durable_state(),
            Self::Custom(v) => v.durable_state(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.durable_state(),
        }
    }
    fn supports_candidate_mask(&self) -> bool {
        match self {
            Self::None(v) => v.supports_candidate_mask(),
            Self::Uniform(v) => v.supports_candidate_mask(),
            Self::Pq(v) => v.supports_candidate_mask(),
            Self::Scalar(v) => v.supports_candidate_mask(),
            Self::Binary(v) => v.supports_candidate_mask(),
            Self::Custom(v) => v.supports_candidate_mask(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.supports_candidate_mask(),
        }
    }
    fn metric_label(&self) -> &'static str {
        match self {
            Self::None(v) => v.metric_label(),
            Self::Uniform(v) => v.metric_label(),
            Self::Pq(v) => v.metric_label(),
            Self::Scalar(v) => v.metric_label(),
            Self::Binary(v) => v.metric_label(),
            Self::Custom(v) => v.metric_label(),
            #[cfg(feature = "experimental-turboquant")]
            Self::TurboQuant(v) => v.metric_label(),
        }
    }
}

impl QuantizationMethod for UnifiedQuantizationLevel {
    fn quantization_type(&self) -> QuantizationType {
        // None at the wrapper level means "no quantization configured" —
        // route the same as the explicit `NoQuantization` arm.
        match self.level_type.as_ref() {
            Some(level) => level.quantization_type(),
            None => QuantizationType::None,
        }
    }
    fn bit_width(&self) -> u8 {
        match self.level_type.as_ref() {
            Some(level) => level.bit_width(),
            None => 32,
        }
    }
    fn lifecycle(&self) -> QuantizationLifecycle {
        match self.level_type.as_ref() {
            Some(level) => level.lifecycle(),
            None => QuantizationLifecycle::Identity,
        }
    }
    fn durable_state(&self) -> Option<DurableQuantState> {
        self.level_type.as_ref().and_then(|l| l.durable_state())
    }
    fn supports_candidate_mask(&self) -> bool {
        self.level_type
            .as_ref()
            .is_some_and(|l| l.supports_candidate_mask())
    }
    fn metric_label(&self) -> &'static str {
        match self.level_type.as_ref() {
            Some(level) => level.metric_label(),
            None => "none",
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "experimental-turboquant")]
    use proximadb_quantization_types::CalibrationMode;

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_variant_2bit_bytes_per_vector() {
        // d=1536, bit_width=2 → ceil(1536*2/8) = 384 bytes + 4 scale = 388
        let q = UnifiedQuantizationLevel::turboquant(2, 0xdeadbeef);
        assert_eq!(q.bits_per_element(), 2);
        assert_eq!(q.bytes_per_vector(1536), 388);
        // Compression ratio matches the headline 16x at 2-bit minus the scale
        // overhead (~6144 / 388 ≈ 15.8).
        let r = q.compression_ratio(1536);
        assert!(r > 15.0 && r < 16.0, "ratio = {}", r);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_variant_4bit_bytes_per_vector() {
        // d=1536, bit_width=4 → ceil(1536*4/8) = 768 bytes + 4 scale = 772
        let q = UnifiedQuantizationLevel::turboquant(4, 0xdeadbeef);
        assert_eq!(q.bits_per_element(), 4);
        assert_eq!(q.bytes_per_vector(1536), 772);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_identity_constructor() {
        let q = UnifiedQuantizationLevel::turboquant_identity(4, 42);
        match &q.level_type {
            Some(QuantizationLevel::TurboQuant(tq)) => {
                assert_eq!(tq.bit_width, 4);
                assert_eq!(tq.rotation_seed, 42);
                assert_eq!(tq.calibration_mode, CalibrationMode::Identity);
            }
            _ => panic!("expected TurboQuant variant"),
        }
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn test_turboquant_serde_round_trip() {
        let q = TurboQuantization::tq_plus(2, 0xcafe_babe_dead_beef);
        let s = serde_json::to_string(&q).unwrap();
        let back: TurboQuantization = serde_json::from_str(&s).unwrap();
        assert_eq!(q, back);
    }

    // ------------------------------------------------------------------
    // QuantizationMethod trait coverage
    // (Phase A — Quantization Trait Convergence Plan)
    // ------------------------------------------------------------------

    #[test]
    fn quantization_method_on_no_quantization() {
        let m = NoQuantization {};
        assert_eq!(m.quantization_type(), QuantizationType::None);
        assert_eq!(m.bit_width(), 32);
        assert_eq!(m.lifecycle(), QuantizationLifecycle::Identity);
        assert!(m.durable_state().is_none());
        assert!(!m.supports_candidate_mask());
        assert_eq!(m.metric_label(), "none");
    }

    #[test]
    fn quantization_method_on_binary_quantization() {
        let m = BinaryQuantization {
            threshold: None,
            sign_based: false,
        };
        assert_eq!(m.quantization_type(), QuantizationType::Binary);
        assert_eq!(m.bit_width(), 1);
        assert_eq!(m.lifecycle(), QuantizationLifecycle::WriteTime);
        assert!(m.durable_state().is_none());
        assert!(!m.supports_candidate_mask());
        assert_eq!(m.metric_label(), "binary");
    }

    #[test]
    fn quantization_method_on_scalar_quantization() {
        let m = ScalarQuantization {
            bits: 8,
            scale: 1.0,
            offset: 0.0,
            clamp_values: false,
        };
        assert_eq!(m.quantization_type(), QuantizationType::Scalar);
        assert_eq!(m.bit_width(), 8);
        assert_eq!(m.lifecycle(), QuantizationLifecycle::WriteTime);
        assert!(m.durable_state().is_none());
        assert!(!m.supports_candidate_mask());
        assert_eq!(m.metric_label(), "scalar");
    }

    #[test]
    fn quantization_method_on_scalar_clamps_bits_into_u8_range() {
        // Pathological i32 values shouldn't cause wrap-around when packed
        // into the u8 trait return — the clamp guards routing/metrics from
        // a config-typo-induced denial of service.
        let m = ScalarQuantization {
            bits: 999,
            scale: 1.0,
            offset: 0.0,
            clamp_values: false,
        };
        assert_eq!(m.bit_width(), 32);
        let neg = ScalarQuantization {
            bits: -7,
            scale: 1.0,
            offset: 0.0,
            clamp_values: false,
        };
        assert_eq!(neg.bit_width(), 1);
    }

    #[test]
    fn quantization_method_on_product_quantization_durable_state_carries_codebook() {
        let m = ProductQuantization {
            bits_per_code: 4,
            num_subvectors: 8,
            codebook_id: Some("cb-xyz".to_string()),
            adaptive_subvectors: false,
        };
        assert_eq!(m.quantization_type(), QuantizationType::Product);
        assert_eq!(m.bit_width(), 4);
        assert_eq!(m.lifecycle(), QuantizationLifecycle::WriteTime);
        assert!(!m.supports_candidate_mask());
        assert_eq!(m.metric_label(), "product");
        let ds = m.durable_state().expect("PQ has durable state");
        assert_eq!(ds.seed_or_codebook_id.as_deref(), Some("cb-xyz"));
        assert!(ds.calibration.is_none());
        assert_eq!(ds.encoded_epoch, 0);
    }

    #[test]
    fn quantization_method_on_uniform_and_custom_route_through_scalar() {
        // Uniform and Custom both surface as Scalar at the router level —
        // their per-variant semantics live in their own encode/score paths.
        let u = UniformQuantization {
            bits: 8,
            scale: None,
            offset: None,
        };
        assert_eq!(u.quantization_type(), QuantizationType::Scalar);
        assert_eq!(u.lifecycle(), QuantizationLifecycle::WriteTime);
        assert_eq!(u.metric_label(), "uniform");

        let c = CustomQuantization {
            type_id: "custom-magic".to_string(),
            bits_per_element: 6,
            config: Default::default(),
        };
        assert_eq!(c.quantization_type(), QuantizationType::Scalar);
        assert_eq!(c.bit_width(), 6);
        assert_eq!(c.metric_label(), "custom");
        let cds = c.durable_state().expect("custom carries its type_id");
        assert_eq!(cds.seed_or_codebook_id.as_deref(), Some("custom-magic"));
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn quantization_method_on_turboquantization() {
        let m = TurboQuantization::tq_plus(4, 0xdead_beef_cafe_babe);
        assert_eq!(m.quantization_type(), QuantizationType::TurboQuant);
        assert_eq!(m.bit_width(), 4);
        assert_eq!(m.lifecycle(), QuantizationLifecycle::ReadTime);
        assert!(m.supports_candidate_mask());
        assert_eq!(m.metric_label(), "turboquant");
        let ds = m.durable_state().expect("TurboQuant has durable state");
        // rotation_seed is tunneled as a hex string for cross-protocol
        // stability. Pinning the format here protects xCatalog round-trip.
        assert_eq!(ds.seed_or_codebook_id.as_deref(), Some("0xdeadbeefcafebabe"));
        assert_eq!(ds.calibration.as_deref(), Some("tq_plus"));
        assert_eq!(ds.encoded_epoch, 0);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn quantization_method_on_turboquantization_identity_tunnels_correct_label() {
        let m = TurboQuantization::identity(2, 0x1);
        let ds = m.durable_state().expect("identity carries durable state");
        assert_eq!(ds.calibration.as_deref(), Some("identity"));
        assert_eq!(m.bit_width(), 2);
    }

    #[test]
    fn quantization_method_blanket_on_quantization_level_delegates_correctly() {
        // QuantizationLevel is the enum every router holds — verify each
        // arm delegates to its inner variant.
        let none = QuantizationLevel::None(NoQuantization {});
        assert_eq!(none.quantization_type(), QuantizationType::None);
        assert_eq!(none.lifecycle(), QuantizationLifecycle::Identity);

        let bin = QuantizationLevel::Binary(BinaryQuantization {
            threshold: None,
            sign_based: false,
        });
        assert_eq!(bin.quantization_type(), QuantizationType::Binary);
        assert_eq!(bin.bit_width(), 1);

        let pq = QuantizationLevel::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 16,
            codebook_id: Some("cb".to_string()),
            adaptive_subvectors: false,
        });
        assert_eq!(pq.quantization_type(), QuantizationType::Product);
        assert!(pq.durable_state().is_some());
    }

    #[test]
    fn quantization_method_blanket_on_unified_wrapper_handles_none_level_type() {
        // UnifiedQuantizationLevel { level_type: None } means "no
        // quantization configured" — the wrapper must route identical to
        // the explicit NoQuantization arm so downstream code can't tell the
        // two apart. This is what kills the "unwrap-or-default 32" bugs we
        // would have seen scattered across call sites.
        let empty = UnifiedQuantizationLevel { level_type: None };
        assert_eq!(empty.quantization_type(), QuantizationType::None);
        assert_eq!(empty.bit_width(), 32);
        assert_eq!(empty.lifecycle(), QuantizationLifecycle::Identity);
        assert!(empty.durable_state().is_none());
        assert!(!empty.supports_candidate_mask());
        assert_eq!(empty.metric_label(), "none");
    }

    #[test]
    fn quantization_method_blanket_on_unified_wrapper_delegates_to_inner() {
        // When a level is present, the wrapper just forwards.
        let q = UnifiedQuantizationLevel::int8();
        assert_eq!(q.quantization_type(), QuantizationType::Scalar);
        assert_eq!(q.bit_width(), 8);
        assert_eq!(q.lifecycle(), QuantizationLifecycle::WriteTime);
    }

    #[cfg(feature = "experimental-turboquant")]
    #[test]
    fn quantization_method_blanket_on_unified_wrapper_routes_turboquant_correctly() {
        let q = UnifiedQuantizationLevel::turboquant(4, 0xabcd);
        assert_eq!(q.quantization_type(), QuantizationType::TurboQuant);
        assert_eq!(q.lifecycle(), QuantizationLifecycle::ReadTime);
        assert!(q.supports_candidate_mask());
        assert_eq!(q.metric_label(), "turboquant");
    }
}
