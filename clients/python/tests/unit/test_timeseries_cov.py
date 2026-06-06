"""Offline unit tests for proximadb_sdk.timeseries.

All transport is mocked via an injected fake client; no network, no server.
"""

from __future__ import annotations

import warnings
from datetime import datetime, timezone
from unittest.mock import MagicMock

import pytest

from proximadb_sdk.timeseries import (
    AggregatedMetric,
    AggregationType,
    CompressionCodec,
    DownsampleMode,
    Metric,
    ProximaDBTimeSeries,
    TimeSeriesCollectionConfig,
    TimeSeriesFilter,
    TimeSeriesQueryResponse,
    TimeSeriesRepository,
    ValueColumn,
    ValueType,
    create_timeseries_api,
)


@pytest.fixture(autouse=True)
def _clear_shared_state():
    """The repository uses class-level shared dicts; reset between tests."""
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()
    yield
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()


def make_client():
    """A fake backend client whose RPCs succeed by default."""
    client = MagicMock()
    client.create_timeseries_collection.return_value = {"collection_id": "cid"}
    client.ingest_timeseries.return_value = {
        "success": True,
        "ingested_count": 0,
        "failed_count": 0,
    }
    client.query_timeseries.return_value = {"points": [], "metrics": [], "total_points": 0}
    return client


# ---------------------------------------------------------------------------
# ValueColumn
# ---------------------------------------------------------------------------


def test_value_column_defaults_and_to_dict():
    vc = ValueColumn(name="complexity")
    assert vc.data_type is ValueType.FLOAT
    assert vc.aggregation is AggregationType.AVG
    d = vc.to_dict()
    assert d == {
        "name": "complexity",
        "data_type": "float",
        "aggregation": "avg",
        "unit": None,
        "description": None,
    }


def test_value_column_string_coercion_and_type_kwarg():
    vc = ValueColumn(name="loc", type="int", aggregation="sum", unit="count")
    assert vc.data_type is ValueType.INT
    assert vc.aggregation is AggregationType.SUM
    assert vc.unit == "count"
    # property
    assert vc.type is ValueType.INT
    vc.type = "float"
    assert vc.data_type is ValueType.FLOAT
    vc.type = ValueType.BOOL
    assert vc.data_type is ValueType.BOOL


def test_value_column_enum_inputs():
    vc = ValueColumn(
        name="x", data_type=ValueType.UINT, aggregation=AggregationType.MAX
    )
    assert vc.data_type is ValueType.UINT
    assert vc.aggregation is AggregationType.MAX


# ---------------------------------------------------------------------------
# TimeSeriesCollectionConfig
# ---------------------------------------------------------------------------


def test_collection_config_retention_parsing_variants():
    assert TimeSeriesCollectionConfig._parse_retention_ms(None) is None
    assert TimeSeriesCollectionConfig._parse_retention_ms("30d") == 30 * 86400 * 1000
    assert TimeSeriesCollectionConfig._parse_retention_ms("12w") == 12 * 7 * 86400 * 1000
    assert TimeSeriesCollectionConfig._parse_retention_ms("1y") == 365 * 86400 * 1000
    assert TimeSeriesCollectionConfig._parse_retention_ms("500ms") == 500
    assert TimeSeriesCollectionConfig._parse_retention_ms("10s") == 10000
    assert TimeSeriesCollectionConfig._parse_retention_ms("5m") == 5 * 60 * 1000
    assert TimeSeriesCollectionConfig._parse_retention_ms("2h") == 2 * 3600 * 1000
    # unknown suffix -> None
    assert TimeSeriesCollectionConfig._parse_retention_ms("nonsense") is None


def test_collection_config_full_construction_and_to_dict():
    cfg = TimeSeriesCollectionConfig(
        name="metrics",
        value_columns=[
            ValueColumn(name="a", data_type="float"),
            {"name": "b", "type": "int"},
        ],
        tags_columns=["host"],
        retention="30d",
        default_compression="zigzag",
        downsampling={"interval": "1h"},
        partitioning={"by": "day"},
        resolution_ms=1000,
    )
    assert cfg.compression is CompressionCodec.ZIGZAG
    assert cfg.tags_columns == ["host"]
    assert cfg.retention == f"{30 * 86400 * 1000}ms"
    assert len(cfg.value_columns) == 2
    d = cfg.to_dict()
    assert d["name"] == "metrics"
    assert d["compression"] == "zigzag"
    assert d["resolution_ms"] == 1000
    assert d["tag_columns"] == ["host"]


def test_collection_config_compression_default_and_retention_none():
    cfg = TimeSeriesCollectionConfig(name="c", tag_columns=["t"])
    assert cfg.compression is CompressionCodec.GORILLA
    assert cfg.retention is None
    assert cfg.tags_columns == ["t"]


def test_collection_config_explicit_retention_ms_wins():
    cfg = TimeSeriesCollectionConfig(name="c", retention_ms=999, retention="30d")
    assert cfg.retention_ms == 999


def test_collection_config_enum_compression():
    cfg = TimeSeriesCollectionConfig(name="c", compression=CompressionCodec.SNP)
    assert cfg.compression is CompressionCodec.SNP


# ---------------------------------------------------------------------------
# Metric / AggregatedMetric
# ---------------------------------------------------------------------------


def test_metric_to_dict_with_datetime():
    dt = datetime(2026, 3, 10, 10, 0, 0)
    m = Metric(timestamp=dt, values={"v": 1.5}, tags={"host": "a"})
    d = m.to_dict()
    assert d["timestamp"] == dt.isoformat()
    assert d["v"] == 1.5
    assert d["host"] == "a"


def test_metric_to_dict_with_string():
    m = Metric(timestamp="2026-03-10T10:00:00Z", values={"v": 1})
    d = m.to_dict()
    assert d["timestamp"] == "2026-03-10T10:00:00Z"


def test_aggregated_metric_to_dict():
    am = AggregatedMetric(
        timestamp=datetime(2026, 1, 1),
        values={"avg": 2.0},
        count=5,
        tags={"k": "v"},
    )
    d = am.to_dict()
    assert d["_count"] == 5
    assert d["avg"] == 2.0
    assert d["k"] == "v"


# ---------------------------------------------------------------------------
# TimeSeriesQueryResponse
# ---------------------------------------------------------------------------


def test_query_response_metrics_and_iteration():
    resp = TimeSeriesQueryResponse(metrics=[{"a": 1}], total_points=1, query_time_ms=3)
    assert len(resp) == 1
    assert list(resp) == [{"a": 1}]
    assert resp.get("total_points") == 1
    assert resp.get("missing", "x") == "x"
    d = resp.to_dict()
    assert d["query_time_ms"] == 3


def test_query_response_falls_back_to_raw_points():
    resp = TimeSeriesQueryResponse(raw_points=[{"p": 1}, {"p": 2}])
    assert len(resp) == 2
    assert list(resp) == [{"p": 1}, {"p": 2}]


# ---------------------------------------------------------------------------
# TimeSeriesFilter
# ---------------------------------------------------------------------------


def test_timeseries_filter_builder():
    f = (
        TimeSeriesFilter()
        .tag("language", "python")
        .tag_in("host", ["a", "b"])
        .and_()
        .gte("complexity", 10)
        .lte("complexity", 100)
        .gt("loc", 1)
        .lt("loc", 1000)
        .time_range("2026-01-01T00:00:00", datetime(2026, 3, 1))
        .limit(50)
        .or_()
    )
    d = f.to_dict()
    assert d["logic"] == "OR"
    assert d["limit"] == 50
    assert d["start_time"] is not None
    assert d["end_time"] is not None
    assert len(d["tag_filters"]) == 2
    assert len(d["value_filters"]) == 4


def test_timeseries_filter_empty_times():
    d = TimeSeriesFilter().to_dict()
    assert d["start_time"] is None
    assert d["end_time"] is None


# ---------------------------------------------------------------------------
# Repository static helpers
# ---------------------------------------------------------------------------


def test_parse_timestamp_variants():
    repo = TimeSeriesRepository(make_client())
    # Z suffix
    dt = repo._parse_timestamp("2026-03-10T10:00:00Z")
    assert dt.tzinfo is None
    # tz-aware offset -> normalized to UTC naive
    dt2 = repo._parse_timestamp("2026-03-10T12:00:00+02:00")
    assert dt2 == datetime(2026, 3, 10, 10, 0, 0)
    # naive string
    dt3 = repo._parse_timestamp("2026-03-10T10:00:00")
    assert dt3 == datetime(2026, 3, 10, 10, 0, 0)
    # datetime passthrough
    dt4 = repo._parse_timestamp(datetime(2026, 1, 1))
    assert dt4 == datetime(2026, 1, 1)


def test_format_timestamp():
    repo = TimeSeriesRepository(make_client())
    s = repo._format_timestamp(datetime(2026, 3, 10, 10, 0, 0))
    assert s.endswith("Z")


def test_normalize_aggregation():
    repo = TimeSeriesRepository(make_client())
    assert repo._normalize_aggregation(None) is None
    assert repo._normalize_aggregation("sum") is AggregationType.SUM
    assert repo._normalize_aggregation(AggregationType.MAX) is AggregationType.MAX


def test_interval_to_bucket_ms():
    repo = TimeSeriesRepository(make_client())
    assert repo._interval_to_bucket_ms(None) is None
    assert repo._interval_to_bucket_ms("") is None
    assert repo._interval_to_bucket_ms("1d") == 86400 * 1000
    assert repo._interval_to_bucket_ms("5m") == 5 * 60 * 1000
    assert repo._interval_to_bucket_ms("100ms") == 100
    assert repo._interval_to_bucket_ms("2h") == 2 * 3600 * 1000
    assert repo._interval_to_bucket_ms("30s") == 30000
    assert repo._interval_to_bucket_ms("100") is None


def test_infer_value_type():
    repo = TimeSeriesRepository(make_client())
    assert repo._infer_value_type(True) is ValueType.BOOL
    assert repo._infer_value_type(3) is ValueType.INT
    assert repo._infer_value_type(3.5) is ValueType.FLOAT
    assert repo._infer_value_type("s") is ValueType.STRING


def test_normalize_metric_from_metric_and_dict():
    repo = TimeSeriesRepository(make_client())
    m = Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"h": "a"})
    n = repo._normalize_metric(m)
    assert n["values"] == {"v": 1}
    assert n["tags"] == {"h": "a"}
    # dict with explicit values/tags
    n2 = repo._normalize_metric(
        {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 2}, "tags": {"h": "b"}}
    )
    assert n2["values"] == {"v": 2}
    # dict with flattened values (no values key)
    n3 = repo._normalize_metric(
        {"timestamp": "2026-01-01T00:00:00Z", "v": 9, "extra": 1}
    )
    assert n3["values"] == {"v": 9, "extra": 1}


def test_bucket_start():
    repo = TimeSeriesRepository(make_client())
    ts = datetime(2026, 1, 1, 10, 30, 0)
    # no bucket -> same
    assert repo._bucket_start(ts, None) == ts
    # 1h bucket -> floored to hour
    b = repo._bucket_start(ts, 3600 * 1000)
    assert b == datetime(2026, 1, 1, 10, 0, 0)


def test_aggregate_value_all_branches():
    repo = TimeSeriesRepository(make_client())
    vals = [1, 2, 3, 4]
    assert repo._aggregate_value(vals, AggregationType.COUNT) == 4
    assert repo._aggregate_value(vals, AggregationType.SUM) == 10
    assert repo._aggregate_value(vals, AggregationType.AVG) == 2.5
    assert repo._aggregate_value(vals, AggregationType.MIN) == 1
    assert repo._aggregate_value(vals, AggregationType.MAX) == 4
    assert repo._aggregate_value(vals, AggregationType.FIRST) == 1
    assert repo._aggregate_value(vals, AggregationType.LAST) == 4
    # unknown agg -> mean fallback
    assert repo._aggregate_value(vals, AggregationType.MEDIAN) == 2.5
    # no numeric values -> None
    assert repo._aggregate_value(["a", "b"], AggregationType.SUM) is None
    # booleans excluded from numeric
    assert repo._aggregate_value([True, False], AggregationType.SUM) is None


# ---------------------------------------------------------------------------
# Collection management
# ---------------------------------------------------------------------------


def test_create_collection_success():
    client = make_client()
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(
        name="metrics", value_columns=[ValueColumn(name="v")], tag_columns=["h"]
    )
    cid = repo.create_collection(cfg)
    assert cid == "cid"
    client.create_timeseries_collection.assert_called_once()
    assert "cid" in repo._collections


def test_create_collection_uses_name_when_no_id():
    client = make_client()
    client.create_timeseries_collection.return_value = {}
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(name="metrics")
    cid = repo.create_collection(cfg)
    assert cid == "metrics"


def test_create_collection_error_wraps():
    from proximadb_sdk.exceptions import ProximaDBError

    client = make_client()
    client.create_timeseries_collection.side_effect = ValueError("boom")
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(name="bad")
    with pytest.raises(ProximaDBError):
        repo.create_collection(cfg)


def test_get_collection_none_when_missing():
    repo = TimeSeriesRepository(make_client())
    assert repo.get_collection("nope") is None


def test_get_and_list_collections_with_points():
    client = make_client()
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(name="c", value_columns=[ValueColumn(name="v")])
    repo.create_collection(cfg)
    repo.ingest(
        "cid",
        [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"h": "a"})],
    )
    info = repo.get_collection("cid")
    assert info["point_count"] == 1
    assert info["oldest_timestamp"] is not None
    assert info["newest_timestamp"] is not None
    listed = repo.list_collections()
    assert any(c["id"] == "cid" for c in listed)


def test_delete_collection():
    repo = TimeSeriesRepository(make_client())
    cfg = TimeSeriesCollectionConfig(name="c")
    cid = repo.create_collection(cfg)
    assert repo.delete_collection(cid) is True
    assert cid not in repo._collections


# ---------------------------------------------------------------------------
# Ingestion
# ---------------------------------------------------------------------------


def test_ingest_empty():
    repo = TimeSeriesRepository(make_client())
    res = repo.ingest("c", [])
    assert res == {"success": True, "ingested_count": 0, "failed_count": 0}


def test_ingest_server_success_path():
    client = make_client()
    client.ingest_timeseries.return_value = {"success": True, "ingested_count": 2}
    repo = TimeSeriesRepository(client)
    res = repo.ingest(
        "code_metrics",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"h": "a"}),
            {"timestamp": "2026-01-01T01:00:00Z", "values": {"v": 2}, "tags": {"h": "a"}},
        ],
    )
    assert res["success"] is True
    client.ingest_timeseries.assert_called_once()
    # collection auto-inferred
    assert "code_metrics" in repo._collections
    assert len(repo._points["code_metrics"]) == 2


def test_ingest_fallback_local_when_server_errors():
    client = make_client()
    client.ingest_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    res = repo.ingest(
        "c",
        [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1})],
    )
    assert res["fallback"] == "local"
    assert res["ingested_count"] == 1


def test_ingest_auto_flush_when_batch_full():
    client = make_client()
    repo = TimeSeriesRepository(client, batch_size=2)
    metrics = [
        Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}),
        Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 2}),
    ]
    repo.ingest("c", metrics)
    # buffer flushed at batch_size
    assert repo._batch_buffer["c"] == []


def test_ingest_batch_flushes():
    client = make_client()
    repo = TimeSeriesRepository(client, batch_size=1000)
    res = repo.ingest_batch(
        "c", [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1})]
    )
    assert "flushed_count" in res
    assert res["flushed_count"] == 1


# ---------------------------------------------------------------------------
# Query
# ---------------------------------------------------------------------------


def test_query_server_returns_metrics():
    client = make_client()
    client.query_timeseries.return_value = {
        "metrics": [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1}, "tags": {}}],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client)
    resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z", aggregation="avg")
    assert resp.total_points == 1
    assert len(resp.metrics) == 1


def test_query_server_returns_raw_points():
    client = make_client()
    client.query_timeseries.return_value = {
        "points": [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1}}],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client)
    resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z")
    assert resp.total_points == 1
    assert len(resp.raw_points) == 1


def test_query_local_fallback_raw_points():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-15T00:00:00Z", values={"v": 5}, tags={"h": "a"}),
            Metric(timestamp="2026-02-15T00:00:00Z", values={"v": 10}, tags={"h": "b"}),
        ],
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "c", "2026-01-01T00:00:00Z", "2026-01-31T00:00:00Z"
        )
    # only the Jan point is within range
    assert resp.total_points == 1
    assert len(resp.raw_points) == 1


def test_query_local_fallback_aggregated():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T00:30:00Z", values={"v": 3}, tags={"h": "a"}),
        ],
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            aggregation="avg",
            interval="1d",
        )
    assert len(resp.metrics) == 1
    # avg of 1 and 3 = 2
    assert resp.metrics[0]["v"] == 2.0


def test_query_local_fallback_ohlc():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"price": 10}, tags={"s": "x"}),
            Metric(timestamp="2026-01-01T01:00:00Z", values={"price": 20}, tags={"s": "x"}),
            Metric(timestamp="2026-01-01T02:00:00Z", values={"price": 5}, tags={"s": "x"}),
            Metric(timestamp="2026-01-01T03:00:00Z", values={"price": 15}, tags={"s": "x"}),
        ],
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            aggregation="ohlc",
            interval="1d",
        )
    m = resp.metrics[0]
    assert m["open"] == 10
    assert m["high"] == 20
    assert m["low"] == 5
    assert m["close"] == 15


def test_query_local_fallback_with_filter():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-05T00:00:00Z", values={"v": 5}, tags={"lang": "py"}),
            Metric(timestamp="2026-01-06T00:00:00Z", values={"v": 50}, tags={"lang": "rs"}),
        ],
    )
    f = TimeSeriesFilter().tag("lang", "py").gte("v", 1)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-01-31T00:00:00Z", filter=f)
    assert resp.total_points == 1


# ---------------------------------------------------------------------------
# _matches_filter direct coverage
# ---------------------------------------------------------------------------


def test_matches_filter_branches():
    repo = TimeSeriesRepository(make_client())
    point = {
        "timestamp": datetime(2026, 1, 15),
        "values": {"v": 50},
        "tags": {"lang": "py", "host": "a"},
    }
    # tag_filters dict (the param) mismatch
    assert repo._matches_filter(point, None, {"lang": "rs"}) is False
    assert repo._matches_filter(point, None, {"lang": "py"}) is True
    # filter None -> True
    assert repo._matches_filter(point, None) is True
    # dict filter with tag_filters as dict
    fd = {"tag_filters": {"lang": "py"}, "value_filters": [], "logic": "AND"}
    assert repo._matches_filter(point, fd) is True
    # tag in-list op
    fd_in = {"tag_filters": [{"key": "lang", "op": "in", "value": ["py", "rs"]}]}
    assert repo._matches_filter(point, fd_in) is True
    # value filter ops
    assert repo._matches_filter(point, {"value_filters": [{"column": "v", "op": "gte", "value": 10}]})
    assert repo._matches_filter(point, {"value_filters": [{"column": "v", "op": "lte", "value": 100}]})
    assert repo._matches_filter(point, {"value_filters": [{"column": "v", "op": "gt", "value": 10}]})
    assert repo._matches_filter(point, {"value_filters": [{"column": "v", "op": "lt", "value": 100}]})
    assert repo._matches_filter(point, {"value_filters": [{"column": "v", "op": "eq", "value": 50}]})
    # time bounds
    assert repo._matches_filter(point, {"start_time": "2026-01-01T00:00:00Z"})
    assert repo._matches_filter(point, {"end_time": "2026-02-01T00:00:00Z"})
    # OR logic with one true
    fd_or = {
        "value_filters": [{"column": "v", "op": "gt", "value": 1000}],
        "tag_filters": [{"key": "lang", "op": "eq", "value": "py"}],
        "logic": "OR",
    }
    assert repo._matches_filter(point, fd_or) is True
    # TimeSeriesFilter object accepted
    assert repo._matches_filter(point, TimeSeriesFilter().tag("lang", "py")) is True


# ---------------------------------------------------------------------------
# get_latest / get_latest_batch
# ---------------------------------------------------------------------------


def test_get_latest_and_batch():
    client = make_client()
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"f": "a"}),
            Metric(timestamp="2026-01-02T00:00:00Z", values={"v": 2}, tags={"f": "a"}),
        ],
    )
    latest = repo.get_latest("c", {"f": "a"})
    assert latest is not None
    assert latest.values["v"] == 2
    # no match
    assert repo.get_latest("c", {"f": "zzz"}) is None
    # batch
    batch = repo.get_latest_batch("c", [{"f": "a"}, {"f": "zzz"}])
    assert batch[0] is not None
    assert batch[1] is None


# ---------------------------------------------------------------------------
# aggregate / downsample / flush
# ---------------------------------------------------------------------------


def test_aggregate_simple():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 4}, tags={"h": "a"}),
        ],
    )
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = repo.aggregate(
            "c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z",
            aggregation="avg", interval="1d", value_column="v",
        )
    assert "results" in res
    assert "query_time_ms" in res


def test_aggregate_pipeline_group_by():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 4}, tags={"h": "b"}),
        ],
    )
    pipeline = [
        {"stage": "group_by", "aggregation": "sum", "bucket_ms": 86400000, "tag_columns": ["h"]}
    ]
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = repo.aggregate(
            "c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z", pipeline=pipeline
        )
    assert "results" in res
    # grouped by host -> two buckets
    assert len(res["results"]) == 2


def test_aggregate_pipeline_non_group_stage():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    repo.ingest(
        "c",
        [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2}, tags={"h": "a"})],
    )
    pipeline = [{"stage": "aggregate", "aggregation": "avg", "bucket_ms": 86400000}]
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = repo.aggregate(
            "c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z", pipeline=pipeline
        )
    assert "metrics" in res


def test_downsample():
    repo = TimeSeriesRepository(make_client())
    res = repo.downsample("src", "dst", "1h", mode=DownsampleMode.SMA)
    assert res == {"success": True, "downsampled": 0}


def test_flush_batch_variants():
    repo = TimeSeriesRepository(make_client())
    # unknown collection
    assert repo.flush_batch("unknown") == {"success": True, "flushed": 0}
    # known but empty
    repo._ensure_collection("c")
    assert repo.flush_batch("c") == {"success": True, "flushed": 0}
    # with data
    repo._batch_buffer["c"] = [{"x": 1}, {"x": 2}]
    assert repo.flush_batch("c") == {"success": True, "flushed": 2}


# ---------------------------------------------------------------------------
# High-level ProximaDBTimeSeries
# ---------------------------------------------------------------------------


def test_high_level_create_collection_from_kwargs():
    client = make_client()
    ts = ProximaDBTimeSeries(client, batch_size=10)
    res = ts.create_collection(
        name="metrics",
        value_columns=[ValueColumn(name="v")],
        tags_columns=["h"],
        retention="7d",
    )
    assert res["success"] is True
    assert res["collection_id"] == "cid"


def test_high_level_create_collection_from_config():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    cfg = TimeSeriesCollectionConfig(name="metrics")
    res = ts.create_collection(config=cfg)
    assert res["collection_id"] == "cid"


def test_high_level_ingest_with_points_and_metrics():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    # points kwarg
    r1 = ts.ingest("c", points=[Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1})])
    assert r1["success"] is True
    # metrics kwarg
    r2 = ts.ingest("c", metrics=[Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 2})])
    assert r2["success"] is True
    # neither
    r3 = ts.ingest("c")
    assert r3["ingested_count"] == 0


def test_high_level_query_get_latest_list_delete_flush():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    ts.ingest("c", points=[Metric(timestamp="2026-01-15T00:00:00Z", values={"v": 1}, tags={"h": "a"})])
    # query via server (returns empty by default)
    resp = ts.query("c", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z")
    assert isinstance(resp, TimeSeriesQueryResponse)
    # latest
    latest = ts.get_latest("c", {"h": "a"})
    assert latest is not None
    # list
    listed = ts.list_collections()
    assert isinstance(listed, list)
    # flush
    assert "success" in ts.flush("c")
    # delete
    assert ts.delete_collection("c") is True


def test_high_level_aggregate():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    ts = ProximaDBTimeSeries(client)
    ts.ingest("c", points=[Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 5}, tags={"h": "a"})])
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = ts.aggregate(
            "c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z",
            aggregation="sum", interval="1d",
        )
    assert "results" in res


# ---------------------------------------------------------------------------
# Factory
# ---------------------------------------------------------------------------


def test_create_timeseries_api_factory():
    client = make_client()
    api = create_timeseries_api(client, batch_size=5, compression=CompressionCodec.NONE)
    assert isinstance(api, ProximaDBTimeSeries)
