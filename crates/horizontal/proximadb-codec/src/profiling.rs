//! Machine-readable compression profiling records.
//!
//! These structs are the PCX-008 handoff format between benchmarks, codec
//! selection tests, xCatalog metadata, and future EXPLAIN fixtures. They carry
//! measured evidence for a codec/layout choice without enabling that choice by
//! default.

use serde::{Deserialize, Serialize};

use crate::strategy::{
    CodecDecision, CompressionProfile, LayoutHints, RejectedCodecCandidate, RejectionReason,
};

/// Compact rejected-candidate record suitable for catalog/explain surfaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionStatsRejectedCandidate {
    pub scheme: String,
    pub reason: RejectionReason,
    pub expected_ratio: Option<f32>,
}

impl From<&RejectedCodecCandidate> for CompressionStatsRejectedCandidate {
    fn from(candidate: &RejectedCodecCandidate) -> Self {
        Self {
            scheme: format!("{:?}", candidate.scheme),
            reason: candidate.reason,
            expected_ratio: candidate.expected_ratio,
        }
    }
}

/// Measured profile for one codec/layout decision over one benchmark or block family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionStatsProfile {
    /// Stable profile id used by xCatalog and EXPLAIN fixtures.
    pub profile_id: String,
    /// Optional table identifier when the profile is cataloged.
    pub table_id: Option<String>,
    /// Optional projection/access-method identifier when this is not a base column.
    pub projection_id: Option<String>,
    /// Policy and workload context used for the decision.
    pub compression_profile: CompressionProfile,
    /// Physical layout hints used for the decision.
    pub layout_hints: LayoutHints,
    /// Selected codec or pilot codec family.
    pub selected_scheme: String,
    /// Estimated raw/encoded ratio from selection.
    pub expected_ratio: Option<f32>,
    /// Measured raw/encoded ratio from profiling.
    pub measured_ratio: f64,
    /// True only when the encoded payload reconstructs exact visible values.
    pub exact_reconstruction: bool,
    /// Raw input bytes measured by the profiler.
    pub raw_bytes: u64,
    /// Encoded payload bytes measured by the profiler.
    pub encoded_bytes: u64,
    /// Number of logical values covered by this profile.
    pub value_count: u64,
    /// Measured encode CPU per block, if the harness captured it.
    pub encode_cpu_ms_per_block: Option<f64>,
    /// Measured decode cost per logical value, if the harness captured it.
    pub decode_ns_per_value: Option<f64>,
    /// Rejected alternatives and reasons.
    pub rejected_candidates: Vec<CompressionStatsRejectedCandidate>,
}

impl CompressionStatsProfile {
    /// Build a stats record from an explainable codec decision.
    pub fn from_decision(
        profile_id: impl Into<String>,
        compression_profile: CompressionProfile,
        layout_hints: LayoutHints,
        decision: &CodecDecision,
        raw_bytes: u64,
        encoded_bytes: u64,
        value_count: u64,
    ) -> Self {
        let rejected_candidates = decision
            .rejected_candidates
            .iter()
            .map(CompressionStatsRejectedCandidate::from)
            .collect();

        Self {
            profile_id: profile_id.into(),
            table_id: None,
            projection_id: None,
            compression_profile,
            layout_hints,
            selected_scheme: format!("{:?}", decision.scheme),
            expected_ratio: Some(decision.expected_ratio),
            measured_ratio: compression_ratio(raw_bytes, encoded_bytes),
            exact_reconstruction: decision.exact_reconstruction,
            raw_bytes,
            encoded_bytes,
            value_count,
            encode_cpu_ms_per_block: None,
            decode_ns_per_value: decision
                .expected_decode_ns_per_value
                .map(|value| value as f64),
            rejected_candidates,
        }
    }

    /// Build a stats record for a measured pilot codec that is not yet a `ProximaScheme`.
    pub fn from_measured_codec(
        profile_id: impl Into<String>,
        compression_profile: CompressionProfile,
        layout_hints: LayoutHints,
        selected_scheme: impl Into<String>,
        exact_reconstruction: bool,
        raw_bytes: u64,
        encoded_bytes: u64,
        value_count: u64,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            table_id: None,
            projection_id: None,
            compression_profile,
            layout_hints,
            selected_scheme: selected_scheme.into(),
            expected_ratio: None,
            measured_ratio: compression_ratio(raw_bytes, encoded_bytes),
            exact_reconstruction,
            raw_bytes,
            encoded_bytes,
            value_count,
            encode_cpu_ms_per_block: None,
            decode_ns_per_value: None,
            rejected_candidates: Vec::new(),
        }
    }

    pub fn with_table_id(mut self, table_id: impl Into<String>) -> Self {
        self.table_id = Some(table_id.into());
        self
    }

    pub fn with_projection_id(mut self, projection_id: impl Into<String>) -> Self {
        self.projection_id = Some(projection_id.into());
        self
    }

    pub fn with_encode_cpu_ms_per_block(mut self, value: f64) -> Self {
        self.encode_cpu_ms_per_block = Some(value);
        self
    }

    pub fn with_decode_ns_per_value(mut self, value: f64) -> Self {
        self.decode_ns_per_value = Some(value);
        self
    }

    pub fn bytes_per_value(&self) -> f64 {
        if self.value_count == 0 {
            0.0
        } else {
            self.encoded_bytes as f64 / self.value_count as f64
        }
    }

    pub fn meets_compression_target(&self) -> bool {
        match self.compression_profile.target_compression_ratio {
            Some(target) => self.measured_ratio >= target as f64,
            None => true,
        }
    }

    /// Protocol-neutral EXPLAIN payload fields for this profile.
    pub fn explain_fields(&self) -> CompressionExplainFields {
        CompressionExplainFields {
            profile_id: self.profile_id.clone(),
            selected_scheme: self.selected_scheme.clone(),
            measured_ratio: self.measured_ratio,
            expected_ratio: self.expected_ratio,
            exact_reconstruction: self.exact_reconstruction,
            raw_bytes: self.raw_bytes,
            encoded_bytes: self.encoded_bytes,
            decode_ns_per_value: self.decode_ns_per_value,
            rejected_candidates: self.rejected_candidates.clone(),
        }
    }
}

/// Small EXPLAIN-facing projection of a full stats profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionExplainFields {
    pub profile_id: String,
    pub selected_scheme: String,
    pub measured_ratio: f64,
    pub expected_ratio: Option<f32>,
    pub exact_reconstruction: bool,
    pub raw_bytes: u64,
    pub encoded_bytes: u64,
    pub decode_ns_per_value: Option<f64>,
    pub rejected_candidates: Vec<CompressionStatsRejectedCandidate>,
}

/// One benchmark/dataset observation emitted as a JSON record by harnesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionBenchmarkRecord {
    pub benchmark: String,
    pub dataset: String,
    pub stats: CompressionStatsProfile,
}

impl CompressionBenchmarkRecord {
    pub fn new(
        benchmark: impl Into<String>,
        dataset: impl Into<String>,
        stats: CompressionStatsProfile,
    ) -> Self {
        Self {
            benchmark: benchmark.into(),
            dataset: dataset.into(),
            stats,
        }
    }
}

fn compression_ratio(raw_bytes: u64, encoded_bytes: u64) -> f64 {
    if raw_bytes == 0 || encoded_bytes == 0 {
        0.0
    } else {
        raw_bytes as f64 / encoded_bytes as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessTemperature, AuthorityMode, CodecParameters, ColumnModality, LayoutHints, LossPolicy,
        PhysicalOrdering, ProximaScheme, RejectedCodecCandidate, RejectionReason,
        StorageSpecialization, WorkloadProfile,
    };

    fn vector_profile() -> CompressionProfile {
        CompressionProfile {
            authority_mode: AuthorityMode::CanonicalRecord,
            loss_policy: LossPolicy::ExactOnly,
            workload_profile: WorkloadProfile::AnnRerank,
            storage_specialization: StorageSpecialization::VectorExact,
            hotness: AccessTemperature::Warm,
            target_compression_ratio: Some(2.0),
            ..CompressionProfile::default()
        }
    }

    #[test]
    fn measured_profile_computes_ratio_and_target_result() {
        let stats = CompressionStatsProfile::from_measured_codec(
            "profile/vector/base-xor",
            vector_profile(),
            LayoutHints::vector_spatial(),
            "VectorBaseXorEntropy",
            true,
            1024,
            256,
            128,
        );

        assert_eq!(stats.measured_ratio, 4.0);
        assert_eq!(stats.bytes_per_value(), 2.0);
        assert!(stats.meets_compression_target());
        assert!(stats.explain_fields().exact_reconstruction);
    }

    #[test]
    fn decision_profile_preserves_rejected_candidates() {
        let decision = CodecDecision {
            scheme: ProximaScheme::Raw,
            parameters: CodecParameters::default(),
            expected_ratio: 1.0,
            expected_decode_ns_per_value: Some(12),
            exact_reconstruction: true,
            rejected_candidates: vec![RejectedCodecCandidate {
                scheme: ProximaScheme::Gorilla,
                reason: RejectionReason::LossyRejected,
                expected_ratio: Some(3.0),
            }],
        };

        let stats = CompressionStatsProfile::from_decision(
            "profile/pax/raw",
            CompressionProfile::default(),
            LayoutHints {
                modality: ColumnModality::Scalar,
                physical_ordering: PhysicalOrdering::PrimaryKey,
                ..LayoutHints::default()
            },
            &decision,
            512,
            512,
            64,
        );

        assert_eq!(stats.selected_scheme, "Raw");
        assert_eq!(stats.expected_ratio, Some(1.0));
        assert_eq!(stats.decode_ns_per_value, Some(12.0));
        assert_eq!(stats.rejected_candidates.len(), 1);
        assert_eq!(
            stats.rejected_candidates[0].reason,
            RejectionReason::LossyRejected
        );
    }

    #[test]
    fn benchmark_record_round_trips_as_json() {
        let stats = CompressionStatsProfile::from_measured_codec(
            "profile/json/path",
            CompressionProfile {
                storage_specialization: StorageSpecialization::JsonStructural,
                workload_profile: WorkloadProfile::DocumentScan,
                ..CompressionProfile::default()
            },
            LayoutHints {
                modality: ColumnModality::JsonStructural,
                physical_ordering: PhysicalOrdering::JsonShape,
                ..LayoutHints::default()
            },
            "Dictionary",
            true,
            4096,
            1024,
            256,
        )
        .with_table_id("docs.events")
        .with_projection_id("json-paths");
        let record = CompressionBenchmarkRecord::new("codec-json", "stable-shapes", stats);

        let encoded = serde_json::to_string(&record).unwrap();
        let decoded: CompressionBenchmarkRecord = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.benchmark, "codec-json");
        assert_eq!(decoded.stats.table_id.as_deref(), Some("docs.events"));
        assert_eq!(decoded.stats.measured_ratio, 4.0);
    }
}
