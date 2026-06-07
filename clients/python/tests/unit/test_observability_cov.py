"""Offline unit tests for proximadb_sdk.observability.

Pure in-memory module (metrics/traces/logs). No transport to mock; we
exercise every class, method, branch, and decorator directly.
"""

import logging

# NOTE on coverage: this venv has editable installs (victor / chromadb /
# opentelemetry) leaking onto sys.path. Under `pytest-cov`, coverage's
# module-discovery (`should_trace` -> `find_spec`) runs inside a worker thread
# and can import those packages, whose module-level code spins a
# ThreadPoolExecutor and joins it while the main thread already holds the
# import lock -> hard deadlock (observed via faulthandler). We defensively
# pre-import the heavy chain HERE, before any coverage-driven find_spec, so the
# modules are already resolved in sys.modules and coverage does not re-import
# them inside a traced worker thread.
for _mod in (
    "opentelemetry",
    "chromadb",
    "lancedb",
    "sentence_transformers",
    "torch",
    "transformers",
    "victor",
):
    try:  # pragma: no cover - environment-dependent, best-effort warm import
        __import__(_mod)
    except Exception:
        pass

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
# Enums + dataclasses
# ---------------------------------------------------------------------------
def test_metric_type_values():
    assert MetricType.COUNTER.value == "counter"
    assert MetricType.GAUGE.value == "gauge"
    assert MetricType.HISTOGRAM.value == "histogram"
    assert MetricType.SUMMARY.value == "summary"


def test_log_level_values():
    assert LogLevel.DEBUG.value == "debug"
    assert LogLevel.CRITICAL.value == "critical"


def test_metric_definition_defaults():
    md = MetricDefinition(
        name="m", metric_type=MetricType.COUNTER, description="d"
    )
    assert md.labels == []
    assert md.buckets is None


# ---------------------------------------------------------------------------
# SpanContext
# ---------------------------------------------------------------------------
def test_span_context_to_headers_sampled():
    ctx = SpanContext(
        trace_id="abc", span_id="def", baggage={"k": "v", "a": "b"}, sampled=True
    )
    h = ctx.to_headers()
    assert h["traceparent"] == "00-abc-def-01"
    assert "k=v" in h["tracestate"]
    assert "a=b" in h["tracestate"]


def test_span_context_to_headers_not_sampled():
    ctx = SpanContext(trace_id="abc", span_id="def", sampled=False)
    h = ctx.to_headers()
    assert h["traceparent"].endswith("-00")
    assert h["tracestate"] == ""


def test_span_context_from_headers_roundtrip():
    ctx = SpanContext(trace_id="t1", span_id="s1", baggage={"u": "1"}, sampled=True)
    h = ctx.to_headers()
    parsed = SpanContext.from_headers(h)
    assert parsed is not None
    assert parsed.trace_id == "t1"
    assert parsed.span_id == "s1"
    assert parsed.sampled is True
    assert parsed.baggage == {"u": "1"}


def test_span_context_from_headers_not_sampled():
    parsed = SpanContext.from_headers({"traceparent": "00-t-s-00"})
    assert parsed is not None
    assert parsed.sampled is False
    assert parsed.baggage == {}


def test_span_context_from_headers_missing():
    assert SpanContext.from_headers({}) is None


def test_span_context_from_headers_malformed():
    # wrong number of parts
    assert SpanContext.from_headers({"traceparent": "00-only-three"}) is None


def test_span_context_from_headers_tracestate_no_equals():
    parsed = SpanContext.from_headers(
        {"traceparent": "00-t-s-01", "tracestate": "noeq,k=v"}
    )
    assert parsed is not None
    assert parsed.baggage == {"k": "v"}


# ---------------------------------------------------------------------------
# Span
# ---------------------------------------------------------------------------
def test_span_attributes_events_status_and_duration():
    ctx = SpanContext(trace_id="t", span_id="s")
    span = Span(name="op", context=ctx, start_time_ns=1_000_000)
    # duration is 0 until ended
    assert span.duration_ms == 0

    span.set_attribute("foo", "bar")
    assert span.attributes["foo"] == "bar"

    span.add_event("evt", {"a": 1})
    span.add_event("evt2")
    assert span.events[0]["name"] == "evt"
    assert span.events[0]["attributes"] == {"a": 1}
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
def test_metrics_default_registration():
    mc = MetricsCollector()
    metrics = mc.get_metrics()
    assert "proximadb_requests_total" in metrics
    assert "proximadb_request_duration_seconds" in metrics


def test_metrics_inc_counter():
    mc = MetricsCollector()
    mc.inc("requests_total", labels={"method": "search", "status": "success"})
    mc.inc("requests_total", 2, labels={"method": "search", "status": "success"})
    vals = mc.get_metrics()["proximadb_requests_total"]
    assert vals["method=search,status=success"] == 3


def test_metrics_inc_unknown_metric_noop():
    mc = MetricsCollector()
    mc.inc("does_not_exist")
    assert "proximadb_does_not_exist" not in mc.get_metrics()


def test_metrics_set_gauge():
    mc = MetricsCollector()
    mc.set("active_connections", 5, labels={"protocol": "grpc"})
    vals = mc.get_metrics()["proximadb_active_connections"]
    assert vals["protocol=grpc"] == 5


def test_metrics_set_unknown_noop():
    mc = MetricsCollector()
    mc.set("nope", 1)  # returns without error
    assert "proximadb_nope" not in mc.get_metrics()


def test_metrics_observe_histogram():
    mc = MetricsCollector()
    mc.observe("request_duration_seconds", 0.02, labels={"method": "search"})
    mc.observe("request_duration_seconds", 3.0, labels={"method": "search"})
    entry = mc.get_metrics()["proximadb_request_duration_seconds"]["method=search"]
    assert entry["count"] == 2
    assert entry["sum"] == pytest.approx(3.02)
    # 0.02 falls into buckets >= 0.025; 3.0 into >= 5.0 etc.
    assert entry["buckets"][0.025] == 1
    assert entry["buckets"][5.0] == 2


def test_metrics_observe_unknown_noop():
    mc = MetricsCollector()
    mc.observe("nope", 1.0)
    assert "proximadb_nope" not in mc.get_metrics()


def test_metrics_observe_no_buckets():
    mc = MetricsCollector()
    mc.register(
        MetricDefinition(
            name="custom_hist",
            metric_type=MetricType.HISTOGRAM,
            description="no buckets",
        )
    )
    mc.observe("custom_hist", 1.5)
    entry = mc.get_metrics()["proximadb_custom_hist"][""]
    assert entry["count"] == 1
    assert entry["buckets"] == {}


def test_metrics_label_key_empty_and_sorted():
    mc = MetricsCollector()
    assert mc._label_key(None) == ""
    assert mc._label_key({}) == ""
    assert mc._label_key({"b": "2", "a": "1"}) == "a=1,b=2"


def test_metrics_export_prometheus_all_types():
    mc = MetricsCollector()
    mc.inc("requests_total", labels={"method": "search", "status": "success"})
    mc.set("active_connections", 7, labels={"protocol": "rest"})
    mc.observe("request_duration_seconds", 0.5, labels={"method": "search"})
    out = mc.export_prometheus()
    assert "# HELP proximadb_requests_total" in out
    assert "# TYPE proximadb_requests_total counter" in out
    assert "proximadb_active_connections{protocol=rest} 7" in out
    assert "_bucket" in out
    assert "_sum" in out
    assert "_count" in out
    assert 'le="+Inf"' in out


def test_metrics_export_prometheus_no_label_counter():
    mc = MetricsCollector()
    mc.register(
        MetricDefinition(
            name="nolabel", metric_type=MetricType.COUNTER, description="x"
        )
    )
    mc.inc("nolabel")
    out = mc.export_prometheus()
    assert "proximadb_nolabel 1" in out


def test_metrics_custom_prefix():
    mc = MetricsCollector(prefix="myapp")
    assert "myapp_requests_total" in mc.get_metrics()


# ---------------------------------------------------------------------------
# Tracer
# ---------------------------------------------------------------------------
def test_tracer_start_and_end_span():
    t = Tracer("svc")
    span = t.start_span("op", attributes={"x": 1})
    assert span.attributes["service.name"] == "svc"
    assert span.attributes["x"] == 1
    assert t.get_current_span() is span
    t.end_span(span)
    assert t.get_current_span() is None
    assert span in t.get_spans()
    assert span.end_time_ns is not None


def test_tracer_child_span_inherits_parent():
    t = Tracer()
    parent = t.start_span("parent")
    child = t.start_span("child", parent=parent.context)
    assert child.context.trace_id == parent.context.trace_id
    assert child.context.parent_span_id == parent.context.span_id
    assert child.context.sampled == parent.context.sampled


def test_tracer_span_processor_called_and_exceptions_swallowed():
    t = Tracer()
    seen = []
    t.add_span_processor(lambda s: seen.append(s.name))

    def boom(_span):
        raise ValueError("processor failed")

    t.add_span_processor(boom)
    span = t.start_span("op")
    t.end_span(span)  # must not raise despite boom
    assert seen == ["op"]


def test_tracer_trace_context_manager_success():
    t = Tracer()
    with t.trace("work", attributes={"a": "b"}) as span:
        assert span.name == "work"
    assert len(t.get_spans()) == 1
    assert t.get_spans()[0].status == "ok"


def test_tracer_trace_context_manager_error():
    t = Tracer()
    with pytest.raises(RuntimeError):
        with t.trace("work") as span:
            raise RuntimeError("kaboom")
    recorded = t.get_spans()[0]
    assert recorded.status == "error"
    assert "kaboom" in recorded.attributes.get("status.message", "")


def test_tracer_end_span_when_not_current():
    t = Tracer()
    s1 = t.start_span("a")
    s2 = t.start_span("b")  # s2 becomes current
    t.end_span(s1)  # s1 is not current; current stays s2
    assert t.get_current_span() is s2


def test_tracer_clear():
    t = Tracer()
    t.end_span(t.start_span("x"))
    assert t.get_spans()
    t.clear()
    assert t.get_spans() == []


def test_tracer_export_otlp():
    t = Tracer()
    with t.trace("op") as span:
        span.set_attribute("k", "v")
        span.add_event("e", {"foo": "bar"})
    otlp = t.export_otlp()
    assert len(otlp) == 1
    rec = otlp[0]
    assert rec["name"] == "op"
    assert rec["status"]["code"] == 1
    assert any(a["key"] == "k" for a in rec["attributes"])
    assert rec["events"][0]["name"] == "e"


def test_tracer_export_otlp_error_status_code():
    t = Tracer()
    span = t.start_span("op")
    span.set_status("error")
    t.end_span(span)
    assert t.export_otlp()[0]["status"]["code"] == 2


# ---------------------------------------------------------------------------
# StructuredLogger
# ---------------------------------------------------------------------------
def test_structured_logger_levels(caplog):
    log = StructuredLogger("test_logger", level=LogLevel.DEBUG)
    captured = []
    log.add_handler(lambda e: captured.append(e))
    with caplog.at_level(logging.DEBUG, logger="test_logger"):
        log.debug("d", a=1)
        log.info("i")
        log.warning("w")
        log.error("e")
        log.critical("c")
    levels = [e["level"] for e in captured]
    assert levels == ["debug", "info", "warning", "error", "critical"]
    assert captured[0]["a"] == 1


def test_structured_logger_handler_exception_swallowed():
    log = StructuredLogger("t2")

    def boom(_e):
        raise ValueError("handler failed")

    log.add_handler(boom)
    log.info("ok")  # must not raise


def test_structured_logger_with_context():
    log = StructuredLogger("ctx")
    captured = []
    log.add_handler(lambda e: captured.append(e))
    child = log.with_context(request_id="r1")
    assert child is not log
    child.info("msg", extra="x")
    assert captured[0]["request_id"] == "r1"
    assert captured[0]["extra"] == "x"


def test_structured_logger_plain_format():
    log = StructuredLogger("plain", json_format=False)
    captured = []
    log.add_handler(lambda e: captured.append(e))
    # no kwargs -> message-only branch
    log.info("hello")
    # with kwargs -> "message extra" branch
    log.info("hi", k="v")
    assert len(captured) == 2


# ---------------------------------------------------------------------------
# Observability
# ---------------------------------------------------------------------------
def test_observability_disabled_subsystems():
    obs = Observability(
        enable_metrics=False, enable_tracing=False, enable_logging=False
    )
    assert obs.metrics is None
    assert obs.tracer is None
    assert obs.logger is None
    # helpers degrade gracefully
    assert obs.get_prometheus_metrics() == ""
    assert obs.get_traces() == []
    obs.record_search("c", 1, 5.0)
    obs.record_insert("c", 1, 5.0)
    obs.record_cache_hit()
    obs.record_cache_miss()
    # trace_operation yields None when no tracer
    with obs.trace_operation("x") as span:
        assert span is None


def test_observability_record_helpers():
    obs = Observability()
    obs.record_search("col", 3, 12.0)
    obs.record_insert("col", 10, 8.0)
    obs.record_cache_hit("query")
    obs.record_cache_miss("query")
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_search_results_total"]["collection=col"] == 3
    assert metrics["proximadb_vectors_inserted_total"]["collection=col"] == 10
    assert metrics["proximadb_cache_hits_total"]["cache_type=query"] == 1
    assert metrics["proximadb_cache_misses_total"]["cache_type=query"] == 1


def test_observability_get_prometheus_and_traces_enabled():
    obs = Observability()
    obs.record_cache_hit("query")
    # enabled-metrics branch (line 770)
    prom = obs.get_prometheus_metrics()
    assert "proximadb_cache_hits_total" in prom
    # enabled-tracer branch (line 776)
    with obs.trace_operation("op"):
        pass
    traces = obs.get_traces()
    assert len(traces) == 1
    assert traces[0]["name"] == "op"


def test_observability_trace_operation_with_tracer():
    obs = Observability()
    with obs.trace_operation("custom", user_id="42") as span:
        assert span is not None
        assert span.attributes["user_id"] == "42"
    assert obs.tracer.get_spans()


def test_observability_instrument_client_success():
    class FakeClient:
        def search(self, *a, **k):
            return ["result"]

        def insert_vectors(self, *a, **k):
            return "ok"

    client = FakeClient()
    obs = Observability(client=client)
    assert client.search("col", [0.1]) == ["result"]
    assert client.insert_vectors() == "ok"
    metrics = obs.metrics.get_metrics()
    total = metrics["proximadb_requests_total"]
    assert total.get("method=search,status=success") == 1
    assert total.get("method=insert,status=success") == 1
    # spans recorded
    assert len(obs.tracer.get_spans()) == 2


def test_observability_instrument_client_error_path():
    class FailClient:
        def search(self, *a, **k):
            raise RuntimeError("search failed")

    client = FailClient()
    obs = Observability(client=client)
    captured = []
    obs.logger.add_handler(lambda e: captured.append(e))
    with pytest.raises(RuntimeError):
        client.search()
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_requests_total"].get("method=search,status=error") == 1
    # error logged
    assert any(e["level"] == "error" for e in captured)
    # span recorded with error status
    span = obs.tracer.get_spans()[0]
    assert span.status == "error"


def test_observability_instrument_missing_method_skipped():
    class Empty:
        pass

    client = Empty()
    # should not raise even though no instrumentable methods exist
    Observability(client=client)


def test_observability_instrument_decorator():
    obs = Observability()

    def fn(x):
        return x * 2

    wrapped = obs.instrument(fn, "double")
    assert wrapped(21) == 42
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_requests_total"].get(
        "method=double,status=success"
    ) == 1


def test_observability_instrument_no_metrics_no_tracer():
    obs = Observability(enable_metrics=False, enable_tracing=False, enable_logging=False)

    def fn():
        raise ValueError("x")

    wrapped = obs.instrument(fn, "op")
    with pytest.raises(ValueError):
        wrapped()


# ---------------------------------------------------------------------------
# traced / metered decorators
# ---------------------------------------------------------------------------
def test_traced_with_observability_arg():
    obs = Observability()

    @traced("traced_op")
    def fn(o, n):
        return n + 1

    assert fn(obs, 4) == 5
    assert any(s.name == "traced_op" for s in obs.tracer.get_spans())


def test_traced_without_observability_arg():
    @traced("traced_op")
    def fn(n):
        return n + 1

    assert fn(10) == 11  # falls through, no obs found


def test_traced_with_obs_but_tracer_disabled():
    obs = Observability(enable_tracing=False)

    @traced("op")
    def fn(o):
        return "done"

    assert fn(obs) == "done"


def test_metered_success_records_metrics():
    obs = Observability()

    @metered("metered_op")
    def fn(o, n):
        return n * 3

    assert fn(obs, 2) == 6
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_requests_total"].get(
        "method=metered_op,status=success"
    ) == 1


def test_metered_error_records_error_status():
    obs = Observability()

    @metered("metered_op")
    def fn(o):
        raise RuntimeError("boom")

    with pytest.raises(RuntimeError):
        fn(obs)
    metrics = obs.metrics.get_metrics()
    assert metrics["proximadb_requests_total"].get(
        "method=metered_op,status=error"
    ) == 1


def test_metered_without_observability_arg():
    @metered("op")
    def fn(n):
        return n

    assert fn(7) == 7  # no obs in args, finally loop finds nothing


def test_metered_obs_without_metrics():
    obs = Observability(enable_metrics=False)

    @metered("op")
    def fn(o):
        return "ok"

    assert fn(obs) == "ok"
