"""Offline unit tests for proximadb_sdk.adapters.grpc_adapter.

Fully offline: the underlying ProximaDBSyncGrpcClient is replaced with a hand
fake before the adapter constructs it, so no gRPC channel is ever opened.
"""

from __future__ import annotations

from typing import Any

import pytest
from pydantic import ValidationError

import proximadb_sdk.protocols.grpc_sync as grpc_sync_mod
from proximadb_sdk.adapters.grpc_adapter import GrpcProtocolAdapter
from proximadb_sdk.models import (
    Collection,
    CollectionConfig,
    OperationMetrics,
    SearchResult,
    VectorOperationResponse,
    VectorRecord,
)

# CollectionConfig.name requires >= 8 chars.
CNAME = "collection_one"


class FakeMetrics:
    def __init__(self, successful_count=0, failed_count=0, duration_ms=0):
        self.successful_count = successful_count
        self.failed_count = failed_count
        self.duration_ms = duration_ms


class FakeResultObj:
    """Generic wrapper object with attributes used by the adapter converters."""

    def __init__(self, **kwargs):
        for k, v in kwargs.items():
            setattr(self, k, v)


class FakeGrpcClient:
    """Hand fake standing in for ProximaDBSyncGrpcClient."""

    def __init__(self, *args, **kwargs):
        self.init_args = args
        self.init_kwargs = kwargs
        self.calls: dict[str, Any] = {}
        self.health_return = FakeResultObj(
            healthy=True, version="1.2.3", uptime_seconds=42, latency_ms=7
        )
        self.create_return = FakeResultObj(id="cid_abcd", name="coll_aaaa", dimension=8)
        self.get_return = FakeResultObj(id="cid_abcd", name="coll_aaaa", dimension=8)
        self.list_return = [FakeResultObj(id="c1_abcde", name="coll_bbbb", dimension=4)]
        self.delete_return = FakeResultObj(success=True)
        self.insert_return = FakeResultObj(success=True, metrics=FakeMetrics(2, 0, 1))
        self.get_vectors_return: Any = []
        self.search_return: Any = []
        self.closed = False

    def health_check(self):
        self.calls["health_check"] = True
        return self.health_return

    def create_collection(self, **kwargs):
        self.calls["create_collection"] = kwargs
        return self.create_return

    def get_collection(self, collection_id):
        self.calls["get_collection"] = collection_id
        return self.get_return

    def list_collections(self):
        self.calls["list_collections"] = True
        return self.list_return

    def delete_collection(self, collection_id):
        self.calls["delete_collection"] = collection_id
        return self.delete_return

    def insert_records(self, **kwargs):
        self.calls["insert_records"] = kwargs
        return self.insert_return

    def upsert_records(self, **kwargs):
        self.calls["upsert_records"] = kwargs
        return self.insert_return

    def get_vectors(self, collection_id, vector_ids, **kwargs):
        self.calls["get_vectors"] = (collection_id, vector_ids, kwargs)
        return self.get_vectors_return

    def delete_vectors(self, collection_id, vector_ids, **kwargs):
        self.calls["delete_vectors"] = (collection_id, vector_ids, kwargs)
        return FakeResultObj(success=True, metrics=FakeMetrics(1, 0, 2))

    def update_vector_metadata(self, collection_id, vector_id, metadata, **kwargs):
        self.calls["update_vector_metadata"] = (collection_id, vector_id, metadata)
        return FakeResultObj(success=True, metrics=FakeMetrics(1, 0, 0))

    def search_vectors(self, **kwargs):
        self.calls["search_vectors"] = kwargs
        return self.search_return

    def close(self):
        self.closed = True


@pytest.fixture
def patched_client(monkeypatch):
    """Patch the grpc client class; return a holder for the created fake."""
    holder: dict[str, Any] = {}

    def factory(*args, **kwargs):
        inst = FakeGrpcClient(*args, **kwargs)
        holder["client"] = inst
        return inst

    monkeypatch.setattr(grpc_sync_mod, "ProximaDBSyncGrpcClient", factory)
    return holder


def make_adapter(patched_client, **kwargs) -> GrpcProtocolAdapter:
    return GrpcProtocolAdapter(**kwargs)


# --------------------------------------------------------------------------
# Construction / properties
# --------------------------------------------------------------------------


def test_init_defaults(patched_client):
    a = make_adapter(patched_client)
    assert a.protocol_name == "grpc"
    assert a.is_connected is True
    assert a._server_address == "localhost:5678"


def test_init_config_url_override(patched_client):
    cfg = FakeResultObj(url="http://myhost:9999")
    a = make_adapter(patched_client, config=cfg)
    assert a._server_address == "myhost:9999"


def test_init_config_base_url_https(patched_client):
    cfg = FakeResultObj(url=None, base_url="https://secure:8443")
    a = make_adapter(patched_client, config=cfg)
    assert a._server_address == "secure:8443"


def test_init_config_no_url_keeps_default(patched_client):
    cfg = FakeResultObj(url=None, base_url=None)
    a = make_adapter(patched_client, config=cfg)
    assert a._server_address == "localhost:5678"


def test_init_explicit_address_ignores_config(patched_client):
    cfg = FakeResultObj(url="http://other:1")
    a = make_adapter(patched_client, server_address="explicit:1234", config=cfg)
    assert a._server_address == "explicit:1234"


def test_init_strips_noise_kwargs(patched_client):
    make_adapter(patched_client, auth="x", url="http://z", base_url="http://z")
    assert "auth" not in patched_client["client"].init_kwargs


# --------------------------------------------------------------------------
# Health
# --------------------------------------------------------------------------


def test_health_healthy(patched_client):
    a = make_adapter(patched_client)
    hs = a.health()
    assert hs.status == "healthy"
    assert hs.version == "1.2.3"
    assert hs.uptime_seconds == 42
    assert hs.services["grpc"] == "ok"


def test_health_unhealthy(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].health_return = FakeResultObj(
        healthy=False, version="0.1", uptime_seconds=1, latency_ms=-5
    )
    hs = a.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unavailable"
    assert hs.timestamp_ms == 0


def test_health_no_healthy_attr(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].health_return = FakeResultObj(foo="bar")
    hs = a.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unknown"


def test_health_exception(patched_client):
    a = make_adapter(patched_client)

    def boom():
        raise RuntimeError("down")

    patched_client["client"].health_check = boom
    hs = a.health()
    assert hs.status == "running"
    assert hs.services["grpc"] == "unavailable"


# --------------------------------------------------------------------------
# Collections
# --------------------------------------------------------------------------


def test_create_collection_with_config(patched_client):
    a = make_adapter(patched_client)
    cfg = CollectionConfig(name=CNAME, dimension=8, distance_metric="cosine")
    coll = a.create_collection(CNAME, config=cfg)
    assert isinstance(coll, Collection)
    assert patched_client["client"].calls["create_collection"]["dimension"] == 8


def test_create_collection_kwargs_only(patched_client):
    a = make_adapter(patched_client)
    coll = a.create_collection(CNAME, dimension=16, extra="ignored")
    assert isinstance(coll, Collection)
    assert patched_client["client"].calls["create_collection"]["dimension"] == 16


def test_create_collection_precision_mapping(patched_client):
    a = make_adapter(patched_client)
    cfg = CollectionConfig(
        name=CNAME, dimension=8, canonical_embedding_precision="fp16"
    )
    a.create_collection(CNAME, config=cfg)
    assert (
        patched_client["client"].calls["create_collection"][
            "canonical_embedding_precision"
        ]
        == 2
    )


def test_create_collection_precision_via_kwargs_enum_value(patched_client):
    a = make_adapter(patched_client)

    class P:
        value = "int8"

    a.create_collection(CNAME, dimension=4, canonical_embedding_precision=P())
    assert (
        patched_client["client"].calls["create_collection"][
            "canonical_embedding_precision"
        ]
        == 4
    )


def test_get_collection_found(patched_client):
    a = make_adapter(patched_client)
    assert isinstance(a.get_collection("cid"), Collection)


def test_get_collection_none(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].get_return = None
    assert a.get_collection("nope") is None


def test_get_collection_exception(patched_client):
    a = make_adapter(patched_client)

    def boom(cid):
        raise RuntimeError("x")

    patched_client["client"].get_collection = boom
    assert a.get_collection("cid") is None


def test_list_collections(patched_client):
    a = make_adapter(patched_client)
    out = a.list_collections()
    assert len(out) == 1
    assert isinstance(out[0], Collection)


def test_list_collections_conversion_error_skipped(patched_client):
    a = make_adapter(patched_client)

    class Bad:
        @property
        def name(self):
            raise ValueError("bad")

    patched_client["client"].list_return = [Bad()]
    assert a.list_collections() == []


def test_delete_collection_success_attr(patched_client):
    a = make_adapter(patched_client)
    assert a.delete_collection("cid") is True


def test_delete_collection_no_success_attr(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].delete_return = FakeResultObj(other=1)
    assert a.delete_collection("cid") is True


def test_delete_collection_exception(patched_client):
    a = make_adapter(patched_client)

    def boom(cid):
        raise RuntimeError("x")

    patched_client["client"].delete_collection = boom
    assert a.delete_collection("cid") is False


# --------------------------------------------------------------------------
# _to_collection direct branches
# --------------------------------------------------------------------------


def test_to_collection_passthrough(patched_client):
    a = make_adapter(patched_client)
    existing = Collection(id="x", config=CollectionConfig(name=CNAME, dimension=2))
    assert a._to_collection(existing) is existing


def test_to_collection_dict_with_config(patched_client):
    a = make_adapter(patched_client)
    out = a._to_collection({"id": "x", "config": {"name": CNAME, "dimension": 3}})
    assert out.id == "x"


def test_to_collection_dict_without_config(patched_client):
    a = make_adapter(patched_client)
    out = a._to_collection({"id": "x", "name": CNAME, "dimension": 5})
    assert out.config.dimension == 5


def test_to_collection_object_fallbacks(patched_client):
    a = make_adapter(patched_client)
    out = a._to_collection(
        FakeResultObj(), fallback_name="fallback_name", fallback_dimension=9
    )
    assert out.config.name == "fallback_name"
    assert out.config.dimension == 9


# --------------------------------------------------------------------------
# Records: insert / upsert
# --------------------------------------------------------------------------


def test_insert_records_dict_payloads(patched_client):
    a = make_adapter(patched_client)
    res = a.insert_records("cid", [{"id": "a", "vector": [1.0]}])
    assert res.total == 1
    assert res.success == 2


def test_insert_records_plain_object_via_proto_converter(patched_client):
    # record that is neither dict nor has model_dump -> ProtoConverter path
    a = make_adapter(patched_client)

    class PlainRec:
        id = "p1"
        vector = [1.0, 2.0]
        metadata = None

    res = a.insert_records("cid", [PlainRec()])
    assert res.total == 1
    sent = patched_client["client"].calls["insert_records"]["records"][0]
    assert sent["id"] == "p1"


def test_insert_records_model_dump(patched_client):
    a = make_adapter(patched_client)
    rec = VectorRecord(id="a", vector=[1.0, 2.0], metadata={"k": "v"})
    res = a.insert_records("cid", [rec])
    assert res.total == 1


def test_insert_records_fallback_insert_vectors(patched_client):
    a = make_adapter(patched_client)

    class OnlyInsertVectors:
        def __init__(self):
            self.calls = {}

        def insert_vectors(self, **kwargs):
            self.calls["insert_vectors"] = kwargs
            return FakeResultObj(success=True, metrics=FakeMetrics(1, 0, 0))

    fake = OnlyInsertVectors()
    a._client = fake
    res = a.insert_records("cid", [{"id": "a"}])
    assert "insert_vectors" in fake.calls
    assert res.total == 1


def test_upsert_records(patched_client):
    a = make_adapter(patched_client)
    res = a.upsert_records("cid", [{"id": "a"}])
    assert res.total == 1
    assert "upsert_records" in patched_client["client"].calls


def test_upsert_records_fallback(patched_client):
    a = make_adapter(patched_client)

    class OnlyInsertVectors:
        def __init__(self):
            self.calls = {}

        def insert_vectors(self, **kwargs):
            self.calls["insert_vectors"] = kwargs
            return FakeResultObj(success=True, metrics=FakeMetrics(1, 0, 0))

    fake = OnlyInsertVectors()
    a._client = fake
    res = a.upsert_records("cid", [{"id": "a"}])
    assert fake.calls["insert_vectors"]["upsert"] is True
    assert res.total == 1


def test_insert_records_with_error_message(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].insert_return = FakeResultObj(
        success=False, metrics=FakeMetrics(0, 1, 0), error_message="boom"
    )
    res = a.insert_records("cid", [{"id": "a"}])
    assert res.errors == ["boom"]
    assert res.failed == 1


# --------------------------------------------------------------------------
# Vector compatibility aliases
# --------------------------------------------------------------------------


def test_insert_vectors_alias(patched_client):
    a = make_adapter(patched_client)
    resp = a.insert_vectors("cid", [{"id": "a"}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "INSERT"


def test_upsert_vectors_alias(patched_client):
    a = make_adapter(patched_client)
    resp = a.upsert_vectors("cid", [{"id": "a"}])
    assert isinstance(resp, VectorOperationResponse)
    assert resp.operation == "UPSERT"


# --------------------------------------------------------------------------
# get_vectors
# --------------------------------------------------------------------------


def test_get_vectors_various_shapes(patched_client):
    a = make_adapter(patched_client)
    client = patched_client["client"]
    existing = VectorRecord(id="v0", vector=[0.0])
    client.get_vectors_return = [
        existing,
        {"id": "v1", "vector": [1.0], "metadata": {}},
        FakeResultObj(id="v2", vector=[2.0], metadata={"a": "b"}),
        FakeResultObj(),  # no .id -> skipped
    ]
    out = a.get_vectors("cid", ["v0", "v1", "v2"])
    assert [r.id for r in out] == ["v0", "v1", "v2"]


def test_get_vectors_none_result(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].get_vectors_return = None
    assert a.get_vectors("cid", ["x"]) == []


def test_get_vectors_not_implemented(patched_client):
    a = make_adapter(patched_client)
    a._client = object()  # has no get_vectors
    assert a.get_vectors("cid", ["x"]) == []


# --------------------------------------------------------------------------
# delete_vectors / update_vector_metadata
# --------------------------------------------------------------------------


def test_delete_vectors(patched_client):
    a = make_adapter(patched_client)
    resp = a.delete_vectors("cid", ["a", "b"])
    assert resp.operation == "DELETE"
    assert resp.success is True


def test_delete_vectors_not_implemented(patched_client):
    a = make_adapter(patched_client)
    a._client = object()  # has no delete_vectors
    # The source's fallback builds VectorOperationResponse without `metrics`,
    # which is a required field -> pydantic ValidationError. The fallback
    # branch is still executed (covered) before raising.
    with pytest.raises(ValidationError):
        a.delete_vectors("cid", ["a"])


def test_update_vector_metadata(patched_client):
    a = make_adapter(patched_client)
    resp = a.update_vector_metadata("cid", "v1", {"k": "v"})
    assert resp.operation == "UPDATE"
    assert resp.success is True


def test_update_vector_metadata_not_implemented(patched_client):
    a = make_adapter(patched_client)
    a._client = object()  # has no update_vector_metadata
    # Same as delete: fallback omits required `metrics` -> ValidationError.
    with pytest.raises(ValidationError):
        a.update_vector_metadata("cid", "v1", {"k": "v"})


# --------------------------------------------------------------------------
# _to_vector_operation_response branches
# --------------------------------------------------------------------------


def test_to_vop_passthrough(patched_client):
    a = make_adapter(patched_client)
    existing = VectorOperationResponse(
        success=True, operation="X", metrics=OperationMetrics()
    )
    assert a._to_vector_operation_response(existing, "INSERT", 1) is existing


def test_to_vop_dict(patched_client):
    a = make_adapter(patched_client)
    resp = a._to_vector_operation_response(
        {"success": True, "successful_count": 4, "failed_count": 1}, "INSERT", 5
    )
    assert resp.metrics.successful_count == 4
    assert resp.metrics.failed_count == 1


def test_to_vop_object_no_metrics_success(patched_client):
    a = make_adapter(patched_client)
    resp = a._to_vector_operation_response(
        FakeResultObj(success=True, metrics=None), "INSERT", 3
    )
    assert resp.metrics.successful_count == 3
    assert resp.metrics.failed_count == 0


def test_to_vop_object_no_metrics_failure(patched_client):
    a = make_adapter(patched_client)
    resp = a._to_vector_operation_response(
        FakeResultObj(success=False, metrics=None, error_message="e"), "INSERT", 3
    )
    assert resp.metrics.successful_count == 0
    assert resp.metrics.failed_count == 3
    assert resp.error_message == "e"


# --------------------------------------------------------------------------
# Search
# --------------------------------------------------------------------------


def test_search_dict_results(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = [
        {"id": "a", "score": 0.9, "vector": [1.0], "metadata": {"k": "v"}},
        {"vector_id": "b", "distance": 0.1},
    ]
    out = a.search("cid", [0.1, 0.2], top_k=2, include_vectors=True)
    assert out[0].id == "a"
    assert out[1].id == "b"


def test_search_numpy_like_query(patched_client):
    a = make_adapter(patched_client)

    class NP:
        def tolist(self):
            return [0.5, 0.6]

    patched_client["client"].search_return = []
    a.search("cid", NP())
    assert patched_client["client"].calls["search_vectors"]["query_vector"] == [
        0.5,
        0.6,
    ]


def test_search_object_results(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = [
        FakeResultObj(id="o1", score=0.7, vector=[1.0, 2.0], metadata={"m": 1}),
        FakeResultObj(id="o2", distance=0.3, vector=None, metadata=None),
        SearchResult(id="pre", score=0.99),
    ]
    out = a.search("cid", [1.0], include_vectors=True, include_metadata=True)
    assert {r.id for r in out} == {"o1", "o2", "pre"}


def test_search_object_missing_vector_and_metadata_attrs(patched_client):
    # include flags True but the result object lacks .vector / .metadata attrs,
    # exercising the partial-branch arms of _to_search_results.
    a = make_adapter(patched_client)

    class OnlyId:
        id = "only"
        score = 0.4

    patched_client["client"].search_return = [OnlyId()]
    out = a.search("cid", [1.0], include_vectors=True, include_metadata=True)
    assert out[0].id == "only"
    assert out[0].vector is None
    assert out[0].metadata is None


def test_search_object_with_falsy_vector_metadata(patched_client):
    # object has .vector/.metadata but they are empty/falsy -> None / {} arms.
    a = make_adapter(patched_client)

    class FalsyAttrs:
        id = "f"
        score = 0.1
        vector = []
        metadata = {}

    patched_client["client"].search_return = [FalsyAttrs()]
    out = a.search("cid", [1.0], include_vectors=True, include_metadata=True)
    assert out[0].vector is None
    assert out[0].metadata == {}


def test_search_skips_object_without_id(patched_client):
    a = make_adapter(patched_client)

    class WithId:
        id = "keep"
        score = 0.9

    patched_client["client"].search_return = [WithId(), object()]
    out = a.search("cid", [1.0])
    assert [r.id for r in out] == ["keep"]


def test_search_none_results(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = None
    assert a.search("cid", [1.0]) == []


def test_batch_search_single_query_wrapped(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = [{"id": "a", "score": 0.9}]
    out = a.batch_search("cid", [[0.1], [0.2]])
    assert len(out) == 1
    assert out[0][0].id == "a"


def test_batch_search_nested_results(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = [
        [{"id": "a", "score": 0.9}],
        [{"id": "b", "score": 0.8}],
    ]
    out = a.batch_search("cid", [[0.1], [0.2]])
    assert len(out) == 2
    assert out[0][0].id == "a"
    assert out[1][0].id == "b"


def test_batch_search_numpy_queries(patched_client):
    a = make_adapter(patched_client)

    class NP:
        def __init__(self, data):
            self._d = data

        def tolist(self):
            return self._d

    patched_client["client"].search_return = []
    a.batch_search("cid", [NP([1.0]), [2.0]])
    sent = patched_client["client"].calls["search_vectors"]["query_vectors"]
    assert sent == [[1.0], [2.0]]


def test_batch_search_empty_results(patched_client):
    a = make_adapter(patched_client)
    patched_client["client"].search_return = []
    assert a.batch_search("cid", [[0.1]]) == []


# --------------------------------------------------------------------------
# close
# --------------------------------------------------------------------------


def test_close(patched_client):
    a = make_adapter(patched_client)
    a.close()
    assert a.is_connected is False
    assert patched_client["client"].closed is True


def test_close_no_close_method(patched_client):
    a = make_adapter(patched_client)
    a._client = object()  # has no close()
    a.close()
    assert a.is_connected is False
