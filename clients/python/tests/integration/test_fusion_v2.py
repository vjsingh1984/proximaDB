"""
Fusion v2 integration tests.

Tests for the ProximaDB Fusion v2 gRPC service (ProximaFusionService).

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import os
import sys

import pytest

# Add src to path for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../src"))

from proximadb_sdk.fusion_v2 import (  # noqa: E402
    FusionHit,
    FusionSearchResponse,
    FusionServiceClient,
)
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient  # noqa: E402


@pytest.mark.integration
class TestFusionV2:
    """Integration tests for the Fusion v2 service."""

    @pytest.fixture
    def grpc_client(self):
        host = os.getenv("PROXIMADB_HOST", "localhost")
        port = int(os.getenv("PROXIMADB_GRPC_PORT", "5679"))
        client = ProximaDBSyncGrpcClient(server_address=f"{host}:{port}", pool_size=2)
        yield client
        client.close()

    @pytest.fixture
    def fusion_client(self, grpc_client):
        return FusionServiceClient(grpc_client)

    def test_fusion_client_creation(self, fusion_client):
        assert fusion_client is not None
        assert fusion_client._grpc_client is not None

    def test_fusion_hit_dataclass(self):
        hit = FusionHit(oid="graph/g/node/x", score=0.91, source_count=2)
        assert hit.oid == "graph/g/node/x"
        assert hit.score == 0.91
        assert hit.source_count == 2

    @pytest.mark.skipif(
        os.getenv("CI") == "true", reason="Skip in CI without ProximaDB server"
    )
    def test_fusion_search(self, fusion_client):
        """End-to-end fusion search (requires a seeded server)."""
        response = fusion_client.fusion_search(
            graph_id="test_graph",
            vector_collection="test_collection",
            query_vector=[0.1] * 128,
            top_k=5,
        )
        assert isinstance(response, FusionSearchResponse)
        assert isinstance(response.results, list)
        assert response.stats is not None


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
