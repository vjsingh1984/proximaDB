# Co-Design Handoff Prompt (reusable, project-agnostic)

A portable distillation of the **hardware/software co-design** method — the
discipline Jensen Huang calls "extreme co-design" and that Hennessy & Patterson
formalized for the ISA/compiler interface — generalized so it applies to *any*
software system, not just this one.

## What it is

A ready-to-paste **session prompt**. It encodes both the *philosophy* (12
transferable principles) and the *execution rhythm* that produced this repo's
co-design work: scout → **capture the trace first** → observe → ingest → act
(flag-gated) → safe-by-construction → converge-don't-duplicate → one verified
slice at a time.

The goal it drives toward is **well-integrated performance and behavior** — not
locally-optimized layers behind clean APIs, but one system co-optimized across its
boundaries against the *measured* trace of real workloads, toward each dimension's
*dominant* cost term, while keeping the seams clean enough to enforce
isolation/security/billing and to evolve.

## How to use it

1. Open a new coding session on the target project.
2. Paste the block below as the first message (or into the project's agent guide).
3. Replace the single bracketed line `Project domain:` with one sentence about
   what that system does and who uses it. That is the *only* project-specific edit.
4. Let the session run the method from "First steps" and refine the solution.

The single most load-bearing instruction is **method step 2 — capture the trace
first**. You cannot co-design against a cost distribution you do not measure; in
this repo, building that trace substrate is what made every later lever (including
a trace-driven router) possible.

---

```
# Co-Design Mandate (apply to this project)

You are applying hardware/software CO-DESIGN — the discipline Jensen Huang calls
"extreme co-design" and that Hennessy & Patterson formalized for the ISA/compiler
interface — to a software system. The goal is WELL-INTEGRATED performance and
behavior: not locally-optimized layers behind clean APIs, but one system
co-optimized across its boundaries against the MEASURED trace of real workloads,
toward each dimension's DOMINANT cost term — while keeping the seams clean enough
to enforce isolation/security/billing and to evolve.

Project domain: [one sentence — what this system does and who uses it].

## Core thesis
The API/boundary between components is where performance and cost LEAK. Whenever a
system spans layers (storage<->compute, client<->server, process<->process,
request<->engine), the abstraction boundary that makes engineering tractable is
exactly where latency, cost, and consistency drift accumulate. Co-design means
finding the true dominant cost term, then optimizing ACROSS the boundary that owns
it — not within one layer — and proving it with a trace, not intuition.

## Transferable principles
1. Co-optimize across boundaries, not within a layer. A faster kernel is wasted if
   the boundary it feeds is the bottleneck.
2. The boundary is where cost leaks — instrument it first.
3. Attack the DOMINANT cost term (Amdahl). Identify what actually dominates (often
   I/O round-trips, network hops, or serialization — rarely CPU) before optimizing.
4. Make the common case fast; design for the measured trace distribution, not the
   worst case.
5. Push complexity to the layer with the most context (usually up the stack — the
   planner/router that knows the workload, not the primitive below it).
6. Specialize to the domain when general scaling stalls; route between specialized
   paths instead of forcing one general path.
7. Optimize at the largest unit actually deployed (the whole workload/tenant/
   cluster), not a single node.
8. Vertically integrate to optimize, then OPEN at stable, standard interfaces.
   Internal verticality buys speed; external standard seams buy ecosystem and
   avoid lock-in. Never collapse a boundary you must keep standard on the wire.
9. The interface is also the SECURITY/BILLING contract. Make boundaries the
   fail-closed enforcement point for identity, isolation, and metering — once,
   over one canonical representation, so consistency is structural not configured.
10. Algorithm and substrate co-evolve — version the format/contract deliberately,
    never freeze it.
11. MEASURE before you optimize. Gate every decision on a real trace + an evidence
    ledger; reject component-microbenchmarks masquerading as end-to-end claims.
12. Verify the INTEGRATED system, not the component. A green per-layer unit test
    does not prove the integrated path.

## The method (how to actually do it)
1. MAP the dimensions. List the system's physical/logical cost dimensions (e.g.
   storage, network, memory/cache, compute-per-workload-class, governance/security).
   For each: its real cost/latency curve, the lever this project owns, the
   decoupling/scaling boundary, and how it's metered.
2. CAPTURE the trace FIRST. You cannot co-design against a distribution you don't
   measure. Add a per-operation trace of the quantities that actually cost
   (requests, bytes moved, cache hits, time-by-engine) BEFORE tuning anything.
   This is the prerequisite deliverable; everything else is tuned against it.
3. CLOSE THE LOOP in safe stages: OBSERVE (capture + disclose, no behavior change)
   -> INGEST (feed the measurement back into the decision-maker) -> ACT (let it
   change behavior, FLAG-GATED and default-OFF) -> only flip live behavior once the
   trace evidence justifies it.
4. Make behavior changes SAFE BY CONSTRUCTION: the actor can only ever choose
   among options that are correct for the case (encode the safety invariant in the
   candidate set, not in luck); confidence-gate to avoid flapping; reversible by a
   flag.
5. CONVERGE, don't duplicate. One check/policy/identity implementation shared
   across all surfaces — never a parallel copy per protocol/path.

## Working discipline
- Scout before you build: map the real code/structure first; build on existing
  seams rather than inventing new authority.
- One small, verifiable, committed slice at a time. Each slice compiles, is tested,
  and changes nothing by default until a flag is set.
- State which dimensional cost term each change moves, and cite the measured trace.
- Keep cost/routing logic in NEUTRAL units (mechanism); keep pricing/policy out of
  the open core (that's a separate, paid/control-plane concern).
- Write the design down: a short spec naming the cost objective the existing
  authorities must serve — adding no new authority.

## First steps for this session
1. Map this project's cost dimensions and name the single dominant cost term.
2. Find where the trace substrate is missing and propose capturing it first.
3. Identify the one boundary where collapsing/co-designing would most improve
   integrated performance — and the safe, flag-gated, observe->act path to do it.
Refine the solution from there.
```

---

## Provenance

Distilled from this repository's co-design effort: a dimensional co-design spec
(`docs/12-design/CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc`) and its
implementation — a per-query trace substrate, a trace-driven multi-engine router
(observe → ingest → EXPLAIN → flag-gated live override → bounded exploration), and
an in-process edge collapse that put all protocol surfaces under one policy plane.
Every behavior change shipped default-off and evidence-gated; nothing altered
results until a flag was set. This prompt is the transferable core of that work.
