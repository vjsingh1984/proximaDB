//! In-memory reference implementation of the [`Ledger`](crate::Ledger) contract.
//!
//! Correct under single-owner (`&mut self`) access; concurrency is provided by the caller holding a
//! lock (a `Mutex` in tests; the partition write lock in production). This is the reference the
//! durable partition-WAL backend (S2/S3) is checked against.

use std::collections::HashMap;

use crate::Ledger;
use crate::types::{CasError, Denied, Nanos, Policy, Reservation, ReserveOutcome, Version};

#[derive(Debug, Clone, Copy)]
struct LimitSpec {
    limit: Option<u64>,
    policy: Policy,
}

/// In-memory reference ledger. Zero dependencies; deterministic (the clock is always passed in).
#[derive(Debug, Default)]
pub struct InMemoryLedger {
    limits: HashMap<String, LimitSpec>,
    spent: HashMap<String, u64>,
    reserved: HashMap<String, u64>,
    leases: HashMap<u64, Reservation>,
    next_id: u64,
    /// Generic compare-and-swap keyspace: key -> (version, value). Disjoint from the scope counters.
    kv: HashMap<String, (Version, i64)>,
}

impl InMemoryLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of currently-held (unsettled, unreclaimed) leases — for tests/introspection.
    #[must_use]
    pub fn active_leases(&self) -> usize {
        self.leases.len()
    }

    fn add_reserved(&mut self, scope: &str, delta: u64) {
        *self.reserved.entry(scope.to_string()).or_insert(0) += delta;
    }

    fn sub_reserved(&mut self, scope: &str, delta: u64) {
        let held = self.reserved.entry(scope.to_string()).or_insert(0);
        *held = held.saturating_sub(delta);
    }

    // --- check / apply seams --------------------------------------------------
    //
    // Splitting the *decision* (`check_*`) from the *mutation* (`apply_*` / `peek_next_id`) lets a
    // durable backend log the decided record and only then apply it (WAL-before-state), while the
    // in-memory `reserve`/`compare_and_swap` above stay a check-then-apply with no logic duplicated.
    // `apply_*` are also the replay path: reconstructing state from a log of decided records.

    /// The admission decision for a `Block` cap (pure — no mutation). `Ok(())` = admit.
    pub(crate) fn check_admit(&self, scope: &str, ceiling: u64) -> Result<(), Denied> {
        if let Some(spec) = self.limits.get(scope).copied()
            && spec.policy == Policy::Block
            && let Some(limit) = spec.limit
        {
            let spent = self.spent(scope);
            let reserved = self.reserved(scope);
            if spent.saturating_add(reserved).saturating_add(ceiling) > limit {
                return Err(Denied {
                    scope: scope.to_string(),
                    limit,
                    spent,
                    reserved,
                    requested_ceiling: ceiling,
                });
            }
        }
        Ok(())
    }

    /// The id the next reservation will take (without consuming it). `apply_reserve` advances past it.
    pub(crate) fn peek_next_id(&self) -> u64 {
        self.next_id
    }

    /// Apply an already-decided reservation (from `reserve`, or from a log record on replay). Bumps
    /// `next_id` past `r.id` so a replayed id and a live id can never collide.
    pub(crate) fn apply_reserve(&mut self, r: Reservation) {
        self.next_id = self.next_id.max(r.id.saturating_add(1));
        self.add_reserved(&r.scope, r.ceiling);
        self.leases.insert(r.id, r);
    }

    /// The version-match decision for a CAS (pure). `Ok(v)` = the version the write would take.
    pub(crate) fn check_cas(
        &self,
        key: &str,
        expected: Option<Version>,
    ) -> Result<Version, CasError> {
        let actual = self.kv.get(key).map(|(v, _)| *v);
        if actual != expected {
            return Err(CasError::VersionMismatch {
                key: key.to_string(),
                expected,
                actual,
            });
        }
        Ok(actual.unwrap_or(0).saturating_add(1))
    }

    /// Apply an already-decided CAS write (from `compare_and_swap`, or a log record on replay).
    pub(crate) fn apply_cas(&mut self, key: &str, version: Version, value: i64) {
        self.kv.insert(key.to_string(), (version, value));
    }
}

impl Ledger for InMemoryLedger {
    fn set_limit(&mut self, scope: &str, limit: Option<u64>, policy: Policy) {
        self.limits
            .insert(scope.to_string(), LimitSpec { limit, policy });
    }

    fn reserve(
        &mut self,
        scope: &str,
        ceiling: u64,
        now_ns: Nanos,
        ttl_ns: Nanos,
    ) -> ReserveOutcome {
        if let Err(denied) = self.check_admit(scope, ceiling) {
            return ReserveOutcome::Denied(denied);
        }
        let reservation = Reservation {
            id: self.peek_next_id(),
            scope: scope.to_string(),
            ceiling,
            expires_at_ns: now_ns.saturating_add(ttl_ns),
        };
        self.apply_reserve(reservation.clone());
        ReserveOutcome::Admitted(reservation)
    }

    fn settle(&mut self, reservation_id: u64, actual: u64) {
        let Some(lease) = self.leases.remove(&reservation_id) else {
            return; // unknown / already settled / reclaimed — idempotent no-op (C3).
        };
        self.sub_reserved(&lease.scope, lease.ceiling);
        *self.spent.entry(lease.scope).or_insert(0) += actual;
    }

    fn reclaim_expired(&mut self, now_ns: Nanos) -> usize {
        let expired: Vec<u64> = self
            .leases
            .iter()
            .filter(|(_, l)| l.expires_at_ns <= now_ns)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            if let Some(lease) = self.leases.remove(id) {
                self.sub_reserved(&lease.scope, lease.ceiling);
            }
        }
        expired.len()
    }

    fn compare_and_swap(
        &mut self,
        key: &str,
        expected: Option<Version>,
        new_value: i64,
    ) -> Result<Version, CasError> {
        let version = self.check_cas(key, expected)?;
        self.apply_cas(key, version, new_value);
        Ok(version)
    }

    fn limit(&self, scope: &str) -> Option<u64> {
        self.limits.get(scope).and_then(|s| s.limit)
    }

    fn policy(&self, scope: &str) -> Policy {
        self.limits.get(scope).map(|s| s.policy).unwrap_or_default()
    }

    fn spent(&self, scope: &str) -> u64 {
        self.spent.get(scope).copied().unwrap_or(0)
    }

    fn reserved(&self, scope: &str) -> u64 {
        self.reserved.get(scope).copied().unwrap_or(0)
    }

    fn get(&self, key: &str) -> Option<(Version, i64)> {
        self.kv.get(key).copied()
    }

    fn reservation_scope(&self, reservation_id: u64) -> Option<String> {
        self.leases.get(&reservation_id).map(|r| r.scope.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    const TTL: Nanos = 60_000_000_000; // 60s in ns
    const NOW: Nanos = 0;

    fn admit(l: &mut InMemoryLedger, scope: &str, ceiling: u64) -> Reservation {
        match l.reserve(scope, ceiling, NOW, TTL) {
            ReserveOutcome::Admitted(r) => r,
            ReserveOutcome::Denied(d) => panic!("expected admit, got denied: {d:?}"),
        }
    }
    fn denied(l: &mut InMemoryLedger, scope: &str, ceiling: u64) -> bool {
        matches!(
            l.reserve(scope, ceiling, NOW, TTL),
            ReserveOutcome::Denied(_)
        )
    }

    // --- C1: atomic conditional admit ---------------------------------------

    #[test]
    fn block_cap_prevents_overshoot() {
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Block);
        let r = admit(&mut l, "g", 100);
        // A near-full cap admits nothing more — even 1 unit — before any usage is known.
        assert!(denied(&mut l, "g", 1));
        // Real usage came in under the ceiling; settle frees the difference.
        l.settle(r.id, 40);
        assert_eq!(l.spent("g"), 40);
        assert_eq!(l.reserved("g"), 0);
        assert_eq!(l.available("g"), 60);
        assert!(
            l.spent("g") + l.reserved("g") <= 100,
            "invariant holds throughout"
        );
    }

    #[test]
    fn warn_scope_admits_over_cap_but_tracks_spend() {
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Warn);
        let a = admit(&mut l, "g", 80);
        l.settle(a.id, 80);
        // 80 spent, cap 100 — an 80 ceiling would breach it, but a soft cap admits anyway.
        let b = admit(&mut l, "g", 80);
        l.settle(b.id, 80);
        assert_eq!(l.spent("g"), 160, "spend accrues past a warn cap");
    }

    #[test]
    fn unset_scope_is_unlimited_but_tracked() {
        let mut l = InMemoryLedger::new();
        assert_eq!(l.available("free"), u64::MAX);
        let r = admit(&mut l, "free", 1_000_000);
        assert_eq!(l.reserved("free"), 1_000_000);
        l.settle(r.id, 999);
        assert_eq!(l.spent("free"), 999);
    }

    #[test]
    fn concurrent_reservations_cannot_oversubscribe() {
        // The load-bearing C1 test: 100 threads race to reserve 100 units each against a 1000 cap.
        // The `Mutex` models the partition write lock that serializes per-scope ops in production;
        // exactly ten fit, and the held total never breaches the cap.
        let ledger = Arc::new(Mutex::new(InMemoryLedger::new()));
        ledger
            .lock()
            .unwrap()
            .set_limit("g", Some(1000), Policy::Block);

        let handles: Vec<_> = (0..100)
            .map(|_| {
                let l = Arc::clone(&ledger);
                thread::spawn(move || {
                    matches!(
                        l.lock().unwrap().reserve("g", 100, NOW, TTL),
                        ReserveOutcome::Admitted(_)
                    )
                })
            })
            .collect();

        let admits: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        let guard = ledger.lock().unwrap();
        assert_eq!(admits, 10, "exactly 1000/100 reservations fit");
        assert_eq!(guard.reserved("g"), 1000);
        assert!(guard.reserved("g") <= 1000, "never oversubscribed");
    }

    // --- C2: TTL leases -----------------------------------------------------

    #[test]
    fn expired_lease_is_reclaimed_without_reading_the_scope() {
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Block);
        let _crashed = admit(&mut l, "g", 80); // reserver "crashes" — never settles
        assert_eq!(l.available("g"), 20);
        // Not yet expired.
        assert_eq!(l.reclaim_expired(NOW), 0);
        assert_eq!(l.available("g"), 20);
        // Past the TTL → reclaimed on a timed basis, capacity restored — no read touched the scope.
        assert_eq!(l.reclaim_expired(TTL + 1), 1);
        assert_eq!(l.reserved("g"), 0);
        assert_eq!(l.available("g"), 100);
        assert_eq!(l.active_leases(), 0);
    }

    #[test]
    fn settle_after_reclaim_is_a_noop() {
        // Documents the TTL tradeoff: a settle arriving after reclaim is dropped (the lease is gone).
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Block);
        let r = admit(&mut l, "g", 50);
        assert_eq!(l.reclaim_expired(TTL + 1), 1);
        l.settle(r.id, 40); // too late
        assert_eq!(l.spent("g"), 0);
        assert_eq!(l.reserved("g"), 0);
    }

    // --- C3: idempotent settle ----------------------------------------------

    #[test]
    fn settle_is_idempotent_under_repeat() {
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Block);
        let r = admit(&mut l, "g", 50);
        l.settle(r.id, 40);
        l.settle(r.id, 40); // at-least-once repeat
        l.settle(r.id, 999); // a replay must not overwrite or double-count
        assert_eq!(l.spent("g"), 40);
        assert_eq!(l.reserved("g"), 0);
    }

    #[test]
    fn zero_settle_releases_without_recording_spend() {
        let mut l = InMemoryLedger::new();
        l.set_limit("g", Some(100), Policy::Block);
        let r = admit(&mut l, "g", 50);
        assert_eq!(l.reserved("g"), 50);
        l.settle(r.id, 0); // the failed/cancelled path
        assert_eq!(l.reserved("g"), 0);
        assert_eq!(l.spent("g"), 0);
    }

    // --- compare-and-swap (generic atomic keyspace) -------------------------

    #[test]
    fn cas_create_then_version_conditional_update() {
        let mut l = InMemoryLedger::new();
        assert_eq!(l.get("k"), None);
        // Create: expect-absent (None) succeeds, version starts at 1.
        assert_eq!(l.compare_and_swap("k", None, 10), Ok(1));
        assert_eq!(l.get("k"), Some((1, 10)));
        // A second create is refused (key now exists).
        assert!(matches!(
            l.compare_and_swap("k", None, 5),
            Err(CasError::VersionMismatch {
                actual: Some(1),
                ..
            })
        ));
        // Version-matched update succeeds and bumps the version.
        assert_eq!(l.compare_and_swap("k", Some(1), 20), Ok(2));
        // A stale-version update is refused.
        assert!(matches!(
            l.compare_and_swap("k", Some(1), 30),
            Err(CasError::VersionMismatch {
                actual: Some(2),
                expected: Some(1),
                ..
            })
        ));
        assert_eq!(l.get("k"), Some((2, 20)));
    }

    #[test]
    fn concurrent_cas_create_has_exactly_one_winner() {
        // N threads race to create the same key; the atomic version check admits exactly one.
        let ledger = Arc::new(Mutex::new(InMemoryLedger::new()));
        let handles: Vec<_> = (0..50)
            .map(|i| {
                let l = Arc::clone(&ledger);
                thread::spawn(move || l.lock().unwrap().compare_and_swap("k", None, i).is_ok())
            })
            .collect();
        let winners: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(winners, 1, "exactly one create wins the race");
        assert_eq!(ledger.lock().unwrap().get("k").map(|(v, _)| v), Some(1));
    }
}
