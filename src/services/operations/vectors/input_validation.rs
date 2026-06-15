//! Pure input-validation helpers extracted from `VectorOperationsService`
//! (Phase 2.1 god-object decomposition, slice 4).
//!
//! These are stateless validators over records / query vectors against a
//! collection's config (finite embeddings, dimension match, required/unique
//! ids). `VectorOperationsService` keeps a thin `&self` wrapper that resolves
//! the collection, then delegates the pure checks here.

use anyhow::Result;
use proximadb_records::{EmbeddingValues, ProximaRecord};

use crate::proto::proximadb_v1::{Collection, CollectionConfig};

/// Find the first non-finite value in an embedding (across precisions), if any,
/// as `(dimension_index, value_as_f32)`.
pub(crate) fn first_non_finite_embedding_value(
    values: &EmbeddingValues,
) -> Option<(usize, f32)> {
    match values {
        EmbeddingValues::Fp32(v) => v
            .iter()
            .copied()
            .enumerate()
            .find(|(_, value)| !value.is_finite()),
        EmbeddingValues::Fp16(v) => v
            .iter()
            .map(|value| value.to_f32())
            .enumerate()
            .find(|(_, value)| !value.is_finite()),
        EmbeddingValues::Bf16(v) => v
            .iter()
            .map(|value| value.to_f32())
            .enumerate()
            .find(|(_, value)| !value.is_finite()),
        EmbeddingValues::Int8Scalar { scale, .. } | EmbeddingValues::UInt8Scalar { scale, .. } => {
            if scale.is_finite() {
                None
            } else {
                Some((0, *scale))
            }
        }
    }
}

/// Validate a batch of records against the (already-resolved) collection config:
/// finite embeddings, dimension match, and — when the collection has indexes —
/// non-empty, bounded, batch-unique ids.
pub(crate) fn validate_records_for_insert(
    collection_id: &str,
    config: &CollectionConfig,
    records: &[ProximaRecord],
) -> Result<()> {
    let has_indexes = !config.index_configs.is_empty();
    let requires_id = has_indexes;
    let expected_dimension = config.dimension;

    if !requires_id && expected_dimension == 0 {
        return Ok(());
    }

    let mut seen_ids = if requires_id {
        Some(std::collections::HashSet::<&str>::with_capacity(
            records.len(),
        ))
    } else {
        None
    };

    let current_time_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    for (i, record) in records.iter().enumerate() {
        let dim = record
            .embeddings
            .first()
            .map(|e| e.values.len())
            .unwrap_or(0);
        let is_tombstone = dim == 0 && record.valid_to_ns.is_some_and(|v| v <= current_time_ns);

        for (embedding_idx, embedding) in record.embeddings.iter().enumerate() {
            if let Some((dimension_idx, value)) = first_non_finite_embedding_value(&embedding.values)
            {
                return Err(anyhow::anyhow!(
                    "Record at index {} embedding {} contains non-finite value at dimension {}: {}",
                    i,
                    embedding_idx,
                    dimension_idx,
                    value
                ));
            }
        }

        if !is_tombstone && expected_dimension > 0 && dim != expected_dimension as usize {
            return Err(anyhow::anyhow!(
                "Record at index {} has dimension {} but collection '{}' expects dimension {}",
                i,
                dim,
                collection_id,
                expected_dimension
            ));
        }

        if let Some(ref mut seen) = seen_ids {
            if record.oid.is_empty() {
                return Err(anyhow::anyhow!(
                    "Record at index {} has empty ID. Collection '{}' requires valid IDs",
                    i,
                    collection_id
                ));
            }

            if record.oid.len() > 256 {
                return Err(anyhow::anyhow!(
                    "Record ID '{}' exceeds maximum length of 256 characters",
                    record.oid
                ));
            }

            if !seen.insert(record.oid.as_str()) {
                return Err(anyhow::anyhow!(
                    "Duplicate ID '{}' found in batch. All IDs must be unique",
                    record.oid
                ));
            }
        }
    }

    Ok(())
}

/// Validate a query vector against the collection: finite values and a
/// dimension that matches the collection config (when configured).
pub(crate) fn validate_query_vector_for_search(
    collection_id: &str,
    collection: &Collection,
    query_vector: &[f32],
) -> Result<()> {
    if let Some((i, value)) = query_vector
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(anyhow::anyhow!(
            "Query vector for collection '{}' contains non-finite value at dimension {}: {}",
            collection_id,
            i,
            value
        ));
    }

    let expected_dimension = collection
        .config
        .as_ref()
        .map(|config| config.dimension)
        .unwrap_or_default();
    if expected_dimension > 0 && query_vector.len() != expected_dimension as usize {
        return Err(anyhow::anyhow!(
            "Query vector has dimension {} but collection '{}' expects dimension {}",
            query_vector.len(),
            collection_id,
            expected_dimension
        ));
    }

    Ok(())
}
