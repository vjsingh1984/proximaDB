"""Offline unit tests for proximadb_sdk.protocols.arrow_flight.

All Flight transport is mocked: ArrowFlightClient._get_client is patched to
return a fake Flight client, so no real channel/socket is ever opened.
"""

import json
import warnings

import pyarrow as pa
import pytest

from proximadb_sdk.protocols import arrow_flight as af
from proximadb_sdk.protocols.arrow_flight import (
    ArrowFlightClient,
    FlightExchangeResult,
    FlightPutResult,
    FlightSearchResult,
    WriteMode,
    arrow_table_to_vectors,
    vectors_to_arrow_table,
)


# --------------------------------------------------------------------------
# Fakes
# --------------------------------------------------------------------------
class FakeBuf:
    """Mimics a pyarrow Buffer with to_pybytes()."""

    def __init__(self, payload):
        self._payload = payload

    def to_pybytes(self):
        if isinstance(self._payload, (bytes, bytearray)):
            return bytes(self._payload)
        return json.dumps(self._payload).encode()


class FakeReader:
    """Reader returned by do_put: has .read() returning a single buffer."""

    def __init__(self, payload):
        self._payload = payload

    def read(self):
        if self._payload is None:
            return None
        return FakeBuf(self._payload)


class FakeWriter:
    def __init__(self, record):
        self.record = record

    def begin(self, schema):
        self.record["began"] = True

    def write_batch(self, batch):
        self.record.setdefault("batches", 0)
        self.record["batches"] += 1

    def close(self):
        self.record["closed"] = True


class FakeExchangeChunk:
    def __init__(self, metadata):
        self.app_metadata = json.dumps(metadata).encode()


class FakeSearchChunk:
    def __init__(self, batch):
        self.data = batch


class FakeAction:
    def __init__(self, type_, description):
        self.type = type_
        self.description = description


class FakeSchemaResult:
    def __init__(self, schema):
        self.schema = schema


class FakeFlightClient:
    """A fully in-memory stand-in for flight.FlightClient."""

    def __init__(self):
        self.calls = []
        self.do_put_payload = {"success": True, "message": "ok"}
        self.do_put_writer_record = {}
        self.exchange_chunks = []
        self.search_chunks = []
        self.actions = [FakeAction("flush_collection", "flush it")]
        self.do_action_result = ["done"]
        self.do_action_error = None
        self.schema_result = None
        self.schema_error = None
        self.closed = False

    def do_put(self, descriptor, schema, options=None):
        self.calls.append(("do_put", descriptor, schema, options))
        writer = FakeWriter(self.do_put_writer_record)
        reader = FakeReader(self.do_put_payload)
        return writer, reader

    def do_exchange(self, descriptor, options=None):
        self.calls.append(("do_exchange", descriptor, options))
        writer = FakeWriter(self.do_put_writer_record)
        reader = iter(self.exchange_chunks)
        return writer, reader

    def do_get(self, ticket, options=None):
        self.calls.append(("do_get", ticket, options))
        return iter(self.search_chunks)

    def do_action(self, action, options=None):
        self.calls.append(("do_action", action, options))
        if self.do_action_error:
            raise self.do_action_error
        return iter(self.do_action_result)

    def list_actions(self, options=None):
        self.calls.append(("list_actions", options))
        return iter(self.actions)

    def get_schema(self, descriptor, options=None):
        self.calls.append(("get_schema", descriptor, options))
        if self.schema_error:
            raise self.schema_error
        return self.schema_result

    def close(self):
        self.closed = True


def make_client(monkeypatch, fake=None, **kwargs):
    """Build an ArrowFlightClient whose _get_client returns a fake."""
    c = ArrowFlightClient("grpc://localhost:5678", **kwargs)
    fake = fake or FakeFlightClient()
    monkeypatch.setattr(c, "_get_client", lambda: fake)
    return c, fake


def simple_table():
    return vectors_to_arrow_table(
        ids=["v1", "v2"],
        vectors=[[0.1, 0.2], [0.3, 0.4]],
    )


# --------------------------------------------------------------------------
# Dataclasses
# --------------------------------------------------------------------------
def test_flight_put_result_aliases():
    r = FlightPutResult(
        success=True,
        vectors_inserted=5,
        message="m",
        metadata={"metrics": {"failed_count": 2}},
    )
    assert r.records_processed == 5
    assert r.records_failed == 2


def test_flight_put_result_failed_from_errors():
    r = FlightPutResult(
        success=True,
        vectors_inserted=1,
        message="m",
        metadata={"errors": ["a", "b", "c"]},
    )
    assert r.records_failed == 3


def test_flight_put_result_failed_empty():
    r = FlightPutResult(success=True, vectors_inserted=1, message="m", metadata={})
    assert r.records_failed == 0


def test_write_mode_constants():
    assert WriteMode.WAL == "wal"
    assert WriteMode.DIRECT == "direct"


# --------------------------------------------------------------------------
# URL / location parsing
# --------------------------------------------------------------------------
def test_parse_location_grpc():
    c = ArrowFlightClient("grpc://host:9999")
    assert c._location is not None


def test_parse_location_grpc_tls():
    c = ArrowFlightClient("grpc+tls://host:9000")
    assert c._location is not None


def test_parse_location_http():
    c = ArrowFlightClient("http://host:8080")
    assert c._location is not None


def test_parse_location_https():
    c = ArrowFlightClient("https://host:8443")
    assert c._location is not None


def test_parse_location_no_port():
    c = ArrowFlightClient("localhost")
    assert c._location is not None


def test_parse_location_bare_hostport():
    c = ArrowFlightClient("localhost:5680")
    assert c._location is not None


# --------------------------------------------------------------------------
# Call options + helpers
# --------------------------------------------------------------------------
def test_get_call_options_no_auth():
    c = ArrowFlightClient("grpc://localhost:5678")
    opts = c._get_call_options()
    assert opts is not None


def test_get_call_options_with_auth():
    c = ArrowFlightClient(
        "grpc://localhost:5678", api_key="secret", tenant_id="tenantA"
    )
    opts = c._get_call_options()
    assert opts is not None


def test_warn_direct_write_fallback():
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        ArrowFlightClient._warn_direct_write_fallback(WriteMode.DIRECT)
    assert any(issubclass(x.category, RuntimeWarning) for x in w)


def test_warn_direct_write_fallback_wal_silent():
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        ArrowFlightClient._warn_direct_write_fallback(WriteMode.WAL)
    assert len(w) == 0


def test_affected_count_successful():
    assert (
        ArrowFlightClient._affected_count({"metrics": {"successful_count": 7}}, 0) == 7
    )


def test_affected_count_total_processed():
    assert (
        ArrowFlightClient._affected_count({"metrics": {"total_processed": 9}}, 0) == 9
    )


def test_affected_count_fallback():
    assert ArrowFlightClient._affected_count({}, 42) == 42


def test_decode_metadata_none():
    assert ArrowFlightClient._decode_metadata(None) == {}


def test_decode_metadata_empty_bytes():
    assert ArrowFlightClient._decode_metadata(b"") == {}


def test_decode_metadata_str():
    assert ArrowFlightClient._decode_metadata('{"a": 1}') == {"a": 1}


def test_decode_metadata_bytes():
    assert ArrowFlightClient._decode_metadata(b'{"b": 2}') == {"b": 2}


def test_decode_metadata_memoryview():
    mv = memoryview(b'{"c": 3}')
    assert ArrowFlightClient._decode_metadata(mv) == {"c": 3}


def test_decode_metadata_to_pybytes():
    assert ArrowFlightClient._decode_metadata(FakeBuf(b'{"d": 4}')) == {"d": 4}


def test_metadata_from_exchange_chunk_app_metadata():
    chunk = FakeExchangeChunk({"type": "progress", "n": 1})
    assert ArrowFlightClient._metadata_from_exchange_chunk(chunk) == {
        "type": "progress",
        "n": 1,
    }


def test_metadata_from_exchange_chunk_nested_data():
    class Inner:
        app_metadata = json.dumps({"x": 5}).encode()

    class Outer:
        data = Inner()

    assert ArrowFlightClient._metadata_from_exchange_chunk(Outer()) == {"x": 5}


def test_metadata_from_exchange_chunk_empty():
    assert ArrowFlightClient._metadata_from_exchange_chunk(object()) == {}


# --------------------------------------------------------------------------
# Schema construction
# --------------------------------------------------------------------------
def test_create_vector_schema():
    schema = ArrowFlightClient.create_vector_schema(4)
    assert "id" in schema.names
    assert "vector" in schema.names
    assert "metadata" in schema.names


# --------------------------------------------------------------------------
# bulk_insert / bulk_upsert / bulk_delete (do_put)
# --------------------------------------------------------------------------
def test_bulk_insert_success(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = {
        "success": True,
        "message": "inserted",
        "metrics": {"successful_count": 2},
    }
    result = c.bulk_insert("col", simple_table())
    assert result.success is True
    assert result.vectors_inserted == 2
    assert fake.do_put_writer_record["closed"] is True
    desc = fake.calls[0][1]
    assert b"col" in desc.command


def test_bulk_insert_fallback_count(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = {}
    result = c.bulk_insert("col", simple_table())
    assert result.vectors_inserted == 2
    assert result.message == "Bulk insert completed"


def test_bulk_insert_empty_reader(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = None
    result = c.bulk_insert("col", simple_table())
    assert result.success is True
    assert result.vectors_inserted == 2


def test_bulk_insert_exception(monkeypatch):
    c, fake = make_client(monkeypatch)

    class BoomReader:
        def read(self):
            raise RuntimeError("network down")

    def do_put(descriptor, schema, options=None):
        return FakeWriter(fake.do_put_writer_record), BoomReader()

    fake.do_put = do_put
    result = c.bulk_insert("col", simple_table())
    assert result.success is False
    assert result.vectors_inserted == 0
    assert "network down" in result.message


def test_bulk_insert_direct_warns(monkeypatch):
    c, fake = make_client(monkeypatch)
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        c.bulk_insert("col", simple_table(), write_mode=WriteMode.DIRECT)
    assert any(issubclass(x.category, RuntimeWarning) for x in w)


def test_bulk_upsert(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = {"success": True, "metrics": {"total_processed": 2}}
    result = c.bulk_upsert("col", simple_table())
    assert result.vectors_inserted == 2
    desc = fake.calls[0][1]
    assert b"upsert" in desc.command


def test_bulk_delete(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = {"success": True}
    result = c.bulk_delete("col", ["a", "b", "c"])
    assert result.success is True
    desc = fake.calls[0][1]
    assert b"delete" in desc.command


def test_bulk_insert_from_batches(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_put_payload = {"success": True, "metrics": {"successful_count": 2}}
    table = simple_table()
    batches = list(table.to_batches())
    result = c.bulk_insert_from_batches("col", iter(batches), table.schema)
    assert result.success is True
    assert result.vectors_inserted == 2


def test_bulk_insert_from_batches_exception(monkeypatch):
    c, fake = make_client(monkeypatch)

    class BoomReader:
        def read(self):
            raise RuntimeError("put failed")

    def do_put(descriptor, schema, options=None):
        return FakeWriter(fake.do_put_writer_record), BoomReader()

    fake.do_put = do_put
    table = simple_table()
    result = c.bulk_insert_from_batches("col", iter(table.to_batches()), table.schema)
    assert result.success is False
    assert "put failed" in result.message


# --------------------------------------------------------------------------
# bulk_write_exchange (do_exchange)
# --------------------------------------------------------------------------
def test_bulk_write_exchange_success(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.exchange_chunks = [
        FakeExchangeChunk({"type": "progress", "batch": 1}),
        FakeExchangeChunk({}),
        FakeExchangeChunk(
            {
                "type": "complete",
                "total_records": 2,
                "total_failed": 0,
                "total_batches": 1,
                "success": True,
            }
        ),
    ]
    result = c.bulk_write_exchange("col", simple_table(), operation="upsert")
    assert isinstance(result, FlightExchangeResult)
    assert result.success is True
    assert result.records_processed == 2
    assert result.batches_processed == 1
    assert len(result.progress) == 1
    assert fake.do_put_writer_record.get("began") is True


def test_bulk_write_exchange_defaults_no_complete(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.exchange_chunks = [FakeExchangeChunk({"type": "progress"})]
    result = c.bulk_write_exchange("col", simple_table(), operation="insert")
    assert result.records_processed == 2
    assert result.batches_processed == 1


def test_bulk_write_exchange_invalid_operation(monkeypatch):
    c, fake = make_client(monkeypatch)
    with pytest.raises(ValueError):
        c.bulk_write_exchange("col", simple_table(), operation="frobnicate")


def test_bulk_write_exchange_exception(monkeypatch):
    c, fake = make_client(monkeypatch)

    def boom_iter():
        raise RuntimeError("exchange broke")
        yield  # pragma: no cover

    def do_exchange(descriptor, options=None):
        return FakeWriter(fake.do_put_writer_record), boom_iter()

    fake.do_exchange = do_exchange
    result = c.bulk_write_exchange("col", simple_table(), operation="upsert")
    assert result.success is False
    assert "exchange broke" in result.message


def test_bulk_upsert_exchange(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.exchange_chunks = [
        FakeExchangeChunk({"type": "complete", "total_records": 2, "success": True})
    ]
    result = c.bulk_upsert_exchange("col", simple_table())
    assert result.records_processed == 2


def test_bulk_delete_exchange(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.exchange_chunks = [
        FakeExchangeChunk({"type": "complete", "total_records": 3, "total_failed": 1})
    ]
    result = c.bulk_delete_exchange("col", ["a", "b", "c"])
    assert result.records_processed == 3
    assert result.records_failed == 1
    assert result.success is False


# --------------------------------------------------------------------------
# search / search_batch (do_get)
# --------------------------------------------------------------------------
def _search_batch():
    cols = {
        "id": pa.array(["r1", "r2"]),
        "vector": pa.array([[0.1, 0.2], [0.3, 0.4]]),
        "score": pa.array([0.9, 0.8], type=pa.float32()),
    }
    return pa.record_batch(cols)


def test_search(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.search_chunks = [FakeSearchChunk(_search_batch())]
    results = c.search("col", [0.1, 0.2], top_k=2)
    assert len(results) == 2
    assert all(isinstance(r, FlightSearchResult) for r in results)
    assert results[0].id == "r1"
    assert results[0].score == pytest.approx(0.9)
    assert results[0].vector == []
    ticket = fake.calls[0][1]
    assert b"col" in ticket.ticket


def test_search_include_vectors(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.search_chunks = [FakeSearchChunk(_search_batch())]
    results = c.search("col", [0.1, 0.2], include_vectors=True, top_k=2)
    assert results[0].vector == [pytest.approx(0.1), pytest.approx(0.2)]


def test_search_no_score_column(monkeypatch):
    c, fake = make_client(monkeypatch)
    batch = pa.record_batch({"id": pa.array(["x"]), "vector": pa.array([[0.5, 0.6]])})
    fake.search_chunks = [FakeSearchChunk(batch)]
    results = c.search("col", [0.5, 0.6])
    assert results[0].score == 0.0


def test_search_with_filter(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.search_chunks = [FakeSearchChunk(_search_batch())]
    results = c.search("col", [0.1, 0.2], filter_metadata={"k": "v"})
    assert len(results) == 2


def test_search_batch(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.search_chunks = [FakeSearchChunk(_search_batch())]
    results = c.search_batch("col", [[0.1, 0.2], [0.3, 0.4]])
    assert len(results) == 2
    assert all(len(r) == 2 for r in results)


# --------------------------------------------------------------------------
# DoAction wrappers
# --------------------------------------------------------------------------
def test_flush_collection(monkeypatch):
    c, fake = make_client(monkeypatch)
    assert c.flush_collection("col") is True
    action = fake.calls[0][1]
    assert action.type == "flush_collection"


def test_compact_collection(monkeypatch):
    c, fake = make_client(monkeypatch)
    assert c.compact_collection("col") is True


def test_flush_and_compact(monkeypatch):
    c, fake = make_client(monkeypatch)
    assert c.flush_and_compact("col") is True


def test_do_action_failure(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.do_action_error = RuntimeError("action failed")
    assert c.flush_collection("col") is False


# --------------------------------------------------------------------------
# list_actions / get_schema
# --------------------------------------------------------------------------
def test_list_actions(monkeypatch):
    c, fake = make_client(monkeypatch)
    actions = c.list_actions()
    assert actions == [("flush_collection", "flush it")]


def test_list_actions_failure(monkeypatch):
    c, fake = make_client(monkeypatch)

    def boom(*a, **k):
        raise RuntimeError("no actions")

    fake.list_actions = boom
    assert c.list_actions() == []


def test_get_schema(monkeypatch):
    c, fake = make_client(monkeypatch)
    schema = pa.schema([pa.field("id", pa.utf8())])
    fake.schema_result = FakeSchemaResult(schema)
    out = c.get_schema("col")
    assert out is schema


def test_get_schema_failure(monkeypatch):
    c, fake = make_client(monkeypatch)
    fake.schema_error = RuntimeError("not found")
    assert c.get_schema("col") is None


# --------------------------------------------------------------------------
# close + lazy client
# --------------------------------------------------------------------------
def test_close_noop_when_no_client():
    c = ArrowFlightClient("grpc://localhost:5678")
    c.close()
    assert c._client is None


def test_close_with_client():
    c = ArrowFlightClient("grpc://localhost:5678")
    fake = FakeFlightClient()
    c._client = fake
    c.close()
    assert fake.closed is True
    assert c._client is None


# --------------------------------------------------------------------------
# Module-level converters
# --------------------------------------------------------------------------
def test_vectors_to_arrow_table_basic():
    table = vectors_to_arrow_table(["a", "b"], [[1.0, 2.0], [3.0, 4.0]])
    assert table.num_rows == 2
    assert "vector" in table.schema.names


def test_vectors_to_arrow_table_with_metadata_and_timestamps():
    table = vectors_to_arrow_table(
        ["a", "b"],
        [[1.0, 2.0], [3.0, 4.0]],
        metadata=[{"k": "v"}, {}],
        timestamps=[100, 200],
    )
    assert table.num_rows == 2


def test_vectors_to_arrow_table_length_mismatch():
    with pytest.raises(ValueError):
        vectors_to_arrow_table(["a"], [[1.0], [2.0]])


def test_vectors_to_arrow_table_empty():
    with pytest.raises(ValueError):
        vectors_to_arrow_table([], [])


def test_arrow_table_to_vectors_roundtrip():
    table = vectors_to_arrow_table(
        ["a", "b"], [[1.0, 2.0], [3.0, 4.0]], metadata=[{"k": "v"}, {}]
    )
    ids, vectors, metadata = arrow_table_to_vectors(table)
    assert ids == ["a", "b"]
    assert vectors[0] == [pytest.approx(1.0), pytest.approx(2.0)]
    assert len(metadata) == 2


def test_arrow_table_to_vectors_no_metadata_column():
    table = pa.table({"id": pa.array(["x"]), "vector": pa.array([[1.0, 2.0]])})
    ids, vectors, metadata = arrow_table_to_vectors(table)
    assert ids == ["x"]
    assert metadata == [None]


def test_arrow_available_flag():
    assert af.ARROW_AVAILABLE is True
