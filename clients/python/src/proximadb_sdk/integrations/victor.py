"""Victor BaseEmbeddingProvider adapter for ProximaDB.

Provides a ``ProximaDBEmbeddingProvider`` class that implements Victor's
``BaseEmbeddingProvider`` interface, allowing ProximaDB to be used as the
vector store backend for Victor's code embedding and semantic search.

Unlike the raw-HTTP implementation in Victor's own ``proximadb_provider.py``,
this adapter uses the ProximaDB SDK for all operations.

Requires: ``pip install proximadb-python[victor]``

Example::

    from victor.storage.vector_stores.base import EmbeddingConfig
    from proximadb_sdk.integrations.victor import ProximaDBEmbeddingProvider

    config = EmbeddingConfig(
        vector_store="proximadb",
        embedding_model="BAAI/bge-small-en-v1.5",
        extra_config={
            "server_url": "http://localhost:5678",
            "collection_name": "code_embeddings",
            "dimension": 384,
        },
    )
    provider = ProximaDBEmbeddingProvider(config)
    await provider.initialize()
    await provider.index_document("id1", "def hello(): pass", {"file_path": "main.py"})
    results = await provider.search_similar("hello function", limit=5)
"""

from __future__ import annotations

from typing import Any, Optional

from victor.storage.vector_stores.base import (
    BaseEmbeddingProvider,
    EmbeddingConfig,
    EmbeddingSearchResult,
)
from victor.storage.vector_stores.models import (
    BaseEmbeddingModel,
    EmbeddingModelConfig,
    create_embedding_model,
)

from proximadb_sdk.models import VectorRecord
from proximadb_sdk.unified_client import ProximaDBClient


class ProximaDBEmbeddingProvider(BaseEmbeddingProvider):
    """Victor embedding provider backed by the ProximaDB SDK.

    This replaces Victor's built-in ``ProximaDBProvider`` which uses raw HTTP
    calls, delegating all vector operations to the ProximaDB Python SDK.

    Args:
        config: Victor ``EmbeddingConfig``. Relevant ``extra_config`` keys:
            - ``server_url``: ProximaDB server URL (default ``http://localhost:5678``)
            - ``collection_name``: Collection name (default ``code_embeddings``)
            - ``dimension``: Vector dimension (default ``384``)
            - ``batch_size``: Embedding batch size (default ``16``)
    """

    def __init__(self, config: EmbeddingConfig) -> None:
        super().__init__(config)
        self._initialized: bool = False
        server_url: str = config.extra_config.get(
            "server_url", "http://localhost:5678"
        )
        self._client = ProximaDBClient(url=server_url)
        self._collection_name: str = config.extra_config.get(
            "collection_name", "code_embeddings"
        )
        self._dimension: int = config.extra_config.get("dimension", 384)
        self.embedding_model: Optional[BaseEmbeddingModel] = None

    async def initialize(self) -> None:
        """Load the embedding model and ensure the collection exists."""
        if self._initialized:
            return

        model_config = EmbeddingModelConfig(
            embedding_type=self.config.embedding_model_type,
            embedding_model=self.config.embedding_model,
            dimension=self._dimension,
            api_key=self.config.embedding_api_key,
            batch_size=self.config.extra_config.get("batch_size", 16),
        )
        self.embedding_model = create_embedding_model(model_config)
        await self.embedding_model.initialize()

        try:
            self._client.create_collection(
                self._collection_name, dimension=self._dimension
            )
        except Exception:
            # Collection may already exist
            pass

        self._initialized = True

    # ------------------------------------------------------------------
    # Embedding delegation
    # ------------------------------------------------------------------

    async def embed_text(self, text: str) -> list[float]:
        """Generate an embedding vector for *text*."""
        if self.embedding_model is None:
            raise RuntimeError("Embedding model not initialized. Call initialize() first.")
        result: list[float] = await self.embedding_model.embed_text(text)
        return result

    async def embed_batch(self, texts: list[str]) -> list[list[float]]:
        """Generate embedding vectors for a batch of texts."""
        if self.embedding_model is None:
            raise RuntimeError("Embedding model not initialized. Call initialize() first.")
        result: list[list[float]] = await self.embedding_model.embed_batch(texts)
        return result

    # ------------------------------------------------------------------
    # Indexing
    # ------------------------------------------------------------------

    async def index_document(
        self,
        doc_id: str,
        content: str,
        metadata: Optional[dict[str, Any]] = None,
    ) -> None:
        """Embed and insert a single document."""
        vector = await self.embed_text(content)
        record = VectorRecord(
            id=doc_id,
            vector=vector,
            source=content,
            metadata=metadata or {},
        )
        self._client.insert_vectors(self._collection_name, records=[record])

    async def index_documents(self, documents: list[dict[str, Any]]) -> None:
        """Embed and insert multiple documents in batch."""
        if not documents:
            return

        contents = [doc["content"] for doc in documents]
        vectors = await self.embed_batch(contents)

        records = [
            VectorRecord(
                id=doc["id"],
                vector=vec,
                source=doc["content"],
                metadata=doc.get("metadata", {}),
            )
            for doc, vec in zip(documents, vectors)
        ]
        self._client.insert_vectors(self._collection_name, records=records)

    # ------------------------------------------------------------------
    # Search
    # ------------------------------------------------------------------

    async def search_similar(
        self,
        query: str,
        limit: int = 10,
        filter_metadata: Optional[dict[str, Any]] = None,
    ) -> list[EmbeddingSearchResult]:
        """Embed *query* and return the most similar documents."""
        query_vector = await self.embed_text(query)
        search_results = self._client.search(
            self._collection_name,
            vector=query_vector,
            top_k=limit,
            metadata_filter=filter_metadata,
        )

        results: list[EmbeddingSearchResult] = []
        for sr in search_results:
            meta = dict(sr.metadata) if sr.metadata else {}
            results.append(
                EmbeddingSearchResult(
                    file_path=meta.get("file_path", ""),
                    symbol_name=meta.get("symbol_name"),
                    content=sr.source or meta.get("content", ""),
                    score=sr.score,
                    line_number=meta.get("line_number"),
                    metadata={
                        k: v
                        for k, v in meta.items()
                        if k not in {"content", "file_path", "symbol_name", "line_number"}
                    },
                )
            )
        return results

    # ------------------------------------------------------------------
    # Deletion
    # ------------------------------------------------------------------

    async def delete_document(self, doc_id: str) -> None:
        """Delete a single document by ID."""
        self._client.delete_vectors(self._collection_name, [doc_id])

    async def delete_by_file(self, file_path: str) -> int:
        """Delete all documents originating from *file_path*.

        Searches for vectors whose ``file_path`` metadata matches, then
        deletes them.  Returns the number of documents deleted.
        """
        # Use a dummy vector to find documents with matching file_path.
        # We search with a large limit to capture all chunks from that file.
        dummy_vector = [0.0] * self._dimension
        try:
            hits = self._client.search(
                self._collection_name,
                vector=dummy_vector,
                top_k=10_000,
                metadata_filter={"file_path": file_path},
            )
        except Exception:
            return 0

        if not hits:
            return 0

        ids_to_delete = [h.id for h in hits]
        self._client.delete_vectors(self._collection_name, ids_to_delete)
        return len(ids_to_delete)

    async def clear_index(self) -> None:
        """Drop and recreate the collection."""
        try:
            self._client.delete_collection(self._collection_name)
        except Exception:
            pass
        try:
            self._client.create_collection(
                self._collection_name, dimension=self._dimension
            )
        except Exception:
            pass

    # ------------------------------------------------------------------
    # Stats / lifecycle
    # ------------------------------------------------------------------

    async def get_stats(self) -> dict[str, Any]:
        """Return provider statistics."""
        return {
            "provider": "proximadb",
            "engine": "SST",
            "collection_name": self._collection_name,
            "dimension": self._dimension,
            "embedding_model_type": self.config.embedding_model_type,
            "embedding_model": self.config.embedding_model,
            "distance_metric": self.config.distance_metric,
        }

    async def close(self) -> None:
        """Release resources."""
        if self.embedding_model is not None:
            await self.embedding_model.close()
            self.embedding_model = None
        self._initialized = False
