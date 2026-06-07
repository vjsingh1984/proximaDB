"""Offline unit tests for proximadb_sdk.timeseries.

Fully offline: the time-series module wraps a generic client object whose
methods (create_timeseries_collection / ingest_timeseries / query_timeseries)
we replace with MagicMock or hand fakes. No network, no server.
"""

from __future__ import annotations

from datetime import datetime, timezone
from unittest.mock import MagicMock

import pytest

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
    """The repository uses class-level shared dicts; reset between tests."""
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()
    yield
    TimeSeriesRepository._shared_batch_buffer.clear()
    TimeSeriesRepository._shared_collections.clear()
    TimeSeriesRepository._shared_points.clear()


def make_client():
    """A client whose RPCs succeed and return server-shaped dicts."""
    client = MagicMock()
    client.create_timeseries_collection.return_value = {"collection_id": "ts1"}
    client.ingest_timeseries.return_value = {
        "success": True,
        "ingested_count": 1,
        "failed_count": 0,
    }
    client.query_timeseries.return_value = {"points": [], "metrics": [], "total_points": 0}
    return client


# =============================================================================
# Enums and ValueColumn
# =============================================================================


def test_value_column_defaults_and_type_alias():
    vc = ValueColumn(name="cpu")
    assert vc.data_type == ValueType.FLOAT
    assert vc.aggregation == AggregationType.AVG
    assert vc.type == ValueType.FLOAT
    d = vc.to_dict()
    assert d["name"] == "cpu"
    assert d["data_type"] == "float"
    assert d["aggregation"] == "avg"


def test_value_column_string_coercion_and_legacy_type_kwarg():
    vc = ValueColumn(name="loc", type="int", aggregation="sum", unit="lines", description="x")
    assert vc.data_type == ValueType.INT
    assert vc.aggregation == AggregationType.SUM
    assert vc.unit == "lines"
    assert vc.description == "x"


def test_value_column_type_setter():
    vc = ValueColumn(name="flag", data_type=ValueType.BOOL)
    assert vc.type == ValueType.BOOL
    vc.type = "string"
    assert vc.data_type == ValueType.STRING
    vc.type = ValueType.UINT
    assert vc.data_type == ValueType.UINT


def test_enums_values():
    assert AggregationType.OHLC.value == "ohlc"
    assert DownsampleMode.EMA.value == "ema"
    assert CompressionCodec.GORILLA.value == "gorilla"


# =============================================================================
# TimeSeriesCollectionConfig
# =============================================================================


def test_collection_config_retention_parsing():
    cfg = TimeSeriesCollectionConfig(name="c", retention="30d")
    assert cfg.retention_ms == 30 * 24 * 60 * 60 * 1000
    assert cfg.retention.endswith("ms")


def test_collection_config_retention_none_and_unknown_suffix():
    assert TimeSeriesCollectionConfig._parse_retention_ms(None) is None
    assert TimeSeriesCollectionConfig._parse_retention_ms("12x") is None
    cfg = TimeSeriesCollectionConfig(name="c")
    assert cfg.retention is None


def test_collection_config_value_columns_from_dict_and_object():
    cfg = TimeSeriesCollectionConfig(
        name="m",
        value_columns=[{"name": "a"}, ValueColumn(name="b", data_type=ValueType.INT)],
        tags_columns=["host"],
    )
    assert [c.name for c in cfg.value_columns] == ["a", "b"]
    assert cfg.tag_columns == ["host"]
    assert cfg.tags_columns == ["host"]


def test_collection_config_default_compression_and_string():
    cfg = TimeSeriesCollectionConfig(name="m", default_compression="zigzag")
    assert cfg.compression == CompressionCodec.ZIGZAG
    cfg2 = TimeSeriesCollectionConfig(name="m", compression=CompressionCodec.NONE)
    assert cfg2.compression == CompressionCodec.NONE


def test_collection_config_to_dict():
    cfg = TimeSeriesCollectionConfig(
        name="m",
        value_columns=[ValueColumn(name="a")],
        tag_columns=["t"],
        retention_ms=1000,
        resolution_ms=500,
    )
    d = cfg.to_dict()
    assert d["name"] == "m"
    assert d["tag_columns"] == ["t"]
    assert d["retention_ms"] == 1000
    assert d["resolution_ms"] == 500
    assert d["compression"] == "gorilla"
    assert d["value_columns"][0]["name"] == "a"


# =============================================================================
# Metric / AggregatedMetric / response
# =============================================================================


def test_metric_to_dict_with_datetime():
    ts = datetime(2026, 3, 10, 10, 0, 0)
    m = Metric(timestamp=ts, values={"v": 1.0}, tags={"host": "a"})
    d = m.to_dict()
    assert d["v"] == 1.0
    assert d["host"] == "a"
    assert d["timestamp"] == ts.isoformat()


def test_metric_to_dict_with_string_timestamp():
    m = Metric(timestamp="2026-03-10T10:00:00Z", values={"v": 2})
    d = m.to_dict()
    assert d["timestamp"] == "2026-03-10T10:00:00Z"
    assert d["v"] == 2


def test_aggregated_metric_to_dict():
    am = AggregatedMetric(
        timestamp=datetime(2026, 1, 1), values={"avg": 5.0}, count=3, tags={"g": "x"}
    )
    d = am.to_dict()
    assert d["_count"] == 3
    assert d["avg"] == 5.0
    assert d["g"] == "x"


def test_query_response_dict_like():
    resp = TimeSeriesQueryResponse(metrics=[{"v": 1}], total_points=1, query_time_ms=5)
    assert resp.get("total_points") == 1
    assert resp.get("missing", "d") == "d"
    assert len(resp) == 1
    assert list(resp) == [{"v": 1}]
    assert resp.to_dict()["query_time_ms"] == 5


def test_query_response_raw_points_iteration():
    resp = TimeSeriesQueryResponse(raw_points=[{"x": 1}, {"x": 2}])
    assert len(resp) == 2
    assert list(resp) == [{"x": 1}, {"x": 2}]


# =============================================================================
# TimeSeriesFilter builder
# =============================================================================


def test_filter_builder_full():
    f = (
        TimeSeriesFilter()
        .tag("language", "python")
        .tag_in("env", ["prod", "stage"])
        .and_()
        .gte("complexity", 10)
        .lte("complexity", 100)
        .gt("loc", 1)
        .lt("loc", 9999)
        .time_range("2026-01-01T00:00:00", "2026-03-01T00:00:00")
        .limit(50)
    )
    d = f.to_dict()
    assert d["logic"] == "AND"
    assert d["limit"] == 50
    assert {"key": "language", "op": "eq", "value": "python"} in d["tag_filters"]
    assert {"key": "env", "op": "in", "value": ["prod", "stage"]} in d["tag_filters"]
    assert len(d["value_filters"]) == 4
    assert d["start_time"] is not None
    assert d["end_time"] is not None


def test_filter_or_logic_and_datetime_range():
    f = TimeSeriesFilter().or_().time_range(
        datetime(2026, 1, 1), datetime(2026, 2, 1)
    )
    d = f.to_dict()
    assert d["logic"] == "OR"
    assert d["start_time"] is not None


def test_filter_empty():
    d = TimeSeriesFilter().to_dict()
    assert d["tag_filters"] == []
    assert d["value_filters"] == []
    assert d["start_time"] is None
    assert d["limit"] is None


# =============================================================================
# Repository static helpers
# =============================================================================


def test_parse_timestamp_variants():
    p = TimeSeriesRepository._parse_timestamp
    assert p("2026-03-10T10:00:00Z") == datetime(2026, 3, 10, 10, 0, 0)
    assert p("2026-03-10T10:00:00+00:00") == datetime(2026, 3, 10, 10, 0, 0)
    naive = datetime(2026, 3, 10, 10, 0, 0)
    assert p(naive) == naive
    aware = datetime(2026, 3, 10, 10, 0, 0, tzinfo=timezone.utc)
    assert p(aware) == naive


def test_format_timestamp():
    s = TimeSeriesRepository._format_timestamp(datetime(2026, 3, 10, 10, 0, 0))
    assert s.endswith("Z")


def test_normalize_aggregation():
    n = TimeSeriesRepository._normalize_aggregation
    assert n(None) is None
    assert n(AggregationType.SUM) == AggregationType.SUM
    assert n("avg") == AggregationType.AVG


def test_interval_to_bucket_ms():
    i = TimeSeriesRepository._interval_to_bucket_ms
    assert i(None) is None
    assert i("") is None
    assert i("5m") == 5 * 60 * 1000
    assert i("1d") == 24 * 60 * 60 * 1000
    assert i("100ms") == 100
    assert i("nounit") is None


def test_infer_value_type():
    iv = TimeSeriesRepository._infer_value_type
    assert iv(True) == ValueType.BOOL
    assert iv(5) == ValueType.INT
    assert iv(3.2) == ValueType.FLOAT
    assert iv("s") == ValueType.STRING


def test_aggregate_value_branches():
    repo = TimeSeriesRepository(client=make_client())
    av = repo._aggregate_value
    assert av([1, 2, 3], AggregationType.COUNT) == 3
    assert av([], AggregationType.SUM) is None
    assert av([1, 2, 3], AggregationType.SUM) == 6
    assert av([2, 4], AggregationType.AVG) == 3
    assert av([1, 5, 3], AggregationType.MIN) == 1
    assert av([1, 5, 3], AggregationType.MAX) == 5
    assert av([7, 8], AggregationType.FIRST) == 7
    assert av([7, 8], AggregationType.LAST) == 8
    # default branch (e.g. MEDIAN -> mean fallback)
    assert av([2, 4], AggregationType.MEDIAN) == 3


def test_bucket_start():
    repo = TimeSeriesRepository(client=make_client())
    ts = datetime(2026, 1, 1, 0, 0, 30)
    assert repo._bucket_start(ts, None) == ts
    bucketed = repo._bucket_start(ts, 60 * 1000)
    assert bucketed == datetime(2026, 1, 1, 0, 0, 0)


# =============================================================================
# Collection management
# =============================================================================


def test_create_collection_success():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    cfg = TimeSeriesCollectionConfig(name="m", value_columns=[ValueColumn(name="v")])
    cid = repo.create_collection(cfg)
    assert cid == "ts1"
    assert "ts1" in repo._collections
    client.create_timeseries_collection.assert_called_once()


def test_create_collection_default_id_from_name():
    client = make_client()
    client.create_timeseries_collection.return_value = {}
    repo = TimeSeriesRepository(client=client)
    cid = repo.create_collection(TimeSeriesCollectionConfig(name="named"))
    assert cid == "named"


def test_create_collection_error_wrapped():
    client = make_client()
    client.create_timeseries_collection.side_effect = RuntimeError("boom")
    repo = TimeSeriesRepository(client=client)
    with pytest.raises(ProximaDBError):
        repo.create_collection(TimeSeriesCollectionConfig(name="m"))


def test_get_list_delete_collection():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    repo.create_collection(
        TimeSeriesCollectionConfig(name="m", value_columns=[ValueColumn(name="v")])
    )
    repo.ingest("ts1", [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})])

    info = repo.get_collection("ts1")
    assert info["point_count"] == 1
    assert info["oldest_timestamp"] is not None

    cols = repo.list_collections()
    assert len(cols) == 1

    assert repo.delete_collection("ts1") is True
    assert repo.get_collection("ts1") is None


def test_get_collection_missing():
    repo = TimeSeriesRepository(client=make_client())
    assert repo.get_collection("nope") is None


def test_collection_info_empty_points():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    repo.create_collection(TimeSeriesCollectionConfig(name="m"))
    info = repo.get_collection("ts1")
    assert info["point_count"] == 0
    assert info["oldest_timestamp"] is None
    assert info["newest_timestamp"] is None


# =============================================================================
# Ingestion
# =============================================================================


def test_ingest_empty():
    repo = TimeSeriesRepository(client=make_client())
    res = repo.ingest("c", [])
    assert res["ingested_count"] == 0


def test_ingest_metric_objects_server_path():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    res = repo.ingest(
        "c",
        [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0}, tags={"h": "a"})],
    )
    assert res == client.ingest_timeseries.return_value
    assert len(repo._points["c"]) == 1
    # collection inferred
    assert "c" in repo._collections


def test_ingest_dict_metrics():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            {
                "timestamp": "2026-01-01T00:00:00Z",
                "values": {"v": 1.0},
                "tags": {"h": "a"},
            }
        ],
    )
    assert len(repo._points["c"]) == 1


def test_ingest_dict_explicit_values_tags():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 5}, "tags": {}}],
    )
    assert repo._points["c"][0]["values"] == {"v": 5}


def test_ingest_server_failure_falls_back_local():
    client = make_client()
    client.ingest_timeseries.side_effect = RuntimeError("down")
    repo = TimeSeriesRepository(client=client)
    res = repo.ingest(
        "c", [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})]
    )
    assert res["fallback"] == "local"
    assert res["ingested_count"] == 1
    assert len(repo._points["c"]) == 1


def test_ingest_autoflush_on_batch_size():
    client = make_client()
    repo = TimeSeriesRepository(client=client, batch_size=2)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0}),
            Metric(timestamp="2026-01-01T00:01:00Z", values={"v": 2.0}),
        ],
    )
    # buffer flushed because len >= batch_size
    assert repo._batch_buffer["c"] == []


def test_ingest_batch_returns_flushed_count():
    client = make_client()
    repo = TimeSeriesRepository(client=client, batch_size=1000)
    res = repo.ingest_batch(
        "c", [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})]
    )
    assert res["flushed_count"] == 1


# =============================================================================
# Query
# =============================================================================


def test_query_server_metrics_path():
    client = make_client()
    client.query_timeseries.return_value = {
        "metrics": [
            {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}, "tags": {}}
        ],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client=client)
    resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert resp.total_points == 1
    assert len(resp.metrics) == 1
    assert isinstance(resp.metrics[0], Metric)


def test_query_server_raw_points_path():
    client = make_client()
    client.query_timeseries.return_value = {
        "points": [{"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}}],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client=client)
    resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert len(resp.raw_points) == 1


def test_query_fallback_local_raw_no_aggregation():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 5.0}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T02:00:00Z", values={"v": 7.0}, tags={"h": "a"}),
        ],
    )
    with pytest.warns(UserWarning):
        resp = repo.query("c", "2026-01-01T00:00:00Z", "2026-01-01T03:00:00Z")
    assert len(resp.raw_points) == 2
    assert resp.total_points == 2


def test_query_fallback_local_with_aggregation_and_filter():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("server down")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2.0}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T00:00:30Z", values={"v": 4.0}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T00:00:10Z", values={"v": 9.0}, tags={"h": "b"}),
        ],
    )
    flt = TimeSeriesFilter().tag("h", "a")
    with pytest.warns(UserWarning):
        resp = repo.query(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
            filter=flt,
            aggregation="avg",
            interval="1m",
        )
    # only h=a points (2,4) -> avg 3.0 in one 1m bucket
    assert len(resp.metrics) == 1
    assert resp.metrics[0]["value"] == 3.0


def test_query_ohlc_aggregation():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("local")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"price": 10.0}),
            Metric(timestamp="2026-01-01T00:00:10Z", values={"price": 15.0}),
            Metric(timestamp="2026-01-01T00:00:20Z", values={"price": 8.0}),
            Metric(timestamp="2026-01-01T00:00:30Z", values={"price": 12.0}),
        ],
    )
    with pytest.warns(UserWarning):
        resp = repo.query(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
            aggregation=AggregationType.OHLC,
            interval="1h",
        )
    m = resp.metrics[0]
    assert m["open"] == 10.0
    assert m["high"] == 15.0
    assert m["low"] == 8.0
    assert m["close"] == 12.0


def test_query_with_dict_filter_and_value_filters():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("local")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 5.0}, tags={"h": "a"}),
            Metric(timestamp="2026-01-01T00:00:01Z", values={"v": 50.0}, tags={"h": "a"}),
        ],
    )
    filt = {
        "tag_filters": {"h": "a"},
        "value_filters": [{"column": "v", "op": "gte", "value": 10}],
        "logic": "AND",
    }
    with pytest.warns(UserWarning):
        resp = repo.query(
            "c", "2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z", filter=filt
        )
    assert len(resp.raw_points) == 1
    assert resp.raw_points[0]["values"]["v"] == 50.0


def test_matches_filter_all_value_ops_and_or_logic():
    repo = TimeSeriesRepository(client=make_client())
    point = {
        "timestamp": datetime(2026, 1, 1),
        "values": {"v": 5},
        "tags": {"h": "a"},
    }
    # tag mismatch via explicit tag_filters arg
    assert repo._matches_filter(point, None, tag_filters={"h": "b"}) is False
    assert repo._matches_filter(point, None, tag_filters={"h": "a"}) is True
    # None filter
    assert repo._matches_filter(point, None) is True

    or_filter = {
        "logic": "OR",
        "value_filters": [
            {"column": "v", "op": "gt", "value": 100},
            {"column": "v", "op": "lt", "value": 10},
        ],
    }
    assert repo._matches_filter(point, or_filter) is True

    lte_gte = {
        "value_filters": [
            {"column": "v", "op": "lte", "value": 5},
            {"column": "v", "op": "gte", "value": 5},
        ]
    }
    assert repo._matches_filter(point, lte_gte) is True

    # tag in-list and time bounds
    tin = {
        "tag_filters": [{"key": "h", "op": "in", "value": ["a", "z"]}],
        "start_time": "2025-01-01T00:00:00Z",
        "end_time": "2027-01-01T00:00:00Z",
    }
    assert repo._matches_filter(point, tin) is True

    # default-op tag (eq) via TimeSeriesFilter object
    tsf = TimeSeriesFilter().tag("h", "a")
    assert repo._matches_filter(point, tsf) is True


# =============================================================================
# get_latest
# =============================================================================


def test_get_latest_and_batch():
    client = make_client()
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0}, tags={"f": "x"}),
            Metric(timestamp="2026-01-01T02:00:00Z", values={"v": 3.0}, tags={"f": "x"}),
            Metric(timestamp="2026-01-01T01:00:00Z", values={"v": 2.0}, tags={"f": "y"}),
        ],
    )
    latest = repo.get_latest("c", {"f": "x"})
    assert latest is not None
    assert latest.values["v"] == 3.0

    assert repo.get_latest("c", {"f": "missing"}) is None

    batch = repo.get_latest_batch("c", [{"f": "x"}, {"f": "y"}, {"f": "z"}])
    assert batch[0].values["v"] == 3.0
    assert batch[1].values["v"] == 2.0
    assert batch[2] is None


# =============================================================================
# aggregate
# =============================================================================


def test_aggregate_simple_path():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("local")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2.0}),
            Metric(timestamp="2026-01-01T00:00:30Z", values={"v": 6.0}),
        ],
    )
    with pytest.warns(UserWarning):
        res = repo.aggregate(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
            aggregation="avg",
            interval="1h",
            value_column="v",
        )
    assert "results" in res
    assert res["results"][0]["value"] == 4.0


def test_aggregate_pipeline_group_by():
    client = make_client()
    client.query_timeseries.side_effect = RuntimeError("local")
    repo = TimeSeriesRepository(client=client)
    repo.ingest(
        "c",
        [
            Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 2.0}, tags={"g": "a"}),
            Metric(timestamp="2026-01-01T00:00:10Z", values={"v": 4.0}, tags={"g": "a"}),
            Metric(timestamp="2026-01-01T00:00:20Z", values={"v": 9.0}, tags={"g": "b"}),
        ],
    )
    with pytest.warns(UserWarning):
        res = repo.aggregate(
            "c",
            "2026-01-01T00:00:00Z",
            "2026-01-01T01:00:00Z",
            pipeline=[
                {
                    "stage": "group_by",
                    "aggregation": "avg",
                    "bucket_ms": 60 * 60 * 1000,
                    "tag_columns": ["g"],
                }
            ],
        )
    assert len(res["results"]) == 2


def test_aggregate_pipeline_non_group_stage():
    client = make_client()
    client.query_timeseries.return_value = {
        "metrics": [
            {"timestamp": "2026-01-01T00:00:00Z", "values": {"v": 1.0}, "tags": {}}
        ],
        "total_points": 1,
    }
    repo = TimeSeriesRepository(client=client)
    res = repo.aggregate(
        "c",
        "2026-01-01T00:00:00Z",
        "2026-01-01T01:00:00Z",
        pipeline=[{"stage": "transform", "aggregation": "sum"}],
    )
    assert isinstance(res["results"], list)


# =============================================================================
# downsample / flush
# =============================================================================


def test_downsample_stub():
    repo = TimeSeriesRepository(client=make_client())
    res = repo.downsample("src", "dst", "1h", mode=DownsampleMode.SMA)
    assert res["success"] is True
    assert res["downsampled"] == 0


def test_flush_batch_paths():
    client = make_client()
    repo = TimeSeriesRepository(client=client, batch_size=1000)
    # unknown collection
    assert repo.flush_batch("none")["flushed"] == 0
    repo.ingest("c", [Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})])
    # has buffered point
    res = repo.flush_batch("c")
    assert res["flushed"] == 1
    # now empty
    assert repo.flush_batch("c")["flushed"] == 0


# =============================================================================
# High-level ProximaDBTimeSeries + factory
# =============================================================================


def test_high_level_create_and_ingest_and_query():
    client = make_client()
    client.query_timeseries.return_value = {"points": [], "metrics": [], "total_points": 0}
    ts = ProximaDBTimeSeries(client, batch_size=1000)

    out = ts.create_collection(
        name="m", value_columns=[ValueColumn(name="v")], tags_columns=["h"]
    )
    assert out["success"] is True
    cid = out["collection_id"]

    ing = ts.ingest(cid, points=[Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})])
    assert ing == client.ingest_timeseries.return_value

    resp = ts.query(cid, "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z")
    assert isinstance(resp, TimeSeriesQueryResponse)


def test_high_level_create_with_config():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    cfg = TimeSeriesCollectionConfig(name="m", value_columns=[ValueColumn(name="v")])
    out = ts.create_collection(config=cfg)
    assert out["success"] is True


def test_high_level_ingest_metrics_kwarg():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    res = ts.ingest("c", metrics=[Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 1.0})])
    assert res == client.ingest_timeseries.return_value


def test_high_level_get_latest_list_delete_flush_aggregate():
    client = make_client()
    ts = ProximaDBTimeSeries(client)
    ts.create_collection(name="m", value_columns=[ValueColumn(name="v")])
    ts.ingest("ts1", points=[Metric(timestamp="2026-01-01T00:00:00Z", values={"v": 9.0}, tags={"h": "a"})])

    latest = ts.get_latest("ts1", {"h": "a"})
    assert latest.values["v"] == 9.0

    assert ts.list_collections()
    assert ts.flush("ts1")["success"] is True

    client.query_timeseries.return_value = {"metrics": [], "points": [], "total_points": 0}
    agg = ts.aggregate("ts1", "2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z", aggregation="sum", interval="1h")
    assert "results" in agg

    assert ts.delete_collection("ts1") is True


def test_factory_function():
    client = make_client()
    api = create_timeseries_api(client, batch_size=5, compression=CompressionCodec.NONE)
    assert isinstance(api, ProximaDBTimeSeries)
    assert api._repository._batch_size == 5
