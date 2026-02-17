import numpy as np

from proximadb_sdk.models import IncludeFields, SearchQuery, VectorSearchRequest
from proximadb_sdk.protocols.rest_sync import ProximaDBClient


def test_convert_metadata_to_rest_format_roundtrip():
    client = ProximaDBClient(url="http://localhost:5678")
    md = {"a": "x", "b": 1, "c": 3.14, "d": True}
    result = client._convert_metadata_to_rest_format(md)
    # Result is a dict mapping keys to SqlValue dicts
    assert isinstance(result, dict)
    # Ensure keys preserved
    assert set(result.keys()) == set(md.keys())
    # Each SqlValue should have exactly one typed value
    for key, sql_value in result.items():
        typed = [
            k
            for k in ("string_value", "int64_value", "number_value", "bool_value")
            if k in sql_value
        ]
        assert len(typed) == 1


def test_search_request_model_dump_excludes_none():
    q = SearchQuery(vector=[0.1, 0.2, 0.3])
    req = VectorSearchRequest(
        collection_id="col",
        queries=[q],
        top_k=5,
        include_fields=IncludeFields(vector=False, metadata=True),
    )
    payload = req.model_dump(exclude_none=True)
    assert "distance_metric_override" not in payload
    assert payload["collection_id"] == "col"
    assert payload["top_k"] == 5
    assert isinstance(payload["queries"][0]["vector"], list)


def test_normalize_vectors_numpy_to_list():
    client = ProximaDBClient(url="http://localhost:5678")
    arr = np.array([[1.0, 2.0], [3.0, 4.0]], dtype=np.float32)
    out = client._normalize_vectors(arr)
    assert isinstance(out, list)
    assert out[0] == [1.0, 2.0]
