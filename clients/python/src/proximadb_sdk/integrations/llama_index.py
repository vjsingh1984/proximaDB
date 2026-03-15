"""LlamaIndex VectorStore adapter for ProximaDB.

Provides a ``ProximaDBVectorStore`` class that implements LlamaIndex's
``VectorStore`` interface, allowing ProximaDB to be used as a drop-in
vector store for RAG pipelines, agents, and LLM applications.

Requires: ``pip install proximadb-python[llama_index]`` or
            ``pip install llama-index-core proximadb-python``

Example::

    from proximadb_sdk.integrations.llama_index import ProximaDBVectorStore
    from proximadb_sdk import ProximaDBClient

    client = ProximaDBClient(url="http://localhost:5678")
    store = ProximaDBVectorStore(
        client=client,
        collection_name="docs",
    )

    # Add documents
    from llama_index.core import Document
    docs = [Document(text="Hello world", metadata={"source": "test"})]
    store.add(docs)

    # Query
    results = store.query("What is ProximaDB?")
"""

from __future__ import annotations

import uuid
from typing import Any, List, Optional

from llama_index.core.base.base_retriever import BaseRetriever
from llama_index.core.schema import Document
from llama_index.core.vector_stores import (
    VectorStore,
    VectorStoreQuery,
    VectorStoreQueryResult,
)
from llama_index.core.vector_stores.types import (
    BasePydanticVectorStore,
    MetadataFilter,
    MetadataFilters,
    Node,
    VectorStoreQueryMode,
)

from proximadb_sdk.models import VectorRecord


class ProximaDBVectorStore(BasePydanticVectorStore):
    """LlamaIndex VectorStore backed by ProximaDB.

    Args:
        client: An existing ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        text_key: Metadata key used to store the original document text.
            Defaults to ``"text"``.
    """

    stores_text: bool = True
    flat_metadata: bool = True

    def __init__(
        self,
        client: Any,
        collection_name: str,
        *,
        text_key: str = "text",
    ) -> None:
        super().__init__()
        self._client = client
        self._collection_name = collection_name
        self._text_key = text_key

    @property
    def client(self) -> Any:
        return self._client

    def add(
        self,
        nodes: List[Node],
    ) -> List[str]:
        """Add nodes to ProximaDB.

        Args:
            nodes: List of nodes with embeddings to add.

        Returns:
            List of node IDs that were added.
        """
        ids: List[str] = []
        records: List[VectorRecord] = []

        for node in nodes:
            doc_id = node.node_id or str(uuid.uuid4())
            ids.append(doc_id)

            # Build metadata from node
            metadata = dict(node.metadata) if node.metadata else {}
            metadata[self._text_key] = node.content

            # Use the embedding from the node if available
            if node.embedding is None:
                raise ValueError(
                    f"Node {doc_id} has no embedding. Please embed before adding."
                )

            records.append(
                VectorRecord(
                    id=doc_id,
                    vector=(
                        node.embedding.tolist()
                        if hasattr(node.embedding, "tolist")
                        else list(node.embedding)
                    ),
                    source=node.content,
                    metadata=metadata,
                )
            )

        self._client.insert_vectors(self._collection_name, records=records)
        return ids

    def delete(self, ref_doc_id: str, **delete_kwargs: Any) -> None:
        """Delete nodes from ProximaDB.

        Args:
            ref_doc_id: The document ID to delete.
            **delete_kwargs: Additional delete arguments (not used).
        """
        self._client.delete_vectors(self._collection_name, ids=[ref_doc_id])

    def query(
        self,
        query: VectorStoreQuery,
        **kwargs: Any,
    ) -> VectorStoreQueryResult:
        """Query ProximaDB for similar nodes.

        Args:
            query: The vector store query.
            **kwargs: Additional query arguments.

        Returns:
            Vector store query result with matching nodes.
        """
        # Convert LlamaIndex query to ProximaDB search
        metadata_filter = None
        if query.filters and query.filters.filters:
            # Convert MetadataFilters to ProximaDB filter format
            metadata_filter = self._convert_filters(query.filters)

        search_results = self._client.search(
            self._collection_name,
            vector=(
                query.query_embedding.tolist()
                if hasattr(query.query_embedding, "tolist")
                else list(query.query_embedding)
            ),
            top_k=query.similarity_top_k,
            metadata_filter=metadata_filter,
        )

        # Convert results back to LlamaIndex format
        nodes = []
        similarities = []
        ids = []

        for result in search_results:
            metadata = dict(result.metadata) if result.metadata else {}
            text_content = result.source or metadata.pop(self._text_key, "")

            nodes.append(
                Node(
                    text=text_content,
                    embedding=None,  # Embeddings not returned by default
                    metadata=metadata,
                    id_=result.id,
                )
            )
            similarities.append(result.score)
            ids.append(result.id)

        return VectorStoreQueryResult(
            nodes=nodes,
            similarities=similarities,
            ids=ids,
        )

    def _convert_filters(self, filters: MetadataFilters) -> Optional[dict[str, Any]]:
        """Convert LlamaIndex MetadataFilters to ProximaDB filter format.

        Args:
            filters: LlamaIndex metadata filters.

        Returns:
            ProximaDB-compatible filter dictionary.
        """
        if not filters.filters:
            return None

        proximadb_filter: dict[str, Any] = {"and": []}

        for f in filters.filters:
            if f.operator == "==" or f.operator == "eq":
                proximadb_filter["and"].append({f.key: f.value})
            elif f.operator == "!=" or f.operator == "ne":
                proximadb_filter["and"].append({f.key: {"$ne": f.value}})
            elif f.operator == ">" or f.operator == "gt":
                proximadb_filter["and"].append({f.key: {"$gt": f.value}})
            elif f.operator == "<" or f.operator == "lt":
                proximadb_filter["and"].append({f.key: {"$lt": f.value}})
            elif f.operator == ">=" or f.operator == "gte":
                proximadb_filter["and"].append({f.key: {"$gte": f.value}})
            elif f.operator == "<=" or f.operator == "lte":
                proximadb_filter["and"].append({f.key: {"$lte": f.value}})
            elif f.operator == "in":
                proximadb_filter["and"].append({f.key: {"$in": f.value}})
            elif f.operator == "nin" or f.operator == "not_in":
                proximadb_filter["and"].append({f.key: {"$nin": f.value}})
            else:
                # Unknown operator, include as-is
                proximadb_filter["and"].append({f.key: f.value})

        return proximadb_filter


class ProximaDBRetriever(BaseRetriever):
    """LlamaIndex Retriever for ProximaDB.

    Args:
        vector_store: The ProximaDBVectorStore to use.
        similarity_top_k: Number of results to return. Defaults to 10.
        filters: Optional metadata filters.
    """

    def __init__(
        self,
        vector_store: ProximaDBVectorStore,
        similarity_top_k: int = 10,
        filters: Optional[MetadataFilters] = None,
    ) -> None:
        self._vector_store = vector_store
        self._similarity_top_k = similarity_top_k
        self._filters = filters

    def _retrieve(self, query_bundle: Any) -> List[Node]:
        """Retrieve nodes for a query bundle.

        Args:
            query_bundle: The query bundle containing the query embedding.

        Returns:
            List of retrieved nodes.
        """
        query = VectorStoreQuery(
            query_embedding=query_bundle.get_embedding(),
            similarity_top_k=self._similarity_top_k,
            filters=self._filters,
            mode=VectorStoreQueryMode.DEFAULT,
        )

        result = self._vector_store.query(query)
        return result.nodes
