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
5. **The async drainer bypasses WAL+memtable and bulk-loads SST
   segments directly.** This is LSM-aware: per-record WAL+memtable
   ingestion is optimal at p99-latency-budgeted batch-of-one writes;
   batched bulk-load is optimal once you've amortized over more than a
   handful of records. The queue's disk tier already provides
   durability before the drainer pulls, so the WAL is redundant
   overhead on the async path. The drainer:
   (a) pulls a batch from one partition,
   (b) embeds them in one inference call,
   (c) sorts by oid,
   (d) writes a single SST segment with one fsync,
   (e) atomically commits the segment to the collection's manifest,
   (f) acks the queue messages.
   Crash recovery is "replay the queue from the last ack" — the SST
   segment is either committed or it isn't; there's no half-state.
6. **Queue partitions are aligned with tenant-to-instance assignment.**
   The drainer for partition K only consumes messages for tenants
   whose target collection lives on the local instance. This avoids
   cross-instance writes entirely in steady state. Scaling events
   trigger lease handoff: old owner drains in-flight, then new owner
   picks up.
7. **Queue files live under `{queue_root}`; collection files live
   under `{collection_root}`. The two trees never intersect.** Search
   enumerates collections via `CatalogManager` and only sees
   `{collection_root}`. `FilesystemFactory` operations from the queue
   are scoped to `{queue_root}`. Even hosting them on different
   physical storage (local NVMe for queue, ADLS for collections) is
   supported. This prevents any chance of a queue segment leaking
   into a vector search result.
8. **At-least-once delivery + idempotent consumer is the durability
   contract** (NOT exactly-once). Drainers may legitimately re-write
   a record after a crash between SST commit and queue ack. The LSM
   compaction in the target collection folds duplicates by oid, so
   customers see the record exactly once in search. Producers MUST
   supply a deterministic `source_record_id` for this to hold — fresh
   UUIDs per call defeat the dedup. This trade-off is deliberate;
   true exactly-once requires distributed transactions we don't build.
9. **Messages are reaped at the segment level, not per-message.** Ack
   commits an offset; sealed segments are deleted only after all
   consumers have committed past their last offset AND (if configured)
   the segment is uploaded to the object archive. Trade-off: ack'd
   messages occupy disk space until the segment rotates. Acceptable
   because the offset.meta is the authoritative cursor.

## Architecture (LSM-aware)

ProximaDB's storage engines (SST, VIPER, NOVA) are LSM-based. The sync
write path and the async write path land at the same final storage layer
(SSTs on disk) but take **deliberately different** routes there because
the optimal LSM ingestion pattern differs by batch size.

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
                   ┌────────────┐      ┌──────────────────────┐
                   │ embed      │      │ queue.send           │
                   │ inline     │      │  → memory tier       │
                   │            │      │  → disk segment      │
                   │            │      │  → group-commit fsync│
                   │            │      │  → MessageReceipt    │
                   │            │      │  → 202 to client     │
                   └────────────┘      └──────────────────────┘
                          │                       │
                          │                       │
                          │            (per-tenant-partitioned drainer
                          │             running on the instance that
                          │             OWNS the target collection;
                          │             no cross-instance RPC)
                          │                       │
                          │             queue.poll(batch)
                          │                       │
                          │             embed batch in one shot
                          │             sort by oid
                          │                       │
                          ▼                       ▼
              ┌──────────────────┐      ┌─────────────────────────┐
              │ WAL append       │      │ BULK-LOAD: write SST    │
              │   ↓ fsync        │      │ segment directly        │
              │ memtable insert  │      │   ↓ one fsync per batch │
              │   ↓ (async flush)│      │ atomic manifest commit  │
              │ SST segment      │      │   ↓                     │
              │   ↓ (compaction) │      │ queue.ack               │
              │ merged tiers     │      │                         │
              └──────────────────┘      └─────────────────────────┘
                       │                          │
                       └─────────┬────────────────┘
                                 ▼
                          searchable corpus
```

### Why the two paths converge at SST but take different routes

| Path | Latency budget | LSM pattern | Why |
|---|---|---|---|
| **Sync** | < 2s p99 | WAL → memtable → SST (background flush) | One record at a time; latency-dominated; memtable serves immediate reads before flush; cheap fsync amortized over the request lifecycle |
| **Async** | 202 in <10ms; searchable in <5min | Queue → bulk-load SST (skip WAL + memtable) | Drainer accumulates N records per batch; one SST write + one fsync amortizes across N; skipping memtable avoids memory pressure from large backfills; sorted-on-write avoids compaction churn |

**Async deliberately bypasses WAL and memtable.** Durability is provided
by the queue's disk tier — the drainer only pulls messages that have
already been fsync'd. On drainer crash mid-SST-write, recovery is:
replay queue (idempotent via `message_id`) → re-derive the SST segment.
This is the LSM bulk-load pattern used by RocksDB `IngestExternalFile`,
Cassandra `SSTableLoader`, and DuckDB COPY.

### Storage isolation — queue files never reach the search index

The queue and the collection storage live in **completely separate file
trees**, both backed by `FilesystemFactory` but with different roots:

```
{queue_root}/                           ← e.g. file:///var/lib/proximadb/queue
  embed-ingest/                         ← vector modality
    0/0000001.qseg
    0/offset.meta
    0/lease.meta
    ...
  billing-events/                       ← cross-cutting
    ...
  doc-index/                            ← document modality
    ...
  graph-edge-discovery/                 ← graph modality
    ...
  dlq.embed-ingest/                     ← dead-letter queue
    ...

{collection_root}/                      ← e.g. file:///var/lib/proximadb/collections
  knowledge/
    sst/00000abc.sst                    ← actual vector data, .sst
    wal/0000001.bcwal                   ← LSM WAL
    manifest.json
  ...
```

ProximaDB search enumerates collections via `CatalogManager` and only
lists files under `{collection_root}`. It has **no knowledge** of
`{queue_root}` and cannot accidentally scan queue segments. Similarly,
the queue's `FilesystemFactory` operations are scoped to `{queue_root}`
only. The two roots can even live on different physical storage
(local NVMe for queue, ADLS for collections) without affecting each
other.

#### Topic naming convention by modality

Each ProximaDB modality owns dedicated topic names; they share the
`{queue_root}` filesystem but cannot interfere because each topic lives
in its own subdirectory:

| Modality | Topic name | Use |
|---|---|---|
| Vector | `embed-ingest` | text → vector embedding before SST bulk-load |
| Document | `doc-index` | tokenize + chunk before document collection write |
| Graph | `graph-edge-discovery` | edge inference before graph mutation |
| Observability | `obs-aggregation` | rollup events before observability collection write |
| Cross-cutting | `billing-events` | meter ticks for KRU/KIU/KSU |
| Cross-cutting | `dlq.{original_topic}` | dead-letter for failed messages from `{original_topic}` |

### Drainer atomicity — process-then-commit + at-least-once semantics

The drainer wraps each batch in a four-step boundary:

```
loop:
  msgs    = consumer.poll(batch_size)        # [A] in-flight; not deleted
  vectors = EmbeddingService.embed(msgs)     # [B] pure compute, no side effect
  sst     = bulk_load_sst(vectors, msgs)     # [C] write SST + atomic manifest swap
  consumer.ack(msg_ids)                      # [D] offset commit → segment reapable
```

The atomic boundary is `[C]`: the SST file is staged as a temp file
under `{collection_root}/{name}/sst/.tmp/`, then a single
compare-and-swap on the collection's manifest publishes it. Either the
segment is committed or it isn't — there's no half-state.

#### Crash recovery scenarios

| Crash between | Outcome on restart |
|---|---|
| `[A]` and `[B]` | Message stays in queue; visibility timeout (or lease expiry) makes it available again; another consumer re-embeds + re-writes |
| `[B]` and `[C]` | Same — inference is pure compute with no committed state; safe to redo |
| `[C]` and `[D]` | SST is already in target collection; message still in queue (not yet ack'd); on restart, drainer re-embeds + writes again → **at-least-once duplicate** → LSM compaction folds records with the same oid |

State is split deliberately:
- **Queue state** (offset commits, in-flight tracking, lease metadata)
  lives in `{queue_root}/...`.
- **Collection state** (SST segments, manifest) lives in the collection
  service via `CatalogManager`.
- The drainer is the **only** component that touches both.

The queue's `Consumer::ack` is the single source of truth for "did
processing complete." The collection service doesn't know about queue
messages — it just sees SST writes arrive.

#### Exactly-once vs at-least-once — picked at-least-once + idempotent consumer

True exactly-once requires distributed transactions (2PC between the
queue and the collection storage). We don't build that; instead we
commit to **at-least-once delivery + idempotent consumer**, which gives
the same observable behavior to customers:

| Property | How we achieve it |
|---|---|
| Message durability before delivery | Queue Strict-mode fsync before producer ack |
| At-least-once delivery | Visibility timeout + lease expiry on crash means unacked messages are re-delivered |
| Idempotent consumer | Drainer uses `tenant_id + source_record_id` as the SST oid; duplicate writes are folded by LSM compaction (same oid = single record) |
| End-to-end exactly-once *behavior* | Customer sees the record exactly once in search because LSM dedup absorbs the duplicate writes from at-least-once delivery |

**Producer contract.** The customer's `source_record_id` must be
deterministic per logical record. If a customer generates a fresh UUID
on every API call for the same logical document, duplicates land as
distinct oids and won't be folded. Idempotent producers are a
prerequisite for our exactly-once-by-content guarantee — this matches
how Kafka+Cassandra, Pulsar+ScyllaDB, and every other production
queue+storage pipeline works.

#### Message removal — segment-level reaping, not per-message delete

We do not delete individual messages on ack. The lifecycle is:

```
1. Producer writes message → appended to active disk segment
2. Drainer ack → offset committed (small write to offset.meta)
3. Active segment fills (16 MB default) → sealed; new active segment created
4. Sealed segment eligible for archive after all partition consumers
   have committed past its last offset
5. Object tier uploader copies sealed segment to object archive
6. Reaper deletes the on-disk sealed segment only after upload + all
   acks past its end
```

Per-message deletion would be O(messages) random disk I/O — catastrophic
for throughput. Segment-level deletion is one `unlink()` call per 16 MB
chunk. The trade-off: a small window where ack'd messages still occupy
disk space until the segment rotates. Acceptable because consumers never
re-deliver ack'd messages, and the offset.meta is the authoritative
cursor — segment retention is just storage, not state.

### Throughput economics — why the queue path is structurally cheaper

The two paths take different LSM routes, but the larger cost difference
lives in the embedding inference stage. The queue absorbs bursty
customer traffic so the embedding worker runs at its optimal sustained
throughput; the sync path has no such buffer.

| Concern | Sync (inline) | Async (queue + drainer) |
|---|---|---|
| Batch size at inference | 1 record per request | 32-128 per drainer pull |
| Per-record GPU/CPU setup | Full per call | Amortized over the batch |
| Effective throughput | ~50ms p99 single inference | ~5ms p99 amortized in batch |
| Capacity sizing | Reserved per-tenant for p99 burst | Pooled across all tenants' async traffic |
| Backpressure surface | **Customer-facing** (HTTP 429, latency tail) | **Internal** (drainer slows, queue depth grows) |
| Cross-tenant batching | Impossible (one call = one tenant) | Possible (one drainer batch can pull from many tenants on the same partition) |
| GPU/CPU utilization | Bursty, idle between requests | Steady-state, saturates the embedding worker |

A 32-record drainer batch costs ~1.5× a single-record inference (model
load + tokenizer setup amortized) → **~21× more records per unit of
compute** vs the sync path. This is the structural reason async ingest
is roughly 5-10× cheaper per record at the inference layer alone (the
LSM bulk-load on top adds further savings at the storage layer).

This throughput differential is the architectural basis for the higher
sync price tier in AnvaiOps pricing (the public-facing rate uses a +33%
premium on sync ingest vs async — that's the customer-visible
translation of "you're consuming reserved capacity that can't be
amortized"). See `docs/PRICING_INTERNAL.md` in the AnvaiOps repo for the
business model that points back to this architectural cost profile.

### Per-tenant routing aligns queue partitions with storage assignment

The drainer **must not** ship records to a different instance — the SST
write needs to land on the instance that owns the target collection's
storage. We achieve this by aligning the queue partition assignment with
the tenant-to-instance assignment:

```
partition_for(tenant_id) = xxhash64(tenant_id) mod partition_count

When partition_count == instance_count and instance K owns the partitions
hashing to K:
  - Each drainer only consumes messages for tenants whose collection
    lives on the same instance
  - SST writes are always local; no cross-instance RPC
  - Per-tenant FIFO is preserved across the instance boundary
```

When instance count changes (scale event), partition reassignment uses
the disk-backed lease + handoff: the old owner finishes draining
in-flight messages before the new owner picks up. Combined with the
queue's `Consumer::nack` retry semantics, this gives at-least-once
delivery with no cross-instance traffic in the steady state.

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
