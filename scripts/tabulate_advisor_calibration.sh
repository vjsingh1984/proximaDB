#!/usr/bin/env bash
# Calibration tabulator for the HNSW param advisor.
#
# Reads the output of a BENCH_HNSW_M / BENCH_HNSW_EF sweep and
# prints (ef, observed_recall, observed_factor, predicted_factor)
# rows + a summary delta. Used to validate / re-tune the
# `recall_factor` table in
# src/index/axis/management/hnsw_param_advisor.rs after a sweep.
#
# The advisor's formula is:
#   raw_ef = k · log₂(N) · log₂(N / 1000) · factor
# Inverted:
#   factor = ef / (k · log₂(N) · log₂(N / 1000))
#
# For N=100K, k=10 the denominator is 10 · 16.61 · 6.64 ≈ 1102.
#
# Usage:
#   ./scripts/tabulate_advisor_calibration.sh /tmp/proximadb_bench_hnsw_m32_sweep_100K
#
# Optional args:
#   N    (default 100000) — must match BENCH_VECTORS the sweep used
#   K    (default 10)     — must match BENCH_TOPK the sweep used
#
# Output schema (TSV-ish, fixed-width):
#   cell                          ef     recall   obs_factor   predicted_recall_for_factor
#
# `predicted_recall_for_factor` looks up the recall_target the
# advisor would map to this factor, so you can eyeball
# over/under-promise.

set -eu

DIR="${1:-/tmp/proximadb_bench_hnsw_m32_sweep_100K}"
N="${N:-100000}"
K="${K:-10}"

if [ ! -d "$DIR" ]; then
  echo "ERROR: directory not found: $DIR" >&2
  exit 1
fi

# Compute the denominator k · log2(N) · log2(N/1000) once.
# bc -l gives floats; the 16.61 / 6.64 for N=100K are pinned in
# the advisor source so we recompute here to match other Ns.
DENOM=$(python3 -c "
import math
N = $N
K = $K
print(K * math.log2(N) * math.log2(max(2, N / 1000)))
")

echo "================================================================"
echo "  Advisor calibration tabulation"
echo "================================================================"
echo "  dir        : $DIR"
echo "  N, k       : $N, $K"
echo "  denom      : $DENOM  (= k · log₂(N) · log₂(N/1000))"
echo "----------------------------------------------------------------"
printf '%-30s %4s %6s %10s %12s   %s\n' \
  "cell" "algo" "knob" "recall" "obs_factor" "matches_recall_target_around"

for f in "$DIR"/*.txt; do
  [ -f "$f" ] || continue
  name=$(basename "$f" .txt)
  recall=$(grep -A1 "AXIS vs force_exact" "$f" 2>/dev/null | tail -1 \
           | awk -F'mean: ' '{print $2}' | awk '{print $1}')

  # Detect algorithm + extract its tunable knob.
  # Priority: HNSW (insert_into_hnsw) → HMGI (insert_hmgi) → IVF (insert_into_ivf).
  algo=""
  knob=""
  if grep -q "site=insert_into_hnsw" "$f" 2>/dev/null; then
    algo="hnsw"
    knob=$(grep "site=insert_into_hnsw" "$f" 2>/dev/null | head -1 \
           | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  elif grep -q "site=insert_hmgi" "$f" 2>/dev/null; then
    algo="hmgi"
    knob=$(grep "site=insert_hmgi" "$f" 2>/dev/null | head -1 \
           | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  elif grep -q "site=insert_into_ivf" "$f" 2>/dev/null; then
    algo="ivf"
    knob=$(grep "site=insert_into_ivf" "$f" 2>/dev/null | head -1 \
           | sed -nE 's/.*n_probe=([0-9]+).*/\1/p')
  fi

  if [ -z "$recall" ] || [ -z "$knob" ]; then
    printf '%-30s %4s %6s %10s %12s   %s\n' \
      "$name" "${algo:-?}" "?" "(running)" "" ""
    continue
  fi

  # Per-algorithm factor + target computation. Constants are
  # pinned from the corresponding Rust modules — keep these in
  # sync with hnsw_param_advisor.rs / ivf_param_advisor.rs.
  case "$algo" in
    hnsw|hmgi)
      # HNSW factor = ef / (k · log₂(N) · log₂(N/1000)). Recall
      # target lookup uses the closed-form formula at m=32 (the
      # mid-band tier — most operational recall_targets land here).
      obs_factor=$(python3 -c "print(round($knob / $DENOM, 3))")
      target=$(python3 -c "
import math
ef = $knob
denom = $DENOM
# Formula at m=32: recall = 1.0 - 0.195 · exp(-3.7 · ef / denom)
factor = ef / denom
predicted_recall = max(0.0, min(1.0, 1.0 - 0.195 * math.exp(-3.7 * factor)))
print(f'{predicted_recall:.3f}')
")
      ;;
    ivf)
      # IVF factor = nprobe / nlist (operationally meaningful —
      # 'fraction of clusters probed'). Recall target via the
      # P2-calibrated formula: ceiling - A · exp(-γ · nprobe).
      # Default to ceiling=0.74 (the N=100K anchor) when N
      # crosses the ceiling threshold.
      obs_factor=$(python3 -c "print(round($knob, 1))")
      target=$(python3 -c "
import math
nprobe = $knob
# IVF formula (single-stage, no rerank): recall = ceiling -
# 0.41 · exp(-0.037 · nprobe). Ceiling = 0.74 at the N=100K
# anchor; smaller N hits 1.0 ceiling because full-scan
# becomes reachable.
n = $N
if n <= 25_000:
    ceiling = 1.0
elif n <= 100_000:
    ceiling = 0.74
elif n <= 330_000:
    ceiling = 0.68
else:
    ceiling = 0.77
predicted = max(0.0, min(ceiling, ceiling - 0.41 * math.exp(-0.037 * nprobe)))
print(f'{predicted:.3f}')
")
      ;;
    *)
      obs_factor="?"
      target="(unsupported)"
      ;;
  esac

  printf '%-30s %4s %6s %10s %12s   %s\n' \
    "$name" "$algo" "$knob" "$recall" "$obs_factor" "$target"
done

echo "----------------------------------------------------------------"
echo "Reading the result:"
echo " * obs_factor = ef / denom — the calibration factor the data shows."
echo " * matches_recall_target_around = the advisor table maps this"
echo "   factor to roughly this recall_target."
echo " * If observed recall is much HIGHER than"
echo "   matches_recall_target_around, advisor OVER-provisions"
echo "   (calls for more ef than needed)."
echo " * If observed recall is much LOWER, advisor UNDER-provisions"
echo "   (the table is too optimistic; ef won't deliver)."
