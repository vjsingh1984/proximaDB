//! # ProximaDB Ledger modality — S1 correctness core
//!
//! The **in-memory, wiring-free correctness core** of the transactional Ledger modality
//! (link: `docs/12-design/adr/ADR-071` / `docs/10-quality/td/TD-LEDGER-1`). It is the pure atomic
//! **reserve → settle** lease state machine plus a generic **compare-and-swap** keyspace — the
//! OLTP primitives ProximaDB lacks today (`StorageKV` is non-atomic, `put_if_absent` is create-only,
//! the transaction manager is unwired).
//!
//! This crate carries **no durability, no transport, and no windows** — those are S2 (the durable
//! WAL, memtable counter, and TTL sweeper) and S3 (proto/gRPC/router). Per the TD build order, the
//! correctness invariant is proven here *first*, in isolation, before any durability or wiring risk
//! is taken on. Neutral units only — counts, never prices.
//!
//! ## The invariants (the acceptance bar — Sandhi TD-0007 C1–C3)
//!
//! - **C1 — atomic conditional admit.** [`Ledger::reserve`] holds a conservative `ceiling`; a
//!   `Block` scope refuses a ceiling that would breach the cap, so `spent + reserved ≤ limit` holds
//!   at every step. Under contention the operation is serialized (in production, by the partition
//!   write lock; here, by the caller's `Mutex`), so concurrent reservers can never oversubscribe.
//! - **C2 — TTL leases.** A reservation is a lease with an expiry; [`Ledger::reclaim_expired`] frees
//!   a crashed reserver's held capacity on a **timed** basis — without a read having to touch the
//!   scope. (The in-memory reclaim here becomes the durable sweeper in S2.)
//! - **C3 — idempotent settle.** [`Ledger::settle`] is a `reserved → settled` transition keyed by
//!   reservation id; a replay is a no-op, so at-least-once delivery never double-counts.
//!
//! [`compare_and_swap`](Ledger::compare_and_swap) extends the same atomic-admit discipline to a
//! generic keyspace (feature flags, config, quotas): a write lands only if the caller's expected
//! version matches the current one.

mod memory;
mod types;

pub use memory::InMemoryLedger;
pub use types::{CasError, Denied, Nanos, Policy, Reservation, ReserveOutcome, Version};

/// The atomic ledger contract — the seam every backend implements.
///
/// `InMemoryLedger` is the reference implementation (this crate); the durable partition-WAL backend
/// (S2/S3) will implement the same trait, so the conformance tests here become backend-agnostic. All
/// mutating methods take `&mut self`: exclusive access *is* the serialization point (the caller holds
/// the partition write lock — modeled in tests by a `Mutex`). Neutral units only — no pricing.
pub trait Ledger {
    /// Set (or clear, with `limit = None`) a scope's cap + policy.
    fn set_limit(&mut self, scope: &str, limit: Option<u64>, policy: Policy);

    /// Atomically admit a call by holding `ceiling` units as a lease expiring at `now_ns + ttl_ns`.
    ///
    /// The check is `spent + reserved + ceiling ≤ limit` for a `Block` scope; a `Warn` scope (soft
    /// cap) and an unset scope always admit. Returns [`ReserveOutcome::Denied`] when a hard cap could
    /// not fit — the call is never dispatched, so the cap cannot be overshot (C1).
    fn reserve(
        &mut self,
        scope: &str,
        ceiling: u64,
        now_ns: Nanos,
        ttl_ns: Nanos,
    ) -> ReserveOutcome;

    /// Idempotently settle a reservation to its actual usage, releasing the lease. A no-op if the id
    /// is unknown (already settled, or reclaimed after expiry) — safe under at-least-once delivery
    /// (C3). `actual = 0` releases the lease without recording spend (the failed/cancelled path).
    fn settle(&mut self, reservation_id: u64, actual: u64);

    /// Reclaim every lease expired at or before `now_ns`; returns how many were reclaimed. A
    /// reclaimed lease releases its held ceiling **without** recording spend, on a timed basis (C2).
    fn reclaim_expired(&mut self, now_ns: Nanos) -> usize;

    /// Atomically write `new_value` at `key` iff the current version equals `expected`
    /// (`expected = None` means "expect the key absent" — a create). Returns the new version on
    /// success, or [`CasError::VersionMismatch`] carrying the observed version.
    fn compare_and_swap(
        &mut self,
        key: &str,
        expected: Option<Version>,
        new_value: i64,
    ) -> Result<Version, CasError>;

    /// The configured cap for a scope (`None` = unset/unlimited).
    fn limit(&self, scope: &str) -> Option<u64>;

    /// The configured policy for a scope (`Block` when unset).
    fn policy(&self, scope: &str) -> Policy;

    /// Settled units recorded against a scope.
    fn spent(&self, scope: &str) -> u64;

    /// Units held by in-flight (unsettled, unexpired) leases.
    fn reserved(&self, scope: &str) -> u64;

    /// The current `(version, value)` at a compare-and-swap key, or `None` if absent.
    fn get(&self, key: &str) -> Option<(Version, i64)>;

    /// Remaining headroom = `limit - spent - reserved` (saturating). Unlimited scopes report
    /// [`u64::MAX`].
    fn available(&self, scope: &str) -> u64 {
        match self.limit(scope) {
            None => u64::MAX,
            Some(limit) => {
                limit.saturating_sub(self.spent(scope).saturating_add(self.reserved(scope)))
            }
        }
    }
}
