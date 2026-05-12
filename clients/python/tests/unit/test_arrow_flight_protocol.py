import json

import pytest

from proximadb_sdk.protocols.arrow_flight import (
    ARROW_AVAILABLE,
    ArrowFlightClient,
    FlightPutResult,
    WriteMode,
    pa,
)


class _Buffer:
    def __init__(self, payload):
        self._payload = payload

    def to_pybytes(self):
        return self._payload


class _Chunk:
    def __init__(self, payload):
        self.app_metadata = payload


class _ExchangeWriter:
    def __init__(self):
        self.schema = None
        self.batches = []
        self.closed = False

    def begin(self, schema):
        self.schema = schema

    def write_batch(self, batch):
        self.batches.append(batch)

    def close(self):
        self.closed = True


class _ExchangeClient:
    def __init__(self, chunks):
        self.chunks = chunks
        self.writer = _ExchangeWriter()
        self.descriptor = None
        self.options = None

    def do_exchange(self, descriptor, options=None):
        self.descriptor = descriptor
        self.options = options
        return self.writer, iter(self.chunks)


class _PutReader:
    def __init__(self, payload):
        self.payload = payload

    def read(self):
        return _Buffer(json.dumps(self.payload).encode())


class _PutWriter:
    def __init__(self):
        self.batches = []
        self.closed = False

    def write_batch(self, batch):
        self.batches.append(batch)

    def close(self):
        self.closed = True


class _PutClient:
    def __init__(self, payload):
        self.payload = payload
        self.writer = _PutWriter()
        self.descriptor = None
        self.schema = None
        self.options = None

    def do_put(self, descriptor, schema, options=None):
        self.descriptor = descriptor
        self.schema = schema
        self.options = options
        return self.writer, _PutReader(self.payload)


def _client_with_exchange(fake_client):
    client = ArrowFlightClient.__new__(ArrowFlightClient)
    client._get_client = lambda: fake_client
    client._get_call_options = lambda: None
    return client


def _client_with_put(fake_client):
    client = ArrowFlightClient.__new__(ArrowFlightClient)
    client._get_client = lambda: fake_client
    client._get_call_options = lambda: None
    return client


def test_affected_count_uses_successful_count():
    metadata = {"metrics": {"successful_count": 7, "total_processed": 9}}

    assert ArrowFlightClient._affected_count(metadata, fallback=3) == 7


def test_affected_count_falls_back_to_total_processed():
    metadata = {"metrics": {"total_processed": 9}}

    assert ArrowFlightClient._affected_count(metadata, fallback=3) == 9


def test_affected_count_falls_back_to_rows():
    assert ArrowFlightClient._affected_count({}, fallback=3) == 3


def test_flight_put_result_record_aliases():
    result = FlightPutResult(
        success=True,
        vectors_inserted=7,
        message="ok",
        metadata={"metrics": {"failed_count": 2}, "errors": ["ignored"]},
    )

    assert result.records_processed == 7
    assert result.records_failed == 2


def test_flight_put_result_records_failed_falls_back_to_errors():
    result = FlightPutResult(
        success=False,
        vectors_inserted=0,
        message="bad",
        metadata={"errors": ["a", "b"]},
    )

    assert result.records_failed == 2


def test_decode_metadata_accepts_pyarrow_buffer_shape():
    payload = {"type": "complete", "operation": "upsert", "total_records": 12}
    encoded = json.dumps(payload).encode()

    assert ArrowFlightClient._decode_metadata(_Buffer(encoded)) == payload


def test_metadata_from_exchange_chunk_reads_app_metadata():
    payload = {"type": "progress", "operation": "delete", "batch": 2}
    encoded = json.dumps(payload).encode()

    assert ArrowFlightClient._metadata_from_exchange_chunk(_Chunk(encoded)) == payload


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_call_options_include_auth_and_tenant_headers():
    client = ArrowFlightClient("localhost:5680", api_key="token-1", tenant_id="tenant-a")

    options = client._get_call_options()

    assert (b"authorization", b"Bearer token-1") in options.headers
    assert (b"x-proximadb-tenant-id", b"tenant-a") in options.headers


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_upsert_doput_sends_upsert_descriptor_and_batches():
    fake = _PutClient(
        {
            "success": True,
            "metrics": {"successful_count": 3, "failed_count": 0},
        }
    )
    client = _client_with_put(fake)
    table = pa.table({"id": ["r1", "r2", "r3"], "category": ["a", "b", "c"]})

    result = client.bulk_upsert("records", table, batch_size=2)

    descriptor = json.loads(fake.descriptor.command)
    assert descriptor["collection_id"] == "records"
    assert descriptor["operation"] == "upsert"
    assert fake.schema == table.schema
    assert [batch.num_rows for batch in fake.writer.batches] == [2, 1]
    assert fake.writer.closed
    assert result.success
    assert result.records_processed == 3


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_upsert_doput_warns_for_direct_write_mode():
    fake = _PutClient(
        {
            "success": True,
            "metrics": {"successful_count": 1, "failed_count": 0},
        }
    )
    client = _client_with_put(fake)
    table = pa.table({"id": ["r1"]})

    with pytest.warns(RuntimeWarning, match="falls back to WAL-backed writes"):
        result = client.bulk_upsert("records", table, write_mode=WriteMode.DIRECT)

    descriptor = json.loads(fake.descriptor.command)
    assert descriptor["write_mode"] == "direct"
    assert result.success
    assert result.records_processed == 1


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_delete_doput_sends_delete_descriptor_and_id_table():
    fake = _PutClient(
        {
            "success": True,
            "metrics": {"successful_count": 2, "failed_count": 0},
        }
    )
    client = _client_with_put(fake)

    result = client.bulk_delete("records", ["r1", "r2"], batch_size=10)

    descriptor = json.loads(fake.descriptor.command)
    assert descriptor["collection_id"] == "records"
    assert descriptor["operation"] == "delete"
    assert fake.schema.names == ["id"]
    assert fake.writer.batches[0].column("id").to_pylist() == ["r1", "r2"]
    assert fake.writer.closed
    assert result.success
    assert result.records_processed == 2


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_upsert_exchange_streams_batches_and_parses_metadata():
    progress = {"type": "progress", "operation": "upsert", "batch": 1}
    complete = {
        "type": "complete",
        "operation": "upsert",
        "total_batches": 1,
        "total_records": 2,
        "total_failed": 0,
        "success": True,
    }
    fake = _ExchangeClient(
        [
            _Chunk(json.dumps(progress).encode()),
            _Chunk(json.dumps(complete).encode()),
        ]
    )
    client = _client_with_exchange(fake)
    table = pa.table({"id": ["r1", "r2"], "category": ["a", "b"]})

    result = client.bulk_upsert_exchange("records", table, batch_size=1)

    assert fake.descriptor.path == [b"bulk_upsert", b"records"]
    assert fake.writer.schema == table.schema
    assert len(fake.writer.batches) == 2
    assert fake.writer.closed
    assert result.success
    assert result.records_processed == 2
    assert result.records_failed == 0
    assert result.batches_processed == 1
    assert result.progress == [progress]
    assert result.metadata == complete


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_write_exchange_handles_many_progress_batches():
    progress = [
        {
            "type": "progress",
            "operation": "upsert",
            "batch": batch,
            "batch_rows": 4,
            "total_records": batch * 4,
        }
        for batch in range(1, 8)
    ]
    complete = {
        "type": "complete",
        "operation": "upsert",
        "total_batches": 7,
        "total_records": 25,
        "total_failed": 0,
        "success": True,
    }
    fake = _ExchangeClient(
        [_Chunk(json.dumps(item).encode()) for item in [*progress, complete]]
    )
    client = _client_with_exchange(fake)
    table = pa.table(
        {
            "id": [f"r{i}" for i in range(25)],
            "category": [f"c{i % 3}" for i in range(25)],
        }
    )

    result = client.bulk_write_exchange("records", table, operation="upsert", batch_size=4)

    assert fake.descriptor.path == [b"bulk_upsert", b"records"]
    assert len(fake.writer.batches) == 7
    assert sum(batch.num_rows for batch in fake.writer.batches) == 25
    assert fake.writer.closed
    assert result.success
    assert result.records_processed == 25
    assert result.records_failed == 0
    assert result.batches_processed == 7
    assert result.progress == progress
    assert result.metadata == complete


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_delete_exchange_sends_id_table():
    complete = {
        "type": "complete",
        "operation": "delete",
        "total_batches": 1,
        "total_records": 2,
        "total_failed": 0,
        "success": True,
    }
    fake = _ExchangeClient([_Chunk(json.dumps(complete).encode())])
    client = _client_with_exchange(fake)

    result = client.bulk_delete_exchange("records", ["r1", "r2"])

    assert fake.descriptor.path == [b"bulk_delete", b"records"]
    assert fake.writer.schema.names == ["id"]
    assert fake.writer.batches[0].column("id").to_pylist() == ["r1", "r2"]
    assert result.success
    assert result.records_processed == 2


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_write_exchange_accepts_plain_upsert_alias():
    complete = {
        "type": "complete",
        "operation": "upsert",
        "total_batches": 1,
        "total_records": 1,
        "total_failed": 0,
        "success": True,
    }
    fake = _ExchangeClient([_Chunk(json.dumps(complete).encode())])
    client = _client_with_exchange(fake)
    table = pa.table({"id": ["r1"]})

    result = client.bulk_write_exchange("records", table, operation="upsert")

    assert fake.descriptor.path == [b"bulk_upsert", b"records"]
    assert result.success
    assert result.records_processed == 1


@pytest.mark.skipif(not ARROW_AVAILABLE, reason="PyArrow is required")
def test_bulk_write_exchange_accepts_plain_delete_alias():
    complete = {
        "type": "complete",
        "operation": "delete",
        "total_batches": 1,
        "total_records": 1,
        "total_failed": 0,
        "success": True,
    }
    fake = _ExchangeClient([_Chunk(json.dumps(complete).encode())])
    client = _client_with_exchange(fake)
    table = pa.table({"id": ["r1"]})

    result = client.bulk_write_exchange("records", table, operation="delete")

    assert fake.descriptor.path == [b"bulk_delete", b"records"]
    assert result.success
    assert result.records_processed == 1
