"""Haystack DocumentStore adapter for ProximaDB.

Provides a ``ProximaDBDocumentStore`` class that implements Haystack's
``DocumentStore`` interface, allowing ProximaDB to be used as a drop-in
document store for RAG pipelines, retrieval-augmented generation, and LLM applications.

Requires: ``pip install proximadb-python[haystack]`` or
            ``pip install haystack-ai-proximadb``

Example::

    from proximadb_sdk.integrations.haystack import ProximaDBDocumentStore
    from proximadb_sdk import ProximaDBClient

    client = ProximaDBClient(url="http://localhost:5678")
    store = ProximaDBDocumentStore(
        client=client,
        collection_name="docs",
        embedding_dim=1536,  # OpenAI embedding dimension
    )

    # Index documents
    from haystack.dataclasses import Document
    docs = [
        Document(content="Hello world", meta={"source": "test"}),
        Document(content="ProximaDB is fast", meta={"source": "docs"}),
    ]
    store.write_documents(docs)

    # Retrieve
    from haystack.components.embedders import OpenAITextEmbedder
    results = store.retrieve_documents(
        embedding_function=OpenAITextEmbedder().embed_queries(["What is ProximaDB?"]),
        top_k=3,
    )
"""

from __future__ import annotations

import asyncio
import uuid
from typing import Any, cast

from haystack import component, default_from_dict, default_to_dict
from haystack.dataclasses import Document
from haystack.document_stores.types import DuplicatePolicy

from proximadb_sdk.integrations._records import insert_records, record_payload
from proximadb_sdk.models import VectorRecord

# Mapping from Haystack 2.x comparison operators to ProximaDB filter operators.
# Equality is emitted as a bare ``{field: value}`` (the format ProximaDB's
# metadata matcher understands directly); the rest use the ``$``-prefixed forms.
_HAYSTACK_OP_TO_PROXIMA = {
    "!=": "$ne",
    ">": "$gt",
    "<": "$lt",
    ">=": "$gte",
    "<=": "$lte",
    "in": "$in",
    "not in": "$nin",
}

# Mapping from Haystack 2.x comparison operators to the typed ops accepted by the
# ProximaDB ``/records/scan`` filter surface (the ``[{field,op,value}]`` form).
# Note the scan surface combines a flat list as a logical AND only and does not
# offer a "not in" op — operators absent here (and any OR/NOT logical nesting)
# cannot be pushed down and are evaluated client-side instead.
_HAYSTACK_OP_TO_SCAN = {
    None: "eq",
    "==": "eq",
    "eq": "eq",
    "!=": "neq",
    ">": "gt",
    ">=": "gte",
    "<": "lt",
    "<=": "lte",
    "in": "in",
}


def _strip_meta_prefix(field: str) -> str:
    """Drop Haystack's ``meta.`` prefix from a metadata field name."""
    return field[len("meta.") :] if field.startswith("meta.") else field


def _haystack_filters_to_scan_filter(
    filters: dict[str, Any] | None,
) -> list[dict[str, Any]] | None:
    """Flatten a Haystack 2.x filter tree into the scan endpoint's typed form.

    Returns a ``[{"field","op","value"}]`` list suitable for the
    ``/records/scan`` ``filter`` (an implicit logical AND that ProximaDB pushes
    into the scan predicate server-side), or ``None`` when nothing can be pushed
    down. Only the AND-combinable comparison subset is lowered: ``OR``/``NOT``
    logical nodes and operators the scan surface lacks (e.g. ``not in``) are
    skipped here and enforced client-side by :func:`_haystack_filter_matches`,
    so the pushed-down predicate is always a *sound* over-approximation (it never
    drops a matching document — it can only return a superset that the
    client-side check then narrows).
    """
    if not filters:
        return None
    conditions: list[dict[str, Any]] = []
    _collect_and_conditions(filters, conditions)
    return conditions or None


def _collect_and_conditions(node: dict[str, Any], out: list[dict[str, Any]]) -> None:
    """Recurse only through AND nodes, collecting pushdown-safe comparisons.

    Anything under an OR/NOT (or an unsupported operator) is intentionally not
    collected: dropping it from the *pushdown* set keeps the server-side result a
    superset of the true match set, which the client-side matcher then filters.
    """
    operator = node.get("operator")
    if "conditions" in node:
        if str(operator).upper() == "AND":
            for child in node["conditions"]:
                if isinstance(child, dict):
                    _collect_and_conditions(child, out)
        # OR / NOT cannot be expressed in the flat scan filter — skip (the
        # client-side matcher enforces them).
        return
    scan_op = _HAYSTACK_OP_TO_SCAN.get(
        operator if operator in _HAYSTACK_OP_TO_SCAN else str(operator)
    )
    if scan_op is None:
        return
    field = _strip_meta_prefix(node.get("field", ""))
    if not field:
        return
    out.append({"field": field, "op": scan_op, "value": node.get("value")})


def _haystack_filter_matches(
    meta: dict[str, Any], filters: dict[str, Any] | None
) -> bool:
    """Evaluate a full Haystack 2.x filter tree against a document's metadata.

    This is the authoritative correctness check applied client-side to every
    scanned record. It supports the full logical tree (``AND``/``OR``/``NOT``)
    and every comparison operator — including the ones the scan endpoint cannot
    push down (``not in``) — so the returned set is exactly the documents the
    caller's filter selects, regardless of what was pushed down server-side.
    """
    if not filters:
        return True
    operator = filters.get("operator")
    if "conditions" in filters:
        children = [c for c in filters["conditions"] if isinstance(c, dict)]
        op = str(operator).upper()
        if op == "OR":
            return any(_haystack_filter_matches(meta, c) for c in children)
        if op == "NOT":
            return not all(_haystack_filter_matches(meta, c) for c in children)
        # Default / AND.
        return all(_haystack_filter_matches(meta, c) for c in children)

    field = _strip_meta_prefix(filters.get("field", ""))
    expected = filters.get("value")
    actual = meta.get(field)
    return _compare(actual, operator, expected)


def _compare(actual: Any, operator: Any, expected: Any) -> bool:
    """Apply a single Haystack comparison operator to a metadata value."""
    if operator in (None, "==", "eq"):
        return actual == expected
    if operator == "!=":
        return actual != expected
    if operator == "in":
        return expected is not None and actual in expected
    if operator == "not in":
        return expected is None or actual not in expected
    if actual is None or expected is None:
        return False
    try:
        if operator == ">":
            return actual > expected
        if operator == ">=":
            return actual >= expected
        if operator == "<":
            return actual < expected
        if operator == "<=":
            return actual <= expected
    except TypeError:
        return False
    # Unknown operator: fall back to equality (mirrors the lenient conversion).
    return actual == expected


def _convert_haystack_filters(filters: dict[str, Any] | None) -> dict[str, Any] | None:
    """Convert a Haystack 2.x filter tree into ProximaDB's filter dict format.

    Supports both the 2.x comparison form
    (``{"field": ..., "operator": ..., "value": ...}``) and the logical form
    (``{"operator": "AND"|"OR"|"NOT", "conditions": [...]}``). The ``meta.``
    prefix Haystack uses for metadata fields is stripped. Returns ``None`` for an
    empty/None filter.
    """
    if not filters:
        return None

    operator = filters.get("operator")

    # Logical node: AND / OR / NOT over nested conditions.
    if "conditions" in filters:
        converted = [
            c
            for c in (_convert_haystack_filters(cond) for cond in filters["conditions"])
            if c
        ]
        if not converted:
            return None
        key = {"AND": "and", "OR": "or", "NOT": "not"}.get(str(operator).upper(), "and")
        return {key: converted}

    # Comparison node: field / operator / value.
    field = filters.get("field", "")
    if field.startswith("meta."):
        field = field[len("meta.") :]
    value = filters.get("value")

    if operator in (None, "==", "eq"):
        return {field: value}
    proxima_op = _HAYSTACK_OP_TO_PROXIMA.get(str(operator))
    if proxima_op is None:
        # Unknown operator: fall back to equality so the predicate is at least
        # applied rather than silently dropped.
        return {field: value}
    return {field: {proxima_op: value}}


class ProximaDBDocumentStore:
    """Haystack DocumentStore backed by ProximaDB.

    This DocumentStore implements Haystack's document storage and retrieval interface,
    using ProximaDB's vector search capabilities for semantic search.

    Args:
        client: An existing ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        embedding_dim: Dimension of embeddings. Required for semantic search.
        text_key: Metadata key used to store the original document text.
            Defaults to ``"content"``.
        namespace: Optional namespace prefix for document IDs.
        max_filter_window: Safety cap on the total number of rows
            :meth:`filter_documents` will accumulate across scan pages before
            stopping. This is a guardrail against unbounded client memory use on
            a huge collection, not an ANN window — ``filter_documents`` performs
            a real cursor-paginated metadata scan, so within this cap it returns
            every matching document.
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        embedding_dim: int,
        *,
        text_key: str = "content",
        namespace: str | None = None,
        max_filter_window: int = 10000,
    ) -> None:
        self._client = client
        self._collection_name = collection_name
        self._embedding_dim = embedding_dim
        self._text_key = text_key
        self._namespace = namespace
        self._max_filter_window = max_filter_window

    @property
    def embedding_dim(self) -> int:
        return self._embedding_dim

    def count_documents(self) -> int:
        """Count the number of documents in the store.

        Returns:
            Number of documents stored.
        """
        # Get collection info to count documents
        collection_info = self._client.get_collection(self._collection_name)
        return collection_info.vector_count if collection_info else 0

    def filter_documents(
        self,
        filters: dict[str, Any] | None = None,
    ) -> list[Document]:
        """Return ALL documents whose metadata matches ``filters``.

        ``filters`` accepts the Haystack 2.x filter syntax (logical ``AND``/
        ``OR``/``NOT`` over comparison nodes). The AND-combinable comparison
        subset is converted to the SDK's vector-free metadata-scan filter and
        pushed down as a server-side predicate; the scan is then paginated via
        cursor until exhausted, so the result is the *complete* matching set —
        not a similarity ranking and not a bounded ANN window.

        The scan endpoint's flat filter cannot express ``OR``/``NOT`` (or a
        ``not in`` comparison), so the full Haystack tree is additionally
        evaluated client-side over every scanned record. That keeps results
        exactly correct while still using server-side pushdown to reduce the
        scanned/egressed set whenever possible. For an unfiltered call
        (``filters is None``) every stored document is returned (up to the
        configurable ``max_filter_window`` total-row safety cap).
        """
        scan_filter = _haystack_filters_to_scan_filter(filters)

        # Real vector-free metadata scan + cursor pagination over the whole
        # matching set (no dummy probe vector, no ANN ordering, no top_k cap):
        # the AND-combinable predicate is pushed down server-side, and the full
        # Haystack tree is enforced client-side for OR/NOT/not-in correctness.
        records = self._client.scan(
            self._collection_name,
            filter=scan_filter,
            max_rows=self._max_filter_window,
            include_vectors=True,
        )
        return [
            self._result_to_document(r)
            for r in records
            if _haystack_filter_matches(dict(r.metadata or {}), filters)
        ]

    def write_documents(
        self,
        documents: list[Document],
        policy: DuplicatePolicy = DuplicatePolicy.FAIL,
    ) -> list[Document]:
        """Index documents for retrieval.

        Args:
            documents: List of Haystack Documents to index.
            policy: Policy for handling duplicate documents.

        Returns:
            List of indexed documents.

        Raises:
            ValueError: If a document without embedding is provided and
                policy is DUPLICATE_POLICY.FAIL.
        """
        records: list[dict[str, Any]] = []
        indexed_docs: list[Document] = []

        for doc in documents:
            doc_id = doc.id or self._generate_id(doc)
            indexed_docs.append(doc.with_id(doc_id))

            # Prepare metadata
            metadata = dict(doc.meta) if doc.meta else {}
            metadata[self._text_key] = doc.content

            # Check for required embedding
            if doc.embedding is None:
                raise ValueError(
                    f"Document {doc_id} has no embedding. "
                    "Please embed documents before writing to ProximaDBDocumentStore."
                )

            if policy == DuplicatePolicy.FAIL and self._record_exists(doc_id):
                raise ValueError(
                    f"Document {doc_id} already exists (policy=DuplicatePolicy.FAIL)"
                )

            records.append(
                record_payload(
                    record_id=doc_id,
                    vector=doc.embedding,
                    text=doc.content,
                    metadata=metadata,
                )
            )

        insert_records(self._client, self._collection_name, records)
        return indexed_docs

    def delete_documents(self, document_ids: list[str]) -> None:
        """Delete documents from the store.

        Args:
            document_ids: List of document IDs to delete.
        """
        self._client.delete_vectors(self._collection_name, ids=document_ids)

    def retrieve_documents(
        self,
        embedding_function: list[float] | list[list[float]],
        top_k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> list[list[Document]]:
        """Retrieve documents using vector similarity search.

        Args:
            embedding_function: Either a single query embedding or a list of query embeddings.
            top_k: Number of documents to retrieve per query.
            filters: Optional metadata filters in Haystack 2.x syntax.

        Returns:
            List of document lists (one list per query).
        """
        metadata_filter = _convert_haystack_filters(filters)

        # Handle both single query and multiple queries
        queries: list[list[float]]
        if isinstance(embedding_function[0], list):
            # Multiple query embeddings
            queries = cast(list[list[float]], embedding_function)
        else:
            # Single query embedding
            queries = [cast(list[float], embedding_function)]

        all_results: list[list[Document]] = []

        for query_embedding in queries:
            search_results = self._client.search(
                self._collection_name,
                vector=query_embedding,
                top_k=top_k,
                metadata_filter=metadata_filter,
            )

            docs = [self._result_to_document(r) for r in search_results]
            all_results.append(docs)

        return all_results

    def _record_exists(self, doc_id: str) -> bool:
        """Return True if a record with ``doc_id`` already exists.

        Uses the SDK's get-by-id endpoint (``get_vector``), which raises when the
        record is absent. Any lookup error is treated as "not present" so that
        writes are not blocked by a transient read failure.
        """
        try:
            record = self._client.get_vector(
                self._collection_name,
                doc_id,
                include_vector=False,
                include_metadata=False,
            )
        except Exception:
            return False
        return record is not None

    def _generate_id(self, document: Document) -> str:
        """Generate a unique document ID.

        Args:
            document: The document to generate an ID for.

        Returns:
            A unique document ID.
        """
        prefix = f"{self._namespace}:" if self._namespace else ""
        return f"{prefix}{uuid.uuid4()}"

    def _result_to_document(self, result: VectorRecord) -> Document:
        """Convert a VectorRecord to a Haystack Document.

        Args:
            result: ProximaDB search result.

        Returns:
            Haystack Document.
        """
        metadata = dict(result.metadata) if result.metadata else {}

        # Extract text content from source or metadata
        text_content: str | None = result.source
        if text_content is None:
            text_content = str(metadata.pop(self._text_key, ""))

        return Document(
            id=result.id,
            content=text_content,
            meta=metadata,
            embedding=result.vector,
        )

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ProximaDBDocumentStore:
        """Deserialize a ProximaDBDocumentStore from a dictionary.

        Args:
            data: Dictionary containing serialized store state.

        Returns:
            Deserialized ProximaDBDocumentStore instance.
        """
        from proximadb_sdk import ProximaDBClient

        # Reconstruct client
        client = ProximaDBClient(
            url=data["client"]["url"],
            api_key=data["client"].get("api_key"),
        )

        return cls(
            client=client,
            collection_name=data["collection_name"],
            embedding_dim=data["embedding_dim"],
            text_key=data.get("text_key", "content"),
            namespace=data.get("namespace"),
            max_filter_window=data.get("max_filter_window", 10000),
        )

    def to_dict(self) -> dict[str, Any]:
        """Serialize the ProximaDBDocumentStore to a dictionary.

        Returns:
            Dictionary containing serialized store state.
        """
        return {
            "type": "ProximaDBDocumentStore",
            "client": {
                "url": self._client.url,
                "api_key": self._client.api_key,
            },
            "collection_name": self._collection_name,
            "embedding_dim": self._embedding_dim,
            "text_key": self._text_key,
            "namespace": self._namespace,
            "max_filter_window": self._max_filter_window,
        }


@component
class ProximaDBRetriever:
    """Haystack 2.x embedding retriever component backed by ProximaDB.

    Registered as a Haystack ``@component`` so it can be wired into a Pipeline
    and connected to an embedder's ``embedding`` output. Its ``run`` method
    accepts a ``query_embedding`` and returns ``{"documents": [...]}``.

    Args:
        document_store: The ProximaDBDocumentStore to query.
        top_k: Number of results to return. Defaults to 10.
        filters: Optional default metadata filters (Haystack 2.x syntax),
            overridable per ``run`` call.
    """

    def __init__(
        self,
        document_store: ProximaDBDocumentStore,
        top_k: int = 10,
        filters: dict[str, Any] | None = None,
    ) -> None:
        self._document_store = document_store
        self._top_k = top_k
        self._filters = filters

    @component.output_types(documents=list[Document])
    def run(
        self,
        query_embedding: list[float],
        top_k: int | None = None,
        filters: dict[str, Any] | None = None,
    ) -> dict[str, list[Document]]:
        """Retrieve documents for a query embedding.

        Args:
            query_embedding: The query embedding vector.
            top_k: Per-call override for the number of results.
            filters: Per-call override for the metadata filters.

        Returns:
            ``{"documents": [...]}`` as expected by Haystack pipelines.
        """
        results = self._document_store.retrieve_documents(
            embedding_function=query_embedding,
            top_k=top_k if top_k is not None else self._top_k,
            filters=filters if filters is not None else self._filters,
        )
        return {"documents": results[0] if results else []}

    @component.output_types(documents=list[Document])
    async def run_async(
        self,
        query_embedding: list[float],
        top_k: int | None = None,
        filters: dict[str, Any] | None = None,
    ) -> dict[str, list[Document]]:
        """Async variant of :meth:`run` for Haystack's ``AsyncPipeline``.

        The ProximaDB SDK client is synchronous, so the blocking retrieval is
        offloaded to a worker thread to keep the event loop responsive.
        """
        return await asyncio.to_thread(
            self.run, query_embedding, top_k=top_k, filters=filters
        )

    def retrieve(self, query_embedding: list[float]) -> list[Document]:
        """Backward-compatible retrieve returning a plain list of documents."""
        return self.run(query_embedding)["documents"]

    def to_dict(self) -> dict[str, Any]:
        """Serialize the component for Haystack pipeline persistence."""
        return default_to_dict(
            self,
            document_store=self._document_store.to_dict(),
            top_k=self._top_k,
            filters=self._filters,
        )

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> ProximaDBRetriever:
        """Deserialize the component, reconstructing its document store."""
        init_params = data.get("init_parameters", {})
        store_data = init_params.get("document_store")
        if store_data is not None:
            init_params["document_store"] = ProximaDBDocumentStore.from_dict(store_data)
        return default_from_dict(cls, data)
