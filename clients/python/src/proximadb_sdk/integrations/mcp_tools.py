"""Agent tool schemas for ProximaDB GraphWalk (TD-046 follow-up).

This module is the canonical machine-readable description of the
ProximaDB graph-traversal *tools* an LLM agent registers — currently
``graph_walk`` (bounded BFS in one call) and ``graph_step`` (one-hop
neighbors of a single node, called repeatedly when the agent needs to
drive traversal). It is paired with the REST endpoints

    POST /api/v1/graph/graphs/{graph_id}/walk
    POST /api/v1/graph/graphs/{graph_id}/step

The schemas use the Anthropic / OpenAI ``function``-style spec
(``name`` + ``description`` + ``input_schema`` JSON Schema). MCP
runtimes consume the same shape via ``tools/list``.

Why a Python module instead of a static JSON file? Two reasons:

1. **Validation lives next to the spec.** The :func:`render_invocation`
   helper turns an agent's tool-call payload into a concrete
   ``(method, path, body)`` triple, applying schema-level filtering
   (drops keys not declared in the schema) and required-arg checks.
   Agents that hallucinate fields stay quietly contained instead of
   producing a 400 from the server.
2. **The constants are deep-copied on read.** Some agent harnesses
   decorate the schema in-place (adding tracing, retry annotations,
   etc.). Returning fresh copies through :func:`list_tools` prevents
   one harness from corrupting another's view.

Both goals are exercised by the test suite under
``clients/python/tests/integrations/test_mcp_tools.py``.
"""

from __future__ import annotations

import copy
from collections.abc import Mapping
from typing import Any

# ----------------------------------------------------------------------
# Tool definitions
#
# The ``input_schema`` follows JSON Schema draft-07. Required fields are
# the smallest set that yields a valid request; optional fields carry
# ``minimum`` / ``maximum`` bounds where the server enforces a range so
# agents can avoid obvious-bad calls without a round trip.
# ----------------------------------------------------------------------

GRAPH_WALK_TOOL: dict[str, Any] = {
    "name": "graph_walk",
    "description": (
        "Bounded breadth-first exploration of a ProximaDB graph from a "
        "starting node. Returns up to `limit` nodes within `max_depth` "
        "hops. Use this when you can predict roughly how many nodes you "
        "want and depth is a useful budget; use `graph_step` instead when "
        "you need to inspect each neighbor before deciding where to go."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "graph_id": {
                "type": "string",
                "description": "Identifier of the graph collection to walk.",
            },
            "start_node_id": {
                "type": "string",
                "description": "ID of the node to start the BFS from.",
            },
            "max_depth": {
                "type": "integer",
                "minimum": 0,
                "description": (
                    "Maximum BFS depth (0 means unbounded; cap with `limit`)."
                ),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum number of nodes to return.",
            },
        },
        "required": ["graph_id", "start_node_id"],
    },
}


GRAPH_STEP_TOOL: dict[str, Any] = {
    "name": "graph_step",
    "description": (
        "Single-step graph navigation: return the immediate neighbors of "
        "one node, optionally filtered by edge type. Call this in a loop "
        "when the agent needs to drive traversal step by step so the "
        "database never has to materialize a subgraph that won't fit in "
        "the agent's context window. The starting node is included in "
        "the response so the agent has its own properties available."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "graph_id": {
                "type": "string",
                "description": "Identifier of the graph collection.",
            },
            "node_id": {
                "type": "string",
                "description": "ID of the node whose neighbors to return.",
            },
            "edge_type": {
                "type": "string",
                "description": (
                    "Optional edge-type filter (omit or empty string to "
                    "follow all edges)."
                ),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum number of neighbors to return.",
            },
        },
        "required": ["graph_id", "node_id"],
    },
}


_TOOLS: dict[str, dict[str, Any]] = {
    GRAPH_WALK_TOOL["name"]: GRAPH_WALK_TOOL,
    GRAPH_STEP_TOOL["name"]: GRAPH_STEP_TOOL,
}


def list_tools() -> list[dict[str, Any]]:
    """Return all registered tool schemas as deep copies.

    The deep copy is deliberate: it lets a caller decorate the returned
    dicts (e.g. add ``cache_control`` for Anthropic prompt caching) without
    corrupting the canonical constants.
    """
    return [copy.deepcopy(tool) for tool in _TOOLS.values()]


# ----------------------------------------------------------------------
# Invocation rendering
# ----------------------------------------------------------------------


def render_invocation(
    tool_name: str, arguments: Mapping[str, Any]
) -> tuple[str, str, dict[str, Any]]:
    """Translate an agent's tool-call payload into an HTTP request triple.

    Returns ``(method, path, body)``. The ``graph_id`` argument is moved
    to the URL path (it identifies the collection in REST terms), and
    any keys not declared in the tool's input schema are silently
    dropped — agents occasionally hallucinate extra parameters, and
    forwarding them produces opaque 400s on the server side.

    Raises:
        ValueError: if the tool name is unknown, or any required schema
            argument is missing from ``arguments``.
    """
    if tool_name not in _TOOLS:
        raise ValueError(f"unknown tool: {tool_name!r}")
    tool = _TOOLS[tool_name]

    schema = tool["input_schema"]
    required = set(schema["required"])
    declared = set(schema["properties"].keys())

    missing = required - set(arguments.keys())
    if missing:
        # Surface a single missing key per error so the agent can fix
        # one problem at a time rather than guessing at multiple.
        raise ValueError(
            f"missing required argument(s) for {tool_name!r}: {sorted(missing)}"
        )

    if "graph_id" not in arguments:
        # Defensive: graph_id is required for both tools, but the loop
        # above already verified that. Keep this assertion to catch a
        # future schema change that drops graph_id from `required`.
        raise ValueError(f"tool {tool_name!r} requires graph_id but none was supplied")

    graph_id = arguments["graph_id"]
    suffix = "walk" if tool_name == "graph_walk" else "step"
    path = f"/api/v1/graph/graphs/{graph_id}/{suffix}"

    # Filter the body: only include declared fields, and drop graph_id
    # (it lives in the path).
    body = {k: v for k, v in arguments.items() if k in declared and k != "graph_id"}

    return ("POST", path, body)


__all__ = [
    "GRAPH_STEP_TOOL",
    "GRAPH_WALK_TOOL",
    "list_tools",
    "render_invocation",
]
