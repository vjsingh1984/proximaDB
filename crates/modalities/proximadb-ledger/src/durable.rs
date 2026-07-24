//! Self-contained **durable** ledger (S2) — the same [`Ledger`](crate::Ledger) contract, backed by
//! an append-only, CRC-framed write-ahead log so accrued spend, caps, and in-flight leases **survive
//! a restart** (the property the in-memory core lacks).
//!
//! This is the durability *core*, proven in isolation — exactly how the anchor consumer (Sandhi)
//! reached durability (an in-memory ledger, then a self-contained durable impl behind the trait,
//! then wiring). The design here is the OLTP synthesis from TD-LEDGER-1:
//!
//! - **memtable-authoritative counter** — the live state is an [`InMemoryLedger`]; reads/writes are
//!   O(1) (memcache/memsql class).
//! - **WAL for recovery** — every *decided* mutation (reserve / settle / CAS / set-limit) is appended
//!   to the log *before* it is applied, and the log is replayed on open to rebuild the counter
//!   (sqlite/memsql class durability). Reclaims are **not** logged — a lease carries its `expires_at`,
//!   so replay reconstructs it and the normal sweeper reclaims it.
//! - **configurable sync** ([`SyncPolicy`]) — per-op `fsync` for hard caps, deferred for throughput
//!   (the redis `appendfsync` lever). Maps onto the consumer's fail-open/closed tiers.
//!
//! The production target for this WAL is the shared ADR-069 local-disk record WAL (S3 wiring); this
//! module owns a private log file so the durability semantics can be tested standalone. Torn tails
//! (a crash mid-append) are tolerated: replay stops at the first record whose length, CRC, or decode
//! is short/bad — an un-acked write is simply not recovered.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, ErrorKind, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Ledger;
use crate::memory::InMemoryLedger;
use crate::types::{CasError, Denied, Nanos, Policy, Reservation, ReserveOutcome, Version, Window};

/// When the WAL is flushed to stable storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// `fsync` after every append — strict durability (a hard `Block` cap that must never be lost).
    PerOp,
    /// Buffer appends; the OS flushes on its own schedule or on an explicit [`DurableLedger::flush`].
    /// Higher throughput, a bounded loss window on power failure — appropriate for soft (`Warn`)
    /// scopes. (Group-commit / interval fsync is a scheduling refinement tracked for S3.)
    Deferred,
}

/// A single decided mutation, as persisted in the log. Reclaims are intentionally absent (re-derived
/// on replay from each reservation's `expires_at_ns`).
#[derive(Debug, Serialize, Deserialize)]
enum LogRecord {
    SetLimit {
        scope: String,
        limit: Option<u64>,
        window: u8,
        policy: u8,
    },
    Reserve {
        id: u64,
        scope: String,
        ceiling: u64,
        expires_at_ns: Nanos,
    },
    Settle {
        id: u64,
        actual: u64,
        settled_at_ns: Nanos,
    },
    Cas {
        key: String,
        version: Version,
        value: i64,
    },
}

fn policy_to_u8(p: Policy) -> u8 {
    match p {
        Policy::Block => 0,
        Policy::Warn => 1,
    }
}
fn policy_from_u8(v: u8) -> Policy {
    match v {
        1 => Policy::Warn,
        _ => Policy::Block,
    }
}

fn window_to_u8(w: Window) -> u8 {
    match w {
        Window::Total => 0,
        Window::Daily => 1,
    }
}
fn window_from_u8(v: u8) -> Window {
    match v {
        1 => Window::Daily,
        _ => Window::Total,
    }
}

/// IEEE CRC-32 (reflected, poly `0xEDB88320`). Inline so the crate needs no checksum dependency; it
/// guards each record so a torn/corrupt tail is detected rather than mis-decoded into bad state.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A durable ledger: an in-memory authoritative counter fronted by a CRC-framed append-only WAL.
pub struct DurableLedger {
    state: InMemoryLedger,
    log: File,
    sync: SyncPolicy,
}

impl DurableLedger {
    /// Open (creating if absent) a durable ledger at `path`, replaying any existing log to recover
    /// spend / caps / in-flight leases. The clock is never read here — expired leases are left for
    /// the caller's [`reclaim_expired`](Ledger::reclaim_expired) sweep, keeping open deterministic.
    pub fn open(path: impl AsRef<Path>, sync: SyncPolicy) -> io::Result<Self> {
        let path = path.as_ref();
        let mut state = InMemoryLedger::new();
        if let Ok(file) = File::open(path) {
            let mut reader = BufReader::new(file);
            while let Some(record) = read_record(&mut reader)? {
                apply_record(&mut state, record);
            }
        }
        let log = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        Ok(Self { state, log, sync })
    }

    /// Flush buffered appends to stable storage (a no-op under [`SyncPolicy::PerOp`], where each
    /// append already synced). Call at a checkpoint or before shutdown under `Deferred`.
    pub fn flush(&mut self) -> io::Result<()> {
        self.log.sync_all()
    }

    /// Held (unsettled, unreclaimed) leases — for tests/introspection.
    #[must_use]
    pub fn active_leases(&self) -> usize {
        self.state.active_leases()
    }

    fn append(&mut self, record: &LogRecord) -> io::Result<()> {
        let payload = bincode::serialize(record).map_err(io::Error::other)?;
        let len = u32::try_from(payload.len()).map_err(io::Error::other)?;
        // Frame: [len u32-le][crc32 u32-le][payload]. The two headers let replay detect a torn tail.
        self.log.write_all(&len.to_le_bytes())?;
        self.log.write_all(&crc32(&payload).to_le_bytes())?;
        self.log.write_all(&payload)?;
        if self.sync == SyncPolicy::PerOp {
            self.log.sync_all()?;
        }
        Ok(())
    }
}

impl Ledger for DurableLedger {
    fn set_limit(&mut self, scope: &str, limit: Option<u64>, window: Window, policy: Policy) {
        let record = LogRecord::SetLimit {
            scope: scope.to_string(),
            limit,
            window: window_to_u8(window),
            policy: policy_to_u8(policy),
        };
        if let Err(e) = self.append(&record) {
            eprintln!("proximadb-ledger: set_limit append failed, not applied: {e}");
            return;
        }
        self.state.set_limit(scope, limit, window, policy);
    }

    fn reserve(
        &mut self,
        scope: &str,
        ceiling: u64,
        now_ns: Nanos,
        ttl_ns: Nanos,
    ) -> ReserveOutcome {
        // Roll the window first so the admit check sees the current window's spend (a hard cap must
        // never be checked against stale spend), then decide (no mutation)…
        self.state.roll(scope, now_ns);
        if let Err(denied) = self.state.check_admit(scope, ceiling) {
            return ReserveOutcome::Denied(denied);
        }
        let reservation = Reservation {
            id: self.state.peek_next_id(),
            scope: scope.to_string(),
            ceiling,
            expires_at_ns: now_ns.saturating_add(ttl_ns),
        };
        // …persist the decision *before* applying it. A hard cap that cannot be durably recorded
        // fails **closed** (deny) — never admit what a restart would forget.
        let record = LogRecord::Reserve {
            id: reservation.id,
            scope: reservation.scope.clone(),
            ceiling: reservation.ceiling,
            expires_at_ns: reservation.expires_at_ns,
        };
        if let Err(e) = self.append(&record) {
            eprintln!("proximadb-ledger: reserve append failed, denying (fail-closed): {e}");
            return ReserveOutcome::Denied(Denied {
                scope: scope.to_string(),
                limit: self.state.limit(scope).unwrap_or(0),
                spent: self.state.spent(scope),
                reserved: self.state.reserved(scope),
                requested_ceiling: ceiling,
            });
        }
        self.state.apply_reserve(reservation.clone());
        ReserveOutcome::Admitted(reservation)
    }

    fn settle(&mut self, reservation_id: u64, actual: u64, now_ns: Nanos) {
        if let Err(e) = self.append(&LogRecord::Settle {
            id: reservation_id,
            actual,
            settled_at_ns: now_ns,
        }) {
            // Settle is idempotent/retryable: skip applying so state never runs ahead of the WAL.
            eprintln!("proximadb-ledger: settle append failed, not applied (retryable): {e}");
            return;
        }
        self.state.settle(reservation_id, actual, now_ns);
    }

    fn reclaim_expired(&mut self, now_ns: Nanos) -> usize {
        // Not logged — a reservation carries its expiry, so replay + this sweep re-derive reclaims.
        self.state.reclaim_expired(now_ns)
    }

    fn compare_and_swap(
        &mut self,
        key: &str,
        expected: Option<Version>,
        new_value: i64,
    ) -> Result<Version, CasError> {
        let version = self.state.check_cas(key, expected)?;
        let record = LogRecord::Cas {
            key: key.to_string(),
            version,
            value: new_value,
        };
        if let Err(e) = self.append(&record) {
            // Could not persist: surface as "did not apply, retry" against the observed version.
            eprintln!("proximadb-ledger: cas append failed, not applied: {e}");
            return Err(CasError::VersionMismatch {
                key: key.to_string(),
                expected,
                actual: self.state.get(key).map(|(v, _)| v),
            });
        }
        self.state.apply_cas(key, version, new_value);
        Ok(version)
    }

    fn limit(&self, scope: &str) -> Option<u64> {
        self.state.limit(scope)
    }
    fn policy(&self, scope: &str) -> Policy {
        self.state.policy(scope)
    }
    fn spent(&self, scope: &str) -> u64 {
        self.state.spent(scope)
    }
    fn reserved(&self, scope: &str) -> u64 {
        self.state.reserved(scope)
    }
    fn get(&self, key: &str) -> Option<(Version, i64)> {
        self.state.get(key)
    }

    fn reservation_scope(&self, reservation_id: u64) -> Option<String> {
        self.state.reservation_scope(reservation_id)
    }
}

/// Apply one recovered log record to the in-memory state (the replay path — no re-logging).
fn apply_record(state: &mut InMemoryLedger, record: LogRecord) {
    match record {
        LogRecord::SetLimit {
            scope,
            limit,
            window,
            policy,
        } => state.set_limit(
            &scope,
            limit,
            window_from_u8(window),
            policy_from_u8(policy),
        ),
        LogRecord::Reserve {
            id,
            scope,
            ceiling,
            expires_at_ns,
        } => state.apply_reserve(Reservation {
            id,
            scope,
            ceiling,
            expires_at_ns,
        }),
        LogRecord::Settle {
            id,
            actual,
            settled_at_ns,
        } => state.settle(id, actual, settled_at_ns),
        LogRecord::Cas {
            key,
            version,
            value,
        } => state.apply_cas(&key, version, value),
    }
}

/// Read one framed record, or `None` at a clean EOF **or a torn/corrupt tail** (short read on any of
/// the three fields, a CRC mismatch, or a decode failure) — an un-acked write is not recovered.
fn read_record<R: Read>(reader: &mut R) -> io::Result<Option<LogRecord>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None), // clean EOF
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut crc_buf = [0u8; 4];
    if reader.read_exact(&mut crc_buf).is_err() {
        return Ok(None); // torn tail
    }
    let expected_crc = u32::from_le_bytes(crc_buf);
    let mut payload = vec![0u8; len];
    if reader.read_exact(&mut payload).is_err() {
        return Ok(None); // torn tail
    }
    if crc32(&payload) != expected_crc {
        return Ok(None); // corrupt tail
    }
    Ok(bincode::deserialize::<LogRecord>(&payload).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Seek;

    const TTL: Nanos = 60_000_000_000; // 60s in ns
    const NOW: Nanos = 0;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn open(dir: &tempfile::TempDir) -> DurableLedger {
        DurableLedger::open(dir.path().join("ledger.wal"), SyncPolicy::PerOp).unwrap()
    }
    fn admit(l: &mut DurableLedger, scope: &str, ceiling: u64) -> Reservation {
        match l.reserve(scope, ceiling, NOW, TTL) {
            ReserveOutcome::Admitted(r) => r,
            ReserveOutcome::Denied(d) => panic!("expected admit, got {d:?}"),
        }
    }

    #[test]
    fn spend_and_caps_survive_reopen() {
        // The property the in-memory ledger lacks: a restart must not zero accrued spend or caps.
        let dir = tmp();
        {
            let mut l = open(&dir);
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            let r = admit(&mut l, "g", 50);
            l.settle(r.id, 40, NOW);
            assert_eq!(l.spent("g"), 40);
        } // dropped — simulate a restart
        let reopened = open(&dir);
        assert_eq!(reopened.spent("g"), 40, "spend persists across restart");
        assert_eq!(reopened.limit("g"), Some(100), "caps persist");
        assert_eq!(reopened.policy("g"), Policy::Block);
        assert_eq!(reopened.reserved("g"), 0);
    }

    #[test]
    fn in_flight_lease_survives_reopen_and_still_settles() {
        let dir = tmp();
        let id = {
            let mut l = open(&dir);
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            admit(&mut l, "g", 80).id // reserved but NOT settled before the "crash"
        };
        let mut reopened = open(&dir);
        assert_eq!(
            reopened.reserved("g"),
            80,
            "the in-flight lease is recovered"
        );
        assert_eq!(reopened.available("g"), 20);
        // A cap check against the recovered lease still holds…
        assert!(matches!(
            reopened.reserve("g", 30, NOW, TTL),
            ReserveOutcome::Denied(_)
        ));
        // …and the recovered lease settles by its original id.
        reopened.settle(id, 55, NOW);
        assert_eq!(reopened.spent("g"), 55);
        assert_eq!(reopened.reserved("g"), 0);
    }

    #[test]
    fn idempotent_settle_survives_reopen() {
        // A settle replayed after a restart must not double-count (C3 across the durability boundary).
        let dir = tmp();
        let id = {
            let mut l = open(&dir);
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            let r = admit(&mut l, "g", 50);
            l.settle(r.id, 40, NOW);
            r.id
        };
        let mut reopened = open(&dir);
        reopened.settle(id, 999, NOW); // replayed settle on an already-settled (now absent) lease
        assert_eq!(reopened.spent("g"), 40, "replayed settle is a no-op");
    }

    #[test]
    fn cas_state_survives_reopen() {
        let dir = tmp();
        {
            let mut l = open(&dir);
            assert_eq!(l.compare_and_swap("flag", None, 10), Ok(1));
            assert_eq!(l.compare_and_swap("flag", Some(1), 20), Ok(2));
        }
        let mut reopened = open(&dir);
        assert_eq!(reopened.get("flag"), Some((2, 20)));
        // The recovered version is authoritative: a stale-version write is refused, a matched one wins.
        assert!(reopened.compare_and_swap("flag", Some(1), 30).is_err());
        assert_eq!(reopened.compare_and_swap("flag", Some(2), 30), Ok(3));
    }

    #[test]
    fn reclaimed_lease_does_not_survive_reopen() {
        // Reclaims aren't logged; the sweep re-derives them from the recovered expiry.
        let dir = tmp();
        {
            let mut l = open(&dir);
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            let _crashed = admit(&mut l, "g", 80);
        }
        let mut reopened = open(&dir);
        assert_eq!(reopened.reserved("g"), 80, "recovered as in-flight");
        assert_eq!(reopened.reclaim_expired(TTL + 1), 1, "swept by its expiry");
        assert_eq!(reopened.reserved("g"), 0);
        assert_eq!(reopened.available("g"), 100);
    }

    #[test]
    fn torn_tail_is_ignored_on_replay() {
        // A crash mid-append leaves a partial final record; replay must recover everything before it
        // and silently drop the torn tail.
        let dir = tmp();
        let path = dir.path().join("ledger.wal");
        {
            let mut l = DurableLedger::open(&path, SyncPolicy::PerOp).unwrap();
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            let r = admit(&mut l, "g", 50);
            l.settle(r.id, 40, NOW);
        }
        // Append 6 bytes of garbage — a plausible torn frame header/body.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[7, 0, 0, 0, 0, 0]).unwrap(); // claims len=7, then truncates
            f.sync_all().unwrap();
        }
        let reopened = DurableLedger::open(&path, SyncPolicy::PerOp).unwrap();
        assert_eq!(reopened.spent("g"), 40, "committed records recovered");
        assert_eq!(reopened.limit("g"), Some(100));
    }

    #[test]
    fn deferred_sync_persists_after_flush() {
        let dir = tmp();
        let path = dir.path().join("ledger.wal");
        {
            let mut l = DurableLedger::open(&path, SyncPolicy::Deferred).unwrap();
            l.set_limit("g", Some(100), Window::Total, Policy::Block);
            let r = admit(&mut l, "g", 30);
            l.settle(r.id, 30, NOW);
            l.flush().unwrap(); // explicit checkpoint under the deferred (throughput) policy
        }
        let reopened = DurableLedger::open(&path, SyncPolicy::Deferred).unwrap();
        assert_eq!(reopened.spent("g"), 30);
    }

    #[test]
    fn block_overshoot_prevented_on_durable_impl() {
        // The C1 invariant holds identically on the durable backend (same decision seam).
        let dir = tmp();
        let mut l = open(&dir);
        l.set_limit("g", Some(100), Window::Total, Policy::Block);
        let r = admit(&mut l, "g", 100);
        assert!(matches!(
            l.reserve("g", 1, NOW, TTL),
            ReserveOutcome::Denied(_)
        ));
        l.settle(r.id, 40, NOW);
        assert!(l.spent("g") + l.reserved("g") <= 100);
    }

    #[test]
    fn appends_are_framed_len_crc_payload() {
        // Guard the on-disk frame shape so a later reader/format change is a conscious break.
        let dir = tmp();
        let path = dir.path().join("ledger.wal");
        {
            let mut l = DurableLedger::open(&path, SyncPolicy::PerOp).unwrap();
            l.set_limit("s", Some(1), Window::Total, Policy::Warn);
        }
        let mut f = File::open(&path).unwrap();
        f.rewind().unwrap();
        let mut len_buf = [0u8; 4];
        f.read_exact(&mut len_buf).unwrap();
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut crc_buf = [0u8; 4];
        f.read_exact(&mut crc_buf).unwrap();
        let mut payload = vec![0u8; len];
        f.read_exact(&mut payload).unwrap();
        assert_eq!(
            crc32(&payload),
            u32::from_le_bytes(crc_buf),
            "framed crc matches payload"
        );
    }

    #[test]
    fn daily_window_config_and_spend_survive_reopen() {
        const DAY: Nanos = 86_400_000_000_000;
        let dir = tmp();
        {
            let mut l = open(&dir);
            l.set_limit("g", Some(100), Window::Daily, Policy::Block);
            let ReserveOutcome::Admitted(r) = l.reserve("g", 80, 0, TTL) else {
                panic!("fits on day 0");
            };
            l.settle(r.id, 80, 0); // 80 spent on day 0
        }
        let mut reopened = open(&dir);
        // Day 0 spend + the Daily window config are recovered: 80 already spent, so 80 more is denied.
        assert!(matches!(
            reopened.reserve("g", 80, 0, TTL),
            ReserveOutcome::Denied(_)
        ));
        // Day 1: the recovered Daily window rolls → spend resets → 80 fits (a Total cap would deny).
        assert!(matches!(
            reopened.reserve("g", 80, DAY, TTL),
            ReserveOutcome::Admitted(_)
        ));
    }
}
