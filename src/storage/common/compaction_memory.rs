/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Process-wide, byte-weighted compaction memory admission.
//!
//! Input segment bytes are the stable workload signal. They are converted to
//! projected peak resident bytes with a measured amplification factor, then
//! admitted against the tightest of process-visible capacity, live available
//! memory, and an optional operator ceiling. All collections share one
//! reservation counter so independent workers cannot double-spend the budget.

use crate::core::config::CompactionConfig;
use proximadb_hardware::{MemorySnapshot, memory_snapshot};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Notify;

const MIB: u64 = 1024 * 1024;
const ADMISSION_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Auditable result of applying the configured policy to one memory snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionMemoryBudget {
    pub capacity_budget_bytes: u64,
    pub available_budget_bytes: u64,
    pub absolute_ceiling_bytes: Option<u64>,
    pub effective_budget_bytes: u64,
    pub reserved_bytes: u64,
    pub remaining_bytes: u64,
    pub max_input_bytes: u64,
}

/// Convert immutable input bytes into a conservative peak-RSS reservation.
pub fn projected_compaction_memory_bytes(input_bytes: u64, config: &CompactionConfig) -> u64 {
    let amplification = config.memory_amplification_factor.max(1.0);
    ((input_bytes as f64) * amplification)
        .ceil()
        .min(u64::MAX as f64) as u64
}

/// Apply the hybrid capacity/live/absolute policy to an injected snapshot.
/// Injection keeps the arithmetic deterministic and independently testable.
pub fn compaction_memory_budget(
    config: &CompactionConfig,
    snapshot: MemorySnapshot,
    reserved_bytes: u64,
) -> CompactionMemoryBudget {
    let capacity_budget_bytes = fraction_of(snapshot.total_bytes, config.memory_budget_fraction);
    let available_budget_bytes =
        fraction_of(snapshot.available_bytes, config.available_memory_fraction);
    let absolute_ceiling_bytes =
        (config.max_memory_mb > 0).then(|| config.max_memory_mb.saturating_mul(MIB));

    let mut effective_budget_bytes = capacity_budget_bytes.min(available_budget_bytes);
    if let Some(absolute) = absolute_ceiling_bytes {
        effective_budget_bytes = effective_budget_bytes.min(absolute);
    }
    let remaining_bytes = effective_budget_bytes.saturating_sub(reserved_bytes);
    let amplification = config.memory_amplification_factor.max(1.0);
    let max_input_bytes = ((remaining_bytes as f64) / amplification).floor() as u64;

    CompactionMemoryBudget {
        capacity_budget_bytes,
        available_budget_bytes,
        absolute_ceiling_bytes,
        effective_budget_bytes,
        reserved_bytes,
        remaining_bytes,
        max_input_bytes,
    }
}

fn fraction_of(bytes: u64, fraction: f64) -> u64 {
    if !fraction.is_finite() || fraction <= 0.0 {
        return 0;
    }
    ((bytes as f64) * fraction.min(1.0))
        .floor()
        .min(u64::MAX as f64) as u64
}

/// Current input-byte budget after accounting for every running collection.
pub fn current_compaction_input_budget(config: &CompactionConfig) -> CompactionMemoryBudget {
    compaction_memory_budget(
        config,
        memory_snapshot(),
        global_admission().reserved_bytes(),
    )
}

pub fn reserved_compaction_memory_bytes() -> u64 {
    global_admission().reserved_bytes()
}

/// One process-wide weighted coordinator. A count semaphore cannot distinguish
/// a 20 MiB maintenance merge from an 850 MiB corpus merge.
#[derive(Debug, Default)]
pub struct CompactionMemoryAdmission {
    reserved_bytes: AtomicU64,
    notify: Notify,
}

impl CompactionMemoryAdmission {
    pub fn reserved_bytes(&self) -> u64 {
        self.reserved_bytes.load(Ordering::Acquire)
    }

    fn try_reserve(
        self: &Arc<Self>,
        projected_bytes: u64,
        budget_bytes: u64,
    ) -> Option<CompactionMemoryReservation> {
        let mut observed = self.reserved_bytes.load(Ordering::Acquire);
        loop {
            if projected_bytes > budget_bytes.saturating_sub(observed) {
                return None;
            }
            match self.reserved_bytes.compare_exchange_weak(
                observed,
                observed.saturating_add(projected_bytes),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(CompactionMemoryReservation {
                        admission: Arc::clone(self),
                        reserved_bytes: projected_bytes,
                    });
                }
                Err(current) => observed = current,
            }
        }
    }
}

/// RAII reservation released on every worker exit path, including errors and
/// cancellation.
#[derive(Debug)]
pub struct CompactionMemoryReservation {
    admission: Arc<CompactionMemoryAdmission>,
    reserved_bytes: u64,
}

impl CompactionMemoryReservation {
    pub fn reserved_bytes(&self) -> u64 {
        self.reserved_bytes
    }
}

impl Drop for CompactionMemoryReservation {
    fn drop(&mut self) {
        self.admission
            .reserved_bytes
            .fetch_sub(self.reserved_bytes, Ordering::AcqRel);
        self.admission.notify.notify_waiters();
    }
}

fn global_admission() -> &'static Arc<CompactionMemoryAdmission> {
    static ADMISSION: OnceLock<Arc<CompactionMemoryAdmission>> = OnceLock::new();
    ADMISSION.get_or_init(|| Arc::new(CompactionMemoryAdmission::default()))
}

/// Wait until the candidate fits both current live memory and the reservations
/// held by other collections. Periodic refresh notices memory released by
/// non-compaction work; `shutdown` prevents manager shutdown from hanging.
pub async fn acquire_compaction_memory(
    input_bytes: u64,
    config: &CompactionConfig,
    shutdown: &AtomicBool,
) -> Option<CompactionMemoryReservation> {
    let projected_bytes = projected_compaction_memory_bytes(input_bytes, config);
    let admission = global_admission();

    loop {
        if shutdown.load(Ordering::Acquire) {
            return None;
        }

        let notified = admission.notify.notified();
        let snapshot = memory_snapshot();
        let budget = compaction_memory_budget(config, snapshot, 0);
        if let Some(reservation) =
            admission.try_reserve(projected_bytes, budget.effective_budget_bytes)
        {
            return Some(reservation);
        }

        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(ADMISSION_RECHECK_INTERVAL) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn hybrid_budget_uses_the_tightest_guard() {
        let mut config = CompactionConfig::default();
        config.memory_budget_fraction = 0.25;
        config.available_memory_fraction = 0.5;
        config.max_memory_mb = 8 * 1024;

        let budget = compaction_memory_budget(
            &config,
            MemorySnapshot {
                total_bytes: 64 * GIB,
                available_bytes: 20 * GIB,
            },
            GIB,
        );

        assert_eq!(budget.capacity_budget_bytes, 16 * GIB);
        assert_eq!(budget.available_budget_bytes, 10 * GIB);
        assert_eq!(budget.absolute_ceiling_bytes, Some(8 * GIB));
        assert_eq!(budget.effective_budget_bytes, 8 * GIB);
        assert_eq!(budget.remaining_bytes, 7 * GIB);
    }

    #[test]
    fn measured_amplification_converts_bytes_without_integer_truncation() {
        let mut config = CompactionConfig::default();
        config.memory_amplification_factor = 9.85;

        assert_eq!(projected_compaction_memory_bytes(100, &config), 985);
    }

    #[test]
    fn weighted_admission_prevents_parallel_oversubscription() {
        let admission = Arc::new(CompactionMemoryAdmission::default());
        let first = admission
            .try_reserve(700, 1_000)
            .expect("first weighted reservation should fit");
        assert!(admission.try_reserve(400, 1_000).is_none());
        assert_eq!(admission.reserved_bytes(), 700);

        drop(first);
        let second = admission
            .try_reserve(400, 1_000)
            .expect("released bytes should be reusable");
        assert_eq!(second.reserved_bytes(), 400);
    }
}
