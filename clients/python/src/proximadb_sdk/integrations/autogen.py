"""AutoGen integrations for ProximaDB.

This module exposes two adapters, targeting the two AutoGen lines:

``ProximaDBMemory`` (AutoGen 0.4+, *recommended*)
    Implements the ``autogen_core.memory.Memory`` protocol, which is the
    canonical RAG / just-in-time-memory integration point in AutoGen 0.4 (the
    0.2 ``VectorDB`` abstraction was removed in the 0.4 rewrite). Plug it into an
    ``AssistantAgent(memory=[...])`` to back retrieval with ProximaDB.

``ProximaDBVectorDB`` (AutoGen 0.2 ``VectorDB``-shaped, standalone helper)
    A standalone helper that mirrors the *shape* of AutoGen 0.2's ``VectorDB``
    abstract class (``create_collection`` / ``insert_docs`` / ``retrieve_docs`` /
    ``get_docs_by_ids`` / ...). AutoGen 0.4 does NOT consume this interface; it is
    retained for users still on 0.2 (``pyautogen``) and as a thin, framework-free
    convenience wrapper. It does not subclass any AutoGen base class.

Requires: ``pip install proximadb-python[autogen]``

Example (0.4 Memory)::

    from autogen_agentchat.agents import AssistantAgent
    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.autogen import ProximaDBMemory

    client = ProximaDBClient(url="http://localhost:5678")
    memory = ProximaDBMemory(
        client=client,
        collection_name="docs",
        embedding_fn=my_embed,  # str -> list[float]
    )
    agent = AssistantAgent("assistant", model_client=..., memory=[memory])

Example (0.2-shaped standalone helper)::

    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.autogen import ProximaDBVectorDB

    client = ProximaDBClient(url="http://localhost:5678")
    db = ProximaDBVectorDB(client=client, embedding_fn=my_embed, dimension=768)
    db.create_collection("docs")
    db.insert_docs([{"id": "1", "content": "Hello", "embedding": [0.1]*768}], "docs")
    results = db.retrieve_docs(["What is this?"], "docs", n_results=5)
"""

from __future__ import annotations

import uuid
from collections.abc import Callable
from typing import Any, Union

# Verify that at least one autogen package is available. ``ProximaDBMemory``
# additionally requires the 0.4 ``autogen_core`` memory protocol; that import is
# performed lazily inside the class so the 0.2-shaped helper stays usable on
# ``pyautogen`` alone.
try:
    import autogen_agentchat  # noqa: F401

    _AUTOGEN_AVAILABLE = True
except ImportError:
    try:
        import pyautogen  # noqa: F401

        _AUTOGEN_AVAILABLE = True
    except ImportError:
        _AUTOGEN_AVAILABLE = False

if not _AUTOGEN_AVAILABLE:
    raise ImportError(
        "AutoGen is not installed. "
        "Install with: pip install autogen-agentchat or pip install pyautogen"
    )

from proximadb_sdk.integrations._records import insert_records, record_payload

# Type aliases matching the *shape* of AutoGen 0.2's VectorDB conventions.
Document = dict[str, Any]
ItemID = Union[str, int]
QueryResults = list[list[tuple[Document, float]]]


class ProximaDBVectorDB:
    """ProximaDB-backed helper shaped after AutoGen 0.2's ``VectorDB``.

    This mirrors the *method shape* of AutoGen 0.2's ``VectorDB`` abstract class
    (``create_collection`` / ``insert_docs`` / ``retrieve_docs`` /
    ``get_docs_by_ids`` / ...) but does NOT subclass any AutoGen type and is NOT
    consumed by AutoGen 0.4 (which removed ``VectorDB`` in favour of the
    ``Memory`` protocol — see :class:`ProximaDBMemory`). Use it on ``pyautogen``
    (0.2), or as a standalone, framework-free convenience wrapper over the SDK.

    Args:
        client: A ``ProximaDBClient`` instance.
        embedding_fn: Callable that takes a string and returns a list of floats.
            Used when documents provide text but no pre-computed embedding.
        dimension: Vector dimension for new collections.
    """

    def __init__(
        self,
        client: Any,
        embedding_fn: Callable[..., list[float]] | None = None,
        dimension: int = 768,
    ) -> None:
        self._client = client
        self._embedding_fn = embedding_fn
        self._dimension = dimension

    def create_collection(
        self,
        collection_name: str,
        overwrite: bool = False,
        get_or_create: bool = True,
    ) -> Any:
        """Create (or get) a ProximaDB collection.

        Args:
            collection_name: Name of the collection.
            overwrite: If True, drop and recreate an existing collection.
            get_or_create: If True, silently ignore "already exists" errors.
        """
        if overwrite:
            try:
                self._client.delete_collection(collection_name)
            except Exception:
                pass

        try:
            self._client.create_collection(collection_name, dimension=self._dimension)
        except Exception:
            if not get_or_create:
                raise
        return collection_name

    def get_collection(self, collection_name: str) -> Any:
        """Return the collection name (ProximaDB is stateless on the client side)."""
        return collection_name

    def delete_collection(self, collection_name: str) -> Any:
        """Delete a ProximaDB collection."""
        self._client.delete_collection(collection_name)

    def insert_docs(
        self,
        docs: list[Document],
        collection_name: str = "default",
        upsert: bool = False,
        **kwargs: Any,
    ) -> None:
        """Insert documents into ProximaDB.

        Each document is a dict that may contain:
        - ``id``: Document ID (auto-generated if missing).
        - ``content`` or ``text``: The document text.
        - ``embedding``: Pre-computed vector (uses ``embedding_fn`` if absent).
        - ``metadata``: Additional metadata dict.
        """
        if not docs:
            return

        records: list[dict[str, Any]] = []
        for doc in docs:
            doc_id = str(doc.get("id", uuid.uuid4()))
            content = doc.get("content") or doc.get("text", "")
            embedding = doc.get("embedding")

            if embedding is None and self._embedding_fn is not None:
                embedding = self._embedding_fn(content)

            if embedding is None:
                continue

            metadata = dict(doc.get("metadata", {}))
            records.append(
                record_payload(
                    record_id=doc_id,
                    vector=embedding,
                    text=content,
                    metadata=metadata,
                )
            )

        if records:
            insert_records(self._client, collection_name, records)

    def update_docs(
        self,
        docs: list[Document],
        collection_name: str = "default",
        **kwargs: Any,
    ) -> None:
        """Update documents (implemented as delete + insert)."""
        ids = [str(doc["id"]) for doc in docs if "id" in doc]
        if ids:
            try:
                self._client.delete_vectors(collection_name, ids)
            except Exception:
                pass
        self.insert_docs(docs, collection_name, **kwargs)

    def delete_docs(
        self,
        ids: list[ItemID],
        collection_name: str = "default",
        **kwargs: Any,
    ) -> None:
        """Delete documents by ID."""
        if ids:
            self._client.delete_vectors(collection_name, [str(i) for i in ids])

    def retrieve_docs(
        self,
        queries: list[str],
        collection_name: str = "default",
        n_results: int = 10,
        distance_threshold: float = -1,
        **kwargs: Any,
    ) -> QueryResults:
        """Retrieve documents for each query.

        Args:
            queries: List of query strings or pre-computed vectors.
            collection_name: Collection to search.
            n_results: Number of results per query.
            distance_threshold: Minimum score threshold (-1 disables).

        Returns:
            ``QueryResults``: a list (per query) of lists of
            ``(Document, float)`` tuples.
        """
        all_results: QueryResults = []
        for query in queries:
            if isinstance(query, str):
                if self._embedding_fn is None:
                    all_results.append([])
                    continue
                vector = self._embedding_fn(query)
            else:
                vector = list(query)

            search_results = self._client.search(
                collection_name, vector=vector, top_k=n_results
            )

            query_docs: list[tuple[Document, float]] = []
            for r in search_results:
                if distance_threshold >= 0 and r.score < distance_threshold:
                    continue
                doc: Document = {
                    "id": r.id,
                    "content": r.source or "",
                    "metadata": dict(r.metadata) if r.metadata else {},
                }
                query_docs.append((doc, r.score))
            all_results.append(query_docs)

        return all_results

    def get_docs_by_ids(
        self,
        ids: list[ItemID] | None = None,
        collection_name: str = "default",
        include: list[str] | None = None,
        **kwargs: Any,
    ) -> list[Document]:
        """Retrieve documents by their IDs via the SDK's get-by-id endpoint.

        Uses ``client.get_vector`` per id (the SDK does expose a direct get-by-id
        API). IDs that are missing or fail to resolve are skipped rather than
        returned as empty placeholders, so callers never receive silently
        content-less documents.

        Args:
            ids: Document IDs to fetch. ``None`` or empty returns ``[]``.
            collection_name: Collection to read from.
            include: Optional ``["content", "metadata", "embedding"]`` selector;
                when omitted all available fields are returned.

        Returns:
            The resolved documents (a subset of ``ids`` if some were not found).
        """
        if not ids:
            return []

        want = set(include) if include else None
        include_embedding = want is None or "embedding" in want

        docs: list[Document] = []
        for doc_id in ids:
            try:
                record = self._client.get_vector(
                    collection_name,
                    str(doc_id),
                    include_vector=include_embedding,
                    include_metadata=True,
                )
            except Exception:
                # Missing id or backend lookup error -> skip (no empty stub).
                continue
            if record is None:
                continue

            metadata = dict(getattr(record, "metadata", {}) or {})
            doc: Document = {
                "id": getattr(record, "id", str(doc_id)),
                "content": getattr(record, "source", None) or "",
                "metadata": metadata,
            }
            if include_embedding:
                embedding = getattr(record, "vector", None)
                if embedding is not None:
                    doc["embedding"] = list(embedding)
            docs.append(doc)
        return docs


# The 0.4 Memory protocol lives in autogen_core. Import lazily at *module* load
# but tolerate its absence so the 0.2-only (pyautogen) install can still import
# this module and use ProximaDBVectorDB. ProximaDBMemory is only defined when the
# 0.4 protocol is available; otherwise it is a stub that raises on instantiation.
try:
    from autogen_core.memory import Memory as _Memory
    from autogen_core.memory import MemoryContent as _MemoryContent
    from autogen_core.memory import MemoryMimeType as _MemoryMimeType
    from autogen_core.memory import MemoryQueryResult as _MemoryQueryResult
    from autogen_core.memory import UpdateContextResult as _UpdateContextResult

    _MEMORY_AVAILABLE = True
    _MemoryBase: type = _Memory
except ImportError:  # pragma: no cover - 0.2-only installs
    _MEMORY_AVAILABLE = False
    _MemoryBase = object  # so the class below can still be defined / imported


class ProximaDBMemory(_MemoryBase):
    """AutoGen 0.4 ``Memory`` implementation backed by ProximaDB.

    This is the idiomatic AutoGen 0.4 RAG integration point: attach it to an
    ``AssistantAgent(memory=[...])`` and the agent will query ProximaDB and inject
    the retrieved snippets into its model context on each turn.

    Args:
        client: A ``ProximaDBClient`` instance.
        collection_name: Collection to read/write.
        embedding_fn: Callable mapping a string to a ``list[float]`` embedding.
            Required because the SDK search API is vector-based and the ``Memory``
            protocol's ``query``/``add`` operate on text.
        top_k: Number of memories to retrieve per query. Defaults to 5.
        text_key: Metadata key used to persist the original text. Defaults to
            ``"text"``.
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        *,
        embedding_fn: Callable[[str], list[float]],
        top_k: int = 5,
        text_key: str = "text",
    ) -> None:
        if not _MEMORY_AVAILABLE:
            raise ImportError(
                "ProximaDBMemory requires AutoGen 0.4+ (the autogen_core.memory "
                "protocol). Install with: pip install autogen-agentchat"
            )
        self._client = client
        self._collection_name = collection_name
        self._embedding_fn = embedding_fn
        self._top_k = top_k
        self._text_key = text_key

    async def add(self, content: Any, cancellation_token: Any = None) -> None:
        """Embed and store a ``MemoryContent`` entry in ProximaDB."""
        text = self._content_to_text(content)
        if not text:
            return
        metadata = dict(getattr(content, "metadata", None) or {})
        record = record_payload(
            record_id=str(uuid.uuid4()),
            vector=self._embedding_fn(text),
            text=text,
            metadata=metadata,
        )
        insert_records(self._client, self._collection_name, [record])

    async def query(
        self,
        query: Any,
        cancellation_token: Any = None,
        **kwargs: Any,
    ) -> Any:
        """Retrieve the most relevant stored memories for ``query``.

        Returns a ``MemoryQueryResult`` of ``MemoryContent`` entries.
        """
        text = self._content_to_text(query)
        if not text:
            return _MemoryQueryResult(results=[])

        top_k = int(kwargs.get("top_k", self._top_k))
        search_results = self._client.search(
            self._collection_name,
            vector=self._embedding_fn(text),
            top_k=top_k,
        )

        results = []
        for r in search_results:
            metadata = dict(getattr(r, "metadata", None) or {})
            content_text = getattr(r, "source", None) or metadata.pop(
                self._text_key, ""
            )
            metadata.setdefault("score", getattr(r, "score", None))
            metadata.setdefault("id", getattr(r, "id", None))
            results.append(
                _MemoryContent(
                    content=content_text,
                    mime_type=_MemoryMimeType.TEXT,
                    metadata=metadata,
                )
            )
        return _MemoryQueryResult(results=results)

    async def update_context(self, model_context: Any) -> Any:
        """Inject retrieved memories into the agent's ``model_context``.

        Queries using the most recent message text and, if any memories are
        found, appends a ``SystemMessage`` summarising them — mirroring the
        behaviour of AutoGen's built-in memory implementations.
        """
        messages = await model_context.get_messages()
        if not messages:
            return _UpdateContextResult(memories=_MemoryQueryResult(results=[]))

        last_text = self._content_to_text(getattr(messages[-1], "content", ""))
        query_result = await self.query(last_text)

        if query_result.results:
            from autogen_core.models import SystemMessage

            snippets = "\n".join(
                f"{i}. {str(m.content)}" for i, m in enumerate(query_result.results, 1)
            )
            await model_context.add_message(
                SystemMessage(
                    content=(
                        "Relevant memory content (in order of relevance):\n"
                        f"{snippets}"
                    )
                )
            )
        return _UpdateContextResult(memories=query_result)

    async def clear(self) -> None:
        """Clear all entries by dropping and recreating the collection."""
        try:
            self._client.delete_collection(self._collection_name)
        except Exception:
            pass

    async def close(self) -> None:
        """No persistent resources to release (the client is owned by the caller)."""
        return None

    @staticmethod
    def _content_to_text(content: Any) -> str:
        """Coerce a ``MemoryContent``/str/other into plain query text."""
        if isinstance(content, str):
            return content
        inner = getattr(content, "content", content)
        return inner if isinstance(inner, str) else str(inner)
