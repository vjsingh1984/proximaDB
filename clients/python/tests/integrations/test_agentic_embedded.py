"""Embedded smoke tests for agentic SDK contracts."""

from __future__ import annotations

import pytest

from proximadb_sdk.adapters.embedded_adapter import EmbeddedProtocolAdapter
from proximadb_sdk.integrations.agentic_io import (
    ProximaEventStore,
)
from proximadb_sdk.integrations.agentic_store import (
    ProximaBaseStore,
    ProximaCheckpointSaver,
)
from proximadb_sdk.integrations.victor_embedded import (
    ProximaDBEmbeddedVictorProvider,
    VictorSymbolRecord,
)

pytest.importorskip("proximadb_embedded")


def _adapter(tmp_path) -> EmbeddedProtocolAdapter:
    return EmbeddedProtocolAdapter(data_dir=str(tmp_path / "embedded_data"))


def test_agentic_store_checkpoint_and_event_use_real_embedded_documents(tmp_path) -> None:
    adapter = _adapter(tmp_path)

    store = ProximaBaseStore(adapter)
    store.put(("tenant", "thread"), "profile", {"role": "planner"})
    assert store.get(("tenant", "thread"), "profile").value == {"role": "planner"}

    saver = ProximaCheckpointSaver(adapter)
    config = {"configurable": {"thread_id": "thread-embedded"}}
    saved_config = saver.put(config, {"id": "checkpoint-1"}, {"source": "test"})
    saver.put_writes(saved_config, [("messages", {"content": "hello"})], "task-1")
    checkpoint = saver.get_tuple(config)
    assert checkpoint is not None
    assert checkpoint.checkpoint["id"] == "checkpoint-1"
    assert checkpoint.pending_writes[0]["channel"] == "messages"

    events = ProximaEventStore(adapter)
    first = events.append("stream-embedded", "Started", {}, expected_version=0)
    second = events.snapshot("stream-embedded", {"done": True})
    assert [event.event_id for event in events.read_stream("stream-embedded")] == [
        first.event_id,
        second.event_id,
    ]


@pytest.mark.asyncio
async def test_victor_embedded_provider_uses_mapper_graph_and_events(tmp_path) -> None:
    adapter = _adapter(tmp_path)
    provider = ProximaDBEmbeddedVictorProvider(adapter, workspace="agentic")

    await provider.upsert_symbol(
        VictorSymbolRecord(
            id="symbol-main",
            name="main",
            kind="Function",
            file_path="app.py",
            language="python",
            line=1,
            content="def main(): pass",
        ),
        vector=[1.0, 0.0, 0.0, 0.0],
    )
    await provider.upsert_symbol(
        VictorSymbolRecord(
            id="symbol-helper",
            name="helper",
            kind="Function",
            file_path="app.py",
            language="python",
            line=4,
            content="def helper(): pass",
        )
    )

    functions = await provider.find_symbols(kind="Function", file_path="app.py")
    assert {symbol.id for symbol in functions} == {"symbol-main", "symbol-helper"}

    linked = await provider.link_symbols(
        "symbol-main",
        "CALLS",
        "symbol-helper",
        properties={"line": 2},
    )
    assert linked["success"] is True

    semantic = await provider.semantic_search([1.0, 0.0, 0.0, 0.0], top_k=1)
    assert semantic[0].id == "symbol-main"

    history = await provider.event_history()
    assert [event["event_type"] for event in history] == [
        "SymbolUpserted",
        "SymbolUpserted",
        "SymbolLinked",
    ]
