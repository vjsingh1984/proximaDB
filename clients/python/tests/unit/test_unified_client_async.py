import asyncio

import pytest


class FakeGrpcAsync:
    def __init__(self, endpoint: str, timeout: float = 60.0):
        self.endpoint = endpoint
        self.timeout = timeout
        self.calls = []

    def shortest_path(self, *args, **kwargs):
        self.calls.append(("shortest_path", args, kwargs))

        class R:
            node_ids = ["n1", "n8"]
            total_weight = 1.0

        return R()


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
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    monkeypatch.setattr(m, "GRPC_OK", True)
    monkeypatch.setattr(m, "GrpcAsyncClient", FakeGrpcAsync)
    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="auto")
    await client.astart()
    resp_sp = await client.graph_shortest_path(
        "n1", "n8", enable_prefetch=True, prefetch_budget=3
    )
    assert getattr(resp_sp, "node_ids", None) == ["n1", "n8"]
    # Traversal goes REST; ensure _rest is available (inject if gRPC path selected)
    if client._rest is None:
        client._rest = FakeRestAsync(url="http://localhost:5678")
    resp_tr = await client.graph_traverse(
        "n1", max_depth=2, enable_prefetch=True, prefetch_budget=3
    )
    assert resp_tr["ok"] is True
    await client.aclose()


@pytest.mark.asyncio
async def test_unified_async_fallback_to_rest_on_failure(monkeypatch):
    import proximadb_sdk.unified_client_async as m
    from proximadb_sdk.unified_client_async import ProximaDBAsyncUnified

    class FailingGrpc(FakeGrpcAsync):
        def __init__(self, *a, **k):
            raise RuntimeError("init failed")

    monkeypatch.setattr(m, "GRPC_OK", True)
    monkeypatch.setattr(m, "GrpcAsyncClient", FailingGrpc)
    monkeypatch.setattr(m, "RestAsyncClient", FakeRestAsync)

    client = ProximaDBAsyncUnified(url="http://localhost:5678", protocol="auto")
    await client.astart()
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
