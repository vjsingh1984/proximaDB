"""AutoGen vector database adapter for ProximaDB.

Provides a ``ProximaDBVectorDB`` class that follows AutoGen's ``VectorDB``
interface conventions, allowing ProximaDB to be used as the vector store
backend for AutoGen RAG agents.

Compatible with both AutoGen 0.2 (pyautogen) and AutoGen 0.4+ (autogen-agentchat).
The adapter defines its own type aliases so it works regardless of which AutoGen
version is installed.

Requires: ``pip install proximadb-python[autogen]``

Example::

    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.autogen import ProximaDBVectorDB

    client = ProximaDBClient(url="http://localhost:5678")
    db = ProximaDBVectorDB(
        client=client,
        embedding_fn=my_embed,
        dimension=768,
    )
    db.create_collection("docs")
    db.insert_docs([{"id": "1", "content": "Hello", "embedding": [0.1]*768}], "docs")
    results = db.retrieve_docs(["What is this?"], "docs", n_results=5)
"""

from __future__ import annotations

import uuid
from typing import Any, Callable, Optional, Union

# Verify that at least one autogen package is available
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

from proximadb_sdk.models import VectorRecord

# Type aliases matching AutoGen's VectorDB conventions
Document = dict[str, Any]
ItemID = Union[str, int]
QueryResults = list[list[tuple[Document, float]]]


class ProximaDBVectorDB:
    """AutoGen-compatible VectorDB backed by ProximaDB.

    Implements the same interface as AutoGen's ``VectorDB`` abstract class
    (``create_collection``, ``insert_docs``, ``retrieve_docs``, etc.) without
    requiring a specific AutoGen version.

    Args:
        client: A ``ProximaDBClient`` instance.
        embedding_fn: Callable that takes a string and returns a list of floats.
            Used when documents provide text but no pre-computed embedding.
        dimension: Vector dimension for new collections.
    """

    def __init__(
        self,
        client: Any,
        embedding_fn: Optional[Callable[..., list[float]]] = None,
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

        records: list[VectorRecord] = []
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
                VectorRecord(
                    id=doc_id,
                    vector=embedding,
                    source=content,
                    metadata=metadata,
                )
            )

        if records:
            self._client.insert_vectors(collection_name, records=records)

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
        ids: Optional[list[ItemID]] = None,
        collection_name: str = "default",
        include: Optional[list[str]] = None,
        **kwargs: Any,
    ) -> list[Document]:
        """Retrieve documents by their IDs.

        Returns stub documents since ProximaDB SDK doesn't expose a
        direct get-by-id endpoint.
        """
        if not ids:
            return []

        docs: list[Document] = []
        for doc_id in ids:
            docs.append(
                {
                    "id": str(doc_id),
                    "content": "",
                    "metadata": {},
                }
            )
        return docs
