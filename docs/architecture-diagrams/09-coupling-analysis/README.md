# Coupling Analysis — Hub Types, Hotspots, Duplication Targets

Evidence (file sizes via `wc -l` + `.victor/init.md` architecture stats):

| File | LOC | Notes |
|---|---|---|
| `clients/rust/src/genrest.rs` | 19,244 | AUTO-GENERATED — duplication surface |
| proto `v1` (17,299) + `v2` (8,469) | 25,768 | PARALLEL proto definitions |
| `src/services/dml/mod.rs` | 11,256 | GOD MODULE, 226 symbols |
| `src/cluster/partition_lease.rs` | 4,578 | 255 symbols |
| `src/embedded/python.rs` | 4,841 | 203 symbols |
| `src/storage/traits/mod.rs` | 1,815 | `UnifiedStorageFormat` ~30 fns |
Graph evidence (`.victor/init.md`): `1,049,585` nodes / `3,225,692` edges / `6,573` IMPLEMENTS / `163` INHERITS.
Hubs: `DistanceMetric` (61 conn), `Duration` (98 conn), `now()` (1,466 refs).
```mermaid
flowchart LR
    NOW["now()<br/>timestamp util<br/>1,466 refs<br/>(MOST USED)"]:::hub
    DUR["Duration<br/>98 connections"]:::hub
    DM["DistanceMetric<br/>61 connections<br/>(crates/foundation/.../distance-types/lib.rs:84)"]:::hub
    REC["ProximaRecord<br/>(canonical record type)"]:::hub
    TC["TenantContext<br/>(proximadb-tenant)<br/>flows to EVERY I/O boundary"]:::hub
    NOW -.->|"used by"| DML["services/dml"]
    NOW -.->|"used by"| WAL["WAL persistence"]
    NOW -.->|"used by"| STG["storage engines"]
    DUR -.->|"lifespan/lease/TTL"| DM
    DM -.->|"every vector op"| REC
    TC -.->|"billing + isolation"| REC
    classDef hub fill:
```
**Implication:** `DistanceMetric`, `Duration`, `now()`, and `TenantContext` are the highest-ceiling /
highest-coupling types. Any schema change to these cascades broadly — treat as stable seams.
```mermaid
flowchart TD
    subgraph GEN["AUTO-GENERATED (do NOT hand-edit)"]
        GR["genrest.rs<br/>clients/rust/src/<br/>19,244 lines<br/>1,749 symbols"]:::gen
        PV1["proto v1<br/>17,299 lines"]:::gen
        PV2["proto v2<br/>8,469 lines"]:::gen
    end
    subgraph GOD["GOD MODULES (decomposition candidates)"]
        DML["dml/mod.rs<br/>11,256 lines<br/>226 symbols<br/>(INSERT/UPDATE/DELETE + predicates + scans)"]:::god
        PLE["partition_lease.rs<br/>4,578 lines<br/>255 symbols<br/>(lease + fence + election)"]:::god
        PY["embedded/python.rs<br/>4,841 lines<br/>203 symbols<br/>(PyO3 surface)"]:::god
        TR["storage/traits/mod.rs<br/>1,815 lines<br/>(UnifiedStorageFormat ~30 fns)"]:::god
    end
    classDef gen fill:
    classDef god fill:
```
```mermaid
flowchart LR
    subgraph PARALLEL_PROTO["Parallel proto definitions"]
        V1["proximadb.v1.rs<br/>559 symbols"]
        V2["proximadb.v2.rs<br/>230 symbols"]
        V1 -.overlap.-> V2
    end
    subgraph MULTI_SDK["Spec-driven SDKs (REST regen per language)"]
        RS["Rust genrest"]
        GO["Go SDK"]
        TS["TS SDK"]
        JV["JVM SDK"]
        PY["Python _generated/rest"]
        RS -.drift risk.-> GO
        GO -.drift risk.-> TS
    end
    subgraph STUB_BOILER["Trait stub boilerplate across engines"]
        SST["SSTEngine"]
        VIP["ViperEngine"]
        NOV["NovaEngine"]
        SWI["SwiftEngine"]
        HEL["HelixEngine"]
        SST -.same trait, ~30 fns.-> VIP
        VIP -.-> NOV
    end
    subgraph CATALOG_BACKEND["Polymorphic catalog backends (likely shared impl)"]
        N["native.rs"]
        I["iceberg.rs"]
        D["delta.rs"]
        O["oltp.rs"]
        N -.shared glue.-> I
    end
    OPENAPI["OpenAPI spec<br/>docs/openapi/proximadb-openapi.yaml"]:::src
    OPENAPI ==>|generates| RS
    OPENAPI ==>|generates| GO
    OPENAPI ==>|generates| TS
    OPENAPI ==>|generates| JV
    OPENAPI ==>|generates| PY
    classDef src fill:
```
| Target | Observation | Action |
|---|---|---|
| `proto v1/v2` overlap | Parallel definitions, drift risk | Continue backward-compatible migration toward v2; document v1 shim surface |
| SDK REST transports | Drift-prone hand-rolled risk | Already spec-driven (CLAUDE
| `UnifiedStorageFormat` stubs (~30 fns × 8 engines) | Default-impl boilerplate | Keep trait default methods (lines 796/844…) absorbing shared logic; do not re-implement per engine |
| `dml/mod.rs` (11,256 lines) | God module | Extract `predicates`, `scans`, `changes_since` into submodules (already partly split via `table_write_executor`, `record_store`) |
| Catalog backends (native/iceberg/delta/oltp) | Shared glue across backends | Promote shared logic into `glue.rs` / `schema.rs` (already started) |
| Hub types (DistanceMetric / Duration / now / TenantContext) | High fan-in | Pin as stable seam — change = wide blast radius; gate with SOLID port traits (CLAUDE
- `1,049,585` nodes, `3,225,692` edges
- `6,573` IMPLEMENTS relationships → heavy abstraction/contract surface
- `163` INHERITS relationships → modest inheritance spine (mostly SDK plugin arch: 9 embedding providers, 8 chunking strategies)
- Branching ratio `3.07` → moderately branching control flow
