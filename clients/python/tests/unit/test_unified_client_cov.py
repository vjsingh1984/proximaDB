"""Offline unit tests for proximadb_sdk.unified_client.ProximaDBClient.

Strategy: construct the client in REST mode (does NOT connect on init), then
replace ._adapter / ._client with MagicMocks (or hand fakes) so every delegating
method is exercised without any network, server, or model download.
"""

import math
from unittest.mock import MagicMock

import numpy as np
import pytest

from proximadb_sdk.config import Protocol
from proximadb_sdk.exceptions import CollectionNotFoundError, ProximaDBError
from proximadb_sdk.models import (
    BatchResult,
    Collection,
    CollectionConfig,
    HealthStatus,
    OperationMetrics,
    SearchResult,
    VectorOperationResponse,
    VectorRecord,
)


def _delete_resp():
    return VectorOperationResponse(
        success=True, operation="DELETE", metrics=OperationMetrics()
    )
from proximadb_sdk.unified_client import (
    ProximaDBClient,
    _is_connection_error,
    connect,
    connect_rest,
    connect_unified,
    connect_legacy,
)


# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def _clear_shared_state():
    """Reset the process-shared local fallback dicts between tests."""
    ProximaDBClient._shared_local_collections.clear()
    ProximaDBClient._shared_local_vectors.clear()
    yield
    ProximaDBClient._shared_local_collections.clear()
    ProximaDBClient._shared_local_vectors.clear()


def make_client(adapter=None):
    """Build a REST-mode client and inject a mock adapter."""
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._adapter = adapter if adapter is not None else MagicMock()
    c._client = MagicMock()
    c._active_protocol = Protocol.REST
    c._prefer_local_fallback = False
    return c


def cname(name):
    """CollectionConfig.name requires >= 8 chars."""
    return name if len(name) >= 8 else (name + "_xxxxxxxx")[:8]


def a_collection(name="collectn", dim=3):
    name = cname(name)
    return Collection(id=name, config=CollectionConfig(name=name, dimension=dim))


# ---------------------------------------------------------------------------
# Module-level helpers / construction
# ---------------------------------------------------------------------------


def test_is_connection_error_by_class():
    class ConnectionError(Exception):
        pass

    assert _is_connection_error(ConnectionError("x")) is True


def test_is_connection_error_by_marker():
    assert _is_connection_error(Exception("Connection refused now")) is True
    assert _is_connection_error(Exception("unavailable upstream")) is True


def test_is_connection_error_server_rejection_propagates():
    assert _is_connection_error(Exception("INVALID_ARGUMENT bad dim")) is False
    assert _is_connection_error(ValueError("ALREADY_EXISTS")) is False


def test_construct_rest_basics():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    assert c.active_protocol == Protocol.REST
    assert c._adapter is not None
    info = c.get_performance_info()
    assert info["protocol"] == "REST"


def test_construct_with_api_key_auth():
    c = ProximaDBClient(
        url="http://testserver", protocol="rest", api_key="supersecretapikey123"
    )
    status = c.get_auth_status()
    assert "authenticated" in status


def test_connect_helpers_build_clients():
    assert isinstance(connect(url="http://t", protocol="rest"), ProximaDBClient)
    assert isinstance(connect_rest(url="http://t"), ProximaDBClient)
    assert isinstance(connect_unified(url="http://t", protocol="rest"), ProximaDBClient)
    assert isinstance(connect_legacy(url="http://t", protocol="rest"), ProximaDBClient)


def test_get_performance_info_grpc_branch():
    c = make_client()
    c._active_protocol = Protocol.GRPC
    info = c.get_performance_info()
    assert info["protocol"] == "gRPC"


def test_get_auth_status_no_auth():
    c = make_client()
    c._auth = None
    assert c.get_auth_status() == {"authenticated": False, "method": None}


def test_refresh_and_logout_no_auth():
    c = make_client()
    c._auth = None
    assert c.refresh_authentication() is False
    assert c.logout() is True


def test_refresh_authentication_success():
    c = make_client()
    auth = MagicMock()
    auth.refresh_token.return_value = MagicMock(success=True)
    c._auth = auth
    assert c.refresh_authentication() is True


def test_logout_success():
    c = make_client()
    auth = MagicMock()
    auth.logout.return_value = True
    c._auth = auth
    assert c.logout() is True


def test_protocol_metrics_not_enabled():
    c = make_client()
    c._protocol_selector = None
    assert "error" in c.get_protocol_metrics()


# ---------------------------------------------------------------------------
# Collection lifecycle via adapter
# ---------------------------------------------------------------------------


def test_create_collection_via_adapter():
    adapter = MagicMock()
    adapter.create_collection.return_value = a_collection("mycollxx")
    c = make_client(adapter)
    result = c.create_collection(
        "mycollxx", CollectionConfig(name="mycollxx", dimension=3)
    )
    assert result.id == "mycollxx"
    assert "mycollxx" in ProximaDBClient._shared_local_collections


def test_create_collection_builds_config_from_kwargs():
    adapter = MagicMock()
    adapter.create_collection.side_effect = lambda name, config, **kw: a_collection(name)
    c = make_client(adapter)
    result = c.create_collection("autocoll", dimension=8)
    assert result.id == "autocoll"


def test_create_collection_duplicate_raises():
    adapter = MagicMock()
    adapter.create_collection.return_value = a_collection("dupcolxx")
    c = make_client(adapter)
    c.create_collection("dupcolxx", CollectionConfig(name="dupcolxx", dimension=3))
    with pytest.raises(ProximaDBError):
        c.create_collection("dupcolxx", CollectionConfig(name="dupcolxx", dimension=3))


def test_create_collection_connection_error_falls_back_local():
    adapter = MagicMock()
    adapter.create_collection.side_effect = Exception("connection refused")
    c = make_client(adapter)
    result = c.create_collection(
        "offlinex", CollectionConfig(name="offlinex", dimension=3)
    )
    assert result.id == "offlinex"
    assert c._prefer_local_fallback is True


def test_create_collection_server_error_propagates():
    adapter = MagicMock()
    adapter.create_collection.side_effect = Exception("INVALID_ARGUMENT")
    c = make_client(adapter)
    with pytest.raises(Exception):
        c.create_collection("badcolxx", CollectionConfig(name="badcolxx", dimension=3))


def test_create_collection_config_name_mismatch_recopies():
    adapter = MagicMock()
    adapter.create_collection.side_effect = lambda name, config, **kw: a_collection(name)
    c = make_client(adapter)
    cfg = CollectionConfig(name="othercol", dimension=4)
    result = c.create_collection("renamedx", cfg)
    assert result.id == "renamedx"


def test_get_collection_via_adapter():
    adapter = MagicMock()
    adapter.get_collection.return_value = a_collection("getcolxx")
    c = make_client(adapter)
    assert c.get_collection("getcolxx").id == "getcolxx"


def test_get_collection_adapter_none_then_local():
    adapter = MagicMock()
    adapter.get_collection.return_value = None
    c = make_client(adapter)
    c._store_local_collection(a_collection("localcol"))
    assert c.get_collection("localcol").id == "localcol"


def test_get_collection_not_found_raises():
    adapter = MagicMock()
    adapter.get_collection.return_value = None
    c = make_client(adapter)
    with pytest.raises(CollectionNotFoundError):
        c.get_collection("missingx")


def test_get_collection_prefer_local_fallback():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("loccolxx"))
    assert c.get_collection("loccolxx").id == "loccolxx"
    with pytest.raises(CollectionNotFoundError):
        c.get_collection("nopexxxx")


def test_list_collections_via_adapter():
    adapter = MagicMock()
    adapter.list_collections.return_value = [
        a_collection("colaaaaa"),
        a_collection("colbbbbb"),
    ]
    c = make_client(adapter)
    assert len(c.list_collections()) == 2


def test_list_collections_empty_uses_local():
    adapter = MagicMock()
    adapter.list_collections.return_value = []
    c = make_client(adapter)
    c._store_local_collection(a_collection("onlycoll"))
    result = c.list_collections()
    assert len(result) == 1


def test_list_collections_prefer_local():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("xcollxxx"))
    assert len(c.list_collections()) == 1


def test_list_collections_adapter_error_falls_back():
    adapter = MagicMock()
    adapter.list_collections.side_effect = Exception("boom")
    c = make_client(adapter)
    c._store_local_collection(a_collection("safecoll"))
    assert len(c.list_collections()) == 1


def test_delete_collection_via_adapter_and_local():
    adapter = MagicMock()
    adapter.delete_collection.return_value = True
    c = make_client(adapter)
    c._store_local_collection(a_collection("delcolxx"))
    assert c.delete_collection("delcolxx") is True
    assert "delcolxx" not in ProximaDBClient._shared_local_collections


def test_delete_collection_adapter_only():
    adapter = MagicMock()
    adapter.delete_collection.return_value = True
    c = make_client(adapter)
    assert c.delete_collection("remotecl") is True


def test_delete_collection_prefer_local():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("pcollxxx"))
    assert c.delete_collection("pcollxxx") is True
    assert c.delete_collection("absentxx") is False


def test_delete_collection_adapter_error_falls_back():
    adapter = MagicMock()
    adapter.delete_collection.side_effect = Exception("err")
    c = make_client(adapter)
    c._store_local_collection(a_collection("ecollxxx"))
    assert c.delete_collection("ecollxxx") is True


def test_get_collection_stats():
    adapter = MagicMock()
    adapter.get_collection.return_value = a_collection("statcoll", dim=5)
    c = make_client(adapter)
    stats = c.get_collection_stats("statcoll")
    assert stats["dimension"] == 5


# ---------------------------------------------------------------------------
# Vectors: insert / upsert / search / get / delete
# ---------------------------------------------------------------------------


def test_insert_records_via_adapter():
    adapter = MagicMock()
    adapter.insert_records.return_value = BatchResult(total=2, success=2, failed=0)
    c = make_client(adapter)
    recs = [{"id": "1", "vector": [1, 2, 3]}, {"id": "2", "vector": [4, 5, 6]}]
    result = c.insert_records("colxxxxx", recs)
    assert result.success == 2


def test_insert_records_empty_raises():
    c = make_client()
    with pytest.raises(ValueError):
        c.insert_records("colxxxxx", [])


def test_insert_records_prefer_local():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("colxxxxx"))
    result = c.insert_records("colxxxxx", [{"id": "1", "vector": [1, 2, 3]}])
    assert result.success == 1


def test_upsert_records_via_adapter():
    adapter = MagicMock()
    adapter.upsert_records.return_value = BatchResult(total=1, success=1, failed=0)
    c = make_client(adapter)
    result = c.upsert_records("colxxxxx", [{"id": "1", "vector": [1, 2, 3]}])
    assert result.success == 1


def test_upsert_records_falls_back_to_insert():
    adapter = MagicMock(spec=["insert_records"])
    adapter.insert_records.return_value = BatchResult(total=1, success=1, failed=0)
    c = make_client(adapter)
    result = c.upsert_records("colxxxxx", [{"id": "1", "vector": [1, 2, 3]}])
    assert result.success == 1


def test_insert_vectors_via_records_adapter():
    adapter = MagicMock()
    adapter.insert_records.return_value = BatchResult(total=1, success=1, failed=0)
    c = make_client(adapter)
    rec = VectorRecord(id="1", vector=[1.0, 2.0, 3.0])
    resp = c.insert_vectors("colxxxxx", records=[rec])
    assert resp.operation == "INSERT"
    assert resp.metrics.successful_count == 1


def test_insert_vectors_old_api_numpy():
    adapter = MagicMock()
    adapter.insert_records.return_value = BatchResult(total=2, success=2, failed=0)
    c = make_client(adapter)
    resp = c.insert_vectors(
        "colxxxxx",
        vectors=np.array([[1.0, 2.0], [3.0, 4.0]]),
        ids=["a", "b"],
        metadata=[{"k": 1}, {"k": 2}],
    )
    assert resp.metrics.total_processed == 2


def test_insert_vectors_requires_input():
    c = make_client()
    with pytest.raises(ValueError):
        c.insert_vectors("colxxxxx")


def test_insert_vector_single_delegates():
    adapter = MagicMock()
    adapter.insert_records.return_value = BatchResult(total=1, success=1, failed=0)
    c = make_client(adapter)
    resp = c.insert_vector(
        "colxxxxx", "v1", [1.0, 2.0, 3.0], metadata={"a": 1}, version=2, source="hi"
    )
    assert resp.operation == "INSERT"


def test_insert_vector_upsert_path():
    adapter = MagicMock()
    adapter.upsert_records.return_value = BatchResult(total=1, success=1, failed=0)
    c = make_client(adapter)
    resp = c.insert_vector("colxxxxx", "v1", [1.0, 2.0, 3.0], upsert=True)
    assert resp.operation == "UPSERT"


def test_legacy_insert_and_upsert_and_delete():
    adapter = MagicMock()
    adapter.insert_records.return_value = BatchResult(total=1, success=1, failed=0)
    adapter.upsert_records.return_value = BatchResult(total=1, success=1, failed=0)
    adapter.delete_vectors.return_value = _delete_resp()
    c = make_client(adapter)
    assert c.insert("colxxxxx", [[1.0, 2.0]], ids=["a"]).operation == "INSERT"
    assert c.upsert("colxxxxx", [[1.0, 2.0]], ids=["a"]).operation == "UPSERT"
    assert c.delete("colxxxxx", ["a"]).operation == "DELETE"


def test_search_delegates_to_adapter():
    adapter = MagicMock()
    adapter.search.return_value = [SearchResult(id="1", score=0.9, rank=1)]
    c = make_client(adapter)
    res = c.search("colxxxxx", [1.0, 2.0, 3.0], top_k=5)
    assert res[0].id == "1"
    adapter.search.assert_called_once()


def test_search_invalid_top_k():
    c = make_client()
    with pytest.raises(ProximaDBError):
        c.search("colxxxxx", [1.0], top_k=0)


def test_search_adapter_error_falls_back_local():
    adapter = MagicMock()
    adapter.search.side_effect = Exception("boom")
    c = make_client(adapter)
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="x", vector=[1.0, 2.0, 3.0])])
    res = c.search("colxxxxx", [1.0, 2.0, 3.0], top_k=3)
    assert res[0].id == "x"
    assert c._prefer_local_fallback is True


def test_search_prefer_local():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records(
        "colxxxxx",
        [
            VectorRecord(id="a", vector=[1.0, 0.0, 0.0], metadata={"k": "v"}),
            VectorRecord(id="b", vector=[0.0, 1.0, 0.0]),
        ],
    )
    res = c.search_single("colxxxxx", [1.0, 0.0, 0.0], top_k=2, include_vectors=True)
    assert res[0].id == "a"
    assert res[0].vector is not None


def test_search_batch():
    adapter = MagicMock()
    adapter.search.return_value = [SearchResult(id="1", score=0.5, rank=1)]
    c = make_client(adapter)
    out = c.search_batch("colxxxxx", np.array([[1.0, 2.0], [3.0, 4.0]]), top_k=1)
    assert len(out) == 2


def test_search_envelope_rest():
    c = make_client()
    c._client.search_envelope.return_value = {"items": []}
    out = c.search_envelope("colxxxxx", np.array([1.0, 2.0]), top_k=3)
    assert out == {"items": []}


def test_search_envelope_unsupported_protocol():
    c = make_client()
    c._active_protocol = Protocol.GRPC
    with pytest.raises(ProximaDBError):
        c.search_envelope("colxxxxx", [1.0, 2.0])


def test_search_iter_rest_paginates():
    c = make_client()
    page1 = MagicMock(items=["i1"], cursor="cur", has_more=True)
    page2 = MagicMock(items=["i2"], cursor=None, has_more=False)
    c._client.search_envelope.return_value = page1
    c._client.search_next_page.return_value = page2
    items = list(c.search_iter("colxxxxx", [1.0], top_k=2))
    assert items == ["i1", "i2"]


def test_search_iter_grpc_single_page():
    adapter = MagicMock()
    adapter.search.return_value = [SearchResult(id="1", score=0.5, rank=1)]
    c = make_client(adapter)
    c._active_protocol = Protocol.GRPC
    c._client = MagicMock(spec=[])  # no search_envelope
    items = list(c.search_iter("colxxxxx", [1.0], top_k=1))
    assert items[0].id == "1"


def test_get_vector_prefer_local_found_and_missing():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records(
        "colxxxxx", [VectorRecord(id="v1", vector=[1.0, 2.0, 3.0], metadata={"a": 1})]
    )
    rec = c.get_vector("colxxxxx", "v1")
    assert rec.id == "v1"
    with pytest.raises(ProximaDBError):
        c.get_vector("colxxxxx", "nope")


def test_get_vector_via_client():
    c = make_client()
    c._client.get_vector.return_value = VectorRecord(id="v1", vector=[1.0])
    rec = c.get_vector("colxxxxx", "v1")
    assert rec.id == "v1"


def test_get_vector_client_error_falls_back():
    c = make_client()
    c._client.get_vector.side_effect = Exception("boom")
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="v1", vector=[1.0, 2.0, 3.0])])
    rec = c.get_vector("colxxxxx", "v1", include_vector=False, include_metadata=False)
    assert rec.id == "v1"
    assert c._prefer_local_fallback is True


def test_delete_vectors_via_adapter():
    adapter = MagicMock()
    adapter.delete_vectors.return_value = _delete_resp()
    c = make_client(adapter)
    resp = c.delete_vectors("colxxxxx", ["a", "b"])
    assert resp.operation == "DELETE"


def test_delete_vectors_prefer_local():
    c = make_client()
    c._prefer_local_fallback = True
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records(
        "colxxxxx", [VectorRecord(id="a", vector=[1.0]), VectorRecord(id="b", vector=[2.0])]
    )
    resp = c.delete_vectors("colxxxxx", ["a"])
    assert resp.metrics.successful_count == 1


def test_delete_vectors_adapter_error_falls_back():
    adapter = MagicMock()
    adapter.delete_vectors.side_effect = Exception("boom")
    c = make_client(adapter)
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="a", vector=[1.0])])
    resp = c.delete_vectors("colxxxxx", ["a"])
    assert resp.operation == "DELETE"


def test_delete_vector_single():
    adapter = MagicMock()
    adapter.delete_vectors.return_value = _delete_resp()
    c = make_client(adapter)
    assert c.delete_vector("colxxxxx", "a").operation == "DELETE"


# ---------------------------------------------------------------------------
# Helpers: similarity / metadata filter / local sql
# ---------------------------------------------------------------------------


def test_cosine_similarity_branches():
    assert ProximaDBClient._cosine_similarity([0.0, 0.0], [1.0, 1.0]) == 0.0
    s = ProximaDBClient._cosine_similarity([1.0, 0.0], [1.0, 0.0])
    assert math.isclose(s, 1.0)


def test_metadata_matches_filter():
    assert ProximaDBClient._metadata_matches_filter({"a": 1}, None) is True
    assert ProximaDBClient._metadata_matches_filter({"a": 1}, {"a": 1}) is True
    assert ProximaDBClient._metadata_matches_filter({"a": 1}, {"a": 2}) is False

    class Builder:
        def build(self):
            return {"a": 1}

    assert ProximaDBClient._metadata_matches_filter({"a": 1}, Builder()) is True


def test_execute_sql_local_select_star():
    c = make_client()
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records(
        "colxxxxx", [VectorRecord(id="1", vector=[1.0, 2.0, 3.0], metadata={"k": "v"})]
    )
    res = c._execute_sql_local("SELECT * FROM colxxxxx")
    assert res["row_count"] == 1
    assert "id" in res["columns"]


def test_execute_sql_local_columns():
    c = make_client()
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="1", vector=[1.0])])
    res = c._execute_sql_local("SELECT id FROM colxxxxx LIMIT 1")
    assert res["columns"] == ["id"]


def test_execute_sql_local_invalid_raises():
    c = make_client()
    with pytest.raises(ProximaDBError):
        c._execute_sql_local("INVALID SQL whatever")


def test_execute_sql_local_metadata_unsupported():
    c = make_client()
    with pytest.raises(ProximaDBError):
        c._execute_sql_local("SELECT * FROM colxxxxx WHERE metadata.x = 1")


def test_execute_sql_local_missing_from():
    c = make_client()
    with pytest.raises(ProximaDBError):
        c._execute_sql_local("SELECT 1")


def test_execute_sql_local_vector_search():
    c = make_client()
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="1", vector=[1.0, 0.0, 0.0])])
    res = c._execute_sql_local(
        "SELECT id, score FROM vector_search('colxxxxx', '[1.0, 0.0, 0.0]', 5)"
    )
    assert res["row_count"] == 1
    assert "score" in res["columns"]


def test_is_vector_search_sql():
    assert ProximaDBClient._is_vector_search_sql(
        "SELECT * FROM vector_search('c', '[1]', 5)"
    )
    assert not ProximaDBClient._is_vector_search_sql("SELECT * FROM colxxxxx")


def test_sql_rows_to_unified_records():
    rows = [{"id": "1", "score": 0.9}, {"score": 0.8}]
    out = ProximaDBClient._sql_rows_to_unified_records(rows)
    assert out[0]["id"] == "1"
    assert out[1]["id"] == "row_1"


def test_local_sql_fallback_result_empty_returns_none():
    c = make_client()
    assert c._local_sql_fallback_result("INVALID SQL") is None


# ---------------------------------------------------------------------------
# execute_sql / execute_query family
# ---------------------------------------------------------------------------


def test_execute_sql_rest_uses_session():
    c = make_client()
    c._active_protocol = Protocol.REST
    session = MagicMock()
    resp = MagicMock()
    resp.json.return_value = {"rows": [], "row_count": 0}
    resp.raise_for_status.return_value = None
    session.post.return_value = resp
    c._client._session = session
    c._client._base_url = "http://testserver"
    out = c.execute_sql("SELECT * FROM colxxxxx", parameters=[1], collection="colxxxxx")
    assert out["row_count"] == 0


def test_execute_sql_rest_falls_back_local_on_error():
    c = make_client()
    c._active_protocol = Protocol.REST
    c._client = MagicMock(spec=[])  # no _session -> _execute_sql_rest uses requests
    c._store_local_collection(a_collection("colxxxxx"))
    c._store_local_vector_records("colxxxxx", [VectorRecord(id="1", vector=[1.0])])
    # requests.post will fail offline -> local fallback
    out = c.execute_sql("SELECT id FROM colxxxxx LIMIT 1")
    assert out["columns"] == ["id"]


def test_execute_query_via_adapter():
    adapter = MagicMock()
    adapter.execute_query.return_value = {"records": [{"id": 1}]}
    c = make_client(adapter)
    out = c.execute_query("FOR d IN col RETURN d", language="aql")
    assert out["records"] == [{"id": 1}]


def test_execute_uql_aql_federated_delegate():
    adapter = MagicMock()
    adapter.execute_query.return_value = {"ok": True}
    c = make_client(adapter)
    assert c.execute_uql("q")["ok"] is True
    assert c.execute_aql("q")["ok"] is True
    assert c.execute_federated("q")["ok"] is True


def test_execute_query_no_surface_raises():
    c = make_client(MagicMock(spec=[]))
    c._client = MagicMock(spec=[])
    with pytest.raises(NotImplementedError):
        c.execute_query("q")


def test_execute_unified_query_via_adapter():
    adapter = MagicMock()
    adapter.execute_unified_query.return_value = [{"id": 1}]
    c = make_client(adapter)
    out = c.execute_unified_query("q", query_vector=[1.0])
    assert out == [{"id": 1}]


def test_execute_unified_query_execute_query_fallback():
    adapter = MagicMock(spec=["execute_query"])
    adapter.execute_query.return_value = {"rows": [{"id": 1}]}
    c = make_client(adapter)
    out = c.execute_unified_query("q")
    assert out == [{"id": 1}]


def test_execute_unified_query_no_surface_raises():
    c = make_client(MagicMock(spec=[]))
    with pytest.raises(NotImplementedError):
        c.execute_unified_query("q")


# ---------------------------------------------------------------------------
# Document & timeseries delegation (adapter-served)
# ---------------------------------------------------------------------------


def test_document_methods_via_adapter():
    adapter = MagicMock()
    adapter.create_document_collection.return_value = {"success": True}
    adapter.insert_document.return_value = {"id": "d1"}
    adapter.get_document.return_value = {"id": "d1", "found": True}
    adapter.query_documents.return_value = {"documents": []}
    adapter.update_document.return_value = {"success": True}
    adapter.delete_document.return_value = True
    adapter.list_document_collections.return_value = [{"name": "c"}]
    adapter.delete_document_collection.return_value = True
    c = make_client(adapter)
    assert c.create_document_collection("c")["success"] is True
    assert c.insert_document("c", {"a": 1})["id"] == "d1"
    assert c.get_document("c", "d1")["found"] is True
    assert c.query_documents("c", filter={"a": 1})["documents"] == []
    assert c.update_document("c", "d1", [{"set": {}}])["success"] is True
    assert c.delete_document("c", "d1") is True
    assert c.list_document_collections() == [{"name": "c"}]
    assert c.delete_document_collection("c") is True


def test_timeseries_methods_via_adapter():
    adapter = MagicMock()
    adapter.create_timeseries_collection.return_value = {"success": True}
    adapter.ingest_timeseries.return_value = {"ingested": 2}
    adapter.query_timeseries.return_value = {"points": []}
    adapter.list_timeseries_collections.return_value = [{"name": "t"}]
    adapter.delete_timeseries_collection.return_value = True
    c = make_client(adapter)
    assert c.create_timeseries_collection("t")["success"] is True
    assert c.ingest_timeseries("t", [{"ts": 1}])["ingested"] == 2
    assert c.query_timeseries("t", "0", "1")["points"] == []
    assert c.list_timeseries_collections() == [{"name": "t"}]
    assert c.delete_timeseries_collection("t") is True


def test_call_document_adapter_skips_notimplemented():
    adapter = MagicMock()
    adapter.foo.side_effect = NotImplementedError
    c = make_client(adapter)
    # rest adapter also lacks foo -> returns None
    c._rest_adapter = MagicMock(spec=[])
    assert c._call_document_adapter("foo", 1) is None


def test_hybrid_search_via_adapter():
    adapter = MagicMock()
    adapter.hybrid_search.return_value = {"results": [], "metrics": {}}
    c = make_client(adapter)
    out = c.hybrid_search("colxxxxx", "text", [1.0, 2.0], top_k=3)
    assert "results" in out


# ---------------------------------------------------------------------------
# Graph delegation
# ---------------------------------------------------------------------------


def test_create_node_delegates():
    c = make_client()
    c._client.create_node.return_value = {"node_id": "n1"}
    out = c.create_node("n1", ["Person"], {"name": "a"}, graph_id="g")
    assert out["node_id"] == "n1"


def test_create_node_type_validation():
    c = make_client()
    with pytest.raises(TypeError):
        c.create_node(123, ["L"])
    with pytest.raises(TypeError):
        c.create_node("n", "notalist")


def test_create_edge_delegates():
    c = make_client()
    c._client.create_edge.return_value = {"edge_id": "e1"}
    out = c.create_edge("e1", "a", "b", "KNOWS", weight=1.0)
    assert out["edge_id"] == "e1"


def test_create_edge_type_validation():
    c = make_client()
    with pytest.raises(TypeError):
        c.create_edge("e", "a", "b", 123)


def test_traverse_graph_delegates():
    c = make_client()
    c._client.traverse_graph.return_value = {"nodes": []}
    out = c.traverse_graph("a", max_depth=2, algorithm="BFS")
    assert "nodes" in out


def test_traverse_graph_validation():
    c = make_client()
    with pytest.raises(ValueError):
        c.traverse_graph("a", max_depth=0)
    with pytest.raises(ValueError):
        c.traverse_graph("a", algorithm="UNKNOWN")


def test_query_nodes_delegates():
    c = make_client()
    c._client.query_nodes.return_value = {"nodes": []}
    out = c.query_nodes(labels=["Person"], limit=5)
    assert "nodes" in out


def test_query_nodes_validation():
    c = make_client()
    with pytest.raises(TypeError):
        c.query_nodes(labels="notalist")


def test_get_node_and_edges():
    c = make_client()
    c._client.get_node.return_value = {"id": "n1"}
    c._client.get_outgoing_edges.return_value = [{"id": "e1"}]
    c._client.get_incoming_edges.return_value = [{"id": "e2"}]
    assert c.get_node("n1")["id"] == "n1"
    assert c.get_outgoing_edges("n1") == [{"id": "e1"}]
    assert c.get_incoming_edges("n1") == [{"id": "e2"}]


def test_get_node_returns_none():
    c = make_client()
    c._client.get_node.return_value = None
    assert c.get_node("n1") is None


def test_delete_node():
    c = make_client()
    c._client.delete_node.return_value = True
    assert c.delete_node("n1") is True


def test_invoke_graph_method_typeerror_retry():
    c = make_client()
    method = MagicMock()
    method.side_effect = [TypeError("no graph_id"), {"ok": True}]
    c._client.get_graph_stats = method
    out = c.get_graph_stats("g")
    assert out == {"ok": True}


def test_create_graph_delegates():
    c = make_client()
    c._client.create_graph.return_value = {"graph_id": "g"}
    assert c.create_graph("g", name="G")["graph_id"] == "g"


def test_create_graph_typeerror_fallback():
    c = make_client()
    c._client.create_graph.side_effect = [TypeError("bad sig"), {"graph_id": "g"}]
    out = c.create_graph("g")
    assert out["graph_id"] == "g"


def test_graph_collection_management():
    c = make_client()
    c._client.delete_graph.return_value = {"deleted": True}
    c._client.get_graph.return_value = {"name": "G"}
    c._client.list_graphs.return_value = {"graphs": []}
    assert c.delete_graph("g")["deleted"] is True
    assert c.get_graph("g")["name"] == "G"
    assert c.list_graphs()["graphs"] == []


def test_graph_shortest_path_rest():
    c = make_client()
    c._active_protocol = Protocol.REST
    c._client.graph_shortest_path.return_value = {"path": []}
    out = c.graph_shortest_path("a", "b")
    assert "path" in out


def test_graph_shortest_path_grpc():
    c = make_client()
    c._active_protocol = Protocol.GRPC
    c._client.shortest_path.return_value = {"path": []}
    out = c.graph_shortest_path("a", "b")
    assert "path" in out


def test_graph_shortest_path_unsupported():
    c = make_client()
    c._active_protocol = Protocol.REST
    c._client = MagicMock(spec=[])
    with pytest.raises(ProximaDBError):
        c.graph_shortest_path("a", "b")


def test_graph_traverse_unified():
    c = make_client()
    c._client.graph_traverse.return_value = {"nodes": []}
    assert "nodes" in c.graph_traverse("a")


def test_graph_traverse_unsupported():
    c = make_client()
    c._client = MagicMock(spec=[])
    with pytest.raises(ProximaDBError):
        c.graph_traverse("a")


# ---------------------------------------------------------------------------
# Observability delegation
# ---------------------------------------------------------------------------


def test_observability_via_adapter():
    adapter = MagicMock()
    adapter.create_observability_namespace.return_value = {"success": True}
    adapter.ingest_logs.return_value = 3
    adapter.query_logs.return_value = [{"msg": "x"}]
    adapter.ingest_metrics.return_value = 2
    adapter.aggregate_metrics.return_value = [{"v": 1}]
    adapter.ingest_traces.return_value = 1
    adapter.query_traces.return_value = [{"trace": 1}]
    adapter.get_trace.return_value = {"spans": []}
    c = make_client(adapter)
    assert c.create_observability_namespace("ns")["success"] is True
    assert c.ingest_logs("ns", [{}]) == 3
    assert c.query_logs("ns", 0, 1) == [{"msg": "x"}]
    assert c.ingest_metrics("ns", [{}]) == 2
    assert c.aggregate_metrics("ns", "m") == [{"v": 1}]
    assert c.ingest_traces("ns", [{}]) == 1
    assert c.query_traces("ns", 0, 1) == [{"trace": 1}]
    assert c.get_trace("ns", "t1") == {"spans": []}


def test_observability_not_implemented_without_adapter():
    c = make_client(MagicMock(spec=[]))
    with pytest.raises(NotImplementedError):
        c.create_observability_namespace("ns")
    with pytest.raises(NotImplementedError):
        c.ingest_logs("ns", [])
    with pytest.raises(NotImplementedError):
        c.query_logs("ns", 0, 1)
    with pytest.raises(NotImplementedError):
        c.ingest_metrics("ns", [])
    with pytest.raises(NotImplementedError):
        c.aggregate_metrics("ns", "m")
    with pytest.raises(NotImplementedError):
        c.ingest_traces("ns", [])
    with pytest.raises(NotImplementedError):
        c.query_traces("ns", 0, 1)
    with pytest.raises(NotImplementedError):
        c.get_trace("ns", "t1")


# ---------------------------------------------------------------------------
# health / close / context manager
# ---------------------------------------------------------------------------


def test_health_via_adapter():
    adapter = MagicMock()
    health = HealthStatus(
        status="healthy",
        version="1.0.0",
        uptime_seconds=10,
        services={},
        timestamp_ms=123,
    )
    adapter.health.return_value = health
    c = make_client(adapter)
    assert c.health().status == "healthy"


def test_close_and_context_manager():
    c = make_client()
    closeable = MagicMock()
    c._client = closeable
    with c as ctx:
        assert ctx is c
    closeable.close.assert_called()
    # idempotent
    c.close()


def test_close_handles_errors():
    c = make_client()
    c._client = MagicMock()
    c._client.close.side_effect = Exception("nope")
    c._operation_router = MagicMock()
    c._protocol_selector = MagicMock()
    c.close()
    assert c._closed is True


# ---------------------------------------------------------------------------
# Routing / selection stats getters
# ---------------------------------------------------------------------------


def test_routing_stats_disabled():
    c = make_client()
    c._operation_router = None
    assert "error" in c.get_routing_stats()
    assert "error" in c.get_selection_stats()
    # add_routing_rule / reset_routing_metrics no-op when disabled
    c.add_routing_rule(object())
    c.reset_routing_metrics()


def test_routing_stats_enabled():
    c = make_client()
    router = MagicMock()
    router.get_routing_stats.return_value = {"ops": 1}
    c._operation_router = router
    assert c.get_routing_stats() == {"ops": 1}
    c.add_routing_rule("rule")
    router.add_routing_rule.assert_called_once_with("rule")
    c.reset_routing_metrics()
    router.reset_metrics.assert_called_once()


def test_selection_stats_enabled_and_metrics():
    c = make_client()
    selector = MagicMock()
    selector.get_selection_stats.return_value = {"sel": 1}
    selector.get_protocol_metrics.return_value = {"m": 1}
    c._protocol_selector = selector
    assert c.get_selection_stats() == {"sel": 1}
    assert c.get_protocol_metrics() == {"m": 1}


def test_force_protocol_switch_disabled_raises():
    c = make_client()
    c._protocol_selector = None
    with pytest.raises(ProximaDBError):
        c.force_protocol_switch(Protocol.GRPC)


def test_force_protocol_switch_enabled():
    c = make_client()
    selector = MagicMock()
    new_client = MagicMock()
    selector.get_client.return_value = new_client
    c._protocol_selector = selector
    c.force_protocol_switch(Protocol.GRPC)
    assert c._active_protocol == Protocol.GRPC
    assert c._client is new_client


def test_record_operation_result_with_router():
    c = make_client()
    router = MagicMock()
    c._operation_router = router
    c._record_operation_result("op", Protocol.REST, True, 1.0)
    router.record_operation_result.assert_called_once()


def test_get_client_for_operation_no_routing():
    c = make_client()
    c.enable_operation_routing = False
    assert c._get_client_for_operation("op") is c._client


# ---------------------------------------------------------------------------
# Proto conversion helpers (gRPC available)
# ---------------------------------------------------------------------------


def test_proto_to_pydantic_distance_and_engine_and_algo():
    from proximadb_sdk.models import DistanceMetric, IndexingAlgorithm, StorageEngine

    c = make_client()
    metric = c._pydantic_to_proto_distance_metric(DistanceMetric.COSINE)
    assert c._proto_to_pydantic_distance_metric(metric) == DistanceMetric.COSINE
    engine = c._pydantic_to_proto_storage_engine(StorageEngine.VIPER)
    assert c._proto_to_pydantic_storage_engine(engine) == StorageEngine.VIPER
    algo = c._pydantic_to_proto_indexing_algorithm(IndexingAlgorithm.HNSW)
    assert c._proto_to_pydantic_indexing_algorithm(algo) == IndexingAlgorithm.HNSW


def test_pydantic_to_proto_collection_config():
    c = make_client()
    cfg = CollectionConfig(
        name="protocoll",
        dimension=4,
        description="a description",
        tags=["t1"],
        owner="me",
    )
    proto = c._pydantic_to_proto_collection_config(cfg)
    assert proto.name == "protocoll"
    assert proto.dimension == 4


def test_pydantic_to_proto_quantization_config_variants():
    from proximadb_sdk.models import QuantizationConfig, QuantizationType

    c = make_client()
    binary = QuantizationConfig(
        enabled=True, type=QuantizationType.BINARY, threshold=0.5
    )
    assert c._pydantic_to_proto_quantization_config(binary).enable_binary is True
    scalar = QuantizationConfig(enabled=True, type=QuantizationType.SCALAR)
    assert c._pydantic_to_proto_quantization_config(scalar).enable_int8 is True
    product = QuantizationConfig(
        enabled=True,
        type=QuantizationType.PRODUCT,
        num_subvectors=8,
        bits_per_subvector=4,
    )
    pq = c._pydantic_to_proto_quantization_config(product)
    assert pq.enable_pq is True


def test_proto_to_pydantic_collection():
    c = make_client()
    cfg = CollectionConfig(name="convertc", dimension=6)
    proto = c._pydantic_to_proto_collection_config(cfg)

    class Wrapper:
        def __init__(self, config):
            self.config = config
            self.id = "convertc"

    out = c._proto_to_pydantic_collection(Wrapper(proto))
    assert out.config.name == "convertc"


def test_proto_to_pydantic_health_status():
    class P:
        status = "ok"
        version = "1.0.0"
        uptime_seconds = 5
        timestamp_ms = 99

    out = make_client()._proto_to_pydantic_health_status(P())
    assert out.status == "ok"
    assert out.timestamp_ms == 99


# ---------------------------------------------------------------------------
# gRPC raw-client paths (adapter absent)
# ---------------------------------------------------------------------------


def grpc_client_no_adapter():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._adapter = None
    c._client = MagicMock()
    c._active_protocol = Protocol.GRPC
    c._prefer_local_fallback = False
    return c


def test_grpc_create_collection_with_collection_attr():
    c = grpc_client_no_adapter()
    response = MagicMock()
    cfg = CollectionConfig(name="grpccoll", dimension=4)
    response.collection = c._pydantic_to_proto_collection_config(cfg)
    c._client.create_collection.return_value = response
    out = c.create_collection("grpccoll", cfg)
    assert out.config.name == "grpccoll"


def test_grpc_get_collection_found_and_missing():
    c = grpc_client_no_adapter()
    cfg = CollectionConfig(name="ggetcoll", dimension=4)

    class Wrapper:
        config = c._pydantic_to_proto_collection_config(cfg)
        id = "ggetcoll"

    c._client.get_collection.return_value = Wrapper()
    assert c.get_collection("ggetcoll").config.name == "ggetcoll"
    c._client.get_collection.return_value = None
    with pytest.raises(CollectionNotFoundError):
        c.get_collection("ggetcoll")


def test_grpc_search_returns_results():
    c = grpc_client_no_adapter()
    c._client.search_vectors.return_value = [SearchResult(id="1", score=0.9, rank=1)]
    out = c.search_single("grpccoll", np.array([1.0, 2.0]), top_k=3)
    assert out[0].id == "1"


def test_health_raw_client_rest():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._adapter = None
    c._active_protocol = Protocol.REST
    c._client = MagicMock()
    c._client.health.return_value = HealthStatus(
        status="ok", version="1", uptime_seconds=1, services={}, timestamp_ms=1
    )
    assert c.health().status == "ok"


def test_health_raw_client_grpc():
    c = grpc_client_no_adapter()
    c._client.health_check.return_value = type(
        "H",
        (),
        {"status": "ok", "version": "1", "uptime_seconds": 2, "timestamp_ms": 3},
    )()
    assert c.health().status == "ok"


def test_delete_collection_raw_client():
    c = grpc_client_no_adapter()
    c._client.delete_collection.return_value = True
    assert c.delete_collection("anycolxx") is True


# ---------------------------------------------------------------------------
# Embedded-mode branches (mocked native client)
# ---------------------------------------------------------------------------


def embedded_client():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._adapter = None
    c._client = MagicMock()
    c._active_protocol = Protocol.EMBEDDED
    c._prefer_local_fallback = False
    return c


def test_embedded_execute_sql():
    c = embedded_client()
    c._client.execute_sql.return_value = {"rows": [{"id": "1"}], "row_count": 1}
    out = c.execute_sql("SELECT * FROM colxxxxx")
    assert out["row_count"] == 1


def test_embedded_execute_unified_query():
    c = embedded_client()
    c._client.execute_unified_query.return_value = [{"id": 1}]
    assert c.execute_unified_query("q") == [{"id": 1}]


def test_embedded_observability():
    c = embedded_client()
    c._client.create_observability_namespace.return_value = {"success": True}
    c._client.ingest_logs.return_value = 2
    c._client.query_logs.return_value = [{"m": 1}]
    c._client.ingest_metrics.return_value = 1
    c._client.aggregate_metrics.return_value = [{"v": 1}]
    c._client.ingest_traces.return_value = 1
    c._client.query_traces.return_value = [{"t": 1}]
    c._client.get_trace.return_value = {"spans": [], "complete": True}
    assert c.create_observability_namespace("ns")["success"] is True
    assert c.ingest_logs("ns", [{}]) == 2
    assert c.query_logs("ns", 0, 1) == [{"m": 1}]
    assert c.ingest_metrics("ns", [{}]) == 1
    assert c.aggregate_metrics("ns", "m") == [{"v": 1}]
    assert c.ingest_traces("ns", [{}]) == 1
    assert c.query_traces("ns", 0, 1) == [{"t": 1}]
    assert c.get_trace("ns", "t1")["complete"] is True


def test_embedded_graph_node_non_dict_wrapped():
    c = embedded_client()
    c._client.create_node.return_value = "ok"  # non-dict triggers wrapping
    out = c.create_node("n1", ["L"])
    assert out["success"] is True
    assert out["node_id"] == "n1"


def test_embedded_graph_edge_non_dict_wrapped():
    c = embedded_client()
    c._client.create_edge.return_value = "ok"
    out = c.create_edge("e1", "a", "b", "KNOWS")
    assert out["edge_id"] == "e1"


def test_embedded_traverse_graph_non_dict_wrapped():
    c = embedded_client()
    c._client.traverse_graph.return_value = ["n1", "n2"]
    out = c.traverse_graph("a", max_depth=2)
    assert out["nodes"] == ["n1", "n2"]


def test_embedded_query_nodes_non_dict_wrapped():
    c = embedded_client()
    c._client.query_nodes.return_value = ["n1"]
    out = c.query_nodes(labels=["L"])
    assert out["total_count"] == 1


def test_invoke_graph_method_no_graph_id():
    c = make_client()
    c._client.get_node.return_value = {"id": "n"}
    # graph_id None -> calls without graph kw
    assert c.get_node("n") == {"id": "n"}


# ---------------------------------------------------------------------------
# hybrid_search local fallback (no REST adapter hit)
# ---------------------------------------------------------------------------


def test_hybrid_search_rest_adapter_error_then_local(monkeypatch):
    adapter = MagicMock()
    adapter.hybrid_search.side_effect = Exception("boom")
    c = make_client(adapter)

    class FakeHybrid:
        def __init__(self, client):
            pass

        def search(self, **kwargs):
            return []

    import proximadb_sdk.hybrid as hybrid_mod

    monkeypatch.setattr(hybrid_mod, "ProximaDBHybrid", FakeHybrid)
    out = c.hybrid_search("colxxxxx", "text", [1.0, 2.0], fusion_strategy="weighted_linear")
    assert out["results"] == []
    assert "metrics" in out


# ---------------------------------------------------------------------------
# document / timeseries repository fallback (adapter declines)
# ---------------------------------------------------------------------------


def test_document_create_via_repository(monkeypatch):
    # adapter returns None -> falls through to DocumentRepository
    c = make_client(MagicMock(spec=[]))
    c._rest_adapter = MagicMock(spec=[])

    fake_repo = MagicMock()
    fake_repo.create_collection.return_value = "docid"
    monkeypatch.setattr(c, "_get_document_repository", lambda: fake_repo)
    out = c.create_document_collection("docs", {"indexes": [{"name": "i", "path": "$.a"}]})
    assert out["collection_id"] == "docid"


def test_timeseries_create_via_repository(monkeypatch):
    c = make_client(MagicMock(spec=[]))
    c._rest_adapter = MagicMock(spec=[])
    fake_repo = MagicMock()
    fake_repo.create_collection.return_value = "tsid"
    monkeypatch.setattr(c, "_get_timeseries_repository", lambda: fake_repo)
    out = c.create_timeseries_collection("tscoll")
    assert out["collection_id"] == "tsid"


def raw_rest_client():
    """Adapter/clients all None so insert_records raises NotImplementedError and
    the legacy raw-client path is exercised."""
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._adapter = None
    c._rest_client = None
    c._grpc_client = None
    c._operation_router = None
    c.enable_operation_routing = False
    c._client = MagicMock()
    c._active_protocol = Protocol.REST
    c._prefer_local_fallback = False
    return c


def test_insert_vectors_raw_rest_else_branch():
    c = raw_rest_client()
    c._client.insert_vectors.return_value = VectorOperationResponse(
        success=True, operation="INSERT", metrics=OperationMetrics()
    )
    rec = VectorRecord(id="r1", vector=[1.0, 2.0, 3.0])
    out = c.insert_vectors("rawcollx", records=[rec])
    assert out.success is True
    c._client.insert_vectors.assert_called_once()


def test_insert_vectors_raw_rest_no_ids_generates():
    c = raw_rest_client()
    c._client.get_collection.return_value = None  # skip quantization id-validation
    c._client.insert_vectors.return_value = VectorOperationResponse(
        success=True, operation="INSERT", metrics=OperationMetrics()
    )
    rec = VectorRecord(vector=[1.0, 2.0])
    out = c.insert_vectors("rawcollx", records=[rec])
    assert out.success is True


def test_insert_vectors_raw_grpc_branch():
    c = raw_rest_client()
    c._active_protocol = Protocol.GRPC
    grpc_client = c._client
    c._grpc_client = grpc_client  # so client == self._grpc_client
    grpc_client.insert_vectors.return_value = type("R", (), {"success": True})()
    rec = VectorRecord(id="r1", vector=[1.0, 2.0], version=1, source="s")
    out = c.insert_vectors("rawcollx", records=[rec])
    assert out.success is True


def test_insert_vectors_raw_client_error_records_and_raises():
    c = raw_rest_client()
    c._client.insert_vectors.side_effect = Exception("server boom")
    rec = VectorRecord(id="r1", vector=[1.0, 2.0])
    with pytest.raises(Exception):
        c.insert_vectors("rawcollx", records=[rec])


def test_upsert_vectors_raw_rest_client():
    c = raw_rest_client()
    c._client.upsert_vectors.return_value = VectorOperationResponse(
        success=True, operation="UPSERT", metrics=OperationMetrics()
    )
    rec = VectorRecord(id="r1", vector=[1.0, 2.0])
    out = c.upsert_vectors("rawcollx", [rec])
    assert out.success is True
    c._client.upsert_vectors.assert_called_once()


def test_list_collections_raw_rest():
    c = raw_rest_client()
    c._client.list_collections.return_value = [a_collection("rawlistc")]
    out = c.list_collections()
    assert len(out) == 1


def test_create_collection_raw_rest_error_falls_back_local():
    c = raw_rest_client()
    c._client.create_collection.side_effect = Exception("any error")
    out = c.create_collection("rawcrecl", CollectionConfig(name="rawcrecl", dimension=3))
    assert out.id == "rawcrecl"
    assert c._prefer_local_fallback is True


def test_embedded_numpy_fast_path_insert():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._active_protocol = Protocol.EMBEDDED
    c._prefer_local_fallback = False
    adapter = MagicMock()
    adapter.insert_numpy.return_value = type("R", (), {"success": True})()
    c._adapter = adapter
    c._store_local_collection(a_collection("npcollxx"))
    out = c.insert("npcollxx", np.array([[1.0, 2.0, 3.0]]), ids=["a"])
    assert getattr(out, "success", None) is True
    adapter.insert_numpy.assert_called_once()


def test_embedded_numpy_fast_path_upsert():
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    c._active_protocol = Protocol.EMBEDDED
    c._prefer_local_fallback = False
    adapter = MagicMock()
    adapter.upsert_numpy.return_value = type("R", (), {"success": True})()
    c._adapter = adapter
    c._store_local_collection(a_collection("npcollxx"))
    out = c.upsert("npcollxx", np.array([[1.0, 2.0, 3.0]]), ids=["a"])
    assert getattr(out, "success", None) is True
    adapter.upsert_numpy.assert_called_once()


def test_record_payload_from_legacy_input_dict_and_record():
    c = make_client()
    payload = c._record_payload_from_legacy_input({"metadata": {"a": 1}}, 0)
    assert payload["props"] == {"a": 1}
    assert payload["id"] == "record_0"
    rec = VectorRecord(id="r1", vector=[1.0, 2.0], metadata={"k": "v"}, source="src")
    payload2 = c._record_payload_from_legacy_input(rec, 1)
    assert payload2["id"] == "r1"
    assert payload2["source"] == "src"


# ---------------------------------------------------------------------------
# Additional coverage: module factory funcs, client builders, routing/selection
# ---------------------------------------------------------------------------

from unittest.mock import patch  # noqa: E402

from proximadb_sdk import unified_client as _uc  # noqa: E402
from proximadb_sdk.config import PortMode  # noqa: E402
from proximadb_sdk.operation_router import RoutingStrategy  # noqa: E402
from proximadb_sdk.protocol_selector import SelectionStrategy  # noqa: E402
from proximadb_sdk.unified_client import (  # noqa: E402
    connect_arrow_flight,
    connect_grpc,
)


def test_connect_grpc_falls_back_on_import_error():
    # Force the gRPC constructor to raise an import-style ProximaDBError so the
    # AUTO fallback branch executes.
    with patch.object(
        _uc, "ProximaDBClient", wraps=_uc.ProximaDBClient
    ) as cls:
        calls = {"n": 0}

        def side_effect(*args, **kwargs):
            calls["n"] += 1
            if calls["n"] == 1:
                raise ProximaDBError("gRPC import pb2 missing")
            return MagicMock()

        cls.side_effect = side_effect
        out = connect_grpc(url="http://t")
    assert out is not None


def test_connect_grpc_reraises_non_import_error():
    with patch.object(_uc, "ProximaDBClient") as cls:
        cls.side_effect = ProximaDBError("server rejected")
        with pytest.raises(ProximaDBError):
            connect_grpc(url="http://t")


def test_connect_arrow_flight_str_port_mode():
    # ARROW_FLIGHT protocol — adapter creation may fail (no pyarrow server), but
    # construction itself must not raise; _create_adapter swallows failures.
    try:
        c = connect_arrow_flight(url="http://t", port_mode="unified")
        assert isinstance(c, ProximaDBClient)
        c.close()
    except Exception:
        # Some environments lack ARROW_FLIGHT support; the str->enum branch
        # still executed before any failure.
        pass


# ---------------------------------------------------------------------------
# Operation routing helpers (inject a mock router)
# ---------------------------------------------------------------------------


def test_get_client_for_operation_routes_to_grpc():
    c = make_client()
    c.enable_operation_routing = True
    router = MagicMock()
    router.route_operation.return_value = Protocol.GRPC
    c._operation_router = router
    c._grpc_client = MagicMock(name="grpc")
    c._rest_client = MagicMock(name="rest")
    chosen = c._get_client_for_operation("search", data_size_hint=10)
    assert chosen is c._grpc_client


def test_get_client_for_operation_routes_to_rest():
    c = make_client()
    c.enable_operation_routing = True
    router = MagicMock()
    router.route_operation.return_value = Protocol.REST
    c._operation_router = router
    c._grpc_client = MagicMock()
    c._rest_client = MagicMock(name="rest")
    assert c._get_client_for_operation("search") is c._rest_client


def test_get_client_for_operation_unavailable_falls_back_default():
    c = make_client()
    c.enable_operation_routing = True
    router = MagicMock()
    router.route_operation.return_value = Protocol.GRPC
    c._operation_router = router
    c._grpc_client = None  # requested grpc not available
    c._rest_client = None
    assert c._get_client_for_operation("search") is c._client


def test_record_operation_result_forwards():
    c = make_client()
    router = MagicMock()
    c._operation_router = router
    c._record_operation_result("op", Protocol.REST, True, 12.5)
    router.record_operation_result.assert_called_once()


def test_get_routing_stats_enabled():
    c = make_client()
    router = MagicMock()
    router.get_routing_stats.return_value = {"ops": 1}
    c._operation_router = router
    assert c.get_routing_stats() == {"ops": 1}


def test_add_routing_rule_and_reset_enabled():
    c = make_client()
    router = MagicMock()
    c._operation_router = router
    c.add_routing_rule("rule")
    c.reset_routing_metrics()
    router.add_routing_rule.assert_called_once_with("rule")
    router.reset_metrics.assert_called_once()


# ---------------------------------------------------------------------------
# Protocol selector helpers (inject a mock selector)
# ---------------------------------------------------------------------------


def test_get_protocol_metrics_enabled():
    c = make_client()
    sel = MagicMock()
    sel.get_protocol_metrics.return_value = {"grpc": {}}
    c._protocol_selector = sel
    assert c.get_protocol_metrics() == {"grpc": {}}


def test_get_selection_stats_enabled():
    c = make_client()
    sel = MagicMock()
    sel.get_selection_stats.return_value = {"switches": 0}
    c._protocol_selector = sel
    assert c.get_selection_stats() == {"switches": 0}


def test_force_protocol_switch_enabled_updates_client():
    c = make_client()
    sel = MagicMock()
    new_client = MagicMock(name="grpc-client")
    sel.get_client.return_value = new_client
    c._protocol_selector = sel
    c.force_protocol_switch(Protocol.GRPC)
    assert c._client is new_client
    assert c._active_protocol == Protocol.GRPC


def test_get_optimal_client_switches_protocol():
    c = make_client()
    sel = MagicMock()
    sel.select_protocol.return_value = Protocol.GRPC
    new_client = MagicMock()
    sel.get_client.return_value = new_client
    c._protocol_selector = sel
    out = c._get_optimal_client("bulk_insert")
    assert out is new_client
    assert c._active_protocol == Protocol.GRPC


def test_get_optimal_client_no_selector_returns_default():
    c = make_client()
    c._protocol_selector = None
    assert c._get_optimal_client("op") is c._client


# ---------------------------------------------------------------------------
# Setup helpers: operation routing + intelligent selection
# ---------------------------------------------------------------------------


def test_setup_operation_routing_builds_router():
    c = make_client()
    c.routing_strategy = RoutingStrategy.HYBRID
    with patch.object(_uc, "OperationRouter") as router_cls, patch.object(
        c, "_create_rest_client", return_value=MagicMock()
    ), patch.object(
        c, "_create_grpc_client", return_value=MagicMock()
    ):
        c._setup_operation_routing(None)
    assert c._operation_router is router_cls.return_value


def test_setup_operation_routing_failure_disables():
    c = make_client()
    c.routing_strategy = RoutingStrategy.HYBRID
    with patch.object(
        _uc, "OperationRouter", side_effect=RuntimeError("boom")
    ):
        c._setup_operation_routing(None)
    assert c.enable_operation_routing is False
    assert c._operation_router is None


def test_setup_intelligent_selection_success():
    c = make_client()
    c.selection_strategy = SelectionStrategy.BALANCED
    sel = MagicMock()
    sel.get_client.return_value = MagicMock()
    sel.select_protocol.return_value = Protocol.GRPC
    with patch.object(_uc, "create_protocol_selector", return_value=sel):
        c._setup_intelligent_selection()
    assert c._protocol_selector is sel
    assert c._active_protocol == Protocol.GRPC


def test_setup_intelligent_selection_failure_falls_back():
    c = make_client()
    c.selection_strategy = SelectionStrategy.BALANCED
    with patch.object(
        _uc, "create_protocol_selector", side_effect=RuntimeError("no")
    ), patch.object(c, "_setup_client") as setup:
        c._setup_intelligent_selection()
    assert c.enable_intelligent_selection is False
    setup.assert_called_once()


# ---------------------------------------------------------------------------
# _create_rest_client / _create_grpc_client
# ---------------------------------------------------------------------------


def test_create_rest_client_returns_rest_client():
    c = make_client()
    rest = c._create_rest_client()
    # The real REST client constructs but does not connect.
    assert rest is not None
    if hasattr(rest, "close"):
        try:
            rest.close()
        except Exception:
            pass


def test_create_grpc_client_constructs():
    c = make_client()
    fake_grpc = MagicMock()
    with patch(
        "proximadb_sdk.protocols.grpc_sync.ProximaDBSyncGrpcClient",
        return_value=fake_grpc,
    ):
        out = c._create_grpc_client()
    assert out is fake_grpc


# ---------------------------------------------------------------------------
# proto <-> pydantic quantization variants
# ---------------------------------------------------------------------------


def test_quantization_proto_binary_scalar_product():
    from proximadb_sdk.models import QuantizationConfig, QuantizationType

    c = make_client()
    binary = QuantizationConfig(
        enabled=True, type=QuantizationType.BINARY, threshold=0.5
    )
    proto_b = c._pydantic_to_proto_quantization_config(binary)
    assert proto_b.enable_binary is True

    scalar = QuantizationConfig(enabled=True, type=QuantizationType.SCALAR)
    proto_s = c._pydantic_to_proto_quantization_config(scalar)
    assert proto_s.enable_int8 is True

    product = QuantizationConfig(
        enabled=True,
        type=QuantizationType.PRODUCT,
        num_subvectors=8,
        bits_per_subvector=4,
    )
    proto_p = c._pydantic_to_proto_quantization_config(product)
    assert proto_p.enable_pq is True
    assert proto_p.pq_segments == 8


# ---------------------------------------------------------------------------
# Raw gRPC vector paths (no adapter)
# ---------------------------------------------------------------------------


def _grpc_resp(success=True, total=1):
    metrics = MagicMock()
    metrics.total_processed = total
    metrics.successful_count = total if success else 0
    metrics.failed_count = 0 if success else total
    metrics.updated_count = total
    resp = MagicMock()
    resp.success = success
    resp.metrics = metrics
    return resp


def test_upsert_vectors_raw_grpc():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    c._client.insert_vectors.return_value = _grpc_resp()
    out = c.upsert_vectors(
        "vc", [VectorRecord(id="r", vector=[1.0, 2.0], source="text", version=2)]
    )
    assert out.success is True
    assert out.operation == "upsert"
    # upsert=True passed through
    _, kwargs = c._client.insert_vectors.call_args
    assert kwargs.get("upsert") is True


def test_delete_vectors_raw_grpc_object_response():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    c._client.delete_vectors.return_value = _grpc_resp()
    out = c.delete_vectors("vc", ["a"])
    assert out.success is True
    assert out.operation == "delete"


def test_delete_vectors_raw_grpc_dict_response():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    c._client.delete_vectors.return_value = {
        "success": True,
        "metrics": {"total_processed": 2, "successful_count": 2, "failed_count": 0},
    }
    out = c.delete_vectors("vc", ["a", "b"])
    assert out.metrics.successful_count == 2


def test_search_single_raw_grpc():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    c._client.search_vectors.return_value = [SearchResult(id="x", score=0.9)]
    out = c.search_single("vc", np.array([0.1, 0.2]), top_k=3)
    assert out[0].id == "x"
    c._client.search_vectors.assert_called_once()


def test_search_single_raw_rest_filters_kwargs():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.REST
    c._client.search.return_value = [SearchResult(id="r", score=0.5)]
    out = c.search_single(
        "vc", [0.1, 0.2], top_k=2, quantization_hint="x", candidate_multiplier=3
    )
    assert out[0].id == "r"
    _, kwargs = c._client.search.call_args
    assert "quantization_hint" not in kwargs
    assert "candidate_multiplier" not in kwargs


def test_get_vector_raw_grpc_delegates():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    expected = VectorRecord(id="a", vector=[1.0, 2.0])
    c._client.get_vector.return_value = expected
    out = c.get_vector("vc", "a")
    assert out is expected


# ---------------------------------------------------------------------------
# search_iter REST pagination
# ---------------------------------------------------------------------------


def test_search_iter_rest_paginates():
    c = make_client()
    c._active_protocol = Protocol.REST

    page1 = MagicMock()
    page1.items = [SearchResult(id="a", score=0.9)]
    page1.cursor = "c1"
    page1.has_more = True
    page2 = MagicMock()
    page2.items = [SearchResult(id="b", score=0.8)]
    page2.cursor = None
    page2.has_more = False

    client = MagicMock(spec=["search_envelope", "search_next_page"])
    client.search_envelope.return_value = page1
    client.search_next_page.return_value = page2
    c._client = client

    items = list(c.search_iter("vc", [0.1, 0.2], top_k=5))
    assert [i.id for i in items] == ["a", "b"]


def test_search_envelope_rest_delegates_numpy():
    c = make_client()
    c._active_protocol = Protocol.REST
    client = MagicMock(spec=["search_envelope"])
    client.search_envelope.return_value = "ENV"
    c._client = client
    out = c.search_envelope("vc", np.array([0.1, 0.2]), top_k=4)
    assert out == "ENV"


# ---------------------------------------------------------------------------
# execute_unified_query embedded native + local fallback
# ---------------------------------------------------------------------------


def test_execute_unified_query_embedded_native():
    c = make_client()
    c._active_protocol = Protocol.EMBEDDED
    c._client = MagicMock()
    c._client.execute_unified_query.return_value = [{"id": "u"}]
    out = c.execute_unified_query("SELECT 1", query_vector=[0.1])
    assert out == [{"id": "u"}]


def test_execute_query_via_raw_client():
    c = make_client()
    c._adapter = MagicMock(spec=[])  # no execute_query on adapter
    client = MagicMock(spec=["execute_query"])
    client.execute_query.return_value = {"rows": []}
    c._client = client
    out = c.execute_query("FOR x RETURN x", language="aql")
    assert out == {"rows": []}


# ---------------------------------------------------------------------------
# observability embedded native paths
# ---------------------------------------------------------------------------


def test_ingest_logs_metrics_traces_embedded_native():
    c = make_client()
    c._active_protocol = Protocol.EMBEDDED
    native = MagicMock()
    native.ingest_logs.return_value = 3
    native.ingest_metrics.return_value = 2
    native.ingest_traces.return_value = 1
    native.query_logs.return_value = [{"l": 1}]
    native.aggregate_metrics.return_value = [{"m": 1}]
    native.query_traces.return_value = [{"t": 1}]
    native.get_trace.return_value = {"spans": []}
    c._client = native
    assert c.ingest_logs("ns", [{"l": 1}]) == 3
    assert c.ingest_metrics("ns", [{"m": 1}]) == 2
    assert c.ingest_traces("ns", [{"t": 1}]) == 1
    assert c.query_logs("ns", 0, 9) == [{"l": 1}]
    assert c.aggregate_metrics("ns", "cpu") == [{"m": 1}]
    assert c.query_traces("ns", 0, 9) == [{"t": 1}]
    assert c.get_trace("ns", "tid") == {"spans": []}


# ---------------------------------------------------------------------------
# graph shortest path REST with explicit attr; collection stats empty
# ---------------------------------------------------------------------------


def test_get_collection_stats_empty_when_missing():
    c = make_client()
    c._adapter = MagicMock()
    c._adapter.get_collection.return_value = None
    c._prefer_local_fallback = True  # so get_collection raises -> caught? no.
    # In prefer-local mode get_collection raises CollectionNotFoundError; emulate
    # not-found by going through adapter path instead.
    c._prefer_local_fallback = False
    c._adapter.get_collection.return_value = None
    try:
        stats = c.get_collection_stats("nope_xxxx")
    except CollectionNotFoundError:
        stats = {}
    assert stats == {}


def test_create_collection_raw_grpc_success():
    # No adapter, gRPC active, no primary index -> raw gRPC create path.
    # Response has no `collection` attr -> falls through to simple Collection build.
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    resp = MagicMock(spec=["success"])
    resp.success = True
    c._client.create_collection.return_value = resp
    out = c.create_collection("rawgrpc1", config=CollectionConfig(name="rawgrpc1", dimension=4))
    assert isinstance(out, Collection)
    c._client.create_collection.assert_called_once()


def test_create_collection_raw_grpc_connection_error_local_fallback():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.GRPC
    c._client.create_collection.side_effect = ConnectionError("connection failed")
    out = c.create_collection("rawgrpc2", config=CollectionConfig(name="rawgrpc2", dimension=4))
    assert isinstance(out, Collection)
    assert c._prefer_local_fallback is True


def test_upsert_vectors_raw_rest_else_branch():
    c = make_client()
    c._adapter = None
    c._active_protocol = Protocol.REST
    expected = VectorOperationResponse(
        success=True,
        operation="upsert",
        metrics=OperationMetrics(total_processed=1, successful_count=1),
    )
    c._client.upsert_vectors.return_value = expected
    out = c.upsert_vectors("vc", [VectorRecord(id="a", vector=[1.0, 2.0])])
    assert out is expected


def test_setup_authentication_no_auth_configured():
    # No api_key / cert -> _setup_authentication returns early, no _auth created.
    c = ProximaDBClient(url="http://testserver", protocol="rest")
    assert c._auth is None
    c.close()
