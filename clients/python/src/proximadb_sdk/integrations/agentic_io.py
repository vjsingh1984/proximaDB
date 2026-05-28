"""Agentic IO helpers for ProximaDB.

This module provides two MVP contracts needed by agent runtimes that are not
yet first-class server resources:

* an append-only event store over document collections
* a lightweight mapper/session for dataclasses, Pydantic models, and dicts

Both helpers target the SDK adapter shape, so they can run over embedded mode
today and move to native v2 endpoints later without changing caller code.
"""

from __future__ import annotations

import time
import uuid
from collections.abc import Sequence
from dataclasses import asdict, dataclass, fields, is_dataclass
from typing import Any, Generic, TypeVar

T = TypeVar("T")


@dataclass(frozen=True)
class EventRecord:
    """Append-only domain event."""

    stream_id: str
    version: int
    event_type: str
    data: dict[str, Any]
    metadata: dict[str, Any]
    event_id: str
    global_position: int
    created_at: float


@dataclass(frozen=True)
class MappedSearchResult:
    """Typed mapper result returned from vector search composition."""

    item: Any
    score: float
    metadata: dict[str, Any]


class ProximaEventStore:
    """Document-backed event store with optimistic stream version checks."""

    def __init__(
        self,
        adapter: Any,
        *,
        collection: str = "__agent_events",
    ) -> None:
        self.adapter = adapter
        self.collection = collection
        self._setup_done = False

    def setup(self) -> None:
        if self._setup_done:
            return
        try:
            self.adapter.create_document_collection(
                self.collection,
                config={
                    "indexed_paths": [
                        "$.stream_id",
                        "$.version",
                        "$.global_position",
                        "$.event_type",
                    ]
                },
            )
        except Exception:
            pass
        self._setup_done = True

    def append(
        self,
        stream_id: str,
        event_type: str,
        data: dict[str, Any],
        *,
        metadata: dict[str, Any] | None = None,
        expected_version: int | None = None,
        event_id: str | None = None,
    ) -> EventRecord:
        """Append an event to a stream.

        `expected_version` implements optimistic concurrency. Use `0` for a
        brand-new stream, or the latest known stream version for an update.
        """
        self.setup()
        existing = self.read_stream(stream_id, limit=100_000)
        current_version = existing[-1].version if existing else 0
        if expected_version is not None and expected_version != current_version:
            raise ValueError(
                "event stream version conflict: "
                f"expected {expected_version}, found {current_version}"
            )

        version = current_version + 1
        global_position = self._next_global_position()
        record = EventRecord(
            stream_id=stream_id,
            version=version,
            event_type=event_type,
            data=dict(data),
            metadata=dict(metadata or {}),
            event_id=event_id or str(uuid.uuid4()),
            global_position=global_position,
            created_at=time.time(),
        )
        self.adapter.insert_document(
            self.collection,
            _event_to_document(record),
            id=_event_doc_id(stream_id, version, record.event_id),
        )
        return record

    def read_stream(
        self,
        stream_id: str,
        *,
        after_version: int = 0,
        limit: int = 100,
    ) -> list[EventRecord]:
        """Read events for one stream in version order."""
        self.setup()
        result = self.adapter.query_documents(
            self.collection,
            filter={"stream_id": stream_id},
            limit=max(limit, 1000),
        )
        events = [_event_from_document(doc) for doc in _documents(result)]
        events = [event for event in events if event.version > after_version]
        events.sort(key=lambda event: event.version)
        return events[:limit]

    def read_all(
        self,
        *,
        after_position: int = 0,
        limit: int = 100,
    ) -> list[EventRecord]:
        """Read the global event log in append order."""
        self.setup()
        result = self.adapter.query_documents(
            self.collection,
            filter=None,
            limit=max(limit, 1000),
        )
        events = [_event_from_document(doc) for doc in _documents(result)]
        events = [event for event in events if event.global_position > after_position]
        events.sort(key=lambda event: event.global_position)
        return events[:limit]

    def snapshot(
        self,
        stream_id: str,
        state: dict[str, Any],
        *,
        metadata: dict[str, Any] | None = None,
    ) -> EventRecord:
        """Append a stream snapshot event."""
        latest = self.read_stream(stream_id, limit=100_000)
        return self.append(
            stream_id,
            "$snapshot",
            state,
            metadata=metadata,
            expected_version=latest[-1].version if latest else 0,
        )

    def _next_global_position(self) -> int:
        events = self.read_all(limit=100_000)
        return (events[-1].global_position if events else 0) + 1


class ProximaMapperSession:
    """Small embedded-first mapper over document, vector, and graph APIs."""

    def __init__(self, adapter: Any, *, default_graph: str = "__agent_graph") -> None:
        self.adapter = adapter
        self.default_graph = default_graph
        self._collections: dict[type[Any], str] = {}
        self._id_fields: dict[type[Any], str] = {}

    def register(
        self,
        model_type: type[T],
        *,
        collection: str | None = None,
        id_field: str = "id",
        indexed_paths: Sequence[str] = (),
    ) -> type[T]:
        """Register a model type and ensure its document collection exists."""
        collection_name = collection or _default_collection_name(model_type)
        self._collections[model_type] = collection_name
        self._id_fields[model_type] = id_field
        try:
            self.adapter.create_document_collection(
                collection_name,
                config={"indexed_paths": list(indexed_paths)},
            )
        except Exception:
            pass
        return model_type

    def upsert(
        self,
        item: Any,
        *,
        collection: str | None = None,
        id: str | None = None,
        vector: list[float] | None = None,
        vector_collection: str | None = None,
        source: str | None = None,
    ) -> str:
        """Insert or replace an item, optionally writing a paired vector."""
        model_type = type(item)
        if model_type not in self._collections and collection is None:
            self.register(model_type)
        collection_name = collection or self._collections[model_type]
        id_field = self._id_fields.get(model_type, "id")
        payload = _model_to_dict(item)
        doc_id = id or str(payload.get(id_field) or uuid.uuid4())
        payload[id_field] = doc_id

        existing = self.adapter.get_document(collection_name, doc_id)
        if existing:
            self.adapter.delete_document(collection_name, doc_id)
        self.adapter.insert_document(collection_name, payload, id=doc_id)

        if vector is not None:
            from proximadb_sdk.integrations._records import (
                insert_records,
                record_payload,
            )

            target_vector_collection = (
                vector_collection or f"{collection_name}__vectors"
            )
            try:
                self.adapter.create_collection(
                    target_vector_collection,
                    dimension=len(vector),
                )
            except Exception:
                pass
            insert_records(
                self.adapter,
                target_vector_collection,
                [
                    record_payload(
                        record_id=doc_id,
                        vector=vector,
                        text=source,
                        metadata={"document_collection": collection_name},
                    )
                ],
            )
        return doc_id

    def get(
        self,
        model_type: type[T],
        id: str,
        *,
        collection: str | None = None,
    ) -> T | None:
        """Fetch and rehydrate one mapped item."""
        collection_name = collection or self._collection_for(model_type)
        doc = self.adapter.get_document(collection_name, id)
        if not doc:
            return None
        return _dict_to_model(model_type, _payload(doc))

    def query(
        self, model_type: type[T], *, collection: str | None = None
    ) -> ProximaQuery[T]:
        """Start a typed document query."""
        return ProximaQuery(self, model_type, collection=collection)

    def delete(
        self,
        model_type: type[T],
        id: str,
        *,
        collection: str | None = None,
    ) -> None:
        """Delete one mapped item."""
        collection_name = collection or self._collection_for(model_type)
        self.adapter.delete_document(collection_name, id)

    def vector_search(
        self,
        model_type: type[T],
        vector: list[float],
        *,
        collection: str | None = None,
        vector_collection: str | None = None,
        top_k: int = 10,
        filter: dict[str, Any] | None = None,
    ) -> list[MappedSearchResult]:
        """Search vectors and fetch mapped documents by hit id."""
        collection_name = collection or self._collection_for(model_type)
        target_vector_collection = vector_collection or f"{collection_name}__vectors"
        hits = self.adapter.search(
            target_vector_collection,
            query_vector=vector,
            top_k=top_k,
            filter=filter,
            include_metadata=True,
        )
        results = []
        for hit in hits:
            doc_id = getattr(hit, "id", None)
            if not doc_id:
                continue
            item = self.get(model_type, doc_id, collection=collection_name)
            if item is None:
                continue
            results.append(
                MappedSearchResult(
                    item=item,
                    score=float(getattr(hit, "score", 0.0)),
                    metadata=dict(getattr(hit, "metadata", {}) or {}),
                )
            )
        return results

    def link(
        self,
        src_id: str,
        edge_type: str,
        dst_id: str,
        *,
        graph: str | None = None,
        edge_id: str | None = None,
        properties: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Create a graph relationship between two mapped object IDs."""
        graph_id = graph or self.default_graph
        edge_identifier = edge_id or f"{src_id}:{edge_type}:{dst_id}"
        try:
            return self.adapter.create_edge(
                edge_identifier,
                edge_type,
                from_node=src_id,
                to_node=dst_id,
                properties=properties or {},
                graph=graph_id,
            )
        except TypeError:
            return self.adapter.create_edge(
                edge_id=edge_identifier,
                from_node_id=src_id,
                to_node_id=dst_id,
                edge_type=edge_type,
                properties=properties or {},
                graph_id=graph_id,
            )

    def _collection_for(self, model_type: type[Any]) -> str:
        if model_type not in self._collections:
            self.register(model_type)
        return self._collections[model_type]


class ProximaQuery(Generic[T]):
    """Minimal typed query builder for mapper sessions."""

    def __init__(
        self,
        session: ProximaMapperSession,
        model_type: type[T],
        *,
        collection: str | None = None,
    ) -> None:
        self.session = session
        self.model_type = model_type
        self.collection = collection
        self._filter: dict[str, Any] = {}
        self._limit = 100
        self._offset = 0

    def where(self, **equals: Any) -> ProximaQuery[T]:
        self._filter.update(equals)
        return self

    def limit(self, limit: int) -> ProximaQuery[T]:
        self._limit = limit
        return self

    def offset(self, offset: int) -> ProximaQuery[T]:
        self._offset = offset
        return self

    def all(self) -> list[T]:
        collection_name = self.collection or self.session._collection_for(
            self.model_type
        )
        result = self.session.adapter.query_documents(
            collection_name,
            filter=self._filter or None,
            limit=self._limit + self._offset,
        )
        docs = _documents(result)
        items = [
            _dict_to_model(self.model_type, _payload(doc))
            for doc in docs[self._offset : self._offset + self._limit]
        ]
        return items

    def first(self) -> T | None:
        items = self.limit(1).all()
        return items[0] if items else None


def _event_doc_id(stream_id: str, version: int, event_id: str) -> str:
    return f"{stream_id}\x1f{version:020d}\x1f{event_id}"


def _event_to_document(event: EventRecord) -> dict[str, Any]:
    return {
        "id": _event_doc_id(event.stream_id, event.version, event.event_id),
        "stream_id": event.stream_id,
        "version": event.version,
        "event_type": event.event_type,
        "data": event.data,
        "metadata": event.metadata,
        "event_id": event.event_id,
        "global_position": event.global_position,
        "created_at": event.created_at,
    }


def _event_from_document(doc: Any) -> EventRecord:
    payload = _payload(doc)
    return EventRecord(
        stream_id=str(payload["stream_id"]),
        version=int(payload["version"]),
        event_type=str(payload["event_type"]),
        data=dict(payload.get("data", {})),
        metadata=dict(payload.get("metadata", {})),
        event_id=str(payload["event_id"]),
        global_position=int(payload.get("global_position", 0)),
        created_at=float(payload.get("created_at", 0.0)),
    )


def _documents(result: Any) -> list[Any]:
    if isinstance(result, dict):
        return list(result.get("documents", []))
    return []


def _payload(doc: Any) -> dict[str, Any]:
    if doc is None:
        return {}
    if isinstance(doc, dict):
        if isinstance(doc.get("document"), dict):
            return dict(doc["document"])
        return dict(doc)
    return {}


def _model_to_dict(item: Any) -> dict[str, Any]:
    if isinstance(item, dict):
        return dict(item)
    if is_dataclass(item) and not isinstance(item, type):
        return asdict(item)
    if hasattr(item, "model_dump"):
        return dict(item.model_dump())
    if hasattr(item, "dict"):
        return dict(item.dict())
    raise TypeError(f"unsupported mapped item type: {type(item).__name__}")


def _dict_to_model(model_type: type[T], payload: dict[str, Any]) -> T:
    if model_type is dict:
        return payload  # type: ignore[return-value]
    if is_dataclass(model_type):
        allowed = {field.name for field in fields(model_type)}
        return model_type(**{k: v for k, v in payload.items() if k in allowed})
    if hasattr(model_type, "model_validate"):
        return model_type.model_validate(payload)
    if hasattr(model_type, "parse_obj"):
        return model_type.parse_obj(payload)
    return model_type(**payload)


def _default_collection_name(model_type: type[Any]) -> str:
    return f"{model_type.__name__.lower()}s"
