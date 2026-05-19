"""LangChain VectorStore adapter for ProximaDB.

Provides a ``ProximaDBVectorStore`` class that implements LangChain's
``VectorStore`` interface, allowing ProximaDB to be used as a drop-in
vector store for RAG pipelines, agents, and chains.

Requires: ``pip install proximadb-python[langchain]``

Example::

    from langchain_openai import OpenAIEmbeddings
    from proximadb_sdk.integrations.langchain import ProximaDBVectorStore
    from proximadb_sdk import ProximaDBClient

    client = ProximaDBClient(url="http://localhost:5678")
    store = ProximaDBVectorStore(
        client=client,
        collection_name="docs",
        embedding=OpenAIEmbeddings(),
    )
    store.add_texts(["Hello world", "ProximaDB is fast"])
    results = store.similarity_search("fast database", k=3)
"""

from __future__ import annotations

import uuid
from typing import Any, Iterable, Optional

from langchain_core.documents import Document
from langchain_core.embeddings import Embeddings
from langchain_core.vectorstores import VectorStore

from proximadb_sdk.integrations._records import insert_records, record_payload


class ProximaDBVectorStore(VectorStore):
    """LangChain VectorStore backed by ProximaDB.

    Args:
        client: An existing ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        embedding: LangChain ``Embeddings`` implementation for text embedding.
        text_key: Metadata key used to store the original document text.
            Defaults to ``"text"``.
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        embedding: Embeddings,
        *,
        text_key: str = "text",
    ) -> None:
        self._client = client
        self._collection_name = collection_name
        self._embedding = embedding
        self._text_key = text_key

    @property
    def embeddings(self) -> Embeddings:
        return self._embedding

    def add_texts(
        self,
        texts: Iterable[str],
        metadatas: Optional[list[dict[str, Any]]] = None,
        ids: Optional[list[str]] = None,
        **kwargs: Any,
    ) -> list[str]:
        """Embed texts and insert them into ProximaDB.

        Returns the list of IDs for the inserted records.
        """
        texts_list = list(texts)
        vectors = self._embedding.embed_documents(texts_list)
        generated_ids: list[str] = []
        records: list[dict[str, Any]] = []

        for i, (text, vector) in enumerate(zip(texts_list, vectors)):
            doc_id = ids[i] if ids and i < len(ids) else str(uuid.uuid4())
            generated_ids.append(doc_id)

            meta: dict[str, Any] = {}
            if metadatas and i < len(metadatas):
                meta.update(metadatas[i])

            records.append(
                record_payload(
                    record_id=doc_id,
                    vector=vector,
                    text=text,
                    metadata=meta,
                )
            )

        insert_records(self._client, self._collection_name, records)
        return generated_ids

    def delete(self, ids: Optional[list[str]] = None, **kwargs: Any) -> Optional[bool]:
        """Delete vectors by ID."""
        if not ids:
            return False
        self._client.delete_vectors(self._collection_name, ids)
        return True

    def similarity_search(
        self,
        query: str,
        k: int = 4,
        **kwargs: Any,
    ) -> list[Document]:
        """Return documents most similar to the query string."""
        results = self.similarity_search_with_score(query, k=k, **kwargs)
        return [doc for doc, _ in results]

    def similarity_search_with_score(
        self,
        query: str,
        k: int = 4,
        **kwargs: Any,
    ) -> list[tuple[Document, float]]:
        """Return documents most similar to the query string with scores."""
        query_vector = self._embedding.embed_query(query)
        return self.similarity_search_by_vector_with_score(query_vector, k=k, **kwargs)

    def similarity_search_by_vector(
        self,
        embedding: list[float],
        k: int = 4,
        **kwargs: Any,
    ) -> list[Document]:
        """Return documents most similar to the given embedding vector."""
        results = self.similarity_search_by_vector_with_score(embedding, k=k, **kwargs)
        return [doc for doc, _ in results]

    def similarity_search_by_vector_with_score(
        self,
        embedding: list[float],
        k: int = 4,
        **kwargs: Any,
    ) -> list[tuple[Document, float]]:
        """Return documents and scores for the given embedding vector."""
        filter_arg = kwargs.get("filter")
        search_results = self._client.search(
            self._collection_name,
            vector=embedding,
            top_k=k,
            metadata_filter=filter_arg,
        )

        docs_and_scores: list[tuple[Document, float]] = []
        for result in search_results:
            metadata = dict(result.metadata) if result.metadata else {}
            page_content = result.source or metadata.pop(self._text_key, "")
            docs_and_scores.append(
                (Document(page_content=page_content, metadata=metadata), result.score)
            )

        return docs_and_scores

    @classmethod
    def from_texts(
        cls,
        texts: list[str],
        embedding: Embeddings,
        metadatas: Optional[list[dict[str, Any]]] = None,
        *,
        client: Any = None,
        collection_name: str = "langchain",
        **kwargs: Any,
    ) -> ProximaDBVectorStore:
        """Create a ProximaDBVectorStore from a list of texts.

        This is a LangChain convention class method. Requires ``client`` kwarg.
        """
        if client is None:
            raise ValueError(
                "ProximaDBVectorStore.from_texts requires a 'client' keyword argument "
                "(an instance of ProximaDBClient)."
            )
        store = cls(
            client=client,
            collection_name=collection_name,
            embedding=embedding,
            **{k: v for k, v in kwargs.items() if k == "text_key"},
        )
        store.add_texts(texts, metadatas=metadatas)
        return store

    @classmethod
    def from_documents(
        cls,
        documents: list[Document],
        embedding: Embeddings,
        *,
        client: Any = None,
        collection_name: str = "langchain",
        **kwargs: Any,
    ) -> ProximaDBVectorStore:
        """Create a ProximaDBVectorStore from LangChain Documents."""
        texts = [doc.page_content for doc in documents]
        metadatas = [doc.metadata for doc in documents]
        return cls.from_texts(
            texts,
            embedding,
            metadatas=metadatas,
            client=client,
            collection_name=collection_name,
            **kwargs,
        )
