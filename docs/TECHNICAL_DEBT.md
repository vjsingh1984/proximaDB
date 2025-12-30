# ProximaDB Technical Debt Register

This document tracks known technical debt items that affect developer experience, correctness, or performance.

## Vision Phase Context

Technical debt items are prioritized based on their impact on the three development phases:
- **Phase 1** (Format & Catalog): Storage engines, WAL, core APIs
- **Phase 2** (Cross-Model Query): Graph, documents, observability, unified query
- **Phase 3** (Ecosystem Integration): External catalogs, streaming, SDKs

## Current Priorities

| ID | Area | Phase | Priority | Status | Notes |
|---|---|---|---|---|---|
| TD-001 | PULSAR/QUASAR wiring | P3 | Medium | Open | Distributed graph engines implemented but not wired to service layer |
| TD-002 | External catalog integration | P3 | Medium | Open | Glue, Iceberg, Delta Lake connectors have stubs |
| TD-003 | Streaming infrastructure | P3 | Medium | Open | Spec complete, implementation pending |
| TD-004 | CDC connectors | P3 | Medium | Open | Outbound CDC ready, inbound connectors are stubs |
| TD-005 | Multi-language SDKs | P3 | Low | Open | Go SDK in progress, others planned |
| TD-006 | mTLS support | P3 | Low | Open | JWT/API Key auth complete, mTLS pending |

## Recently Resolved (Phase 2 Complete)

| ID | Area | Resolution | Phase |
|---|---|---|---|
| TD-R01 | Unified query wiring | FederatedQueryContext fully implemented | P2 |
| TD-R02 | Document engine persistence | WAL-backed with full read/write path | P2 |
| TD-R03 | Graph metadata persistence | ORION WAL persistence complete | P2 |
| TD-R04 | Observability log indexing | Full-text search via inverted index | P2 |
| TD-R05 | ORION update replay | WAL replay handles all operations | P2 |
| TD-R06 | pgwire completeness | Prepared statements, DDL/DML complete | P2 |
| TD-R07 | Cross-model joins | LATERAL joins across all models | P2 |
| TD-R08 | Extra metadata filtering | JSON/binary filtering in columnar strategy | P1 |
| TD-R09 | SST dual block types | Legacy types deprecated | P1 |
| TD-R10 | WAL wiring (REST/gRPC) | Document + observability wired | P1 |
| TD-R11 | Metric aggregation | Aggregation engine wired | P2 |
| TD-R12 | DataFusion integration | TableProvider, ScanExec, VectorRecordBridge | P2 |
| TD-R13 | SIMD decoders | AVX2, NEON, scalar backends complete | P1 |
| TD-R14 | Smart I/O layer | ParallelReader, IoMetrics complete | P1 |
| TD-R15 | CentroidTree | O(log n) vector pruning complete | P1 |

## Phase Status Summary

| Phase | Description | Debt Status |
|---|---|---|
| Phase 1 | Format & Catalog | ✅ No outstanding debt |
| Phase 2 | Cross-Model Query | ✅ No outstanding debt |
| Phase 3 | Ecosystem Integration | 🔄 6 items in progress |

## Notes

* If you fix a debt item, add a short entry under "Recently Resolved" and remove it from "Current Priorities".
* Keep this list short and execution‑focused.
* All Phase 1 and Phase 2 technical debt has been resolved.
* Phase 3 items are tracked but not blocking MVP release.

---

_Last Updated_: December 30, 2025
