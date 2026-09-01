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
pub(crate) fn record_pending(
    collection_data_dir: &Path,
    output: &Path,
    failed_inputs: &[String],
) -> Result<()> {
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
    let mut ledger = ledger.clone();
    ledger.updated_at = now_iso();
    let data = serde_json::to_vec_pretty(&ledger).context("serialize retire-pending ledger")?;
    std::fs::write(sidecar_path(collection_data_dir), &data)
        .with_context(|| format!("write {}", sidecar_path(collection_data_dir).display()))?;
    Ok(())
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
