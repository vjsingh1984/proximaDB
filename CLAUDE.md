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
5. **Meter every dimension as a TAM surface.** Storage (KSU), read/compute (KRU/KIU), network
   **outgress (KOU — metered/shipped: read + result direction at pgwire/REST/Flight; distinct from
   KEU = embedding)**, and cache are metered per-tenant; governance is metered as tier entitlement.
====

### Core Directives
1.  **Workspace Isolation (MANDATORY — this repo is worked on by multiple concurrent sessions):** NEVER edit, commit, `checkout`, `reset`, `stash`, or `branch -f` in the main checkout, and NEVER touch another worktree's branch or uncommitted WIP. Every task gets its own git worktree + branch off `origin/develop`: run `eval "$(scripts/worktree.sh new <type/topic>)"` to create one and `cd` into it. Run `scripts/worktree.sh guard` before editing — if it fails, you are in the shared checkout; stop and make a worktree. Clean up with `scripts/worktree.sh rm <type/topic>` once your PR merges. **Reclaim disk routinely:** worktree `target/` build caches are tens of GB each and are the dominant disk consumer, so run `scripts/worktree.sh clean` (preview with `--dry-run`) at the start/end of a session to drop every worktree whose branch has landed in `develop` — it detects **both** merge-commit and **squash-merged** PRs and reports the space reclaimed. Full rationale + commands: `docs/10-quality/WORKSPACE_ISOLATION.md`.
2.  **Architecture Source Of Truth:** Use `docs/12-design/README.adoc` as the architecture index. Pay special attention to the `DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc` (strategic pivot) and `CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc` (co-design cost spine) documents.
3.  **Performance by design (think like a lead engineer / PM):** When designing or reviewing, weigh the perf levers this system already provides rather than naive swaps. **Transport:** route bulk/columnar work to **Arrow Flight** (zero-copy), typed RPC to gRPC, and REST for ergonomics — they coexist on the multiplexed port, so do NOT propose "gRPC instead of REST" or a new transport (Avro-RPC, etc.). **Storage/IO:** prefer **vector quantization** (SQ8/RaBitQ + f32 rerank) over byte-compressing float vectors; zstd for cold/warehouse tiers; ranged reads + zone-map pruning over whole-object scans; cache repeated reads (footer/tenant cache). Apply a lever when it's simple and in scope; otherwise file it as a TD with the rationale.
4.  **Safety First:** NO `.unwrap()`, `.expect()`, or `panic!()` in production code. Use `Result` and `?`.
5.  **Token Efficiency:** When running long-output commands, use `grep` with context flags.
6.  **Measure, Don't Assert:** Gate performance claims on the evidence ledger (`BENCHMARK_EVIDENCE.toml`); reject "indicative" kernel benchmarks masquerading as end-to-end metrics.
7.  **PR Sizing for CI Efficiency (batch related work):** GitHub Actions runners are a shared, finite resource and every PR/push triggers a full CI run — so do NOT slice cohesive work into many small PRs. Consolidate related commits (a feature + its tests + docs + adjacent follow-ups) into a single **medium-to-large, "meaty" but coherent** PR: large enough to amortize one CI run across all the work, small enough to stay reviewable and single-purpose. Avoid both extremes — trivial one-commit PRs that each cost a full runner cycle, and sprawling unfocused mega-PRs that are hard to review or bisect. When you have stacked or independent branches headed to the same base, rebase them onto one branch and open **one** PR rather than N. Always verify locally (`cargo clippy`/tests) before pushing so a runner cycle isn't spent catching what you could have caught locally.
8.  **Storage-Format Migration (mixed-read-safe, gated):** Any change to an on-disk/wire format, codec, block, segment, or manifest MUST be **mixed-read-safe** — old and new coexist, detected by a version/magic byte, never a flag-day. Ship **default-OFF** (per-collection opt-in or env gate) until baked. Readers/recovery default to the legacy format when the marker is absent — never assume the new format. Gate with **round-trip + recall/quality tests**, not just compile: quantized/lossy formats must hold recall within tolerance of the f32 baseline. Reuse the existing inverse (e.g. `PaxSegmentScanner::read_records`); do not hand-roll a decoder. Canonical phased pattern: `docs/12-design/RABITQ_PAX_SEGMENT_MIGRATION_PLAN_2026_06.adoc`.
9.  **CI/CD Tiering & Security Governance:** CI runs in tiers matched to the `feat → develop → qa → main` promotion flow: **feat→develop LIGHT** (compile + fmt/clippy + layering + proto + panic-policy + security-audit + docs), **develop→qa MEDIUM** (+ unit/integration tests + feature-matrix), **qa→main FULL** (+ coverage + python multi-version + docker + benchmarks + CodeQL). Protected branches gate on the `CI Success` aggregator; **`develop` requires it too** — don't expect to merge red. Heavy scans (CodeQL) run only at the qa→main boundary + weekly. **Never commit secrets/credentials** (even in tests/fixtures — secret-scanning + push-protection are on; CodeQL flags credential literals). Report vulnerabilities via `SECURITY.md` private reporting, not public issues.
10. **Quality Ratchets (correctness beyond compile):** Conformance is ratcheted, not asserted — the TPC-H (22) / TPC-DS (16) pgwire suites and ANN recall@k harnesses carry counts that **only go up**; a change that regresses a ratchet is a failure. ANN/quantization changes are gated against the **f32 recall baseline** (within tolerance). Performance claims are gated on the evidence ledger (`BENCHMARK_EVIDENCE.toml`) — reject kernel microbenchmarks masquerading as end-to-end metrics.
11. **Determinism & Test Hygiene:** Tests must be deterministic and isolated — use **`nextest`** for process isolation; **no non-daemon background threads** (an un-shutdown pool deadlocks the interpreter/runner at exit); pin temp state under `tempdir`, not shared paths. Verify the **server binary builds**, not just `--lib` (a green `--lib` skips `#[cfg(test)]` + feature-gated code). Run perf-sensitive checks on a quiet machine. Known pre-existing flake: `native_volcano_stream_truncates_without_error` (streaming hang) — re-run, don't chase.
12. **Agentic Engineering (Model + Harness):** This repo is developed in the *agentic engineering* mode of Google's *The New SDLC With Vibe Coding* (Osmani, Saboo, Kartakis, 2026): "an agent is a model plus a harness" — and the harness, not the model, is the work (~"10% model, 90% harness"). ProximaDB's harness IS this repo: the rule files (CLAUDE/GEMINI/AGENTS.md = **static context**), per-session **memory**, worktree **sandboxes**, the **guardrails/hooks** (panic-policy, layering, tenant-path, OSS-boundary, secret-scan, the CI tiers), multi-agent **orchestration**, and **observability** (Prometheus/billing). The differentiator of this mode is "not whether you use AI, it is how outputs get verified": production work runs the disciplined end — spec/ADR → tests **and evals** → CI gates — never vibe-coding (reserve that for throwaway spikes). Since "AI turns implementation from writing into reviewing", review generated code for its failure modes — **hallucinated dependencies/APIs, plausible-but-wrong logic, silent duplication of an existing primitive** — not just style. Keep these rule files **lean and high-signal** (static context is paid on every call); push detail to on-demand docs/skills (progressive disclosure).
13. **Evals for Non-Deterministic Surfaces:** Per the tests-vs-evals split — "tests cover the deterministic parts; evals cover the parts that aren't deterministic" — any ranked, generated, or model-driven surface (ANN recall@k, hybrid/RRF ranking, embedding/semantic relevance, RAG / Graph-RAG retrieval, Text-to-AQL / RUBICON plans) MUST ship an **eval suite with a real rubric**, covering both **output** (is the result correct) and **trajectory** (was the route / tool-calls / plan sound) — "set the bar at the eval, not the demo." Gate shared/agentic workflows on **eval thresholds** like test coverage (a regression fails CI; ties to the Quality-Ratchet mandate), and **version the prompts + eval suites in the PR** that changes the behavior. Watch production for drift.
14. **Commit/PR Hygiene (no agent attribution):** Do NOT add AI-agent authorship to commit messages or PR bodies — no `Co-Authored-By: <AI>` trailers, agent model/product signatures (e.g. "Claude Code"/"Claude Opus"), agent no-reply emails, or "Generated with …" footers. The human drives the code; `scripts/check_no_agent_attribution.py` enforces this and rejects the commit/PR otherwise. (Mentions of CLAUDE.md/GEMINI.md or the Anthropic API in *content* are fine — only authorship attribution is blocked.)
15. **SDK REST Transport is Spec-Driven (generated, never hand-rolled) — sync AND async:** Every SDK's REST surface, in every language, MUST be generated from the canonical OpenAPI spec (`docs/openapi/proximadb-openapi.yaml`, itself `utoipa`-emitted from the axum handlers) via the per-language generator (Python `openapi-python-client` → `_generated/rest`, emitting `sync`/`sync_detailed` **and** `asyncio`/`asyncio_detailed` per endpoint), wired behind a thin hand-written ergonomic facade (pooling/retries/locality-gzip/auth). Do NOT hand-roll REST request-building or response-parsing in SDK code (raw `httpx`/`requests` against hand-written routes/payloads) — it reintroduces the server↔SDK drift the spec exists to kill. The **async** client wires the generated `asyncio` functions exactly as the **sync** client wires the generated `sync` functions (see `clients/python/src/proximadb_sdk/protocols/_rest_codegen.py`). The merge-blocking `<lang>-sdk-codegen-drift` gate enforces conformance; a spec change MUST regenerate every SDK client in the same PR. (gRPC = generated proto stubs; Arrow Flight = Arrow schema; pgwire = standard PG driver.) Ref TD-126.
16. **Read/Write Correctness Invariants (read-paths-filter-dead-records + writes-invalidate-caches + Strong-bypasses-cache + use-`valid_to_ns`-not-`expires_at`):** Four non-negotiable invariants for read-after-write correctness in this multi-tenant DB, enforced by the canonical predicate + ratchet tests:
    - **a. Every read boundary filters dead records via the canonical predicate.** A record is dead when `valid_to_ns == Some(0)` (tombstone) OR `valid_to_ns > 0 && valid_to_ns < now_ns` (TTL-expired). Use `ProximaRecord::is_dead(now_ns)` / `is_record_dead(valid_to_ns, now_ns)` (records crate) — **never** the unit-muddled derived `expires_at` (ms on the proto wire, secs in the WAL path — display only). Apply it at every read surface (vector search, scan, get-by-id, SQL/pgwire, Arrow Flight, graph, document, count/stats). Defense-in-depth: the predicate is idempotent, so multiple layers applying it is correct and encouraged.
    - **b. Every write path invalidates read-serving caches post-commit.** INSERT, UPDATE, DELETE, and DDL must invalidate the query/plan caches for the (tenant, collection) after the WAL write succeeds (via `invalidate_query_cache` / `invalidate_collection_cache` → `CacheInvalidationCoordinator`). A write that does not invalidate is a cache-coherence bug (stale reads).
    - **c. `VectorFreshnessMode::Strong` reads bypass the query cache.** A Strong read always computes fresh (delta-merge); it must never return a result cached before a concurrent write. Gate the query-cache lookup on `!Strong`.
    - **d. Tombstones in the same WAL delta suppress the live copy.** When a record's live copy and its tombstone are both unflushed (insert-then-delete-before-flush), the merge must drop both — never let the live copy survive beside its own tombstone.

### Mandate: Full ANSI SQL over pgwire
ProximaDB is driving toward **full ANSI/standard SQL support over the PostgreSQL
wire protocol (pgwire)**. Treat **TPC-H and TPC-DS over pgwire as the conformance
driver** (first cuts live in the qa-gate integration suites). As you implement or
touch query paths:

- **Submit queries to ProximaDB and let it route** — the engine owns engine
  selection. The pgwire SELECT path lowers SQL and calls
  `ComputeScheduler::route_select` → **DataFusion (OLAP) for parquet-backed
  tables, Volcano/native otherwise** (`src/network/postgres/relational_pipeline.rs`,
  `src/query/compute_scheduler.rs`). Never bypass pgwire/the router in product
  code or end-to-end tests to call an engine directly.
- A table reaches the DataFusion OLAP engine when it is **parquet-backed** — via
  `ALTER TABLE … MATERIALIZE` (warehouse materializer, auto-wired from `data_dir`)
  or a federated/external Parquet storage layout.
- **Fix SQL wiring/lowering gaps incrementally as you find them** — relax
  vector-collection-era constraints that leak into relational DDL (e.g. the former
  8-char collection-name minimum blocked `part`/`orders`/`region`), extend the
  relational frontend/planner/executor and DataFusion lowering, and prefer real
  ANSI semantics over ad-hoc shortcuts. Each TPC-H/TPC-DS query that starts passing
  is the ratchet of progress.
- **Diagnostics:** the pgwire client surfaces failures as a generic `db error`;
  the real cause is in the server `DbError` (code/message) — inspect via
  `e.as_db_error()` and run with `RUST_LOG=proximadb=debug`. Keep server-side error
  messages specific and actionable.

*See `GEMINI.md` for the comprehensive list of engineering mandates.*