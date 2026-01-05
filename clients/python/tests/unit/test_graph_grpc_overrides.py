import pytest
from unittest.mock import Mock, patch

import types
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient
import proximadb_sdk.protocols.grpc_sync as grpc_mod


class MockResourcePool:
    """Mock resource pool that doesn't actually create connections"""

    def __init__(self, factory, max_size=5, **kwargs):
        self.factory = factory
        self.max_size = max_size
        self._resources = []

    def acquire(self, timeout=None):
        return Mock()

    def release(self, resource):
        pass

    def close(self):
        pass

    def get_stats(self):
        """Return mock pool stats as dict"""
        return {
            "total_resources": self.max_size,
            "available_resources": self.max_size,
            "in_use_resources": 0,
            "resources_created": self.max_size,
            "resources_destroyed": 0,
        }


def test_grpc_metadata_overrides(monkeypatch):
    captured = {}

    class FakeStub:
        def __init__(self, channel):
            pass

        def ShortestPath(self, req, timeout=None, metadata=None, compression=None):
            captured["metadata"] = dict(metadata or [])

            # Return a minimal response-like object
            class R:
                node_ids = [req.start_node_id, req.target_node_id]
                total_weight = 1.0

            return R()

    # Patch the stub class used inside client
    # Patch the module-level graph stubs used by client
    monkeypatch.setattr(
        grpc_mod,
        "v1_graph_pb2_grpc",
        types.SimpleNamespace(GraphServiceStub=FakeStub),
        raising=False,
    )

    # Create a fake request class
    class FakeShortestPathRequest:
        def __init__(self, **kwargs):
            for k, v in kwargs.items():
                setattr(self, k, v)

    monkeypatch.setattr(
        grpc_mod,
        "v1_graph_pb2",
        types.SimpleNamespace(
            ShortestPathRequest=FakeShortestPathRequest,
            ShortestPathAlgorithm=types.SimpleNamespace(
                SHORTEST_PATH_ALGORITHM_DIJKSTRA=1,
                SHORTEST_PATH_ALGORITHM_ASTAR=2,
            ),
        ),
        raising=False,
    )

    # Patch connection pool to bypass real channels
    from proximadb_sdk.protocols.connection_pools import (
        GrpcConnectionPool,
        GrpcChannelContext,
    )

    class FakePool:
        def __init__(self, *a, **k):
            pass

        def get_metrics(self):
            return {}

        def close(self):
            pass

    class FakeCtx:
        def __init__(self, pool):
            pass

        def __enter__(self):
            return None

        def __exit__(self, exc_type, exc, tb):
            return False

    monkeypatch.setattr(
        "proximadb_sdk.protocols.connection_pools.GrpcConnectionPool", FakePool
    )
    monkeypatch.setattr(
        "proximadb_sdk.protocols.connection_pools.GrpcChannelContext", FakeCtx
    )
    monkeypatch.setattr("proximadb_sdk.resource_pool.ResourcePool", MockResourcePool)
    monkeypatch.setattr("grpc.insecure_channel", lambda *a, **k: Mock())

    client = ProximaDBSyncGrpcClient("localhost:5679")
    resp = client.shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        enable_prefetch=True,
        prefetch_budget=9,
    )
    assert captured["metadata"]["x-graph-prefetch-enabled"] == "true"
    assert captured["metadata"]["x-graph-prefetch-budget"] == "9"
