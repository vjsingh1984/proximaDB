"""Tests for the framework-agnostic GraphWalk HTTP client.

This is the small core that LangChain / CrewAI / LlamaIndex wrappers
sit on top of (TD-046 follow-up). The client takes a ``base_url`` and
a ``transport`` callable so we can exercise the path/body construction
without standing up a real HTTP server -- the transport is the test
seam.
"""

from __future__ import annotations

from typing import Any

import pytest

from proximadb_sdk.integrations.graph_walk_client import (  # noqa: E402
    GraphWalkClient,
    GraphWalkError,
    StubTransport,
)

# ---- happy path: walk -----------------------------------------------


def test_walk_builds_correct_request() -> None:
    transport = StubTransport(
        response={"data": {"nodes": [{"id": "n1"}, {"id": "n2"}]}}
    )
    client = GraphWalkClient("http://proximadb:5678", transport=transport)

    result = client.walk(graph_id="ontology", start_node_id="n1", max_depth=2, limit=50)

    # The mcp_tools renderer is the single source of truth for the URL
    # path and body shape; this test pins that the client uses it.
    assert transport.calls == [
        (
            "POST",
            "http://proximadb:5678/api/v1/graph/graphs/ontology/walk",
            {"start_node_id": "n1", "max_depth": 2, "limit": 50},
        )
    ]
    assert result == {"data": {"nodes": [{"id": "n1"}, {"id": "n2"}]}}


def test_step_builds_correct_request() -> None:
    transport = StubTransport(response={"data": {"nodes": [{"id": "n1"}]}})
    client = GraphWalkClient("http://proximadb:5678", transport=transport)

    result = client.step(graph_id="g", node_id="n1", edge_type="REFERS_TO", limit=10)

    assert transport.calls == [
        (
            "POST",
            "http://proximadb:5678/api/v1/graph/graphs/g/step",
            {"node_id": "n1", "edge_type": "REFERS_TO", "limit": 10},
        )
    ]
    assert "data" in result


def test_step_omits_empty_edge_type() -> None:
    """``edge_type`` is optional. Passing ``None`` must NOT include the
    field in the body -- otherwise the server might reject it as an
    explicit empty filter when the caller really meant "all edges"."""
    transport = StubTransport(response={"data": {}})
    client = GraphWalkClient("http://x", transport=transport)

    client.step(graph_id="g", node_id="n1", limit=5)

    _, _, body = transport.calls[0]
    assert "edge_type" not in body
    assert body == {"node_id": "n1", "limit": 5}


# ---- base URL hygiene -----------------------------------------------


def test_base_url_trailing_slash_stripped() -> None:
    """A trailing slash on base_url + a leading slash on the path
    would produce ``//api/v1/...``. The client must normalize one or
    the other so URLs are always well-formed."""
    transport = StubTransport(response={"data": {}})
    client = GraphWalkClient("http://x:5678/", transport=transport)

    client.walk(graph_id="g", start_node_id="n", max_depth=1, limit=10)

    _, url, _ = transport.calls[0]
    assert url == "http://x:5678/api/v1/graph/graphs/g/walk"


# ---- error propagation ----------------------------------------------


def test_transport_error_is_wrapped() -> None:
    """Network errors from the underlying transport are wrapped in
    GraphWalkError so callers don't have to care which transport
    backend raised what (httpx vs requests vs aiohttp vs stub)."""

    class FailingTransport:
        def __call__(self, method: str, url: str, json: dict[str, Any]) -> dict:
            raise ConnectionError("backend down")

    client = GraphWalkClient("http://x", transport=FailingTransport())
    with pytest.raises(GraphWalkError, match="backend down"):
        client.walk(graph_id="g", start_node_id="n", max_depth=1, limit=10)


def test_invalid_argument_passes_through_render_invocation() -> None:
    """``mcp_tools.render_invocation`` raises ValueError on missing
    required args. The client must surface that as a clear error
    rather than swallowing it or producing a malformed request."""
    transport = StubTransport(response={"data": {}})
    client = GraphWalkClient("http://x", transport=transport)

    # Caller forgets graph_id -- handled by the client's typed
    # signature in normal use, but render_invocation also enforces.
    # Here we exercise the underlying invariant via the lower-level
    # `invoke` API.
    with pytest.raises(ValueError):
        client.invoke("graph_walk", {"start_node_id": "n"})


# ---- discovery / observability --------------------------------------


def test_invoke_supports_either_tool_name() -> None:
    """``invoke(name, args)`` is the dispatch primitive an agent uses
    when it has the tool name as a string. Both registered names must
    work."""
    transport = StubTransport(response={"data": {}})
    client = GraphWalkClient("http://x", transport=transport)

    client.invoke("graph_walk", {"graph_id": "g", "start_node_id": "n"})
    client.invoke("graph_step", {"graph_id": "g", "node_id": "n"})

    paths = [c[1] for c in transport.calls]
    assert paths[0].endswith("/walk")
    assert paths[1].endswith("/step")


def test_stub_transport_records_each_call() -> None:
    """StubTransport is the test seam every wrapper test will rely on.
    Verify it actually records calls in order (regression guard)."""
    transport = StubTransport(response={"data": {}})
    client = GraphWalkClient("http://x", transport=transport)

    client.walk(graph_id="a", start_node_id="x", max_depth=1, limit=1)
    client.walk(graph_id="b", start_node_id="y", max_depth=1, limit=1)

    assert len(transport.calls) == 2
    assert "graphs/a/walk" in transport.calls[0][1]
    assert "graphs/b/walk" in transport.calls[1][1]
