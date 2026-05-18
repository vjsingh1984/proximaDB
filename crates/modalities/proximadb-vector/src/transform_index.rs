//! Rebuildable transform projections for context-disentangled vector search.
//!
//! These types describe centroid plus residual/topic embeddings over canonical
//! records. They are AXIS projections, not durable truth: xCatalog must own the
//! projection descriptor, model identity, freshness, and repair source.

/// Projection descriptor for a context-disentangled embedding family.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformProjectionSpec {
    /// xCatalog projection name.
    pub projection_name: String,
    /// Source vector field on the canonical record.
    pub source_field: String,
    /// Embedding model used to derive centroid/residual vectors.
    pub model_id: String,
    /// Vector dimensionality for centroid and residual vectors.
    pub dimensions: usize,
    /// Maximum residual/topic vectors retained per source record.
    pub max_residuals: usize,
    /// Canonical collection/table used to rebuild the projection.
    pub repair_source: String,
    /// Maximum acceptable lag behind canonical records.
    pub max_freshness_lag_ms: u64,
}

impl TransformProjectionSpec {
    /// Validate projection metadata before registering or building it.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.projection_name.is_empty() {
            return Err("projection_name must not be empty");
        }
        if self.source_field.is_empty() {
            return Err("source_field must not be empty");
        }
        if self.model_id.is_empty() {
            return Err("model_id must not be empty");
        }
        if self.dimensions == 0 {
            return Err("dimensions must be > 0");
        }
        if self.max_residuals == 0 {
            return Err("max_residuals must be > 0");
        }
        if self.repair_source.is_empty() {
            return Err("repair_source must not be empty");
        }
        Ok(())
    }

    /// True when the projection requires synchronous maintenance.
    pub fn requires_synchronous_maintenance(&self) -> bool {
        self.max_freshness_lag_ms == 0
    }
}

/// Disentangled vector projection for one canonical record.
#[derive(Debug, Clone, PartialEq)]
pub struct DisentangledVectorProjection {
    /// Canonical record oid.
    pub record_oid: String,
    /// Tenant/RLS partition key.
    pub tenant_id: String,
    /// Centroid embedding representing common context.
    pub centroid: Vec<f32>,
    /// Residual/topic embeddings for independent semantic aspects.
    pub residuals: Vec<Vec<f32>>,
    /// Projection version/epoch for freshness checks.
    pub projection_epoch: u64,
    /// Source record version used to build this projection.
    pub source_record_version: u64,
}

impl DisentangledVectorProjection {
    /// Validate shape consistency against a projection spec.
    pub fn validate_against(&self, spec: &TransformProjectionSpec) -> Result<(), &'static str> {
        spec.validate()?;
        if self.record_oid.is_empty() {
            return Err("record_oid must not be empty");
        }
        if self.tenant_id.is_empty() {
            return Err("tenant_id must not be empty");
        }
        if self.centroid.len() != spec.dimensions {
            return Err("centroid dimensions do not match spec");
        }
        if self.residuals.len() > spec.max_residuals {
            return Err("residual count exceeds spec");
        }
        if self
            .residuals
            .iter()
            .any(|residual| residual.len() != spec.dimensions)
        {
            return Err("residual dimensions do not match spec");
        }
        if self.projection_epoch == 0 {
            return Err("projection_epoch must be > 0");
        }
        Ok(())
    }

    /// Reconstruct a topic vector as centroid plus residual.
    pub fn topic_vector(&self, residual_index: usize) -> Option<Vec<f32>> {
        let residual = self.residuals.get(residual_index)?;
        Some(
            self.centroid
                .iter()
                .zip(residual)
                .map(|(centroid, residual)| centroid + residual)
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TransformProjectionSpec {
        TransformProjectionSpec {
            projection_name: "docs_context_disentangled".to_string(),
            source_field: "embedding".to_string(),
            model_id: "test-model".to_string(),
            dimensions: 3,
            max_residuals: 2,
            repair_source: "docs".to_string(),
            max_freshness_lag_ms: 1_000,
        }
    }

    #[test]
    fn validates_disentangled_projection_shape() {
        let projection = DisentangledVectorProjection {
            record_oid: "doc-1".to_string(),
            tenant_id: "tenant-a".to_string(),
            centroid: vec![0.1, 0.2, 0.3],
            residuals: vec![vec![0.01, -0.02, 0.03]],
            projection_epoch: 7,
            source_record_version: 3,
        };

        assert!(projection.validate_against(&spec()).is_ok());
        assert!(!spec().requires_synchronous_maintenance());
        assert_eq!(projection.topic_vector(0), Some(vec![0.11, 0.18, 0.33]));
    }

    #[test]
    fn rejects_projection_shape_drift() {
        let projection = DisentangledVectorProjection {
            record_oid: "doc-1".to_string(),
            tenant_id: "tenant-a".to_string(),
            centroid: vec![0.1, 0.2],
            residuals: vec![vec![0.01, -0.02, 0.03]],
            projection_epoch: 7,
            source_record_version: 3,
        };

        assert_eq!(
            projection.validate_against(&spec()),
            Err("centroid dimensions do not match spec")
        );
    }

    #[test]
    fn rejects_uncataloged_repair_source() {
        let mut spec = spec();
        spec.repair_source.clear();

        assert_eq!(spec.validate(), Err("repair_source must not be empty"));
    }
}
