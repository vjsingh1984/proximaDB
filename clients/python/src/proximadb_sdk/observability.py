"""
Observability Module for ProximaDB Python SDK

Provides OpenTelemetry integration, Prometheus metrics export,
distributed tracing, and structured logging.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import time
import logging
import threading
from contextlib import contextmanager
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional, TypeVar, Union
from functools import wraps

# Type variable for decorator
F = TypeVar("F", bound=Callable[..., Any])


class MetricType(str, Enum):
    """Types of metrics"""

    COUNTER = "counter"
    GAUGE = "gauge"
    HISTOGRAM = "histogram"
    SUMMARY = "summary"


class LogLevel(str, Enum):
    """Log levels"""

    DEBUG = "debug"
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


@dataclass
class MetricDefinition:
    """Definition for a metric"""

    name: str
    metric_type: MetricType
    description: str
    labels: List[str] = field(default_factory=list)
    buckets: Optional[List[float]] = None  # For histograms


@dataclass
class SpanContext:
    """Distributed tracing span context"""

    trace_id: str
    span_id: str
    parent_span_id: Optional[str] = None
    baggage: Dict[str, str] = field(default_factory=dict)
    sampled: bool = True

    def to_headers(self) -> Dict[str, str]:
        """Convert to HTTP headers for propagation"""
        return {
            "traceparent": f"00-{self.trace_id}-{self.span_id}-{'01' if self.sampled else '00'}",
            "tracestate": ",".join(f"{k}={v}" for k, v in self.baggage.items()),
        }

    @classmethod
    def from_headers(cls, headers: Dict[str, str]) -> Optional["SpanContext"]:
        """Extract span context from HTTP headers"""
        traceparent = headers.get("traceparent")
        if not traceparent:
            return None

        parts = traceparent.split("-")
        if len(parts) != 4:
            return None

        baggage = {}
        tracestate = headers.get("tracestate", "")
        if tracestate:
            for item in tracestate.split(","):
                if "=" in item:
                    k, v = item.split("=", 1)
                    baggage[k.strip()] = v.strip()

        return cls(
            trace_id=parts[1],
            span_id=parts[2],
            sampled=parts[3] == "01",
            baggage=baggage,
        )


@dataclass
class Span:
    """Represents a trace span"""

    name: str
    context: SpanContext
    start_time_ns: int
    end_time_ns: Optional[int] = None
    status: str = "ok"
    attributes: Dict[str, Any] = field(default_factory=dict)
    events: List[Dict[str, Any]] = field(default_factory=list)

    def set_attribute(self, key: str, value: Any) -> None:
        """Set a span attribute"""
        self.attributes[key] = value

    def add_event(self, name: str, attributes: Optional[Dict[str, Any]] = None) -> None:
        """Add an event to the span"""
        self.events.append(
            {
                "name": name,
                "timestamp_ns": time.time_ns(),
                "attributes": attributes or {},
            }
        )

    def set_status(self, status: str, message: Optional[str] = None) -> None:
        """Set span status"""
        self.status = status
        if message:
            self.attributes["status.message"] = message

    def end(self) -> None:
        """End the span"""
        self.end_time_ns = time.time_ns()

    @property
    def duration_ms(self) -> float:
        """Get span duration in milliseconds"""
        if self.end_time_ns is None:
            return 0
        return (self.end_time_ns - self.start_time_ns) / 1_000_000


class MetricsCollector:
    """
    Collects and exports metrics.

    Supports Prometheus-compatible metrics export.
    """

    def __init__(self, prefix: str = "proximadb"):
        self._prefix = prefix
        self._metrics: Dict[str, MetricDefinition] = {}
        self._values: Dict[str, Any] = {}
        self._lock = threading.Lock()

        # Register default SDK metrics
        self._register_default_metrics()

    def _register_default_metrics(self) -> None:
        """Register default ProximaDB SDK metrics"""
        self.register(
            MetricDefinition(
                name="requests_total",
                metric_type=MetricType.COUNTER,
                description="Total number of requests",
                labels=["method", "status"],
            )
        )
        self.register(
            MetricDefinition(
                name="request_duration_seconds",
                metric_type=MetricType.HISTOGRAM,
                description="Request duration in seconds",
                labels=["method"],
                buckets=[
                    0.001,
                    0.005,
                    0.01,
                    0.025,
                    0.05,
                    0.1,
                    0.25,
                    0.5,
                    1.0,
                    2.5,
                    5.0,
                    10.0,
                ],
            )
        )
        self.register(
            MetricDefinition(
                name="vectors_inserted_total",
                metric_type=MetricType.COUNTER,
                description="Total vectors inserted",
                labels=["collection"],
            )
        )
        self.register(
            MetricDefinition(
                name="search_results_total",
                metric_type=MetricType.COUNTER,
                description="Total search results returned",
                labels=["collection"],
            )
        )
        self.register(
            MetricDefinition(
                name="active_connections",
                metric_type=MetricType.GAUGE,
                description="Number of active connections",
                labels=["protocol"],
            )
        )
        self.register(
            MetricDefinition(
                name="cache_hits_total",
                metric_type=MetricType.COUNTER,
                description="Total cache hits",
                labels=["cache_type"],
            )
        )
        self.register(
            MetricDefinition(
                name="cache_misses_total",
                metric_type=MetricType.COUNTER,
                description="Total cache misses",
                labels=["cache_type"],
            )
        )

    def register(self, metric: MetricDefinition) -> None:
        """Register a metric definition"""
        full_name = f"{self._prefix}_{metric.name}"
        self._metrics[full_name] = metric
        self._values[full_name] = {}

    def inc(
        self, name: str, value: float = 1, labels: Optional[Dict[str, str]] = None
    ) -> None:
        """Increment a counter"""
        full_name = f"{self._prefix}_{name}"
        if full_name not in self._metrics:
            return

        label_key = self._label_key(labels)
        with self._lock:
            if label_key not in self._values[full_name]:
                self._values[full_name][label_key] = 0
            self._values[full_name][label_key] += value

    def set(
        self, name: str, value: float, labels: Optional[Dict[str, str]] = None
    ) -> None:
        """Set a gauge value"""
        full_name = f"{self._prefix}_{name}"
        if full_name not in self._metrics:
            return

        label_key = self._label_key(labels)
        with self._lock:
            self._values[full_name][label_key] = value

    def observe(
        self, name: str, value: float, labels: Optional[Dict[str, str]] = None
    ) -> None:
        """Observe a histogram value"""
        full_name = f"{self._prefix}_{name}"
        if full_name not in self._metrics:
            return

        metric = self._metrics[full_name]
        label_key = self._label_key(labels)

        with self._lock:
            if label_key not in self._values[full_name]:
                self._values[full_name][label_key] = {
                    "count": 0,
                    "sum": 0,
                    "buckets": {b: 0 for b in (metric.buckets or [])},
                }

            entry = self._values[full_name][label_key]
            entry["count"] += 1
            entry["sum"] += value

            if metric.buckets:
                for bucket in metric.buckets:
                    if value <= bucket:
                        entry["buckets"][bucket] += 1

    def _label_key(self, labels: Optional[Dict[str, str]]) -> str:
        """Generate a key from labels"""
        if not labels:
            return ""
        return ",".join(f"{k}={v}" for k, v in sorted(labels.items()))

    def export_prometheus(self) -> str:
        """Export metrics in Prometheus format"""
        lines = []

        with self._lock:
            for full_name, metric in self._metrics.items():
                lines.append(f"# HELP {full_name} {metric.description}")
                lines.append(f"# TYPE {full_name} {metric.metric_type.value}")

                for label_key, value in self._values.get(full_name, {}).items():
                    labels_str = f"{{{label_key}}}" if label_key else ""

                    if metric.metric_type == MetricType.HISTOGRAM:
                        # Export histogram buckets
                        for bucket, count in value.get("buckets", {}).items():
                            lines.append(
                                f'{full_name}_bucket{{le="{bucket}"{", " + label_key if label_key else ""}}} {count}'
                            )
                        lines.append(
                            f'{full_name}_bucket{{le="+Inf"{", " + label_key if label_key else ""}}} {value.get("count", 0)}'
                        )
                        lines.append(
                            f'{full_name}_sum{labels_str} {value.get("sum", 0)}'
                        )
                        lines.append(
                            f'{full_name}_count{labels_str} {value.get("count", 0)}'
                        )
                    else:
                        lines.append(f"{full_name}{labels_str} {value}")

        return "\n".join(lines)

    def get_metrics(self) -> Dict[str, Any]:
        """Get all metrics as dictionary"""
        with self._lock:
            return {name: dict(values) for name, values in self._values.items()}


class Tracer:
    """
    Distributed tracer for ProximaDB operations.

    Compatible with OpenTelemetry trace format.
    """

    def __init__(self, service_name: str = "proximadb-python-sdk"):
        self._service_name = service_name
        self._spans: List[Span] = []
        self._current_span: Optional[Span] = None
        self._lock = threading.Lock()
        self._span_processors: List[Callable[[Span], None]] = []

    def add_span_processor(self, processor: Callable[[Span], None]) -> None:
        """Add a span processor for export"""
        self._span_processors.append(processor)

    def start_span(
        self,
        name: str,
        parent: Optional[SpanContext] = None,
        attributes: Optional[Dict[str, Any]] = None,
    ) -> Span:
        """Start a new span"""
        import uuid

        if parent:
            context = SpanContext(
                trace_id=parent.trace_id,
                span_id=uuid.uuid4().hex[:16],
                parent_span_id=parent.span_id,
                baggage=parent.baggage.copy(),
                sampled=parent.sampled,
            )
        else:
            context = SpanContext(
                trace_id=uuid.uuid4().hex,
                span_id=uuid.uuid4().hex[:16],
            )

        span = Span(
            name=name,
            context=context,
            start_time_ns=time.time_ns(),
            attributes=attributes or {},
        )

        # Add service name
        span.attributes["service.name"] = self._service_name

        with self._lock:
            self._current_span = span

        return span

    def end_span(self, span: Span) -> None:
        """End a span and export it"""
        span.end()

        with self._lock:
            self._spans.append(span)
            if self._current_span == span:
                self._current_span = None

        # Process span
        for processor in self._span_processors:
            try:
                processor(span)
            except Exception:
                pass

    @contextmanager
    def trace(
        self,
        name: str,
        parent: Optional[SpanContext] = None,
        attributes: Optional[Dict[str, Any]] = None,
    ):
        """Context manager for tracing"""
        span = self.start_span(name, parent, attributes)
        try:
            yield span
        except Exception as e:
            span.set_status("error", str(e))
            raise
        finally:
            self.end_span(span)

    def get_current_span(self) -> Optional[Span]:
        """Get the current active span"""
        with self._lock:
            return self._current_span

    def get_spans(self) -> List[Span]:
        """Get all recorded spans"""
        with self._lock:
            return list(self._spans)

    def clear(self) -> None:
        """Clear recorded spans"""
        with self._lock:
            self._spans.clear()

    def export_otlp(self) -> List[Dict[str, Any]]:
        """Export spans in OTLP-compatible format"""
        with self._lock:
            return [
                {
                    "traceId": span.context.trace_id,
                    "spanId": span.context.span_id,
                    "parentSpanId": span.context.parent_span_id,
                    "name": span.name,
                    "startTimeUnixNano": span.start_time_ns,
                    "endTimeUnixNano": span.end_time_ns,
                    "status": {"code": 1 if span.status == "ok" else 2},
                    "attributes": [
                        {"key": k, "value": {"stringValue": str(v)}}
                        for k, v in span.attributes.items()
                    ],
                    "events": [
                        {
                            "name": e["name"],
                            "timeUnixNano": e["timestamp_ns"],
                            "attributes": [
                                {"key": k, "value": {"stringValue": str(v)}}
                                for k, v in e.get("attributes", {}).items()
                            ],
                        }
                        for e in span.events
                    ],
                }
                for span in self._spans
            ]


class StructuredLogger:
    """
    Structured JSON logger for ProximaDB operations.

    Provides consistent log format with context propagation.
    """

    def __init__(
        self,
        name: str = "proximadb",
        level: LogLevel = LogLevel.INFO,
        json_format: bool = True,
    ):
        self._name = name
        self._level = level
        self._json_format = json_format
        self._context: Dict[str, Any] = {}
        self._handlers: List[Callable[[Dict[str, Any]], None]] = []

        # Set up Python logging
        self._logger = logging.getLogger(name)
        self._logger.setLevel(getattr(logging, level.value.upper()))

    def add_handler(self, handler: Callable[[Dict[str, Any]], None]) -> None:
        """Add a log handler"""
        self._handlers.append(handler)

    def with_context(self, **context) -> "StructuredLogger":
        """Create a logger with additional context"""
        new_logger = StructuredLogger(
            self._name,
            self._level,
            self._json_format,
        )
        new_logger._context = {**self._context, **context}
        new_logger._handlers = self._handlers
        return new_logger

    def _log(self, level: LogLevel, message: str, **kwargs) -> None:
        """Internal log method"""
        import json as json_module

        entry = {
            "timestamp": time.time(),
            "level": level.value,
            "logger": self._name,
            "message": message,
            **self._context,
            **kwargs,
        }

        # Call handlers
        for handler in self._handlers:
            try:
                handler(entry)
            except Exception:
                pass

        # Log to Python logger
        if self._json_format:
            log_msg = json_module.dumps(entry)
        else:
            extra = " ".join(f"{k}={v}" for k, v in kwargs.items())
            log_msg = f"{message} {extra}" if extra else message

        log_level = getattr(logging, level.value.upper())
        self._logger.log(log_level, log_msg)

    def debug(self, message: str, **kwargs) -> None:
        """Log debug message"""
        self._log(LogLevel.DEBUG, message, **kwargs)

    def info(self, message: str, **kwargs) -> None:
        """Log info message"""
        self._log(LogLevel.INFO, message, **kwargs)

    def warning(self, message: str, **kwargs) -> None:
        """Log warning message"""
        self._log(LogLevel.WARNING, message, **kwargs)

    def error(self, message: str, **kwargs) -> None:
        """Log error message"""
        self._log(LogLevel.ERROR, message, **kwargs)

    def critical(self, message: str, **kwargs) -> None:
        """Log critical message"""
        self._log(LogLevel.CRITICAL, message, **kwargs)


class Observability:
    """
    Unified observability for ProximaDB SDK.

    Combines metrics, tracing, and logging with automatic
    instrumentation of client operations.

    Example:
        >>> from proximadb_sdk import ProximaDBClient
        >>> from proximadb_sdk.observability import Observability
        >>>
        >>> client = ProximaDBClient("http://localhost:5678")
        >>> obs = Observability(client)
        >>>
        >>> # Metrics are collected automatically
        >>> results = client.search("collection", [0.1, 0.2, ...], top_k=10)
        >>>
        >>> # Export Prometheus metrics
        >>> print(obs.metrics.export_prometheus())
        >>>
        >>> # Get trace data
        >>> spans = obs.tracer.get_spans()
    """

    def __init__(
        self,
        client=None,
        service_name: str = "proximadb-python-sdk",
        enable_metrics: bool = True,
        enable_tracing: bool = True,
        enable_logging: bool = True,
    ):
        """
        Initialize observability.

        Args:
            client: ProximaDBClient to instrument (optional)
            service_name: Service name for tracing
            enable_metrics: Enable metrics collection
            enable_tracing: Enable distributed tracing
            enable_logging: Enable structured logging
        """
        self._client = client
        self._service_name = service_name

        self.metrics = MetricsCollector() if enable_metrics else None
        self.tracer = Tracer(service_name) if enable_tracing else None
        self.logger = StructuredLogger(service_name) if enable_logging else None

        if client:
            self._instrument_client(client)

    def _instrument_client(self, client) -> None:
        """Instrument a ProximaDB client"""
        # Wrap key methods
        methods_to_instrument = [
            ("search", "search"),
            ("insert_vectors", "insert"),
            ("get_vector", "get"),
            ("delete_vector", "delete"),
            ("create_collection", "create_collection"),
        ]

        for method_name, operation in methods_to_instrument:
            if hasattr(client, method_name):
                original = getattr(client, method_name)
                wrapped = self._wrap_method(original, operation)
                setattr(client, method_name, wrapped)

    def _wrap_method(self, method: Callable, operation: str) -> Callable:
        """Wrap a method with observability"""

        @wraps(method)
        def wrapper(*args, **kwargs):
            start_time = time.time()
            status = "success"

            # Start trace span
            span = None
            if self.tracer:
                span = self.tracer.start_span(
                    f"proximadb.{operation}",
                    attributes={"operation": operation},
                )

            try:
                result = method(*args, **kwargs)

                # Record metrics on success
                if self.metrics:
                    duration = time.time() - start_time
                    self.metrics.inc(
                        "requests_total",
                        labels={"method": operation, "status": "success"},
                    )
                    self.metrics.observe(
                        "request_duration_seconds",
                        duration,
                        labels={"method": operation},
                    )

                return result

            except Exception as e:
                status = "error"
                if span:
                    span.set_status("error", str(e))

                if self.metrics:
                    self.metrics.inc(
                        "requests_total",
                        labels={"method": operation, "status": "error"},
                    )

                if self.logger:
                    self.logger.error(
                        f"Operation failed: {operation}",
                        operation=operation,
                        error=str(e),
                    )

                raise

            finally:
                if span and self.tracer:
                    self.tracer.end_span(span)

        return wrapper

    def instrument(self, method: Callable, operation: str) -> Callable:
        """
        Decorator to instrument a method.

        Args:
            method: Method to instrument
            operation: Operation name for metrics/traces

        Example:
            >>> @obs.instrument
            ... def my_operation():
            ...     pass
        """
        return self._wrap_method(method, operation)

    @contextmanager
    def trace_operation(self, name: str, **attributes):
        """
        Context manager for tracing custom operations.

        Example:
            >>> with obs.trace_operation("my_operation", user_id="123"):
            ...     # Do work
            ...     pass
        """
        if self.tracer:
            with self.tracer.trace(name, attributes=attributes) as span:
                yield span
        else:
            yield None

    def record_search(
        self,
        collection: str,
        result_count: int,
        duration_ms: float,
    ) -> None:
        """Record search operation metrics"""
        if self.metrics:
            self.metrics.inc(
                "search_results_total", result_count, labels={"collection": collection}
            )
            self.metrics.observe(
                "request_duration_seconds",
                duration_ms / 1000,
                labels={"method": "search"},
            )

    def record_insert(
        self,
        collection: str,
        vector_count: int,
        duration_ms: float,
    ) -> None:
        """Record insert operation metrics"""
        if self.metrics:
            self.metrics.inc(
                "vectors_inserted_total",
                vector_count,
                labels={"collection": collection},
            )
            self.metrics.observe(
                "request_duration_seconds",
                duration_ms / 1000,
                labels={"method": "insert"},
            )

    def record_cache_hit(self, cache_type: str = "query") -> None:
        """Record a cache hit"""
        if self.metrics:
            self.metrics.inc("cache_hits_total", labels={"cache_type": cache_type})

    def record_cache_miss(self, cache_type: str = "query") -> None:
        """Record a cache miss"""
        if self.metrics:
            self.metrics.inc("cache_misses_total", labels={"cache_type": cache_type})

    def get_prometheus_metrics(self) -> str:
        """Get metrics in Prometheus format"""
        if self.metrics:
            return self.metrics.export_prometheus()
        return ""

    def get_traces(self) -> List[Dict[str, Any]]:
        """Get traces in OTLP format"""
        if self.tracer:
            return self.tracer.export_otlp()
        return []


def traced(operation: str):
    """
    Decorator for tracing functions.

    Example:
        >>> @traced("my_operation")
        ... def my_function():
        ...     pass
    """

    def decorator(func: F) -> F:
        @wraps(func)
        def wrapper(*args, **kwargs):
            # Get tracer from first arg if it's an Observability instance
            obs = None
            for arg in args:
                if isinstance(arg, Observability):
                    obs = arg
                    break

            if obs and obs.tracer:
                with obs.tracer.trace(operation):
                    return func(*args, **kwargs)
            else:
                return func(*args, **kwargs)

        return wrapper  # type: ignore

    return decorator


def metered(operation: str):
    """
    Decorator for metering functions.

    Example:
        >>> @metered("my_operation")
        ... def my_function():
        ...     pass
    """

    def decorator(func: F) -> F:
        @wraps(func)
        def wrapper(*args, **kwargs):
            start = time.time()
            status = "success"

            try:
                result = func(*args, **kwargs)
                return result
            except Exception:
                status = "error"
                raise
            finally:
                # Find observability in args
                for arg in args:
                    if isinstance(arg, Observability) and arg.metrics:
                        duration = time.time() - start
                        arg.metrics.inc(
                            "requests_total",
                            labels={"method": operation, "status": status},
                        )
                        arg.metrics.observe(
                            "request_duration_seconds",
                            duration,
                            labels={"method": operation},
                        )
                        break

        return wrapper  # type: ignore

    return decorator


__all__ = [
    # Main classes
    "Observability",
    "MetricsCollector",
    "Tracer",
    "StructuredLogger",
    # Data classes
    "MetricDefinition",
    "SpanContext",
    "Span",
    # Enums
    "MetricType",
    "LogLevel",
    # Decorators
    "traced",
    "metered",
]
