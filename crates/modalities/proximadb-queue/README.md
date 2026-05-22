# proximadb-queue

Tiered persistent queue for ProximaDB's async ingest, billing event, and
LLM extraction work paths. Memory + disk + object-store tiers, per-tenant
partitioning, strict-by-default `fsync` durability via group commit.

## When to use the queue vs the WAL

This crate is **not** a replacement for ProximaDB's per-collection WAL.
They serve different roles and stack together:

| Role | WAL (per-collection, existing) | Queue (per-topic/partition, this crate) |
|---|---|---|
| What it's for | Durability for collection state mutations | Buffer for async work waiting to commit to a collection |
| Unit of work | Vector record insert / update / delete | Opaque message — an *intent* to do work |
| Consumer model | Single — the storage engine | Many — drainer pulls embed-ingest; rollup pulls billing-events; … |
| Offset semantics | Implicit (flush watermark) | Explicit per-partition commit |
| Replay semantics | Reconstruct collection state | Re-deliver unacked messages |
| Schema | `ProximaRecord`, schema-validated | `Vec<u8>` opaque payload |
| Tenancy | One stream per collection | One stream per topic + N partitions hashed by `tenant_id` |
| Retention | Until flush + checkpoint | Until consumer ack + archive |
| Backpressure surface | Storage engine internal | Customer request handler (→ HTTP 429) |

### Routing matrix

| Workload | Path | Why |
|---|---|---|
| Sync vector ingest | WAL only | Single consumer; <2s p99 budget; no async semantics needed |
| Async vector ingest | **Queue → WAL** | 202 ack <10ms; retry on transient embed failure; per-tenant fairness via partition hashing |
| Billing events (KRU/KIU/KSU) | **Queue → billing-events collection** | Distinct consumer; strict durability; doesn't belong in any vector collection's WAL |
| LLM extraction tasks | **Queue → extraction-results** | Long-running; different priority/concurrency; opt-in feature |
| Connector watermarks | WAL of `anvaiops_connector_state` | Already designed; tiny per-poll mutation; single consumer |
| Backpressure to client | **Queue → 429** | Queue sees the call first; WAL pressure is downstream and propagates as queue consumer slowdown |
| Search results streaming | NEITHER | RPC stream, not persistent |

The customer never reaches this crate directly. It's a coordination
primitive between the public API layer (REST, gRPC, Arrow Flight, UQL,
AQL) and the storage engine. From the customer's perspective the
distinction between "queue → WAL" and "WAL only" is just
`X-Ingest-Mode: sync` vs `X-Ingest-Mode: async`.

### Locked decisions

These are non-negotiable architectural constraints. New code must
respect them; deviations require explicit revision of this document.

1. **Sync ingest never touches the queue.** When
   `X-Ingest-Mode: sync` (default), the request handler calls the
   in-process `EmbeddingService` inline and writes to the WAL directly.
   No `queue.send` on this path, ever. Adding a queue hop to sync would
   break the <2s p99 sync SLA and double the failure surface area.
2. **The queue is internal-only.** No customer-direct `publish` /
   `subscribe` API is exposed. All access is mediated through the
   existing public ingest surfaces (REST, gRPC, Arrow Flight, UQL,
   AQL). Customers using ProximaDB as a generic message broker is
   explicitly out of scope; if that demand emerges, build a dedicated
   product on top of this crate.
3. **`partition_for(tenant_id)` is the single source of truth for
   partition routing.** Producers cannot override the partition;
   tenant_id → partition is deterministic per the topic's
   `partition_count`. This guarantees per-tenant FIFO and prevents
   accidental cross-tenant ordering bugs.
4. **At most one consumer per `(consumer_group, partition)`.** The
   in-process `DashMap`-backed lease enforces this within a single
   ProximaDB process; cross-process disk-backed leases extend the same
   guarantee across replicas when the disk tier lands.

## Architecture (one-screen)

```
                           customer
                              │
                              ▼
              ┌────────────────────────────────┐
              │ ProximaDB request handlers     │
              │ (REST /v3/documents, Arrow     │
              │  Flight DoPut, gRPC Insert)    │
              └────────────────────────────────┘
                     │                   │
            mode=sync│                   │mode=async
                     ▼                   ▼
              ┌────────────┐      ┌────────────────────┐
              │ inline     │      │ queue.send         │
              │ embed      │      │  → memory tier     │
              │            │      │  → disk segment    │
              │            │      │  → group-commit    │
              │            │      │    fsync           │
              │            │      │  → MessageReceipt  │
              └────────────┘      │  → 202 to client   │
                     │            └────────────────────┘
                     │                       │
                     │              (background drainer)
                     │              queue.poll → embed
                     │                       │
                     ▼                       ▼
              ┌──────────────────────────────────────┐
              │ request_handlers.insert →            │
              │ WAL append → memtable → HNSW/SST     │
              └──────────────────────────────────────┘
```

## Phase 1B scope

| Layer | Status |
|---|---|
| Memory tier — lock-free `ArrayQueue` per partition with soft/hard pressure | ✅ |
| Producer / Consumer / QueueClient public API | ✅ |
| `partition_for(tenant_id)` xxhash64-based routing | ✅ |
| Topic auto-create on first send | ✅ |
| Lazy mode (memory-only ack) | ✅ |
| Disk tier — segment writer + group-commit fsync via `FilesystemFactory` | 🔜 |
| Object tier — sealed-segment upload to `adls://` / `s3://` / `gcs://` | 🔜 |
| Offset store — durable per-partition committed-offset metadata | 🔜 |
| Recovery — startup replay from disk + object store | 🔜 |
| Cross-process partition leases | 🔜 (currently in-process via DashMap) |
| Strict-mode `fsync_at` is *guaranteed* (currently approximated) | 🔜 |
| Embedding drainer wired to `embed-ingest` topic | 🔜 |
| REST `/v3/documents?mode=async` rewires to `producer.send` | 🔜 |

The first four columns ship as focused follow-up commits; each integration
piece (drainer, REST async-mode rewire) lands after the disk tier is
durable.

## Test coverage

```bash
cargo test -p proximadb-queue
```

19 tests, 0 failures:

- **roundtrip (3)** — produce → consume → ack happy path; Lazy mode
  receipt shape; partition determinism.
- **inline topic (2)** — `partition_for` stability + distribution.
- **edge cases (14)** — soft pressure at 75%; hard pressure at 95%; FIFO
  within partition; concurrent producers (8 tasks × 32 sends);
  `max_batch` cap; `poll` timeout returns empty; subscribe to unknown
  partition errors cleanly; auto-create topic on first send; ack of
  unknown message id is no-op; `partition_for` always in range; poll
  without subscription returns promptly; second consumer in same group
  sees remaining messages; `shutdown` is idempotent; handle creation is
  cheap.

## Config

```toml
[queue]
root = "file:///var/lib/proximadb/queue"     # or adls://anvaiops/queue
object_archive = "adls://anvaiops/queue-cold" # optional cold tier
default_sync_mode = "strict"                  # or "lazy"

[queue.topics.embed-ingest]
partition_count = 16
memory_capacity = 4096                        # entries per partition
disk_rotation_size_mb = 16
archive_after = "1h"
max_attempts = 5                              # then → DLQ
group_commit_max_wait = "5ms"
group_commit_max_batch = 64
```

Env-var overrides: `PROXIMADB_QUEUE_ROOT`, `PROXIMADB_QUEUE_OBJECT_ARCHIVE`,
`PROXIMADB_QUEUE_SYNC_MODE`. Per-topic overrides via the config file or
the runtime API only.
