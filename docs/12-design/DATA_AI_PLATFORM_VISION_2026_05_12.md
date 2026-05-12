# ProximaDB Data and AI Platform Vision

Date: 2026-05-12

This document defines the target shape for ProximaDB as an open, protocol-compatible, multi-modal data and AI platform. The goal is to avoid painting the system into a storage-engine-specific or API-specific corner while still creating real ProximaDB stickiness through catalog, optimization, governance, agentic semantics, and lifecycle automation.

## North Star

ProximaDB should be the control plane and serving plane for context-rich applications:

- Standard clients connect through PostgreSQL compatibility, Arrow Flight, REST, and gRPC.
- Logical schemas are portable and independent of physical storage layout.
- Storage engines are selected by catalog policy: SST, VIPER, HELIX, SWIFT, NOVA, RAPTOR, and future lakehouse backends.
- Compute engines are pluggable: Rust native, DataFusion, Arrow Flight consumers, Spark for Parquet/VIPER, and future Trino.
- AI workloads are first-class: agent memory/checkpoints/events, classic ML feature stores, AutoML, model registry, experiment tracking, and online inference metadata.

The product should be sticky because it understands cross-modal data, agent workflows, optimization, governance, and lifecycle state better than commodity storage or compute engines. It should not be sticky because user data is trapped.

## Design Principles

1. **Logical SQL first, physical layout second.**
   Pgwire accepts PostgreSQL-compatible DDL and SQL. Physical storage choices are expressed as catalog options, not as a new SQL dialect per engine.

2. **Open clients, open compute, open storage.**
   JDBC, SQLAlchemy, ODBC, BI tools, Calcite, Arrow Flight, Spark, and Trino should be able to participate without bespoke client lock-in.

3. **Catalog is the source of truth.**
   Schema, storage engine, layout, indexes, modality projections, governance, lineage, feature definitions, model metadata, and workload policy live in catalog/xCatalog.

4. **ProximaDB adds semantic value.**
   Storage and compute can be replaceable. ProximaDB owns cross-modal planning, agentic primitives, vector/graph/document fusion, workload-aware optimization, and policy enforcement.

5. **Maturity is explicit.**
   Supported, beta, and experimental surfaces remain documented. Experimental engines or APIs must not be silently marketed as production-ready.

## Planes

### 1. Compatibility Plane

Purpose: let standard ecosystem clients connect without custom SDK dependency.

Surfaces:

- PostgreSQL wire protocol for JDBC, SQLAlchemy, ODBC, BI tools, psql, and Calcite.
- SQL DDL/DML/query for relational, JSONB, vector columns, and engine/layout options.
- PostgreSQL-compatible metadata introspection where feasible: `information_schema`, `pg_catalog`, type OIDs, prepared statements, transactions where supported.

Pgwire should support:

```sql
CREATE TABLE agent_memory (
  record_id TEXT NOT NULL,
  tenant_id TEXT NOT NULL,
  thread_id TEXT NOT NULL,
  namespace TEXT NOT NULL,
  key TEXT NOT NULL,
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
  embedding VECTOR(384),
  PRIMARY KEY (record_id)
) WITH (
  storage_engine = 'SST',
  layout = 'hybrid',
  xcatalog_namespace = 'agentic.default'
);
```

Alternative engine selection:

```sql
CREATE TABLE features (
  entity_id TEXT NOT NULL,
  event_time TIMESTAMP NOT NULL,
  features JSONB NOT NULL,
  embedding VECTOR(768)
) WITH (
  storage_engine = 'VIPER',
  layout = 'columnar',
  table_format = 'parquet'
);
```

The SQL surface should not require clients to know internal Rust structs. The same DDL should be usable from JDBC, SQLAlchemy, ODBC, and Calcite.

### 2. Catalog and xCatalog Control Plane

Purpose: make storage and compute replaceable while preserving semantics.

Catalog entities:

- Namespace/database/schema/table/collection.
- Logical schema: fields, types, constraints, primary keys, JSONB paths, vector dimensions.
- Physical layout: engine, layout, file format, compaction policy, partitioning, clustering, tiering.
- Modality projections: relational columns, document payload paths, graph labels/edges, vector indexes, event streams, observability fields.
- Query capabilities: supported operators, pushdown, indexes, statistics, cost model.
- Governance: RLS policies, ACLs, tenant isolation, masking, retention.
- Lineage: source datasets, transformations, model training runs, feature definitions, model outputs.
- Lifecycle metadata: schema versions, migrations, experiment/model versions, agent checkpoint/event streams.

xCatalog should be the bridge across:

- Internal catalog.
- Iceberg/Delta/Polaris/Unity/Glue/Hive-style external catalogs.
- MLflow model registry and experiment metadata.
- Future Trino/Spark connector metadata.

### 3. Storage Plane

Purpose: optimize physical persistence without leaking complexity to clients.

Canonical engine roles:

- **SST**: hybrid record/index-friendly layout for write-heavy serving, agent state, memory, events, and metadata.
- **VIPER**: Parquet columnar layout for analytics, features, offline training, Spark/DataFusion/Trino interoperability.
- **HELIX**: locality-aware vector layout for spatial/semantic locality workloads.
- **NOVA**: progressive/mixed analytical layout.
- **SWIFT**: low-latency/small-collection serving, gated by maturity.
- **RAPTOR**: adaptive/experimental analytics, gated by maturity.
- **Graph engines**: ORION for supported local graph serving; PULSAR/QUASAR only when maturity gates are met.
- **Event log**: append-only events for agent replay, audit, CDC, and MLOps lineage.

Physical engine selection should be:

- explicit via DDL options,
- recommended by AutoML/workload advisor,
- overridden by policy,
- visible in `EXPLAIN`.

### 4. Compute Plane

Purpose: route execution to the right compute without changing the client contract.

Compute targets:

- Rust-native low-latency execution for serving paths.
- DataFusion for SQL/vectorized execution over Arrow/Parquet and internal scans.
- Arrow Flight for high-throughput data exchange and external compute.
- Spark for VIPER/Parquet offline analytics and training pipelines.
- Trino later for federated SQL and lakehouse query interoperability.
- Python SDK for AutoGluon/PyCaret workflows where training runs live naturally in Python.

Planner responsibilities:

- Choose compute based on query shape, data size, engine capabilities, and freshness.
- Push filters/projections/vector predicates to storage when possible.
- Route bulk analytical scans to columnar/Arrow paths.
- Route low-latency agent/serving lookups to native Rust/SST/vector/graph paths.
- Emit explainable physical plans with engine and compute choices.

### 5. Agentic Plane

Purpose: provide first-class primitives for agent runtimes instead of forcing application-side glue.

Objects:

- Long-term memory store: `(namespace, key) -> JSONB payload`, optional embeddings.
- Checkpoints: thread state, checkpoint namespace/id, pending writes, replay.
- Event streams: append-only events, optimistic versioning, snapshots.
- Tool/run traces: inputs, outputs, latency, errors, causation/correlation IDs.
- Code/symbol knowledge: documents, graph edges, vectors, call/reference relationships.

Surfaces:

- SDK adapters for LangGraph, Victor, and other agent frameworks.
- Pgwire DDL for backing layout.
- REST/gRPC for operational APIs.
- Unified query for cross-modal retrieval.

The agentic plane should use ordinary catalog-backed tables/collections underneath, not a separate hardcoded agent schema.

### 6. Classic ML, AutoML, and MLOps Plane

Purpose: cover the data-oriented ML lifecycle, not just vector search.

Capabilities:

- Feature store:
  - offline features in VIPER/Parquet,
  - online features in SST/hybrid,
  - point-in-time correctness,
  - entity/time keys,
  - feature lineage and freshness.
- Experiment tracking:
  - runs, params, metrics, artifacts, datasets, lineage.
  - compatible mapping to MLflow concepts.
- Model registry:
  - model name/version/stage,
  - artifact URI,
  - signature/schema,
  - training dataset/version,
  - evaluation metrics,
  - deployment status.
- AutoML integration:
  - AutoGluon/PyCaret adapters in Python SDK for training orchestration.
  - ProximaDB stores features, datasets, run metadata, metrics, model registry entries, and lineage.
  - ProximaDB AutoML optimizes database physical choices: engine, indexes, quantization, cache, compaction, partitioning.
- Monitoring:
  - inference logs,
  - prediction/label joins,
  - drift metrics,
  - data quality,
  - model performance over time.

Pgwire is useful for feature tables, run/metric queries, and metadata access. It is not enough for the entire MLOps lifecycle because model artifacts, long-running training jobs, AutoGluon/PyCaret orchestration, and deployment transitions need a separate API/SDK surface.

Recommended split:

- SQL/pgwire: features, predictions, labels, run metrics, model registry metadata, lineage tables.
- Arrow Flight: bulk training datasets and prediction exports.
- Python SDK: AutoGluon/PyCaret orchestration, MLflow import/export, artifact registration, model promotion workflows.
- REST/gRPC: job management, registry operations, governance, serving metadata.

## Data-Oriented Domains Covered

| Domain | ProximaDB Surface | Notes |
| --- | --- | --- |
| Relational | pgwire SQL, catalog schema | JDBC/SQLAlchemy/ODBC compatibility target |
| Document | JSONB columns and document collections | Schema-flexible with indexed paths |
| Graph | graph projections and graph engines | Labels/edges stored in catalog, queried via Cypher/extensions |
| Vector | VECTOR columns, vector collections, indexes | pgvector compatibility plus native vector engines |
| Time-series | timestamp keys, partitions, future TST | ASOF/downsampling roadmap |
| Event log | append-only streams | agent replay, audit, CDC, lineage |
| Observability | logs/metrics/traces | platform telemetry and model monitoring |
| Feature store | online/offline features | point-in-time correctness required |
| Model registry | catalog-backed registry | MLflow-compatible mapping |
| Experiment tracking | runs/params/metrics/artifacts | SQL queryable metadata, external artifact storage |
| Agent state | memory/checkpoints/events/tools | LangGraph/Victor-style adapters |
| Data lake/lakehouse | VIPER/Parquet, external catalogs | Spark/Trino/DataFusion interoperability |
| Governance | RLS, ACL, masking, retention | one policy layer across modalities |
| Lineage | catalog/event log | data, model, query, and agent provenance |

## Where Pgwire Helps and Where It Does Not

Pgwire helps when:

- Clients already use JDBC/SQLAlchemy/ODBC.
- The task is schema creation, metadata introspection, relational querying, feature-table access, or BI integration.
- Calcite or another SQL planner needs a PostgreSQL-like endpoint.
- Users want storage-engine options through SQL DDL.

Pgwire is insufficient alone when:

- Bulk data movement needs Arrow columnar batches.
- ML training needs large dataset streams.
- Model artifacts need upload/download/versioning.
- Long-running AutoML jobs need cancellation/status/results APIs.
- Agent frameworks need native memory/checkpoint/event methods.
- Graph traversal needs rich path/neighbor operations beyond SQL result tables.

Therefore pgwire is a compatibility plane, not the only API.

## Stickiness Without Lock-In

ProximaDB should avoid proprietary-data lock-in and instead create stickiness through:

- Cross-modal catalog and query semantics.
- Engine-aware optimizer and workload advisor.
- Unified governance/RLS/retention/masking across modalities.
- Agentic primitives that reduce framework glue.
- MLOps lineage joining features, models, runs, predictions, drift, and agent actions.
- Operational simplicity: one catalog, one policy system, one observability story.
- Embedded-to-server continuity.
- Explainability: users can see why a query used SST, VIPER, DataFusion, Arrow Flight, or native Rust.

## Architecture Roadmap

### Phase 0: Guardrails

- Remove hardcoded agent-specific proto/schema as source of truth.
- Keep SDK-authored DDL and catalog options as the flexible schema authoring path.
- Validate pgwire-compatible DDL through Rust parser/executor tests.
- Document maturity honestly in supported-surface docs.

### Phase 1: Pgwire DDL and Catalog Options

- Support quoted identifiers, JSONB, VECTOR(n), primary keys, indexes.
- Parse `WITH (storage_engine='...', layout='...', table_format='...')`.
- Parse `USING <engine>` as storage engine only, not logical data model, when engine name is SST/VIPER/HELIX/SWIFT/NOVA/RAPTOR.
- Persist storage options into catalog/xCatalog.
- Expose options through information schema or ProximaDB catalog tables.

### Phase 2: Mixed Schema Lowering

- Lower logical DDL into:
  - relational columns,
  - JSONB payload paths,
  - vector index specs,
  - graph projections,
  - event streams,
  - feature-store declarations.
- Keep the logical schema stable while allowing engine migration.
- Add `EXPLAIN CREATE TABLE` or catalog explain for physical layout.

### Phase 3: Execution Routing

- Route OLTP/agent serving to Rust-native/SST/graph/vector paths.
- Route analytical columnar scans to VIPER/DataFusion/Arrow Flight.
- Add Spark export/read contract for VIPER/Parquet.
- Add Trino connector after catalog and Arrow/Parquet metadata are stable.

### Phase 4: Agentic Runtime

- Finish LangGraph BaseStore/checkpoint parity.
- Finish Victor provider direct protocol integration.
- Add schema-backed memory/checkpoint/event resources.
- Add cross-modal query execution, not just parser/planner tests.

### Phase 5: MLOps and AutoML

- Add Python SDK MLOps module:
  - `FeatureStore`,
  - `ExperimentTracker`,
  - `ModelRegistry`,
  - `AutoMLRunner`.
- Integrate AutoGluon and PyCaret as optional extras.
- Add MLflow import/export or tracking adapter.
- Store artifacts externally or in object storage; store metadata/lineage in ProximaDB.
- Connect model monitoring to observability/event tables.

### Phase 6: External Catalogs and Commodity Compute

- Stabilize Iceberg/Delta/Polaris/Unity mappings.
- Implement DataFusion TableProvider over catalog scans.
- Implement Spark DataSource for VIPER/Parquet and feature datasets.
- Add Trino connector once catalog contracts settle.

## Key Risks

- **Overloading pgwire**: trying to force model artifacts and long-running ML jobs through SQL will create a poor API. Use SDK/REST/gRPC for job/artifact lifecycle.
- **Engine-name ambiguity**: `USING GRAPH` is logical model; `USING VIPER` is physical engine. Parser and docs must keep this clear.
- **Catalog drift**: external catalog, internal catalog, pgwire, and Arrow schema must derive from one canonical type system.
- **Premature production claims**: experimental engines and modality paths need clear gates.
- **Too many APIs with inconsistent semantics**: REST/gRPC/pgwire/Arrow/Python must share catalog and planner contracts.

## Immediate Implementation Items

1. Keep `clients/python/.../agentic_ddl.py` as the SDK DDL authoring surface for agentic/mixed schemas.
2. Add DDL options support for `WITH (storage_engine, layout, table_format, xcatalog_namespace)`.
3. Add Rust tests proving `CREATE TABLE ... JSONB ... VECTOR(n)` parses and executes through DDL/DML executor.
4. Add storage-options detection tests for SST/VIPER/HELIX/SWIFT/NOVA/RAPTOR.
5. Persist storage options into catalog metadata.
6. Grow the Python SDK MLOps contract skeleton into executable adapters for MLflow, AutoGluon, and PyCaret.
7. Add Arrow Flight dataset export/import tests for feature tables.
