# Agentic AI Backing Store MVP

Date: 2026-05-12

This note maps industry agent persistence patterns, LangGraph's current store/checkpoint
contracts, and Victor's local storage model to the ProximaDB embedded MVP.

## Reference Contracts

LangGraph separates persistence into two contracts:

- Checkpointer: per-thread graph state, super-step checkpoints, pending writes, replay,
  time travel, and delete-by-thread semantics.
- Store/BaseStore: cross-thread long-term memory as JSON documents addressed by
  `(namespace_tuple, key)`, with `put`, `get`, `search`, `delete`, namespace listing,
  pagination, metadata filters, and optional semantic search over selected fields.

Victor has similar but broader local needs:

- Global database: settings, profiles, RL outcomes, tool/model preferences, cross-project
  patterns.
- Project database: graph nodes/edges, conversations, sessions, entity memory, change
  tracking.
- Unified symbol store: today SQLite + LanceDB; planned ProximaDB single-engine backend.
- Event sourcing: append-only domain events with stream id, stream version, optimistic
  concurrency, global position, replay, and snapshots.
- Checkpoints: workflow/session checkpoints with state snapshots.
- Vector providers: `BaseEmbeddingProvider` for embedding generation, vector indexing,
  metadata-filtered search, per-file deletion, stats, and close.

## Agentic Store Capabilities

An agentic AI backing store should cover these lanes:

| Lane | Required Capability | ProximaDB Fit |
| --- | --- | --- |
| Short-term thread state | checkpoint put/get/list, pending writes, replay, fork/update, delete thread | Needs explicit Python checkpointer adapter |
| Long-term memory | namespaced JSON KV, prefix namespace listing, filters, semantic search, per-item timestamps | Mostly covered by document + vector engines; needs BaseStore-compatible wrapper |
| Semantic memory/RAG | dense/sparse vectors, metadata filters, batch upsert/search, hybrid lexical/vector | Covered by vector engines; embedded API covers vector MVP |
| Project/code graph | nodes, edges, labels, properties, traversal, callers/callees, impact analysis | Embedded Python exposes graph CRUD/traversal; Victor provider already uses server SDK |
| Documents | JSON document CRUD/query, schema-free records, indexed paths | Embedded API exposes document CRUD/query; engine is still experimental/in-memory per supported surface |
| Relational metadata | typed rows, joins, ordering, pagination, SQL-ish access | Server has experimental SEQUOIA; embedded `execute_sql` exists but needs ORM-like mapper tests |
| Event log/audit | append-only events, stream version, causation/correlation, replay, snapshots | EventLog exists experimentally; needs Python-facing event-store adapter |
| Observability | logs, metrics, traces, trace lookup, metrics aggregation | Embedded Python exposes logs/metrics/traces; engine is experimental/in-memory per supported surface |
| Cross-modal query | vector/document/graph/log/metric SQL extensions, result fusion | Query layer exists; embedded proof tests are needed |
| Operational envelope | local embedded mode, WAL, flush, stats, backup/recovery, tenant/filter pushdown | Strong for vectors; uneven for newer modalities |

## Current ProximaDB Status

Strong MVP base:

- Embedded Python exposes vector collection CRUD, batch insert/search, NumPy transfer,
  flush, stats, graph node/edge CRUD/traversal, document CRUD/query, logs/metrics/traces,
  `execute_sql`, and `execute_unified_query`.
- Supported-surface docs mark SST/VIPER/NOVA/HELIX vector engines and ORION graph as the
  mature lanes.
- Unified/federated query code recognizes `VECTOR_SEARCH`, `GRAPH_QUERY`,
  `DOCUMENT_QUERY`, `LOGS`, and `METRICS`.

Main gaps before claiming full agentic backing-store coverage:

- LangGraph-compatible `BaseStore` wrapper is not present.
- LangGraph-compatible checkpointer/saver is not present.
- SQLAlchemy-like Python mapper/session layer is not present. For MVP, a lightweight
  mapper over embedded documents/records is enough; full SQLAlchemy dialect can come later.
- Event store adapter for Victor's `EventStore` protocol is not present.
- Victor's existing `ProximaDBMultiModelProvider` targets the server SDK, not the embedded
  `proximadb_embedded` package.
- Document, relational, observability, and event engines are still marked experimental or
  in-memory in `SUPPORTED_SURFACE.md`; MVP claims must be local/experimental until durability
  tests prove otherwise.
- Cross-modal embedded tests need to prove one real flow, not only parser/fusion units.

## MVP Shape

Build embedded-first adapters in this order:

1. `ProximaBaseStore`
   - Implements LangGraph-style `put/get/search/delete/list_namespaces`.
   - Stores item JSON in document collection `lg_store_items`.
   - Stores optional embeddings in vector collection `lg_store_vectors`.
   - Uses namespace tuple, key, created_at, updated_at, value, and index fields.

2. `ProximaCheckpointSaver`
   - Stores checkpoints in document/relational-compatible collections:
     `lg_checkpoints` and `lg_checkpoint_writes`.
   - Keys by `thread_id`, `checkpoint_ns`, `checkpoint_id`.
   - Supports `put`, `put_writes`, `get_tuple`, `list`, and `delete_thread`.
   - Uses EventLog later for append-only checkpoint audit.

3. Victor embedded provider
   - Add `proximadb_embedded` backend beside SQLite + LanceDB.
   - Implement `UnifiedSymbolStoreProtocol` against embedded graph + vector + document APIs.
   - Keep stable Victor IDs as ProximaDB record IDs.

4. Lightweight ORM/IO layer
   - Pydantic/dataclass mapper: `create_table/profile`, `upsert`, `get`, `query`,
     `delete`, `vector_search`, `graph_link`.
   - Do not start with a full SQLAlchemy dialect; first prove shape mapping to Rust-backed
     embedded APIs with typed JSON/document rows and stable IDs.

5. End-to-end proof
   - One pytest creates an embedded DB, writes LangGraph-style memories, checkpoints,
     Victor code symbols, graph edges, events, logs, and vectors.
   - It then runs semantic search, graph traversal, checkpoint replay lookup, event replay,
     and one cross-modal query through `execute_unified_query` or equivalent adapter calls.

## Acceptance Criteria

- Python import works with `import proximadb_embedded`.
- No server process is required for the MVP path.
- BaseStore semantics match LangGraph behavior for namespace prefix search, limit/offset,
  filters, timestamps, and optional semantic search.
- Checkpointer supports latest checkpoint lookup, history listing, pending writes, and
  delete-by-thread.
- Victor can swap from SQLite + LanceDB to ProximaDB embedded for a small indexed repo.
- Tests explicitly label experimental lanes and do not claim production durability for
  document/relational/observability/event storage until supported-surface status changes.
