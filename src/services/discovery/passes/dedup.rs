//! Dedup refinement pass (Phase 8 F1 keystone, S3).
//!
//! Reads the pinned snapshot's records via the v2 canonical storage-inclusive
//! read path (`VectorOperationsService::list_all_records_with_tenant_context`,
//! which merges WAL/memtable + flushed storage), detects near-duplicate
//! embeddings by cosine similarity, and removes the duplicates via the v2
//! canonical delete path (`delete_records_with_tenant_context`, tombstones).
//! The executor then atomically republishes the refined snapshot.
//!
//! Scope (MVP): O(n^2) greedy detection — fine for offline batch scale; a
//! future version can route distance through the SIMD provider and add LSH
//! blocking for larger corpora.

use anyhow::Result;

use super::PassContext;
use crate::services::discovery::DiscoveryJobResult;

/// Cosine-similarity threshold at/above which two records are treated as
/// near-duplicates. Exact / near-exact duplicates score ~1.0.
const DEFAULT_DEDUP_SIMILARITY: f32 = 0.9999;

/// Run the dedup pass against `ctx.collection_id`, returning the refinement
/// result. Identity pass (no-op) if the canonical read/write path is not wired.
pub async fn run(ctx: &PassContext) -> Result<DiscoveryJobResult> {
    let Some(vector_ops) = ctx.vector_ops.as_ref() else {
        // No canonical path wired: identity pass (republish unchanged).
        return Ok(DiscoveryJobResult::default());
    };
    // Resolve the user-facing collection name to the canonical internal id the
    // write path keys WAL + storage under (the catalog/snapshot side uses the
    // name; the vector data side uses the resolved id).
    let collection_id = vector_ops
        .resolve_collection_id(ctx.collection_id.as_str())
        .await;
    let collection_id = collection_id.as_str();

    // v2 canonical storage-inclusive read: WAL/memtable + flushed storage,
    // merged by oid (freshest wins). Records carry embeddings.
    let records = vector_ops
        .list_all_records_with_tenant_context(collection_id, None)
        .await?;
    let input = records.len() as u64;

    // Extract (oid, fp32 vector), skipping records without an embedding.
    let mut vectors: Vec<(String, Vec<f32>)> = Vec::with_capacity(records.len());
    for record in &records {
        if let Some(cell) = record.embeddings.first() {
            let view = cell.as_fp32_cow();
            if !view.is_empty() {
                vectors.push((record.oid.clone(), view.into_owned()));
            }
        }
    }

    // Greedy near-duplicate detection: keep the first occurrence, mark later
    // records within the similarity threshold of any kept record as duplicates.
    let mut kept: Vec<usize> = Vec::new();
    let mut duplicate_ids: Vec<String> = Vec::new();
    for (idx, (oid, vec)) in vectors.iter().enumerate() {
        let is_duplicate = kept
            .iter()
            .any(|&k| cosine_similarity(vec, &vectors[k].1) >= DEFAULT_DEDUP_SIMILARITY);
        if is_duplicate {
            duplicate_ids.push(oid.clone());
        } else {
            kept.push(idx);
        }
    }

    let removed = duplicate_ids.len() as u64;
    if !duplicate_ids.is_empty() {
        // v2 canonical delete (tombstones).
        vector_ops
            .delete_records_with_tenant_context(collection_id, duplicate_ids, None)
            .await?;
    }

    let mut result = DiscoveryJobResult {
        input_record_count: input,
        refined_record_count: input.saturating_sub(removed),
        removed_count: removed,
        ..Default::default()
    };
    result
        .quality_metrics
        .insert("dedup_removed".to_string(), removed as f64);
    result
        .quality_metrics
        .insert("dedup_input".to_string(), input as f64);
    Ok(result)
}

/// Cosine similarity in [-1, 1]; 0.0 for mismatched/empty/degenerate inputs.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[cfg(test)]
mod tests {
    use super::cosine_similarity;

    #[test]
    fn identical_vectors_score_one() {
        let v = [0.3f32, 0.4, 0.5];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn mismatched_or_empty_is_zero() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }
}
