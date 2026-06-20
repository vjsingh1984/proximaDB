# ProximaDB Agent Guide (CLAUDE.md)

This document serves as the foundational mandate for Anthropic AI agents working on ProximaDB. It mirrors the constraints and directions defined in `GEMINI.md`.

## 🚀 Project Overview
ProximaDB is a high-performance, cloud-native vector and graph database built in Rust. It combines semantic vector search with native graph traversal for RAG and knowledge graph applications.

[WARNING]
====
**🚨 CRITICAL ARCHITECTURAL PIVOT (2026-06-04) 🚨**
ProximaDB has shifted from a monolithic custom WAL/PAX architecture to an **Intelligent Multi-Engine Routing** system running over decoupled **Object Storage**.

When modifying architecture or execution paths, you MUST adhere to the dual-path mandate:
1. **Data Warehouse/Relational Workloads:** Driven by DataFusion/Polars executing over standard Parquet files managed by Iceberg manifests.
2. **Vector Search/ANN Workloads:** Driven by specialized high-performance engines (SST, HELIX, NOVA) utilizing the custom PAX block format.

You must also strictly enforce SaaS Operational constraints:
- **Path Isolation:** All Object Storage writes must be prefixed by `DrPathBuilder` (`data/{tenant_id}/{namespace_id}/...`). Do not use raw schema locations.
- **Financial Telemetry:** Plumb `TenantContext` to all I/O boundaries to emit accurate billing metrics.
====

[IMPORTANT]
====
**🧬 CO-DESIGN MANDATE (2026-06-19)**
ProximaDB is co-designed the way NVIDIA co-designs silicon+compilers and RISC co-designed the
ISA+compiler: the five physical dimensions — **object storage, network, local disk/cache,
compute-per-modality, and governance/security** — are co-optimized as one system against the
**measured trace distribution**, toward each dimension's **dominant cost term** (for a cloud DB that
is I/O round-trips and egress, *not* CPU). See `docs/12-design/CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc`.

When changing a storage format, reader, codec, cache, or engine you MUST:
1. **Co-design, don't locally optimize.** State which *dimensional cost term* the change moves
   (a 4× codec is irrelevant if you still pay N footer round-trips). Optimize across the
   storage↔compute boundary, not within one layer.
2. **Trace before you tune.** Justify with a *measured per-query trace* (bytes/requests/latency/
   cache-hit), never a component-kernel microbenchmark. New routes/readers/writers must emit the
   query-scoped I/O trace so the dimension they touch is observable; the `ComputeScheduler` costs
   routes from measured quantities, not static heuristics.
3. **Treat the boundary as the contract.** Every I/O boundary carries `TenantContext` (isolation +
   billing, fail-closed) and writes under `DrPathBuilder`. Isolation is *structural*, never a
   per-query predicate.
4. **Vertical inside, standard outside.** Co-design internals (PAX + engine + cache) freely; expose
   only at stable seams (pgwire, Arrow Flight, Iceberg manifests, REST v2).
5. **Meter every dimension as a TAM surface.** Storage (KSU), read/compute (KRU/KIU), egress (KEU —
   the open gap to close), and cache are metered per-tenant; governance is metered as tier entitlement.
====

### Core Directives
1.  **Workspace Isolation (MANDATORY — this repo is worked on by multiple concurrent sessions):** NEVER edit, commit, `checkout`, `reset`, `stash`, or `branch -f` in the main checkout, and NEVER touch another worktree's branch or uncommitted WIP. Every task gets its own git worktree + branch off `origin/develop`: run `eval "$(scripts/worktree.sh new <type/topic>)"` to create one and `cd` into it. Run `scripts/worktree.sh guard` before editing — if it fails, you are in the shared checkout; stop and make a worktree. Clean up with `scripts/worktree.sh rm <type/topic>` once your PR merges. Full rationale + commands: `docs/10-quality/WORKSPACE_ISOLATION.md`.
2.  **Architecture Source Of Truth:** Use `docs/12-design/README.adoc` as the architecture index. Pay special attention to the `DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc` (strategic pivot) and `CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc` (co-design cost spine) documents.
3.  **Safety First:** NO `.unwrap()`, `.expect()`, or `panic!()` in production code. Use `Result` and `?`.
4.  **Token Efficiency:** When running long-output commands, use `grep` with context flags.
5.  **Measure, Don't Assert:** Gate performance claims on the evidence ledger (`BENCHMARK_EVIDENCE.toml`); reject "indicative" kernel benchmarks masquerading as end-to-end metrics.

*See `GEMINI.md` for the comprehensive list of engineering mandates.*