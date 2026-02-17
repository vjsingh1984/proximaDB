"""Unit tests for the DSPy retrieval model adapter.

These tests mock the ProximaDB client and embedding function so they can
run without a live server or real embedding model.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

# Skip entire module if dspy is not installed
dspy = pytest.importorskip("dspy")

from proximadb_sdk.integrations.dspy import ProximaDBRM  # noqa: E402
from proximadb_sdk.models import SearchResult  # noqa: E402


def _fake_embed(text: str) -> list[float]:
    return [float(len(text))] * 3


@pytest.fixture
def mock_client():
    client = MagicMock()
    client.search = MagicMock(return_value=[])
    return client


@pytest.fixture
def rm(mock_client):
    return ProximaDBRM(
        client=mock_client,
        collection_name="test_coll",
        embedding_fn=_fake_embed,
        k=3,
    )


class TestForwardSingleQuery:
    def test_returns_prediction(self, rm, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.95, source="hello world", metadata={}),
            SearchResult(id="r2", score=0.80, source="foo bar", metadata={}),
        ]
        result = rm.forward("test query")
        assert isinstance(result, dspy.Prediction)
        assert len(result.passages) == 2
        assert "hello world" in result.passages
        assert "foo bar" in result.passages

    def test_empty_results(self, rm, mock_client):
        mock_client.search.return_value = []
        result = rm.forward("nothing")
        assert isinstance(result, dspy.Prediction)
        assert result.passages == []

    def test_custom_k(self, rm, mock_client):
        mock_client.search.return_value = []
        rm.forward("query", k=10)
        call_kwargs = mock_client.search.call_args
        assert call_kwargs.kwargs["top_k"] == 10

    def test_default_k(self, rm, mock_client):
        mock_client.search.return_value = []
        rm.forward("query")
        call_kwargs = mock_client.search.call_args
        assert call_kwargs.kwargs["top_k"] == 3


class TestForwardMultiQuery:
    def test_multiple_queries(self, rm, mock_client):
        mock_client.search.side_effect = [
            [SearchResult(id="r1", score=0.9, source="alpha", metadata={})],
            [SearchResult(id="r2", score=0.8, source="beta", metadata={})],
        ]
        result = rm.forward(["q1", "q2"])
        assert isinstance(result, dspy.Prediction)
        assert "alpha" in result.passages
        assert "beta" in result.passages
        assert mock_client.search.call_count == 2

    def test_deduplicates_passages(self, rm, mock_client):
        mock_client.search.side_effect = [
            [SearchResult(id="r1", score=0.9, source="same text", metadata={})],
            [SearchResult(id="r2", score=0.8, source="same text", metadata={})],
        ]
        result = rm.forward(["q1", "q2"])
        assert result.passages.count("same text") == 1


class TestSourceFallback:
    def test_none_source_gives_empty_string(self, rm, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.9, source=None, metadata={}),
        ]
        result = rm.forward("query")
        # Empty strings are excluded from passages
        assert result.passages == []
