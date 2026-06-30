"""OpenAPI contract gate for the Python REST SDK.

For every covered SDK method, this test:
  1. Monkey-patches ``_make_request`` to capture the outbound request and
     return a programmed minimal-valid response.
  2. Calls the SDK method with a sample input.
  3. Validates the captured request body / query parameters against the
     corresponding OpenAPI operation defined in
     ``docs/openapi/proximadb-openapi.yaml``.

It runs entirely offline — no server is started. Drift between the SDK and
the OpenAPI contract therefore fails this test deterministically.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import httpx
import pytest
import yaml
from jsonschema import Draft202012Validator, RefResolver

from proximadb_sdk.models import CollectionConfig, ColumnDefinition, SchemaDefinition
from proximadb_sdk.protocols.rest_sync import ProximaDBClient

# ---------------------------------------------------------------------------
# Spec loading
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[4]
SPEC_PATH = REPO_ROOT / "docs" / "openapi" / "proximadb-openapi.yaml"


@pytest.fixture(scope="module")
def openapi_spec() -> dict[str, Any]:
    assert SPEC_PATH.exists(), f"OpenAPI spec missing at {SPEC_PATH}"
    with SPEC_PATH.open() as f:
        return yaml.safe_load(f)


@pytest.fixture(scope="module")
def ref_resolver(openapi_spec):
    return RefResolver.from_schema(openapi_spec)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _operation_for(
    spec: dict[str, Any], path_template: str, method: str
) -> dict[str, Any]:
    paths = spec.get("paths", {})
    assert path_template in paths, f"Path {path_template} not in OpenAPI spec"
    op = paths[path_template].get(method.lower())
    assert op is not None, f"{method} {path_template} not defined in spec"
    return op


def _path_parameters(spec: dict[str, Any], path_template: str) -> list[dict[str, Any]]:
    """Return parameter objects declared at the path level (shared params)."""
    raw = spec["paths"][path_template].get("parameters", [])
    resolved: list[dict[str, Any]] = []
    for p in raw:
        if "$ref" in p:
            resolved.append(_resolve_ref(spec, p["$ref"]))
        else:
            resolved.append(p)
    return resolved


def _resolve_ref(spec: dict[str, Any], ref: str) -> dict[str, Any]:
    assert ref.startswith("#/"), f"unsupported $ref form: {ref}"
    node: Any = spec
    for part in ref[2:].split("/"):
        node = node[part]
    return node


def _request_body_schema(
    spec: dict[str, Any], operation: dict[str, Any]
) -> dict[str, Any] | None:
    body = operation.get("requestBody")
    if not body:
        return None
    schema = body["content"]["application/json"]["schema"]
    if "$ref" in schema:
        schema = _resolve_ref(spec, schema["$ref"])
    return schema


def _validate(schema: dict[str, Any], instance: Any, resolver: RefResolver) -> None:
    Draft202012Validator(schema, resolver=resolver).validate(instance)


# ---------------------------------------------------------------------------
# Capturing transport
# ---------------------------------------------------------------------------


class _CapturedRequest:
    def __init__(self) -> None:
        self.method: str | None = None
        self.endpoint: str | None = None
        self.json_body: Any = None
        self.params: dict[str, Any] | None = None


def _patched_client(
    monkeypatch, response_body: Any, status_code: int = 200
) -> tuple[ProximaDBClient, _CapturedRequest]:
    client = ProximaDBClient(url="http://contract.test")
    captured = _CapturedRequest()

    def fake_make_request(self, method: str, endpoint: str, **kwargs):  # noqa: ARG001
        captured.method = method
        captured.endpoint = endpoint
        captured.json_body = kwargs.get("json")
        captured.params = kwargs.get("params")
        return httpx.Response(
            status_code=status_code,
            request=httpx.Request(method, f"http://contract.test{endpoint}"),
            content=json.dumps(response_body).encode("utf-8"),
            headers={"content-type": "application/json"},
        )

    monkeypatch.setattr(ProximaDBClient, "_make_request", fake_make_request)
    return client, captured


# ---------------------------------------------------------------------------
# Per-operation contract checks
# ---------------------------------------------------------------------------


def test_get_health_live_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    response_body = {"status": "ok"}
    client, captured = _patched_client(monkeypatch, response_body)

    probe = client.live()

    # Operation exists; path matches.
    op = _operation_for(openapi_spec, "/health/live", "get")
    assert op["operationId"] == "getLiveness"
    assert captured.method == "GET"
    assert captured.endpoint == "/health/live"

    # Response shape conforms.
    resp_schema = op["responses"]["200"]["content"]["application/json"]["schema"]
    if "$ref" in resp_schema:
        resp_schema = _resolve_ref(openapi_spec, resp_schema["$ref"])
    _validate(resp_schema, probe.model_dump(), ref_resolver)


def test_get_health_ready_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    response_body = {"status": "ready"}
    client, captured = _patched_client(monkeypatch, response_body)

    probe = client.ready()

    op = _operation_for(openapi_spec, "/health/ready", "get")
    assert op["operationId"] == "getReadiness"
    assert captured.method == "GET"
    assert captured.endpoint == "/health/ready"

    resp_schema = op["responses"]["200"]["content"]["application/json"]["schema"]
    if "$ref" in resp_schema:
        resp_schema = _resolve_ref(openapi_spec, resp_schema["$ref"])
    _validate(resp_schema, probe.model_dump(), ref_resolver)


def test_create_collection_request_matches_spec(
    monkeypatch, openapi_spec, ref_resolver
):
    response_body = {
        "collection_id": "col_abc",
        "name": "my_collection",
        "dimension": 128,
        "engine": "viper",
        "proxima_record_enabled": True,
        "created_at": "2026-05-23T00:00:00Z",
    }
    client, captured = _patched_client(monkeypatch, response_body)

    schema = SchemaDefinition(
        columns=[
            ColumnDefinition(name="title", data_type="text"),
            ColumnDefinition(name="price", data_type="float", filterable=True),
        ]
    )
    client.create_collection(
        "my_collection",
        config=CollectionConfig(name="my_collection", dimension=128),
        schema=schema,
        initial_capacity=10_000,
    )

    op = _operation_for(openapi_spec, "/api/v2/collections", "post")
    assert op["operationId"] == "createCollection"
    assert captured.method == "POST"
    assert captured.endpoint == "/api/v2/collections"

    req_schema = _request_body_schema(openapi_spec, op)
    assert req_schema is not None
    _validate(req_schema, captured.json_body, ref_resolver)

    # Ensure the new pass-throughs actually landed in the body.
    assert captured.json_body["initial_capacity"] == 10_000
    assert "columns" in captured.json_body["schema"]


def test_list_collections_query_params_match_spec(monkeypatch, openapi_spec):
    response_body = {"collections": [], "total_count": 0}
    client, captured = _patched_client(monkeypatch, response_body)

    client.list_collections(limit=25, offset=50, include_stats=True)

    op = _operation_for(openapi_spec, "/api/v2/collections", "get")
    assert op["operationId"] == "listCollections"
    assert captured.method == "GET"

    # Resolve declared parameters and check the SDK uses only spec-defined keys
    # with values that satisfy the parameter schemas.
    declared: dict[str, dict[str, Any]] = {}
    for p in op.get("parameters", []):
        param = _resolve_ref(openapi_spec, p["$ref"]) if "$ref" in p else p
        if param["in"] == "query":
            declared[param["name"]] = param["schema"]

    assert set(captured.params or {}).issubset(
        declared.keys()
    ), f"Unknown query params: {set(captured.params) - declared.keys()}"
    for name, schema in declared.items():
        if name in (captured.params or {}):
            value = captured.params[name]
            # Coerce SDK-supplied booleans serialized as 'true'/'false' strings
            if schema.get("type") == "boolean" and isinstance(value, str):
                value = value.lower() == "true"
            Draft202012Validator(schema).validate(value)


def test_get_schema_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    response_body = {
        "schema_id": "sch_1",
        "schema_version": "v1",
        "collection_id": "col_abc",
        "schema": {
            "columns": [
                {"name": "title", "data_type": "text"},
            ],
        },
        "created_at": "2026-05-23T00:00:00Z",
    }
    client, captured = _patched_client(monkeypatch, response_body)

    schema_resp = client.get_schema("col_abc")

    op = _operation_for(
        openapi_spec, "/api/v2/collections/{collection_id}/schema", "get"
    )
    assert op["operationId"] == "getCollectionSchema"
    assert captured.method == "GET"
    assert captured.endpoint == "/api/v2/collections/col_abc/schema"

    resp_schema = op["responses"]["200"]["content"]["application/json"]["schema"]
    if "$ref" in resp_schema:
        resp_schema = _resolve_ref(openapi_spec, resp_schema["$ref"])

    # by_alias=True so the `schema_` field re-emits as `schema` per OpenAPI;
    # exclude_none avoids emitting Python None for unset optionals (OpenAPI
    # spec does not mark them nullable).
    _validate(
        resp_schema,
        schema_resp.model_dump(by_alias=True, exclude_none=True),
        ref_resolver,
    )


def test_update_schema_request_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    response_body = {
        "schema_id": "sch_2",
        "schema_version": "v2",
        "previous_schema_id": "sch_1",
        "changes": [],
        "warnings": [],
        "updated_at": "2026-05-23T00:00:00Z",
    }
    client, captured = _patched_client(monkeypatch, response_body)

    schema = SchemaDefinition(
        columns=[ColumnDefinition(name="title", data_type="text")],
        enforcement="strict",
    )
    client.update_schema("col_abc", schema, force=True)

    op = _operation_for(
        openapi_spec, "/api/v2/collections/{collection_id}/schema", "put"
    )
    assert op["operationId"] == "updateCollectionSchema"
    assert captured.method == "PUT"
    assert captured.endpoint == "/api/v2/collections/col_abc/schema"

    req_schema = _request_body_schema(openapi_spec, op)
    assert req_schema is not None
    _validate(req_schema, captured.json_body, ref_resolver)
    assert captured.json_body["force"] is True


def test_create_graph_request_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    # Proves the rebased graph create_graph op (full-model round-trip through
    # CreateGraphRequest) emits a spec-conformant method/URL/body via the
    # generated REST client.
    response_body = {"graph_id": "social", "name": "Social", "node_count": 0}
    client, captured = _patched_client(monkeypatch, response_body)

    client.create_graph("social", name="Social", description="people")

    op = _operation_for(openapi_spec, "/api/v2/graphs", "post")
    assert op["operationId"] == "createGraph"
    assert captured.method == "POST"
    assert captured.endpoint == "/api/v2/graphs"

    req_schema = _request_body_schema(openapi_spec, op)
    assert req_schema is not None
    _validate(req_schema, captured.json_body, ref_resolver)
    assert captured.json_body["graph_id"] == "social"


def test_create_node_request_matches_spec(monkeypatch, openapi_spec, ref_resolver):
    # Proves the rebased graph create_node op (sourced via _path_only because the
    # public API's bare-list embedding does not conform to EmbeddingInput) still
    # emits the spec-governed method/URL and a spec-conformant body when no
    # embedding is supplied.
    response_body = {"id": "n1", "labels": ["Person"]}
    client, captured = _patched_client(monkeypatch, response_body)

    client.create_node("n1", ["Person"], properties={"age": 30}, graph_id="social")

    op = _operation_for(openapi_spec, "/api/v2/graphs/{graph_id}/nodes", "post")
    assert op["operationId"] == "createNode"
    assert captured.method == "POST"
    assert captured.endpoint == "/api/v2/graphs/social/nodes"

    req_schema = _request_body_schema(openapi_spec, op)
    assert req_schema is not None
    _validate(req_schema, captured.json_body, ref_resolver)
    assert captured.json_body["node"]["id"] == "n1"
