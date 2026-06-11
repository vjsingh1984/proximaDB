"""Offline unit tests for proximadb_sdk.observability.

Fully self-contained: the observability module collects metrics/traces/logs
in memory with no network transport, so no mocking of sockets is needed.
"""

import time

import pytest

from proximadb_sdk.observability import (
    LogLevel,
    MetricDefinition,
    MetricsCollector,
    MetricType,
    Observability,
    Span,
    SpanContext,
    StructuredLogger,
    Tracer,
    metered,
    traced,
)


# ---------------------------------------------------------------------------
# SpanContext
# ---------------------------------------------------------------------------
def test_span_context_to_headers_sampled():
    ctx = SpanContext(trace_id="abc", span_id="def", baggage={"k": "v", "a": "b"})
    headers = ctx.to_headers()
    assert headers["traceparent"] == "00-abc-def-01"
    assert "k=v" in headers["tracestate"]
    assert "a=b" in headers["tracestate"]


def test_span_context_to_headers_not_sampled():
    ctx = SpanContext(trace_id="abc", span_id="def", sampled=False)
    headers = ctx.to_headers()
    assert headers["traceparent"].endswith("-00")
    assert headers["tracestate"] == ""


def test_span_context_from_headers_roundtrip():
    ctx = SpanContext(trace_id="t1", span_id="s1", baggage={"x": "y"})
    parsed = SpanContext.from_headers(ctx.to_headers())
    assert parsed is not None
    assert parsed.trace_id == "t1"
    assert parsed.span_id == "s1"
    assert parsed.sampled is True
    assert parsed.baggage == {"x": "y"}


def test_span_context_from_headers_not_sampled():
    parsed = SpanContext.from_headers({"traceparent": "00-t-s-00"})
    assert parsed is not None
    assert parsed.sampled is False


def test_span_context_from_headers_missing():
    assert SpanContext.from_headers({}) is None


def test_span_context_from_headers_malformed():
    assert SpanContext.from_headers({"traceparent": "00-only-three"}) is None


def test_span_context_from_headers_bad_tracestate_item():
    # tracestate item without '=' is skipped
    parsed = SpanContext.from_headers(
        {"traceparent": "00-t-s-01", "tracestate": "novalue,good=1"}
    )
    assert parsed is not None
    assert parsed.baggage == {"good": "1"}


# ---------------------------------------------------------------------------
# Span
# ---------------------------------------------------------------------------
def test_span_attributes_events_status_and_duration():
    ctx = SpanContext(trace_id="t", span_id="s")
    span = Span(name="op", context=ctx, start_time_ns=time.time_ns())
    assert span.duration_ms == 0  # not ended yet

    span.set_attribute("foo", "bar")
    assert span.attributes["foo"] == "bar"

    span.add_event("evt", {"a": 1})
    span.add_event("evt2")
    assert len(span.events) == 2
    assert span.events[1]["attributes"] == {}

    span.set_status("error", "boom")
    assert span.status == "error"
    assert span.attributes["status.message"] == "boom"

    span.set_status("ok")  # no message branch
    assert span.status == "ok"

    span.end()
    assert span.end_time_ns is not None
    assert span.duration_ms >= 0


# ---------------------------------------------------------------------------
# MetricsCollector
# ---------------------------------------------------------------------------
def test_metrics_collector_defaults_registered():
    mc = MetricsCollector()
    metrics = mc.get_metrics()
    assert "proximadb_requests_total" in metrics
    assert "proximadb_request_duration_seconds" in metrics


def test_metrics_inc_counter():
    mc = MetricsCollector()
    mc.inc("requests_total", labels={"method": "search", "status": "success"})
    mc.inc("requests_total", 2, labels={"method": "search", "status": "success"})
    values = mc.get_metrics()["proximadb_requests_total"]
    assert values["method=search,status=success"] == 3


def test_metrics_inc_unknown_noop():
    mc = MetricsCollector()
    mc.inc("does_not_exist")
    assert "proximadb_does_not_exist" not in mc.get_metrics()


def test_metrics_set_gauge():
    mc = MetricsCollector()
    mc.set("active_connections", 5, labels={"protocol": "grpc"})
    assert mc.get_metrics()["proximadb_active_connections"]["protocol=grpc"] == 5


def test_metrics_set_unknown_noop():
    mc = MetricsCollector()
    mc.set("nope", 1)
    assert "proximadb_nope" not in mc.get_metrics()


def test_metrics_observe_histogram():
    mc = MetricsCollector()
    mc.observe("request_duration_seconds", 0.02, labels={"method": "search"})
    mc.observe("request_duration_seconds", 0.2, labels={"method": "search"})
    entry = mc.get_metrics()["proximadb_request_duration_seconds"]["method=search"]
    assert entry["count"] == 2
    assert entry["sum"] == pytest.approx(0.22)
    # 0.02 falls in buckets >= 0.025; 0.2 falls in >= 0.25
    assert entry["buckets"][0.025] == 1
    assert entry["buckets"][0.25] == 2


def test_metrics_observe_unknown_noop():
    mc = MetricsCollector()
    mc.observe("nope", 1.0)
    assert "proximadb_nope" not in mc.get_metrics()


def test_metrics_label_key_empty():
    mc = MetricsCollector()
    mc.inc("requests_total")  # no labels -> empty key
    assert "" in mc.get_metrics()["proximadb_requests_total"]


def test_metrics_register_custom():
    mc = MetricsCollector(prefix="custom")
    mc.register(
        MetricDefinition(
            name="my_gauge", metric_type=MetricType.GAUGE, description="d"
        )
    )
    mc.set("my_gauge", 9)
    assert mc.get_metrics()["custom_my_gauge"][""] == 9


def test_metrics_export_prometheus_all_types():
    mc = MetricsCollector()
    mc.inc("requests_total", labels={"method": "search", "status": "success"})
    mc.set("active_connections", 3, labels={"protocol": "rest"})
    mc.observe("request_duration_seconds", 0.005, labels={"method": "search"})
    # Counter without labels to exercise no-label branch
    mc.inc("requests_total")
    out = mc.export_prometheus()
    assert "# HELP proximadb_requests_total" in out
    assert "# TYPE proximadb_requests_total counter" in out
    assert "proximadb_active_connections{protocol=rest} 3" in out
    assert "proximadb_request_duration_seconds_bucket" in out
    assert 'le="+Inf"' in out
    assert "proximadb_request_duration_seconds_sum" in out
    assert "proximadb_request_duration_seconds_count" in out


# ---------------------------------------------------------------------------
# Tracer
# ---------------------------------------------------------------------------
def test_tracer_start_end_span():
    tr = Tracer(service_name="svc")
    span = tr.start_span("op", attributes={"k": "v"})
    assert span.attributes["service.name"] == "svc"
    assert span.attributes["k"] == "v"
    assert tr.get_current_span() is span
    tr.end_span(span)
    assert tr.get_current_span() is None
    assert span in tr.get_spans()


def test_tracer_child_span_inherits_trace_id():
    tr = Tracer()
    parent = tr.start_span("parent")
    child = tr.start_span("child", parent=parent.context)
    assert child.context.trace_id == parent.context.trace_id
    assert child.context.parent_span_id == parent.context.span_id


def test_tracer_span_processor_called_and_errors_swallowed():
    tr = Tracer()
    seen = []
    tr.add_span_processor(lambda s: seen.append(s.name))

    def bad(_s):
        raise ValueError("processor boom")

    tr.add_span_processor(bad)
    span = tr.start_span("op")
    tr.end_span(span)
    assert "op" in seen


def test_tracer_trace_context_manager_success():
    tr = Tracer()
    with tr.trace("work", attributes={"a": 1}) as span:
        assert span.status == "ok"
    assert tr.get_spans()[-1].name == "work"
    assert tr.get_spans()[-1].end_time_ns is not None


def test_tracer_trace_context_manager_error():
    tr = Tracer()
    with pytest.raises(RuntimeError):
        with tr.trace("work") as span:
            raise RuntimeError("oops")
    assert span.status == "error"
    assert span.attributes["status.message"] == "oops"


def test_tracer_clear_and_export_otlp():
    tr = Tracer()
    parent = tr.start_span("p")
    parent.add_event("e", {"x": 1})
    tr.end_span(parent)
    child = tr.start_span("c", parent=parent.context)
    child.set_status("error", "bad")
    tr.end_span(child)

    otlp = tr.export_otlp()
    assert len(otlp) == 2
    names = {s["name"] for s in otlp}
    assert names == {"p", "c"}
    err = next(s for s in otlp if s["name"] == "c")
    assert err["status"]["code"] == 2
    ok = next(s for s in otlp if s["name"] == "p")
    assert ok["status"]["code"] == 1
    assert ok["events"][0]["name"] == "e"

    tr.clear()
    assert tr.get_spans() == []


# ---------------------------------------------------------------------------
# StructuredLogger
# ---------------------------------------------------------------------------
def test_structured_logger_levels_and_handler():
    captured = []
    logger = StructuredLogger(name="t", level=LogLevel.DEBUG)
    logger.add_handler(lambda e: captured.append(e))

    logger.debug("d", a=1)
    logger.info("i")
    logger.warning("w")
    logger.error("e")
    logger.critical("c")

    levels = [e["level"] for e in captured]
    assert levels == ["debug", "info", "warning", "error", "critical"]
    assert captured[0]["a"] == 1


def test_structured_logger_handler_error_swallowed():
    logger = StructuredLogger(level=LogLevel.DEBUG)

    def bad(_e):
        raise ValueError("handler boom")

    logger.add_handler(bad)
    # Should not raise
    logger.info("hi")


def test_structured_logger_non_json_format():
    logger = StructuredLogger(level=LogLevel.DEBUG, json_format=False)
    logger.info("plain message", foo="bar")
    logger.info("no extra")  # no kwargs branch


def test_structured_logger_with_context_shares_handlers():
    captured = []
    base = StructuredLogger(level=LogLevel.DEBUG)
    base.add_handler(lambda e: captured.append(e))
    child = base.with_context(request_id="r1")
    child.info("msg")
    assert captured[-1]["request_id"] == "r1"


# ---------------------------------------------------------------------------
# Observability
# ---------------------------------------------------------------------------
class FakeClient:
    def __init__(self):
        self.calls = []

    def search(self, *a, **k):
        self.calls.append("search")
        return ["r"]

    def insert_vectors(self, *a, **k):
        raise RuntimeError("insert failed")

    # get_vector intentionally present
    def get_vector(self, *a, **k):
        return "vec"


def test_observability_disabled_components():
    obs = Observability(
        enable_metrics=False, enable_tracing=False, enable_logging=False
    )
    assert obs.metrics is None
    assert obs.tracer is None
    assert obs.logger is None
    assert obs.get_prometheus_metrics() == ""
    assert obs.get_traces() == []
    # record_* are no-ops when metrics disabled
    obs.record_search("c", 1, 1.0)
    obs.record_insert("c", 1, 1.0)
    obs.record_cache_hit()
    obs.record_cache_miss()


def test_observability_instruments_client_success():
    client = FakeClient()
    obs = Observability(client=client)
    result = client.search("col", [0.1], top_k=5)
    assert result == ["r"]
    metrics = obs.metrics.get_metrics()["proximadb_requests_total"]
    assert metrics["method=search,status=success"] == 1
    spans = obs.tracer.get_spans()
    assert any(s.name == "proximadb.search" for s in spans)


def test_observability_instruments_client_error():
    client = FakeClient()
    obs = Observability(client=client)
    with pytest.raises(RuntimeError):
        client.insert_vectors("col", [])
    metrics = obs.metrics.get_metrics()["proximadb_requests_total"]
    assert metrics["method=insert,status=error"] == 1
    # error span recorded
    err_span = next(s for s in obs.tracer.get_spans() if s.name == "proximadb.insert")
    assert err_span.status == "error"


def test_observability_instrument_decorator():
    obs = Observability(client=None)

    def fn(x):
        return x * 2

    wrapped = obs.instrument(fn, "double")
    assert wrapped(21) == 42
    metrics = obs.metrics.get_metrics()["proximadb_requests_total"]
    assert metrics["method=double,status=success"] == 1


def test_observability_trace_operation_with_tracer():
    obs = Observability(client=None)
    with obs.trace_operation("custom", user="u1") as span:
        assert span is not None
        assert span.attributes["user"] == "u1"
    assert any(s.name == "custom" for s in obs.tracer.get_spans())


def test_observability_trace_operation_without_tracer():
    obs = Observability(client=None, enable_tracing=False)
    with obs.trace_operation("custom") as span:
        assert span is None


def test_observability_record_helpers():
    obs = Observability(client=None)
    obs.record_search("c1", 5, 12.0)
    obs.record_insert("c1", 3, 8.0)
    obs.record_cache_hit("query")
    obs.record_cache_miss("vector")
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_search_results_total"]["collection=c1"] == 5
    assert metrics["proximadb_vectors_inserted_total"]["collection=c1"] == 3
    assert metrics["proximadb_cache_hits_total"]["cache_type=query"] == 1
    assert metrics["proximadb_cache_misses_total"]["cache_type=vector"] == 1


def test_observability_get_prometheus_and_traces():
    obs = Observability(client=None)
    obs.record_cache_hit()
    assert "proximadb_cache_hits_total" in obs.get_prometheus_metrics()
    with obs.trace_operation("x"):
        pass
    traces = obs.get_traces()
    assert any(t["name"] == "x" for t in traces)


def test_observability_instrument_logs_error_when_logger_enabled():
    client = FakeClient()
    obs = Observability(client=client)
    captured = []
    obs.logger.add_handler(lambda e: captured.append(e))
    with pytest.raises(RuntimeError):
        client.insert_vectors()
    assert any(e["level"] == "error" for e in captured)


# ---------------------------------------------------------------------------
# Module-level decorators: traced / metered
# ---------------------------------------------------------------------------
def test_traced_with_observability_arg():
    obs = Observability(client=None)

    @traced("decorated_op")
    def fn(o, val):
        return val

    assert fn(obs, 7) == 7
    assert any(s.name == "decorated_op" for s in obs.tracer.get_spans())


def test_traced_without_observability_arg():
    @traced("op")
    def fn(val):
        return val + 1

    assert fn(10) == 11


def test_traced_observability_without_tracer():
    obs = Observability(client=None, enable_tracing=False)

    @traced("op")
    def fn(o):
        return "done"

    assert fn(obs) == "done"


def test_metered_success_records_metrics():
    obs = Observability(client=None)

    @metered("metered_op")
    def fn(o, x):
        return x

    assert fn(obs, 5) == 5
    metrics = obs.metrics.get_metrics()["proximadb_requests_total"]
    assert metrics["method=metered_op,status=success"] == 1


def test_metered_error_records_status_error():
    obs = Observability(client=None)

    @metered("metered_op")
    def fn(o):
        raise ValueError("boom")

    with pytest.raises(ValueError):
        fn(obs)
    metrics = obs.metrics.get_metrics()["proximadb_requests_total"]
    assert metrics["method=metered_op,status=error"] == 1


def test_metered_without_observability_arg():
    @metered("op")
    def fn(x):
        return x

    assert fn(99) == 99
