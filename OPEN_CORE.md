# ProximaDB Open-Core Boundary

This document declares, in plain terms, what is free and open source forever
and what the commercial layer sells. It exists so nobody has to guess where
the seam is.

## The open-source engine is the whole engine

ProximaDB is licensed under [Apache-2.0](LICENSE). The **full single-node
engine** is open source with **no feature gates**:

- **Every data model** — vector, graph, document, time-series, and events
- **Postgres wire protocol** (pgwire), including the pgvector-compatible operator
- **Hybrid search** (dense + BM25 fusion)
- **The Web UI dashboard**
- **All SDKs and client libraries**
- **The reference MCP server**

There is no "community edition" subset, no enterprise-only flag in this
repository, and no capability that stops working at scale. **Self-hosting
ProximaDB at any scale is free, and will remain free.**

The single-node scoping above is deliberate: **multi-node coordination**
(consensus, replication, failover orchestration) is the commercial line, sold
as operations — while single-node durability and data scale-out over object
storage remain fully open
([ADR-084](docs/12-design/adr/ADR-084-oss-commercial-placement-worm-and-distribution.adoc)).

(Support tiers — Supported / Beta / Experimental / Planned — describe
engineering maturity, not licensing. See
[docs/SUPPORTED_SURFACE.adoc](docs/SUPPORTED_SURFACE.adoc). Nothing is held
back for payment.)

## What the commercial layer sells: operations, never features

The commercial layer (**AnvaiOps**, a separate private repository) sells
**operations** around the engine, never engine features:

- Managed multi-tenant hosting
- Governance and capability entitlements
- Usage metering and billing
- Disaster recovery
- Private connectivity
- A **governed MCP** that layers entitlement, governance, and economics over
  the open reference MCP

The principle:

> **The engine measures; the commercial layer adds meaning and money —
> we gate operations, never features.**

ProximaDB ships mechanism and neutral meters (counts, bytes, durations).
Pricing, entitlement policy, and billing semantics live entirely outside this
repository (see
[docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc](docs/12-design/OSS_ENTERPRISE_BOUNDARY_2026_06_17.adoc)).
If a change to this repository would make an engine capability conditional on
a commercial entitlement, that change is on the wrong side of the seam.

## Trademark

The **ProximaDB** name and logo are trademarks of Vijaykumar Singh. The
Apache-2.0 license covers the code; it does not grant rights to the name or
logo. You may state truthfully that your product runs on or is compatible
with ProximaDB, but do not use the name or logo in a way that implies
endorsement or official status.
