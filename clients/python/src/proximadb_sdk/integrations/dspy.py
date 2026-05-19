"""DSPy retrieval model for ProximaDB.

Provides a ``ProximaDBRM`` class that implements DSPy's ``dspy.Retrieve``
interface, allowing ProximaDB to serve as the retrieval backend for DSPy
pipelines.

Requires: ``pip install proximadb-python[dspy]``

Example::

    import dspy
    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.integrations.dspy import ProximaDBRM

    client = ProximaDBClient(url="http://localhost:5678")
    rm = ProximaDBRM(
        client=client,
        collection_name="docs",
        embedding_fn=my_embed,
        k=5,
    )
    dspy.settings.configure(rm=rm)
    result = rm("What is ProximaDB?")
"""

from __future__ import annotations

from typing import Any, Callable, Optional, Union

import dspy


class ProximaDBRM(dspy.Retrieve):
    """DSPy retrieval model backed by ProximaDB.

    Args:
        client: A ``ProximaDBClient`` instance.
        collection_name: Name of the ProximaDB collection.
        embedding_fn: Callable that takes a string and returns a list of floats.
        k: Default number of passages to retrieve.
    """

    def __init__(
        self,
        client: Any,
        collection_name: str,
        embedding_fn: Callable[..., list[float]],
        k: int = 3,
    ) -> None:
        super().__init__(k=k)
        self._client = client
        self._collection_name = collection_name
        self._embedding_fn = embedding_fn

    def forward(
        self,
        query_or_queries: Union[str, list[str]],
        k: Optional[int] = None,
        **kwargs: Any,
    ) -> dspy.Prediction:
        """Retrieve passages for the given query or queries.

        Args:
            query_or_queries: A single query string or a list of queries.
            k: Number of passages to retrieve (overrides default).

        Returns:
            ``dspy.Prediction`` with a ``passages`` field containing the
            retrieved text passages.
        """
        k = k or self.k
        queries = (
            [query_or_queries]
            if isinstance(query_or_queries, str)
            else list(query_or_queries)
        )

        all_passages: list[str] = []
        for query in queries:
            vector = self._embedding_fn(query)
            results = self._client.search(self._collection_name, vector=vector, top_k=k)
            for r in results:
                passage = r.source or ""
                if passage and passage not in all_passages:
                    all_passages.append(passage)

        return dspy.Prediction(passages=all_passages)
