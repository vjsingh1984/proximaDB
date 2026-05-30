#!/usr/bin/env bash
# TDD harness for BENCH_HNSW_EF end-to-end wiring.
#
# Verifies that setting BENCH_HNSW_EF=N in the environment flows
# through to:
#   1. The IndexAlgorithm::HNSW.ef_search field in the strategy spec
#      the bench builds and passes to update_collection_strategy.
#   2. The AxisHnswConfig.ef field that `insert_into_hnsw` reads from
#      the strategy spec (since 476dc951a wired this end-to-end).
#   3. The runtime `search_ef` used by AxisHnswIndex::search_with_filter
#      (which honors max(config.ef, sqrt(N), top_k)).
#
# Both insert and runtime values are surfaced via the axis_diag
# tracing target — the test greps the bench's stderr for those logs
# and asserts the values match the BENCH_HNSW_EF input.
#
# Failure modes this catches:
# * env_usize parser breaks
# * strategy construction silently ignores ef_search
# * insert_into_hnsw reverts to AxisHnswConfig::default ef (= 50)
# * search_with_filter loses the override
#
# Usage:
#   ./scripts/test_bench_hnsw_ef_wiring.sh
#
# Exit codes:
#   0 — all wiring checks pass
#   1 — a wiring check failed (details printed to stderr)

# NOTE: intentionally NOT enabling `pipefail`. The script uses many
# `grep ... | head -1 | sed ...` pipelines to scrape ef values out of
# bench stderr; `head -1` closes the pipe early which SIGPIPEs grep,
# and pipefail would surface that as a spurious exit 141. We
# explicitly check each extracted variable for the expected value
# instead.
set -eu

BIN_GLOB="$(dirname "$0")/../target/release/deps/bench_warm_path_profile-*"
BIN="$(ls -t $BIN_GLOB 2>/dev/null | grep -v '\.d$' | head -1)"

if [ ! -x "$BIN" ]; then
  echo "FAIL: bench binary not found. Build with:" >&2
  echo "  cargo bench --bench bench_warm_path_profile --no-run" >&2
  exit 1
fi

# Small N keeps each cell sub-second. Queries=1 means we get exactly
# one axis_diag event per cell — easier to assert on. Picking ef
# values that span both sides of sqrt(N): at N=1000 the size_aware_ef
# floor is sqrt(1000)≈31 clamped to 50, so any ef >= 50 should
# survive the `config.ef.max(size_aware_ef)` computation in
# search_with_filter.
N=1000
EF_VALUES=(50 100 200 300 500)
FAIL=0
LOG=/tmp/bench_ef_wiring_test.log

echo "================================================================="
echo "  TDD: BENCH_HNSW_EF end-to-end wiring"
echo "================================================================="
echo "  binary: $BIN"
echo "  N:      $N vectors (small for fast iteration)"
echo "  ef set: ${EF_VALUES[*]}"
echo "-----------------------------------------------------------------"

echo "  [BENCH_INDEX=hnsw] (legacy HNSW path: insert_into_hnsw)"
for EF in "${EF_VALUES[@]}"; do
  printf "    ef=%-4s  " "$EF"

  BENCH_HNSW_EF=$EF \
  BENCH_VECTORS=$N BENCH_DIM=128 BENCH_QUERIES=1 \
  BENCH_INDEX=hnsw BENCH_METRIC=cosine \
  "$BIN" > "$LOG" 2>&1 || {
    echo "FAIL: bench binary exited non-zero for ef=$EF" >&2
    FAIL=1
    continue
  }

  insert_ef=$(grep "site=insert_into_hnsw" "$LOG" | head -1 \
    | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  search_ef=$(grep "site=AxisHnswIndex::search_with_filter" "$LOG" \
    | head -1 | sed -nE 's/.*search_ef=([0-9]+).*/\1/p')
  spec_source=$(grep "site=insert_into_hnsw" "$LOG" | head -1 \
    | sed -nE 's/.*spec_source=([a-z_]+).*/\1/p')

  EXPECTED=$EF
  expected_search_ef=$([ "$EF" -lt 50 ] && echo "50" || echo "$EF")

  if [ "$insert_ef" != "$EXPECTED" ]; then
    echo "FAIL: insert_into_hnsw.ef_search expected $EXPECTED, got '$insert_ef'"
    FAIL=1
  elif [ "$search_ef" != "$expected_search_ef" ]; then
    echo "FAIL: search_with_filter.search_ef expected $expected_search_ef, got '$search_ef'"
    FAIL=1
  elif [ "$spec_source" != "strategy" ]; then
    echo "FAIL: spec_source expected 'strategy', got '$spec_source'"
    FAIL=1
  else
    echo "OK   (insert.ef_search=$insert_ef  runtime.search_ef=$search_ef  spec_source=$spec_source)"
  fi
done

echo ""
echo "  [BENCH_INDEX=hmgi] (HMGI partition: insert_hmgi)"
# HMGI cell now also calls update_collection_strategy with the
# BENCH_HNSW_EF spec, so the HMGI partition's HNSW config should
# track the env var (was previously hardcoded to 50 via
# AxisHnswConfig::default()).
for EF in "${EF_VALUES[@]}"; do
  printf "    ef=%-4s  " "$EF"

  BENCH_HNSW_EF=$EF \
  BENCH_VECTORS=$N BENCH_DIM=128 BENCH_QUERIES=1 \
  BENCH_INDEX=hmgi BENCH_METRIC=cosine \
  "$BIN" > "$LOG" 2>&1 || {
    echo "FAIL: bench binary exited non-zero for ef=$EF" >&2
    FAIL=1
    continue
  }

  insert_ef=$(grep "site=insert_hmgi" "$LOG" | head -1 \
    | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  search_ef=$(grep "site=AxisHnswIndex::search_with_filter" "$LOG" \
    | head -1 | sed -nE 's/.*search_ef=([0-9]+).*/\1/p')
  spec_source=$(grep "site=insert_hmgi" "$LOG" | head -1 \
    | sed -nE 's/.*spec_source=([a-z_]+).*/\1/p')

  EXPECTED=$EF
  expected_search_ef=$([ "$EF" -lt 50 ] && echo "50" || echo "$EF")

  if [ "$insert_ef" != "$EXPECTED" ]; then
    echo "FAIL: insert_hmgi.ef_search expected $EXPECTED, got '$insert_ef'"
    FAIL=1
  elif [ "$search_ef" != "$expected_search_ef" ]; then
    echo "FAIL: search_with_filter.search_ef expected $expected_search_ef, got '$search_ef'"
    FAIL=1
  elif [ "$spec_source" != "strategy" ]; then
    echo "FAIL: spec_source expected 'strategy', got '$spec_source'"
    FAIL=1
  else
    echo "OK   (insert.ef_search=$insert_ef  runtime.search_ef=$search_ef  spec_source=$spec_source)"
  fi
done

echo "-----------------------------------------------------------------"
if [ "$FAIL" -eq 0 ]; then
  echo "  ✅ all wiring checks passed"
  rm -f "$LOG"
  exit 0
else
  echo "  ❌ at least one wiring check failed (last bench output in $LOG)"
  exit 1
fi
