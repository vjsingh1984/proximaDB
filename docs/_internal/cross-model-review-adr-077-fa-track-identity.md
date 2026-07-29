
---

## PART 3 — Verified Results (2026-07-28)

Two lenses were run through Claude agents (the first model in the multi-LLM pass); both were **verified against source by the session lead before relay**, not forwarded raw. Lens A and D are left for external models.

### Lens B (security lowering) — one real fail-open, found and fixed

| # | Finding (verified) | Disposition |
|---|---|---|
| 1 | "Strict walker is dead code in production" | **Severity overstated.** RLS enforcement is dead-wired (`apply_rls_filter` has zero callers), so nothing is *live*. The strict walker being inert is by design (FA-c wiring is lane-blocked). Real risk is documentation implying FA-a2 is shipped defense — wording tightened. |
| 2 | `Not(Equals)` admits a cross-class row that `NotEquals` denies; generalizes to `Not(any comparison)` | **Real, fixed** in #1277 via a 3-valued substrate (`Not(None)=None→deny`). |
| 3 | The deny-biased property test's grammar had no cross-class rows | **Real** — the test that should have caught #2 couldn't. Widened in #1277. |
| 4 | Over-deny on `Not(And([Eq,Eq]))` (availability) | **Real, NOT fixed.** A De-Morgan lowering was tried and **reverted**: the property test caught it introduced a fail-open on `Not(Not(Eq))`. Over-deny is the safe direction; documented as a known limitation. |
| 5 | `json_eq` epsilon vs `compare_json_values` exact float | **Real, low severity, noted**, not changed. |

### Lens C (key encoders) — the scary headline is a negative result

**No encoder-disagreement missed-fence exists.** Every disagreement the lens could construct is caught by `write_mutations`' oid-keyed INSERT-conflict backstop, or fails closed. The two-encoder situation is ugly but **not exploitable for silent PK duplicates** — recorded so it isn't re-investigated.

The real finding redirected to the services lane:

| # | Finding (verified) | Disposition |
|---|---|---|
| 2 | `execute_upsert` bypasses `check_unique_conflict`, the CKS PK fence, and FK enforcement → silent UNIQUE/FK violation | **Real, High.** Filed as **TD-DML-1** for the F5/write-path owner. Not in the catalog/FA lane. |
| 1 | CKS fence encodes only the first column of a composite PK (over-fence, spurious reject) | **Real, Medium.** This is the known single-column-PK limitation already documented in ADR-077; services lane. |
| 3–5 | UPDATE bypasses CKS; Decimal/structured PK fail closed; DELETE rejects some PK types | Lower severity, services lane. |

### Lessons from the pass

- **Verify before relay.** The agent overstated #B1's severity ("live in production" — it's dead-wired) and proposed a fix for #B4 (`re-add the guard`) that doesn't work, plus the De-Morgan fix that introduced a worse bug. Correct diagnosis, wrong severity and wrong fixes — caught only by checking against source.
- **The property test earned its keep twice**: it was too narrow to catch #B2 (the finding), then it caught the unsound De-Morgan "fix" for #B4. A narrow grammar gives false assurance; the grammar is now widened.
- **Cross-model value was concrete**: Lens B found a fail-open in code the session lead wrote and had "verified," and Lens C turned a scary-looking encoder situation into a clean negative result plus a real bug in a different lane.
