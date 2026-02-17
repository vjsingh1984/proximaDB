"""Unit tests for the LangGraph retriever integration.

These tests mock the ProximaDB client and LangChain embeddings so they can
run without a live server or real embedding model.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

# Skip entire module if langchain-core is not installed (LangGraph depends on it)
langchain_core = pytest.importorskip("langchain_core")

from langchain_core.embeddings import Embeddings  # noqa: E402
from langchain_core.tools import BaseTool  # noqa: E402

from proximadb_sdk.integrations.langgraph import create_retriever_tool  # noqa: E402
from proximadb_sdk.models import SearchResult  # noqa: E402


class FakeEmbeddings(Embeddings):
    """Deterministic embeddings for testing."""

    def embed_documents(self, texts: list[str]) -> list[list[float]]:
        return [[float(len(t))] * 3 for t in texts]

    def embed_query(self, text: str) -> list[float]:
        return [float(len(text))] * 3


@pytest.fixture
def mock_client():
    client = MagicMock()
    client.insert_vectors = MagicMock()
    client.search = MagicMock(return_value=[])
    return client


class TestCreateRetrieverTool:
    def test_returns_base_tool(self, mock_client):
        tool = create_retriever_tool(
            client=mock_client,
            collection_name="test_coll",
            embedding=FakeEmbeddings(),
        )
        assert isinstance(tool, BaseTool)

    def test_custom_name_and_description(self, mock_client):
        tool = create_retriever_tool(
            client=mock_client,
            collection_name="docs",
            embedding=FakeEmbeddings(),
            name="my_retriever",
            description="Search my docs.",
        )
        assert tool.name == "my_retriever"
        assert "Search my docs" in tool.description

    def test_tool_invokes_search(self, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.95, source="hello", metadata={}),
        ]
        tool = create_retriever_tool(
            client=mock_client,
            collection_name="test_coll",
            embedding=FakeEmbeddings(),
            k=2,
        )
        # Invoke the tool with a query string
        result = tool.invoke("test query")
        mock_client.search.assert_called_once()
        # Result should contain the document content
        assert "hello" in result
