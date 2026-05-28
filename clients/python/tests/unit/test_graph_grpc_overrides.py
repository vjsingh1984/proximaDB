import types
from unittest.mock import Mock

import proximadb_sdk.protocols.grpc_sync as grpc_mod
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient


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


def _patch_grpc_client(monkeypatch, fake_stub, fake_pb2):
    monkeypatch.setattr(
        grpc_mod,
        "v1_graph_pb2_grpc",
        types.SimpleNamespace(GraphServiceStub=fake_stub),
        raising=False,
    )
    monkeypatch.setattr(grpc_mod, "v1_graph_pb2", fake_pb2, raising=False)

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
    _patch_grpc_client(
        monkeypatch,
        FakeStub,
        grpc_mod.v1_graph_pb2,
    )

    client = ProximaDBSyncGrpcClient("localhost:5679")
    client.shortest_path(
        start_node_id="n1",
        target_node_id="n8",
        enable_prefetch=True,
        prefetch_budget=9,
    )
    assert captured["metadata"]["x-graph-prefetch-enabled"] == "true"
    assert captured["metadata"]["x-graph-prefetch-budget"] == "9"


def test_grpc_graph_read_helpers_use_existing_query_endpoints(monkeypatch):
    captured = {}

    class FakeMessage:
        def __init__(self, **kwargs):
            for key, value in kwargs.items():
                setattr(self, key, value)

    class FakeStub:
        def __init__(self, channel):
            pass

        def QueryEdges(self, req, timeout=None):
            captured["query_edges"] = req
            return types.SimpleNamespace(
                success=True,
                edges=[
                    types.SimpleNamespace(
                        id="e1",
                        from_node_id="n1",
                        to_node_id="n2",
                        edge_type="CALLS",
                    )
                ],
                next_token="offset:3",
            )

        def GetNode(self, req, timeout=None):
            captured["get_node"] = req
            return types.SimpleNamespace(id=req.node_id)

        def DeleteNode(self, req, timeout=None):
            captured["delete_node"] = req
            return types.SimpleNamespace(id=req.node_id)

    fake_pb2 = types.SimpleNamespace(
        EdgeQuery=FakeMessage,
        GetNodeRequest=FakeMessage,
        DeleteNodeRequest=FakeMessage,
        PropertyFilter=FakeMessage,
        PROPERTY_FILTER_OPERATOR_EQUALS=1,
    )
    _patch_grpc_client(monkeypatch, FakeStub, fake_pb2)

    client = ProximaDBSyncGrpcClient("localhost:5679")
    monkeypatch.setattr(client, "_convert_to_property_value", lambda value: value)
    monkeypatch.setattr(
        client, "_convert_edge_from_proto", lambda edge: {"id": edge.id}
    )
    monkeypatch.setattr(
        client, "_convert_node_from_proto", lambda node: {"id": node.id}
    )

    edge_result = client.query_edges(
        edge_type="CALLS",
        from_node_id="n1",
        to_node_id="n2",
        properties={"lang": "python"},
        limit=3,
        offset=1,
        graph_id="code",
    )
    assert edge_result["success"] is True
    assert edge_result["edges"] == [{"id": "e1"}]
    assert edge_result["total_count"] == 1
    assert edge_result["next_token"] == "offset:3"
    assert captured["query_edges"].graph_id == "code"
    assert captured["query_edges"].from_node_id == "n1"
    assert captured["query_edges"].to_node_id == "n2"
    assert captured["query_edges"].edge_types == ["CALLS"]
    assert captured["query_edges"].limit == 3
    assert captured["query_edges"].offset == 1
    assert captured["query_edges"].filters[0].key == "lang"
    assert captured["query_edges"].filters[0].value == "python"

    assert client.get_node("n42", graph_id="code") == {"id": "n42"}
    assert captured["get_node"].graph_id == "code"
    assert captured["get_node"].node_id == "n42"

    assert client.delete_node("n42", graph_id="code") == {"id": "n42"}
    assert captured["delete_node"].graph_id == "code"
    assert captured["delete_node"].node_id == "n42"


def test_grpc_directional_edge_helpers_reuse_query_edges(monkeypatch):
    client = object.__new__(ProximaDBSyncGrpcClient)
    captured = []

    def fake_query_edges(**kwargs):
        captured.append(kwargs)
        return {"edges": [{"id": kwargs.get("edge_type") or "*"}]}

    monkeypatch.setattr(client, "query_edges", fake_query_edges)

    outgoing = client.get_outgoing_edges(
        node_id="n1", edge_types=["CALLS", "IMPORTS"], graph_id="code"
    )
    incoming = client.get_incoming_edges(node_id="n2", graph_id="code")

    assert outgoing == [{"id": "CALLS"}, {"id": "IMPORTS"}]
    assert incoming == [{"id": "*"}]
    assert captured[0] == {
        "edge_type": "CALLS",
        "from_node_id": "n1",
        "graph_id": "code",
        "limit": 10000,
    }
    assert captured[1] == {
        "edge_type": "IMPORTS",
        "from_node_id": "n1",
        "graph_id": "code",
        "limit": 10000,
    }
    assert captured[2] == {
        "edge_type": "",
        "to_node_id": "n2",
        "graph_id": "code",
        "limit": 10000,
    }
