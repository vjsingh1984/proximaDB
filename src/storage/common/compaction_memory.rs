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
use std::sync::Mutex;
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

/// The execution shape selected before any input read, sort, or model fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionExecutionMode {
    InMemory,
    LocalSpill,
}

/// Resources one compaction must reserve for its complete execution shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionResourcePlan {
    pub mode: CompactionExecutionMode,
    pub memory_bytes: u64,
    pub scratch_bytes: u64,
}

/// Process-wide capacity against which a complete resource plan is admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionResourceBudget {
    pub memory_bytes: u64,
    pub scratch_bytes: u64,
}

/// Resources reserved by all admitted compactions in this process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompactionResourceUsage {
    pub memory_bytes: u64,
    pub scratch_bytes: u64,
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

fn projected_bytes(input_bytes: u64, amplification: f64) -> Option<u64> {
    if !amplification.is_finite() || amplification < 1.0 {
        return None;
    }
    Some(
        ((input_bytes as f64) * amplification)
            .ceil()
            .min(u64::MAX as f64) as u64,
    )
}

/// Select one complete execution shape from independent RAM and local-scratch
/// currencies. In-memory execution remains preferred when it fits. Spill is a
/// fallback, never a way to bypass the RAM guard.
pub fn plan_compaction_resources(
    input_bytes: u64,
    config: &CompactionConfig,
    snapshot: MemorySnapshot,
    available_scratch_bytes: u64,
    usage: CompactionResourceUsage,
) -> Option<CompactionResourcePlan> {
    let memory_budget = compaction_memory_budget(config, snapshot, usage.memory_bytes);
    let in_memory_bytes = projected_bytes(input_bytes, config.memory_amplification_factor)?;
    if in_memory_bytes <= memory_budget.remaining_bytes {
        return Some(CompactionResourcePlan {
            mode: CompactionExecutionMode::InMemory,
            memory_bytes: in_memory_bytes,
            scratch_bytes: 0,
        });
    }

    if !config.spill_enabled || config.spill_working_memory_mb == 0 {
        return None;
    }

    let spill_memory_bytes = config.spill_working_memory_mb.saturating_mul(MIB);
    if spill_memory_bytes > memory_budget.remaining_bytes {
        return None;
    }

    let mut scratch_budget_bytes = fraction_of(
        available_scratch_bytes,
        config.spill_available_disk_fraction,
    );
    if config.spill_max_disk_mb > 0 {
        scratch_budget_bytes =
            scratch_budget_bytes.min(config.spill_max_disk_mb.saturating_mul(MIB));
    }
    let remaining_scratch_bytes = scratch_budget_bytes.saturating_sub(usage.scratch_bytes);
    let spill_scratch_bytes =
        projected_bytes(input_bytes, config.spill_scratch_amplification_factor)?;
    if spill_scratch_bytes > remaining_scratch_bytes {
        return None;
    }

    Some(CompactionResourcePlan {
        mode: CompactionExecutionMode::LocalSpill,
        memory_bytes: spill_memory_bytes,
        scratch_bytes: spill_scratch_bytes,
    })
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

/// Atomically coordinates the two currencies required by spill compaction.
/// A single mutex is intentional: independent atomics could reserve RAM and
/// then fail scratch admission, exposing a partially committed reservation.
#[derive(Debug, Default)]
pub struct CompactionResourceAdmission {
    usage: Mutex<CompactionResourceUsage>,
    notify: Notify,
}

impl CompactionResourceAdmission {
    fn locked_usage(&self) -> std::sync::MutexGuard<'_, CompactionResourceUsage> {
        match self.usage.lock() {
            Ok(usage) => usage,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn usage(&self) -> CompactionResourceUsage {
        *self.locked_usage()
    }

    pub fn try_reserve(
        self: &Arc<Self>,
        plan: CompactionResourcePlan,
        budget: CompactionResourceBudget,
    ) -> Option<CompactionResourceReservation> {
        let mut usage = self.locked_usage();
        if plan.memory_bytes > budget.memory_bytes.saturating_sub(usage.memory_bytes)
            || plan.scratch_bytes > budget.scratch_bytes.saturating_sub(usage.scratch_bytes)
        {
            return None;
        }

        usage.memory_bytes = usage.memory_bytes.saturating_add(plan.memory_bytes);
        usage.scratch_bytes = usage.scratch_bytes.saturating_add(plan.scratch_bytes);
        drop(usage);

        Some(CompactionResourceReservation {
            admission: Arc::clone(self),
            plan,
        })
    }
}

/// RAII release for an atomically admitted RAM+scratch plan.
#[derive(Debug)]
pub struct CompactionResourceReservation {
    admission: Arc<CompactionResourceAdmission>,
    plan: CompactionResourcePlan,
}

impl CompactionResourceReservation {
    pub fn plan(&self) -> CompactionResourcePlan {
        self.plan
    }
}

impl Drop for CompactionResourceReservation {
    fn drop(&mut self) {
        let mut usage = self.admission.locked_usage();
        usage.memory_bytes = usage.memory_bytes.saturating_sub(self.plan.memory_bytes);
        usage.scratch_bytes = usage.scratch_bytes.saturating_sub(self.plan.scratch_bytes);
        drop(usage);
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

    #[test]
    fn planner_prefers_memory_when_the_complete_projection_fits() {
        let mut config = CompactionConfig::default();
        config.memory_amplification_factor = 2.0;
        config.memory_budget_fraction = 1.0;
        config.available_memory_fraction = 1.0;
        config.spill_enabled = true;
        config.spill_working_memory_mb = 128;
        config.spill_scratch_amplification_factor = 4.0;

        let plan = plan_compaction_resources(
            100 * MIB,
            &config,
            MemorySnapshot {
                total_bytes: GIB,
                available_bytes: GIB,
            },
            10 * GIB,
            CompactionResourceUsage::default(),
        )
        .expect("the in-memory plan should fit");

        assert_eq!(plan.mode, CompactionExecutionMode::InMemory);
        assert_eq!(plan.memory_bytes, 200 * MIB);
        assert_eq!(plan.scratch_bytes, 0);
    }

    #[test]
    fn planner_spills_only_when_both_fixed_memory_and_scratch_fit() {
        let mut config = CompactionConfig::default();
        config.memory_amplification_factor = 12.0;
        config.memory_budget_fraction = 1.0;
        config.available_memory_fraction = 1.0;
        config.max_memory_mb = 512;
        config.spill_enabled = true;
        config.spill_working_memory_mb = 128;
        config.spill_scratch_amplification_factor = 3.0;
        config.spill_available_disk_fraction = 0.5;

        let plan = plan_compaction_resources(
            100 * MIB,
            &config,
            MemorySnapshot {
                total_bytes: 8 * GIB,
                available_bytes: 8 * GIB,
            },
            GIB,
            CompactionResourceUsage::default(),
        )
        .expect("the local-spill plan should fit");

        assert_eq!(plan.mode, CompactionExecutionMode::LocalSpill);
        assert_eq!(plan.memory_bytes, 128 * MIB);
        assert_eq!(plan.scratch_bytes, 300 * MIB);

        assert!(
            plan_compaction_resources(
                200 * MIB,
                &config,
                MemorySnapshot {
                    total_bytes: 8 * GIB,
                    available_bytes: 8 * GIB,
                },
                GIB,
                CompactionResourceUsage {
                    memory_bytes: 0,
                    scratch_bytes: 100 * MIB,
                },
            )
            .is_none(),
            "the planner must not overcommit the 512 MiB effective scratch budget"
        );
    }

    #[test]
    fn spill_never_bypasses_the_ram_guard_or_default_off_policy() {
        let mut config = CompactionConfig::default();
        config.memory_amplification_factor = 12.0;
        config.memory_budget_fraction = 1.0;
        config.available_memory_fraction = 1.0;
        config.max_memory_mb = 64;
        config.spill_working_memory_mb = 128;
        config.spill_scratch_amplification_factor = 2.0;
        let snapshot = MemorySnapshot {
            total_bytes: GIB,
            available_bytes: GIB,
        };

        assert!(
            plan_compaction_resources(
                100 * MIB,
                &config,
                snapshot,
                10 * GIB,
                CompactionResourceUsage::default(),
            )
            .is_none(),
            "spill is default-off"
        );

        config.spill_enabled = true;
        assert!(
            plan_compaction_resources(
                100 * MIB,
                &config,
                snapshot,
                10 * GIB,
                CompactionResourceUsage::default(),
            )
            .is_none(),
            "a 128 MiB spill working set cannot fit a 64 MiB memory ceiling"
        );
    }

    #[test]
    fn dual_resource_reservation_is_atomic_and_raii_released() {
        let admission = Arc::new(CompactionResourceAdmission::default());
        let first = admission
            .try_reserve(
                CompactionResourcePlan {
                    mode: CompactionExecutionMode::LocalSpill,
                    memory_bytes: 200,
                    scratch_bytes: 700,
                },
                CompactionResourceBudget {
                    memory_bytes: 1_000,
                    scratch_bytes: 1_000,
                },
            )
            .expect("first reservation fits");

        assert!(
            admission
                .try_reserve(
                    CompactionResourcePlan {
                        mode: CompactionExecutionMode::LocalSpill,
                        memory_bytes: 100,
                        scratch_bytes: 400,
                    },
                    CompactionResourceBudget {
                        memory_bytes: 1_000,
                        scratch_bytes: 1_000,
                    },
                )
                .is_none(),
            "failed scratch admission must not partially reserve memory"
        );
        assert_eq!(
            admission.usage(),
            CompactionResourceUsage {
                memory_bytes: 200,
                scratch_bytes: 700,
            }
        );

        drop(first);
        assert_eq!(admission.usage(), CompactionResourceUsage::default());
    }
}
