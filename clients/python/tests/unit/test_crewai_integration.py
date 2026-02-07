"""Unit tests for the CrewAI integration adapter.

These tests mock the ProximaDB client and embedding function so they can
run without a live server or real embedding model.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

# Skip entire module if crewai is not installed
crewai = pytest.importorskip("crewai")

from proximadb_sdk.integrations.crewai import (  # noqa: E402
    ProximaDBKnowledgeSource,
    ProximaDBSearchTool,
)
from proximadb_sdk.models import SearchResult  # noqa: E402


def _fake_embed(text: str) -> list[float]:
    return [float(len(text))] * 3


@pytest.fixture
def mock_client():
    client = MagicMock()
    client.insert_vectors = MagicMock()
    client.search = MagicMock(return_value=[])
    return client


class TestProximaDBSearchTool:
    def test_run_returns_results(self, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.95, source="hello world", metadata={}),
            SearchResult(id="r2", score=0.80, source="foo bar", metadata={}),
        ]
        tool = ProximaDBSearchTool(
            client=mock_client,
            collection_name="test_coll",
            embedding_fn=_fake_embed,
            top_k=2,
        )
        result = tool._run("test query")
        assert "[1]" in result
        assert "0.950" in result
        assert "hello world" in result
        assert "[2]" in result
        mock_client.search.assert_called_once()

    def test_run_no_results(self, mock_client):
        mock_client.search.return_value = []
        tool = ProximaDBSearchTool(
            client=mock_client,
            collection_name="test_coll",
            embedding_fn=_fake_embed,
        )
        result = tool._run("nothing")
        assert result == "No relevant documents found."

    def test_embedding_fn_called_with_query(self, mock_client):
        calls: list[str] = []

        def tracking_embed(text: str) -> list[float]:
            calls.append(text)
            return [0.1, 0.2, 0.3]

        tool = ProximaDBSearchTool(
            client=mock_client,
            collection_name="c",
            embedding_fn=tracking_embed,
        )
        tool._run("my query")
        assert calls == ["my query"]


class TestProximaDBKnowledgeSource:
    def test_add(self, mock_client):
        ks = ProximaDBKnowledgeSource(
            client=mock_client,
            collection_name="test_coll",
            embedding_fn=_fake_embed,
        )
        ids = ks.add(["hello", "world"], metadatas=[{"k": "v1"}, {"k": "v2"}])
        assert len(ids) == 2
        mock_client.insert_vectors.assert_called_once()
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        assert records[0].source == "hello"
        assert records[0].metadata["k"] == "v1"
        assert records[1].source == "world"

    def test_add_with_custom_ids(self, mock_client):
        ks = ProximaDBKnowledgeSource(
            client=mock_client,
            collection_name="c",
            embedding_fn=_fake_embed,
        )
        ids = ks.add(["text"], ids=["custom_id"])
        assert ids == ["custom_id"]
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        assert records[0].id == "custom_id"

    def test_query(self, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.9, source="result text", metadata={"a": 1}),
        ]
        ks = ProximaDBKnowledgeSource(
            client=mock_client,
            collection_name="c",
            embedding_fn=_fake_embed,
        )
        results = ks.query("test", limit=3)
        assert len(results) == 1
        assert results[0]["id"] == "r1"
        assert results[0]["text"] == "result text"
        assert results[0]["score"] == 0.9
        assert results[0]["metadata"]["a"] == 1
        mock_client.search.assert_called_once()

    def test_query_empty(self, mock_client):
        mock_client.search.return_value = []
        ks = ProximaDBKnowledgeSource(
            client=mock_client,
            collection_name="c",
            embedding_fn=_fake_embed,
        )
        results = ks.query("nothing")
        assert results == []
