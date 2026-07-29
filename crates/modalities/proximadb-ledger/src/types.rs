//! Core value types for the ledger correctness model (ADR-071 / TD-LEDGER-1).
//!
//! Neutral units only — every quantity here is a **count**, never a price (consistent with
//! ADR-067). No dollars, tiers, or SKUs cross this boundary.

/// Wall-clock time in **nanoseconds**. Chosen to match `proximadb-records`' `valid_to_ns`, so S2 can
/// persist a lease's expiry without a unit conversion. The core never reads the system clock — every
/// operation takes `now_ns` as an argument, which keeps the whole state machine deterministic and
/// unit-testable (the property that makes the C1–C3 invariants provable without a real clock).
pub type Nanos = i64;

/// Monotonic version stamp for a compare-and-swap key. Starts at 1 on create; increments per write.
pub type Version = u64;

/// How a scope reacts when a reservation's ceiling would exceed its limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Policy {
    /// Hard cap — a ceiling that would breach the limit is **refused** before admission (a cap that
    /// must never be overshot).
    #[default]
    Block,
    /// Soft cap — never refused; spend still accrues so a downstream alert subsystem can notify.
    Warn,
}

/// The window over which a scope's spend accrues before it resets. Calendar-aligned in UTC.
///
/// `Monthly` is intentionally absent for now (it needs civil-calendar math); `Total` + `Daily` are
/// pure integer arithmetic on the nanosecond clock, which keeps this crate dependency-free.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Window {
    /// Never resets — a lifetime cap.
    #[default]
    Total,
    /// Resets at each UTC midnight.
    Daily,
}

/// Nanoseconds in a day.
pub const NANOS_PER_DAY: Nanos = 86_400 * 1_000_000_000;

/// The inclusive start (in ns) of the current window containing `now_ns`. Calendar-aligned in UTC
/// (the Unix epoch is itself UTC midnight, so flooring to a day boundary *is* UTC midnight). `Total`
/// returns 0 (all-time). `now_ns` is expected non-negative (a real wall clock).
pub fn window_start_ns(now_ns: Nanos, window: Window) -> Nanos {
    match window {
        Window::Total => 0,
        Window::Daily => (now_ns / NANOS_PER_DAY) * NANOS_PER_DAY,
    }
}

/// A held reservation: a lease over `ceiling` units in `scope`, valid until `expires_at_ns`.
///
/// `id` is opaque and assigned by the ledger; the caller settles by it or lets it expire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub id: u64,
    pub scope: String,
    pub ceiling: u64,
    pub expires_at_ns: Nanos,
}

/// Why a [`reserve`](crate::Ledger::reserve) was refused: the ceiling did not fit under a hard cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub scope: String,
    pub limit: u64,
    pub spent: u64,
    pub reserved: u64,
    pub requested_ceiling: u64,
}

/// Result of an atomic reserve: admitted (with the lease) or denied (over a hard cap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveOutcome {
    Admitted(Reservation),
    Denied(Denied),
}

/// Why a [`compare_and_swap`](crate::Ledger::compare_and_swap) failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasError {
    /// The key's current version did not match `expected` — another writer got there first. Carries
    /// the observed `actual` version (`None` = key absent) so the caller can retry against it.
    VersionMismatch {
        key: String,
        expected: Option<Version>,
        actual: Option<Version>,
    },
}
