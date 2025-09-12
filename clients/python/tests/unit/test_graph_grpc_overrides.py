import pytest

import types
from proximadb.protocols.grpc_sync import ProximaDBSyncGrpcClient
import proximadb.protocols.grpc_sync as grpc_mod


def test_grpc_metadata_overrides(monkeypatch):
    captured = {}

    class FakeStub:
        def __init__(self, channel):
            pass

        def ShortestPath(self, req, timeout=None, metadata=None, compression=None):
            captured['metadata'] = dict(metadata or [])
            # Return a minimal response-like object
            class R:
                node_ids = [req.start_node_id, req.target_node_id]
                total_weight = 1.0
            return R()

    # Patch the stub class used inside client
    # Patch the module-level graph stubs used by client
    monkeypatch.setattr(grpc_mod, "v1_graph_pb2_grpc", types.SimpleNamespace(GraphServiceStub=FakeStub), raising=False)
    monkeypatch.setattr(grpc_mod, "v1_graph_pb2", types.SimpleNamespace(ShortestPathAlgorithm=types.SimpleNamespace(
        SHORTEST_PATH_ALGORITHM_DIJKSTRA=1,
        SHORTEST_PATH_ALGORITHM_ASTAR=2,
    )), raising=False)

    # Patch connection pool to bypass real channels
    from proximadb.protocols.connection_pools import GrpcConnectionPool, GrpcChannelContext

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

    monkeypatch.setattr("proximadb.protocols.connection_pools.GrpcConnectionPool", FakePool)
    monkeypatch.setattr("proximadb.protocols.connection_pools.GrpcChannelContext", FakeCtx)

    client = ProximaDBSyncGrpcClient("localhost:5679")
    resp = client.shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        enable_prefetch=True,
        prefetch_budget=9,
    )
    assert captured['metadata']["x-graph-prefetch-enabled"] == "true"
    assert captured['metadata']["x-graph-prefetch-budget"] == "9"
