//! `DiscoveryTrigger` — the feedback arm of the CS/CD flywheel (Phase 8 F1, T1.9).
//!
//! The executor + snapshot coordinator are the *refine → republish* half. This
//! is the *feedback → trigger* half: it turns serving-side signals (recall
//! degradation, freshness-SLA breaches, workload drift) into scheduled
//! `DiscoveryJob`s, which is what makes the loop *continuous* rather than
//! operator-invoked.
//!
//! Coalescing is the core invariant: a sustained signal (e.g. a recall probe
//! that fails every tick) must NOT enqueue a job every tick. A new job is
//! enqueued only when no job of the same kind is already pending/running for the
//! collection — a minimal scheduling policy. Richer cost-aware scheduling
//! (budget, prioritization via `src/automl/`) is a follow-up that wraps this.
//!
//! Signal sources (the `RecallProbeGate`, the freshness state machine, the
//! AutoML `WorkloadAnalyzer`) call [`DiscoveryService::on_signal`] /
//! [`DiscoveryTrigger::on_signal`]; this module owns the policy, not the
//! plumbing to any specific source.

use std::sync::Arc;

use super::job::{DiscoveryJob, DiscoveryJobKind, DiscoveryJobStatus};
use super::registry::DiscoveryRegistry;

/// A serving-side signal that may warrant offline refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSignal {
    /// Recall has degraded for a collection (e.g. a `RecallProbeGate` FAIL
    /// streak) — cluster centroids are likely stale.
    RecallDegraded,
    /// A collection's freshness SLA was breached / its projection went `Stale`
    /// — the served index lags the current data.
    FreshnessBreached,
    /// Workload drift / access-pattern shift detected (AutoML `WorkloadAnalyzer`).
    WorkloadDrift,
}

impl TriggerSignal {
    /// The refinement kind this signal maps to.
    ///
    /// All current signals indicate index/cluster staleness, so they map to
    /// `Recluster` — the first real pass. As `ReEmbed` / `QualityScan` land,
    /// signals that specifically imply model change or data-quality issues will
    /// map to those instead.
    pub fn kind(self) -> DiscoveryJobKind {
        match self {
            TriggerSignal::RecallDegraded
            | TriggerSignal::FreshnessBreached
            | TriggerSignal::WorkloadDrift => DiscoveryJobKind::Recluster,
        }
    }
}

/// Feedback arm: turns serving signals into scheduled discovery jobs, coalescing
/// against jobs already in flight for the same `(collection, kind)`.
pub struct DiscoveryTrigger {
    registry: Arc<DiscoveryRegistry>,
}

impl DiscoveryTrigger {
    pub fn new(registry: Arc<DiscoveryRegistry>) -> Self {
        Self { registry }
    }

    /// Consider a signal for a collection. Enqueues a discovery job unless one
    /// of the same kind is already `Scheduled`/`Running` for that collection
    /// (coalescing). Returns the enqueued job, or `None` if coalesced.
    pub fn on_signal(&self, collection_id: &str, signal: TriggerSignal) -> Option<DiscoveryJob> {
        let kind = signal.kind();
        if self.has_in_flight(collection_id, kind) {
            return None;
        }
        Some(
            self.registry
                .schedule(DiscoveryJob::new(collection_id, kind)),
        )
    }

    /// True if a job of `kind` for `collection_id` is already `Scheduled` or
    /// `Running` (i.e. not yet terminal).
    fn has_in_flight(&self, collection_id: &str, kind: DiscoveryJobKind) -> bool {
        self.registry
            .list_for_collection(collection_id)
            .iter()
            .any(|j| {
                j.kind == kind
                    && matches!(
                        j.status,
                        DiscoveryJobStatus::Scheduled | DiscoveryJobStatus::Running
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger() -> DiscoveryTrigger {
        DiscoveryTrigger::new(Arc::new(DiscoveryRegistry::new()))
    }

    #[test]
    fn signal_enqueues_a_recluster_job() {
        let t = trigger();
        let job = t.on_signal("c1", TriggerSignal::RecallDegraded).unwrap();
        assert_eq!(job.kind, DiscoveryJobKind::Recluster);
        assert_eq!(job.status, DiscoveryJobStatus::Scheduled);
    }

    #[test]
    fn sustained_signal_is_coalesced_while_in_flight() {
        let t = trigger();
        let first = t.on_signal("c1", TriggerSignal::RecallDegraded);
        assert!(first.is_some(), "first signal enqueues");
        // Same kind still pending → coalesced (no flood).
        assert!(
            t.on_signal("c1", TriggerSignal::FreshnessBreached)
                .is_none(),
            "second signal of the same kind (Recluster) while in flight is coalesced"
        );
    }

    #[test]
    fn re_enqueues_after_prior_job_terminal() {
        let registry = Arc::new(DiscoveryRegistry::new());
        let t = DiscoveryTrigger::new(registry.clone());

        let job = t.on_signal("c1", TriggerSignal::WorkloadDrift).unwrap();
        // Drive the prior job to terminal.
        let mut done = registry.get(&job.job_id).unwrap();
        done.status = DiscoveryJobStatus::Complete;
        registry.upsert(done);

        // A fresh signal now enqueues again.
        assert!(
            t.on_signal("c1", TriggerSignal::WorkloadDrift).is_some(),
            "after the prior job completes, a new signal enqueues again"
        );
    }

    #[test]
    fn distinct_collections_do_not_coalesce() {
        let t = trigger();
        assert!(t.on_signal("c1", TriggerSignal::RecallDegraded).is_some());
        assert!(
            t.on_signal("c2", TriggerSignal::RecallDegraded).is_some(),
            "a different collection gets its own job"
        );
    }
}
