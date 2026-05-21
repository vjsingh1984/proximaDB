"""Adapter-layer record write convergence tests."""

import pytest

from proximadb_sdk.adapters.rest_adapter import RestProtocolAdapter
from proximadb_sdk.config import Protocol
from proximadb_sdk.models import BatchResult, OperationMetrics
from proximadb_sdk.models_v2 import ProximaRecord, TypedValue
from proximadb_sdk.protocols.grpc_sync import ProximaDBSyncGrpcClient
from proximadb_sdk.unified_client import ProximaDBClient as LegacyProximaDBClient
from proximadb_sdk.unified_client_v2 import ProximaDBClient


class RecordingRestClient:
    def __init__(self):
        self.calls = []

    def insert_records(self, collection_id, records, **kwargs):
        self.calls.append(("insert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    def upsert_records(self, collection_id, records, **kwargs):
        self.calls.append(("upsert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    def execute_query(self, query, **kwargs):
        self.calls.append(("execute_query", query, kwargs))
        return {"records": [{"id": "r1"}], "total_count": 1}

    def explain_query(self, query, **kwargs):
        self.calls.append(("explain_query", query, kwargs))
        return {"plan": {"root": "scan"}}


def make_rest_adapter(client):
    adapter = RestProtocolAdapter.__new__(RestProtocolAdapter)
    adapter._client = client
    adapter._url = "http://localhost:5678"
    adapter._connected = True
    return adapter


def test_rest_adapter_insert_vectors_delegates_to_record_insert():
    client = RecordingRestClient()
    adapter = make_rest_adapter(client)

    response = adapter.insert_vectors(
        "items",
        [{"id": "r1", "vector": [0.1, 0.2], "props": {"kind": "note"}}],
    )

    assert response.success == 1
    assert client.calls == [
        (
            "insert_records",
            "items",
            [{"id": "r1", "vector": [0.1, 0.2], "props": {"kind": "note"}}],
            {},
        )
    ]


def test_rest_adapter_upsert_vectors_delegates_to_record_upsert():
    client = RecordingRestClient()
    adapter = make_rest_adapter(client)

    response = adapter.upsert_vectors(
        "items",
        [{"id": "r1", "vector": [0.1, 0.2], "props": {"kind": "note"}}],
    )

    assert response.success == 1
    assert client.calls[0][0] == "upsert_records"


def test_rest_adapter_query_methods_delegate_to_openapi_client():
    client = RecordingRestClient()
    adapter = make_rest_adapter(client)

    result = adapter.execute_uql(
        "SEARCH items RETURN id",
        parameters={"tenant": "acme"},
        collection="items",
        limit=5,
    )
    federated = adapter.execute_federated(
        "SELECT * FROM VECTOR_SEARCH('items', ?, ?)",
        parameters=[[0.1], 5],
    )
    plan = adapter.explain_query("SEARCH items RETURN id", language="uql")

    assert result == {"records": [{"id": "r1"}], "total_count": 1}
    assert federated == {"records": [{"id": "r1"}], "total_count": 1}
    assert plan == {"plan": {"root": "scan"}}
    assert client.calls == [
        (
            "execute_query",
            "SEARCH items RETURN id",
            {
                "language": "uql",
                "parameters": {"tenant": "acme"},
                "collection": "items",
                "limit": 5,
            },
        ),
        (
            "execute_query",
            "SELECT * FROM VECTOR_SEARCH('items', ?, ?)",
            {
                "language": "federated",
                "parameters": [[0.1], 5],
                "collection": None,
                "limit": None,
            },
        ),
        (
            "explain_query",
            "SEARCH items RETURN id",
            {"language": "uql", "collection": None},
        ),
    ]



class RecordingUnifiedAdapter:
    def __init__(self):
        self.calls = []

    def insert_records(self, collection_id, records, **kwargs):
        self.calls.append(("insert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    def upsert_records(self, collection_id, records, **kwargs):
        self.calls.append(("upsert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    def execute_query(self, query, **kwargs):
        self.calls.append(("execute_query", query, kwargs))
        return {"records": [{"id": "r1"}], "total_count": 1}

    def explain_query(self, query, **kwargs):
        self.calls.append(("explain_query", query, kwargs))
        return {"plan": {"root": "scan"}}


def make_unified_client(adapter):
    client = ProximaDBClient.__new__(ProximaDBClient)
    client._adapter = adapter
    return client


def test_unified_client_insert_vectors_delegates_to_record_insert():
    adapter = RecordingUnifiedAdapter()
    client = make_unified_client(adapter)

    response = client.insert_vectors(
        "items",
        vectors=[[1.0, 2.0]],
        ids=["r1"],
        metadata=[{"kind": "note"}],
    )

    assert response.success == 1
    method, collection, records, _kwargs = adapter.calls[0]
    assert method == "insert_records"
    assert collection == "items"
    assert records[0].id == "r1"
    assert records[0].metadata == {"kind": "note"}


def test_unified_client_insert_records_uses_adapter_record_contract():
    adapter = RecordingUnifiedAdapter()
    client = make_unified_client(adapter)

    result = client.insert_records(
        "items",
        [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
    )

    assert result.success == 1
    assert adapter.calls == [
        (
            "insert_records",
            "items",
            [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
            {},
        )
    ]


def test_unified_client_v2_query_methods_delegate_to_adapter():
    adapter = RecordingUnifiedAdapter()
    client = make_unified_client(adapter)

    result = client.execute_aql(
        "FIND related entities",
        parameters={"tenant": "acme"},
        collection="graph",
        limit=20,
    )
    plan = client.explain_query("SEARCH items RETURN id", language="uql")

    assert result == {"records": [{"id": "r1"}], "total_count": 1}
    assert plan == {"plan": {"root": "scan"}}
    assert adapter.calls[-2:] == [
        (
            "execute_query",
            "FIND related entities",
            {
                "language": "aql",
                "parameters": {"tenant": "acme"},
                "collection": "graph",
                "limit": 20,
            },
        ),
        (
            "explain_query",
            "SEARCH items RETURN id",
            {"language": "uql", "collection": None},
        ),
    ]


def test_legacy_unified_client_vector_insert_prefers_record_contract():
    adapter = RecordingUnifiedAdapter()
    client = LegacyProximaDBClient.__new__(LegacyProximaDBClient)
    client._adapter = adapter
    client._prefer_local_fallback = False
    client._rest_client = None
    client._grpc_client = None

    response = client.insert_vectors(
        "items",
        vectors=[[1.0, 2.0]],
        ids=["r1"],
        metadata=[{"kind": "note"}],
    )

    assert response.success == 1
    method, collection, records, _kwargs = adapter.calls[0]
    assert method == "insert_records"
    assert collection == "items"
    assert records == [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}]


def test_legacy_unified_client_vector_upsert_prefers_record_contract():
    adapter = RecordingUnifiedAdapter()
    client = LegacyProximaDBClient.__new__(LegacyProximaDBClient)
    client._adapter = adapter
    client._prefer_local_fallback = False
    client._rest_client = None
    client._grpc_client = None

    response = client.upsert_vectors(
        "items",
        [{"id": "r1", "vector": [1.0, 2.0], "metadata": {"kind": "note"}}],
    )

    assert response.success == 1
    method, collection, records, _kwargs = adapter.calls[0]
    assert method == "upsert_records"
    assert collection == "items"
    assert records == [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}]


def test_legacy_unified_client_uql_uses_rest_query_contract():
    adapter = RecordingUnifiedAdapter()
    client = LegacyProximaDBClient.__new__(LegacyProximaDBClient)
    client._adapter = adapter
    client._client = None

    result = client.execute_uql(
        "SEARCH items RETURN id",
        parameters=["tenant-a"],
        collection="items",
        limit=10,
    )

    assert result == {"records": [{"id": "r1"}], "total_count": 1}
    assert adapter.calls[-1] == (
        "execute_query",
        "SEARCH items RETURN id",
        {
            "language": "uql",
            "parameters": ["tenant-a"],
            "collection": "items",
            "limit": 10,
        },
    )


def test_legacy_execute_unified_query_returns_records_from_v2_query():
    adapter = RecordingUnifiedAdapter()
    client = LegacyProximaDBClient.__new__(LegacyProximaDBClient)
    client._adapter = adapter
    client._active_protocol = None

    result = client.execute_unified_query("SEARCH items RETURN id")

    assert result == [{"id": "r1"}]
    assert adapter.calls[-1] == (
        "execute_query",
        "SEARCH items RETURN id",
        {"language": "uql"},
    )


def test_legacy_unified_client_exposes_rest_search_envelope():
    class RecordingRestSearchClient:
        def __init__(self):
            self.calls = []

        def search_envelope(self, **kwargs):
            self.calls.append(kwargs)
            return {"items": [{"id": "r1"}], "has_more": False}

    rest_client = RecordingRestSearchClient()
    client = LegacyProximaDBClient.__new__(LegacyProximaDBClient)
    client._active_protocol = Protocol.REST
    client._client = rest_client

    result = client.search_envelope(
        "items",
        [0.1, 0.2],
        top_k=1,
        include_vectors=True,
        include_metadata=False,
    )

    assert result == {"items": [{"id": "r1"}], "has_more": False}
    assert rest_client.calls == [
        {
            "collection_id": "items",
            "vector": [0.1, 0.2],
            "top_k": 1,
            "include_vectors": True,
            "include_metadata": False,
        }
    ]


def test_grpc_sync_client_builds_v2_record_proto():
    from proximadb.v2 import record_pb2

    client = ProximaDBSyncGrpcClient.__new__(ProximaDBSyncGrpcClient)
    record = ProximaRecord(
        id="r1",
        vector=[1.0, 2.0],
        typed_fields={"price": TypedValue.float_(9.99)},
        flexible_fields={"kind": "note"},
    )

    proto = client._record_proto_for_grpc(record)

    assert proto.id == "r1"
    assert list(proto.vector) == [1.0, 2.0]
    assert proto.props["kind"].declared_type == record_pb2.TEXT
    assert proto.props["kind"].text_value == "note"
    assert proto.props["price"].declared_type == record_pb2.FLOAT
    assert proto.props["price"].float_value == 9.99


class RecordingGrpcClient(ProximaDBSyncGrpcClient):
    def __init__(self):
        self.calls = []

    def insert_records(self, collection_id, records, **kwargs):
        self.calls.append(("insert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )

    def upsert_records(self, collection_id, records, **kwargs):
        self.calls.append(("upsert_records", collection_id, records, kwargs))
        return BatchResult(
            total=len(records),
            success=len(records),
            failed=0,
            metrics=OperationMetrics(
                total_processed=len(records),
                successful_count=len(records),
                failed_count=0,
            ),
        )


def test_grpc_sync_insert_vectors_delegates_to_v2_record_insert():
    client = RecordingGrpcClient()

    response = client.insert_vectors(
        "items",
        [{"id": "r1", "vector": [1.0, 2.0], "metadata": {"kind": "note"}}],
    )

    assert response.success is True
    assert client.calls == [
        (
            "insert_records",
            "items",
            [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
            {},
        )
    ]


def test_grpc_sync_upsert_vectors_delegates_to_v2_record_upsert():
    client = RecordingGrpcClient()

    response = client.insert_vectors(
        "items",
        [{"id": "r1", "vector": [1.0, 2.0], "metadata": {"kind": "note"}}],
        upsert=True,
    )

    assert response.success is True
    assert client.calls == [
        (
            "upsert_records",
            "items",
            [{"id": "r1", "vector": [1.0, 2.0], "props": {"kind": "note"}}],
            {},
        )
    ]


def test_grpc_sync_search_vectors_builds_v2_typed_search_request():
    from proximadb.v2 import record_pb2

    captured = {}
    client = ProximaDBSyncGrpcClient.__new__(ProximaDBSyncGrpcClient)
    client.timeout = 1.0

    class Stub:
        def Search(self, request, timeout=None):
            captured["request"] = request
            result = record_pb2.TypedSearchResult(id="r1", score=0.9)
            result.props["kind"].CopyFrom(client._python_to_v2_typed_value("note"))
            return record_pb2.TypedSearchResponse(results=[result])

    client._execute_record_with_pool = lambda _name, op: op(Stub())

    response = client.search_vectors(
        "items",
        query_vector=[0.1, 0.2],
        top_k=3,
        metadata_filters={"tenant": "acme"},
    )

    request = captured["request"]
    assert isinstance(request, record_pb2.TypedSearchRequest)
    assert request.collection_id == "items"
    assert list(request.query_vector) == pytest.approx([0.1, 0.2])
    assert request.top_k == 3
    assert request.filters[0].field_name == "tenant"
    assert request.filters[0].operator == record_pb2.EQ
    assert response.results[0].metadata == {"kind": "note"}


def test_grpc_sync_delete_vectors_builds_v2_delete_batch():
    from proximadb.v2 import record_pb2

    captured = {}
    client = ProximaDBSyncGrpcClient.__new__(ProximaDBSyncGrpcClient)
    client.timeout = 1.0

    class Stub:
        def DeleteRecords(self, request, timeout=None):
            captured["request"] = request
            return record_pb2.ProximaRecordBatchResponse(
                success=True,
                total_processed=len(request.records),
                success_count=len(request.records),
                failed_count=0,
            )

    client._execute_record_with_pool = lambda _name, op: op(Stub())

    response = client.delete_vectors("items", ["r1", "r2"])

    request = captured["request"]
    assert isinstance(request, record_pb2.ProximaRecordBatch)
    assert request.collection_id == "items"
    assert request.write_mode == record_pb2.DELETE
    assert [record.id for record in request.records] == ["r1", "r2"]
    assert response["deleted_count"] == 2
