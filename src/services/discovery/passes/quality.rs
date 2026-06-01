//! Quality-scan pass (Phase 8 F1) — analysis-only refinement.
//!
//! Scans the pinned snapshot's embeddings for integrity/quality signals
//! (missing embeddings, zero-norm/degenerate vectors, non-finite values,
//! dimension drift, mean norm) and reports them as `quality_metrics`. No
//! external model is needed. The records are unchanged (`refined == input`,
//! `removed == 0`); the executor republishes the pinned snapshot. The signals
//! surface data-quality drift an operator — or the F1 trigger arm — can act on.

use anyhow::Result;

use super::PassContext;
use crate::services::discovery::DiscoveryJobResult;

/// Below this L2 norm an embedding is treated as degenerate (zero/near-zero).
const ZERO_NORM_EPS: f64 = 1e-9;

/// Run the quality-scan pass against `ctx.collection_id`. Identity pass (no-op)
/// if the canonical read path is not wired.
pub async fn run(ctx: &PassContext) -> Result<DiscoveryJobResult> {
    let Some(vector_ops) = ctx.vector_ops.as_ref() else {
        return Ok(DiscoveryJobResult::default());
    };
    let collection_id = vector_ops
        .resolve_collection_id(ctx.collection_id.as_str())
        .await;

    let records = vector_ops
        .list_all_records_with_tenant_context(collection_id.as_str(), None)
        .await?;
    let input = records.len() as u64;

    // Collect fp32 embeddings; count records with no usable embedding.
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(records.len());
    let mut missing_embedding: u64 = 0;
    for record in &records {
        match record.embeddings.first() {
            Some(cell) => {
                let view = cell.as_fp32_cow();
                if view.is_empty() {
                    missing_embedding += 1;
                } else {
                    vectors.push(view.into_owned());
                }
            }
            None => missing_embedding += 1,
        }
    }

    let stats = compute_quality(&vectors);

    // Quality scan never removes records: refined == input, removed == 0.
    let mut result = DiscoveryJobResult {
        input_record_count: input,
        refined_record_count: input,
        removed_count: 0,
        ..Default::default()
    };
    let m = &mut result.quality_metrics;
    m.insert("quality_input".to_string(), input as f64);
    m.insert("quality_embedded".to_string(), vectors.len() as f64);
    m.insert(
        "quality_missing_embedding".to_string(),
        missing_embedding as f64,
    );
    m.insert("quality_zero_norm".to_string(), stats.zero_norm as f64);
    m.insert("quality_nonfinite".to_string(), stats.nonfinite as f64);
    m.insert(
        "quality_dim_mismatch".to_string(),
        stats.dim_mismatch as f64,
    );
    m.insert("quality_mean_norm".to_string(), stats.mean_norm);
    Ok(result)
}

#[derive(Debug, Default, PartialEq)]
struct QualityStats {
    zero_norm: u64,
    nonfinite: u64,
    dim_mismatch: u64,
    mean_norm: f64,
}

/// Per-embedding integrity stats. `dim_mismatch` is measured against the modal
/// (first) dimension; `mean_norm` is the mean L2 norm over all vectors.
fn compute_quality(vectors: &[Vec<f32>]) -> QualityStats {
    if vectors.is_empty() {
        return QualityStats::default();
    }
    let modal_dim = vectors[0].len();
    let mut stats = QualityStats::default();
    let mut norm_sum = 0.0f64;
    for v in vectors {
        if v.len() != modal_dim {
            stats.dim_mismatch += 1;
        }
        let mut has_nonfinite = false;
        let mut sumsq = 0.0f64;
        for &x in v {
            if !x.is_finite() {
                has_nonfinite = true;
            } else {
                sumsq += (x as f64) * (x as f64);
            }
        }
        if has_nonfinite {
            stats.nonfinite += 1;
        }
        let norm = sumsq.sqrt();
        if norm < ZERO_NORM_EPS {
            stats.zero_norm += 1;
        }
        norm_sum += norm;
    }
    stats.mean_norm = norm_sum / vectors.len() as f64;
    stats
}

#[cfg(test)]
mod tests {
    use super::compute_quality;

    #[test]
    fn empty_is_all_zero() {
        let s = compute_quality(&[]);
        assert_eq!(s.zero_norm, 0);
        assert_eq!(s.mean_norm, 0.0);
    }

    #[test]
    fn flags_zero_norm_nonfinite_and_dim_mismatch() {
        let vectors = vec![
            vec![1.0f32, 0.0, 0.0],   // unit norm 1.0
            vec![0.0f32, 0.0, 0.0],   // zero-norm
            vec![f32::NAN, 1.0, 0.0], // non-finite
            vec![1.0f32, 1.0],        // dim mismatch (2 vs 3)
        ];
        let s = compute_quality(&vectors);
        assert_eq!(s.zero_norm, 1);
        assert_eq!(s.nonfinite, 1);
        assert_eq!(s.dim_mismatch, 1);
        assert!(s.mean_norm > 0.0);
    }
}
