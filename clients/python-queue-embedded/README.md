# proximadb-queue-embedded

In-process Python bindings for the ProximaDB tiered persistent queue
(`proximadb-queue` Rust crate). Same durability + lease semantics as
the Rust API, surfaced through a synchronous Python interface (the
internal tokio runtime is hidden from the caller).

Internal-only by architectural design — see locked invariant #2 in
`crates/modalities/proximadb-queue/README.md`. Customers reach the
queue only through the public REST/gRPC/Arrow Flight ingest paths;
this wheel exists for internal tooling, tests, and drainer
integration on Python-managed pods.

## Install

```bash
cd clients/python-queue-embedded
maturin develop --features proximadb-queue/python
```

## Usage

```python
from proximadb_queue_embedded import QueueClient, partition_for

client = QueueClient(
    root="file:///var/lib/proximadb/queue",
    topics={"embed-ingest": {"partition_count": 4}},
)

producer = client.producer()
receipt = producer.send("embed-ingest", "tenant-a", b"{...}")
# receipt.fsynced == True when topic sync_mode is Strict (default)

consumer = client.consumer("g")
consumer.subscribe("embed-ingest", [0, 1, 2, 3])
batch = consumer.poll(max_batch=32, max_wait_ms=200)
for msg in batch:
    process(msg.payload)
consumer.ack([m.message_id for m in batch])

client.shutdown()
```

`partition_for("tenant-a", 16)` is exposed for callers that need to
compute partition assignments outside an open client (e.g. routing
config generators).

## Tests

```bash
pytest tests/
```

Tests will skip cleanly if `maturin develop` hasn't been run.
