"""Offline unit tests for proximadb_sdk.arrow_export.

Uses real pyarrow (a hard dependency, no network) for table/schema construction,
but mocks the Arrow Flight client so no channel/socket is ever opened. polars is
not installed in this env, so the to_polars ImportError branch is exercised; we
also inject a stub polars module to exercise the success path.
"""

import sys
import types
from dataclasses import FrozenInstanceError  # noqa: F401  (kept for clarity)
from types import SimpleNamespace
from unittest.mock import MagicMock

import pyarrow as pa
import pytest

from proximadb_sdk import arrow_export
from proximadb_sdk.arrow_export import (
    ArrowExportClient,
    FileFormat,
    FileInfo,
    connect_arrow,
    read_proximadb_collection,
    read_proximadb_file,
)


# ---------------------------------------------------------------------------
# Helpers to build fake Flight objects (only the attributes the code touches)
# ---------------------------------------------------------------------------


def _schema_with_metadata(metadata=None, fields=None):
    fields = fields or [pa.field("id", pa.string())]
    schema = pa.schema(fields)
    if metadata:
        schema = schema.with_metadata(metadata)
    return schema


def _fake_flight_info(
    *,
    path_parts=("col", "data", "block_0.arrow"),
    metadata=None,
    fields=None,
    total_bytes=1024,
    total_records=10,
    schema=None,
):
    if schema is None and fields is None:
        fields = [pa.field("id", pa.string())]
    sch = schema if schema is not None else _schema_with_metadata(metadata, fields)
    descriptor = SimpleNamespace(path=list(path_parts))
    return SimpleNamespace(
        schema=sch,
        descriptor=descriptor,
        total_bytes=total_bytes,
        total_records=total_records,
        endpoints=[SimpleNamespace(ticket="ticket-123")],
    )


class _FakeReader:
    """Mimics a Flight do_get reader: read_all() + iteration over batch wrappers."""

    def __init__(self, table):
        self._table = table

    def read_all(self):
        return self._table

    def __iter__(self):
        for batch in self._table.to_batches():
            yield SimpleNamespace(data=batch)


def _make_client(monkeypatch, fake_flight_client):
    """Create an ArrowExportClient with its .client property stubbed."""
    c = ArrowExportClient(host="testhost", port=5680)
    # Replace the cached client and short-circuit the lazy property.
    c._client = fake_flight_client
    monkeypatch.setattr(
        type(c),
        "client",
        property(lambda self: self._client),
    )
    return c


# ---------------------------------------------------------------------------
# FileFormat / FileInfo
# ---------------------------------------------------------------------------


def test_fileformat_values():
    assert FileFormat.ARROW.value == "arrow"
    assert FileFormat.PARQUET.value == "parquet"
    assert FileFormat.SST.value == "sst"


def test_fileinfo_from_flight_info_arrow_with_vector_dimension():
    vec_field = pa.field("vector", pa.list_(pa.float32(), 768))
    info = _fake_flight_info(
        path_parts=("emb", "data", "block_0.arrow"),
        metadata={b"num_batches": b"3", b"modified_at": b"42"},
        fields=[pa.field("id", pa.string()), vec_field],
        total_bytes=2048,
        total_records=100,
    )
    fi = FileInfo.from_flight_info(info)
    assert fi.format == FileFormat.ARROW
    assert fi.path == "emb/data/block_0.arrow"
    assert fi.filename == "block_0.arrow"
    assert fi.size_bytes == 2048
    assert fi.total_records == 100
    assert fi.num_batches == 3
    assert fi.modified_at == 42
    assert fi.dimension == 768


def test_fileinfo_from_flight_info_parquet_format():
    info = _fake_flight_info(path_parts=("c", "x.parquet"))
    fi = FileInfo.from_flight_info(info)
    assert fi.format == FileFormat.PARQUET
    assert fi.filename == "x.parquet"


def test_fileinfo_from_flight_info_sst_format():
    info = _fake_flight_info(path_parts=("c", "x.sst"))
    fi = FileInfo.from_flight_info(info)
    assert fi.format == FileFormat.SST


def test_fileinfo_from_flight_info_negative_totals_clamped_to_zero():
    info = _fake_flight_info(total_bytes=-1, total_records=-5)
    fi = FileInfo.from_flight_info(info)
    assert fi.size_bytes == 0
    assert fi.total_records == 0


def test_fileinfo_from_flight_info_no_descriptor_path():
    info = _fake_flight_info()
    info.descriptor = SimpleNamespace(path=None)
    fi = FileInfo.from_flight_info(info)
    assert fi.path == ""
    assert fi.filename == ""


def test_fileinfo_from_flight_info_no_schema():
    info = _fake_flight_info()
    info.schema = None
    fi = FileInfo.from_flight_info(info)
    # No schema -> dimension stays 0, metadata defaults applied
    assert fi.dimension == 0
    assert fi.num_batches == 1
    assert fi.modified_at == 0


def test_fileinfo_from_flight_info_bytes_path_parts():
    info = _fake_flight_info(path_parts=(b"col", b"block.arrow"))
    fi = FileInfo.from_flight_info(info)
    assert fi.path == "col/block.arrow"


def test_fileinfo_from_flight_info_str_metadata_keys():
    # metadata with already-str keys/values still parsed
    info = _fake_flight_info(metadata={"num_batches": "7"})
    fi = FileInfo.from_flight_info(info)
    assert fi.num_batches == 7


# ---------------------------------------------------------------------------
# Construction / connection lifecycle
# ---------------------------------------------------------------------------


def test_init_builds_uri_default():
    c = ArrowExportClient(host="h", port=1234)
    assert c._uri == "grpc://h:1234"
    assert c._client is None


def test_init_tls_overrides_scheme():
    c = ArrowExportClient(host="h", port=1234, scheme="grpc", tls=True)
    assert c._uri == "grpc+tls://h:1234"
    assert c._tls is True


def test_init_requires_pyarrow(monkeypatch):
    monkeypatch.setattr(arrow_export, "_PYARROW_AVAILABLE", False)
    with pytest.raises(ImportError, match="PyArrow is required"):
        ArrowExportClient()


def test_client_property_lazily_connects(monkeypatch):
    fake_fc = MagicMock(name="FlightClient")
    fake_flight = SimpleNamespace(connect=MagicMock(return_value=fake_fc))
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    c = ArrowExportClient(host="h", port=99, auth_token="tok")
    # property triggers connect
    got = c.client
    assert got is fake_fc
    fake_flight.connect.assert_called_once_with("grpc://h:99")
    # second access reuses cached client (no second connect)
    assert c.client is fake_fc
    assert fake_flight.connect.call_count == 1


def test_close_and_context_manager(monkeypatch):
    fake_fc = MagicMock(name="FlightClient")
    fake_flight = SimpleNamespace(connect=MagicMock(return_value=fake_fc))
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    c = ArrowExportClient()
    with c as entered:
        assert entered is c
        _ = c.client  # force creation
    fake_fc.close.assert_called_once()
    assert c._client is None
    # close again when already None is a no-op
    c.close()


# ---------------------------------------------------------------------------
# list_files / get_file_info / get_schema
# ---------------------------------------------------------------------------


def _client_with_flights(monkeypatch, infos):
    fake_fc = MagicMock()
    fake_fc.list_flights.return_value = iter(infos)
    return _make_client(monkeypatch, fake_fc)


def test_list_files_all(monkeypatch):
    infos = [
        _fake_flight_info(path_parts=("c", "a.arrow")),
        _fake_flight_info(path_parts=("c", "b.parquet")),
    ]
    c = _client_with_flights(monkeypatch, infos)
    files = c.list_files("c")
    assert len(files) == 2
    c._client.list_flights.assert_called_once()


def test_list_files_format_filter(monkeypatch):
    infos = [
        _fake_flight_info(path_parts=("c", "a.arrow")),
        _fake_flight_info(path_parts=("c", "b.parquet")),
    ]
    c = _client_with_flights(monkeypatch, infos)
    files = c.list_files("c", format_filter=FileFormat.PARQUET)
    assert len(files) == 1
    assert files[0].format == FileFormat.PARQUET


def test_list_files_pattern_filter(monkeypatch):
    infos = [
        _fake_flight_info(path_parts=("c", "block_0.arrow")),
        _fake_flight_info(path_parts=("c", "other.arrow")),
    ]
    c = _client_with_flights(monkeypatch, infos)
    files = c.list_files("c", pattern="block_*.arrow")
    assert len(files) == 1
    assert files[0].filename == "block_0.arrow"


def test_get_file_info(monkeypatch):
    info = _fake_flight_info(path_parts=("c", "a.arrow"))
    fake_fc = MagicMock()
    fake_fc.get_flight_info.return_value = info
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc"))
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    c = _make_client(monkeypatch, fake_fc)
    fi = c.get_file_info("c/a.arrow")
    assert fi.filename == "a.arrow"
    fake_flight.FlightDescriptor.for_path.assert_called_once_with("c", "a.arrow")
    fake_fc.get_flight_info.assert_called_once_with("desc")


def test_get_schema(monkeypatch):
    schema = pa.schema([pa.field("id", pa.string())])
    info = _fake_flight_info(schema=schema)
    fake_fc = MagicMock()
    fake_fc.get_flight_info.return_value = info
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc"))
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    c = _make_client(monkeypatch, fake_fc)
    got = c.get_schema("c/a.arrow")
    assert got is schema


# ---------------------------------------------------------------------------
# read_file / read_batches / read_collection
# ---------------------------------------------------------------------------


def _table():
    return pa.table({"id": ["a", "b"], "n": [1, 2]})


def _read_client(monkeypatch, info, table):
    fake_fc = MagicMock()
    fake_fc.get_flight_info.return_value = info
    fake_fc.do_get.return_value = _FakeReader(table)
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc"))
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    return _make_client(monkeypatch, fake_fc)


def test_read_file(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    got = c.read_file("c/a.arrow")
    assert got.num_rows == 2
    c._client.do_get.assert_called_once_with("ticket-123")


def test_read_file_no_endpoints_raises(monkeypatch):
    info = _fake_flight_info()
    info.endpoints = []
    c = _read_client(monkeypatch, info, _table())
    with pytest.raises(ValueError, match="No endpoints"):
        c.read_file("c/a.arrow")


def test_read_batches(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    batches = list(c.read_batches("c/a.arrow"))
    assert len(batches) >= 1
    assert sum(b.num_rows for b in batches) == 2


def test_read_batches_no_endpoints_returns_empty(monkeypatch):
    info = _fake_flight_info()
    info.endpoints = []
    c = _read_client(monkeypatch, info, _table())
    assert list(c.read_batches("c/a.arrow")) == []


def test_read_collection(monkeypatch):
    table = _table()
    list_info_a = _fake_flight_info(path_parts=("c", "a.arrow"))
    list_info_b = _fake_flight_info(path_parts=("c", "b.arrow"))
    fake_fc = MagicMock()
    fake_fc.list_flights.return_value = iter([list_info_a, list_info_b])
    fake_fc.get_flight_info.return_value = _fake_flight_info()
    fake_fc.do_get.return_value = _FakeReader(table)
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc"))
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)
    c = _make_client(monkeypatch, fake_fc)
    # do_get returns a fresh reader each call; make it a side_effect
    fake_fc.do_get.side_effect = lambda t: _FakeReader(_table())
    combined = c.read_collection("c")
    assert combined.num_rows == 4


def test_read_collection_empty_returns_empty_table(monkeypatch):
    fake_fc = MagicMock()
    fake_fc.list_flights.return_value = iter([])
    c = _make_client(monkeypatch, fake_fc)
    table = c.read_collection("c")
    assert table.num_rows == 0


# ---------------------------------------------------------------------------
# Format conversions
# ---------------------------------------------------------------------------


def test_to_polars_import_error_when_unavailable(monkeypatch):
    monkeypatch.setattr(arrow_export, "_POLARS_AVAILABLE", False)
    c = ArrowExportClient()
    with pytest.raises(ImportError, match="Polars is required"):
        c.to_polars("c/a.arrow")


def test_to_polars_success_with_stub(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    sentinel_df = object()
    stub_pl = SimpleNamespace(from_arrow=MagicMock(return_value=sentinel_df))
    monkeypatch.setattr(arrow_export, "_POLARS_AVAILABLE", True)
    monkeypatch.setattr(arrow_export, "pl", stub_pl)
    df = c.to_polars("c/a.arrow", rechunk=False)
    assert df is sentinel_df
    stub_pl.from_arrow.assert_called_once()
    assert stub_pl.from_arrow.call_args.kwargs["rechunk"] is False


def test_to_duckdb_import_error_when_unavailable(monkeypatch):
    monkeypatch.setattr(arrow_export, "_DUCKDB_AVAILABLE", False)
    c = ArrowExportClient()
    with pytest.raises(ImportError, match="DuckDB is required"):
        c.to_duckdb("c/a.arrow")


def test_to_duckdb_creates_connection(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    fake_conn = MagicMock()
    stub_duckdb = SimpleNamespace(connect=MagicMock(return_value=fake_conn))
    monkeypatch.setattr(arrow_export, "_DUCKDB_AVAILABLE", True)
    monkeypatch.setattr(arrow_export, "duckdb", stub_duckdb)
    conn = c.to_duckdb("c/a.arrow")
    assert conn is fake_conn
    stub_duckdb.connect.assert_called_once_with(":memory:")
    fake_conn.register.assert_called_once()
    assert fake_conn.register.call_args.args[0] == "vectors"


def test_to_duckdb_uses_existing_connection(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    stub_duckdb = SimpleNamespace(connect=MagicMock())
    monkeypatch.setattr(arrow_export, "_DUCKDB_AVAILABLE", True)
    monkeypatch.setattr(arrow_export, "duckdb", stub_duckdb)
    existing = MagicMock()
    conn = c.to_duckdb("c/a.arrow", table_name="t2", conn=existing)
    assert conn is existing
    stub_duckdb.connect.assert_not_called()
    existing.register.assert_called_once()
    assert existing.register.call_args.args[0] == "t2"


def test_to_pandas(monkeypatch):
    table = _table()
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, table)
    df = c.to_pandas("c/a.arrow")
    assert list(df["id"]) == ["a", "b"]


def test_to_numpy(monkeypatch):
    vectors = pa.table(
        {"vector": [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]}
    )
    info = _fake_flight_info()
    c = _read_client(monkeypatch, info, vectors)
    arr = c.to_numpy("c/a.arrow")
    assert arr.shape == (2, 3)
    assert arr.dtype.name == "float32"
    assert arr[1][2] == pytest.approx(6.0)


# ---------------------------------------------------------------------------
# collection_stats
# ---------------------------------------------------------------------------


def test_collection_stats_empty(monkeypatch):
    fake_fc = MagicMock()
    fake_fc.list_flights.return_value = iter([])
    c = _make_client(monkeypatch, fake_fc)
    stats = c.collection_stats("c")
    assert stats["num_files"] == 0
    assert stats["total_records"] == 0
    assert stats["total_size_mb"] == 0.0
    assert stats["formats"] == {}


def test_collection_stats_aggregates(monkeypatch):
    infos = [
        _fake_flight_info(
            path_parts=("c", "a.arrow"),
            total_bytes=1024 * 1024,
            total_records=10,
            fields=[pa.field("vector", pa.list_(pa.float32(), 4))],
        ),
        _fake_flight_info(
            path_parts=("c", "b.arrow"),
            total_bytes=1024 * 1024,
            total_records=20,
            fields=[pa.field("vector", pa.list_(pa.float32(), 4))],
        ),
        _fake_flight_info(
            path_parts=("c", "c.parquet"),
            total_bytes=512,
            total_records=5,
        ),
    ]
    c = _client_with_flights(monkeypatch, infos)
    stats = c.collection_stats("c")
    assert stats["num_files"] == 3
    assert stats["total_records"] == 35
    assert stats["total_size_bytes"] == 2 * 1024 * 1024 + 512
    assert stats["total_size_mb"] == pytest.approx(2 + 512 / (1024 * 1024))
    assert stats["dimension"] == 4
    assert stats["formats"]["arrow"]["count"] == 2
    assert stats["formats"]["arrow"]["records"] == 30
    assert stats["formats"]["parquet"]["count"] == 1


# ---------------------------------------------------------------------------
# Convenience functions
# ---------------------------------------------------------------------------


def test_connect_arrow_returns_client():
    c = connect_arrow(host="h", port=5)
    assert isinstance(c, ArrowExportClient)
    assert c._uri == "grpc://h:5"


def test_read_proximadb_file(monkeypatch):
    table = _table()
    info = _fake_flight_info()

    fake_fc = MagicMock()
    fake_fc.get_flight_info.return_value = info
    fake_fc.do_get.return_value = _FakeReader(table)
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc")),
        connect=MagicMock(return_value=fake_fc),
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)

    got = read_proximadb_file("c/a.arrow", host="h", port=1)
    assert got.num_rows == 2
    fake_fc.close.assert_called_once()


def test_read_proximadb_collection(monkeypatch):
    info = _fake_flight_info(path_parts=("c", "a.arrow"))
    fake_fc = MagicMock()
    fake_fc.list_flights.return_value = iter([info])
    fake_fc.get_flight_info.return_value = _fake_flight_info()
    fake_fc.do_get.side_effect = lambda t: _FakeReader(_table())
    fake_flight = SimpleNamespace(
        FlightDescriptor=SimpleNamespace(for_path=MagicMock(return_value="desc")),
        connect=MagicMock(return_value=fake_fc),
    )
    monkeypatch.setattr(arrow_export, "flight", fake_flight)

    got = read_proximadb_collection("c", host="h", port=1)
    assert got.num_rows == 2
    fake_fc.close.assert_called_once()
