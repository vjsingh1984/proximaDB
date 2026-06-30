# v1/v2 API Untangle — Deprecate the Real v1, Collapse to One Clean v1

Date: 2026-06-22
Status: In progress (step 1 — deprecation markers)

## Goal

Collapse the current messy v1/v2 split into a single coherent API so that, **after
release, the outside world only ever sees `v1`**. The "v2" surface today is a
single canonical data plane (`proto/proximadb/v2/record.proto`); most "v1"
services are in fact the *current* API that merely live in a `v1/`-named
directory. We deprecate and remove the genuinely-superseded v1, then rename the
surviving surface to a clean `v1` before release.

## Two distinct buckets (do not conflate)

**Real v1 — deprecate now → remove:** compatibility shims superseded by the
canonical ProximaRecord API (`v2/record.proto`, `/api/v2/...`):

- gRPC `VectorService` (`proto/proximadb/v1/vector.proto`)
- gRPC `CollectionService` (`proto/proximadb/v1/collection.proto`)
- REST `/api/v1/{search,vectors/batch,collections}` compat endpoints

**Current API in a v1-named dir — DO NOT deprecate** (no v2 replacement exists;
will be renamed to clean `v1` at the pre-release collapse): `GraphService`,
`HybridSearchService`, `DocumentService`, `QueryService` (sql), observability,
ranking, relations, cluster, security, streaming, catalog. (Note: the graph REST
routes already serve `/api/v2/graphs/...` even though the server module is named
`rest/v1/graph.rs` — that's a naming artifact, resolved at the collapse, not a
deprecation.)

## Done

- ✅ `option deprecated = true` on `VectorService` + `CollectionService` (services
  and all RPCs). Generated stubs in **all client languages** inherit the marker
  on their next regen — the single-source-of-truth deprecation.
- ✅ REST `/api/v1/*` already emits `Deprecation: true` + migration message via
  `add_rest_v1_deprecation_headers` (`crates/.../rest/v1/mod.rs`).
- ✅ Python SDK graph fix: `query_nodes`/`query_edges` now unwrap the v2
  `{data:{items,has_more}}` envelope (was yielding dict keys → broke
  `get_outgoing_edges`/`get_incoming_edges`). Verified against a live embedded
  engine via Victor's code-graph parity battery.

## Follow-up (tracked)

- ☐ **Hand-wrapper deprecation markers** in each of the 9 client SDKs (`clients/`:
  go, go-embedded, java-embedded, jvm, nodejs-embedded, python, python-embedded,
  python-queue-embedded, rust). Add idiomatic markers **only** on the
  hand-written convenience methods that call the v1 shim
  (`/api/v1/*` or the v1 `VectorService`/`CollectionService` stubs) — **not** the
  high-level methods that already route to v2 records. Do per client as it is next
  touched/built so each can be compile-verified.
- ☐ **Remove** the deprecated v1 (`VectorService`/`CollectionService`,
  `/api/v1/*`) once consumers are off it.
- ☐ **Pre-release rename**: collapse the surviving surface to a single clean `v1`.
- ☐ (Optional) Stand up a spec→codegen pipeline so the 9 clients regenerate
  instead of drifting (the edge-list envelope bug above was drift a generated
  client would not have had).
