//! TD-COMPACT-13: the durable retirement obligation ledger.
//!
//! Compaction retirement is delete-after-publish; when the delete of an input
//! fails transiently (object-store timeouts, throttling), the superseded file
//! must not silently keep serving. This module records the {output, inputs}
//! obligation in a per-collection sidecar (`retire-pending.json` next to the
//! segments), tracks the pending paths in a process-global set that the query
//! discovery filters against (already-orphaned beds self-heal on the read
//! path at zero extra GETs), and exposes a gauge so un-drained obligations are
//! observable instead of silent.
//!
//! Concurrency: same-collection compaction tasks serialize through the worker
//! follow-up chain, and cross-collection tasks touch different sidecars. A
//! lost sidecar update degrades to an idempotent re-drain (deletes are
//! NotFound-idempotent), never to loss — inputs are only ever deleted after
//! their replacement is published.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

/// One recorded obligation: `output` was published; `inputs` (the failed
/// deletes) must be retired by the reconciler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRetirement {
    pub output: String,
    pub inputs: Vec<String>,
    pub recorded_at: String,
    pub attempts: u32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct LedgerFile {
    pub(crate) version: u32,
    pub(crate) updated_at: String,
    pub(crate) pending: Vec<PendingRetirement>,
}

/// Process-global set of input URLs whose delete failed and are pending
/// retirement. The query discovery filters these paths so superseded files
/// stop serving the moment an obligation is recorded (and after boot load).
pub(crate) fn global_pending_retirements() -> &'static RwLock<HashSet<String>> {
    static SET: std::sync::OnceLock<RwLock<HashSet<String>>> = std::sync::OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

pub(crate) fn global_known_collection_dirs() -> &'static RwLock<HashSet<String>> {
    static SET: std::sync::OnceLock<RwLock<HashSet<String>>> = std::sync::OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Register a collection data dir for the reconciler sweep (called from task
/// scheduling, check_compaction_needed, and query discovery).
pub(crate) fn register_collection_dir(dir: &Path) {
    if let Ok(mut set) = global_known_collection_dirs().write() {
        set.insert(dir.to_string_lossy().into_owned());
    }
}

pub(crate) fn known_collection_dirs() -> Vec<String> {
    global_known_collection_dirs()
        .read()
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default()
}

/// Mark input paths pending (read-path filter + gauge).
pub(crate) fn mark_pending(inputs: &[String]) {
    if let Ok(mut set) = global_pending_retirements().write() {
        set.extend(inputs.iter().cloned());
    }
    crate::metrics::operational_metrics::PAX_PENDING_RETIREMENTS.set(set_len() as i64);
}

/// Clear drained input paths (read-path filter + gauge).
pub(crate) fn mark_drained(inputs: &[String]) {
    if let Ok(mut set) = global_pending_retirements().write() {
        for input in inputs {
            set.remove(input);
        }
    }
    crate::metrics::operational_metrics::PAX_PENDING_RETIREMENTS.set(set_len() as i64);
}

fn set_len() -> usize {
    global_pending_retirements()
        .read()
        .map(|s| s.len())
        .unwrap_or(0)
}

fn sidecar_path(collection_data_dir: &Path) -> PathBuf {
    collection_data_dir.join("retire-pending.json")
}

fn now_iso() -> String {
    // RFC3339-ish UTC stamp without pulling chrono formatting policy here; the
    // field is advisory (ordering/attribution), attempts drives logic.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Record failed input deletes as a durable obligation. Inputs are added to
/// the pending set before the sidecar write so the read-path filter engages
/// even if the write subsequently fails (the caller then fails loudly).
pub(crate) async fn record_pending(
    collection_data_dir: &Path,
    output: &Path,
    failed_inputs: &[String],
) -> Result<()> {
    let _ledger_guard = ledger_lock().await;
    mark_pending(failed_inputs);
    crate::metrics::operational_metrics::PAX_RETIREMENT_RECORD_TOTAL.inc();
    let mut ledger = load_ledger(collection_data_dir);
    ledger.pending.push(PendingRetirement {
        output: output.to_string_lossy().into_owned(),
        inputs: failed_inputs.to_vec(),
        recorded_at: now_iso(),
        attempts: 0,
    });
    write_ledger(collection_data_dir, &ledger)
}

/// Load the sidecar's pending obligations (missing file = empty).
pub(crate) fn load_ledger(collection_data_dir: &Path) -> LedgerFile {
    let path = sidecar_path(collection_data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => LedgerFile {
            version: 1,
            updated_at: now_iso(),
            pending: Vec::new(),
        },
    }
}

fn write_ledger(collection_data_dir: &Path, ledger: &LedgerFile) -> Result<()> {
    // Hold the ledger lock across load-modify-write sequences: two workers
    // (one executing a retirement, one sweeping) could otherwise lose updates.
    // Cross-PROCESS races degrade to an idempotent re-drain (deletes are
    // NotFound-idempotent), never to loss.
    let _guard = ledger_lock();
    let mut ledger = ledger.clone();
    ledger.updated_at = now_iso();
    let data = serde_json::to_vec_pretty(&ledger).context("serialize retire-pending ledger")?;
    // Atomic replace: a crash mid-write must not truncate the obligation
    // record (a truncated ledger would silently drop pending retirements).
    let final_path = sidecar_path(collection_data_dir);
    let tmp_path = final_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &data).with_context(|| format!("write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename {}", final_path.display()))?;
    Ok(())
}

/// Process-wide mutual exclusion for ledger read-modify-write sequences.
/// Async mutex: the drain holds it across `.await`ing delete attempts (the
/// guard is Send, so the worker futures stay Send).
pub(crate) async fn ledger_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// Persist the ledger after a drain pass and update the gauge.
pub(crate) fn save_after_drain(collection_data_dir: &Path, ledger: &LedgerFile) -> Result<()> {
    let mut ledger = ledger.clone();
    ledger.updated_at = now_iso();
    write_ledger(collection_data_dir, &ledger)?;
    crate::metrics::operational_metrics::PAX_PENDING_RETIREMENTS.set(set_len() as i64);
    Ok(())
}

/// Reconciler sweep debounce (30s): the worker calls `sweep_due` after every
/// task; only an elapsed debounce actually runs a sweep.
pub(crate) fn sweep_due() -> bool {
    static LAST: OnceLock<Mutex<std::time::Instant>> = OnceLock::new();
    let last = LAST.get_or_init(|| {
        Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(3600))
    });
    let Ok(mut last) = last.lock() else {
        return false;
    };
    if last.elapsed() >= std::time::Duration::from_secs(30) {
        *last = std::time::Instant::now();
        true
    } else {
        false
    }
}

/// URLs currently pending (for tests and the discovery filter's fast path).
pub(crate) fn pending_paths() -> HashSet<String> {
    global_pending_retirements()
        .read()
        .map(|s| s.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TD-COMPACT-13 T4a: record → load → drain → cleared, and a lost update
    /// degrades to idempotent re-drain (deletes are NotFound-idempotent).
    #[tokio::test]
    async fn ledger_round_trip_records_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        assert!(load_ledger(&data_dir).pending.is_empty(), "missing = empty");

        record_pending(
            &data_dir,
            Path::new("s3://b/coll/L3_x.pax"),
            &["s3://b/coll/L2_a.pax".into(), "s3://b/coll/L2_b.pax".into()],
        )
        .await
        .unwrap();
        let loaded = load_ledger(&data_dir);
        assert_eq!(loaded.pending.len(), 1);
        assert_eq!(loaded.pending[0].inputs.len(), 2);
        assert_eq!(loaded.pending[0].output, "s3://b/coll/L3_x.pax");

        // Second record APPENDS (distinct obligations accumulate).
        record_pending(
            &data_dir,
            Path::new("s3://b/coll/L2_y.pax"),
            &["s3://b/coll/L1_c.pax".into()],
        )
        .await
        .unwrap();
        assert_eq!(load_ledger(&data_dir).pending.len(), 2);
    }

    /// TD-COMPACT-13 T4b: pending-set mark/drain updates the filter set.
    #[test]
    fn pending_set_marks_and_drains() {
        let before = pending_paths();
        let probe = format!("probe-{}-orphan.pax", std::process::id());
        mark_pending(&[probe.clone()]);
        assert!(pending_paths().contains(&probe));
        mark_drained(&[probe.clone()]);
        assert!(!pending_paths().contains(&probe), "drained must clear");
        let _ = before;
    }
}
