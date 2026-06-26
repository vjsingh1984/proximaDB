//! A6 storage-write fence seam.
//!
//! The fence rejects a stale pod's vector flush at the live storage-write
//! boundary: after a lease takeover, a displaced pod that still flushes buffered
//! data to shared storage would durably resurrect deleted/tombstoned records
//! (CLAUDE.md #16; ADR-025 deletion vectors are a dependent of A6). Lease-on-write
//! (#337) closes the *routing* half; this fence is the *boundary* backstop.
//!
//! ## Why a trait here (layering)
//! The ground-truth fence decision lives on
//! [`crate::cluster::partition_lease::PartitionLeaseManager::is_fenced_out`] (#346),
//! but `FlushParameters` is `Serialize`/`Deserialize` so it cannot carry an
//! `Arc<PartitionLeaseManager>`, and `storage` must not depend upward on
//! `cluster`. So the flush path (storage layer) holds an
//! `Option<Arc<dyn StorageWriteFence>>`, and the concrete adapter over the lease
//! manager is constructed in the `network` layer (`shared_services`), which
//! already depends on both. The flush component receives the **same**
//! `PartitionLeaseManager` instance the network write-gates use — never a
//! throwaway — so its view of ownership is consistent across the pod.
//!
//! ## Default-OFF (ship-dark, D5)
//! Enforcement is gated by [`write_fencing_enabled`] (`PROXIMADB_WRITE_FENCING=1`).
//! With the gate off — the default — the fence is never consulted and ingest is
//! byte-identical to the pre-fence path. Fail-open is the bootstrap posture: a
//! missing fence/tenant or a durable-read error proceeds (the routing layer is the
//! first line; this is the backstop).

use async_trait::async_trait;
use std::sync::Arc;

/// A6 storage-write fence: is THIS pod fenced out of writing `(tenant,
/// collection)` to shared storage right now?
///
/// Implemented by an adapter over the cluster `PartitionLeaseManager` and injected
/// into the flush path from `shared_services`. Contract mirrors
/// `PartitionLeaseManager::is_fenced_out`: return `true` iff a *live* lease (not
/// released, not expired) is held by a **different** pod; fail-open (`false`) on
/// any uncertainty.
#[async_trait]
pub trait StorageWriteFence: Send + Sync {
    /// `true` ⇒ reject this pod's flush of `(tenant_id, collection_id)` at
    /// `now_ms`; `false` ⇒ allow it to proceed.
    async fn is_fenced_out(&self, tenant_id: &str, collection_id: &str, now_ms: i64) -> bool;
}

/// The A6 fence is enforced only when `PROXIMADB_WRITE_FENCING == "1"`.
///
/// Default-OFF (D5): any other value — including unset — leaves the fence dark, so
/// the flush path behaves exactly as before. Read at the flush call site (cheap;
/// once per flush, never per row) and passed explicitly into the decision so the
/// decision itself stays deterministic and unit-testable without touching process
/// env (CLAUDE.md #13).
pub fn write_fencing_enabled() -> bool {
    std::env::var("PROXIMADB_WRITE_FENCING")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Outcome of the boundary fence check at a flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceDecision {
    /// Allow the flush to proceed to the storage write.
    Proceed,
    /// Reject the flush: this pod is fenced out of `(tenant, collection)`.
    Fenced,
}

/// Pure, deterministic boundary decision used by the flush coordinator.
///
/// Separated from env/I/O so it can be unit-tested directly (CLAUDE.md #13):
/// `enabled` is passed in (call site reads [`write_fencing_enabled`]), and the
/// durable lease read is the injected `fence`. Fail-open whenever enforcement is
/// off, no fence is wired, or the tenant could not be resolved — the routing layer
/// (lease-on-write) remains the first line of defense.
pub async fn evaluate_fence(
    enabled: bool,
    fence: Option<&Arc<dyn StorageWriteFence>>,
    tenant_id: Option<&str>,
    collection_id: &str,
    now_ms: i64,
) -> FenceDecision {
    if !enabled {
        return FenceDecision::Proceed;
    }
    let (Some(fence), Some(tenant_id)) = (fence, tenant_id) else {
        return FenceDecision::Proceed;
    };
    if fence.is_fenced_out(tenant_id, collection_id, now_ms).await {
        FenceDecision::Fenced
    } else {
        FenceDecision::Proceed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic stub fence — returns a fixed verdict, no lease infra.
    struct StubFence(bool);

    #[async_trait]
    impl StorageWriteFence for StubFence {
        async fn is_fenced_out(&self, _tenant: &str, _collection: &str, _now_ms: i64) -> bool {
            self.0
        }
    }

    fn fence(verdict: bool) -> Arc<dyn StorageWriteFence> {
        Arc::new(StubFence(verdict))
    }

    /// Default-OFF: with enforcement disabled the fence is never consulted, even
    /// when it would report fenced — ingest is byte-identical to the pre-fence path.
    #[tokio::test]
    async fn disabled_always_proceeds() {
        let f = fence(true);
        assert_eq!(
            evaluate_fence(false, Some(&f), Some("t"), "c", 0).await,
            FenceDecision::Proceed
        );
    }

    /// Enabled + fenced ⇒ reject (the stale-pod case).
    #[tokio::test]
    async fn enabled_and_fenced_rejects() {
        let f = fence(true);
        assert_eq!(
            evaluate_fence(true, Some(&f), Some("t"), "c", 0).await,
            FenceDecision::Fenced
        );
    }

    /// Enabled + holder (not fenced) ⇒ proceed.
    #[tokio::test]
    async fn enabled_not_fenced_proceeds() {
        let f = fence(false);
        assert_eq!(
            evaluate_fence(true, Some(&f), Some("t"), "c", 0).await,
            FenceDecision::Proceed
        );
    }

    /// Fail-open when the tenant could not be resolved, even if enabled + a fence
    /// is wired (the routing layer remains the first line of defense).
    #[tokio::test]
    async fn missing_tenant_fails_open() {
        let f = fence(true);
        assert_eq!(
            evaluate_fence(true, Some(&f), None, "c", 0).await,
            FenceDecision::Proceed
        );
    }

    /// Fail-open when no fence is wired (lease store unavailable).
    #[tokio::test]
    async fn missing_fence_fails_open() {
        assert_eq!(
            evaluate_fence(true, None, Some("t"), "c", 0).await,
            FenceDecision::Proceed
        );
    }
}
