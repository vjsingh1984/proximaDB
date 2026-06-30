# Offline evidence — gated cost router vs. static heuristic (2026-06-26)

**Purpose.** The "prove offline first" gate (CLAUDE.md mandate #6) for closing the
routing loop: before flipping `PROXIMADB_ROUTE_COST_OVERRIDE` from advisory to
acting, show that the gated cost router's cumulative **regret** beats the static
heuristic on replayed/calibrated traces. This note records the result; the eval is
versioned with it (mandate #13) at `tests/route_cost_offline_eval.rs`.

**Method.** The eval drives the **real** `RouteCostModel`
(`src/query/route_cost_model.rs`) — `observe` / `recommend_override` /
`exploration_choice`, the exact code a live override runs — over a cost surface
**calibrated to measured cost ratios**, with cost carried through the model's own
`score()` via `range_gets` (the dominant per-request term, `per_get = 20`). It is a
controller-convergence proof, *not* an end-to-end latency claim. Deterministic (the
model has no internal RNG; jitter + workload order are functions of the round
index) → a stable CI ratchet. It builds an isolated model and ships **no**
production routing change.

Two OLAP/parquet shape-classes share the static rule (→ DataFusion) but differ in
the right answer:

| shape-class | static picks | truth | note |
|---|---|---|---|
| `olap/parquet` | DataFusion | DataFusion (100) < Native (160) | static **correct** |
| `olap/parquet/card=l/part=m` | DataFusion | Native (120) < DataFusion (220) | static **wrong** — DataFusion's per-partition GET fan-out dominates |

**Result (4,000 rounds, regret = cumulative cost over the per-shape oracle):**

| shape-class | controller regret | static regret |
|---|---|---|
| `olap/parquet` (static correct) | 180 | 0 |
| `olap/parquet/card=l/part=m` (static wrong) | **1,500** | **200,000** |
| **TOTAL** | **1,680** | **200,000** |

- On the shape-class where the static heuristic is wrong, the gated controller
  learns to flip to Native and **eliminates 99.25%** of the regret (1,500 vs
  200,000).
- Where static is already optimal, the controller adds only **180** units of
  bounded exploration regret (warming the worse arm a few times), then yields to
  exploitation — no flapping.
- **Learning is sublinear:** first-quarter controller regret 1,680 →
  last-quarter **0**. All regret is paid during warm-up; once arms are warm the
  controller tracks the oracle.

**Conclusion.** The mechanism is sound: the gated cost router converges to the
cheaper arm with sublinear regret and does no meaningful harm where the static rule
is already right. This is the evidence required to consider flipping
`PROXIMADB_ROUTE_COST_OVERRIDE` — a **separate** change, out of scope here.

**Caveats / follow-ups.**
- Calibrated surface, not captured production traces — the inter-arm cost *ratios*
  are plausible but synthetic. Replacing the surface with a corpus of real
  per-backend `IoTraceSnapshot`s (the observer already serializes them) is the next
  fidelity step before a production flip.
- The controller is the existing EWMA-greedy-with-forced-exploration policy. A
  Thompson-sampling upgrade (reusing `BetaDistribution` from
  `src/query/rl_planner/bandit.rs`, today wired for ANN path-selection) is a
  candidate if the EWMA policy leaves regret on the table on richer surfaces —
  re-prove here before adopting.
