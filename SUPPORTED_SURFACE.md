# ProximaDB Supported Surface

This root-level file is kept only as a compatibility pointer for older links.
The authoritative v0.2 product support contract now lives at:

- [`docs/SUPPORTED_SURFACE.adoc`](docs/SUPPORTED_SURFACE.adoc)

For release work, use that AsciiDoc document as the single source of truth. It
defines the narrow v0.2 supported surface, the Beta/Experimental boundaries, and
the explicit "Not Supported in v0.2" list.

Current v0.2 positioning in short:

- Supported: single-node canonical REST/gRPC v2 record CRUD and vector search.
- Beta: hybrid retrieval, document, graph, observability, pgwire/federated SQL,
  object-economy vector routes, filtered ANN routing, security runtime, and
  time/event foundations.
- Experimental or post-MVP: distributed execution, PULSAR/QUASAR production
  claims, full SQL parity, external-table execution, Arrow Flight as a
  customer-facing transport, Spark shard-aware partitioning, JVM DataSource V2
  filter pushdown, collection-default freshness, recall-gate enforcement,
  filtered-ANN recall SLA, and object-economy benchmark/SLA claims.

Do not add release claims here. Update `docs/SUPPORTED_SURFACE.adoc` instead.
