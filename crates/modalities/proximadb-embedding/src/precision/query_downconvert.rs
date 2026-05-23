//! Query vector downconverter — PR 8b of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` §"Query vector
//! downconvert (Q10)".
//!
//! At search entry, the planner downconverts the inbound fp32 query
//! vector ONCE to the collection's canonical precision before handing it
//! to the ANN kernels. Doing the conversion at the search boundary (not
//! per-stored-vector during the inner loop) is the whole point of the
//! Q10 design: an O(dim) cost paid once vs. an O(N×dim) cost paid per
//! query.
//!
//! Callers always pass `&[f32]` because every public protocol surface
//! (REST v1, REST v2, gRPC `Search`, pgwire vector literal) parses the
//! query into fp32 before any storage-aware code sees it. If a future
//! protocol surfaces precision-aware queries, swap the call site to
//! `prepare_query_from` to skip the fp32 round-trip.

use proximadb_records::{EmbeddingScalarType, EmbeddingValues};

use crate::precision::boundary::{EmbeddingOutput, project_to_canonical};

/// Project a fp32 query vector into the collection's canonical precision.
///
/// `query` is the raw fp32 vector parsed from the network protocol.
/// `canonical` is the collection's canonical embedding precision (read
/// from `CatalogTableSchema.canonical_embedding_precision`, PR 6b).
///
/// Returns values in canonical shape; ANN kernels then dispatch on
/// `(query.precision, stored.precision)`. For lossy targets (int8,
/// uint8), the per-vector scale/zero-point computed here is what the
/// kernel uses to reconstruct quantized distance — so the choice of
/// scaling MUST match the writer-side quantizer (it does:
/// `EmbeddingValues::from_fp32_lossy` is the shared point of truth).
pub fn prepare_query(query: &[f32], canonical: EmbeddingScalarType) -> EmbeddingValues {
    EmbeddingValues::from_fp32_lossy(query, canonical)
}

/// Precision-aware variant: project from an arbitrary upstream output
/// (e.g. a re-ranker that produced fp16 candidates) into canonical
/// without the fp32 round-trip when source and target match.
///
/// Reuses the boundary adapter so writer and query paths are guaranteed
/// to apply identical conversions.
pub fn prepare_query_from(
    output: EmbeddingOutput,
    canonical: EmbeddingScalarType,
) -> EmbeddingValues {
    project_to_canonical(output, canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::{bf16, f16};

    fn sample_query() -> Vec<f32> {
        vec![-1.0, -0.5, -0.125, 0.0, 0.125, 0.5, 1.0, 0.7654321]
    }

    #[test]
    fn prepare_query_to_fp32_is_identity_clone() {
        let q = sample_query();
        let prepared = prepare_query(&q, EmbeddingScalarType::Fp32);
        match prepared {
            EmbeddingValues::Fp32(v) => assert_eq!(v, q),
            _ => panic!("expected Fp32"),
        }
    }

    #[test]
    fn prepare_query_to_fp16_narrows_within_tolerance() {
        let q = sample_query();
        let prepared = prepare_query(&q, EmbeddingScalarType::Fp16);
        let back = match prepared {
            EmbeddingValues::Fp16(v) => v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>(),
            _ => panic!("expected Fp16"),
        };
        for (i, (&got, &want)) in back.iter().zip(q.iter()).enumerate() {
            assert!((got - want).abs() < 1e-3, "element {i}");
        }
    }

    #[test]
    fn prepare_query_to_bf16_round_trips_within_bf16_tolerance() {
        let q = sample_query();
        let prepared = prepare_query(&q, EmbeddingScalarType::Bf16);
        let back = match prepared {
            EmbeddingValues::Bf16(v) => v.iter().map(|x| x.to_f32()).collect::<Vec<f32>>(),
            _ => panic!("expected Bf16"),
        };
        for (i, (&got, &want)) in back.iter().zip(q.iter()).enumerate() {
            assert!((got - want).abs() < 1e-2, "element {i}");
        }
    }

    #[test]
    fn prepare_query_to_int8_uses_symmetric_scale() {
        let q = sample_query();
        let prepared = prepare_query(&q, EmbeddingScalarType::Int8Scalar);
        match prepared {
            EmbeddingValues::Int8Scalar {
                values,
                scale,
                zero_point,
            } => {
                assert_eq!(values.len(), q.len());
                assert_eq!(zero_point, 0);
                assert!(scale > 0.0);
            }
            _ => panic!("expected Int8Scalar"),
        }
    }

    #[test]
    fn prepare_query_to_uint8_uses_zero_point() {
        let q = vec![-2.0f32, -1.0, 0.0, 1.0, 2.0];
        let prepared = prepare_query(&q, EmbeddingScalarType::UInt8Scalar);
        match prepared {
            EmbeddingValues::UInt8Scalar {
                values,
                scale,
                zero_point,
            } => {
                assert_eq!(values.len(), q.len());
                assert!(zero_point > 0, "uint8 zero-point shifts negatives");
                assert!(scale > 0.0);
            }
            _ => panic!("expected UInt8Scalar"),
        }
    }

    #[test]
    fn prepare_query_matches_writer_quantization_byte_for_byte() {
        // Critical invariant: the writer-side `from_fp32_lossy` and the
        // query-side `prepare_query` MUST produce identical bytes for
        // identical input. If they diverge, the int8/uint8 distance
        // kernels' reconstructed scores are wrong.
        let q = sample_query();
        for target in [
            EmbeddingScalarType::Fp32,
            EmbeddingScalarType::Fp16,
            EmbeddingScalarType::Bf16,
            EmbeddingScalarType::Int8Scalar,
            EmbeddingScalarType::UInt8Scalar,
        ] {
            let writer = EmbeddingValues::from_fp32_lossy(&q, target);
            let query = prepare_query(&q, target);
            assert_eq!(writer, query, "writer/query diverged for {target:?}");
        }
    }

    #[test]
    fn prepare_query_from_skips_fp32_round_trip_for_native_fp16() {
        // prepare_query_from(Fp16, Fp16) must return the identical Vec<f16>
        // without going through fp32 (boundary.rs's identity shortcut).
        let src: Vec<f16> = sample_query().iter().map(|&x| f16::from_f32(x)).collect();
        let prepared = prepare_query_from(
            EmbeddingOutput::Fp16(src.clone()),
            EmbeddingScalarType::Fp16,
        );
        match prepared {
            EmbeddingValues::Fp16(v) => assert_eq!(v, src),
            _ => panic!("expected Fp16 variant"),
        }
    }

    #[test]
    fn prepare_query_from_bf16_to_fp32_promotes_losslessly() {
        let src: Vec<bf16> = sample_query().iter().map(|&x| bf16::from_f32(x)).collect();
        let prepared = prepare_query_from(
            EmbeddingOutput::Bf16(src.clone()),
            EmbeddingScalarType::Fp32,
        );
        match prepared {
            EmbeddingValues::Fp32(v) => {
                assert_eq!(v.len(), src.len());
                for (got, want) in v.iter().zip(src.iter()) {
                    assert_eq!(*got, want.to_f32());
                }
            }
            _ => panic!("expected Fp32 variant"),
        }
    }

    #[test]
    fn empty_query_returns_empty_values() {
        let prepared = prepare_query(&[], EmbeddingScalarType::Fp16);
        assert_eq!(prepared.len(), 0);
    }
}
