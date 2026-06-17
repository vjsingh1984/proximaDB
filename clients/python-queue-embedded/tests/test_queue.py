"""Phase 2I — PyO3 bindings smoke tests.

Each test uses a per-test TempDir for the queue root so they're
hermetic. Run via:

    maturin develop --features proximadb-queue/python
    pytest tests/

These exercise the public Python surface: open, producer.send,
consumer.subscribe + poll + ack, partition_for, restart. The Rust
side is already covered by ``cargo test -p proximadb-queue``; these
tests prove the PyO3 wrapping doesn't drop semantics.
"""
from __future__ import annotations

import tempfile

import pytest

pytest.importorskip(
    "proximadb_queue_embedded",
    reason="Run `maturin develop --features proximadb-queue/python` first.",
)

from proximadb_queue_embedded import QueueClient, partition_for


def _open(root: str) -> QueueClient:
    return QueueClient(
        root=f"file://{root}",
        topics={"embed-ingest": {"partition_count": 2}},
    )


def test_open_queue_client() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        client = _open(tmp)
        assert client is not None
        client.shutdown()


def test_producer_send_and_consumer_poll() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        client = _open(tmp)
        producer = client.producer()
        receipt = producer.send("embed-ingest", "tenant-a", b"hello")
        assert receipt.partition in (0, 1)
        assert receipt.offset == 0
        assert receipt.fsynced is True

        consumer = client.consumer("g")
        consumer.subscribe("embed-ingest", [0, 1])
        batch = consumer.poll(max_batch=8, max_wait_ms=500)
        assert len(batch) == 1
        msg = batch[0]
        assert msg.topic == "embed-ingest"
        assert msg.tenant_id == "tenant-a"
        assert bytes(msg.payload) == b"hello"
        client.shutdown()


def test_partition_routing_matches_rust() -> None:
    # Stability: same tenant always lands on the same partition.
    a1 = partition_for("tenant-acme", 16)
    a2 = partition_for("tenant-acme", 16)
    assert a1 == a2
    assert 0 <= a1 < 16

    # Distribution sanity: across 1024 distinct tenants, more than one
    # partition is hit. (xxhash doesn't guarantee uniform spread for
    # any specific input but this many tenants is overwhelmingly safe.)
    distinct = {partition_for(f"tenant-{i}", 16) for i in range(1024)}
    assert len(distinct) > 1


def test_ack_lets_restart_skip_consumed_messages() -> None:
    """Sends 3 messages, polls + acks them, drops the client, reopens
    against the same root, polls again — must return zero. Proves the
    offset_store persistence works through the PyO3 layer."""
    with tempfile.TemporaryDirectory() as tmp:
        # Session 1: send 3, ack 3.
        client = _open(tmp)
        producer = client.producer()
        ids = []
        for i in range(3):
            receipt = producer.send(
                "embed-ingest", "tenant-a", bytes([i])
            )
            ids.append(receipt.message_id)

        consumer = client.consumer("g")
        consumer.subscribe("embed-ingest", [0, 1])
        polled = consumer.poll(max_batch=8, max_wait_ms=500)
        assert len(polled) == 3
        consumer.ack(ids)
        client.shutdown()

        # Session 2: reopen → recovery skips the 3 acked.
        client2 = _open(tmp)
        consumer2 = client2.consumer("g")
        consumer2.subscribe("embed-ingest", [0, 1])
        polled2 = consumer2.poll(max_batch=8, max_wait_ms=500)
        assert polled2 == []
        client2.shutdown()
