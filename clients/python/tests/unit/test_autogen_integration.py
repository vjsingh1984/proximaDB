"""Unit tests for the AutoGen VectorDB adapter.

These tests mock the ProximaDB client and embedding function so they can
run without a live server or real embedding model.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

# Skip entire module if autogen is not installed
pytest.importorskip("autogen_agentchat")

from proximadb_sdk.integrations.autogen import ProximaDBVectorDB  # noqa: E402
from proximadb_sdk.models import SearchResult  # noqa: E402


def _fake_embed(text: str) -> list[float]:
    return [float(len(text))] * 3


@pytest.fixture
def mock_client():
    client = MagicMock()
    client.create_collection = MagicMock()
    client.delete_collection = MagicMock()
    client.insert_vectors = MagicMock()
    client.delete_vectors = MagicMock()
    client.search = MagicMock(return_value=[])
    return client


@pytest.fixture
def db(mock_client):
    return ProximaDBVectorDB(
        client=mock_client,
        embedding_fn=_fake_embed,
        dimension=3,
    )


class TestCreateCollection:
    def test_create_collection(self, db, mock_client):
        result = db.create_collection("test_coll")
        assert result == "test_coll"
        mock_client.create_collection.assert_called_once_with(
            "test_coll", dimension=3
        )

    def test_create_collection_overwrite(self, db, mock_client):
        db.create_collection("test_coll", overwrite=True)
        mock_client.delete_collection.assert_called_once_with("test_coll")
        mock_client.create_collection.assert_called_once()

    def test_get_or_create_swallows_existing(self, db, mock_client):
        mock_client.create_collection.side_effect = Exception("exists")
        # Should not raise because get_or_create=True by default
        result = db.create_collection("test_coll")
        assert result == "test_coll"

    def test_get_or_create_false_raises(self, db, mock_client):
        mock_client.create_collection.side_effect = Exception("exists")
        with pytest.raises(Exception, match="exists"):
            db.create_collection("test_coll", get_or_create=False)


class TestDeleteCollection:
    def test_delete_collection(self, db, mock_client):
        db.delete_collection("test_coll")
        mock_client.delete_collection.assert_called_once_with("test_coll")


class TestInsertDocs:
    def test_insert_with_embeddings(self, db, mock_client):
        docs = [
            {"id": "d1", "content": "hello", "embedding": [1.0, 2.0, 3.0]},
            {"id": "d2", "content": "world", "embedding": [4.0, 5.0, 6.0]},
        ]
        db.insert_docs(docs, "test_coll")
        mock_client.insert_vectors.assert_called_once()
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        assert len(records) == 2
        assert records[0].id == "d1"
        assert records[0].source == "hello"
        assert records[0].vector == [1.0, 2.0, 3.0]

    def test_insert_with_embedding_fn(self, db, mock_client):
        docs = [{"id": "d1", "content": "hello"}]
        db.insert_docs(docs, "test_coll")
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        # embedding_fn returns [float(len("hello"))] * 3 = [5.0, 5.0, 5.0]
        assert records[0].vector == [5.0, 5.0, 5.0]

    def test_insert_empty_list(self, db, mock_client):
        db.insert_docs([], "test_coll")
        mock_client.insert_vectors.assert_not_called()

    def test_insert_uses_text_key(self, db, mock_client):
        docs = [{"id": "d1", "text": "from text key", "embedding": [1.0, 2.0, 3.0]}]
        db.insert_docs(docs, "test_coll")
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        assert records[0].source == "from text key"

    def test_insert_with_metadata(self, db, mock_client):
        docs = [
            {
                "id": "d1",
                "content": "hi",
                "embedding": [1.0, 2.0, 3.0],
                "metadata": {"key": "val"},
            }
        ]
        db.insert_docs(docs, "test_coll")
        records = mock_client.insert_vectors.call_args.kwargs["records"]
        assert records[0].metadata["key"] == "val"


class TestDeleteDocs:
    def test_delete_docs(self, db, mock_client):
        db.delete_docs(["id1", "id2"], "test_coll")
        mock_client.delete_vectors.assert_called_once_with(
            "test_coll", ["id1", "id2"]
        )

    def test_delete_empty(self, db, mock_client):
        db.delete_docs([], "test_coll")
        mock_client.delete_vectors.assert_not_called()


class TestRetrieveDocs:
    def test_retrieve_string_queries(self, db, mock_client):
        mock_client.search.return_value = [
            SearchResult(
                id="r1", score=0.95, source="hello", metadata={"key": "val"}
            ),
        ]
        results = db.retrieve_docs(["test query"], "test_coll", n_results=5)
        assert len(results) == 1  # one query
        assert len(results[0]) == 1  # one result
        doc, score = results[0][0]
        assert doc["id"] == "r1"
        assert doc["content"] == "hello"
        assert score == 0.95

    def test_retrieve_with_threshold(self, db, mock_client):
        mock_client.search.return_value = [
            SearchResult(id="r1", score=0.95, source="high", metadata={}),
            SearchResult(id="r2", score=0.30, source="low", metadata={}),
        ]
        results = db.retrieve_docs(
            ["query"], "test_coll", n_results=10, distance_threshold=0.5
        )
        assert len(results[0]) == 1  # only the high-score result
        assert results[0][0][0]["content"] == "high"

    def test_retrieve_empty(self, db, mock_client):
        mock_client.search.return_value = []
        results = db.retrieve_docs(["query"], "test_coll")
        assert results == [[]]

    def test_retrieve_no_embedding_fn(self, mock_client):
        db = ProximaDBVectorDB(client=mock_client, embedding_fn=None)
        results = db.retrieve_docs(["query"], "test_coll")
        assert results == [[]]
        mock_client.search.assert_not_called()


class TestGetDocsByIds:
    def test_get_docs_by_ids(self, db):
        docs = db.get_docs_by_ids(["id1", "id2"], "test_coll")
        assert len(docs) == 2
        assert docs[0]["id"] == "id1"
        assert docs[1]["id"] == "id2"

    def test_get_docs_empty(self, db):
        docs = db.get_docs_by_ids([], "test_coll")
        assert docs == []

    def test_get_docs_none(self, db):
        docs = db.get_docs_by_ids(None, "test_coll")
        assert docs == []
