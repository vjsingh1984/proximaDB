//! Embedding service boundary downconverter — PR 8a of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` §"Embedding
//! service boundary downconversion (Q15)".
//!
//! Every model output (BgeModel ONNX path, OpenAI/Cohere/Azure/Byo HTTP
//! clients) flows through `project_to_canonical()` exactly once, before
//! the resulting `EmbeddingValues` is wrapped in an `EmbeddingCell` and
//! written to the canonical store. The adapter pattern keeps each model
//! producer free to emit its native dtype (fp32, fp16, bf16) without
//! caring about the collection's canonical precision policy.
//!
//! The function is pure (no I/O, no metrics) so it's safe to call from
//! anywhere in the ingest path. The caller is responsible for emitting
//! the `proximadb_embedding_precision_conversions_total{from,to,site}`
//! counter (PR 7b) when the conversion actually narrows precision.

use proximadb_records::{EmbeddingScalarType, EmbeddingValues};

/// What a model or HTTP client produces as its native output. New
/// variants land when an upstream provider adds a new dtype (e.g. when
/// Cohere ships an int8 output mode).
#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddingOutput {
    /// Most common: fp32 JSON / proto / ndarray.
    Fp32(Vec<f32>),
    /// fp16 output from a fp16-weight ONNX session (BGE-large fp16 export).
    Fp16(Vec<half::f16>),
    /// bf16 output. Reserved for Phase 6 hardware paths.
    Bf16(Vec<half::bf16>),
}

impl EmbeddingOutput {
    /// Native scalar type of this output, before any projection.
    pub fn native_precision(&self) -> EmbeddingScalarType {
        match self {
            Self::Fp32(_) => EmbeddingScalarType::Fp32,
            Self::Fp16(_) => EmbeddingScalarType::Fp16,
            Self::Bf16(_) => EmbeddingScalarType::Bf16,
        }
    }

    /// Number of elements, regardless of variant. Useful for batched
    /// converters that need to skip empty outputs early.
    pub fn len(&self) -> usize {
        match self {
            Self::Fp32(v) => v.len(),
            Self::Fp16(v) => v.len(),
            Self::Bf16(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Materialize this output as an owned `Vec<f32>`. fp32 inputs reuse
    /// their storage; fp16/bf16 inputs allocate a new buffer.
    fn to_fp32(self) -> Vec<f32> {
        match self {
            Self::Fp32(v) => v,
            Self::Fp16(v) => v.iter().map(|x| x.to_f32()).collect(),
            Self::Bf16(v) => v.iter().map(|x| x.to_f32()).collect(),
        }
    }
}

/// Project a model `output` into the collection's canonical precision.
///
/// Identity shortcuts (fp32→fp32, fp16→fp16, bf16→bf16) avoid the fp32
/// round-trip entirely. Every other path goes through fp32 because the
/// `EmbeddingValues::from_fp32_lossy` quantizer (per-vector symmetric
/// int8, zero-point-aware uint8) operates on fp32 source data.
///
/// Returns the values in canonical shape; the caller wraps them in an
/// `EmbeddingCell` with the matching `precision` discriminant.
pub fn project_to_canonical(
    output: EmbeddingOutput,
    canonical: EmbeddingScalarType,
) -> EmbeddingValues {
    match (&output, canonical) {
        // === Identity shortcuts — skip the fp32 round-trip ===
        (EmbeddingOutput::Fp32(_), EmbeddingScalarType::Fp32) => {
            if let EmbeddingOutput::Fp32(v) = output {
                EmbeddingValues::Fp32(v)
            } else {
                unreachable!()
            }
        }
        (EmbeddingOutput::Fp16(_), EmbeddingScalarType::Fp16) => {
            if let EmbeddingOutput::Fp16(v) = output {
                EmbeddingValues::Fp16(v)
            } else {
                unreachable!()
            }
        }
        (EmbeddingOutput::Bf16(_), EmbeddingScalarType::Bf16) => {
            if let EmbeddingOutput::Bf16(v) = output {
                EmbeddingValues::Bf16(v)
            } else {
                unreachable!()
            }
        }
        // === Everything else: promote to fp32, then narrow to canonical ===
        _ => EmbeddingValues::from_fp32_lossy(&output.to_fp32(), canonical),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::{bf16, f16};

    fn sample_fp32() -> Vec<f32> {
        // Span a useful range for fp16 / int8 round-trip checks.
        vec![-1.0, -0.5, -0.125, 0.0, 0.125, 0.5, 1.0, 0.7654321]
    }

    #[test]
    fn native_precision_matches_variant() {
        assert_eq!(
            EmbeddingOutput::Fp32(vec![]).native_precision(),
            EmbeddingScalarType::Fp32
        );
        assert_eq!(
            EmbeddingOutput::Fp16(vec![]).native_precision(),
            EmbeddingScalarType::Fp16
        );
        assert_eq!(
            EmbeddingOutput::Bf16(vec![]).native_precision(),
            EmbeddingScalarType::Bf16
        );
    }

    #[test]
    fn len_and_empty_reflect_underlying_vec() {
        let o = EmbeddingOutput::Fp16(vec![f16::from_f32(0.0); 5]);
        assert_eq!(o.len(), 5);
        assert!(!o.is_empty());
        let o = EmbeddingOutput::Fp32(vec![]);
        assert!(o.is_empty());
    }

    #[test]
    fn fp32_to_fp32_is_identity_no_alloc() {
        let src = sample_fp32();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::Fp32,
        );
        match projected {
            EmbeddingValues::Fp32(v) => assert_eq!(v, src),
            _ => panic!("expected Fp32 variant"),
        }
    }

    #[test]
    fn fp16_to_fp16_is_identity() {
        let src: Vec<f16> = sample_fp32().iter().map(|&x| f16::from_f32(x)).collect();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp16(src.clone()),
            EmbeddingScalarType::Fp16,
        );
        match projected {
            EmbeddingValues::Fp16(v) => assert_eq!(v, src),
            _ => panic!("expected Fp16 variant"),
        }
    }

    #[test]
    fn bf16_to_bf16_is_identity() {
        let src: Vec<bf16> = sample_fp32().iter().map(|&x| bf16::from_f32(x)).collect();
        let projected = project_to_canonical(
            EmbeddingOutput::Bf16(src.clone()),
            EmbeddingScalarType::Bf16,
        );
        match projected {
            EmbeddingValues::Bf16(v) => assert_eq!(v, src),
            _ => panic!("expected Bf16 variant"),
        }
    }

    #[test]
    fn fp32_to_fp16_narrows_and_round_trips_within_fp16_epsilon() {
        let src = sample_fp32();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::Fp16,
        );
        let back = match projected {
            EmbeddingValues::Fp16(v) => v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>(),
            _ => panic!("expected Fp16 variant"),
        };
        // fp16 has ~3 decimal digits of precision; allow 1e-3 per element.
        for (i, (&got, &want)) in back.iter().zip(src.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-3,
                "element {i}: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn fp16_to_fp32_promotes_losslessly() {
        let src: Vec<f16> = sample_fp32().iter().map(|&x| f16::from_f32(x)).collect();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp16(src.clone()),
            EmbeddingScalarType::Fp32,
        );
        let back = match projected {
            EmbeddingValues::Fp32(v) => v,
            _ => panic!("expected Fp32 variant"),
        };
        // Promotion is bit-exact (fp16 → fp32 has no rounding).
        for (i, (&got, &want)) in back.iter().zip(src.iter()).enumerate() {
            assert_eq!(got, want.to_f32(), "element {i}");
        }
    }

    #[test]
    fn fp32_to_bf16_round_trips_within_bf16_epsilon() {
        let src = sample_fp32();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::Bf16,
        );
        let back = match projected {
            EmbeddingValues::Bf16(v) => v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>(),
            _ => panic!("expected Bf16 variant"),
        };
        // bf16 has the same range as fp32 but only 7-bit mantissa: ~3e-3
        // worst case for values near 1.0.
        for (i, (&got, &want)) in back.iter().zip(src.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-2,
                "element {i}: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn fp32_to_int8_quantizes_with_per_vector_symmetric_scale() {
        let src = sample_fp32();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::Int8Scalar,
        );
        match projected {
            EmbeddingValues::Int8Scalar {
                values,
                scale,
                zero_point,
            } => {
                assert_eq!(values.len(), src.len());
                assert_eq!(zero_point, 0, "int8 is symmetric (zero_point = 0)");
                assert!(scale > 0.0);
                // Reconstruct and verify within int8 tolerance: per-element
                // error ≤ scale (one quantization step).
                for (i, (&q, &want)) in values.iter().zip(src.iter()).enumerate() {
                    let back = q as f32 * scale;
                    assert!(
                        (back - want).abs() <= scale * 1.01,
                        "element {i}: got {back}, want {want}, scale={scale}"
                    );
                }
            }
            _ => panic!("expected Int8Scalar variant"),
        }
    }

    #[test]
    fn fp32_to_uint8_uses_zero_point() {
        let src = vec![-2.0, -1.0, 0.0, 1.0, 2.0];
        let projected = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::UInt8Scalar,
        );
        match projected {
            EmbeddingValues::UInt8Scalar {
                values,
                scale,
                zero_point,
            } => {
                assert_eq!(values.len(), src.len());
                assert!(zero_point > 0, "uint8 zero-point must shift negatives");
                assert!(scale > 0.0);
                for (i, (&q, &want)) in values.iter().zip(src.iter()).enumerate() {
                    let back = (q as f32 - zero_point as f32) * scale;
                    assert!(
                        (back - want).abs() <= scale * 1.01,
                        "element {i}: got {back}, want {want}"
                    );
                }
            }
            _ => panic!("expected UInt8Scalar variant"),
        }
    }

    #[test]
    fn fp16_input_to_int8_goes_via_fp32() {
        // Cross-narrowing: source fp16, canonical int8. Should still
        // produce a valid quantized output (per-vector scale).
        let src_f32 = sample_fp32();
        let src_f16: Vec<f16> = src_f32.iter().map(|&x| f16::from_f32(x)).collect();
        let projected = project_to_canonical(
            EmbeddingOutput::Fp16(src_f16),
            EmbeddingScalarType::Int8Scalar,
        );
        match projected {
            EmbeddingValues::Int8Scalar { values, scale, .. } => {
                assert_eq!(values.len(), src_f32.len());
                assert!(scale > 0.0);
            }
            _ => panic!("expected Int8Scalar variant"),
        }
    }

    #[test]
    fn byte_size_halves_when_fp32_projected_to_fp16() {
        let src = vec![0.5f32; 1024];
        let fp32 = project_to_canonical(
            EmbeddingOutput::Fp32(src.clone()),
            EmbeddingScalarType::Fp32,
        );
        let fp16 = project_to_canonical(EmbeddingOutput::Fp32(src), EmbeddingScalarType::Fp16);
        let fp32_bytes = match fp32 {
            EmbeddingValues::Fp32(v) => v.len() * std::mem::size_of::<f32>(),
            _ => unreachable!(),
        };
        let fp16_bytes = match fp16 {
            EmbeddingValues::Fp16(v) => v.len() * std::mem::size_of::<f16>(),
            _ => unreachable!(),
        };
        assert_eq!(
            fp32_bytes,
            fp16_bytes * 2,
            "fp16 storage must be exactly half of fp32 (LLD §Motivation)"
        );
    }
}
