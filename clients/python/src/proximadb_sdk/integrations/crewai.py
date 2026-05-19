"""CrewAI integration for ProximaDB.

Provides a ``ProximaDBSearchTool`` (CrewAI ``BaseTool``) and a
``ProximaDBKnowledgeSource`` for embedding-backed knowledge retrieval.

Requires: ``pip install proximadb-python[crewai]``

Example::

    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.crewai import ProximaDBSearchTool

    client = ProximaDBClient(url="http://localhost:5678")
    tool = ProximaDBSearchTool(
        client=client,
        collection_name="docs",
        embedding_fn=my_embed,
    )
    # Use in a CrewAI agent
    agent = Agent(tools=[tool], ...)
"""

from __future__ import annotations

import uuid
from typing import Any, Callable, Optional, Type

from crewai.tools import BaseTool
from pydantic import BaseModel, Field

from proximadb_sdk.integrations._records import insert_records, record_payload


class _SearchInput(BaseModel):
    """Input schema for ProximaDBSearchTool."""

    query: str = Field(..., description="The search query text")


class ProximaDBSearchTool(BaseTool):
    """CrewAI tool for semantic search against ProximaDB.

    Args:
        client: A ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection to search.
        embedding_fn: Callable that takes a string and returns a list of floats.
        top_k: Number of results to return per search.
        name: Tool name visible to the agent.
        description: Tool description visible to the agent.
    """

    name: str = "proximadb_search"
    description: str = (
        "Search for relevant documents in ProximaDB using semantic similarity. "
        "Input should be a natural language query string."
    )
    args_schema: Type[BaseModel] = _SearchInput

    client: Any = Field(exclude=True)
    collection_name: str = Field(default="documents")
    embedding_fn: Callable[..., list[float]] = Field(exclude=True)
    top_k: int = Field(default=5)

    model_config = {"arbitrary_types_allowed": True}

    def _run(self, query: str) -> str:
        """Execute search and return formatted results."""
        vector = self.embedding_fn(query)
        results = self.client.search(
            self.collection_name, vector=vector, top_k=self.top_k
        )
        if not results:
            return "No relevant documents found."

        parts: list[str] = []
        for i, r in enumerate(results, 1):
            text = r.source or ""
            score = r.score
            parts.append(f"[{i}] (score={score:.3f}) {text}")
        return "\n".join(parts)


class ProximaDBKnowledgeSource:
    """CrewAI-compatible knowledge source backed by ProximaDB.

    Provides ``add`` and ``query`` methods for embedding-backed knowledge
    storage and retrieval.

    Args:
        client: A ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        embedding_fn: Callable that takes a string and returns a list of floats.
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        embedding_fn: Callable[..., list[float]],
    ) -> None:
        self._client = client
        self._collection_name = collection_name
        self._embedding_fn = embedding_fn

    def add(
        self,
        texts: list[str],
        metadatas: Optional[list[dict[str, Any]]] = None,
        ids: Optional[list[str]] = None,
    ) -> list[str]:
        """Embed and insert texts into ProximaDB.

        Returns the list of IDs for the inserted records.
        """
        records: list[dict[str, Any]] = []
        generated_ids: list[str] = []

        for i, text in enumerate(texts):
            doc_id = ids[i] if ids and i < len(ids) else str(uuid.uuid4())
            generated_ids.append(doc_id)
            vector = self._embedding_fn(text)
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

    def query(self, query: str, limit: int = 5) -> list[dict[str, Any]]:
        """Search for documents similar to *query*.

        Returns a list of dicts with ``id``, ``text``, ``score``, and
        ``metadata`` keys.
        """
        vector = self._embedding_fn(query)
        results = self._client.search(self._collection_name, vector=vector, top_k=limit)
        return [
            {
                "id": r.id,
                "text": r.source or "",
                "score": r.score,
                "metadata": dict(r.metadata) if r.metadata else {},
            }
            for r in results
        ]
