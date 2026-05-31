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
printf '%-30s %6s %10s %12s   %s\n' \
  "cell" "ef" "recall" "obs_factor" "matches_recall_target_around"

for f in "$DIR"/*.txt; do
  [ -f "$f" ] || continue
  name=$(basename "$f" .txt)
  recall=$(grep -A1 "AXIS vs force_exact" "$f" 2>/dev/null | tail -1 \
           | awk -F'mean: ' '{print $2}' | awk '{print $1}')
  # Try strategy-spec ef first (matrix bench), then axis_diag.
  ef=$(grep "site=insert_into_hnsw" "$f" 2>/dev/null | head -1 \
       | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  if [ -z "$ef" ]; then
    ef=$(grep "site=insert_hmgi" "$f" 2>/dev/null | head -1 \
         | sed -nE 's/.*ef_search=([0-9]+).*/\1/p')
  fi
  if [ -z "$recall" ] || [ -z "$ef" ]; then
    printf '%-30s %6s %10s %12s   %s\n' "$name" "?" "(running)" "" ""
    continue
  fi

  obs_factor=$(python3 -c "print(round($ef / $DENOM, 3))")

  # Map obs_factor → which recall_target in the advisor's TABLE
  # produces this factor. Pinned from
  # src/index/axis/management/hnsw_param_advisor.rs.
  target=$(python3 -c "
table = [
    (0.75, 0.12), (0.80, 0.14), (0.85, 0.16), (0.90, 0.18),
    (0.92, 0.25), (0.95, 0.37), (0.975, 0.55),
    (0.99, 0.82), (0.995, 1.10),
]
obs = $obs_factor
# Find the two table entries that bracket obs and linearly interpolate
# the recall they correspond to.
if obs <= table[0][1]:
    print(f'≤{table[0][0]:.3f}')
elif obs >= table[-1][1]:
    print(f'≥{table[-1][0]:.3f}')
else:
    for i in range(len(table) - 1):
        lo_r, lo_f = table[i]
        hi_r, hi_f = table[i+1]
        if lo_f <= obs <= hi_f:
            t = (obs - lo_f) / (hi_f - lo_f) if hi_f > lo_f else 0.0
            interp = lo_r + t * (hi_r - lo_r)
            print(f'{interp:.3f}')
            break
")

  printf '%-30s %6s %10s %12s   %s\n' \
    "$name" "$ef" "$recall" "$obs_factor" "$target"
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
