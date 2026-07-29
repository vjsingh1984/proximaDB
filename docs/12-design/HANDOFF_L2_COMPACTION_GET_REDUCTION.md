# Handoff: L2+ Compaction GET-Reduction — Review + Pressure-Test + Implement

## Context

ProximaDB's SIFT1M end-to-end benchmark measured **205 GETs/query** at 1M scale — 10× the competitive threshold. Root cause: 9 PAX segments (3 L0 + 1 L1 + 5 L2) because the compaction framework's `level_multiplier=2.0` makes the L2 file-count threshold unreachable (5 × 2² = 20; only 5 L2 files exist). Each segment adds ~7-8 reads per cold query. Fix: `level_multiplier=1.0` → L2 threshold drops to 5 → L2 files merge → 1-2 segments → GETs/query drops to ~25.

Full design: `docs/12-design/adr/ADR-080-vector-search-compaction-tuning.adoc` + `docs/10-quality/td/TD-COMPACT-10-l2-segment-consolidation-gap.adoc` (on develop after this docs PR merges).

## What to Review

1. **Is `level_multiplier=1.0` correct for vector search?** The trade-off: ~2× more frequent L2 merges (cheap: $0.0065/merge) vs ~5× fewer segments (the GET multiplier). For a cloud DB where GETs dominate COGS, fewer segments is the right optimization.

2. **Is the settle pass (D2) needed, or is `level_multiplier=1.0` alone sufficient?** During sustained ingest, the L0 early-return (`compaction_utils.rs:264`) starves the higher-level loop. After ingest settles, the loop runs and L2 compaction triggers (with multiplier=1.0). The settle pass accelerates this. Is it necessary, or does the existing post-ingest compaction suffice?

3. **Does the `compaction_utils.rs:287-350` higher-level loop work correctly with multiplier=1.0?** The loop computes `level_file_threshold = l0_file_threshold × level_multiplier^level` = `5 × 1.0^level` = 5 for all levels. At L2 with 5 files: `should_compact = true` → produces a `CompactionTaskInfo { source_level: 2, target_level: 3 }`. Does the downstream `perform_compaction_enhanced` handle L2→L3 correctly?

4. **The GET model (7-8 reads/segment)** — is this accurate? The per-segment reads: 1 header (72B) + 1 A0 directory (~KB) + 1-2 probed RaBitQ ranges (~3MB, coalesced) + 1 footer (~KB) + 1-2 SQ8 survivor ranges (~5-10MB) + 1-2 OID ranges (~256KB). With SegmentInvariantsCache + SurvivorRangeCache, the header/A0/footer should hit the cache after the first query per segment. Is the model correct?

5. **The acceptance criteria** — ≤30 GETs/query at 1M cold. With level_multiplier=1.0 + flush_floor=128MB, will the 1M corpus settle to 1-2 segments?

## VERIFIED Results (level_multiplier=1.0, 2026-07-29)

| Metric | Before (9 segs) | After (2 segs) |
|---|---|---|
| Segments | 9 | **2** |
| Cold GETs/query | 205 | **48.5** (4.2x) |
| Cold p50 | 149.6ms | **44.8ms** (3.3x) |
| Recall@10 | 0.978 | **0.9815** |
| COGS/M ($0.005/10K Hot) | $103 | **$24.25** |
| Margin @ $50/M | -106% | **+51.5%** |

nprobe = sqrt(k_c) scales with segment size. 2 large segments (nprobe~14 each)
vs 9 small (nprobe~6 each). Cell probes: 28 vs 54 = 2x, not 4.5x.

Path to <=20: fix invariants cache (-4), warm survivor (-7), coalescing (-2).

## Original Measured Numbers (2026-07-29, develop @ 79209453a)

- Ingest: 1M in 29.5s (33,888 vec/s), flush 0.35-0.43s each (async compaction)
- Recall@10: 0.978
- Cold p50/p95: 149.6ms / 177.4ms
- GETs/query (cold): 205 (40,991 total / 200 queries)
- Bytes/query: 172.1 MB
- Survivor hit% (cold): 22.4%
- Invariants hit% (cold): 33.2%
- Segments: 9 (3 L0 @ 7.1MB + 1 L1 @ 43MB + 5 L2 @ 183-226MB)

## Competitor Economics (Azure, 1M vecs + 1M queries/month)

| Offering | Monthly | At 2 segments (GETs≤25) |
|---|---|---|
| ProximaDB @ 33% margin | $211 (current 205 GETs) | **$24** (projected) |
| S3 Vectors (AWS) | $2.53 | AWS-only, bare ANN |
| Turbopuffer (AWS) | $16.00 floor | Close to undercut |
| Qdrant (Azure VM) | $250 | **DECISIVE WIN** |
| Pinecone | $350 | **DECISIVE WIN** |

## Key Files

- `src/storage/common/compaction_utils.rs` — the unified compaction framework (lines 264-350: L0 check + higher-level loop).
- `src/core/config.rs` — `CompactionConfig { level_multiplier, l0_file_threshold, ... }`.
- `src/storage/engines/sst/segment_format.rs` — PaxSegmentScanner (the per-segment read path).
- `src/storage/engines/sst/flush/mod.rs` — `should_trigger_compaction` (the flush-path trigger).
- `src/storage/auto_flush_driver.rs` — the 30s tick driver (potential settle-pass site).
- `config/config.toml` — `[storage] compaction_config` section.

## Acceptance Criteria for the Implementation

1. `level_multiplier` = 1.0 (default in `CompactionConfig::default()`) + TOML-controllable.
2. After 1M SIFT ingest + 120s settle: ≤3 segments (not 9).
3. Cold GETs/query ≤ 30 (measured via `PROXIMADB_COUNT_FS_IO=1` + Prometheus `proximadb_object_store_gets_total`).
4. Recall@10 ≥ 0.98 maintained.
5. Retrieval margin > 50% at the current $50/M KRU rate.
6. No regression: the 1M async-compaction verification (ingest throughput, L0 bounded, recall) still passes.

## How to Verify

```bash
# 1. Boot with level_multiplier=1.0 + production flush_floor
# In config.toml [storage] section:
#   compaction_config = { level_multiplier = 1.0, ... }
# Or via env override if available.

# 2. Ingest 1M SIFT + settle
python3 scripts/bench/async_compaction_1m_verify.py

# 3. Wait 120s, count segments
find /tmp/proximadb/d* -name "*.pax" | wc -l   # expect ≤3

# 4. Cold sweep (restart server for cold caches)
pkill proximadb-server; sleep 3; <boot>; sleep 20
python3 scripts/bench/cache_hot_vs_cold_1m.py
# Or: python3 /tmp/sift1m_full_bench.py sweep

# 5. Check GETs/query
# Scrape proxidb_object_store_gets_total delta / NQ
# Expect ≤ 30 (not 205)
```
