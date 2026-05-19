"""REST SDK record-ingest migration tests."""

from unittest.mock import Mock, patch

import numpy as np
import pytest

from proximadb_sdk.models_v2 import ProximaRecord, TypedValue
from proximadb_sdk.protocols.rest_sync import ProximaDBClient


class JsonResponse:
    def __init__(self, payload):
        self._payload = payload

    def json(self):
        return self._payload


def make_client():
    with patch.object(ProximaDBClient, "_create_http_client", return_value=Mock()):
        return ProximaDBClient(url="http://localhost:5678")


def test_insert_records_posts_v2_record_payload():
    client = make_client()
    client._make_request = Mock(
        return_value=JsonResponse(
            {"inserted_count": 1, "failed_count": 0, "inserted_ids": ["r1"]}
        )
    )

    result = client.insert_records(
        "items",
        [
            {
                "id": "r1",
                "vector": np.array([0.1, 0.2], dtype=np.float32),
                "props": {"kind": "note", "payload": {"x": 1}},
            }
        ],
    )

    assert result.success == 1
    method, endpoint = client._make_request.call_args.args
    payload = client._make_request.call_args.kwargs["json"]
    assert method == "POST"
    assert endpoint == "/api/v2/collections/items/records/batch"
    assert payload["records"][0]["id"] == "r1"
    assert payload["records"][0]["vector"] == pytest.approx([0.1, 0.2])
    assert payload["records"][0]["props"]["payload"] == {
        "type": "jsonb",
        "value": {"x": 1},
    }


def test_insert_vectors_is_record_endpoint_alias():
    client = make_client()
    client._make_request = Mock(
        return_value=JsonResponse(
            {"inserted_count": 2, "failed_count": 0, "inserted_ids": ["a", "b"]}
        )
    )

    result = client.insert_vectors(
        "items",
        np.array([[1.0, 2.0], [3.0, 4.0]], dtype=np.float32),
        ids=["a", "b"],
        metadata=[{"tenant": "acme"}, {"tenant": "beta"}],
    )

    assert result.success == 2
    endpoint = client._make_request.call_args.args[1]
    payload = client._make_request.call_args.kwargs["json"]
    assert endpoint == "/api/v2/collections/items/records/batch"
    assert payload["records"] == [
        {"id": "a", "vector": [1.0, 2.0], "props": {"tenant": "acme"}},
        {"id": "b", "vector": [3.0, 4.0], "props": {"tenant": "beta"}},
    ]


def test_models_v2_proximarecord_normalizes_typed_fields():
    client = make_client()
    client._make_request = Mock(
        return_value=JsonResponse(
            {"inserted_count": 1, "failed_count": 0, "inserted_ids": ["p1"]}
        )
    )

    record = ProximaRecord(
        id="p1",
        vector=[0.1, 0.2],
        typed_fields={"price": TypedValue.float_(9.99)},
    )

    client.insert_records("items", [record])

    props = client._make_request.call_args.kwargs["json"]["records"][0]["props"]
    assert props["price"] == {"type": "float", "value": 9.99}


def test_delete_vector_uses_record_delete_endpoint():
    client = make_client()
    client._make_request = Mock(return_value=JsonResponse({"success": True, "id": "r1"}))

    result = client.delete_vector("items", "r1")

    assert result.success is True
    assert result.deleted_count == 1
    assert client._make_request.call_args.args == (
        "DELETE",
        "/api/v2/collections/items/records/r1",
    )
