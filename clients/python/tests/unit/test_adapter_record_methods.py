"""Adapter-layer record write convergence tests."""

from proximadb_sdk.adapters.rest_adapter import RestProtocolAdapter
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
