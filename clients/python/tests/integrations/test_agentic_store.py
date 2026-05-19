"""Tests for agentic persistence helpers.

These tests pin the SDK contract without requiring LangGraph or a live
ProximaDB server. The fake adapter implements the document/vector subset used
by embedded, REST, and gRPC adapters.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from proximadb_sdk.integrations.agentic_store import (
    ProximaBaseStore,
    ProximaCheckpointSaver,
)

@dataclass
class _Hit:
    id: str
    score: float
    metadata: dict[str, Any]


class FakeAdapter:
    def __init__(self) -> None:
        self.documents: dict[str, dict[str, dict[str, Any]]] = {}
        self.vectors: dict[str, dict[str, dict[str, Any]]] = {}

    def create_document_collection(
        self, name: str, config: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        self.documents.setdefault(name, {})
        return {"success": True}

    def insert_document(
        self, collection_name: str, document: dict[str, Any], id: str | None = None
    ) -> dict[str, Any]:
        doc_id = id or document["id"]
        self.documents.setdefault(collection_name, {})[doc_id] = dict(document)
        return {"success": True, "id": doc_id}

    def get_document(self, collection_name: str, doc_id: str) -> dict[str, Any] | None:
        document = self.documents.get(collection_name, {}).get(doc_id)
        if document is None:
            return None
        return {"id": doc_id, "document": dict(document)}

    def query_documents(
        self,
        collection_name: str,
        filter: dict[str, Any] | None = None,
        limit: int = 100,
        **_: Any,
    ) -> dict[str, Any]:
        docs = []
        for doc_id, document in self.documents.get(collection_name, {}).items():
            if filter and any(document.get(k) != v for k, v in filter.items()):
                continue
            docs.append({"id": doc_id, "document": dict(document)})
        return {"documents": docs[:limit], "count": min(len(docs), limit)}

    def delete_document(self, collection_name: str, doc_id: str) -> dict[str, Any]:
        self.documents.get(collection_name, {}).pop(doc_id, None)
        return {"success": True}

    def create_collection(self, collection_id: str, **_: Any) -> dict[str, Any]:
        self.vectors.setdefault(collection_id, {})
        return {"success": True}

    def insert_records(
        self, collection_id: str, records: list[dict[str, Any]]
    ) -> dict[str, Any]:
        bucket = self.vectors.setdefault(collection_id, {})
        for record in records:
            bucket[record.get("id") or ""] = dict(record)
        return {"success": True, "count": len(records)}

    def search(
        self,
        collection_id: str,
        query_vector: list[float],
        top_k: int = 10,
        filter: dict[str, Any] | None = None,
        **_: Any,
    ) -> list[_Hit]:
        del query_vector
        hits = []
        for record in self.vectors.get(collection_id, {}).values():
            metadata = dict(record.get("props") or {})
            if filter and any(metadata.get(k) != v for k, v in filter.items()):
                continue
            hits.append(_Hit(record.get("id") or "", 1.0, metadata))
        return hits[:top_k]

    def delete_vectors(
        self, collection_id: str, vector_ids: list[str]
    ) -> dict[str, Any]:
        bucket = self.vectors.get(collection_id, {})
        for vector_id in vector_ids:
            bucket.pop(vector_id, None)
        return {"success": True}


def _embed(texts: list[str]) -> list[list[float]]:
    return [[float(len(text)), float(text.count("agent"))] for text in texts]


def test_base_store_put_get_search_and_namespaces() -> None:
    adapter = FakeAdapter()
    store = ProximaBaseStore(adapter, embed=_embed, dims=2)

    store.put(("tenant-a", "user-1"), "profile", {"role": "planner", "score": 4})
    store.put(("tenant-a", "user-1"), "pref", {"role": "coder", "score": 9})
    store.put(("tenant-a", "user-2"), "profile", {"role": "planner"})

    item = store.get(("tenant-a", "user-1"), "profile")
    assert item is not None
    assert item.value == {"role": "planner", "score": 4}

    filtered = store.search(("tenant-a", "user-1"), filter={"role": "coder"})
    assert [item.key for item in filtered] == ["pref"]

    semantic = store.search(("tenant-a", "user-1"), query="agent memory")
    assert {item.key for item in semantic} == {"profile", "pref"}

    assert store.list_namespaces(prefix=("tenant-a",)) == [
        ("tenant-a", "user-1"),
        ("tenant-a", "user-2"),
    ]
    assert store.list_namespaces(suffix=("user-2",), limit=1, offset=0) == [
        ("tenant-a", "user-2")
    ]

    store.delete(("tenant-a", "user-1"), "profile")
    assert store.get(("tenant-a", "user-1"), "profile") is None


def test_checkpoint_saver_put_get_list_writes_and_delete_thread() -> None:
    adapter = FakeAdapter()
    saver = ProximaCheckpointSaver(adapter)
    base_config = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "main"}}

    first_config = saver.put(
        base_config,
        {"id": "cp-1", "state": {"step": 1}},
        {"source": "loop"},
    )
    saver.put_writes(first_config, [("messages", {"text": "hello"})], task_id="task-1")

    second_config = saver.put(
        first_config,
        {"id": "cp-2", "state": {"step": 2}},
        {"source": "loop"},
    )

    latest = saver.get_tuple(base_config)
    assert latest is not None
    assert latest.checkpoint["id"] == "cp-2"

    explicit = saver.get_tuple(first_config)
    assert explicit is not None
    assert explicit.checkpoint["id"] == "cp-1"
    assert explicit.pending_writes[0]["channel"] == "messages"

    assert [item.checkpoint["id"] for item in saver.list(base_config)] == [
        "cp-2",
        "cp-1",
    ]
    assert second_config["configurable"]["checkpoint_id"] == "cp-2"

    saver.delete_thread("thread-1")
    assert saver.get_tuple(base_config) is None
