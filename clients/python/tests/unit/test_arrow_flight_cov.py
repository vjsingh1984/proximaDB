"""Offline unit tests for proximadb_sdk.protocols.arrow_flight.

Every transport (pyarrow.flight.FlightClient) is mocked. No real network,
no real server, no socket blocking. Real pyarrow is used only to build
tables/schemas/tickets/descriptors in-memory.
"""

import sys
import types

# --------------------------------------------------------------------------
# Offline guard: the project's coverage config (pyproject.toml
# [tool.coverage.run] source_pkgs = ["proximadb_sdk"]) makes coverage import
# every SDK submodule at session end to report unexecuted files. Two of those
# modules load transformer models from the network at *import* time
# (embedding_providers.finbert_provider / multi_bert_provider), which would
# block forever and hang the whole run. We pre-seed sys.modules with harmless
# stubs so any later import is a no-op and stays fully offline. This touches
# only our own test process; it does not modify any source file.
# --------------------------------------------------------------------------
for _name in (
    "proximadb_sdk.embedding_providers.finbert_provider",
    "proximadb_sdk.embedding_providers.multi_bert_provider",
):
    if _name not in sys.modules:
        _stub = types.ModuleType(_name)
        _stub.__dict__["__doc__"] = "offline test stub"
        sys.modules[_name] = _stub

import json
import warnings

import pytest

import proximadb_sdk.protocols.arrow_flight as af
from proximadb_sdk.protocols.arrow_flight import (
    ArrowFlightClient,
    FlightExchangeResult,
    FlightPutResult,
    FlightSearchResult,
    WriteMode,
    arrow_table_to_vectors,
    vectors_to_arrow_table,
)

pa = pytest.importorskip("pyarrow")
flight = pytest.importorskip("pyarrow.flight")


# --------------------------------------------------------------------------
# Fakes for the Flight transport
# --------------------------------------------------------------------------


class FakeBuf:
    def __init__(self, data: bytes):
        self._data = data

    def to_pybytes(self) -> bytes:
        return self._data

    def __bool__(self) -> bool:
        return bool(self._data)


class FakeWriter:
    def __init__(self):
        self.batches = []
        self.closed = False
        self.began_with = None

    def write_batch(self, batch):
        self.batches.append(batch)

    def close(self):
        self.closed = True

    def begin(self, schema):
        self.began_with = schema


class FakeReader:
    """Reader for do_put: a single .read() returning a result buffer."""

    def __init__(self, result_data):
        if result_data is None:
            self._buf = None
        else:
            self._buf = FakeBuf(json.dumps(result_data).encode())

    def read(self):
        return self._buf


class FakeChunk:
    def __init__(self, app_metadata):
        self.app_metadata = app_metadata


class FakeExchangeReader:
    """Iterable reader for do_exchange chunks."""

    def __init__(self, chunks):
        self._chunks = chunks

    def __iter__(self):
        return iter(self._chunks)


class FakeDataChunk:
    """do_get chunk: .data is a record batch."""

    def __init__(self, batch):
        self.data = batch


class FakeGetReader:
    def __init__(self, chunks):
        self._chunks = chunks

    def __iter__(self):
        return iter(self._chunks)


class FakeAction:
    def __init__(self, type_, description):
        self.type = type_
        self.description = description


class FakeFlightClient:
    """Stand-in for flight.FlightClient. Never opens a channel."""

    instances = []

    def __init__(self, location, options=None):
        self.location = location
        self.options = options
        self.closed = False
        # programmable behaviors
        self.do_put_result = {"success": True, "message": "ok"}
        self.do_put_writer = None
        self.do_put_reader = None
        self.exchange_chunks = []
        self.exchange_writer = None
        self.get_chunks = []
        self.do_action_raises = False
        self.actions = [FakeAction("flush_collection", "flush")]
        self.list_actions_raises = False
        self.schema_result = None
        self.get_schema_raises = False
        self.calls = []
        FakeFlightClient.instances.append(self)

    def do_put(self, descriptor, schema, options=None):
        self.calls.append(("do_put", descriptor, schema, options))
        self.do_put_writer = FakeWriter()
        self.do_put_reader = FakeReader(self.do_put_result)
        return self.do_put_writer, self.do_put_reader

    def do_exchange(self, descriptor, options=None):
        self.calls.append(("do_exchange", descriptor, options))
        self.exchange_writer = FakeWriter()
        return self.exchange_writer, FakeExchangeReader(self.exchange_chunks)

    def do_get(self, ticket, options=None):
        self.calls.append(("do_get", ticket, options))
        return FakeGetReader(self.get_chunks)

    def do_action(self, action, options=None):
        self.calls.append(("do_action", action, options))
        if self.do_action_raises:
            raise RuntimeError("action boom")
        return iter([b"result"])

    def list_actions(self, options=None):
        if self.list_actions_raises:
            raise RuntimeError("list boom")
        return iter(self.actions)

    def get_schema(self, descriptor, options=None):
        if self.get_schema_raises:
            raise RuntimeError("schema boom")
        return self.schema_result

    def close(self):
        self.closed = True


class FakeClientOptions:
    def __init__(self, *a, **k):
        self.args = a
        self.kwargs = k


@pytest.fixture(autouse=True)
def _patch_flight(monkeypatch):
    """Patch the Flight transport so no channel is ever opened."""
    FakeFlightClient.instances = []
    monkeypatch.setattr(flight, "FlightClient", FakeFlightClient)
    # FlightClientOptions may not exist on this pyarrow version; the source
    # constructs it unconditionally, so provide a stub regardless.
    monkeypatch.setattr(flight, "FlightClientOptions", FakeClientOptions, raising=False)
    # FlightCallOptions / descriptors / tickets / actions are the real
    # pyarrow types except we don't need a real channel.
    yield


def make_client(**kw):
    return ArrowFlightClient("grpc://localhost:5678", **kw)


def sample_table():
    return vectors_to_arrow_table(
        ids=["v1", "v2"],
        vectors=[[0.1, 0.2], [0.3, 0.4]],
        metadata=[{"k": "a"}, {"k": "b"}],
        timestamps=[1, 2],
    )


# --------------------------------------------------------------------------
# Construction & URL parsing
# --------------------------------------------------------------------------


def test_init_and_lazy_client():
    c = make_client(api_key="key", tenant_id="t1")
    assert c._client is None
    client = c._get_client()
    assert isinstance(client, FakeFlightClient)
    # second call returns cached
    assert c._get_client() is client


def test_parse_location_variants():
    c = make_client()
    # grpc:// with port
    loc = c._parse_location("grpc://host:1234")
    assert "host" in str(loc).lower() or loc is not None
    # http://
    c._parse_location("http://h:99")
    # no port -> default
    c._parse_location("justhost")
    # grpc+tls
    c._parse_location("grpc+tls://h:443")
    # https
    c._parse_location("https://h:443")


def test_arrow_unavailable_raises(monkeypatch):
    monkeypatch.setattr(af, "ARROW_AVAILABLE", False)
    with pytest.raises(ImportError):
        ArrowFlightClient("grpc://localhost:5678")


def test_call_options_with_auth():
    c = make_client(api_key="secret", tenant_id="tenant-9")
    opts = c._get_call_options()
    assert opts is not None


def test_call_options_no_auth():
    c = make_client()
    assert c._get_call_options() is not None


# --------------------------------------------------------------------------
# Static helpers
# --------------------------------------------------------------------------


def test_decode_metadata_variants():
    d = ArrowFlightClient
    assert d._decode_metadata(None) == {}
    assert d._decode_metadata(b"") == {}
    assert d._decode_metadata('{"a": 1}') == {"a": 1}
    assert d._decode_metadata(b'{"a": 2}') == {"a": 2}
    assert d._decode_metadata(memoryview(b'{"a": 3}')) == {"a": 3}
    assert d._decode_metadata(FakeBuf(b'{"a": 4}')) == {"a": 4}


def test_metadata_from_exchange_chunk():
    cls = ArrowFlightClient
    chunk = FakeChunk(b'{"type": "progress"}')
    assert cls._metadata_from_exchange_chunk(chunk) == {"type": "progress"}

    class NestedData:
        app_metadata = b'{"type": "complete"}'

    class NestedChunk:
        data = NestedData()

    assert cls._metadata_from_exchange_chunk(NestedChunk()) == {"type": "complete"}

    class Empty:
        pass

    assert cls._metadata_from_exchange_chunk(Empty()) == {}


def test_affected_count():
    cls = ArrowFlightClient
    assert cls._affected_count({"metrics": {"successful_count": 7}}, 3) == 7
    assert cls._affected_count({"metrics": {"total_processed": 5}}, 3) == 5
    assert cls._affected_count({}, 11) == 11


def test_warn_direct_write_fallback():
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        ArrowFlightClient._warn_direct_write_fallback(WriteMode.DIRECT)
        assert any(issubclass(x.category, RuntimeWarning) for x in w)
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        ArrowFlightClient._warn_direct_write_fallback(WriteMode.WAL)
        assert len(w) == 0


def test_create_vector_schema():
    schema = ArrowFlightClient.create_vector_schema(4)
    assert "id" in schema.names
    assert "vector" in schema.names
    assert "score" in schema.names


# --------------------------------------------------------------------------
# Dataclass helpers
# --------------------------------------------------------------------------


def test_flight_put_result_aliases():
    r = FlightPutResult(True, 5, "ok", {"metrics": {"failed_count": 2}})
    assert r.records_processed == 5
    assert r.records_failed == 2
    r2 = FlightPutResult(True, 5, "ok", {"errors": ["e1", "e2", "e3"]})
    assert r2.records_failed == 3
    r3 = FlightPutResult(True, 5, "ok", {})
    assert r3.records_failed == 0


# --------------------------------------------------------------------------
# bulk_insert / upsert / delete (DoPut)
# --------------------------------------------------------------------------


def test_bulk_insert_success():
    c = make_client()
    fc = c._get_client()
    fc.do_put_result = {
        "success": True,
        "message": "done",
        "metrics": {"successful_count": 2},
    }
    res = c.bulk_insert("col", sample_table())
    assert res.success is True
    assert res.vectors_inserted == 2
    assert res.message == "done"
    assert fc.do_put_writer.closed is True
    # descriptor command carries collection_id
    descriptor = fc.calls[0][1]
    assert b"col" in descriptor.command


def test_bulk_insert_empty_result_falls_back_to_total():
    c = make_client()
    fc = c._get_client()
    fc.do_put_result = None  # reader.read() returns falsy
    res = c.bulk_insert("col", sample_table())
    assert res.success is True
    assert res.vectors_inserted == 2  # total_rows fallback
    assert "completed" in res.message


def test_bulk_insert_exception_path():
    c = make_client()
    fc = c._get_client()

    # The do_put call is outside the try; force the failure during streaming
    # (write_batch) so the except branch is exercised.
    class BadWriter(FakeWriter):
        def write_batch(self, batch):
            raise RuntimeError("put failed")

    orig_do_put = fc.do_put

    def do_put(descriptor, schema, options=None):
        w, r = orig_do_put(descriptor, schema, options=options)
        return BadWriter(), r

    fc.do_put = do_put
    res = c.bulk_insert("col", sample_table())
    assert res.success is False
    assert res.vectors_inserted == 0
    assert "put failed" in res.message


def test_bulk_insert_direct_mode_warns():
    c = make_client()
    c._get_client()
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        c.bulk_insert("col", sample_table(), write_mode=WriteMode.DIRECT)
        assert any(issubclass(x.category, RuntimeWarning) for x in w)


def test_bulk_upsert():
    c = make_client()
    c._get_client()
    res = c.bulk_upsert("col", sample_table())
    assert isinstance(res, FlightPutResult)


def test_bulk_delete():
    c = make_client()
    c._get_client()
    res = c.bulk_delete("col", ["v1", "v2", "v3"])
    assert isinstance(res, FlightPutResult)


def test_bulk_delete_arrow_unavailable(monkeypatch):
    c = make_client()
    monkeypatch.setattr(af, "ARROW_AVAILABLE", False)
    with pytest.raises(ImportError):
        c.bulk_delete("col", ["v1"])


def test_bulk_insert_from_batches():
    c = make_client()
    fc = c._get_client()
    fc.do_put_result = {"success": True, "metrics": {"total_processed": 2}}
    table = sample_table()
    batches = list(table.to_batches())
    res = c.bulk_insert_from_batches("col", iter(batches), table.schema)
    assert res.success is True
    assert res.vectors_inserted == 2


def test_bulk_insert_from_batches_exception():
    c = make_client()
    fc = c._get_client()

    class BadWriter(FakeWriter):
        def write_batch(self, batch):
            raise RuntimeError("stream failed")

    orig_do_put = fc.do_put

    def do_put(descriptor, schema, options=None):
        _, r = orig_do_put(descriptor, schema, options=options)
        return BadWriter(), r

    fc.do_put = do_put
    table = sample_table()
    res = c.bulk_insert_from_batches("col", iter(table.to_batches()), table.schema)
    assert res.success is False
    assert "stream failed" in res.message


# --------------------------------------------------------------------------
# bulk_write_exchange (DoExchange)
# --------------------------------------------------------------------------


def test_bulk_write_exchange_success():
    c = make_client()
    fc = c._get_client()
    fc.exchange_chunks = [
        FakeChunk(b'{"type": "progress", "batch": 1}'),
        FakeChunk(b""),  # empty metadata -> skipped
        FakeChunk(
            json.dumps(
                {
                    "type": "complete",
                    "total_records": 2,
                    "total_failed": 0,
                    "total_batches": 1,
                    "success": True,
                }
            ).encode()
        ),
    ]
    res = c.bulk_write_exchange("col", sample_table(), operation="insert")
    assert isinstance(res, FlightExchangeResult)
    assert res.success is True
    assert res.records_processed == 2
    assert res.batches_processed == 1
    assert len(res.progress) == 1
    # descriptor path carries operation + collection
    descriptor = fc.calls[0][1]
    assert descriptor.path == [b"bulk_insert", b"col"]
    assert fc.exchange_writer.began_with is not None


def test_bulk_write_exchange_invalid_operation():
    c = make_client()
    with pytest.raises(ValueError):
        c.bulk_write_exchange("col", sample_table(), operation="bogus")


def test_bulk_write_exchange_operation_aliases():
    c = make_client()
    fc = c._get_client()
    fc.exchange_chunks = []
    res = c.bulk_write_exchange("col", sample_table(), operation="upsert")
    # no complete metadata -> fallbacks
    assert res.records_processed == 2  # total_rows fallback
    assert res.batches_processed == 0


def test_bulk_write_exchange_exception():
    c = make_client()
    fc = c._get_client()

    class BadWriter(FakeWriter):
        def write_batch(self, batch):
            raise RuntimeError("exchange failed")

    def do_exchange(descriptor, options=None):
        return BadWriter(), FakeExchangeReader([])

    fc.do_exchange = do_exchange
    res = c.bulk_write_exchange("col", sample_table(), operation="insert")
    assert res.success is False
    assert "exchange failed" in res.message


def test_bulk_upsert_exchange():
    c = make_client()
    fc = c._get_client()
    fc.exchange_chunks = []
    res = c.bulk_upsert_exchange("col", sample_table())
    assert isinstance(res, FlightExchangeResult)


def test_bulk_delete_exchange():
    c = make_client()
    fc = c._get_client()
    fc.exchange_chunks = []
    res = c.bulk_delete_exchange("col", ["v1", "v2"])
    assert isinstance(res, FlightExchangeResult)


def test_bulk_delete_exchange_arrow_unavailable(monkeypatch):
    c = make_client()
    monkeypatch.setattr(af, "ARROW_AVAILABLE", False)
    with pytest.raises(ImportError):
        c.bulk_delete_exchange("col", ["v1"])


# --------------------------------------------------------------------------
# search / search_batch (DoGet)
# --------------------------------------------------------------------------


def _search_batch_table(include_score=True):
    cols = {
        "id": pa.array(["a", "b"], type=pa.utf8()),
        "vector": pa.array([[0.1, 0.2], [0.3, 0.4]]),
    }
    if include_score:
        cols["score"] = pa.array([0.9, 0.8], type=pa.float32())
    return pa.record_batch(cols)


def test_search_with_scores_and_vectors():
    c = make_client()
    fc = c._get_client()
    fc.get_chunks = [FakeDataChunk(_search_batch_table(include_score=True))]
    results = c.search("col", [0.1, 0.2], top_k=5, include_vectors=True)
    assert len(results) == 2
    assert all(isinstance(r, FlightSearchResult) for r in results)
    assert results[0].id == "a"
    assert results[0].score == pytest.approx(0.9)
    assert results[0].vector == pytest.approx([0.1, 0.2])
    # ticket carries the query json
    ticket = fc.calls[0][1]
    payload = json.loads(ticket.ticket)
    assert payload["collection_id"] == "col"
    assert payload["top_k"] == 5


def test_search_without_score_column_and_no_vectors():
    c = make_client()
    fc = c._get_client()
    fc.get_chunks = [FakeDataChunk(_search_batch_table(include_score=False))]
    results = c.search("col", [0.1, 0.2], include_vectors=False, filter_metadata={"k": "v"})
    assert results[0].score == 0.0
    assert results[0].vector == []


def test_search_batch():
    c = make_client()
    fc = c._get_client()
    fc.get_chunks = [FakeDataChunk(_search_batch_table())]
    results = c.search_batch("col", [[0.1, 0.2], [0.3, 0.4]], top_k=3)
    assert len(results) == 2
    assert all(len(r) == 2 for r in results)


# --------------------------------------------------------------------------
# DoAction wrappers
# --------------------------------------------------------------------------


def test_flush_compact_actions_success():
    c = make_client()
    fc = c._get_client()
    assert c.flush_collection("col") is True
    assert c.compact_collection("col") is True
    assert c.flush_and_compact("col") is True
    types = [call[1].type for call in fc.calls if call[0] == "do_action"]
    assert "flush_collection" in types
    assert "compact_collection" in types
    assert "flush_and_compact" in types


def test_do_action_failure(capsys):
    c = make_client()
    fc = c._get_client()
    fc.do_action_raises = True
    assert c.flush_collection("col") is False


def test_list_actions_success():
    c = make_client()
    fc = c._get_client()
    fc.actions = [FakeAction("a1", "d1"), FakeAction("a2", "d2")]
    out = c.list_actions()
    assert out == [("a1", "d1"), ("a2", "d2")]


def test_list_actions_failure():
    c = make_client()
    fc = c._get_client()
    fc.list_actions_raises = True
    assert c.list_actions() == []


# --------------------------------------------------------------------------
# get_schema
# --------------------------------------------------------------------------


def test_get_schema_success():
    c = make_client()
    fc = c._get_client()
    real_schema = ArrowFlightClient.create_vector_schema(3)

    class SchemaResult:
        schema = real_schema

    fc.schema_result = SchemaResult()
    out = c.get_schema("col")
    assert out is real_schema


def test_get_schema_failure():
    c = make_client()
    fc = c._get_client()
    fc.get_schema_raises = True
    assert c.get_schema("col") is None


# --------------------------------------------------------------------------
# close
# --------------------------------------------------------------------------


def test_close():
    c = make_client()
    fc = c._get_client()
    c.close()
    assert fc.closed is True
    assert c._client is None
    # closing again is a no-op
    c.close()


# --------------------------------------------------------------------------
# module-level conversion helpers
# --------------------------------------------------------------------------


def test_vectors_to_arrow_table_with_metadata_and_timestamps():
    table = vectors_to_arrow_table(
        ids=["v1", "v2"],
        vectors=[[0.1, 0.2], [0.3, 0.4]],
        metadata=[{"cat": "A"}, {}],
        timestamps=[100, 200],
    )
    assert table.num_rows == 2
    assert "vector" in table.schema.names


def test_vectors_to_arrow_table_no_metadata_no_timestamps():
    table = vectors_to_arrow_table(
        ids=["v1"],
        vectors=[[0.1, 0.2, 0.3]],
    )
    assert table.num_rows == 1


def test_vectors_to_arrow_table_length_mismatch():
    with pytest.raises(ValueError):
        vectors_to_arrow_table(ids=["v1", "v2"], vectors=[[0.1]])


def test_vectors_to_arrow_table_empty():
    with pytest.raises(ValueError):
        vectors_to_arrow_table(ids=[], vectors=[])


def test_vectors_to_arrow_table_arrow_unavailable(monkeypatch):
    monkeypatch.setattr(af, "ARROW_AVAILABLE", False)
    with pytest.raises(ImportError):
        vectors_to_arrow_table(ids=["v1"], vectors=[[0.1]])


def test_arrow_table_to_vectors_roundtrip():
    table = vectors_to_arrow_table(
        ids=["v1", "v2"],
        vectors=[[0.1, 0.2], [0.3, 0.4]],
        metadata=[{"k": "a"}, {"k": "b"}],
    )
    ids, vectors, metadata = arrow_table_to_vectors(table)
    assert ids == ["v1", "v2"]
    assert len(vectors) == 2
    assert len(metadata) == 2


def test_arrow_table_to_vectors_no_metadata_column():
    table = pa.table(
        {
            "id": pa.array(["v1"], type=pa.utf8()),
            "vector": pa.array([[0.1, 0.2]]),
        }
    )
    ids, vectors, metadata = arrow_table_to_vectors(table)
    assert ids == ["v1"]
    assert metadata == [None]


def test_arrow_table_to_vectors_arrow_unavailable(monkeypatch):
    table = pa.table({"id": pa.array(["v1"]), "vector": pa.array([[0.1]])})
    monkeypatch.setattr(af, "ARROW_AVAILABLE", False)
    with pytest.raises(ImportError):
        arrow_table_to_vectors(table)
