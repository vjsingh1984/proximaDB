"""Tests for the MCP / OpenAI / Anthropic tool-call schema for GraphWalk.

These schemas are the machine-readable contract that agent runtimes
consume to register ProximaDB graph traversal as a callable tool. The
tests pin both the *shape* (so regressions break loudly) and the
*content invariants* (names, required parameters, parameter types).

TD-046 follow-up. Paired with the REST endpoints
``POST /api/v1/graph/graphs/:graph_id/walk`` and ``/step``.
"""

from __future__ import annotations

import json

import pytest

# Module-under-test ---------------------------------------------------
from proximadb_sdk.integrations.mcp_tools import (  # noqa: E402
    GRAPH_STEP_TOOL,
    GRAPH_WALK_TOOL,
    list_tools,
    render_invocation,
)

# ---- shape: each tool is a valid MCP/OpenAI function spec ----------


@pytest.mark.parametrize("tool", [GRAPH_WALK_TOOL, GRAPH_STEP_TOOL])
def test_tool_has_name_and_description(tool: dict) -> None:
    assert isinstance(tool["name"], str) and tool["name"]
    assert isinstance(tool["description"], str) and len(tool["description"]) > 20


@pytest.mark.parametrize("tool", [GRAPH_WALK_TOOL, GRAPH_STEP_TOOL])
def test_tool_input_schema_is_jsonschema_object(tool: dict) -> None:
    schema = tool["input_schema"]
    assert schema["type"] == "object"
    assert isinstance(schema["properties"], dict)
    assert isinstance(schema["required"], list)


@pytest.mark.parametrize("tool", [GRAPH_WALK_TOOL, GRAPH_STEP_TOOL])
def test_tool_round_trips_through_json(tool: dict) -> None:
    """Every tool must serialize to JSON cleanly. Agent runtimes consume
    JSON, so a non-serializable value here is an immediate prod bug."""
    text = json.dumps(tool)
    assert json.loads(text) == tool


# ---- content invariants: walk tool ---------------------------------


def test_walk_tool_required_args() -> None:
    schema = GRAPH_WALK_TOOL["input_schema"]
    assert set(schema["required"]) == {"graph_id", "start_node_id"}
    # Optional args present in properties for tooling autocompletion
    assert "max_depth" in schema["properties"]
    assert "limit" in schema["properties"]


def test_walk_tool_parameter_types() -> None:
    props = GRAPH_WALK_TOOL["input_schema"]["properties"]
    assert props["graph_id"]["type"] == "string"
    assert props["start_node_id"]["type"] == "string"
    assert props["max_depth"]["type"] == "integer"
    assert props["limit"]["type"] == "integer"
    # Min bounds protect agents from passing nonsense
    assert props["max_depth"]["minimum"] == 0
    assert props["limit"]["minimum"] == 1


# ---- content invariants: step tool ---------------------------------


def test_step_tool_required_args() -> None:
    schema = GRAPH_STEP_TOOL["input_schema"]
    assert set(schema["required"]) == {"graph_id", "node_id"}
    assert "edge_type" in schema["properties"]
    assert "limit" in schema["properties"]


def test_step_tool_edge_type_is_optional_string() -> None:
    props = GRAPH_STEP_TOOL["input_schema"]["properties"]
    assert props["edge_type"]["type"] == "string"
    # edge_type is opt-in (empty string = all edges); not in required.
    assert "edge_type" not in GRAPH_STEP_TOOL["input_schema"]["required"]


# ---- discovery API ------------------------------------------------


def test_list_tools_returns_both() -> None:
    tools = list_tools()
    names = [t["name"] for t in tools]
    assert "graph_walk" in names
    assert "graph_step" in names
    assert len(tools) == 2


def test_list_tools_returns_independent_copies() -> None:
    """Mutation of a returned tool dict must not affect the canonical
    constants. This protects against an agent runtime that decorates
    the schema in-place from corrupting future calls."""
    tools = list_tools()
    tools[0]["name"] = "tampered"
    tools_again = list_tools()
    assert all(t["name"] in {"graph_walk", "graph_step"} for t in tools_again)


# ---- invocation rendering -----------------------------------------


def test_render_invocation_walk_produces_valid_request() -> None:
    """The invocation rendering takes an agent's tool-call payload and
    produces the (HTTP method, path, body) tuple the SDK or any HTTP
    client can fire. This is the bit that prevents agents from hitting
    the wrong URL."""
    method, path, body = render_invocation(
        "graph_walk",
        {"graph_id": "ontology", "start_node_id": "n1", "max_depth": 2, "limit": 50},
    )
    assert method == "POST"
    assert path == "/api/v1/graph/graphs/ontology/walk"
    assert body == {"start_node_id": "n1", "max_depth": 2, "limit": 50}
    # graph_id must NOT be in the body -- it's a path param.
    assert "graph_id" not in body


def test_render_invocation_step_produces_valid_request() -> None:
    method, path, body = render_invocation(
        "graph_step",
        {
            "graph_id": "ontology",
            "node_id": "n1",
            "edge_type": "REFERS_TO",
            "limit": 10,
        },
    )
    assert method == "POST"
    assert path == "/api/v1/graph/graphs/ontology/step"
    assert body == {"node_id": "n1", "edge_type": "REFERS_TO", "limit": 10}
    assert "graph_id" not in body


def test_render_invocation_unknown_tool_raises() -> None:
    with pytest.raises(ValueError, match="unknown tool"):
        render_invocation("graph_teleport", {"graph_id": "x"})


def test_render_invocation_missing_required_arg_raises() -> None:
    # Walk needs both graph_id and start_node_id.
    with pytest.raises(ValueError, match="start_node_id"):
        render_invocation("graph_walk", {"graph_id": "ontology"})


def test_render_invocation_strips_unknown_args() -> None:
    """Agents sometimes hallucinate extra keys. The renderer must drop
    them so the server doesn't see surprise fields and reject the
    request."""
    _, _, body = render_invocation(
        "graph_walk",
        {
            "graph_id": "g",
            "start_node_id": "n",
            "max_depth": 1,
            "limit": 10,
            "halt_if_paradox_detected": True,  # not in schema
        },
    )
    assert "halt_if_paradox_detected" not in body
