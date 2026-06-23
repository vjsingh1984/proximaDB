import pytest


class FakeGrpcAsync:
    """Fake native-async gRPC client (records core-op + lifecycle calls).

    Mirrors the post-TD-126 ``ProximaDBAsyncGrpcClient`` surface: it has
    ``connect``/``close`` and the record core ops, but NO graph methods (graph
    always goes REST in the new design).
    """

    def __init__(self, endpoint: str, timeout: float = 60.0):
        self.endpoint = endpoint
        self.timeout = timeout
        self.calls = []
        self.closed = False

    async def connect(self):
        self.calls.append(("connect",))
        return self

    async def close(self):
        self.closed = True


class FakeRestAsync:
    def __init__(self, url: str, timeout: float = 60.0):
        self.url = url
        self.timeout = timeout
        self.calls = []

    async def aclose(self):
        return None

    async def graph_shortest_path(self, *args, **kwargs):
        self.calls.append(("graph_shortest_path", args, kwargs))
        return {"ok": True, "path": ["n1", "n8"]}

    async def graph_traverse(self, *args, **kwargs):
        self.calls.append(("graph_traverse", args, kwargs))
        return {"ok": True, "nodes": ["n1", "n2"]}


@pytest.mark.asyncio
async def test_unified_async_uses_grpc_when_available(monkeypatch):
    """auto + grpc available: native-async gRPC connected for core ops; graph=REST."""
    import proximadb_sdk.protocols.grpc_async as ga
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    monkeypatch.setattr(m, "_grpc_available", lambda: True)
    monkeypatch.setattr(ga, "ProximaDBAsyncGrpcClient", FakeGrpcAsync)
    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="auto")
    await client.astart()
    # Native-async gRPC connected and selected for core ops.
    assert client._use_grpc is True
    assert isinstance(client._grpc, FakeGrpcAsync)
    assert ("connect",) in client._grpc.calls
    # Graph ops always go over the async REST client (no graph on the new
    # native-async gRPC client).
    resp_sp = await client.graph_shortest_path(
        "n1", "n8", enable_prefetch=True, prefetch_budget=3
    )
    assert resp_sp["ok"] is True
    resp_tr = await client.graph_traverse(
        "n1", max_depth=2, enable_prefetch=True, prefetch_budget=3
    )
    assert resp_tr["ok"] is True
    await client.aclose()
    assert client._grpc is None  # aclose awaited grpc.close()


@pytest.mark.asyncio
async def test_unified_async_fallback_to_rest_on_failure(monkeypatch):
    """gRPC connect failure: facade falls back to REST core ops (no _use_grpc)."""
    import proximadb_sdk.protocols.grpc_async as ga
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    class FailingGrpc(FakeGrpcAsync):
        async def connect(self):
            raise RuntimeError("connect failed")

    monkeypatch.setattr(m, "_grpc_available", lambda: True)
    monkeypatch.setattr(ga, "ProximaDBAsyncGrpcClient", FailingGrpc)
    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="auto")
    await client.astart()
    assert client._use_grpc is False  # fell back to REST
    assert client._grpc is None
    resp_sp = await client.graph_shortest_path("n1", "n8")
    assert resp_sp["ok"] is True
    await client.aclose()


@pytest.mark.asyncio
async def test_unified_async_forced_rest(monkeypatch):
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="rest")
    await client.astart()
    resp_tr = await client.graph_traverse("n1")
    assert resp_tr["ok"] is True
    await client.aclose()


@pytest.mark.asyncio
async def test_unified_async_forced_rest_shortest_path(monkeypatch):
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="rest")
    await client.astart()
    resp_sp = await client.graph_shortest_path("n1", "n8")
    assert resp_sp["ok"] is True
    await client.aclose()
