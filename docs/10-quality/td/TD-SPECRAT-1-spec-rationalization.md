// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
= TD-SPECRAT-1: spec rationalization — expose live-but-unspec'd REST surfaces in controlled per-area waves

[cols="1,3", options="header"]
|===
| Field | Value
| Status | Wave 1 landing
| Severity | Medium — ~100 route literals across ~15 areas are live on the wire but invisible to every SDK
| Component | `src/network/rest/openapi_supplement.yaml`; `docs/openapi/proximadb-openapi.yaml`; `clients/*` (regenerated)
| Relates to | [[TD-SSO-3]] (the census that surfaced the gap); TD-126 (spec-from-code); ADR-041
|===

== The gap

`make verify-openapi-spec` only diffs *annotated* handlers against the committed
spec. A mounted-but-unannotated route is therefore silently born unexposed: it
works over raw REST but no generated SDK (Python/Go/TS/Rust — all spec-driven,
TD-126) can see it. A 2026-09-03 census counted ~100 such route literals across
~15 areas (ABAC, catalog, collections admin, observability, events,
model-registries, external-collections, timeseries, graph analytics, streaming,
prepared-SQL, Iceberg-REST, legacy v1, primary-pod, progressive/rank search).

Notably `/api/v2/nl/translate` was LIVE (mounted, real LLM calls) while absent
from the spec — found by the TD-SSO-3 adversarial fact-check.

== The program (maintainer direction, 2026-09-03)

Expose the unexposed in **controlled, reasoned, rationalized iterations — one
area at a time** — then rebuild the SDKs from the specs. Per area:

1. **Rationalize** each endpoint: does it stay as-is, and is it REAL?
   Fabricated-success stubs are NOT exposed (below).
2. **Expose** via the supplement yaml (the established staging pattern for
   handlers that live outside the root crate's annotated set) or utoipa
   annotations when the handler is in the root crate.
3. **Regenerate** the spec + all four SDKs in the same PR (drift gates enforce).
4. **Adversarial review** per PR; LLM/ranked surfaces additionally need eval
   suites (repo directive 13).

Later waves migrate supplement entries → in-code annotations mechanically
(requires utoipa in `proximadb-api` or moving DTOs; deliberately deferred).

== Wave 1: observability + nl/translate

**Exposed (6 paths; spec 39 → 45):**

* `POST /api/v2/observability/namespaces` — createObservabilityNamespace
* `POST /api/v2/observability/namespaces/{ns}/logs/bulk` — ingestLogs
* `POST /api/v2/observability/namespaces/{ns}/metrics` — ingestMetric
* `POST /api/v2/observability/namespaces/{ns}/metrics/bulk` — ingestMetrics
* `POST /api/v2/observability/namespaces/{ns}/metrics/aggregate` — aggregateMetrics
* `POST /api/v2/nl/translate` — translateNaturalLanguage (AV-SQL 3-agent flow;
  real LLM calls; output is model-generated — the schema says so)

All carry typed request/response schemas (not loose `additionalProperties`),
default-value documentation, and 400/401 (500 for the LLM path) error refs.

**Deliberately NOT exposed — stubs (rationalization rule: never expose
fabricated success):**

* `POST …/metrics/promql` — returns an empty vector; "Full PromQL wiring comes
  with the CHRONO engine" (documented stub).
* `POST …/traces/bulk`, `POST …/traces/search` — return success/empty WITHOUT
  touching the observability port (silent-Ok anti-pattern). Exposure deferred
  until the trace path is actually wired.

**Already exposed via supplement (unchanged):** logs + logs/search.

== Follow-ups filed by this wave

* Eval suite for `/api/v2/nl/translate` (directive 13 — model-generated
  surface; prompts + rubric versioned when behavior changes).
* Trace ingest/query: wire the port or return 501 — then expose.
* Waves 2+: area order per product priority (observability diagnostics,
  ABAC, collections admin, ...).
