//! The transport-agnostic **ledger service port** (S3) — a concurrency-safe, **tenant-scoped**
//! facade over any [`Ledger`]. This is the type a transport (gRPC/REST) wraps and shares as
//! `Arc<LedgerService<_>>`, mirroring how the fusion gRPC service holds `Arc<FusionService>`.
//!
//! It adds the one thing the raw ledger lacks and the co-design mandate requires: **fail-closed
//! tenant isolation.** Every scope and CAS key is namespaced by tenant, so one tenant can neither
//! read nor spend against another's budget. The underlying [`Ledger`] stays tenant-agnostic (neutral
//! counts keyed by an opaque string); tenancy is a property of this port, exactly where the request's
//! `TenantContext` will be threaded in when the gRPC layer lands (S3 slice 2).
//!
//! Interior-mutable (a `Mutex` around the ledger) so `&self` methods can be shared across async
//! handlers. The lock is the per-node serialization point — the same role the partition write lock
//! plays in the durable design (ADR-071). Generic over `L: Ledger`, so tests run against the fast
//! in-memory core and production wraps [`DurableLedger`](crate::DurableLedger).

use std::sync::Mutex;

use crate::Ledger;
use crate::types::{CasError, Nanos, Policy, ReserveOutcome, Version};

/// Namespace separator between tenant and scope/key. ASCII Unit Separator — not valid in the tenant
/// ids or scope names the gateway mints, so the composed key is unambiguous.
const SEP: char = '\u{1f}';

/// A concurrency-safe, tenant-scoped facade over a [`Ledger`].
pub struct LedgerService<L: Ledger> {
    inner: Mutex<L>,
}

impl<L: Ledger> LedgerService<L> {
    /// Wrap a ledger (in-memory for tests, [`DurableLedger`](crate::DurableLedger) in production).
    pub fn new(ledger: L) -> Self {
        Self {
            inner: Mutex::new(ledger),
        }
    }

    fn key(tenant: &str, scope: &str) -> String {
        let mut k = String::with_capacity(tenant.len() + 1 + scope.len());
        k.push_str(tenant);
        k.push(SEP);
        k.push_str(scope);
        k
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, L> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Configure a scope's cap + policy, isolated to `tenant`.
    pub fn set_limit(&self, tenant: &str, scope: &str, limit: Option<u64>, policy: Policy) {
        self.lock()
            .set_limit(&Self::key(tenant, scope), limit, policy);
    }

    /// Reserve a ceiling for `tenant`'s `scope`. The returned reservation/denial carries the caller's
    /// bare `scope` (the tenant namespacing is an internal detail).
    pub fn reserve(
        &self,
        tenant: &str,
        scope: &str,
        ceiling: u64,
        now_ns: Nanos,
        ttl_ns: Nanos,
    ) -> ReserveOutcome {
        let mut outcome = self
            .lock()
            .reserve(&Self::key(tenant, scope), ceiling, now_ns, ttl_ns);
        // Present the caller's own scope, not the namespaced key.
        match &mut outcome {
            ReserveOutcome::Admitted(r) => r.scope = scope.to_string(),
            ReserveOutcome::Denied(d) => d.scope = scope.to_string(),
        }
        outcome
    }

    /// Settle a reservation by id, **verified against `tenant`**. Returns `true` if the settle was
    /// applied (or was a harmless no-op for an unknown / already-settled / reclaimed id), and `false`
    /// if the reservation belongs to a *different* tenant — a cross-tenant settle attempt, refused.
    ///
    /// Closes the bearer-capability gap: a reservation's owning tenant is recovered from its
    /// namespaced scope (WAL-durable, so it survives a restart), so a leaked or guessed id cannot
    /// settle another tenant's lease. The lookup + settle are one locked (atomic) step.
    pub fn settle(&self, tenant: &str, reservation_id: u64, actual: u64) -> bool {
        let mut ledger = self.lock();
        match ledger.reservation_scope(reservation_id) {
            // A live reservation owned by another tenant — refuse without touching it.
            Some(scope) if !Self::scope_belongs_to(&scope, tenant) => false,
            // Owned by this tenant, or unknown (already settled / reclaimed → idempotent no-op).
            _ => {
                ledger.settle(reservation_id, actual);
                true
            }
        }
    }

    /// Does a namespaced `scope` belong to `tenant` — i.e. is it exactly `tenant` + [`SEP`] + …?
    fn scope_belongs_to(scope: &str, tenant: &str) -> bool {
        scope
            .strip_prefix(tenant)
            .and_then(|rest| rest.chars().next())
            == Some(SEP)
    }

    /// Reclaim expired leases across all tenants (the timed sweep — the caller/runtime schedules it).
    pub fn reclaim_expired(&self, now_ns: Nanos) -> usize {
        self.lock().reclaim_expired(now_ns)
    }

    /// Atomic conditional write on `tenant`'s `key` (feature flags / config / quotas).
    pub fn compare_and_swap(
        &self,
        tenant: &str,
        key: &str,
        expected: Option<Version>,
        new_value: i64,
    ) -> Result<Version, CasError> {
        self.lock()
            .compare_and_swap(&Self::key(tenant, key), expected, new_value)
            .map_err(|e| unscope_cas_error(e, tenant, key))
    }

    /// The configured cap for `tenant`'s `scope`.
    pub fn limit(&self, tenant: &str, scope: &str) -> Option<u64> {
        self.lock().limit(&Self::key(tenant, scope))
    }

    /// The configured policy for `tenant`'s `scope`.
    pub fn policy(&self, tenant: &str, scope: &str) -> Policy {
        self.lock().policy(&Self::key(tenant, scope))
    }

    /// Settled spend for `tenant`'s `scope`.
    pub fn spent(&self, tenant: &str, scope: &str) -> u64 {
        self.lock().spent(&Self::key(tenant, scope))
    }

    /// In-flight (reserved) units for `tenant`'s `scope`.
    pub fn reserved(&self, tenant: &str, scope: &str) -> u64 {
        self.lock().reserved(&Self::key(tenant, scope))
    }

    /// The `(version, value)` at `tenant`'s CAS `key`.
    pub fn get(&self, tenant: &str, key: &str) -> Option<(Version, i64)> {
        self.lock().get(&Self::key(tenant, key))
    }
}

/// Rewrite a namespaced CAS error's `key` back to the caller's bare key.
fn unscope_cas_error(error: CasError, _tenant: &str, key: &str) -> CasError {
    match error {
        CasError::VersionMismatch {
            key: _,
            expected,
            actual,
        } => CasError::VersionMismatch {
            key: key.to_string(),
            expected,
            actual,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryLedger;

    const TTL: Nanos = 60_000_000_000;
    const NOW: Nanos = 0;

    fn svc() -> LedgerService<InMemoryLedger> {
        LedgerService::new(InMemoryLedger::new())
    }
    fn admitted(o: ReserveOutcome) -> crate::Reservation {
        match o {
            ReserveOutcome::Admitted(r) => r,
            ReserveOutcome::Denied(d) => panic!("expected admit, got {d:?}"),
        }
    }

    #[test]
    fn tenants_are_isolated_on_the_same_scope_name() {
        let s = svc();
        // Same scope name "team", different tenants → independent caps + spend.
        s.set_limit("acme", "team", Some(100), Policy::Block);
        s.set_limit("globex", "team", Some(100), Policy::Block);

        let a = admitted(s.reserve("acme", "team", 90, NOW, TTL));
        assert!(s.settle("acme", a.id, 90));
        assert_eq!(s.spent("acme", "team"), 90);
        assert_eq!(
            s.spent("globex", "team"),
            0,
            "globex untouched by acme's spend"
        );

        // globex still has its full cap: a 90 reserve fits despite acme being near-full.
        assert!(matches!(
            s.reserve("globex", "team", 90, NOW, TTL),
            ReserveOutcome::Admitted(_)
        ));
        // acme is near-full: a 90 reserve does NOT fit (90 spent + 90 > 100).
        assert!(matches!(
            s.reserve("acme", "team", 90, NOW, TTL),
            ReserveOutcome::Denied(_)
        ));
    }

    #[test]
    fn returned_reservation_carries_the_bare_scope() {
        let s = svc();
        s.set_limit("acme", "team", Some(100), Policy::Block);
        let r = admitted(s.reserve("acme", "team", 10, NOW, TTL));
        assert_eq!(
            r.scope, "team",
            "caller sees its own scope, not the namespaced key"
        );
        let denied = s.reserve("acme", "team", 1_000, NOW, TTL);
        if let ReserveOutcome::Denied(d) = denied {
            assert_eq!(d.scope, "team");
        } else {
            panic!("expected denial");
        }
    }

    #[test]
    fn cas_keys_are_tenant_isolated() {
        let s = svc();
        // Same flag name, two tenants — independent versions/values.
        assert_eq!(s.compare_and_swap("acme", "flag", None, 1), Ok(1));
        assert_eq!(s.compare_and_swap("globex", "flag", None, 2), Ok(1));
        assert_eq!(s.get("acme", "flag"), Some((1, 1)));
        assert_eq!(s.get("globex", "flag"), Some((1, 2)));
        // acme's version-1 update must not be affected by globex.
        assert_eq!(s.compare_and_swap("acme", "flag", Some(1), 9), Ok(2));
        assert_eq!(s.get("acme", "flag"), Some((2, 9)));
        assert_eq!(
            s.get("globex", "flag"),
            Some((1, 2)),
            "globex flag unchanged"
        );
        // A CAS error reports the caller's bare key.
        assert!(matches!(
            s.compare_and_swap("acme", "flag", Some(1), 0),
            Err(CasError::VersionMismatch { key, .. }) if key == "flag"
        ));
    }

    #[test]
    fn reserve_settle_reclaim_round_trip() {
        let s = svc();
        s.set_limit("acme", "team", Some(100), Policy::Block);
        let r = admitted(s.reserve("acme", "team", 80, NOW, TTL));
        assert_eq!(s.reserved("acme", "team"), 80);
        // A crashed reserver's lease is swept by expiry.
        assert_eq!(s.reclaim_expired(TTL + 1), 1);
        assert_eq!(s.reserved("acme", "team"), 0);
        // Settling the (now reclaimed) lease is a harmless no-op — unknown id, returns true.
        assert!(s.settle("acme", r.id, 40));
        assert_eq!(s.spent("acme", "team"), 0);
    }

    #[test]
    fn cross_tenant_settle_is_refused() {
        // The bearer-capability gap, closed: a reservation id leaked/guessed by another tenant
        // cannot settle the owner's lease.
        let s = svc();
        s.set_limit("acme", "team", Some(100), Policy::Block);
        let r = admitted(s.reserve("acme", "team", 50, NOW, TTL));
        // globex presents acme's reservation id — refused, and acme's lease is untouched.
        assert!(!s.settle("globex", r.id, 50), "cross-tenant settle refused");
        assert_eq!(s.reserved("acme", "team"), 50, "acme's lease still held");
        assert_eq!(s.spent("acme", "team"), 0);
        // The rightful owner settles fine…
        assert!(s.settle("acme", r.id, 40));
        assert_eq!(s.spent("acme", "team"), 40);
        assert_eq!(s.reserved("acme", "team"), 0);
        // …and settling an unknown id is a harmless no-op (idempotent), reported as applied.
        assert!(s.settle("acme", 99_999, 10));
        assert_eq!(s.spent("acme", "team"), 40);
    }
}
