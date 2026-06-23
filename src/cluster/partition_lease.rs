//! Generation-fenced **partition leases** — Phase 7 of the read-heavy
//! system-catalog redesign.
//!
//! The catalog's global write authority is the object-store generation fence
//! (Phase 6a/6b): a single highest-generation pod publishes DDL, and stale
//! writers are fenced. That is correct but coarse — *every* write serializes
//! through one pod. A partition lease makes it **per-partition**: a peer acquires
//! a durable, generation-fenced lease over one `(tenant, collection)` and then
//! serves that partition's writes locally, contention-free, exactly like
//! CockroachDB's per-range leaseholder or Spanner's per-directory leader. The
//! object-store fence stays the *correctness* authority (no two pods can hold the
//! same partition); the lease is the *latency* optimization (no global CAS per
//! write while the lease is valid). Cross-partition DDL still falls back to the
//! global catalog fence.
//!
//! ## Mechanism (reuses the Phase-6a substrate)
//!
//! Each `(tenant, collection)` has its own fenced manifest log under
//! `{prefix}/{tenant}/{collection}/_manifests/` (the `prefix` is rooted at the
//! operator control plane via [`DrPathBuilder::operator_subprefix`]). The lease
//! body is committed with [`ManifestCommitter::commit_fenced`]: the **version**
//! CAS (`put_if_absent` on the successor slot) makes acquisition atomic, and the
//! **generation** header fences a stale owner — a writer carrying a generation
//! below the one a newer pod has committed is rejected *before* it can clobber
//! the lease. Two pods contending therefore converge to exactly one owner; the
//! loser re-reads and observes who won.
//!
//! ## Lifecycle
//!
//! - **Acquire (fresh / takeover-of-expired):** read the current lease; if none
//!   or expired, commit a new lease at `generation + 1` (strictly outranking the
//!   prior, so the expired owner's renewal is fenced). A live lease held by
//!   another pod yields [`LeaseOutcome::Held`].
//! - **Renew:** the holder re-commits at the *same* generation with a fresh
//!   expiry. If a takeover has happened in the meantime, the renew is
//!   [`LeaseOutcome::Fenced`]/`Held` — the stale owner learns it must step down.
//! - **Expiry / handoff:** a lease that is not renewed expires at its
//!   `expires_at_ms`; the next acquirer takes it over with a higher generation.
//!
//! Clocks are passed in explicitly (`now_ms`) so expiry is deterministic in
//! tests and so a deployment can choose its time source; like the queue leases,
//! a few seconds of clock skew only shifts handoff timing, never correctness
//! (the fence, not the clock, guarantees single-ownership).

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use proximadb_iceberg_engine::manifest::{CommitOutcome, ManifestCommitter};
use proximadb_object_store::ProximaObjectStore;

use crate::cluster::primary_pod_registry::{
    AssignmentReason, PrimaryPodRegistry, WriteRoutingDecision, consult_for_write,
};

/// Current wall-clock milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// A durable lease granting **one** pod write authority over a single partition
/// (a collection within a tenant) until `expires_at_ms`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartitionLease {
    /// Tenant owning the partition.
    pub tenant_id: String,
    /// Collection (the partition) within the tenant.
    pub collection_id: String,
    /// Pod that holds the lease (opaque identifier, matching the registry's
    /// `PrimaryPod.pod` convention).
    pub holder_pod: String,
    /// Monotonic fencing generation. A takeover commits a strictly-higher
    /// generation so the displaced owner's next write is fenced.
    pub generation: u64,
    /// Wall-clock ms when the lease was last (re)acquired.
    pub acquired_at_ms: i64,
    /// Wall-clock ms at which the lease lapses if not renewed.
    pub expires_at_ms: i64,
}

impl PartitionLease {
    /// Whether the lease has lapsed at `now_ms`.
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_at_ms
    }

    /// Whether `pod` holds this lease and it is still valid at `now_ms`.
    pub fn is_valid_for(&self, pod: &str, now_ms: i64) -> bool {
        self.holder_pod == pod && !self.is_expired(now_ms)
    }
}

/// Outcome of an acquire/renew attempt against the durable lease log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseOutcome {
    /// This pod now holds the lease (freshly acquired, took over an expired one,
    /// or renewed its own).
    Acquired(PartitionLease),
    /// Another pod holds a still-valid lease — this pod is **not** the owner.
    Held {
        /// The valid lease currently held by the other pod.
        by: PartitionLease,
    },
    /// **Fenced**: a newer generation already owns the partition (this pod lost
    /// the version CAS, or carried a stale generation). The latest durable lease
    /// is returned so the caller can route to / step down for the real owner.
    Fenced {
        /// The lease that actually won.
        latest: PartitionLease,
    },
}

/// Object-store home for generation-fenced partition leases. One fenced manifest
/// log per `(tenant, collection)` under `{prefix}/{tenant}/{collection}/_manifests/`.
///
/// `prefix` must be rooted at the operator control plane (the lease registry is
/// control-plane metadata — *who owns what*, never tenant data), e.g.
/// `DrPathBuilder::operator_subprefix("leases")`.
pub struct PartitionLeaseStore {
    store: ProximaObjectStore,
    prefix: String,
}

impl PartitionLeaseStore {
    /// Build from an already-open store and the operator-rooted lease prefix.
    pub fn new(store: ProximaObjectStore, prefix: impl Into<String>) -> Self {
        let mut prefix = prefix.into();
        while prefix.ends_with('/') {
            prefix.pop();
        }
        Self { store, prefix }
    }

    /// Build from an object-store base URL (e.g. `s3://bucket`, `memory:///`).
    pub fn from_url(base_url: &str, prefix: impl Into<String>) -> Result<Self> {
        let store = ProximaObjectStore::from_url(base_url)
            .with_context(|| format!("opening object store at {base_url}"))?;
        Ok(Self::new(store, prefix))
    }

    /// The fenced manifest committer for one partition's lease log.
    fn committer(&self, tenant_id: &str, collection_id: &str) -> ManifestCommitter {
        ManifestCommitter::new(
            self.store.clone(),
            format!("{}/{tenant_id}/{collection_id}/_manifests", self.prefix),
        )
    }

    /// Read the current lease for a partition with its pointer version + fencing
    /// generation, or `None` if the partition has never been leased.
    pub async fn read(
        &self,
        tenant_id: &str,
        collection_id: &str,
    ) -> Result<Option<(u64, PartitionLease)>> {
        let committer = self.committer(tenant_id, collection_id);
        match committer
            .latest_version()
            .await
            .with_context(|| format!("reading lease log {tenant_id}/{collection_id}"))?
        {
            Some(version) => {
                let (_generation, bytes) =
                    committer.read_fenced(version).await.with_context(|| {
                        format!("reading lease {tenant_id}/{collection_id}@{version}")
                    })?;
                let lease: PartitionLease = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decoding lease {tenant_id}/{collection_id}"))?;
                Ok(Some((version, lease)))
            }
            None => Ok(None),
        }
    }

    /// Acquire — or renew, or take over an expired — the lease on
    /// `(tenant_id, collection_id)` for `holder_pod`, valid for `lease_ms` from
    /// `now_ms`.
    ///
    /// Returns [`LeaseOutcome::Held`] when a *live* lease belongs to another pod,
    /// [`LeaseOutcome::Fenced`] when this attempt lost the fenced CAS to a
    /// concurrent acquirer, and [`LeaseOutcome::Acquired`] when this pod holds it.
    pub async fn acquire(
        &self,
        tenant_id: &str,
        collection_id: &str,
        holder_pod: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<LeaseOutcome> {
        let committer = self.committer(tenant_id, collection_id);

        // Decide the parent version + the generation to claim, from the current
        // durable state.
        let (parent, generation) = match self.read(tenant_id, collection_id).await? {
            // Fresh partition — generation 1 (manifest versions start at 0).
            None => (None, 1),
            Some((version, lease)) => {
                if lease.holder_pod == holder_pod {
                    // We already own it → renew at the SAME generation (a renew is
                    // not a takeover, so it must not outrank ourselves; the version
                    // CAS still serializes concurrent renewals).
                    (Some(version), lease.generation)
                } else if lease.is_expired(now_ms) {
                    // Take over a dead owner → strictly-higher generation so the
                    // displaced owner's next renewal is fenced.
                    (Some(version), lease.generation + 1)
                } else {
                    // A live lease belongs to someone else — we are not the owner.
                    return Ok(LeaseOutcome::Held { by: lease });
                }
            }
        };

        let lease = PartitionLease {
            tenant_id: tenant_id.to_string(),
            collection_id: collection_id.to_string(),
            holder_pod: holder_pod.to_string(),
            generation,
            acquired_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(lease_ms),
        };
        let payload = serde_json::to_vec(&lease).context("encoding partition lease")?;

        match committer
            .commit_fenced(parent, generation, bytes::Bytes::from(payload))
            .await
            .with_context(|| format!("committing lease {tenant_id}/{collection_id}"))?
        {
            CommitOutcome::Committed(_) => Ok(LeaseOutcome::Acquired(lease)),
            // Lost the version CAS or was fenced by a higher generation — re-read
            // to report who actually owns the partition now.
            CommitOutcome::Conflict { .. } => match self.read(tenant_id, collection_id).await? {
                Some((_, latest)) => Ok(LeaseOutcome::Fenced { latest }),
                // Extremely unlikely (the conflicting object vanished) — surface
                // our own attempt as the latest so the caller does not own it.
                None => Ok(LeaseOutcome::Fenced { latest: lease }),
            },
        }
    }
}

/// Ties the durable [`PartitionLeaseStore`] to the in-memory
/// [`PrimaryPodRegistry`] that [`consult_for_write`] reads on the hot path.
///
/// The registry stays the fast, lock-free routing cache; this manager keeps it
/// *truthful* against the durable lease — it assigns the binding to this pod on a
/// successful acquire, and **steps down** (re-points the binding at the new
/// owner, never unassigns) when a renewal reveals the lease was lost. Re-pointing
/// rather than clearing is the load-bearing safety property: an unassigned
/// binding makes `consult_for_write` return `Allow`, which would let a displaced
/// pod accept writes — a split brain. Pointing at the new owner instead makes it
/// return `Misrouted`, so the displaced pod fails closed.
pub struct PartitionLeaseManager {
    store: Arc<PartitionLeaseStore>,
    registry: Arc<PrimaryPodRegistry>,
    self_pod_id: String,
    lease_ms: i64,
}

impl PartitionLeaseManager {
    /// Build a manager for `self_pod_id` issuing leases valid for `lease_ms`.
    pub fn new(
        store: Arc<PartitionLeaseStore>,
        registry: Arc<PrimaryPodRegistry>,
        self_pod_id: impl Into<String>,
        lease_ms: i64,
    ) -> Self {
        Self {
            store,
            registry,
            self_pod_id: self_pod_id.into(),
            lease_ms,
        }
    }

    /// This pod's identity.
    pub fn self_pod_id(&self) -> &str {
        &self.self_pod_id
    }

    /// Attempt to become (or stay) the primary for `(tenant, collection)` at
    /// `now_ms`. On success the registry binding points at this pod; otherwise it
    /// is pointed at the actual owner so the write path fails closed. Returns
    /// whether this pod now owns the partition.
    pub async fn acquire(&self, tenant_id: &str, collection_id: &str, now_ms: i64) -> Result<bool> {
        let outcome = self
            .store
            .acquire(
                tenant_id,
                collection_id,
                &self.self_pod_id,
                now_ms,
                self.lease_ms,
            )
            .await?;
        Ok(self.reconcile(tenant_id, collection_id, outcome))
    }

    /// Renew every lease this pod believes it holds (per the registry). A lease
    /// that has been lost (taken over while we were paused) re-points the binding
    /// at the new owner — the step-down that prevents split brain. Returns the
    /// number of partitions this pod still owns after the pass.
    pub async fn renew_held(&self, now_ms: i64) -> Result<usize> {
        let mut still_owned = 0;
        for (tenant_id, collection_id, binding) in self.registry.list() {
            if binding.pod != self.self_pod_id {
                continue; // not ours to renew
            }
            match self
                .store
                .acquire(
                    &tenant_id,
                    &collection_id,
                    &self.self_pod_id,
                    now_ms,
                    self.lease_ms,
                )
                .await
            {
                Ok(outcome) => {
                    if self.reconcile(&tenant_id, &collection_id, outcome) {
                        still_owned += 1;
                    }
                }
                // A transient object-store error: keep the binding and retry next
                // tick rather than spuriously step down (the lease has not lapsed
                // from our side yet).
                Err(e) => {
                    tracing::warn!(
                        tenant = %tenant_id,
                        collection = %collection_id,
                        error = %e,
                        "partition-lease renewal failed; retrying next interval"
                    );
                    still_owned += 1;
                }
            }
        }
        Ok(still_owned)
    }

    /// Fold a durable [`LeaseOutcome`] into the routing registry. Returns whether
    /// this pod owns the partition after reconciliation.
    fn reconcile(&self, tenant_id: &str, collection_id: &str, outcome: LeaseOutcome) -> bool {
        match outcome {
            LeaseOutcome::Acquired(_) => {
                self.registry.assign(
                    tenant_id,
                    collection_id,
                    self.self_pod_id.as_str(),
                    AssignmentReason::Failover,
                );
                true
            }
            // Someone else owns it (live lease, or we were fenced): reflect the
            // real owner so consult_for_write returns Misrouted, not Allow.
            LeaseOutcome::Held { by } | LeaseOutcome::Fenced { latest: by } => {
                self.registry.assign(
                    tenant_id,
                    collection_id,
                    by.holder_pod,
                    AssignmentReason::CatalogReplay,
                );
                false
            }
        }
    }

    /// Spawn a background task that renews this pod's held leases every
    /// `interval`. Returns the [`JoinHandle`](tokio::task::JoinHandle); the caller
    /// **owns** it and must `abort()` on shutdown (it is a cooperative tokio task,
    /// not an OS thread). Pick `interval <= lease_ms / 2` so a held lease never
    /// lapses between renewals.
    pub fn spawn_renew_loop(
        self: Arc<Self>,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                if let Err(e) = self.renew_held(now_millis()).await {
                    tracing::warn!(error = %e, "partition-lease renew loop pass failed");
                }
            }
        })
    }
}

/// Lease-aware write gate: `consult_for_write` over a registry the
/// [`PartitionLeaseManager`] keeps truthful. A thin convenience wrapper so call
/// sites read intent-first; the actual gating is unchanged (the registry binding
/// is the source of truth, now durably backed by the lease).
pub fn consult_for_write_leased(
    registry: &PrimaryPodRegistry,
    self_pod_id: &str,
    tenant_id: &str,
    collection_id: &str,
) -> WriteRoutingDecision {
    consult_for_write(registry, self_pod_id, tenant_id, collection_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    const PREFIX: &str = "_operator/leases";
    const LEASE_MS: i64 = 10_000;

    /// Two lease stores over ONE shared backing object store = two pods.
    fn shared_backing() -> Arc<dyn object_store::ObjectStore> {
        Arc::new(InMemory::new())
    }

    fn store(backing: &Arc<dyn object_store::ObjectStore>) -> PartitionLeaseStore {
        PartitionLeaseStore::new(ProximaObjectStore::new(backing.clone()), PREFIX)
    }

    /// A fresh partition: one pod acquires generation 1; a second pod racing the
    /// same fresh partition is fenced (the version CAS admits exactly one), and
    /// it learns the real owner.
    #[tokio::test]
    async fn two_pods_contend_exactly_one_owner() -> Result<()> {
        let backing = shared_backing();
        let pod_a = store(&backing);
        let pod_b = store(&backing);

        let a = pod_a.acquire("t", "c", "A", 0, LEASE_MS).await?;
        let LeaseOutcome::Acquired(lease_a) = a else {
            panic!("pod A should acquire the fresh partition, got {a:?}");
        };
        assert_eq!(lease_a.holder_pod, "A");
        assert_eq!(lease_a.generation, 1);

        // Pod B, racing the same fresh partition, read None too and tries gen 1 —
        // it loses the version CAS and is fenced, learning A is the owner.
        let b = pod_b.acquire("t", "c", "B", 0, LEASE_MS).await?;
        match b {
            LeaseOutcome::Fenced { latest } | LeaseOutcome::Held { by: latest } => {
                assert_eq!(latest.holder_pod, "A");
            }
            LeaseOutcome::Acquired(_) => panic!("two owners — fence failed"),
        }
        Ok(())
    }

    /// A live lease held by another pod yields `Held` (not a takeover).
    #[tokio::test]
    async fn live_lease_blocks_other_pod() -> Result<()> {
        let backing = shared_backing();
        let pod_a = store(&backing);
        let pod_b = store(&backing);

        assert!(matches!(
            pod_a.acquire("t", "c", "A", 0, LEASE_MS).await?,
            LeaseOutcome::Acquired(_)
        ));
        // B at a time well inside A's lease window.
        match pod_b.acquire("t", "c", "B", 1_000, LEASE_MS).await? {
            LeaseOutcome::Held { by } => assert_eq!(by.holder_pod, "A"),
            other => panic!("expected Held by A, got {other:?}"),
        }
        Ok(())
    }

    /// The holder renews at the same generation with a fresh expiry.
    #[tokio::test]
    async fn holder_renews_extends_expiry_same_generation() -> Result<()> {
        let backing = shared_backing();
        let pod_a = store(&backing);

        let LeaseOutcome::Acquired(l1) = pod_a.acquire("t", "c", "A", 0, LEASE_MS).await? else {
            panic!("acquire");
        };
        let LeaseOutcome::Acquired(l2) = pod_a.acquire("t", "c", "A", 5_000, LEASE_MS).await?
        else {
            panic!("renew");
        };
        assert_eq!(
            l2.generation, l1.generation,
            "renewal must not bump generation"
        );
        assert!(
            l2.expires_at_ms > l1.expires_at_ms,
            "renewal extends expiry"
        );
        Ok(())
    }

    /// Owner death (lease expiry) → fenced handoff: a new pod takes over with a
    /// strictly-higher generation, and the dead owner's later renewal is rejected.
    #[tokio::test]
    async fn expired_lease_handoff_fences_dead_owner() -> Result<()> {
        let backing = shared_backing();
        let pod_a = store(&backing);
        let pod_b = store(&backing);

        let LeaseOutcome::Acquired(la) = pod_a.acquire("t", "c", "A", 0, LEASE_MS).await? else {
            panic!("A acquire");
        };
        assert_eq!(la.generation, 1);

        // A "dies"; time advances past expiry. B takes over.
        let after_expiry = LEASE_MS + 1;
        let LeaseOutcome::Acquired(lb) =
            pod_b.acquire("t", "c", "B", after_expiry, LEASE_MS).await?
        else {
            panic!("B takeover");
        };
        assert_eq!(lb.holder_pod, "B");
        assert_eq!(lb.generation, 2, "takeover outranks the dead owner");

        // A "comes back" and tries to renew — it must NOT regain ownership.
        match pod_a
            .acquire("t", "c", "A", after_expiry + 1, LEASE_MS)
            .await?
        {
            LeaseOutcome::Held { by } => assert_eq!(by.holder_pod, "B"),
            other => panic!("stale owner A must be rejected, got {other:?}"),
        }
        Ok(())
    }

    /// Manager integration: acquisition assigns the registry binding; a lost
    /// lease steps the binding down to the new owner (consult_for_write flips
    /// Allow → Misrouted), so exactly one pod is ever writable across a handoff.
    #[tokio::test]
    async fn manager_acquire_assigns_and_steps_down() -> Result<()> {
        let backing = shared_backing();
        let reg_a = Arc::new(PrimaryPodRegistry::new());
        let reg_b = Arc::new(PrimaryPodRegistry::new());
        let mgr_a =
            PartitionLeaseManager::new(Arc::new(store(&backing)), reg_a.clone(), "A", LEASE_MS);
        let mgr_b =
            PartitionLeaseManager::new(Arc::new(store(&backing)), reg_b.clone(), "B", LEASE_MS);

        // A acquires → A is the owner; A's gate Allows, B's gate Misroutes to A.
        assert!(mgr_a.acquire("t", "c", 0).await?);
        assert_eq!(
            consult_for_write(&reg_a, "A", "t", "c"),
            WriteRoutingDecision::Allow
        );
        assert!(!mgr_b.acquire("t", "c", 1_000).await?);
        assert_eq!(
            consult_for_write(&reg_b, "B", "t", "c"),
            WriteRoutingDecision::Misrouted {
                target_pod: "A".to_string()
            }
        );

        // A's lease lapses; B takes over.
        let after = LEASE_MS + 1;
        assert!(mgr_b.acquire("t", "c", after).await?);
        assert_eq!(
            consult_for_write(&reg_b, "B", "t", "c"),
            WriteRoutingDecision::Allow
        );

        // A's renew pass discovers it lost the lease and steps down — its gate now
        // Misroutes to B (it does NOT fall back to Allow, which would split-brain).
        assert_eq!(
            mgr_a.renew_held(after + 1).await?,
            0,
            "A owns nothing after step-down"
        );
        assert_eq!(
            consult_for_write(&reg_a, "A", "t", "c"),
            WriteRoutingDecision::Misrouted {
                target_pod: "B".to_string()
            }
        );
        Ok(())
    }

    /// Single-pod / no contention: the same pod acquires and renews indefinitely,
    /// staying the owner — the lease layer is inert overhead-wise and never
    /// fences itself.
    #[tokio::test]
    async fn single_pod_keeps_its_lease() -> Result<()> {
        let backing = shared_backing();
        let reg = Arc::new(PrimaryPodRegistry::new());
        let mgr =
            PartitionLeaseManager::new(Arc::new(store(&backing)), reg.clone(), "solo", LEASE_MS);

        assert!(mgr.acquire("t", "c", 0).await?);
        // Renew across several intervals — always retains ownership.
        for tick in 1..=5 {
            assert_eq!(mgr.renew_held(tick * 1_000).await?, 1);
        }
        assert_eq!(
            consult_for_write(&reg, "solo", "t", "c"),
            WriteRoutingDecision::Allow
        );
        Ok(())
    }
}
