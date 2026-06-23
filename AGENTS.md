# ProximaDB Agent Index (AGENTS.md)

This document acts as the index and unified trajectory guide for all Autonomous AI Agents operating in the ProximaDB workspace.

## Primary Instructions
Depending on your identity, read the corresponding mandate file first:
*   **Gemini Models:** Read `GEMINI.md`
*   **Anthropic/Claude Models:** Read `CLAUDE.md`

## 🚨 Active Development Trajectory (2026-06-04 Pivot) 🚨

ProximaDB is undergoing a massive architectural shift to support a robust SaaS MVP. All agents must align their problem-solving, code generation, and architectural suggestions with the following truths:

1.  **Intelligent Multi-Engine Routing:** We do not have a single physical execution layer. We route queries based on workload profiles.
    *   **DataFusion & Polars:** Used for analytical (OLAP) and standard relational workloads over decoupled Object Storage (Iceberg/Parquet). DataFusion is the **interim** OLAP engine; a native, push-based, morsel-driven vectorized OLAP engine is the build target, landing behind the same `ComputeBackend` seam — never assume DataFusion is the terminal OLAP backend (see `docs/12-design/NATIVE_OLAP_ENGINE_BLUEPRINT_2026_06_22.adoc`).
    *   **Volcano & Specialized Engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR):** Used for low-latency point lookups (OLTP) and high-performance Vector/ANN searches over PAX block formats.
2.  **SaaS Mandates:**
    *   **Isolation:** Multi-tenant path isolation via `DrPathBuilder` is mandatory. Do not write to root directories or raw schema locations.
    *   **Billing:** Code boundaries crossing into I/O or heavy compute must accept `TenantContext` to emit Prometheus metrics for billing.

## Domain-Specific Agent Guides
For specific agentic workflows, refer to the documents in `docs/11-usecases/`:
*   `AGENTIC_AI_API_CONTRACTS_V2_2026_05_12.md`
*   `AGENTIC_AI_BACKING_STORE_MVP_2026_05_12.md`