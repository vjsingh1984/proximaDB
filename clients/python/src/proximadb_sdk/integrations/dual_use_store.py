"""Dual-Use Embedding integration (TD-051, arXiv:2604.14403).

Killingback / Meshi / Li / Zamani / Karimzadehgan 2026 (Google) propose
a unified model in which the **same representation** serves both
retrieval kNN and as the compressed context fed to the LLM. They
report performance matching traditional RAG using ~1/10 of the LLM
context, with no extra storage cost vs. multi-vector retrieval.

The contribution of *that* paper is the model itself. The contribution
of *this* module is the database-side integration story: a thin
abstraction so callers who plug in a paper-compatible model never
have to hand the database raw text alongside the embedding. The model
is the contract; the SDK just makes the contract cheap to use.

Storage saved: roughly the size of every document's raw text. For
typical RAG corpora that is ~50% of footprint.

Architecture::

    user text  ──►  model.embed(text) ──► vector ──► ProximaDB.insert (NO raw text)
                                                              │
                                          ┌───────────────────┘
                                          ▼
    user query ──► model.embed(query) ──► ProximaDB.search (include_vectors=True)
                                                              │
                                          ┌───────────────────┘
                                          ▼
                            for each result:
                              text = model.decompress(result.vector)

Critical invariant: ``search`` must request vectors. Without them the
model has nothing to decompress, and the dual-use pattern breaks.
:class:`DualUseStore` enforces that on every call.

This module is a Python SDK helper only. It does not require server
changes — the database already supports inserting vectors without
metadata text.
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from typing import Any, List, Optional, Protocol, Sequence, runtime_checkable

from proximadb_sdk.integrations._records import insert_records, record_payload


@runtime_checkable
class DualUseModel(Protocol):
    """Protocol every paper-compatible model must satisfy.

    A model is "dual-use" when:

    1. ``embed(text)`` produces a vector usable as a retrieval key.
    2. ``decompress(embedding)`` recovers (a faithful approximation
       of) the original text from that same vector.

    The Google paper's model is one such implementation. Future
    open-source releases may provide others. ProximaDB has no opinion
    on which model you use — we just need both functions.
    """

    def embed(self, text: str) -> List[float]:
        """Encode `text` into a retrieval-ready vector that is also
        the compressed context."""
        ...

    def decompress(self, embedding: Sequence[float]) -> str:
        """Recover the (approximation of the) original text from
        a vector previously produced by :meth:`embed`."""
        ...


@dataclass
class DualUseRetrievalResult:
    """One result returned by :meth:`DualUseStore.retrieve`.

    Attributes:
        id: Document ID assigned at insert time.
        score: Server-supplied similarity score; higher is better.
        text: Decompressed text recovered from the stored vector.
            Lossy reconstruction depending on the model.
    """

    id: str
    score: float
    text: str


class DualUseStore:
    """Wraps a ProximaDB client + a :class:`DualUseModel`.

    Provides ``add`` / ``add_many`` / ``retrieve`` / ``delete`` against
    a fixed collection, where the database stores only embeddings (no
    raw text) and reconstruction happens client-side via the model's
    ``decompress`` function.

    Args:
        client: A ProximaDB client. The integration uses
            ``insert_records``, ``search``, and ``delete_vectors`` only.
            Any object satisfying that surface (real or stub) works.
        collection_id: Name of the ProximaDB collection.
        model: Anything implementing the :class:`DualUseModel`
            protocol.
    """

    def __init__(
        self,
        client: Any,
        collection_id: str,
        model: DualUseModel,
    ) -> None:
        self._client = client
        self._collection_id = collection_id
        self._model = model

    # ---- writes ---------------------------------------------------

    def add(self, text: str, doc_id: Optional[str] = None) -> str:
        """Embed and store `text` without keeping the raw text.

        Returns the document ID (caller-supplied if given, else a
        freshly-generated UUID4).
        """
        vector = self._model.embed(text)
        assigned_id = doc_id if doc_id is not None else _new_id()
        record = record_payload(record_id=assigned_id, vector=vector)
        insert_records(self._client, self._collection_id, [record])
        return assigned_id

    def add_many(
        self,
        texts: Sequence[str],
        ids: Optional[Sequence[str]] = None,
    ) -> List[str]:
        """Batch variant of :meth:`add`.

        Empty input is a no-op (no client call). Mismatched ``ids``
        length raises ``ValueError``.
        """
        if ids is not None and len(ids) != len(texts):
            raise ValueError(
                f"ids length ({len(ids)}) must match texts length "
                f"({len(texts)})"
            )
        if not texts:
            return []

        records: List[dict[str, Any]] = []
        out_ids: List[str] = []
        for i, text in enumerate(texts):
            vector = self._model.embed(text)
            doc_id = ids[i] if ids is not None else _new_id()
            records.append(record_payload(record_id=doc_id, vector=vector))
            out_ids.append(doc_id)

        insert_records(self._client, self._collection_id, records)
        return out_ids

    # ---- reads ----------------------------------------------------

    def retrieve(
        self, query: str, top_k: int = 10
    ) -> List[DualUseRetrievalResult]:
        """Embed `query`, search, and decompress every returned vector.

        ``include_vectors=True`` is forced on the search call — the
        store cannot reconstruct text without the vector. Results
        whose vectors are missing (e.g. server pruned them) are
        silently skipped rather than producing garbage text.
        """
        query_vector = self._model.embed(query)
        results = self._client.search(
            self._collection_id,
            vector=query_vector,
            top_k=top_k,
            include_vectors=True,
        )

        out: List[DualUseRetrievalResult] = []
        for r in results:
            vector = getattr(r, "vector", None)
            if not vector:
                # No vector -> no decompress possible. Skip rather
                # than yield garbage. Robustness > completeness here.
                continue
            text = self._model.decompress(vector)
            out.append(
                DualUseRetrievalResult(
                    id=r.id,
                    score=r.score,
                    text=text,
                )
            )
        return out

    # ---- deletes --------------------------------------------------

    def delete(self, ids: Sequence[str]) -> None:
        """Delete documents by ID. Empty list is a no-op."""
        if not ids:
            return
        self._client.delete_vectors(self._collection_id, list(ids))


def _new_id() -> str:
    """Generate a fresh document ID. Centralized so tests and
    callers know the format (UUID4 string)."""
    return uuid.uuid4().hex


__all__ = [
    "DualUseModel",
    "DualUseRetrievalResult",
    "DualUseStore",
]
