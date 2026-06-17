#!/usr/bin/env bash
# Matrix bench: AXIS index types × metrics, 10K vectors.
#
# Runs the pre-built bench binary DIRECTLY (no cargo invocations)
# so each cell only does its own setup+queries — no per-cell
# workspace rebuild overhead.
#
# Prerequisite: build the bench once with
#   cargo bench --bench bench_warm_path_profile --no-run
#
# Usage:
#   ./scripts/bench_axis_matrix.sh
#
# Tunables (env):
#   BENCH_VECTORS  (default 10000)
#   BENCH_DIM      (default 128)
#   BENCH_QUERIES  (default 20)
#   OUT_DIR        (default /tmp/proximadb_bench_matrix)

# NOTE: intentionally NOT enabling `pipefail`. The script uses
# `ls -t ... | grep -v ... | head -1` and grep | head | sed extractors
# in the summary block; `head -1` closes the pipe early which SIGPIPEs
# the upstream and trips pipefail (silent exit 141). Each step checks
# its own preconditions instead.
set -eu

VECTORS="${BENCH_VECTORS:-10000}"
DIM="${BENCH_DIM:-128}"
QUERIES="${BENCH_QUERIES:-20}"
OUT_DIR="${OUT_DIR:-/tmp/proximadb_bench_matrix}"

BIN_GLOB="$(dirname "$0")/../target/release/deps/bench_warm_path_profile-*"
BIN="$(ls -t $BIN_GLOB 2>/dev/null | grep -v '\.d$' | head -1)"

if [ ! -x "$BIN" ]; then
  echo "ERROR: bench binary not found. Build with:" >&2
  echo "  cargo bench --bench bench_warm_path_profile --no-run" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR"/*.txt

INDEXES=("flat" "hnsw" "hmgi" "ivf")
METRICS=("cosine" "euclidean" "dotproduct")

echo "================================================================="
echo "  ProximaDB AXIS matrix bench (direct binary)"
echo "================================================================="
echo "  binary   : $BIN"
echo "  vectors  : $VECTORS"
echo "  dim      : $DIM"
echo "  queries  : $QUERIES"
echo "  out_dir  : $OUT_DIR"
echo "  cells    : ${#INDEXES[@]} algos × ${#METRICS[@]} metrics = $((${#INDEXES[@]} * ${#METRICS[@]}))"
echo "-----------------------------------------------------------------"

for idx in "${INDEXES[@]}"; do
  for met in "${METRICS[@]}"; do
    name="${idx}_${met}"
    out="$OUT_DIR/${name}.txt"
    echo "→ running ${name}  → ${out}"
    BENCH_VECTORS="$VECTORS" \
    BENCH_DIM="$DIM" \
    BENCH_QUERIES="$QUERIES" \
    BENCH_INDEX="$idx" \
    BENCH_METRIC="$met" \
    "$BIN" 2>&1 | tee "$out" >/dev/null
  done
done

echo "================================================================="
echo "  Summary"
echo "================================================================="
printf '%-6s %-10s %-12s %-12s %-12s %-12s %-12s\n' \
  "INDEX" "METRIC" "WARM_p50_us" "ID_RECALL" "SCORE_REC" "AXIS_BUILD_ms" "FLUSH_ms"

for idx in "${INDEXES[@]}"; do
  for met in "${METRICS[@]}"; do
    out="$OUT_DIR/${idx}_${met}.txt"
    [ -f "$out" ] || continue
    p50=$(grep -E "^    total" "$out" | head -1 | awk -F'p50=' '{print $2}' | awk '{print $1}' | tr -d 'us')
    id_rec=$(grep -A1 "ID-overlap recall" "$out" | tail -1 | awk -F'mean: ' '{print $2}' | awk '{print $1}')
    sc_rec=$(grep -A1 "Score-threshold recall" "$out" | tail -1 | awk -F'mean: ' '{print $2}' | awk '{print $1}')
    [ -z "$id_rec" ] && id_rec="n/a"
    [ -z "$sc_rec" ] && sc_rec="n/a"
    build=$(grep "^Setup:" "$out" | sed -n 's/.*axis_build \([0-9]*\) ms.*/\1/p')
    [ -z "$build" ] && build="-"
    flush=$(grep "^Setup:" "$out" | sed -n 's/.*flush \([0-9]*\) ms.*/\1/p')
    printf '%-6s %-10s %-12s %-12s %-12s %-12s %-12s\n' \
      "$idx" "$met" "${p50:-?}" "$id_rec" "$sc_rec" "$build" "${flush:-?}"
  done
done
