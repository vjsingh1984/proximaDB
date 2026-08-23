# Cross-Model Review: ADR-076 + TD-COMPACT-6/7/8 — Inline Compaction → Async Queued Compaction + L0 Admission Control

**Date:** 2026-07-28
**Reviewers:** Claude Opus (initial analysis) → Victor (cross-model verification)
**Basis:** develop @ 290ee95ce, PR #1237 merge
**Artifacts reviewed:**
- `docs/12-design/adr/ADR-076-async-compaction-l0-admission-control.adoc` (Proposed)
- `docs/10-quality/td/TD-COMPACT-6-async-compaction.adoc` (Open)
- `docs/10-quality/td/TD-COMPACT-7-l0-admission-control.adoc` (Open)
- `docs/10-quality/td/TD-COMPACT-8-training-debounce.adoc` (Open)

---

## 1. Executive Summary

**Overall Verdict:** The Claude Opus review is technically sound. All structural claims about the compaction architecture, the "two instances" waste, the idle worker infrastructure, the missing admission control, and the missing debounce are verified against develop@290ee95ce source. The implementation TDs (6/7/8) correctly scope the code changes needed. No hallucinations, no invented APIs. One claim — "7 redundant compactions" — is an inference, not directly observable in code, but its reasoning chain is correct given the code structure.

---

## 2. Line-by-Line Verification of Key Claims

### 2.1 "Two Compaction Instances Created" Claim

**Review claim:** `engine.rs` creates two `Compaction` instances — one at construction time (L148) that is **idle** and one at `start()` (L211) that actually has workers.

**Code evidence (develop@290ee95ce):**

| Location | Code | Role |
|---|---|---|
| `storage/engine.rs:147-148` | `let sst_config = config.sst_config.clone().unwrap_or_default(); let compaction_manager = Arc::new(Compaction::new(sst_config).await?);` | Instance #1 — stored on `Self` at L165. **No `start_workers()` called.** |
| `storage/engine.rs:210-213` | `let sst_config = self.config.sst_config.clone().unwrap_or_default(); let mut temp_manager = Compaction::new(sst_config).await?; temp_manager.start_workers(2).await?; self.compaction_manager = Arc::new(temp_manager);` | Instance #2 — **replaces** the field. Workers started. |
| `storage/engine.rs:59-60` | `pub fn compaction_manager(&self) -> Arc<Compaction> { self.compaction_manager.clone() }` | Accessor — callers get instance #2. |
| `storage/engines/sst/core.rs:433-435` | `let compaction_manager = Some(Arc::new(Compaction::new(config.clone()).await.map_err(...)?));` | **SstEngine** has its own instance (separate from `StorageEngine`). |
| `storage/engines/sst/core.rs:747-748` | `pub fn compaction_manager(&self) -> Option<&Arc<Compaction>> { self.compaction_manager.as_ref() }` | **SstEngine** accessor — this is the one flush uses at `flush/mod.rs:749`. |

**Finding:** ✅ **VERIFIED.** Instance #1 (L148, idle) is a code smell but is also **harmless** — it's Arc-replaced before any compaction path touches it, and the instance it replaces has no workers and holds no resources. The "waste" is ~200 bytes of struct allocation. The SstEngine instance (`core.rs:433`) is the one that actually runs compaction and is a distinct entity from the StorageEngine one — this is architecture, not a bug.

**Severity adjustment:** The review's tone implies this is a bug. It is not — it's clean code (the replaced instance is dropped, no leak). It's aesthetic waste.

---

### 2.2 "Idle Worker Infrastructure" Claim

**Review claim:** `schedule_compaction` and `worker_loop` exist but are never called from the flush path; the inline `run_due_compaction` is the actual path.

**Code evidence (develop@290ee95ce):**

| Location | Code | Role |
|---|---|---|
| `compaction.rs:589-631` | `pub async fn schedule_compaction(&self, task: SstCompactionTask) -> Result<()>` | Priority-ordered enqueue with output-file dedup. **Never called from `flush/`.** |
| `compaction.rs:525-554` | `pub async fn start_workers(&mut self, worker_count: usize)` | Spawns tokio tasks running `worker_loop`. | 
| `compaction.rs:~792-902` | `async fn worker_loop(...)` | Dequeues tasks, runs `perform_compaction`. |
| `compaction.rs:642-658` | `pub async fn run_due_compaction(...)` | "Deterministic and test-safe: it does NOT enqueue to the background workers" — inline `.await` on `perform_compaction_enhanced`. |
| `flush/mod.rs:756-763` | `compaction.run_due_compaction(cid, collection_dir, self.config(), l0_threshold, ...).await` | **This is the actual call.** Inline, blocking. |
| `code grep "schedule_compaction" src/storage/engines/sst/flush/` | **No matches** | Confirmed: flush never calls schedule. |

**Finding:** ✅ **VERIFIED in full.** The flush path calls `run_due_compaction` (inline) not `schedule_compaction` (queued). The worker infrastructure at `compaction.rs:792-902` IS functional code — it dequeues, runs compaction, tracks stats, handles shutdown — but is gated behind `schedule_compaction` which flush never calls. The workers spin on `task_notify.notified()` and never wake for compaction work.

---

### 2.3 "7 Redundant Compactions" Claim

**Review claim:** Each flush compacts all current L0 segments, and each subsequent flush re-compacts the previous compaction's output, so 7 flushes → 7 compactions instead of 1 post-ingest pass.

**Reasoning chain:** Since the L0 counter resets after each inline compaction (the output becomes the "new L0"), the per-flush gate fires again on the next flush. With 7 flushes during a 1M-vector ingest, you get 7 compactions — each merging the previous compaction's output + the new flush segment.

**Code evidence:**

| Location | Code | Supports chain? |
|---|---|---|
| `flush/mod.rs:878` | `if files.len() >= threshold { return Ok(Some(threshold)); }` | ✅ L0 count gate fires at ≥ threshold. |
| `flush/mod.rs:731` | `self.should_trigger_compaction(storage_url, collection_tags).await?` | ✅ Called per flush. |
| `compaction.rs:756-763` | `run_due_compaction(...).await` — inline, blocks flush return. | ✅ Compaction runs inside flush, so output segments exist before the next flush. |
| `flush/mod.rs:860-870` | `should_trigger_compaction` re-discovers L0 files each call. | ✅ The next flush will see the compaction output as an L0 file. |

**Finding:** ✅ **INFERENCE VERIFIED AS SOUND.** The code structure confirms the mechanism: inline compaction means output segments are visible to the next flush's L0 discovery. The exact count (7) depends on the flush schedule (how many flushes per ingest), but the **structural redundancy** — that each flush re-compacts previous compaction outputs — is directly visible in the code. The TD-COMPACT-5 training arm compounds this: with `threshold=1`, it fires on **every** flush with an untrained L0, making the redundant-compaction problem worse.

---

### 2.4 "No Admission Control" Claim

**Review claim:** No `l0_slowdown_trigger` or `l0_stop_trigger` fields exist in `SstConfig`.

**Code evidence:**

| Search | Result |
|---|---|
| `grep "l0_slowdown\|l0_stop\|training_debounce\|l0_pause\|l0_throttle" src/core/config.rs` | **No matches** |
| `grep "l0_slowdown\|l0_stop" src/storage/` | **No matches** |
| `SstConfig` fields (`config.rs:1154-1227`) | No admission control fields. Fields are: `level_count`, `compaction_threshold`, `compaction_config`, `block_size_kb`, `compaction_strategy`, `compression`, `compression_level`, `bloom_filter_config`, `cache_size_mb`, `segment_invariants_cache_mb`, `survivor_cache_mb`, `max_files_per_level`, `level_size_multiplier`, `max_levels`, `background_thread_count`, `data_directory`, `mmap_enabled`, `prefetch_enabled`, `prefetch_size_kb`, `decompression_cache_config`, `vector_encoding_strategy`, `synonym_ring`. |

**Finding:** ✅ **VERIFIED.** No admission control fields exist. The only backpressure mechanism is the memtable-based one in `auto_flush_driver.rs`.

---

### 2.5 "No Training Debounce" Claim

**Review claim:** The TD-COMPACT-5 training arm fires on every flush with an untrained L0; there is no `AtomicI64 last_flush_epoch` or `training_debounce_secs` gate.

**Code evidence:**

| Location | Code | 
|---|---|
| `flush/mod.rs:885-906` | Training arm: `if !ivf_probe_enabled() { return Ok(None); }` then loops L0 files → `segment_is_untrained()` → returns `Some(1)`. **No time-based gating.** |
| `flush/mod.rs:912-930` | `segment_is_untrained()` — stat + 72-byte header read. |
| `compaction.rs:220-249` | `Compaction` struct — no `last_flush_epoch`, no `last_training_compaction` fields. |
| `grep "last_flush\|training_debounce" src/storage/engines/sst/` | **No matches.** |

**Finding:** ✅ **VERIFIED.** The training arm fires unconditionally per flush. No debounce exists. The TD correctly notes this is a problem only with the async switch — when compaction is inline, training runs-to-completion before the next flush, so the next flush sees a trained segment and the arm self-clears. When compaction is async (TD-COMPACT-6), the arm would re-fire before the worker finishes, producing duplicate training compactions.

---

## 3. Boundary Contract Verification (D4)

The ADR defines 4 surfaces. Against the code:

| Surface | Claim | Code Match | Notes |
|---|---|---|---|
| **Flush** | Currently owns compaction inline → should only produce L0 .pax | ✅ `flush/mod.rs:756-763` holds the inline call. The target is to NOT call anything. | TD-COMPACT-6 implements this. |
| **Compaction worker** | Already exists, just needs to be wired | ✅ `compaction.rs:792-902` (worker_loop) + `:589-631` (schedule). Workers are spawned at `engine.rs:212`. | The wire-up is: change `flush/mod.rs:756` from `run_due_compaction()` to `schedule_compaction()`. |
| **Admission control** | Check L0 watermarks at auto-flush driver | ✅ `auto_flush_driver.rs` is the right point — it already gates flush decisions. | TD-COMPACT-7. |
| **Training debounce** | Gate training arm with time since last compaction | ✅ Gate point at `flush/mod.rs:885` (training arm entry). New field on `Compaction` or `SstEngine`. | TD-COMPACT-8. |

✅ All boundaries correctly identified and matched to code locations.

---

## 4. Dimensional Co-Design Analysis (per CODESIGN_DIMENSIONAL_ARCHITECTURE)

| Dimension | ADR-076 Claim | Verified? | Notes |
|---|---|---|---|
| **Storage (KSU)** | Compaction writes under `DrPathBuilder` | ✅ | Uses same `collection_dir` as flush. |
| **Read/Compute (KRU/KIU)** | Query latency drops during ingest | ✅ | Compaction CPU moves off query-path threads to dedicated workers. |
| **Network Outgress (KOU)** | Not directly affected | ✅ | Intra-cloud I/O, not metered egress. |
| **Cache** | ADR-065 per-segment cache unaffected | ✅ | Compaction output inherits same cache population. |
| **Governance** | TenantContext flows to compaction I/O | ⚠️ **Unverified** | The code path uses `collection_dir` which is `DrPathBuilder`-prefixed, but `TenantContext` plumbing is not visible in the compaction code reviewed. This is noted for D1 implementation. |

---

## 5. Dependency Analysis: TD-COMPACT-6/7/8

| TD | Depends On | Blocks | Risk |
|---|---|---|---|
| **TD-COMPACT-6** (D1, async) | Nothing | TD-COMPACT-7 (watermarks only meaningful with async) | **Low** — 1-line call swap (`run_due_compaction` → `schedule_compaction`). The worker infra is already production-grade. |
| **TD-COMPACT-8** (D3, debounce) | Nothing (orthogonal to D1) | Nothing | **Low** — AtomicI64 + comparison in the training arm. |
| **TD-COMPACT-7** (D2, watermarks) | TD-COMPACT-6 (watermarks only useful when compaction is async) | Nothing | **Low** — 2 new fields on SstConfig + check in auto_flush_driver. |

**Implementation order recommendation:** TD-COMPACT-6 first (enables the whole design), then 7+8 in parallel (independent of each other, both depend on 6 for testing).

---

## 6. Findings: Agreement and Divergence

### Findings Agreed Upon (Claude Opus = Victor)

1. ✅ Inline compaction is blocking flush and causing redundant compactions.
2. ✅ Worker infrastructure (`schedule_compaction` + `worker_loop`) exists and is idle.
3. ✅ No admission control (L0 watermarks) exists.
4. ✅ Training arm has no debounce.
5. ✅ The fix path (TD-COMPACT-6 → 7+8) is correct and minimal.
6. ✅ All code citations are to real locations with matching code.
7. ✅ The dimensional analysis correctly identifies GETs as the dominant cost term.

### Divergence / Refinements

| Claim | Claude Opus | Victor | Resolution |
|---|---|---|---|
| "Two instances" is a bug | Implies it's a resource leak | It's aesthetic — Arc-replaced, no leak, no correctness impact | **Downgraded to code smell.** Not worthy of a TD. The review's concern is noted but over-weighted. |
| "7 redundant compactions" | Stated as measured fact | Inference from code structure — depends on flush count per ingest | **Correct inference.** The mechanism is sound. The exact count varies; annotate with "up to N-1 redundant" rather than "7". |
| `schedule_compaction` dedup | Not mentioned in review | `compaction.rs:601-609` has active-compaction dedup | **Missing from review.** The review could have noted this as a design precedent (the queue already has dedup logic the async path inherits). |
| `SstEngine.compaction_manager()` vs `StorageEngine.compaction_manager()` | Review doesn't distinguish them | They are different instances, different accessor paths | **Documentation gap.** The review should note which instance flush uses (`SstEngine`'s via `core.rs:747`) vs which one the `StorageEngine` accessor returns. |
| `#[cfg(test)]` inline path | Not in TD | The TD says "The inline path remains as a `#[cfg(test)]` synchronous option" | **Design note only.** No `#[cfg(test)]` gate currently exists. The implementation should add it when D1 ships. |

---

## 7. Hallucination / Fabrication Check

| Check