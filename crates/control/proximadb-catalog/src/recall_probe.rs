// Recall probe gate — LLD §5 rollout safety.
//
// The quantized candidate route (QuIVer / 2-bit Sign-Magnitude) is recall-
// sensitive: it works beautifully on cosine-native contrastive embeddings
// but degrades on multimodal CLIP-style spaces (78% Recall@10 at ef=64 in
// the paper). Defaulting it on for every tenant would be a customer-visible
// recall regression on the wrong corpus.
//
// This module implements the LLD-mandated gate: enable the quantized route
// by default *only* after the recall-probe set passes the tenant's target
// for **three consecutive builds**. A single failure resets the streak.
//
// The state lives in a `RecallProbeGate` keyed on `(tenant_id, collection)`.
// Phase 5's stats refresher persists it; Phase 4 ships the pure state
// machine so tests can pin the gate's behavior on the synthetic probe set.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// One probe outcome: pass / fail relative to the configured recall target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeOutcome {
    Pass,
    Fail,
}

/// Per-collection probe state. Tracks the consecutive-pass streak and the
/// resulting gate state so the runtime can read a single field per request.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeState {
    /// Consecutive passes observed at the most recent build. Resets to
    /// zero on any failure. Caps at `passes_required` — additional passes
    /// after the gate opens are tracked separately so we don't overflow.
    pub consecutive_passes: u32,
    /// Whether the gate is currently open (route may default on).
    pub gate_open: bool,
    /// Total number of probes observed (for observability).
    pub total_observations: u64,
    /// Total number of failures observed (for observability).
    pub total_failures: u64,
}

/// Tunable knobs.
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    /// Required consecutive passes before the gate opens. LLD §5 default is 3.
    pub passes_required: u32,
    /// Whether the collection is allowed to participate in the gate at all.
    /// Multimodal / non-cosine collections set this to `false` and the gate
    /// stays closed regardless of probe outcome (LLD §5 explicit guardrail).
    pub eligible: bool,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            passes_required: 3,
            eligible: true,
        }
    }
}

/// Unique identifier for a probe scope — typically tenant + collection.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProbeScope {
    pub tenant_id: String,
    pub collection: String,
}

impl ProbeScope {
    pub fn new(tenant_id: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            collection: collection.into(),
        }
    }
}

/// In-memory gate. Cheap to clone (wraps an `Arc<RwLock<…>>`). Phase 5 will
/// hydrate the state from the metadata store on startup.
#[derive(Clone)]
pub struct RecallProbeGate {
    inner: Arc<RwLock<HashMap<ProbeScope, (ProbeConfig, ProbeState)>>>,
}

impl RecallProbeGate {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a scope with custom config. Idempotent — re-registering an
    /// existing scope updates only the config, preserving the state.
    pub async fn register(&self, scope: ProbeScope, config: ProbeConfig) {
        let mut g = self.inner.write().await;
        let entry = g
            .entry(scope)
            .or_insert_with(|| (config, ProbeState::default()));
        entry.0 = config;
        // If the collection was just marked ineligible, force the gate closed
        // so a stale "open" state can't leak through.
        if !config.eligible {
            entry.1.gate_open = false;
            entry.1.consecutive_passes = 0;
        }
    }

    /// Record a probe outcome. Returns the new state for callers that want
    /// to log the transition without re-reading.
    pub async fn observe(&self, scope: &ProbeScope, outcome: ProbeOutcome) -> ProbeState {
        let mut g = self.inner.write().await;
        let entry = g
            .entry(scope.clone())
            .or_insert_with(|| (ProbeConfig::default(), ProbeState::default()));
        let (config, state) = entry;
        state.total_observations += 1;
        if !config.eligible {
            // Ineligible collections never open the gate. We still count
            // observations so dashboards can see something is being probed.
            state.gate_open = false;
            state.consecutive_passes = 0;
            if outcome == ProbeOutcome::Fail {
                state.total_failures += 1;
            }
            return state.clone();
        }
        match outcome {
            ProbeOutcome::Pass => {
                if state.consecutive_passes < config.passes_required {
                    state.consecutive_passes += 1;
                }
                if state.consecutive_passes >= config.passes_required {
                    state.gate_open = true;
                }
            }
            ProbeOutcome::Fail => {
                state.consecutive_passes = 0;
                state.gate_open = false;
                state.total_failures += 1;
            }
        }
        state.clone()
    }

    /// Read the current gate state. Returns the default-closed state when
    /// the scope has never been registered or observed — the safe choice.
    pub async fn is_open(&self, scope: &ProbeScope) -> bool {
        self.inner
            .read()
            .await
            .get(scope)
            .map(|(_, s)| s.gate_open)
            .unwrap_or(false)
    }

    /// Snapshot the entire state for observability / persistence.
    pub async fn snapshot(&self) -> HashMap<ProbeScope, ProbeState> {
        self.inner
            .read()
            .await
            .iter()
            .map(|(k, (_, v))| (k.clone(), v.clone()))
            .collect()
    }
}

impl Default for RecallProbeGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ProbeScope {
        ProbeScope::new("tenant-a", "knowledge")
    }

    #[tokio::test]
    async fn unregistered_scope_is_closed_by_default() {
        let gate = RecallProbeGate::new();
        assert!(!gate.is_open(&scope()).await);
    }

    #[tokio::test]
    async fn three_consecutive_passes_open_the_gate() {
        let gate = RecallProbeGate::new();
        let s = scope();
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(!gate.is_open(&s).await);
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(!gate.is_open(&s).await);
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(gate.is_open(&s).await, "gate should open after 3rd pass");
    }

    #[tokio::test]
    async fn any_failure_closes_the_gate_and_resets_streak() {
        let gate = RecallProbeGate::new();
        let s = scope();
        for _ in 0..3 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        assert!(gate.is_open(&s).await);
        gate.observe(&s, ProbeOutcome::Fail).await;
        assert!(
            !gate.is_open(&s).await,
            "single failure must close the gate"
        );
        // After failure, need another 3 consecutive passes.
        gate.observe(&s, ProbeOutcome::Pass).await;
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(!gate.is_open(&s).await);
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(gate.is_open(&s).await);
    }

    #[tokio::test]
    async fn ineligible_scope_never_opens_even_after_passes() {
        let gate = RecallProbeGate::new();
        let s = scope();
        gate.register(
            s.clone(),
            ProbeConfig {
                passes_required: 1,
                eligible: false,
            },
        )
        .await;
        for _ in 0..10 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        assert!(!gate.is_open(&s).await, "ineligible scope must stay closed");
    }

    #[tokio::test]
    async fn marking_open_scope_ineligible_forces_close() {
        let gate = RecallProbeGate::new();
        let s = scope();
        for _ in 0..3 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        assert!(gate.is_open(&s).await);
        gate.register(
            s.clone(),
            ProbeConfig {
                passes_required: 3,
                eligible: false,
            },
        )
        .await;
        assert!(
            !gate.is_open(&s).await,
            "ineligibility must close an open gate"
        );
    }

    #[tokio::test]
    async fn observation_counters_track_both_pass_and_fail() {
        let gate = RecallProbeGate::new();
        let s = scope();
        gate.observe(&s, ProbeOutcome::Pass).await;
        gate.observe(&s, ProbeOutcome::Fail).await;
        gate.observe(&s, ProbeOutcome::Pass).await;
        let snap = gate.snapshot().await;
        let state = snap.get(&s).expect("present");
        assert_eq!(state.total_observations, 3);
        assert_eq!(state.total_failures, 1);
    }

    #[tokio::test]
    async fn custom_passes_required_works() {
        let gate = RecallProbeGate::new();
        let s = scope();
        gate.register(
            s.clone(),
            ProbeConfig {
                passes_required: 5,
                eligible: true,
            },
        )
        .await;
        for _ in 0..4 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        assert!(!gate.is_open(&s).await);
        gate.observe(&s, ProbeOutcome::Pass).await;
        assert!(
            gate.is_open(&s).await,
            "gate should open on 5th pass with custom config"
        );
    }

    #[tokio::test]
    async fn different_scopes_are_independent() {
        let gate = RecallProbeGate::new();
        let a = ProbeScope::new("tenant-a", "kb");
        let b = ProbeScope::new("tenant-b", "kb");
        for _ in 0..3 {
            gate.observe(&a, ProbeOutcome::Pass).await;
        }
        gate.observe(&b, ProbeOutcome::Fail).await;
        assert!(gate.is_open(&a).await);
        assert!(!gate.is_open(&b).await);
    }

    #[tokio::test]
    async fn observation_after_open_does_not_re_increment_streak_overflow() {
        let gate = RecallProbeGate::new();
        let s = scope();
        for _ in 0..3 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        // Many additional passes — `consecutive_passes` must cap at the
        // required value, not grow unboundedly.
        for _ in 0..10_000 {
            gate.observe(&s, ProbeOutcome::Pass).await;
        }
        let snap = gate.snapshot().await;
        let state = snap.get(&s).expect("present");
        assert!(
            state.consecutive_passes <= 3,
            "streak must cap at passes_required"
        );
        assert!(state.gate_open);
        assert_eq!(state.total_observations, 10_003);
    }
}
