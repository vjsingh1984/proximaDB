"""Framework-agnostic HTTP client for the ProximaDB GraphWalk endpoints.

This is the small, testable core that LangChain / CrewAI / LlamaIndex
tool wrappers sit on top of (TD-046 follow-up). The client takes a
``base_url`` and an injected ``transport`` callable so:

- The path/body construction can be exercised without a live HTTP
  server (use :class:`StubTransport` in tests).
- Production callers can plug in any HTTP backend (``httpx``,
  ``requests``, ``aiohttp``) without dragging it into the SDK's
  required dependencies.

The actual schema (path layout, required arguments, hallucinated-key
stripping) lives in :mod:`proximadb_sdk.integrations.mcp_tools` —
this client merely composes the renderer with a transport. There is
exactly one source of truth for the wire contract.

Robustness contract:

- Transport-level errors (connection refused, timeouts) are wrapped
  in :class:`GraphWalkError` so callers don't have to care which
  backend raised what.
- ``mcp_tools.render_invocation`` exceptions (missing required args,
  unknown tool) propagate as ``ValueError`` -- they indicate a caller
  bug and the message names the offending argument.
- Trailing slashes on ``base_url`` are normalized so URLs are always
  well-formed (``http://x:5678/`` + ``/api/v1/...`` →
  ``http://x:5678/api/v1/...``, not ``http://x:5678//api/v1/...``).
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any, Protocol

from proximadb_sdk.integrations.mcp_tools import render_invocation


class Transport(Protocol):
    """HTTP transport injected into :class:`GraphWalkClient`.

    Production wrappers will be a closure around ``httpx.Client`` or
    similar. Tests use :class:`StubTransport`.
    """

    def __call__(
        self, method: str, url: str, json: dict[str, Any]
    ) -> dict[str, Any]: ...


class GraphWalkError(RuntimeError):
    """Raised when the underlying HTTP transport fails.

    Wraps the original exception so callers handling
    ``GraphWalkError`` don't need to import every transport's
    error type. Attribute ``__cause__`` is preserved.
    """


@dataclass
class StubTransport:
    """Test-only transport that records every call and returns a
    canned response. Use this in unit tests for any wrapper built on
    top of :class:`GraphWalkClient`."""

    response: dict[str, Any] = field(default_factory=dict)
    calls: list[tuple[str, str, dict[str, Any]]] = field(default_factory=list)

    def __call__(self, method: str, url: str, json: dict[str, Any]) -> dict[str, Any]:
        self.calls.append((method, url, dict(json)))
        return self.response


class GraphWalkClient:
    """Compose :func:`mcp_tools.render_invocation` with an HTTP
    transport to fire ProximaDB GraphWalk requests.

    Parameters:
        base_url: ProximaDB server URL. Trailing slashes are normalized.
        transport: Callable ``(method, url, json) -> response_dict``.
            Inject your HTTP backend of choice. Defaults to a stub
            that raises -- you must supply one in production.
    """

    def __init__(self, base_url: str, transport: Transport | None = None) -> None:
        self._base_url = base_url.rstrip("/")
        if transport is None:
            transport = self._no_transport_configured  # type: ignore[assignment]
        self._transport: Transport = transport

    @staticmethod
    def _no_transport_configured(
        method: str, url: str, json: dict[str, Any]
    ) -> dict[str, Any]:
        raise GraphWalkError(
            "GraphWalkClient: no transport configured. Pass a "
            "transport= callable (e.g. httpx.Client.post wrapper) "
            "or use StubTransport in tests."
        )

    # ---- typed convenience methods ---------------------------------

    def walk(
        self,
        *,
        graph_id: str,
        start_node_id: str,
        max_depth: int = 2,
        limit: int = 100,
    ) -> dict[str, Any]:
        """Bounded BFS expansion. See :data:`mcp_tools.GRAPH_WALK_TOOL`
        for the full schema and parameter semantics."""
        return self.invoke(
            "graph_walk",
            {
                "graph_id": graph_id,
                "start_node_id": start_node_id,
                "max_depth": max_depth,
                "limit": limit,
            },
        )

    def step(
        self,
        *,
        graph_id: str,
        node_id: str,
        edge_type: str | None = None,
        limit: int = 50,
    ) -> dict[str, Any]:
        """Single-step neighbor expansion. See
        :data:`mcp_tools.GRAPH_STEP_TOOL` for the full schema."""
        args: dict[str, Any] = {
            "graph_id": graph_id,
            "node_id": node_id,
            "limit": limit,
        }
        # Only include edge_type when explicitly set so the server
        # treats omission as "all edges" rather than as an empty
        # filter. mcp_tools.render_invocation strips unknown keys but
        # also forwards declared ones; better to not declare it at
        # all when it's not meaningful.
        if edge_type is not None and edge_type != "":
            args["edge_type"] = edge_type
        return self.invoke("graph_step", args)

    # ---- low-level dispatch ---------------------------------------

    def invoke(self, tool_name: str, arguments: Mapping[str, Any]) -> dict[str, Any]:
        """Generic dispatch: call any registered tool by name.

        This is the entry point an LLM agent's tool-call loop hits
        when it has the tool name as a string and the arguments as a
        dict. Argument validation and path/body construction live in
        :func:`render_invocation`; we only handle the HTTP turn.
        """
        method, path, body = render_invocation(tool_name, arguments)
        url = f"{self._base_url}{path}"
        try:
            return self._transport(method, url, body)
        except GraphWalkError:
            # Already wrapped (e.g. by _no_transport_configured); pass
            # through without double-wrapping.
            raise
        except Exception as exc:
            raise GraphWalkError(str(exc)) from exc


__all__ = [
    "GraphWalkClient",
    "GraphWalkError",
    "StubTransport",
    "Transport",
]
