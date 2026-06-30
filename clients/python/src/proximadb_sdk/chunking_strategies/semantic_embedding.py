"""
Embedding-based semantic chunking strategy

This strategy splits text at *semantic* breakpoints discovered from embedding
similarity (à la LlamaIndex's ``SemanticSplitterNodeParser``), as opposed to the
purely structural/regex :class:`~.semantic.SemanticStrategy` (headers, topic
words). Both have value and coexist:

- :class:`~.semantic.SemanticStrategy` — structural boundaries, zero deps, fast.
- :class:`SemanticEmbeddingStrategy` — true semantic boundaries from embeddings.

Algorithm
---------
1. Split the text into sentences (reusing the same splitter as
   :class:`~.sentence.SentenceStrategy` — no new dependency).
2. Embed the sentences in a single batched call via an *injected* provider.
3. For each adjacent pair of sentences, compute the cosine *distance* between
   a window of ``buffer_size`` sentences ending at ``i`` and one starting at
   ``i + 1`` (the buffer adds local context so single odd sentences don't
   trigger spurious splits).
4. Place a breakpoint wherever the distance exceeds the
   ``breakpoint_percentile_threshold`` percentile of the distance distribution.
5. Group sentences between breakpoints into :class:`TextChunk`s, honouring the
   existing ``min_chunk_size`` / ``max_chunk_size`` guardrails.

Provider-injection seam (lazy boundary)
---------------------------------------
The embedding provider is **injected**, never imported at module top level. It
may be either a ``core.BaseEmbeddingProvider``-style object exposing a batch
``embed`` (or ``encode``) method, or a plain ``Callable[[list[str]],
list[list[float]]]``. Importing this module — and ``import proximadb_sdk`` /
``proximadb_sdk.chunking_strategies`` — therefore pulls **no** heavy embedding
deps (sentence-transformers etc.). If the strategy runs without a provider it
raises a clear, actionable error rather than silently falling back.
"""

from typing import Any

import numpy as np

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk
from .sentence import SentenceStrategy

# An injected provider may be a BaseEmbeddingProvider-style object (with an
# ``embed``/``encode`` batch method) OR a plain batch callable. We deliberately
# avoid importing the concrete provider class so the lazy boundary stays intact.
EmbeddingProvider = Any


class SemanticEmbeddingStrategy(ChunkingStrategyInterface):
    """
    Embedding-breakpoint semantic chunking.

    Requires an injected embedding provider on the config
    (``ChunkingConfig.embedding_provider``). See the module docstring for the
    accepted provider shapes and the algorithm.
    """

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        # Reuse the sentence splitter wholesale — same config-driven endings,
        # abbreviation handling, etc. No new dependency, no duplicated regex.
        self._sentence_splitter = SentenceStrategy(config)

    # ------------------------------------------------------------------ #
    # Provider plumbing (lazy — no embeddings imported at module level)
    # ------------------------------------------------------------------ #
    def _embed_sentences(self, sentences: list[str]) -> np.ndarray:
        """
        Embed all sentences in a single batched call via the injected provider.

        Returns an array of shape ``(len(sentences), dim)``. Raises a clear,
        actionable error if no provider was injected.
        """
        provider: EmbeddingProvider | None = getattr(
            self.config, "embedding_provider", None
        )
        if provider is None:
            raise ValueError(
                "SemanticEmbeddingStrategy requires an embedding provider, but "
                "ChunkingConfig.embedding_provider is None. Inject a "
                "proximadb_sdk.embedding_providers.core.BaseEmbeddingProvider "
                "(e.g. via get_provider(...)) or a plain "
                "Callable[[list[str]], list[list[float]]]. The built-in local "
                "providers require the 'embeddings' extra: "
                "pip install 'proximadb[embeddings]'."
            )

        # BATCHED: one call for all sentences, never per-sentence.
        if callable(provider) and not hasattr(provider, "embed"):
            # Plain Callable[[list[str]], list[list[float]]]
            raw = provider(sentences)
        elif hasattr(provider, "embed"):
            # BaseEmbeddingProvider-style (.embed -> np.ndarray | list[list])
            raw = provider.embed(sentences)
        elif hasattr(provider, "encode"):
            # sentence-transformers-style (.encode batch)
            raw = provider.encode(sentences)
        else:
            raise TypeError(
                "embedding_provider must be a Callable[[list[str]], "
                "list[list[float]]] or expose an 'embed'/'encode' batch method; "
                f"got {type(provider).__name__}."
            )

        embeddings = np.asarray(raw, dtype=np.float64)
        if embeddings.ndim != 2 or embeddings.shape[0] != len(sentences):
            raise ValueError(
                "Embedding provider returned an unexpected shape "
                f"{embeddings.shape}; expected ({len(sentences)}, dim)."
            )
        return embeddings

    # ------------------------------------------------------------------ #
    # Similarity / breakpoint math
    # ------------------------------------------------------------------ #
    @staticmethod
    def _cosine_distance(a: np.ndarray, b: np.ndarray) -> float:
        """Cosine distance ``1 - cos_sim`` between two vectors (0..2)."""
        na = float(np.linalg.norm(a))
        nb = float(np.linalg.norm(b))
        if na == 0.0 or nb == 0.0:
            # Degenerate vector — treat as maximally dissimilar so a zero
            # vector never silently glues unrelated content together.
            return 1.0
        return 1.0 - float(np.dot(a, b) / (na * nb))

    def _grouped_distances(self, embeddings: np.ndarray) -> list[float]:
        """
        Distance between consecutive buffered sentence groups.

        For each gap ``i`` (between sentence ``i`` and ``i+1``) we compare the
        mean embedding of the ``buffer_size`` sentences ending at ``i`` against
        the mean of the ``buffer_size`` sentences starting at ``i+1``.
        """
        buffer_size = max(1, int(getattr(self.config, "buffer_size", 1)))
        n = embeddings.shape[0]
        distances: list[float] = []
        for i in range(n - 1):
            left = embeddings[max(0, i - buffer_size + 1) : i + 1]
            right = embeddings[i + 1 : min(n, i + 1 + buffer_size)]
            left_mean = left.mean(axis=0)
            right_mean = right.mean(axis=0)
            distances.append(self._cosine_distance(left_mean, right_mean))
        return distances

    def _breakpoint_indices(self, distances: list[float]) -> set[int]:
        """
        Indices ``i`` after which to break (break between sentence ``i`` and
        ``i+1``) — where the distance exceeds the configured percentile.
        """
        if not distances:
            return set()
        threshold_pct = float(
            getattr(self.config, "breakpoint_percentile_threshold", 95.0)
        )
        threshold = float(np.percentile(distances, threshold_pct))
        # Strictly-greater so a flat (single-topic) distribution where every gap
        # equals the percentile produces NO breakpoints.
        return {i for i, d in enumerate(distances) if d > threshold}

    # ------------------------------------------------------------------ #
    # Chunk assembly
    # ------------------------------------------------------------------ #
    def _emit_chunk(
        self,
        group: list[str],
        start_pos: int,
        source_id: str,
        chunk_index: int,
        base_metadata: dict[str, Any],
    ) -> TextChunk:
        chunk_text = " ".join(group)
        chunk = TextChunk(
            text=chunk_text,
            start_pos=start_pos,
            end_pos=start_pos + len(chunk_text),
            chunk_id=f"{source_id}_chunk_{chunk_index}",
            metadata={
                **base_metadata,
                "chunk_type": "semantic_embedding",
                "sentence_count": len(group),
            },
        )
        self.add_chunk_metadata(chunk, chunk_index, -1, "semantic_embedding")
        return chunk

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks at embedding-derived semantic breakpoints."""
        self.validate_config()

        if not text or not text.strip():
            return []

        base_metadata = base_metadata or {}

        sentences = self._sentence_splitter._split_into_sentences(text)
        if not sentences:
            return []

        # Trivial case: a single sentence is its own chunk; no embedding needed.
        if len(sentences) == 1:
            return [self._emit_chunk(sentences, 0, source_id, 0, base_metadata)]

        embeddings = self._embed_sentences(sentences)
        distances = self._grouped_distances(embeddings)
        breakpoints = self._breakpoint_indices(distances)

        chunks: list[TextChunk] = []
        current: list[str] = []
        current_start = 0
        chunk_index = 0

        def flush() -> None:
            nonlocal current, current_start, chunk_index
            if not current:
                return
            chunk = self._emit_chunk(
                current, current_start, source_id, chunk_index, base_metadata
            )
            chunks.append(chunk)
            chunk_index += 1
            current_start = chunk.end_pos + 1
            current = []

        for i, sentence in enumerate(sentences):
            prospective = (" ".join(current + [sentence])) if current else sentence
            # max_chunk_size guardrail: force a break before we overflow.
            if current and len(prospective) > self.config.max_chunk_size:
                flush()

            current.append(sentence)

            # Semantic breakpoint after sentence i — but respect min_chunk_size
            # so a breakpoint doesn't emit a sub-minimum chunk; in that case we
            # keep accreting until the guardrail is met.
            if i in breakpoints:
                if len(" ".join(current)) >= self.config.min_chunk_size:
                    flush()

        flush()

        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def __repr__(self) -> str:
        return (
            "SemanticEmbeddingStrategy("
            f"buffer_size={getattr(self.config, 'buffer_size', 1)}, "
            "breakpoint_percentile_threshold="
            f"{getattr(self.config, 'breakpoint_percentile_threshold', 95.0)})"
        )
