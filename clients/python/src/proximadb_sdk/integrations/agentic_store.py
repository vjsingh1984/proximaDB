"""Agentic persistence adapters for ProximaDB.

This module provides small, dependency-light contracts that mirror the two
storage surfaces agent frameworks such as LangGraph need:

* a namespaced long-term memory store (`ProximaBaseStore`)
* a thread/checkpoint saver (`ProximaCheckpointSaver`)

The classes intentionally depend on the SDK protocol adapter shape instead of a
specific transport. They work with embedded mode, REST, or gRPC once the adapter
implements the document and vector methods used here.
"""

from __future__ import annotations

import builtins
import json
import time
import uuid
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from typing import Any

from proximadb_sdk.integrations._records import insert_records, record_payload

Namespace = tuple[str, ...]
EmbeddingFn = Callable[[Sequence[str]], list[list[float]]]


@dataclass(frozen=True)
class StoreItem:
    """A namespaced long-term memory item."""

    namespace: Namespace
    key: str
    value: dict[str, Any]
    created_at: float
    updated_at: float
    score: float | None = None


@dataclass(frozen=True)
class CheckpointTuple:
    """A persisted checkpoint and its associated writes."""

    config: dict[str, Any]
    checkpoint: dict[str, Any]
    metadata: dict[str, Any]
    parent_config: dict[str, Any] | None
    pending_writes: list[dict[str, Any]]


class ProximaBaseStore:
    """LangGraph-style BaseStore backed by ProximaDB documents and vectors.

    Documents are the source of truth. Vectors are optional and only populated
    when an embedding function is supplied and the write is indexable.
    """

    def __init__(
        self,
        adapter: Any,
        *,
        collection: str = "__agent_store_items",
        vector_collection: str = "__agent_store_vectors",
        embed: EmbeddingFn | None = None,
        dims: int | None = None,
    ) -> None:
        self.adapter = adapter
        self.collection = collection
        self.vector_collection = vector_collection
        self.embed = embed
        self.dims = dims
        self._setup_done = False

    def setup(self) -> None:
        """Create backing collections if they do not already exist."""
        if self._setup_done:
            return

        try:
            self.adapter.create_document_collection(
                self.collection,
                config={"indexed_paths": ["$.namespace_path", "$.key"]},
            )
        except Exception:
            pass

        if self.embed is not None and self.dims:
            try:
                self.adapter.create_collection(
                    self.vector_collection, dimension=self.dims
                )
            except TypeError:
                self.adapter.create_collection(
                    self.vector_collection,
                    config={"dimension": self.dims},
                )
            except Exception:
                pass

        self._setup_done = True

    def put(
        self,
        namespace: Sequence[str],
        key: str,
        value: dict[str, Any],
        *,
        index: Iterable[str] | bool | None = None,
    ) -> None:
        """Store or update an item under `(namespace, key)`.

        `index=False` disables vector indexing for the item. A list of fields
        indexes only those JSON fields. `None` indexes the full JSON value when
        an embedding function is configured.
        """
        self.setup()
        ns = _namespace(namespace)
        doc_id = _store_doc_id(ns, key)
        existing = self.adapter.get_document(self.collection, doc_id)
        now = time.time()
        created_at = (
            _document_payload(existing).get("created_at", now) if existing else now
        )
        payload = {
            "id": doc_id,
            "namespace": list(ns),
            "namespace_path": _namespace_path(ns),
            "key": key,
            "value": value,
            "created_at": created_at,
            "updated_at": now,
        }

        if existing:
            try:
                self.adapter.delete_document(self.collection, doc_id)
            except Exception:
                pass
        self.adapter.insert_document(self.collection, payload, id=doc_id)

        if self.embed is not None and index is not False:
            text = _text_for_index(value, index)
            if text:
                vector = self.embed([text])[0]
                insert_records(
                    self.adapter,
                    self.vector_collection,
                    [
                        record_payload(
                            record_id=doc_id,
                            vector=vector,
                            text=text,
                            metadata={
                                "namespace_path": _namespace_path(ns),
                                "key": key,
                                "store_collection": self.collection,
                            },
                        )
                    ],
                )

    def get(self, namespace: Sequence[str], key: str) -> StoreItem | None:
        """Retrieve one item by namespace and key."""
        self.setup()
        ns = _namespace(namespace)
        doc = self.adapter.get_document(self.collection, _store_doc_id(ns, key))
        if not doc:
            return None
        return _store_item_from_document(doc)

    def delete(self, namespace: Sequence[str], key: str) -> None:
        """Delete one item."""
        self.setup()
        ns = _namespace(namespace)
        doc_id = _store_doc_id(ns, key)
        self.adapter.delete_document(self.collection, doc_id)
        try:
            self.adapter.delete_vectors(self.vector_collection, [doc_id])
        except Exception:
            pass

    def search(
        self,
        namespace: Sequence[str],
        *,
        query: str | None = None,
        filter: dict[str, Any] | None = None,
        limit: int = 10,
        offset: int = 0,
    ) -> list[StoreItem]:
        """Search items by namespace, optional metadata filter, and query text."""
        self.setup()
        ns = _namespace(namespace)
        namespace_path = _namespace_path(ns)

        if query and self.embed is not None:
            vector = self.embed([query])[0]
            hits = self.adapter.search(
                self.vector_collection,
                query_vector=vector,
                top_k=limit + offset,
                filter={"namespace_path": namespace_path},
                include_metadata=True,
            )
            items: list[StoreItem] = []
            for hit in hits[offset : offset + limit]:
                key = getattr(hit, "metadata", {}).get("key")
                if not key:
                    continue
                item = self.get(ns, key)
                if item and _matches_filter(item.value, filter):
                    items.append(
                        StoreItem(
                            namespace=item.namespace,
                            key=item.key,
                            value=item.value,
                            created_at=item.created_at,
                            updated_at=item.updated_at,
                            score=float(getattr(hit, "score", 0.0)),
                        )
                    )
            return items

        result = self.adapter.query_documents(
            self.collection,
            filter={"namespace_path": namespace_path},
            limit=limit + offset,
        )
        docs = result.get("documents", []) if isinstance(result, dict) else []
        items = [_store_item_from_document(doc) for doc in docs]
        filtered = [
            item for item in items if item and _matches_filter(item.value, filter)
        ]
        filtered.sort(key=lambda item: item.updated_at, reverse=True)
        return filtered[offset : offset + limit]

    def list_namespaces(
        self,
        *,
        prefix: Sequence[str] = (),
        suffix: Sequence[str] = (),
        max_depth: int | None = None,
        limit: int = 1000,
        offset: int = 0,
    ) -> list[Namespace]:
        """List known namespaces, optionally constrained by prefix and depth."""
        self.setup()
        prefix_ns = _namespace(prefix)
        suffix_ns = _namespace(suffix)
        result = self.adapter.query_documents(
            self.collection,
            filter=None,
            limit=max(limit + offset, 1000),
        )
        docs = result.get("documents", []) if isinstance(result, dict) else []
        namespaces = set()
        for doc in docs:
            payload = _document_payload(doc)
            ns = _namespace(payload.get("namespace", []))
            if not ns[: len(prefix_ns)] == prefix_ns:
                continue
            if suffix_ns and not ns[-len(suffix_ns) :] == suffix_ns:
                continue
            if max_depth is not None:
                ns = ns[:max_depth]
            namespaces.add(ns)
        return sorted(namespaces)[offset : offset + limit]


class ProximaCheckpointSaver:
    """LangGraph-style checkpoint saver backed by ProximaDB documents."""

    def __init__(
        self,
        adapter: Any,
        *,
        checkpoint_collection: str = "__agent_checkpoints",
        writes_collection: str = "__agent_checkpoint_writes",
    ) -> None:
        self.adapter = adapter
        self.checkpoint_collection = checkpoint_collection
        self.writes_collection = writes_collection
        self._setup_done = False

    def setup(self) -> None:
        if self._setup_done:
            return
        for collection in (self.checkpoint_collection, self.writes_collection):
            try:
                self.adapter.create_document_collection(
                    collection,
                    config={
                        "indexed_paths": [
                            "$.thread_id",
                            "$.checkpoint_ns",
                            "$.checkpoint_id",
                        ]
                    },
                )
            except Exception:
                pass
        self._setup_done = True

    def put(
        self,
        config: dict[str, Any],
        checkpoint: dict[str, Any],
        metadata: dict[str, Any],
        new_versions: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Persist a checkpoint and return an updated runnable config."""
        self.setup()
        thread_id, checkpoint_ns, parent_id = _checkpoint_keys(config)
        checkpoint_id = str(
            checkpoint.get("id")
            or checkpoint.get("checkpoint_id")
            or metadata.get("checkpoint_id")
            or uuid.uuid4()
        )
        next_config = {
            **config,
            "configurable": {
                **config.get("configurable", {}),
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint_id,
            },
        }
        doc_id = _checkpoint_doc_id(thread_id, checkpoint_ns, checkpoint_id)
        payload = {
            "id": doc_id,
            "thread_id": thread_id,
            "checkpoint_ns": checkpoint_ns,
            "checkpoint_id": checkpoint_id,
            "parent_checkpoint_id": parent_id,
            "config": next_config,
            "parent_config": config if parent_id else None,
            "checkpoint": checkpoint,
            "metadata": metadata,
            "new_versions": new_versions or {},
            "created_at": time.time(),
        }
        existing = self.adapter.get_document(self.checkpoint_collection, doc_id)
        if existing:
            self.adapter.delete_document(self.checkpoint_collection, doc_id)
        self.adapter.insert_document(self.checkpoint_collection, payload, id=doc_id)
        return next_config

    async def aput(
        self,
        config: dict[str, Any],
        checkpoint: dict[str, Any],
        metadata: dict[str, Any],
        new_versions: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Async wrapper for LangGraph async execution paths."""
        return self.put(config, checkpoint, metadata, new_versions)

    def put_writes(
        self,
        config: dict[str, Any],
        writes: Sequence[tuple[str, Any]],
        task_id: str,
        task_path: str = "",
    ) -> None:
        """Persist pending task writes for a checkpoint."""
        self.setup()
        thread_id, checkpoint_ns, checkpoint_id = _checkpoint_keys(config)
        for index, (channel, value) in enumerate(writes):
            doc_id = _write_doc_id(
                thread_id, checkpoint_ns, checkpoint_id, task_id, index
            )
            payload = {
                "id": doc_id,
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint_id,
                "task_id": task_id,
                "task_path": task_path,
                "index": index,
                "channel": channel,
                "value": value,
                "created_at": time.time(),
            }
            existing = self.adapter.get_document(self.writes_collection, doc_id)
            if existing:
                self.adapter.delete_document(self.writes_collection, doc_id)
            self.adapter.insert_document(self.writes_collection, payload, id=doc_id)

    async def aput_writes(
        self,
        config: dict[str, Any],
        writes: Sequence[tuple[str, Any]],
        task_id: str,
        task_path: str = "",
    ) -> None:
        """Async wrapper for LangGraph async execution paths."""
        self.put_writes(config, writes, task_id, task_path)

    def get_tuple(self, config: dict[str, Any]) -> CheckpointTuple | None:
        """Fetch the latest or explicitly requested checkpoint tuple."""
        self.setup()
        thread_id, checkpoint_ns, checkpoint_id = _checkpoint_keys(config)
        docs = self._checkpoint_docs(thread_id, checkpoint_ns)
        if checkpoint_id:
            docs = [
                doc
                for doc in docs
                if _document_payload(doc).get("checkpoint_id") == checkpoint_id
            ]
        if not docs:
            return None
        docs.sort(
            key=lambda doc: _document_payload(doc).get("created_at", 0), reverse=True
        )
        return self._tuple_from_doc(docs[0])

    async def aget_tuple(self, config: dict[str, Any]) -> CheckpointTuple | None:
        """Async wrapper for LangGraph async execution paths."""
        return self.get_tuple(config)

    def list(
        self,
        config: dict[str, Any],
        *,
        before: dict[str, Any] | None = None,
        limit: int | None = None,
    ) -> builtins.list[CheckpointTuple]:
        """List checkpoints for a thread, newest first."""
        self.setup()
        thread_id, checkpoint_ns, _checkpoint_id = _checkpoint_keys(config)
        before_id = None
        if before:
            _, _, before_id = _checkpoint_keys(before)
        docs = self._checkpoint_docs(thread_id, checkpoint_ns)
        docs.sort(
            key=lambda doc: _document_payload(doc).get("created_at", 0), reverse=True
        )
        if before_id:
            docs = [
                doc
                for doc in docs
                if _document_payload(doc).get("checkpoint_id") < before_id
            ]
        if limit is not None:
            docs = docs[:limit]
        return [self._tuple_from_doc(doc) for doc in docs]

    async def alist(
        self,
        config: dict[str, Any],
        *,
        before: dict[str, Any] | None = None,
        limit: int | None = None,
    ) -> builtins.list[CheckpointTuple]:
        """Async wrapper for LangGraph async execution paths."""
        return self.list(config, before=before, limit=limit)

    def delete_thread(self, thread_id: str) -> None:
        """Delete checkpoints and pending writes associated with a thread."""
        self.setup()
        for collection in (self.checkpoint_collection, self.writes_collection):
            result = self.adapter.query_documents(
                collection,
                filter={"thread_id": thread_id},
                limit=100_000,
            )
            for doc in result.get("documents", []):
                payload = _document_payload(doc)
                self.adapter.delete_document(collection, payload["id"])

    def _checkpoint_docs(
        self, thread_id: str, checkpoint_ns: str
    ) -> builtins.list[dict[str, Any]]:
        result = self.adapter.query_documents(
            self.checkpoint_collection,
            filter={"thread_id": thread_id, "checkpoint_ns": checkpoint_ns},
            limit=100_000,
        )
        return result.get("documents", []) if isinstance(result, dict) else []

    def _writes_for(
        self, thread_id: str, checkpoint_ns: str, checkpoint_id: str
    ) -> builtins.list[dict[str, Any]]:
        result = self.adapter.query_documents(
            self.writes_collection,
            filter={
                "thread_id": thread_id,
                "checkpoint_ns": checkpoint_ns,
                "checkpoint_id": checkpoint_id,
            },
            limit=100_000,
        )
        writes = [_document_payload(doc) for doc in result.get("documents", [])]
        writes.sort(key=lambda item: item.get("index", 0))
        return writes

    def _tuple_from_doc(self, doc: dict[str, Any]) -> CheckpointTuple:
        payload = _document_payload(doc)
        return CheckpointTuple(
            config=payload.get("config", {}),
            checkpoint=payload.get("checkpoint", {}),
            metadata=payload.get("metadata", {}),
            parent_config=payload.get("parent_config"),
            pending_writes=self._writes_for(
                payload["thread_id"],
                payload.get("checkpoint_ns", ""),
                payload["checkpoint_id"],
            ),
        )


def _namespace(namespace: Sequence[str]) -> Namespace:
    return tuple(str(part) for part in namespace)


def _namespace_path(namespace: Namespace) -> str:
    return "\x1f".join(namespace)


def _store_doc_id(namespace: Namespace, key: str) -> str:
    return f"{_namespace_path(namespace)}\x1e{key}"


def _document_payload(doc: Any) -> dict[str, Any]:
    if doc is None:
        return {}
    if isinstance(doc, dict):
        if isinstance(doc.get("document"), dict):
            return doc["document"]
        return doc
    return {}


def _store_item_from_document(doc: Any) -> StoreItem | None:
    payload = _document_payload(doc)
    if not payload:
        return None
    return StoreItem(
        namespace=_namespace(payload.get("namespace", [])),
        key=str(payload.get("key", "")),
        value=dict(payload.get("value", {})),
        created_at=float(payload.get("created_at", 0.0)),
        updated_at=float(payload.get("updated_at", 0.0)),
    )


def _text_for_index(value: dict[str, Any], index: Iterable[str] | bool | None) -> str:
    if index is None or index is True:
        return json.dumps(value, sort_keys=True)
    fields = []
    for field in index:
        item = value.get(field)
        if item is not None:
            fields.append(str(item))
    return "\n".join(fields)


def _matches_filter(value: dict[str, Any], filter: dict[str, Any] | None) -> bool:
    if not filter:
        return True
    return all(value.get(key) == expected for key, expected in filter.items())


def _checkpoint_keys(config: dict[str, Any]) -> tuple[str, str, str | None]:
    configurable = config.get("configurable", {})
    thread_id = str(configurable.get("thread_id") or "")
    if not thread_id:
        raise ValueError("checkpoint config requires configurable.thread_id")
    checkpoint_ns = str(configurable.get("checkpoint_ns") or "")
    checkpoint_id = configurable.get("checkpoint_id")
    return thread_id, checkpoint_ns, str(checkpoint_id) if checkpoint_id else None


def _checkpoint_doc_id(thread_id: str, checkpoint_ns: str, checkpoint_id: str) -> str:
    return f"{thread_id}\x1f{checkpoint_ns}\x1f{checkpoint_id}"


def _write_doc_id(
    thread_id: str,
    checkpoint_ns: str,
    checkpoint_id: str | None,
    task_id: str,
    index: int,
) -> str:
    return f"{thread_id}\x1f{checkpoint_ns}\x1f{checkpoint_id or ''}\x1f{task_id}\x1f{index}"
