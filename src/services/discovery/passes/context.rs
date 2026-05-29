//! `PassContext` — the inputs every refinement pass receives.
//!
//! A pass operates against a *pinned* read-only snapshot of a collection while
//! serving continues on live data (the CS/CD invariant). The context carries
//! the target collection, that pin, and the optional capability handles a pass
//! may require. A pass whose required capability is absent returns an identity
//! result (no-op) rather than failing the job — this keeps the walking-skeleton
//! and lightweight-test paths working without a fully wired service graph.
//!
//! New capabilities (embedding service for `ReEmbed`, an IVF index handle for
//! `Recluster`, …) are added as further optional fields here so the executor
//! stays decoupled from any individual pass's requirements.

use std::sync::Arc;

use crate::services::snapshot::SnapshotPin;
use crate::services::VectorOperationsService;

/// Inputs to a discovery refinement pass.
pub struct PassContext {
    /// Target collection (logical name).
    pub collection_id: String,
    /// Read-only snapshot the pass must operate against.
    pub snapshot: SnapshotPin,
    /// Canonical v2 read/write path. `None` => capability-dependent passes run
    /// as identity passes.
    pub vector_ops: Option<Arc<VectorOperationsService>>,
}

impl PassContext {
    /// Build a context for a pass against `snapshot`.
    pub fn new(snapshot: SnapshotPin) -> Self {
        Self {
            collection_id: snapshot.collection_id.clone(),
            snapshot,
            vector_ops: None,
        }
    }

    /// Attach the canonical vector-operations service (the v2 read/write path).
    pub fn with_vector_ops(
        mut self,
        vector_ops: Option<Arc<VectorOperationsService>>,
    ) -> Self {
        self.vector_ops = vector_ops;
        self
    }
}
