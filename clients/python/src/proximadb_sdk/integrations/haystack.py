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

import uuid
from typing import Any, List, Optional, Union

from haystack.dataclasses import Document
from haystack.document_stores import DuplicatePolicy

from proximadb_sdk.models import VectorRecord


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
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        embedding_dim: int,
        *,
        text_key: str = "content",
        namespace: Optional[str] = None,
    ) -> None:
        self._client = client
        self._collection_name = collection_name
        self._embedding_dim = embedding_dim
        self._text_key = text_key
        self._namespace = namespace

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
        filters: Optional[dict[str, Any]] = None,
    ) -> List[Document]:
        """Filter documents based on metadata criteria.

        Args:
            filters: Metadata filters in ProximaDB format.

        Returns:
            List of matching documents.
        """
        # Use a dummy embedding vector to get all documents
        dummy_embedding = [0.0] * self._embedding_dim

        search_results = self._client.search(
            self._collection_name,
            vector=dummy_embedding,
            top_k=10000,  # Large number to get all docs
            metadata_filter=filters,
        )

        return [self._result_to_document(r) for r in search_results]

    def write_documents(
        self,
        documents: List[Document],
        policy: DuplicatePolicy = DuplicatePolicy.FAIL,
    ) -> List[Document]:
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
        records: List[VectorRecord] = []
        indexed_docs: List[Document] = []

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

            if policy == DuplicatePolicy.FAIL:
                # Check if document exists
                existing = self._client.get_vectors(self._collection_name, ids=[doc_id])
                if existing:
                    raise ValueError(
                        f"Document {doc_id} already exists (policy=DuplicatePolicy.FAIL)"
                    )

            records.append(
                VectorRecord(
                    id=doc_id,
                    vector=doc.embedding,
                    source=doc.content,
                    metadata=metadata,
                )
            )

        self._client.insert_vectors(self._collection_name, records=records)
        return indexed_docs

    def delete_documents(self, document_ids: List[str]) -> None:
        """Delete documents from the store.

        Args:
            document_ids: List of document IDs to delete.
        """
        self._client.delete_vectors(self._collection_name, ids=document_ids)

    def retrieve_documents(
        self,
        embedding_function: Union[List[float], List[List[float]]],
        top_k: int = 10,
        filters: Optional[dict[str, Any]] = None,
    ) -> List[List[Document]]:
        """Retrieve documents using vector similarity search.

        Args:
            embedding_function: Either a single query embedding or a list of query embeddings.
            top_k: Number of documents to retrieve per query.
            filters: Optional metadata filters.

        Returns:
            List of document lists (one list per query).
        """
        # Handle both single query and multiple queries
        if isinstance(embedding_function[0], list):
            # Multiple query embeddings
            queries = embedding_function
        else:
            # Single query embedding
            queries = [embedding_function]

        all_results: List[List[Document]] = []

        for query_embedding in queries:
            search_results = self._client.search(
                self._collection_name,
                vector=query_embedding,
                top_k=top_k,
                metadata_filter=filters,
            )

            docs = [self._result_to_document(r) for r in search_results]
            all_results.append(docs)

        return all_results

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
        text_content = result.source
        if text_content is None:
            text_content = metadata.pop(self._text_key, "")

        return Document(
            id=result.id,
            content=text_content,
            meta=metadata,
            embedding=result.vector,
        )

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProximaDBDocumentStore":
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
        }


class ProximaDBRetriever:
    """Haystack Retriever for ProximaDB.

    Args:
        document_store: The ProximaDBDocumentStore to use.
        top_k: Number of results to return. Defaults to 10.
        filters: Optional metadata filters.
    """

    def __init__(
        self,
        document_store: ProximaDBDocumentStore,
        top_k: int = 10,
        filters: Optional[dict[str, Any]] = None,
    ) -> None:
        self._document_store = document_store
        self._top_k = top_k
        self._filters = filters

    def retrieve(
        self,
        query_embedding: List[float],
    ) -> List[Document]:
        """Retrieve documents for a query embedding.

        Args:
            query_embedding: The query embedding vector.

        Returns:
            List of retrieved documents.
        """
        results = self._document_store.retrieve_documents(
            embedding_function=query_embedding,
            top_k=self._top_k,
            filters=self._filters,
        )
        return results[0] if results else []
