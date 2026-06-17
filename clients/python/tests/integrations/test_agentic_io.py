"""Tests for agentic event and mapper helpers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import pytest

from proximadb_sdk.integrations.agentic_io import (
    ProximaEventStore,
    ProximaMapperSession,
)
from proximadb_sdk.models import VectorRecord


@dataclass
class _Hit:
    id: str
    score: float
    metadata: dict[str, Any]


@dataclass
class Symbol:
    id: str
    name: str
    language: str


class FakeAdapter:
    def __init__(self) -> None:
        self.documents: dict[str, dict[str, dict[str, Any]]] = {}
        self.vectors: dict[str, dict[str, VectorRecord]] = {}
        self.edges: list[dict[str, Any]] = []

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

    def insert_vectors(
        self, collection_id: str, records: list[VectorRecord]
    ) -> dict[str, Any]:
        bucket = self.vectors.setdefault(collection_id, {})
        for record in records:
            bucket[record.id or ""] = record
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
            metadata = dict(record.metadata or {})
            if filter and any(metadata.get(k) != v for k, v in filter.items()):
                continue
            hits.append(_Hit(record.id or "", 0.9, metadata))
        return hits[:top_k]

    def create_edge(
        self,
        edge_id: str,
        edge_type: str,
        from_node: str,
        to_node: str,
        properties: dict[str, Any] | None = None,
        graph: str | None = None,
        **_: Any,
    ) -> dict[str, Any]:
        edge = {
            "edge_id": edge_id,
            "edge_type": edge_type,
            "from_node": from_node,
            "to_node": to_node,
            "properties": properties or {},
            "graph": graph,
        }
        self.edges.append(edge)
        return {"success": True, **edge}


def test_event_store_append_replay_snapshot_and_version_conflict() -> None:
    adapter = FakeAdapter()
    store = ProximaEventStore(adapter)

    first = store.append(
        "project-1",
        "SymbolIndexed",
        {"symbol": "main"},
        expected_version=0,
    )
    second = store.append(
        "project-1",
        "SymbolLinked",
        {"from": "main", "to": "helper"},
        expected_version=1,
    )
    other = store.append("project-2", "Started", {}, expected_version=0)
    snapshot = store.snapshot("project-1", {"symbols": 2})

    assert [event.version for event in store.read_stream("project-1")] == [1, 2, 3]
    assert (
        store.read_stream("project-1", after_version=1)[0].event_id == second.event_id
    )
    assert snapshot.event_type == "$snapshot"
    assert [event.event_id for event in store.read_all()] == [
        first.event_id,
        second.event_id,
        other.event_id,
        snapshot.event_id,
    ]

    with pytest.raises(ValueError, match="version conflict"):
        store.append("project-1", "StaleWrite", {}, expected_version=1)


def test_mapper_session_upsert_query_vector_search_link_and_delete() -> None:
    adapter = FakeAdapter()
    session = ProximaMapperSession(adapter)
    session.register(Symbol, collection="symbols", indexed_paths=["$.language"])

    session.upsert(
        Symbol(id="sym-main", name="main", language="rust"),
        vector=[1.0, 0.0],
        source="fn main",
    )
    session.upsert(Symbol(id="sym-helper", name="helper", language="python"))

    loaded = session.get(Symbol, "sym-main", collection="symbols")
    assert loaded == Symbol(id="sym-main", name="main", language="rust")

    rust_symbols = (
        session.query(Symbol, collection="symbols").where(language="rust").all()
    )
    assert rust_symbols == [Symbol(id="sym-main", name="main", language="rust")]

    hits = session.vector_search(Symbol, [1.0, 0.0], collection="symbols")
    assert len(hits) == 1
    assert hits[0].item.name == "main"
    assert hits[0].score == 0.9

    edge = session.link(
        "sym-main",
        "CALLS",
        "sym-helper",
        graph="code",
        properties={"line": 7},
    )
    assert edge["success"] is True
    assert adapter.edges[0]["edge_type"] == "CALLS"

    session.delete(Symbol, "sym-helper", collection="symbols")
    assert session.get(Symbol, "sym-helper", collection="symbols") is None
