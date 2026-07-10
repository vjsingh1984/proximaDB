00-system-overview/       → Top-level architecture, deployment, use cases
01-network-layer/         → Network stack: gRPC, REST, pgwire, Arrow Flight
02-query-pipeline/        → Query routing, ComputeScheduler, DataFusion/Volcano
03-storage-layer/         → Storage engines, PAX, WAL, Iceberg, cache, transactions
04-index-layer/           → HNSW, Compact, IVF indices
04-services-catalog/      → Services, RecordStore, catalog, CDC, connectors
05-cluster-distributed/   → Cluster, partition lease, coordination
07-datafusion-integration/→ DataFusion OLAP integration (ADR-052)
07-graph-subsystem/       → Graph engine (ORION)
08-ai-llm/                → AI/LLM subsystem, embedding pipeline
09-clients-sdk/           → Python SDK architecture
09-coupling-analysis/     → God-file coupling analysis
09-security-governance/   → Security, tenant isolation, governance
10-cross-cutting/         → Observability, bootstrap
10-deployment/            → System deployment diagram
11-crate-dependency-map/  → Workspace crate layering