"""
Document v2 integration tests.

Tests for the ProximaDB Document v2 gRPC service. Mirrors the entity v2
integration test harness: a create -> get -> query -> update -> aggregate
-> delete lifecycle, asserting that the canonical v2 ``TypedValue`` body
round-trips losslessly (including rich types: UUID-as-text, decimal-as-text,
lists, booleans, ints, floats).

These hit a live ProximaDB gRPC server (env-configurable host/port) and are
skipped under CI where no server is available.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import os
import sys

import pytest

# Add src to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../src"))

from proximadb_sdk.document_v2 import (
    AggregateDocumentsResponse,
    CreateDocumentResponse,
    Document,
    DocumentServiceClient,
    QueryDocumentsResponse,
)
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient


@pytest.mark.integration
class TestDocumentV2:
    """Integration tests for Document v2 service."""

    @pytest.fixture
    def grpc_client(self):
        """Create gRPC client for testing."""
        # Use environment variables or defaults
        host = os.getenv("PROXIMADB_HOST", "localhost")
        port = int(os.getenv("PROXIMADB_GRPC_PORT", "5679"))
        address = f"{host}:{port}"

        client = ProximaDBSyncGrpcClient(server_address=address, pool_size=2)
        yield client
        client.close()

    @pytest.fixture
    def document_client(self, grpc_client):
        """Create document service client."""
        return DocumentServiceClient(grpc_client)

    def test_document_client_creation(self, document_client):
        """Test document client can be created."""
        assert document_client is not None
        assert document_client._grpc_client is not None

    def test_document_dataclass(self):
        """Test Document dataclass creation."""
        document = Document(
            collection_id="test_collection",
            id="test-doc-1",
            props={"title": "Test Doc", "views": 0},
        )

        assert document.collection_id == "test_collection"
        assert document.id == "test-doc-1"
        assert document.props["title"] == "Test Doc"

    def test_document_to_dict(self):
        """Test Document.to_dict() method."""
        document = Document(
            collection_id="test_collection",
            id="test-doc-1",
            props={"title": "Test"},
        )

        document_dict = document.to_dict()
        assert document_dict["collection_id"] == "test_collection"
        assert document_dict["id"] == "test-doc-1"
        assert document_dict["props"]["title"] == "Test"

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_document_lifecycle(self, document_client):
        """Full create -> get -> query -> update -> aggregate -> delete lifecycle,
        asserting TypedValue round-trips for the full ProximaValue type system.
        """
        collection = "test_documents"

        # --- Create: rich-typed body (text, int, float, bool, list, and
        # UUID/decimal carried as text — the canonical v2 path the codec
        # preserves). ---
        uuid_hex = "550e8400e29b41d4a716446655440000"
        create_response = document_client.create_document(
            collection_id=collection,
            document_id="test-lifecycle-doc",
            props={
                "title": "Lifecycle Doc",
                "views": 42,
                "score": 9.5,
                "published": True,
                "tags": ["alpha", "beta"],
                "owner_uuid": uuid_hex,
                "price": "19.99",
            },
        )

        assert isinstance(create_response, CreateDocumentResponse)
        created = create_response.document
        assert created.id == "test-lifecycle-doc"
        assert created.props["title"] == "Lifecycle Doc"
        assert created.props["views"] == 42
        assert created.props["score"] == 9.5
        assert created.props["published"] is True
        assert created.props["tags"] == ["alpha", "beta"]
        # UUID/decimal survive as text (the round-trippable form).
        assert created.props["owner_uuid"] == uuid_hex
        assert created.props["price"] == "19.99"

        # --- Get ---
        fetched = document_client.get_document(collection, "test-lifecycle-doc")
        assert fetched is not None
        assert fetched.id == "test-lifecycle-doc"
        assert fetched.props["title"] == "Lifecycle Doc"
        assert fetched.props["views"] == 42
        assert fetched.props["tags"] == ["alpha", "beta"]

        # --- Query (scan) ---
        query_response = document_client.query_documents(
            collection_id=collection,
            limit=10,
        )
        assert isinstance(query_response, QueryDocumentsResponse)
        assert any(doc.id == "test-lifecycle-doc" for doc in query_response.documents)

        # --- Update (SET a field) ---
        updated = document_client.update_document(
            collection_id=collection,
            document_id="test-lifecycle-doc",
            updates=[
                {"operation": "set", "path": "title", "value": "Updated Title"},
                {"operation": "inc", "path": "views", "value": 8},
            ],
        )
        assert updated.props["title"] == "Updated Title"
        assert updated.props["views"] == 50

        # --- Aggregate ($group count by a constant key = total count) ---
        aggregate_response = document_client.aggregate_documents(
            collection_id=collection,
            pipeline=[
                {
                    "group": {
                        "key": "_id",
                        "aggregations": [
                            {
                                "output_field": "count",
                                "type": "count",
                                "input_path": "",
                            }
                        ],
                    }
                },
            ],
        )
        assert isinstance(aggregate_response, AggregateDocumentsResponse)
        assert len(aggregate_response.results) >= 1
        assert aggregate_response.results[0]["count"] >= 1

        # --- Delete ---
        deleted = document_client.delete_document(collection, "test-lifecycle-doc")
        assert deleted is True

        # Verify it's gone
        assert document_client.get_document(collection, "test-lifecycle-doc") is None

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_create_generates_id_when_empty(self, document_client):
        """Server generates a UUID when the client omits the id."""
        collection = "test_documents"
        response = document_client.create_document(
            collection_id=collection,
            props={"title": "Auto-id"},
        )
        assert response.document.id  # non-empty server-assigned id
        # Clean up
        document_client.delete_document(collection, response.document.id)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
