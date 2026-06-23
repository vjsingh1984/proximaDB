"""
Deterministic unit tests for the embedding-based semantic chunking strategy.

These tests use a STUB embedding provider returning fixed, controlled vectors so
breakpoints are fully deterministic — NO real sentence-transformers model is
loaded. They also assert the lazy import boundary stays intact (importing the
strategy pulls no heavy embedding deps) and that embedding is batched.
"""

import subprocess
import sys

import numpy as np
import pytest

from proximadb_sdk.chunking_strategies.base import ChunkingConfig, ChunkingStrategy
from proximadb_sdk.chunking_strategies.factory import (
    ChunkingStrategyFactory,
    get_chunking_strategy,
)
from proximadb_sdk.chunking_strategies.semantic_embedding import (
    SemanticEmbeddingStrategy,
)

# Two clearly-distinct topic clusters. Sentences 0-2 are topic A ("pets"),
# sentences 3-5 are topic B ("markets"). With these vectors there is exactly
# one large cosine jump — between sentence 2 and 3 — so a percentile threshold
# yields exactly one breakpoint => two chunks.
TWO_TOPIC_TEXT = (
    "The cat sat on the warm mat. My dog loves the green park. "
    "A pet always needs daily care. "
    "Stock prices rose sharply today. The market closed much higher. "
    "Investors felt very optimistic."
)

SINGLE_TOPIC_TEXT = (
    "The cat sat on the warm mat. My dog loves the green park. "
    "A pet always needs daily care. The kitten chased a small ball."
)


def _topic_vector(sentence: str) -> list[float]:
    """Map a sentence to one of two orthogonal clusters by keyword."""
    pet_words = ("cat", "dog", "pet", "kitten")
    if any(w in sentence for w in pet_words):
        return [1.0, 0.0]
    return [0.0, 1.0]


class _StubProvider:
    """BaseEmbeddingProvider-style stub: a single batched `embed` call."""

    def __init__(self):
        self.calls = 0
        self.batch_sizes = []

    def embed(self, texts):
        self.calls += 1
        self.batch_sizes.append(len(texts))
        return np.array([_topic_vector(t) for t in texts], dtype=np.float64)


def _callable_provider_factory():
    """Plain Callable[[list[str]], list[list[float]]] stub with a call counter."""
    state = {"calls": 0}

    def provider(texts):
        state["calls"] += 1
        return [_topic_vector(t) for t in texts]

    return provider, state


def _make(text_strategy_kwargs):
    return get_chunking_strategy(
        ChunkingStrategy.SEMANTIC_EMBEDDING,
        min_chunk_size=1,
        **text_strategy_kwargs,
    )


def test_clean_topic_shift_produces_single_breakpoint():
    provider = _StubProvider()
    strat = _make(
        {"embedding_provider": provider, "breakpoint_percentile_threshold": 80.0}
    )
    chunks = strat.chunk(TWO_TOPIC_TEXT, "doc")

    assert len(chunks) == 2, [c.text for c in chunks]
    # The split lands between the two topics.
    assert "pet" in chunks[0].text
    assert "Stock prices" in chunks[1].text
    for c in chunks:
        assert c.metadata["chunk_type"] == "semantic_embedding"
        assert c.metadata["total_chunks"] == 2
        assert c.metadata["chunking_strategy"] == "semantic_embedding"


def test_single_topic_produces_no_spurious_breaks():
    provider = _StubProvider()
    strat = _make(
        {"embedding_provider": provider, "breakpoint_percentile_threshold": 95.0}
    )
    chunks = strat.chunk(SINGLE_TOPIC_TEXT, "doc")

    # All sentences share one cluster -> all distances are 0 -> no gap exceeds
    # the percentile -> a single chunk.
    assert len(chunks) == 1, [c.text for c in chunks]
    assert chunks[0].metadata["total_chunks"] == 1


def test_empty_and_whitespace_input_returns_no_chunks():
    provider = _StubProvider()
    strat = _make({"embedding_provider": provider})
    assert strat.chunk("", "doc") == []
    assert strat.chunk("   \n  ", "doc") == []
    # No embedding work for empty input.
    assert provider.calls == 0


def test_single_sentence_input_is_one_chunk_without_embedding():
    provider = _StubProvider()
    strat = _make({"embedding_provider": provider})
    chunks = strat.chunk("Just one lonely sentence here.", "doc")
    assert len(chunks) == 1
    assert chunks[0].text == "Just one lonely sentence here."
    # Trivial single-sentence path must not call the embedder.
    assert provider.calls == 0


def test_missing_provider_raises_actionable_error():
    # No embedding_provider injected.
    strat = ChunkingStrategyFactory.create_strategy(
        ChunkingStrategy.SEMANTIC_EMBEDDING,
        ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC_EMBEDDING, min_chunk_size=1),
    )
    with pytest.raises(ValueError) as exc:
        strat.chunk(TWO_TOPIC_TEXT, "doc")
    msg = str(exc.value)
    # Error must name the config field and the embeddings extra.
    assert "embedding_provider" in msg
    assert "proximadb[embeddings]" in msg


def test_embedding_is_batched_single_call():
    provider = _StubProvider()
    strat = _make({"embedding_provider": provider})
    strat.chunk(TWO_TOPIC_TEXT, "doc")
    # Exactly one batched embed call covering all six sentences.
    assert provider.calls == 1
    assert provider.batch_sizes == [6]


def test_accepts_plain_callable_provider():
    provider, state = _callable_provider_factory()
    strat = _make(
        {"embedding_provider": provider, "breakpoint_percentile_threshold": 80.0}
    )
    chunks = strat.chunk(TWO_TOPIC_TEXT, "doc")
    assert len(chunks) == 2
    assert state["calls"] == 1  # batched


def test_max_chunk_size_guardrail_forces_split_within_topic():
    # Single topic (no semantic breakpoint) but a tiny max_chunk_size forces
    # the guardrail to split anyway.
    provider = _StubProvider()
    strat = get_chunking_strategy(
        ChunkingStrategy.SEMANTIC_EMBEDDING,
        # chunk_size must also be small: ChunkingConfig.__post_init__ clamps
        # max_chunk_size up to chunk_size (default 512) otherwise.
        chunk_size=40,
        chunk_overlap=0,
        min_chunk_size=1,
        max_chunk_size=40,
        embedding_provider=provider,
    )
    chunks = strat.chunk(SINGLE_TOPIC_TEXT, "doc")
    assert len(chunks) > 1
    for c in chunks:
        # Guardrail respected (allowing the final single-sentence remainder).
        assert len(c.text) <= 40 or c.metadata["sentence_count"] == 1


def test_lazy_import_boundary_no_heavy_deps():
    # Importing the strategy + running it with a stub provider must NOT pull
    # sentence-transformers. Run in a CLEAN subprocess so sibling tests in this
    # process (which legitimately load real models) can't pollute the check.
    code = (
        "import sys\n"
        "from proximadb_sdk.chunking_strategies.base import ChunkingConfig, ChunkingStrategy\n"
        "from proximadb_sdk.chunking_strategies.factory import get_chunking_strategy\n"
        "assert 'sentence_transformers' not in sys.modules\n"
        "s = get_chunking_strategy(ChunkingStrategy.SEMANTIC_EMBEDDING, min_chunk_size=1,\n"
        "    embedding_provider=lambda xs: [[1.0, 0.0] for _ in xs])\n"
        "s.chunk('One sentence here. Another sentence there.', 'doc')\n"
        "assert 'sentence_transformers' not in sys.modules, 'heavy dep leaked'\n"
        "print('LAZY_OK')\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", code],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    assert "LAZY_OK" in result.stdout


def test_factory_registers_strategy():
    assert "semantic_embedding" in ChunkingStrategyFactory.list_strategies()
    strat = ChunkingStrategyFactory.create_strategy(
        ChunkingStrategy.SEMANTIC_EMBEDDING,
        ChunkingConfig(
            strategy=ChunkingStrategy.SEMANTIC_EMBEDDING,
            embedding_provider=_StubProvider(),
        ),
    )
    assert isinstance(strat, SemanticEmbeddingStrategy)
