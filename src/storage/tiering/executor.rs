/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 */

//! Tier migration executor — the final deferred piece of the tier-migration
//! pipeline.
//!
//! [`TieringPolicyEngine`] produces [`MigrationTask`]s; the executor here
//! takes those tasks and physically moves bytes between tier storage
//! locations using [`FilesystemFactory`].
//!
//! ## What the executor does
//!
//! 1. **Resolve source/target URLs** from the configured per-tier path
//!    map (`hot_tier_path`, `warm_tier_path`, `cold_tier_path`,
//!    `archive_tier_path`). Both must be configured — otherwise the
//!    task fails with `TierPathNotConfigured`.
//! 2. **Move atomically** via [`FilesystemFactory::move_atomic`], which
//!    routes by URL scheme (`file://` → local, `s3://` → S3, etc.) and
//!    uses a copy-then-delete pattern with the configured atomic
//!    coordinator under the hood.
//! 3. **Record result** as [`MigrationResult`] including bytes moved,
//!    duration, and any error.
//!
//! ## What the executor does NOT do (yet)
//!
//! * **Catalog-aware promotion**: the executor doesn't update any
//!   catalog state to reflect the new tier. That's the caller's job —
//!   on a successful result they should record `MigrationStatus` via
//!   the policy engine's `record_migration_complete` so the next
//!   evaluation doesn't re-propose the same migration.
//! * **Per-vector granularity**: a migration task carries
//!   `(collection, item_id)`. The executor treats `item_id` as a
//!   path-relative identifier — typically an SST segment file name
//!   ("seg-0001.sst") or a directory ("collection_id/level-0"). It does
//!   not extract individual vectors from a multi-vector SST file. If
//!   per-vector migration is needed, the producer (planner) must
//!   shape tasks at vector-file granularity.
//! * **Mid-flight crash recovery beyond what `move_atomic` provides**:
//!   `move_atomic` is copy-then-delete; if the process crashes after
//!   the copy succeeded but before the delete, both source and target
//!   exist. The next evaluation should detect "already at target" and
//!   skip — see [`Self::execute`] idempotency check.
//!
//! ## Concurrency model
//!
//! Single-task `execute` is fully `Send + Sync`. The batched
//! `execute_batch` uses a `tokio::sync::Semaphore` to bound
//! `max_concurrent_migrations` so cloud-storage GETs/PUTs don't
//! stampede.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use super::migration::{MigrationResult, MigrationStatus, MigrationTask};
use super::policy::PerformanceTier;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Errors that can occur during migration execution.
///
/// These propagate as the `error` field on [`MigrationResult`]; the
/// executor itself never panics — every failure becomes a structured
/// `MigrationResult { success: false, error: Some(...) }`.
#[derive(Debug, thiserror::Error)]
pub enum MigrationExecutionError {
    #[error("tier path not configured for tier {0:?}")]
    TierPathNotConfigured(PerformanceTier),

    #[error("filesystem error: {0}")]
    Filesystem(String),

    #[error("source and target paths resolved to the same URL: {0}")]
    SourceEqualsTarget(String),
}

/// Executor for tier-migration tasks. Construct once, share via `Arc`,
/// call `execute` or `execute_batch` from policy-engine driven loops.
pub struct TierMigrationExecutor {
    /// Routes I/O to the right backend based on URL scheme
    /// (file:// → LocalFs, s3:// → S3, etc.).
    filesystem: Arc<FilesystemFactory>,

    /// Per-tier URL base. Resolved from `SstTieringConfig.{hot,warm,
    /// cold,archive}_tier_path` at construction. Missing entries cause
    /// `TierPathNotConfigured` errors at execute time.
    tier_paths: HashMap<PerformanceTier, String>,
}

impl TierMigrationExecutor {
    /// Construct a new executor with the given filesystem factory and
    /// per-tier URL bases. The bases are typically loaded from
    /// `SstTieringConfig`; pass them in pre-validated.
    pub fn new(
        filesystem: Arc<FilesystemFactory>,
        tier_paths: HashMap<PerformanceTier, String>,
    ) -> Self {
        Self {
            filesystem,
            tier_paths,
        }
    }

    /// Build the per-tier path map directly from an `SstTieringConfig`.
    /// Convenience for the bootstrap path in `SharedServices` so the
    /// executor and the integration share a single source of truth.
    pub fn from_tiering_config(
        filesystem: Arc<FilesystemFactory>,
        cfg: &crate::storage::engines::sst::tiering_integration::SstTieringConfig,
    ) -> Self {
        let mut tier_paths = HashMap::new();
        if let Some(p) = cfg.hot_tier_path.clone() {
            tier_paths.insert(PerformanceTier::Hot, p);
        }
        if let Some(p) = cfg.warm_tier_path.clone() {
            tier_paths.insert(PerformanceTier::Warm, p);
        }
        if let Some(p) = cfg.cold_tier_path.clone() {
            tier_paths.insert(PerformanceTier::Cold, p);
        }
        if let Some(p) = cfg.archive_tier_path.clone() {
            tier_paths.insert(PerformanceTier::Archive, p);
        }
        Self::new(filesystem, tier_paths)
    }

    /// Resolve a tier + collection + item to a fully-qualified URL.
    ///
    /// Layout: `{tier_base}/{collection}/{item_id}`. The collection
    /// component scopes per-tenant data so two collections with the
    /// same item_id (e.g. "seg-0001.sst") don't collide across
    /// tenants.
    fn resolve_url(
        &self,
        tier: PerformanceTier,
        collection: &str,
        item_id: &str,
    ) -> Result<String, MigrationExecutionError> {
        let base = self
            .tier_paths
            .get(&tier)
            .ok_or(MigrationExecutionError::TierPathNotConfigured(tier))?;
        // Trim trailing slash from base to avoid `file:///path//collection`.
        let base_trimmed = base.trim_end_matches('/');
        Ok(format!("{}/{}/{}", base_trimmed, collection, item_id))
    }

    /// Execute a single migration task. Never panics; failures land on
    /// the returned `MigrationResult.error`.
    ///
    /// Metrics: every invocation increments the in-flight gauge for
    /// the duration of the call and emits a final
    /// `proximadb_tier_migrations_total` / `_bytes_total` /
    /// `_duration_seconds` triple at exit. The in-flight gauge is
    /// guarded by RAII so a panic still decrements it.
    pub async fn execute(&self, task: &MigrationTask) -> MigrationResult {
        let _in_flight = crate::metrics::tier_migration_metrics::InFlightGuard::enter();
        let result = self.execute_inner(task).await;
        crate::metrics::tier_migration_metrics::record_migration_result(&result);
        result
    }

    /// Internal: the actual migration logic. Separated from `execute`
    /// so the in-flight gauge guard and the metrics emission can wrap
    /// the body without leaking gauge state on early returns.
    async fn execute_inner(&self, task: &MigrationTask) -> MigrationResult {
        let started = Instant::now();

        let source_url = match self.resolve_url(task.source_tier, &task.collection, &task.item_id) {
            Ok(u) => u,
            Err(e) => return failed_result(task, started, e.to_string()),
        };
        let target_url = match self.resolve_url(task.target_tier, &task.collection, &task.item_id) {
            Ok(u) => u,
            Err(e) => return failed_result(task, started, e.to_string()),
        };

        if source_url == target_url {
            return failed_result(
                task,
                started,
                MigrationExecutionError::SourceEqualsTarget(source_url).to_string(),
            );
        }

        debug!(
            "🪜 TierMigrationExecutor: migrating {} from {} to {}",
            task.id, source_url, target_url
        );

        // Idempotency check: if the target already exists, treat as a
        // resumption of a previously-completed migration whose source
        // delete didn't finish. Skip the copy and just retry the delete.
        let target_already_exists = match self.filesystem.exists(&target_url).await {
            Ok(exists) => exists,
            Err(e) => {
                warn!(
                    "🪜 TierMigrationExecutor: exists() check on target failed ({}); proceeding with full move",
                    e
                );
                false
            }
        };

        if target_already_exists {
            info!(
                "🪜 TierMigrationExecutor: target {} already exists; retrying source-delete only (resumed migration)",
                target_url
            );
            // Best-effort delete of the source; if source doesn't exist,
            // the migration is fully complete from a prior run.
            let _ = self.filesystem.delete(&source_url).await;
            return MigrationResult {
                task_id: task.id.clone(),
                collection: task.collection.clone(),
                item_id: task.item_id.clone(),
                source_tier: task.source_tier,
                target_tier: task.target_tier,
                success: true,
                bytes_migrated: task.estimated_bytes,
                duration: started.elapsed(),
                error: None,
            };
        }

        // Ensure the target's parent directory exists. LocalFs
        // open_file(create=true) doesn't auto-create parents and
        // cloud backends sometimes treat missing prefixes silently —
        // so we explicitly prep the parent on every move. Best-effort:
        // if create_dir_all reports an error, we continue and let
        // move_atomic surface the canonical write-time error.
        if let Some((parent_url, _)) = target_url.rsplit_once('/')
            && let Err(e) = self.filesystem.create_dir_all(parent_url).await
        {
            debug!(
                "🪜 TierMigrationExecutor: create_dir_all on {} returned {} (continuing)",
                parent_url, e
            );
        }

        // Atomic copy then delete via the filesystem layer. The
        // filesystem's move_atomic handles same-backend renames and
        // cross-backend copy-then-delete uniformly.
        let move_result = self.filesystem.move_atomic(&source_url, &target_url).await;

        match move_result {
            Ok(()) => {
                let bytes_migrated = match self.filesystem.metadata(&target_url).await {
                    Ok(md) => md.size,
                    Err(_) => task.estimated_bytes, // fall back to estimate
                };

                MigrationResult {
                    task_id: task.id.clone(),
                    collection: task.collection.clone(),
                    item_id: task.item_id.clone(),
                    source_tier: task.source_tier,
                    target_tier: task.target_tier,
                    success: true,
                    bytes_migrated,
                    duration: started.elapsed(),
                    error: None,
                }
            }
            Err(e) => failed_result(
                task,
                started,
                MigrationExecutionError::Filesystem(e.to_string()).to_string(),
            ),
        }
    }

    /// Execute multiple tasks concurrently, bounded by
    /// `max_concurrent`. The returned `Vec<MigrationResult>` is in the
    /// SAME order as `tasks`, regardless of which finished first.
    pub async fn execute_batch(
        &self,
        tasks: &[MigrationTask],
        max_concurrent: usize,
    ) -> Vec<MigrationResult> {
        if tasks.is_empty() {
            return Vec::new();
        }
        let limit = max_concurrent.max(1);
        let sem = Arc::new(Semaphore::new(limit));
        let mut handles = Vec::with_capacity(tasks.len());

        for (idx, task) in tasks.iter().cloned().enumerate() {
            let sem = sem.clone();
            let exec = self.clone_for_task();
            let h = tokio::spawn(async move {
                // `Semaphore::acquire` only fails after the semaphore is
                // explicitly closed via `close()`, which we never do.
                // Holding the permit for the lifetime of the task is the
                // intended back-pressure mechanism.
                #[allow(clippy::expect_used)]
                let _permit = sem.acquire().await.expect("semaphore closed");
                let result = exec.execute(&task).await;
                (idx, result)
            });
            handles.push(h);
        }

        // Collect in the original order so callers can correlate
        // results with their input task list by index.
        let mut indexed: Vec<(usize, MigrationResult)> = Vec::with_capacity(handles.len());
        for h in handles {
            match h.await {
                Ok(pair) => indexed.push(pair),
                Err(join_err) => {
                    // A task panicked or was cancelled. Synthesize a
                    // failed result at index 0 so the caller still
                    // sees the failure — we lost the task identity
                    // to the join error, but better than dropping
                    // silently.
                    indexed.push((
                        usize::MAX,
                        MigrationResult {
                            task_id: "<panicked>".to_string(),
                            collection: String::new(),
                            item_id: String::new(),
                            source_tier: PerformanceTier::Warm,
                            target_tier: PerformanceTier::Warm,
                            success: false,
                            bytes_migrated: 0,
                            duration: std::time::Duration::from_secs(0),
                            error: Some(format!("task join failed: {}", join_err)),
                        },
                    ));
                }
            }
        }
        indexed.sort_by_key(|(idx, _)| *idx);
        indexed.into_iter().map(|(_, r)| r).collect()
    }

    /// Internal: produce a cheap clone for use across spawned tasks.
    /// `FilesystemFactory` is wrapped in `Arc`; the path map is
    /// snapshotted (small, copied once per spawn).
    fn clone_for_task(&self) -> TierMigrationExecutor {
        TierMigrationExecutor {
            filesystem: Arc::clone(&self.filesystem),
            tier_paths: self.tier_paths.clone(),
        }
    }
}

fn failed_result(task: &MigrationTask, started: Instant, error: String) -> MigrationResult {
    MigrationResult {
        task_id: task.id.clone(),
        collection: task.collection.clone(),
        item_id: task.item_id.clone(),
        source_tier: task.source_tier,
        target_tier: task.target_tier,
        success: false,
        bytes_migrated: 0,
        duration: started.elapsed(),
        error: Some(error),
    }
}

/// Mark a task as in-progress / completed / failed. Caller pattern:
/// take a `MigrationTask`, mutate its status to `InProgress`, pass to
/// `execute`, then mutate status from the returned `MigrationResult`.
///
/// This is a free helper rather than a method on the executor so the
/// caller stays in control of task lifetime.
pub fn apply_result_to_task(task: &mut MigrationTask, result: &MigrationResult) {
    task.status = if result.success {
        MigrationStatus::Completed
    } else {
        MigrationStatus::Failed
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    use crate::storage::persistence::filesystem::FilesystemFactory;

    async fn local_only_executor(hot_dir: &TempDir, cold_dir: &TempDir) -> TierMigrationExecutor {
        let cfg = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let factory = Arc::new(FilesystemFactory::create(cfg).await.expect("factory"));
        let mut paths = HashMap::new();
        paths.insert(
            PerformanceTier::Hot,
            format!("file://{}", hot_dir.path().display()),
        );
        paths.insert(
            PerformanceTier::Cold,
            format!("file://{}", cold_dir.path().display()),
        );
        TierMigrationExecutor::new(factory, paths)
    }

    fn demotion_task(collection: &str, item_id: &str, estimated_bytes: u64) -> MigrationTask {
        MigrationTask::new(
            collection.to_string(),
            item_id.to_string(),
            PerformanceTier::Hot,
            PerformanceTier::Cold,
            estimated_bytes,
        )
    }

    #[tokio::test]
    async fn execute_fails_when_source_tier_path_missing() {
        let factory = Arc::new(
            FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .unwrap(),
        );
        // Only Cold is configured; source Hot is missing.
        let mut paths = HashMap::new();
        paths.insert(PerformanceTier::Cold, "file:///tmp/cold".to_string());
        let exec = TierMigrationExecutor::new(factory, paths);

        let task = demotion_task("c1", "seg-1.sst", 1024);
        let result = exec.execute(&task).await;

        assert!(!result.success);
        let err = result.error.expect("error message");
        assert!(
            err.contains("tier path not configured") && err.contains("Hot"),
            "error must name the missing source tier: {}",
            err
        );
    }

    #[tokio::test]
    async fn execute_fails_when_source_equals_target() {
        let factory = Arc::new(
            FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .unwrap(),
        );
        // Both tiers point at the same base — a misconfiguration we
        // refuse to act on so we don't truncate the source.
        let mut paths = HashMap::new();
        paths.insert(PerformanceTier::Hot, "file:///tmp/shared".to_string());
        paths.insert(PerformanceTier::Cold, "file:///tmp/shared".to_string());
        let exec = TierMigrationExecutor::new(factory, paths);

        let task = demotion_task("c1", "seg-1.sst", 1024);
        let result = exec.execute(&task).await;

        assert!(!result.success);
        let err = result.error.expect("error message");
        assert!(
            err.contains("same URL"),
            "error must name the same-URL condition: {}",
            err
        );
    }

    #[tokio::test]
    async fn execute_moves_local_file_between_tiers() {
        let hot = TempDir::new().expect("hot dir");
        let cold = TempDir::new().expect("cold dir");

        // Pre-seed a source file under {hot}/c1/seg-1.sst.
        let coll_dir = hot.path().join("c1");
        std::fs::create_dir_all(&coll_dir).expect("mkdir");
        let src_path = coll_dir.join("seg-1.sst");
        std::fs::write(&src_path, b"payload").expect("write src");

        let exec = local_only_executor(&hot, &cold).await;
        let task = demotion_task("c1", "seg-1.sst", 7);
        let result = exec.execute(&task).await;

        assert!(result.success, "migration must succeed: {:?}", result.error);
        assert_eq!(
            result.bytes_migrated, 7,
            "bytes_migrated must match actual file size"
        );

        // Source gone, target present.
        assert!(
            !src_path.exists(),
            "source file must be removed after move_atomic"
        );
        let dst_path = cold.path().join("c1").join("seg-1.sst");
        assert!(dst_path.exists(), "target file must exist after migration");
    }

    #[tokio::test]
    async fn execute_is_idempotent_when_target_already_exists() {
        // Simulate a crash mid-migration: target file exists, source
        // file is also still there. A retry must complete the
        // source-delete step and report success.
        let hot = TempDir::new().expect("hot dir");
        let cold = TempDir::new().expect("cold dir");

        // Pre-seed BOTH source and target — this is the crash state.
        let src_coll_dir = hot.path().join("c1");
        let dst_coll_dir = cold.path().join("c1");
        std::fs::create_dir_all(&src_coll_dir).expect("mkdir src");
        std::fs::create_dir_all(&dst_coll_dir).expect("mkdir dst");
        let src_path = src_coll_dir.join("seg-1.sst");
        let dst_path = dst_coll_dir.join("seg-1.sst");
        std::fs::write(&src_path, b"old-payload").expect("write src");
        std::fs::write(&dst_path, b"new-payload").expect("write dst");

        let exec = local_only_executor(&hot, &cold).await;
        let task = demotion_task("c1", "seg-1.sst", 11);
        let result = exec.execute(&task).await;

        assert!(
            result.success,
            "resumption must report success: {:?}",
            result.error
        );
        // Source cleaned up, target preserved (the migration is "done").
        assert!(
            !src_path.exists(),
            "source must be deleted on resumption to converge to single-tier state"
        );
        assert!(
            dst_path.exists(),
            "target must remain intact — it represents the completed migration"
        );
    }

    #[tokio::test]
    async fn execute_batch_preserves_input_order() {
        let hot = TempDir::new().expect("hot dir");
        let cold = TempDir::new().expect("cold dir");
        let coll_dir = hot.path().join("c1");
        std::fs::create_dir_all(&coll_dir).expect("mkdir");
        for i in 0..4 {
            std::fs::write(coll_dir.join(format!("seg-{}.sst", i)), b"x").expect("write");
        }
        let exec = local_only_executor(&hot, &cold).await;
        let tasks: Vec<_> = (0..4)
            .map(|i| demotion_task("c1", &format!("seg-{}.sst", i), 1))
            .collect();

        let results = exec.execute_batch(&tasks, 2).await;
        assert_eq!(results.len(), 4);
        for (i, r) in results.iter().enumerate() {
            assert!(r.success, "task {} must succeed: {:?}", i, r.error);
            assert_eq!(
                r.item_id,
                format!("seg-{}.sst", i),
                "result at index {} must correspond to task at same index",
                i
            );
        }
    }

    #[tokio::test]
    async fn execute_batch_handles_empty_input() {
        let factory = Arc::new(
            FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .unwrap(),
        );
        let exec = TierMigrationExecutor::new(factory, HashMap::new());
        let results = exec.execute_batch(&[], 4).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn apply_result_to_task_sets_completed_on_success() {
        let mut task = demotion_task("c1", "seg-1.sst", 1);
        let result = MigrationResult {
            task_id: task.id.clone(),
            collection: task.collection.clone(),
            item_id: task.item_id.clone(),
            source_tier: task.source_tier,
            target_tier: task.target_tier,
            success: true,
            bytes_migrated: 1,
            duration: Duration::from_millis(5),
            error: None,
        };
        apply_result_to_task(&mut task, &result);
        assert_eq!(task.status, MigrationStatus::Completed);
    }

    #[tokio::test]
    async fn apply_result_to_task_sets_failed_on_error() {
        let mut task = demotion_task("c1", "seg-1.sst", 1);
        let result = MigrationResult {
            task_id: task.id.clone(),
            collection: task.collection.clone(),
            item_id: task.item_id.clone(),
            source_tier: task.source_tier,
            target_tier: task.target_tier,
            success: false,
            bytes_migrated: 0,
            duration: Duration::from_millis(5),
            error: Some("boom".to_string()),
        };
        apply_result_to_task(&mut task, &result);
        assert_eq!(task.status, MigrationStatus::Failed);
    }
}
