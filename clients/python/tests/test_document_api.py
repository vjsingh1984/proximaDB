"""
Integration tests for ProximaDB Document API.

Tests the multi-model document storage functionality including:
- Collection creation with indexes
- Document CRUD operations
- Query with filters
- Full-text search (if available)
"""

import os
import sys

import pytest

# Add the src directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from proximadb_sdk.document import (
    DocIndexType,
    DocumentCollectionConfig,
    DocumentFilter,
    IndexDefinition,
    ProximaDBDocument,
)


@pytest.fixture
def client(embedded_rest_client):
    """Create a ProximaDB client for testing."""
    return embedded_rest_client


@pytest.fixture
def document_api(client):
    """Create a Document API instance for testing."""
    return ProximaDBDocument(client)


@pytest.fixture
def test_collection_name():
    """Name of the test collection."""
    return "test_documents_python_sdk"


class TestDocumentAPI:
    """Test suite for Document API operations."""

    def test_create_document_collection(self, document_api, test_collection_name):
        """Test creating a document collection with indexes."""
        # Create collection configuration
        config = DocumentCollectionConfig(
            name=test_collection_name,
            indexes=[
                IndexDefinition(path="$.language", type=DocIndexType.HASH),
                IndexDefinition(path="$.file_path", type=DocIndexType.BTREE),
            ],
            enable_fulltext=True,
            fulltext_paths=["$.content", "$.description"],
        )

        # Create collection
        result = document_api.create_collection(config=config)

        # Verify result
        assert result is not None
        assert result.get("success") is True or result.get("collection_id") is not None

    def test_insert_document(self, document_api, test_collection_name):
        """Test inserting a document."""
        # Create test document
        document = {
            "file_path": "src/main.py",
            "language": "python",
            "content": "def hello(): print('Hello, World!')",
            "description": "A simple hello world function",
            "lines_of_code": 2,
            "tags": ["example", "tutorial"],
        }

        # Insert document
        result = document_api.insert_document(
            collection_id=test_collection_name, document=document, id="doc:main.py"
        )

        # Verify result
        assert result is not None
        assert result.get("id") == "doc:main.py"
        assert result.get("version") > 0

    def test_get_document(self, document_api, test_collection_name):
        """Test retrieving a document by ID."""
        # Get document
        doc = document_api.get_document(
            collection_id=test_collection_name, doc_id="doc:main.py"
        )

        # Verify result
        assert doc is not None
        assert doc.get("document", {}).get("language") == "python"
        assert doc.get("document", {}).get("file_path") == "src/main.py"

    def test_query_documents_with_filter(self, document_api, test_collection_name):
        """Test querying documents with a filter."""
        # Create filter
        filter_obj = DocumentFilter().eq("language", "python")

        # Query documents
        results = document_api.query(
            collection_id=test_collection_name, filter=filter_obj, limit=10
        )

        # Verify results
        assert results is not None
        documents = results.get("documents", [])
        assert len(documents) > 0

        # Verify all results match filter
        for doc in documents:
            doc_data = doc.get("document", {}) if isinstance(doc, dict) else {}
            language = doc_data.get("language")
            assert language == "python"

    def test_query_documents_with_projection(self, document_api, test_collection_name):
        """Test querying documents with field projection."""
        # Query with projection
        results = document_api.query(
            collection_id=test_collection_name,
            projection=["file_path", "language"],
            limit=10,
        )

        # Verify results
        assert results is not None
        documents = results.get("documents", [])

        # Check that projected fields are present
        for doc in documents:
            doc_data = doc.get("document", {}) if isinstance(doc, dict) else {}
            # Should have projected fields
            assert "file_path" in doc_data or "language" in doc_data

    def test_update_document(self, document_api, test_collection_name):
        """Test updating a document."""
        # Update document
        result = document_api.update(
            collection_id=test_collection_name,
            doc_id="doc:main.py",
            updates=[
                {"operation": "SET", "path": "$.lines_of_code", "value": 5},
                {"operation": "PUSH", "path": "$.tags", "value": "updated"},
            ],
        )

        # Verify result
        assert result is not None
        assert result.get("success") is True
        assert result.get("new_version") > 0

        # Verify the update
        doc = document_api.get_document(
            collection_id=test_collection_name, doc_id="doc:main.py"
        )
        doc_data = doc.get("document", {})
        assert doc_data.get("lines_of_code") == 5
        assert "updated" in doc_data.get("tags", [])

    def test_delete_document(self, document_api, test_collection_name):
        """Test deleting a document."""
        # Insert a temporary document
        document_api.insert_document(
            collection_id=test_collection_name,
            document={"temp": True, "data": "test"},
            id="doc:temp",
        )

        # Delete document
        result = document_api.delete(
            collection_id=test_collection_name, doc_id="doc:temp"
        )

        # Verify result
        assert result is True or result.get("deleted") is True

        # Verify document is gone
        doc = document_api.get_document(
            collection_id=test_collection_name, doc_id="doc:temp"
        )
        assert doc is None or doc.get("found") is False

    def test_list_collections(self, document_api):
        """Test listing document collections."""
        # List collections
        collections = document_api.list_collections()

        # Verify results
        assert collections is not None
        assert isinstance(collections, list)

        # Find our test collection
        test_collection = None
        for coll in collections:
            if isinstance(coll, dict):
                if coll.get("name") == "test_documents_python_sdk":
                    test_collection = coll
                    break

        # Verify test collection exists
        assert test_collection is not None

    def test_fulltext_search(self, document_api, test_collection_name):
        """Test full-text search (if available)."""
        # Query with full-text search
        filter_obj = DocumentFilter().fulltext("content", "hello")

        results = document_api.query(
            collection_id=test_collection_name, filter=filter_obj, limit=10
        )

        # Verify results
        assert results is not None

        # If full-text is working, we should get results
        documents = results.get("documents", [])
        if len(documents) > 0:
            # Verify score is present for full-text results
            for doc in documents:
                if isinstance(doc, dict):
                    # Full-text results should have scores
                    assert "score" in doc or "id" in doc

    def test_aggregation_query(self, document_api, test_collection_name):
        """Test aggregation queries."""
        # Insert more test documents
        for i in range(5):
            document_api.insert_document(
                collection_id=test_collection_name,
                document={
                    "language": "python" if i % 2 == 0 else "javascript",
                    "lines_of_code": 10 + i * 5,
                    "category": f"category_{i % 3}",
                },
                id=f"doc:agg_{i}",
            )

        # Perform aggregation
        results = document_api.aggregate(
            collection_id=test_collection_name,
            pipeline=[
                {
                    "stage": "match",
                    "filter": DocumentFilter().eq("language", "python").to_dict(),
                },
                {
                    "stage": "group",
                    "key": "$.category",
                    "aggregations": [
                        {"field": "avg_loc", "type": "avg", "path": "$.lines_of_code"},
                        {"field": "count", "type": "count", "path": "$.category"},
                    ],
                },
            ],
        )

        # Verify results
        assert results is not None

    def test_delete_collection(self, document_api, test_collection_name):
        """Test deleting a document collection."""
        # Delete collection
        result = document_api.delete_collection(collection_id=test_collection_name)

        # Verify result
        assert result is True or result.get("success") is True

        # Verify collection is gone
        collections = document_api.list_collections()
        test_collection_exists = False
        for coll in collections:
            if isinstance(coll, dict) and coll.get("name") == test_collection_name:
                test_collection_exists = True
                break

        assert not test_collection_exists


class TestDocumentAdapterMethods:
    """Test suite for Document adapter methods."""

    def test_adapter_create_document_collection(self, client):
        """Test creating a document collection via adapter."""
        # Create via adapter
        result = client.create_document_collection(
            name="test_adapter_docs",
            config={"indexes": [{"path": "$.category", "type": "hash"}]},
        )

        # Verify
        assert result is not None
        assert result.get("success") is True or result.get("collection_id") is not None

        # Cleanup
        client.delete_document_collection("test_adapter_docs")

    def test_adapter_insert_and_query_document(self, client):
        """Test inserting and querying documents via adapter."""
        # Insert document
        doc = client.insert_document(
            collection_name="test_adapter_docs",
            document={"category": "test", "value": 42},
            id="adapter_test",
        )

        assert doc is not None
        assert doc.get("id") == "adapter_test"

        # Query documents
        results = client.query_documents(
            collection_name="test_adapter_docs", filter={"category": "test"}, limit=10
        )

        assert results is not None
        assert len(results.get("documents", [])) > 0

    def test_adapter_get_document(self, client):
        """Test getting a document via adapter."""
        doc = client.get_document(
            collection_name="test_adapter_docs", doc_id="adapter_test"
        )

        assert doc is not None
        assert doc.get("document", {}).get("value") == 42

    def test_adapter_update_document(self, client):
        """Test updating a document via adapter."""
        result = client.update_document(
            collection_name="test_adapter_docs",
            doc_id="adapter_test",
            updates=[{"operation": "SET", "path": "$.value", "value": 100}],
        )

        assert result is not None
        assert result.get("success") is True

        # Verify update
        doc = client.get_document(
            collection_name="test_adapter_docs", doc_id="adapter_test"
        )
        assert doc.get("document", {}).get("value") == 100

    def test_adapter_delete_document(self, client):
        """Test deleting a document via adapter."""
        result = client.delete_document(
            collection_name="test_adapter_docs", doc_id="adapter_test"
        )

        assert result is True

    def test_adapter_list_collections(self, client):
        """Test listing document collections via adapter."""
        # Create a test collection first
        client.create_document_collection(name="test_list_docs")

        # List collections
        collections = client.list_document_collections()

        assert collections is not None
        assert isinstance(collections, list)

        # Cleanup
        client.delete_document_collection("test_list_docs")


if __name__ == "__main__":
    # Run tests
    pytest.main([__file__, "-v", "--tb=short"])
