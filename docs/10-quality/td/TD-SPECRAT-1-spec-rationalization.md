// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
= TD-SPECRAT-1: spec rationalization — expose live-but-unspec'd REST surfaces in controlled per-area waves

[cols="1,3", options="header"]
|===
| Field | Value
| Status | Waves 1 (observability+nl) & 2 (stub-rule fixes, TD-SPECRAT-2) landed; wave 3 (ABAC) landing
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

== Wave 3: ABAC control-plane (this PR)

**Exposed (9 paths / 14 operations; spec 45 → 54):** the complete
`/api/v2/abac/*` operator surface from
`src/network/rest/canonical/abac_admin.rs` — policy-bindings
(PUT/DELETE/GET), attribute-bindings (POST/GET), predicate-objects
(PUT/GET/DELETE/list), grants (POST/list/DELETE, ADR-090 entitlement
layer), and tenant-posture (GET/PUT, TD-SEC-2).

Rationalization notes specific to this surface:

* **Real, not stubs:** every handler writes through the same durable
  `FileSystem*` stores the live enforcer reads (hot-reload, no restart);
  the handlers carry their own operator-gate + fail-closed test suite.
* **Deployment gating documented, not hidden:** these routes are
  registered only under `--features abac-policy` (default OFF); the spec
  notes that a default build 404s the path rather than returning the
  documented error shapes.
* **Error envelope carried verbatim:** the surface's flat
  `{error, message, code}` `OperatorErrorResponse` is documented as its own
  schema (`AbacOperatorErrorResponse`) rather than being normalized into
  the canonical nested `ErrorResponse` — what the handler emits is what
  the spec says.
* **`FilterExpression` modeled as free-form JSON** (`AbacFilterExpression`,
  additionalProperties): the recursive externally-tagged union cannot be
  expressed as a self-referential `oneOf` that openapi-python-client
  processes (it drops the schema and every referencing endpoint); the
  description documents the exact construction shape. This is the one
  untyped model on the surface — recorded as a deliberate trade.

**Tooling defect found and fixed (root cause, both generators):**
`clients/{go,rust}/codegen/openapi_31_to_30.py` down-converts the
serde_yaml-emitted (YAML 1.2) spec with PyYAML, which resolves scalars per
YAML 1.1 — bare `Off` (and `On`/`Yes`/`No`/`y`/`n`) parsed as *booleans*.
Effect: progenitor hard-failed ("type error unexpected value type") while
oapi-codegen **silently emitted `False AbacGrantEnforcement = "false"`**
into the committed Go SDK shape. Fix: a `SafeLoader` subclass restricted
to the YAML 1.2 core-schema boolean set (`true|false` + case variants), so
`Off` stays the string `"Off"` while real booleans parse unchanged. A
full-spec scan confirmed only the two new ABAC enums (`Off`,
`Null` — `Null` is quoted by serde_yaml itself) were ever exposed to the
hazard, so no previously-committed generated code changes.

**Adversarial-review ratchets:** the contract gate now asserts the exact
9-path / 14-operation ABAC surface, verifies YAML 1.2 boolean handling, and
requires `grant_enforcement` in `AbacTenantSecurityPosture`. The latter caught
and corrected a response-schema mismatch: the Rust handler's non-optional
field is always serialized, while the initial supplement allowed generated
SDKs to treat it as absent.

**Waves 4+ (candidate order, product-priority pending):** collections
admin (affinity/pinning/primary-pod/branch-merge), catalog routing/explain,
graph analytics (fusion-search, impact-analysis), timeseries, streaming
ingest, prepared-SQL, external-collections, model-registries, events,
progressive/rank search. Legacy `/api/v1` is excluded (retiring per
SUPPORTED_SURFACE); Iceberg-REST is excluded (an external standard
protocol consumed directly by Spark/Trino — not our OpenAPI→SDK loop).
