"""
Entity v2 integration tests.

Tests for the ProximaDB Entity v2 gRPC service.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import os
import sys

import pytest

# Add src to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../src"))

from proximadb_sdk.entity_v2 import (
    Entity,
    EntityServiceClient,
    SearchEntitiesResponse,
    UpsertEntityResponse,
)
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient


@pytest.mark.integration
class TestEntityV2:
    """Integration tests for Entity v2 service."""

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
    def entity_client(self, grpc_client):
        """Create entity service client."""
        return EntityServiceClient(grpc_client)

    def test_entity_client_creation(self, entity_client):
        """Test entity client can be created."""
        assert entity_client is not None
        assert entity_client._grpc_client is not None

    def test_entity_dataclass(self):
        """Test Entity dataclass creation."""
        entity = Entity(
            id="test-entity-1",
            collection_id="test_collection",
            flexible_metadata={"name": "Test Entity", "type": "person"},
        )

        assert entity.id == "test-entity-1"
        assert entity.collection_id == "test_collection"
        assert entity.flexible_metadata["name"] == "Test Entity"

    def test_entity_to_dict(self):
        """Test Entity.to_dict() method."""
        entity = Entity(
            id="test-entity-1",
            collection_id="test_collection",
            flexible_metadata={"name": "Test"},
        )

        entity_dict = entity.to_dict()
        assert entity_dict["id"] == "test-entity-1"
        assert entity_dict["collection_id"] == "test_collection"
        assert entity_dict["flexible_metadata"]["name"] == "Test"

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_upsert_entity(self, entity_client):
        """Test upserting an entity."""
        response = entity_client.upsert_entity(
            collection_id="test_entities",
            flexible_metadata={
                "name": "John Doe",
                "age": 30,
            },
            entity_id="test-john-doe",
        )

        assert isinstance(response, UpsertEntityResponse)
        assert response.success is True
        assert response.entity_id == "test-john-doe"

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_get_entity(self, entity_client):
        """Test getting an entity."""
        # First upsert
        entity_client.upsert_entity(
            collection_id="test_entities",
            flexible_metadata={"name": "Jane Doe"},
            entity_id="test-jane-doe",
        )

        # Then get
        entity = entity_client.get_entity("test_entities", "test-jane-doe")

        assert entity is not None
        assert entity.id == "test-jane-doe"
        assert entity.flexible_metadata.get("name") == "Jane Doe"

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_search_entities(self, entity_client):
        """Test searching entities."""
        # Upsert some entities
        for i in range(3):
            entity_client.upsert_entity(
                collection_id="test_entities",
                flexible_metadata={"name": f"Person {i}", "type": "person"},
                entity_id=f"test-person-{i}",
            )

        # Search
        response = entity_client.search_entities(
            collection_id="test_entities",
            top_k=10,
        )

        assert isinstance(response, SearchEntitiesResponse)
        assert len(response.entities) >= 3

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_delete_entity(self, entity_client):
        """Test deleting an entity."""
        # First upsert
        entity_client.upsert_entity(
            collection_id="test_entities",
            flexible_metadata={"name": "To Delete"},
            entity_id="test-to-delete",
        )

        # Then delete
        deleted = entity_client.delete_entity("test_entities", "test-to-delete")
        assert deleted is True

        # Verify it's gone
        entity = entity_client.get_entity("test_entities", "test-to-delete")
        assert entity is None


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
