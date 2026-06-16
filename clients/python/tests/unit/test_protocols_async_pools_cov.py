"""
Offline unit tests for proximadb_sdk.protocols async + connection-pool modules.

Targets:
- protocols/rest_async.py   (ProximaDBAsyncClient — httpx.AsyncClient mocked)
- protocols/grpc_async.py   (deprecated shim that inherits grpc_sync)
- protocols/connection_pools.py (Grpc/Rest pools + factories + contexts)

Everything is fully offline: httpx.AsyncClient is replaced with an in-memory
fake, gRPC channel creation/validation is monkeypatched so no real socket is
ever opened, and the ResourcePool background maintenance thread sleeps on a
patched time.sleep so it never blocks the suite.
"""

import sys
import types
import warnings
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# rest_async.ProximaDBAsyncClient
# ---------------------------------------------------------------------------
from proximadb_sdk.protocols import connection_pools as cp
from proximadb_sdk.protocols import rest_async
from proximadb_sdk.protocols.connection_pools import (
    GrpcChannelContext,
    GrpcChannelFactory,
    GrpcConnectionPool,
    PoolHealth,
    PoolMetrics,
    RestClientContext,
    RestConnectionPool,
)


class _FakeResponse:
    def __init__(self, payload, status_code=200):
        self._payload = payload
        self.status_code = status_code
        self.headers = {}
        self.raised = False

    def raise_for_status(self):
        self.raised = True
        return None

    def json(self):
        return self._payload


class _FakeAsyncClient:
    """Stand-in for httpx.AsyncClient that records calls and returns canned json."""

    def __init__(self, *args, **kwargs):
        self.init_args = args
        self.init_kwargs = kwargs
        self.posts = []
        self.closed = False
        self.next_payload = {"ok": True}

    async def post(self, path, json=None, headers=None):
        self.posts.append({"path": path, "json": json, "headers": headers})
        return _FakeResponse(self.next_payload)

    async def aclose(self):
        self.closed = True


@pytest.fixture
def async_client(monkeypatch):
    fake = _FakeAsyncClient()
    # Patch the httpx.AsyncClient used inside rest_async so __init__ never opens
    # a real transport.
    monkeypatch.setattr(rest_async.httpx, "AsyncClient", lambda *a, **k: fake)
    client = rest_async.ProximaDBAsyncClient(url="http://testserver:5678/")
    # client._client is the fake instance
    return client, fake


@pytest.mark.asyncio
async def test_async_client_init_strips_trailing_slash(async_client):
    client, fake = async_client
    assert client._base_url == "http://testserver:5678"
    assert client._timeout == 60.0
    assert client._client is fake


@pytest.mark.asyncio
async def test_async_client_custom_timeout(monkeypatch):
    fake = _FakeAsyncClient()
    monkeypatch.setattr(rest_async.httpx, "AsyncClient", lambda *a, **k: fake)
    client = rest_async.ProximaDBAsyncClient(url="http://h", timeout=12.5)
    assert client._timeout == 12.5


@pytest.mark.asyncio
async def test_aclose(async_client):
    client, fake = async_client
    await client.aclose()
    assert fake.closed is True


@pytest.mark.asyncio
async def test_shortest_path_minimal(async_client):
    client, fake = async_client
    fake.next_payload = {"path": ["a", "b"]}
    out = await client.graph_shortest_path("a", "b")
    assert out == {"path": ["a", "b"]}
    call = fake.posts[-1]
    assert call["path"] == "/api/v2/graphs/default/shortest-path"
    body = call["json"]
    assert body["start_node_id"] == "a"
    assert body["target_node_id"] == "b"
    assert body["algorithm"] == "DIJKSTRA"
    # No optional fields set
    assert "max_depth" not in body
    assert "edge_types" not in body
    assert "k" not in body
    assert "enable_prefetch" not in body
    # default content-type header only
    assert call["headers"]["Content-Type"] == "application/json"
    assert "x-graph-prefetch-enabled" not in call["headers"]


@pytest.mark.asyncio
async def test_shortest_path_all_options(async_client):
    client, fake = async_client
    out = await client.graph_shortest_path(
        "x",
        "y",
        max_depth=7,
        edge_types=["KNOWS", "LIKES"],
        algorithm="ASTAR",
        k=3,
        enable_prefetch=True,
        prefetch_budget=42,
        graph_id="g1",
    )
    assert out == {"ok": True}
    call = fake.posts[-1]
    assert call["path"] == "/api/v2/graphs/g1/shortest-path"
    body = call["json"]
    assert body["max_depth"] == 7
    assert body["edge_types"] == ["KNOWS", "LIKES"]
    assert body["k"] == 3
    assert body["algorithm"] == "ASTAR"
    assert body["enable_prefetch"] is True
    assert body["prefetch_budget"] == 42
    hdr = call["headers"]
    assert hdr["x-graph-prefetch-enabled"] == "true"
    assert hdr["x-graph-prefetch-budget"] == "42"


@pytest.mark.asyncio
async def test_shortest_path_prefetch_disabled(async_client):
    client, fake = async_client
    await client.graph_shortest_path("a", "b", enable_prefetch=False)
    call = fake.posts[-1]
    assert call["headers"]["x-graph-prefetch-enabled"] == "false"
    assert call["json"]["enable_prefetch"] is False


@pytest.mark.asyncio
async def test_traverse_minimal(async_client):
    client, fake = async_client
    fake.next_payload = {"nodes": []}
    out = await client.graph_traverse("root")
    assert out == {"nodes": []}
    call = fake.posts[-1]
    assert call["path"] == "/api/v2/graphs/default/traverse"
    body = call["json"]
    assert body["start_node_id"] == "root"
    assert body["max_depth"] == 3
    assert body["algorithm"] == "BFS"
    assert "limit" not in body
    assert "timeout_ms" not in body
    assert "max_frontier" not in body


@pytest.mark.asyncio
async def test_traverse_all_options(async_client):
    client, fake = async_client
    await client.graph_traverse(
        "root",
        max_depth=5,
        edge_types=["E"],
        algorithm="DFS",
        limit=10,
        timeout_ms=2500,
        max_frontier=99,
        enable_prefetch=True,
        prefetch_budget=8,
        graph_id="gg",
    )
    call = fake.posts[-1]
    assert call["path"] == "/api/v2/graphs/gg/traverse"
    body = call["json"]
    assert body["max_depth"] == 5
    assert body["edge_types"] == ["E"]
    assert body["limit"] == 10
    assert body["timeout_ms"] == 2500
    assert body["max_frontier"] == 99
    assert body["algorithm"] == "DFS"
    assert body["enable_prefetch"] is True
    assert body["prefetch_budget"] == 8
    hdr = call["headers"]
    assert hdr["x-graph-prefetch-enabled"] == "true"
    assert hdr["x-graph-prefetch-budget"] == "8"


@pytest.mark.asyncio
async def test_traverse_prefetch_disabled(async_client):
    client, fake = async_client
    await client.graph_traverse("root", enable_prefetch=False)
    call = fake.posts[-1]
    assert call["headers"]["x-graph-prefetch-enabled"] == "false"
    assert call["json"]["enable_prefetch"] is False


# ---------------------------------------------------------------------------
# grpc_async deprecated shim
# ---------------------------------------------------------------------------
def test_grpc_async_module_is_deprecated_shim():
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        from proximadb_sdk.protocols import grpc_async

    from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient

    assert issubclass(grpc_async.ProximaDBClient, ProximaDBSyncGrpcClient)
    assert grpc_async.AsyncGrpcClient is grpc_async.ProximaDBClient


def test_grpc_async_client_construction_warns():
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        from proximadb_sdk.protocols import grpc_async

    # Patch the parent __init__ so no real channel is created.
    with patch.object(
        grpc_async.ProximaDBSyncGrpcClient, "__init__", return_value=None
    ) as parent_init:
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            obj = grpc_async.ProximaDBClient("localhost:5679", foo="bar")
        assert obj is not None
        parent_init.assert_called_once()
        args, kwargs = parent_init.call_args
        assert args[0] == "localhost:5679"
        assert kwargs == {"foo": "bar"}
        assert any(issubclass(w.category, DeprecationWarning) for w in caught)


# ---------------------------------------------------------------------------
# connection_pools — PoolMetrics / PoolHealth
# ---------------------------------------------------------------------------
def test_pool_metrics_defaults_and_health_enum():
    m = PoolMetrics()
    assert m.total_connections == 0
    assert m.active_connections == 0
    assert m.health_status == PoolHealth.HEALTHY
    assert PoolHealth.DEGRADED.value == "degraded"
    assert PoolHealth.UNHEALTHY.value == "unhealthy"


# ---------------------------------------------------------------------------
# GrpcChannelFactory
# ---------------------------------------------------------------------------
def test_grpc_factory_channel_options_no_compression():
    f = GrpcChannelFactory(endpoint="localhost:5679")
    opt_names = [o[0] for o in f.channel_options]
    assert "grpc.max_receive_message_length" in opt_names
    assert "grpc.keepalive_time_ms" in opt_names
    assert "grpc.default_compression_algorithm" not in opt_names
    assert f.endpoint == "localhost:5679"
    assert f.use_tls is False


def test_grpc_factory_channel_options_with_compression():
    f = GrpcChannelFactory(endpoint="h:1", compression=cp.grpc.Compression.Gzip)
    opt_names = [o[0] for o in f.channel_options]
    assert "grpc.default_compression_algorithm" in opt_names
    assert "grpc.default_compression_level" in opt_names


def test_grpc_factory_create_insecure(monkeypatch):
    f = GrpcChannelFactory(endpoint="localhost:5679")
    sentinel = object()
    monkeypatch.setattr(cp.grpc, "insecure_channel", lambda ep, options: sentinel)
    assert f.create() is sentinel


def test_grpc_factory_create_secure(monkeypatch):
    f = GrpcChannelFactory(endpoint="localhost:5679", use_tls=True)
    sentinel = object()
    monkeypatch.setattr(cp.grpc, "ssl_channel_credentials", lambda: "creds")
    captured = {}

    def fake_secure(ep, creds, options):
        captured["ep"] = ep
        captured["creds"] = creds
        return sentinel

    monkeypatch.setattr(cp.grpc, "secure_channel", fake_secure)
    assert f.create() is sentinel
    assert captured["ep"] == "localhost:5679"
    assert captured["creds"] == "creds"


def test_grpc_factory_create_raises_when_unavailable(monkeypatch):
    f = GrpcChannelFactory(endpoint="h:1")
    monkeypatch.setattr(cp, "GRPC_AVAILABLE", False)
    with pytest.raises(ImportError):
        f.create()


def test_grpc_factory_validate_ok(monkeypatch):
    f = GrpcChannelFactory(endpoint="h:1")
    fake_future = MagicMock()
    fake_future.result.return_value = None
    monkeypatch.setattr(cp.grpc, "channel_ready_future", lambda ch: fake_future)
    assert f.validate(MagicMock()) is True


def test_grpc_factory_validate_failure(monkeypatch):
    f = GrpcChannelFactory(endpoint="h:1")

    def boom(ch):
        raise RuntimeError("not ready")

    monkeypatch.setattr(cp.grpc, "channel_ready_future", boom)
    assert f.validate(MagicMock()) is False


def test_grpc_factory_reset_is_noop():
    f = GrpcChannelFactory(endpoint="h:1")
    assert f.reset(MagicMock()) is None


def test_grpc_factory_dispose_and_destroy(monkeypatch):
    f = GrpcChannelFactory(endpoint="h:1")
    # Avoid the real 50ms sleep inside dispose.
    monkeypatch.setattr(cp.time, "sleep", lambda *_a, **_k: None)
    ch = MagicMock()
    f.dispose(ch)
    ch.close.assert_called_once()
    # destroy delegates to dispose
    ch2 = MagicMock()
    f.destroy(ch2)
    ch2.close.assert_called_once()


def test_grpc_factory_dispose_suppresses_errors(monkeypatch):
    f = GrpcChannelFactory(endpoint="h:1")
    monkeypatch.setattr(cp.time, "sleep", lambda *_a, **_k: None)
    ch = MagicMock()
    ch.close.side_effect = RuntimeError("boom")
    # Should not raise
    f.dispose(ch)


# ---------------------------------------------------------------------------
# GrpcChannelContext
# ---------------------------------------------------------------------------
def test_grpc_context_with_channel_directly():
    channel = object()
    ctx = GrpcChannelContext(channel)
    assert ctx.pool is None
    assert ctx._owns_channel is False
    with ctx as c:
        assert c is channel


def test_grpc_context_with_pool_acquires_and_returns():
    pool = MagicMock()
    channel = object()
    pool.get_channel.return_value = channel
    ctx = GrpcChannelContext(pool)
    assert ctx.pool is pool
    assert ctx._owns_channel is True
    with ctx as c:
        assert c is channel
    pool.get_channel.assert_called_once()
    pool.return_channel.assert_called_once_with(channel, success=True)


def test_grpc_context_with_pool_returns_failure_on_exc():
    pool = MagicMock()
    pool.get_channel.return_value = object()
    ctx = GrpcChannelContext(pool)
    try:
        with ctx:
            raise ValueError("kaboom")
    except ValueError:
        pass
    _, kwargs = pool.return_channel.call_args
    assert kwargs["success"] is False


def test_rest_client_context():
    client = MagicMock()
    ctx = RestClientContext(client)
    assert ctx.client is client
    with ctx as c:
        assert c is ctx
    # __exit__ returns False
    assert ctx.__exit__(None, None, None) is False


# ---------------------------------------------------------------------------
# GrpcConnectionPool — patch ResourcePool so no real channel / thread work.
# ---------------------------------------------------------------------------
class _FakeResourcePool:
    def __init__(self, *args, **kwargs):
        self.init_kwargs = kwargs
        self.released = []
        self.closed = False
        self.acquire_calls = 0
        self._stats = {
            "active": 1,
            "idle": 2,
            "resources_created": 3,
            "total_acquisitions": 7,
        }

    def acquire(self, timeout=None, **kwargs):
        self.acquire_calls += 1
        return f"channel-{self.acquire_calls}"

    def release(self, resource):
        self.released.append(resource)

    def get_stats(self):
        return dict(self._stats)

    def close(self):
        self.closed = True


@pytest.fixture
def grpc_pool(monkeypatch):
    monkeypatch.setattr(cp, "ResourcePool", _FakeResourcePool)
    pool = GrpcConnectionPool(endpoint="localhost:5679", pool_size=3)
    return pool


def test_grpc_pool_init_and_warmup(grpc_pool):
    assert grpc_pool.endpoint == "localhost:5679"
    assert grpc_pool.pool_size == 3
    # warm-up acquired+released min(pool_size,5)=3 channels
    rp = grpc_pool._pool
    assert isinstance(rp, _FakeResourcePool)
    assert len(rp.released) == 3


def test_grpc_pool_get_and_return_channel(grpc_pool):
    rp = grpc_pool._pool
    rp.released.clear()
    ch = grpc_pool.get_channel()
    assert ch.startswith("channel-")
    grpc_pool.return_channel(ch)
    assert rp.released == [ch]


def test_grpc_pool_return_channel_failure(grpc_pool):
    rp = grpc_pool._pool
    rp.released.clear()
    grpc_pool.return_channel("c", success=False)
    assert rp.released == ["c"]


def test_grpc_pool_get_connection_context(grpc_pool):
    rp = grpc_pool._pool
    rp.released.clear()
    with grpc_pool.get_connection() as ctx:
        assert isinstance(ctx, GrpcChannelContext)
        # ctx wraps a channel directly (not a pool)
        assert ctx._owns_channel is False
    assert len(rp.released) == 1


def test_grpc_pool_active_connections(grpc_pool):
    assert grpc_pool.get_active_connections() == 1


def test_grpc_pool_get_metrics_healthy(grpc_pool):
    m = grpc_pool.get_metrics()
    assert isinstance(m, PoolMetrics)
    assert m.total_connections == 3
    assert m.active_connections == 1
    assert m.idle_connections == 2
    assert m.requests_served == 7
    assert m.health_status == PoolHealth.HEALTHY


def test_grpc_pool_get_metrics_degraded(grpc_pool):
    grpc_pool._pool._stats = {
        "active": 0,
        "idle": 0,
        "resources_created": 5,
        "total_acquisitions": 1,
    }
    m = grpc_pool.get_metrics()
    assert m.health_status == PoolHealth.DEGRADED


def test_grpc_pool_get_metrics_unhealthy(grpc_pool):
    grpc_pool._pool._stats = {
        "active": 0,
        "idle": 0,
        "resources_created": 0,
        "total_acquisitions": 0,
    }
    m = grpc_pool.get_metrics()
    assert m.health_status == PoolHealth.UNHEALTHY


def test_grpc_pool_warmup_handles_acquire_failure(monkeypatch):
    class _FailingPool(_FakeResourcePool):
        def acquire(self, timeout=None, **kwargs):
            raise RuntimeError("cannot create")

    monkeypatch.setattr(cp, "ResourcePool", _FailingPool)
    # Should not raise despite warm-up failures.
    pool = GrpcConnectionPool(endpoint="h:1", pool_size=2)
    assert pool._pool.released == []


def test_grpc_pool_close(grpc_pool):
    grpc_pool.close()
    assert grpc_pool._pool.closed is True


def test_grpc_pool_close_suppresses_errors(grpc_pool):
    def boom():
        raise RuntimeError("nope")

    grpc_pool._pool.close = boom
    # Should not raise.
    grpc_pool.close()


# ---------------------------------------------------------------------------
# RestConnectionPool — httpx.Client mocked so no real socket.
# ---------------------------------------------------------------------------
class _FakeHttpxClient:
    instances = []

    def __init__(self, *args, **kwargs):
        self.init_kwargs = kwargs
        self.closed = False
        _FakeHttpxClient.instances.append(self)
        # mimic httpx.Client.limits attribute used by _initialize_pools
        limits = kwargs.get("limits")
        self.limits = limits

    def close(self):
        self.closed = True


@pytest.fixture
def rest_pool(monkeypatch):
    _FakeHttpxClient.instances = []
    monkeypatch.setattr(cp.httpx, "Client", _FakeHttpxClient)
    pool = RestConnectionPool(
        base_url="http://testserver:5678", pool_size=4, timeout=15.0
    )
    return pool


def test_rest_pool_init_from_base_url(rest_pool):
    assert rest_pool.base_url == "http://testserver:5678"
    assert rest_pool.pool_size == 4
    assert rest_pool.timeout == 15.0
    # three specialized pools created
    assert set(rest_pool._pools.keys()) == {"read", "write", "search"}
    # total connections = 20 + 10 + 15
    assert rest_pool.metrics.total_connections == 45
    assert rest_pool.metrics.idle_connections == 45


def test_rest_pool_init_from_config(monkeypatch):
    _FakeHttpxClient.instances = []
    monkeypatch.setattr(cp.httpx, "Client", _FakeHttpxClient)
    from proximadb_sdk.config import ClientConfig

    config = ClientConfig(url="http://cfg:5678")
    pool = RestConnectionPool(config=config)
    assert pool.base_url == "http://cfg:5678"
    assert pool.config is config


def test_rest_pool_init_requires_config_or_url():
    with pytest.raises(ValueError):
        RestConnectionPool()


def test_rest_pool_get_client_maps_operations(rest_pool):
    read_c = rest_pool.get_client("health")
    assert read_c is rest_pool._pools["read"]
    write_c = rest_pool.get_client("insert_vectors")
    assert write_c is rest_pool._pools["write"]
    search_c = rest_pool.get_client("vector_search")
    assert search_c is rest_pool._pools["search"]
    # unknown -> read
    unk = rest_pool.get_client("nonexistent_op")
    assert unk is rest_pool._pools["read"]


def test_rest_pool_get_client_updates_metrics(rest_pool):
    before_active = rest_pool.metrics.active_connections
    before_idle = rest_pool.metrics.idle_connections
    rest_pool.get_client("read")
    assert rest_pool.metrics.active_connections == before_active + 1
    assert rest_pool.metrics.idle_connections == before_idle - 1


def test_rest_pool_return_client_updates_metrics(rest_pool):
    client = rest_pool.get_client("read")
    active_after_get = rest_pool.metrics.active_connections
    rest_pool.return_client(client, success=True, response_time_ms=12.0)
    assert rest_pool.metrics.requests_served == 1
    assert rest_pool.metrics.active_connections == active_after_get - 1
    assert rest_pool.metrics.avg_response_time_ms == 12.0


def test_rest_pool_return_client_avg_response_time_window(rest_pool):
    client = rest_pool.get_client("read")
    for i in range(105):
        rest_pool.return_client(client, response_time_ms=float(i + 1))
    # window capped at 100 samples
    assert len(rest_pool._request_times) == 100
    assert rest_pool.metrics.avg_response_time_ms > 0


def test_rest_pool_return_client_zero_time_skips_avg(rest_pool):
    client = rest_pool.get_client("read")
    rest_pool.return_client(client, response_time_ms=0.0)
    assert rest_pool._request_times == []


def test_rest_pool_get_connection_context(rest_pool):
    with rest_pool.get_connection("search_vectors") as ctx:
        assert isinstance(ctx, RestClientContext)
        assert ctx.client is rest_pool._pools["search"]
    assert rest_pool.metrics.requests_served >= 1


def test_rest_pool_get_metrics(rest_pool):
    m = rest_pool.get_metrics()
    assert isinstance(m, PoolMetrics)
    assert m.health_status == PoolHealth.HEALTHY
    assert m.last_health_check > 0


def test_rest_pool_map_operation_default(rest_pool):
    assert rest_pool._map_operation_to_pool("search_vectors") == "search"
    assert rest_pool._map_operation_to_pool("delete_vector") == "write"
    assert rest_pool._map_operation_to_pool("get_collection") == "read"
    assert rest_pool._map_operation_to_pool("weird") == "read"


def test_rest_pool_close(rest_pool):
    clients = list(rest_pool._pools.values())
    rest_pool.close()
    assert rest_pool._pools == {}
    assert all(c.closed for c in clients)


def test_rest_pool_close_suppresses_errors(rest_pool):
    bad = list(rest_pool._pools.values())[0]

    def boom():
        raise RuntimeError("close failed")

    bad.close = boom
    # Should not raise.
    rest_pool.close()
    assert rest_pool._pools == {}
