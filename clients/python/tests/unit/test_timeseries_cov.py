"""Offline unit tests for proximadb_sdk.timeseries.

All transport is via an injected MagicMock client; no network, no DB boot.
"""

from __future__ import annotations

import warnings
from datetime import datetime, timezone

import pytest
from unittest.mock import MagicMock

from proximadb_sdk.exceptions import ProximaDBError
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
    """Reset the class-level shared dicts before/after each test."""
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()
    yield
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()


def make_client():
    """A MagicMock client whose timeseries methods return plausible dicts."""
    client = MagicMock()
    client.create_timeseries_collection.return_value = {"collection_id": "ts1"}
    client.ingest_timeseries.return_value = {
        "success": True,
        "ingested_count": 1,
        "failed_count": 0,
    }
    client.query_timeseries.return_value = {"points": [], "total_points": 0}
    return client


# ---------------------------------------------------------------------------
# Enums + simple data models
# ---------------------------------------------------------------------------


def test_value_column_string_coercion_and_type_property():
    vc = ValueColumn(name="c", type="int", aggregation="sum", unit="ms", description="d")
    assert vc.data_type == ValueType.INT
    assert vc.type == ValueType.INT
    assert vc.aggregation == AggregationType.SUM
    d = vc.to_dict()
    assert d["name"] == "c"
    assert d["data_type"] == "int"
    assert d["aggregation"] == "sum"
    assert d["unit"] == "ms"

    # exercise the type setter (string + enum branches)
    vc.type = "float"
    assert vc.data_type == ValueType.FLOAT
    vc.type = ValueType.BOOL
    assert vc.data_type == ValueType.BOOL


def test_value_column_enum_passthrough():
    vc = ValueColumn(
        name="c", data_type=ValueType.UINT, aggregation=AggregationType.MAX
    )
    assert vc.data_type == ValueType.UINT
    assert vc.aggregation == AggregationType.MAX


def test_collection_config_retention_parsing_and_dict():
    cfg = TimeSeriesCollectionConfig(
        name="m",
        value_columns=[ValueColumn(name="v"), {"name": "w", "type": "int"}],
        tags_columns=["host"],
        retention="30d",
        compression="zigzag",
    )
    assert cfg.name == "m"
    assert len(cfg.value_columns) == 2
    assert cfg.value_columns[1].data_type == ValueType.INT
    assert cfg.tags_columns == ["host"]
    assert cfg.retention_ms == 30 * 24 * 60 * 60 * 1000
    assert cfg.retention.endswith("ms")
    assert cfg.compression == CompressionCodec.ZIGZAG
    d = cfg.to_dict()
    assert d["compression"] == "zigzag"
    assert d["tag_columns"] == ["host"]


def test_collection_config_default_compression_and_no_retention():
    cfg = TimeSeriesCollectionConfig(
        name="m",
        tag_columns=["a"],
        default_compression="gorilla",
    )
    assert cfg.compression == CompressionCodec.GORILLA
    assert cfg.retention_ms is None
    assert cfg.retention is None


def test_retention_parsing_variants_and_unknown():
    p = TimeSeriesCollectionConfig._parse_retention_ms
    assert p(None) is None
    assert p("500ms") == 500
    assert p("2s") == 2000
    assert p("5m") == 5 * 60 * 1000
    assert p("3h") == 3 * 60 * 60 * 1000
    assert p("1w") == 7 * 24 * 60 * 60 * 1000
    assert p("1y") == 365 * 24 * 60 * 60 * 1000
    assert p("garbage") is None


def test_metric_to_dict_datetime_and_str():
    m = Metric(
        timestamp=datetime(2026, 3, 10, 10, 0, 0),
        values={"v": 1.0},
        tags={"host": "a"},
    )
    d = m.to_dict()
    assert "T" in d["timestamp"]
    assert d["v"] == 1.0
    assert d["host"] == "a"

    m2 = Metric(timestamp="2026-03-10T10:00:00Z", values={"v": 2})
    assert m2.to_dict()["timestamp"] == "2026-03-10T10:00:00Z"


def test_aggregated_metric_to_dict():
    am = AggregatedMetric(
        timestamp=datetime(2026, 1, 1),
        values={"avg": 5.0},
        count=3,
        tags={"k": "v"},
    )
    d = am.to_dict()
    assert d["_count"] == 3
    assert d["avg"] == 5.0
    assert d["k"] == "v"


def test_query_response_dict_like():
    resp = TimeSeriesQueryResponse(
        metrics=[{"a": 1}], total_points=1, query_time_ms=5
    )
    assert resp.get("total_points") == 1
    assert resp.get("missing", "x") == "x"
    assert len(resp) == 1
    assert list(resp) == [{"a": 1}]
    assert resp.to_dict()["query_time_ms"] == 5

    raw_resp = TimeSeriesQueryResponse(raw_points=[{"b": 2}, {"c": 3}])
    assert len(raw_resp) == 2
    assert list(raw_resp) == [{"b": 2}, {"c": 3}]


# ---------------------------------------------------------------------------
# TimeSeriesFilter builder
# ---------------------------------------------------------------------------


def test_filter_builder_fluent():
    f = (
        TimeSeriesFilter()
        .tag("language", "python")
        .tag_in("host", ["a", "b"])
        .and_()
        .gte("c", 10)
        .lte("c", 100)
        .gt("d", 1)
        .lt("d", 9)
        .or_()
        .time_range("2026-01-01T00:00:00", datetime(2026, 3, 1))
        .limit(50)
    )
    d = f.to_dict()
    assert d["logic"] == "OR"
    assert d["limit"] == 50
    assert d["start_time"] is not None
    assert d["end_time"] is not None
    assert len(d["tag_filters"]) == 2
    assert len(d["value_filters"]) == 4


def test_filter_empty_dict():
    d = TimeSeriesFilter().to_dict()
    assert d["start_time"] is None
    assert d["end_time"] is None
    assert d["limit"] is None
    assert d["logic"] == "AND"


# ---------------------------------------------------------------------------
# Static helpers on the repository
# ---------------------------------------------------------------------------


def test_parse_and_format_timestamp():
    repo = TimeSeriesRepository(make_client())
    dt = repo._parse_timestamp("2026-03-10T10:00:00Z")
    assert dt.tzinfo is None
    # datetime passthrough with tz
    aware = datetime(2026, 3, 10, 10, 0, 0, tzinfo=timezone.utc)
    dt2 = repo._parse_timestamp(aware)
    assert dt2.tzinfo is None
    out = repo._format_timestamp(dt)
    assert out.endswith("Z")


def test_normalize_aggregation_and_interval():
    repo = TimeSeriesRepository(make_client())
    assert repo._normalize_aggregation(None) is None
    assert repo._normalize_aggregation("sum") == AggregationType.SUM
    assert repo._normalize_aggregation(AggregationType.AVG) == AggregationType.AVG

    assert repo._interval_to_bucket_ms(None) is None
    assert repo._interval_to_bucket_ms("1d") == 24 * 60 * 60 * 1000
    assert repo._interval_to_bucket_ms("5m") == 5 * 60 * 1000
    assert repo._interval_to_bucket_ms("100ms") == 100
    assert repo._interval_to_bucket_ms("xyz") is None


def test_infer_value_type():
    repo = TimeSeriesRepository(make_client())
    assert repo._infer_value_type(True) == ValueType.BOOL
    assert repo._infer_value_type(3) == ValueType.INT
    assert repo._infer_value_type(3.5) == ValueType.FLOAT
    assert repo._infer_value_type("x") == ValueType.STRING


def test_normalize_metric_variants():
    repo = TimeSeriesRepository(make_client())
    # Metric object
    n1 = repo._normalize_metric(
        Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1}, tags={"t": "a"})
    )
    assert n1["values"] == {"v": 1}
    # dict with explicit values+tags
    n2 = repo._normalize_metric(
        {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 2}, "tags": {"t": "b"}}
    )
    assert n2["tags"] == {"t": "b"}
    # flat dict: values inferred from non-reserved keys
    n3 = repo._normalize_metric(
        {"timestamp": "2026-01-01T00:00:00Z", "v": 3, "host": "z"}
    )
    assert n3["values"]["v"] == 3
    assert n3["values"]["host"] == "z"


# ---------------------------------------------------------------------------
# Collection management
# ---------------------------------------------------------------------------


def test_create_collection_success():
    client = make_client()
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(
        name="cm",
        value_columns=[ValueColumn(name="complexity", data_type=ValueType.FLOAT)],
        tag_columns=["file"],
    )
    cid = repo.create_collection(cfg)
    assert cid == "ts1"
    client.create_timeseries_collection.assert_called_once()
    assert "ts1" in repo._collections


def test_create_collection_error_wrapped():
    client = make_client()
    client.create_timeseries_collection.side_effect = RuntimeError("boom")
    repo = TimeSeriesRepository(client)
    cfg = TimeSeriesCollectionConfig(name="bad")
    with pytest.raises(ProximaDBError):
        repo.create_collection(cfg)


def test_get_list_delete_collection():
    client = make_client()
    repo = TimeSeriesRepository(client)
    assert repo.get_collection("nope") is None

    cfg = TimeSeriesCollectionConfig(
        name="cm", value_columns=[ValueColumn(name="v")]
    )
    repo.create_collection(cfg)
    repo.ingest(
        "ts1",
        [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}}],
    )
    info = repo.get_collection("ts1")
    assert info is not None
    assert info["point_count"] == 1
    assert info["oldest_timestamp"] is not None

    cols = repo.list_collections()
    assert any(c["id"] == "ts1" for c in cols)

    assert repo.delete_collection("ts1") is True
    assert repo.get_collection("ts1") is None


# ---------------------------------------------------------------------------
# Ingestion
# ---------------------------------------------------------------------------


def test_ingest_empty():
    repo = TimeSeriesRepository(make_client())
    res = repo.ingest("c", [])
    assert res["ingested_count"] == 0


def test_ingest_server_path_and_infer_collection():
    client = make_client()
    repo = TimeSeriesRepository(client)
    res = repo.ingest(
        "cm",
        [
            {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}, "tags": {"h": "a"}},
            Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 2.0}, tags={"h": "a"}),
        ],
    )
    assert res["success"] is True
    client.ingest_timeseries.assert_called_once()
    # collection auto-inferred
    assert "cm" in repo._collections
    assert len(repo._points["cm"]) == 2


def test_ingest_fallback_local_on_server_error():
    client = make_client()
    client.ingest_timeseries.side_effect = ConnectionError("down")
    repo = TimeSeriesRepository(client)
    res = repo.ingest(
        "cm", [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}}]
    )
    assert res["fallback"] == "local"
    assert res["ingested_count"] == 1
    assert len(repo._points["cm"]) == 1


def test_ingest_auto_flush_small_batch_size():
    client = make_client()
    repo = TimeSeriesRepository(client, batch_size=1)
    repo.ingest("cm", [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}}])
    # batch should have been flushed (buffer reset to empty)
    assert repo._batch_buffer["cm"] == []


def test_ingest_batch_flushes():
    client = make_client()
    repo = TimeSeriesRepository(client, batch_size=1000)
    res = repo.ingest_batch(
        "cm", [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}}]
    )
    assert "flushed_count" in res


# ---------------------------------------------------------------------------
# Query
# ---------------------------------------------------------------------------


def _seed(repo, cid="cm"):
    repo.ingest(
        cid,
        [
            {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 10.0}, "tags": {"h": "a"}},
            {"timestamp": "2026-01-01T00:30:00Z", "values": {"v": 20.0}, "tags": {"h": "a"}},
            {"timestamp": "2026-01-01T01:00:00Z", "values": {"v": 30.0}, "tags": {"h": "b"}},
        ],
    )


def test_query_server_with_metrics():
    client = make_client()
    client.query_timeseries.return_value = {
        "metrics": [
            {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1}, "tags": {}},
        ],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client)
    resp = repo.query("cm", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert resp.total_points == 1
    assert len(resp.metrics) == 1


def test_query_server_with_raw_points():
    client = make_client()
    client.query_timeseries.return_value = {
        "points": [{"timestamp": "2026-01-01T00:00:00Z", "v": 1}],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client)
    resp = repo.query("cm", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert len(resp.raw_points) == 1


def test_query_fallback_raw_local():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query("cm", "2026-01-01T00:00:00Z", "2026-01-01T02:00:00Z")
    assert resp.total_points == 3
    assert len(resp.raw_points) == 3


def test_query_fallback_aggregated_local():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            aggregation="avg",
            interval="1d",
        )
    assert len(resp.metrics) >= 1
    # single 1d bucket averaging 10,20,30 -> 20
    assert resp.metrics[0]["value"] == 20.0


def test_query_fallback_with_filter_and_tag_filters():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    f = TimeSeriesFilter().gte("v", 20)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            filter=f,
            tag_filters={"h": "a"},
        )
    # only h=a AND v>=20 -> the 20.0 point
    assert resp.total_points == 1


def test_query_fallback_ohlc():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = repo.query(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            aggregation="ohlc",
            interval="1d",
        )
    m = resp.metrics[0]
    assert m["open"] == 10.0
    assert m["high"] == 30.0
    assert m["low"] == 10.0
    assert m["close"] == 30.0


# ---------------------------------------------------------------------------
# Matching / filter dict variants
# ---------------------------------------------------------------------------


def test_matches_filter_dict_forms():
    repo = TimeSeriesRepository(make_client())
    point = {
        "timestamp": repo._parse_timestamp("2026-01-01T00:00:00Z"),
        "values": {"v": 50},
        "tags": {"h": "a", "env": "prod"},
    }
    # tag_filters short-circuit miss
    assert repo._matches_filter(point, None, {"h": "b"}) is False
    # None filter -> True
    assert repo._matches_filter(point, None) is True
    # dict filter with dict-style tag_filters + value_filters + time bounds
    fdict = {
        "tag_filters": {"h": "a"},
        "value_filters": [{"column": "v", "op": "gt", "value": 10}],
        "start_time": "2025-12-31T00:00:00Z",
        "end_time": "2026-12-31T00:00:00Z",
        "logic": "AND",
    }
    assert repo._matches_filter(point, fdict) is True
    # OR logic with one matching tag-in
    or_dict = {
        "tag_filters": [{"key": "h", "op": "in", "value": ["a", "x"]}],
        "value_filters": [{"column": "v", "op": "lt", "value": 1}],
        "logic": "OR",
    }
    assert repo._matches_filter(point, or_dict) is True
    # lte / eq fallthrough
    lte_dict = {"value_filters": [{"column": "v", "op": "lte", "value": 50}]}
    assert repo._matches_filter(point, lte_dict) is True
    eq_dict = {"value_filters": [{"column": "v", "op": "eq", "value": 50}]}
    assert repo._matches_filter(point, eq_dict) is True


# ---------------------------------------------------------------------------
# get_latest / get_latest_batch
# ---------------------------------------------------------------------------


def test_get_latest_and_batch():
    repo = TimeSeriesRepository(make_client())
    _seed(repo)
    latest = repo.get_latest("cm", {"h": "a"})
    assert latest is not None
    assert latest.values["v"] == 20.0  # 00:30 is latest for h=a

    assert repo.get_latest("cm", {"h": "missing"}) is None

    batch = repo.get_latest_batch("cm", [{"h": "a"}, {"h": "b"}, {"h": "none"}])
    assert batch[0].values["v"] == 20.0
    assert batch[1].values["v"] == 30.0
    assert batch[2] is None


# ---------------------------------------------------------------------------
# Aggregate / downsample / flush
# ---------------------------------------------------------------------------


def test_aggregate_simple():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = repo.aggregate(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            aggregation="avg",
            interval="1d",
            value_column="v",
        )
    assert "results" in res
    assert "query_time_ms" in res
    assert res["metrics"]


def test_aggregate_pipeline_group_by_and_plain():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client)
    _seed(repo)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res = repo.aggregate(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            pipeline=[
                {
                    "stage": "group_by",
                    "aggregation": "sum",
                    "bucket_ms": 24 * 60 * 60 * 1000,
                    "tag_columns": ["h"],
                    "value_columns": ["v"],
                },
            ],
        )
    assert res["results"]

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        res2 = repo.aggregate(
            "cm",
            "2026-01-01T00:00:00Z",
            "2026-01-01T02:00:00Z",
            pipeline=[{"stage": "other", "aggregation": "max", "bucket_ms": 86400000}],
        )
    assert "results" in res2


def test_downsample_and_flush():
    repo = TimeSeriesRepository(make_client())
    ds = repo.downsample("src", "dst", "1h", mode=DownsampleMode.SMA)
    assert ds["success"] is True

    # flush on unknown collection
    assert repo.flush_batch("unknown")["flushed"] == 0
    # flush after ingest (buffer non-empty since batch_size large)
    _seed(repo)
    fl = repo.flush_batch("cm")
    assert fl["flushed"] == 3
    # flush again -> empty buffer
    assert repo.flush_batch("cm")["flushed"] == 0


def test_aggregate_value_branches():
    repo = TimeSeriesRepository(make_client())
    vals = [1, 2, 3, True, "x", None]
    assert repo._aggregate_value(vals, AggregationType.COUNT) == 6
    assert repo._aggregate_value(vals, AggregationType.SUM) == 6
    assert repo._aggregate_value(vals, AggregationType.AVG) == 2
    assert repo._aggregate_value(vals, AggregationType.MIN) == 1
    assert repo._aggregate_value(vals, AggregationType.MAX) == 3
    assert repo._aggregate_value(vals, AggregationType.FIRST) == 1
    assert repo._aggregate_value(vals, AggregationType.LAST) == 3
    # unknown -> avg fallback
    assert repo._aggregate_value(vals, AggregationType.MEDIAN) == 2
    # no numeric -> None
    assert repo._aggregate_value(["a", None], AggregationType.SUM) is None


def test_value_column_names_branches():
    repo = TimeSeriesRepository(make_client())
    # explicit
    assert repo._value_column_names("c", ["x", "y"]) == ["x", "y"]
    # from config
    repo._collections["c"] = TimeSeriesCollectionConfig(
        name="c", value_columns=[ValueColumn(name="v")]
    )
    repo._points["c"] = []
    assert repo._value_column_names("c") == ["v"]
    # no config, infer from points
    repo._collections.pop("c", None)
    repo._points["c"] = [{"values": {"a": 1, "b": 2}}]
    assert set(repo._value_column_names("c")) == {"a", "b"}
    # nothing
    repo._points["empty"] = []
    assert repo._value_column_names("empty") == []


# ---------------------------------------------------------------------------
# High-level ProximaDBTimeSeries facade
# ---------------------------------------------------------------------------


def test_high_level_create_query_ingest_flow():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("down")
    ts = ProximaDBTimeSeries(client, batch_size=1000)

    created = ts.create_collection(
        name="cm",
        value_columns=[ValueColumn(name="v")],
        tags_columns=["h"],
        retention="7d",
    )
    assert created["success"] is True
    assert created["collection_id"] == "ts1"

    ts.ingest(
        "ts1",
        points=[{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 5.0}, "tags": {"h": "a"}}],
    )
    # also exercise metrics= branch
    ts.ingest(
        "ts1",
        metrics=[{"timestamp": "2026-01-01T00:30:00Z", "values": {"v": 7.0}, "tags": {"h": "a"}}],
    )

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        resp = ts.query("ts1", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert isinstance(resp, TimeSeriesQueryResponse)

    latest = ts.get_latest("ts1", {"h": "a"})
    assert latest.values["v"] == 7.0

    cols = ts.list_collections()
    assert any(c["id"] == "ts1" for c in cols)

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        agg = ts.aggregate(
            "ts1",
            "2026-01-01T00:00:00Z",
            "2026-01-02T00:00:00Z",
            aggregation="avg",
            interval="1d",
        )
    assert "metrics" in agg

    assert ts.flush("ts1")["success"] is True
    assert ts.delete_collection("ts1") is True


def test_high_level_create_with_explicit_config():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    cfg = TimeSeriesCollectionConfig(
        name="explicit", value_columns=[ValueColumn(name="v")]
    )
    res = ts.create_collection(config=cfg)
    assert res["collection_id"] == "ts1"


def test_factory_function():
    client = make_client()
    api = create_timeseries_api(client, batch_size=5)
    assert isinstance(api, ProximaDBTimeSeries)
    assert api._repository._batch_size == 5
