# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Standalone performance data models for ProximaDB.

This module provides Pydantic models for performance metrics, benchmark results,
and validation reporting. These models are intentionally decoupled from the core
SDK models to allow performance scripts to operate independently.
"""

from __future__ import annotations

from datetime import datetime
from enum import Enum
from typing import Any

from pydantic import BaseModel, Field


class ValidationStatus(str, Enum):
    """Status of a validation check."""

    PASS = "pass"
    WARN = "warn"
    FAIL = "fail"
    SKIP = "skip"


class LatencyStats(BaseModel):
    """Latency statistics for an operation."""

    min_ms: float = Field(description="Minimum latency in milliseconds")
    max_ms: float = Field(description="Maximum latency in milliseconds")
    avg_ms: float = Field(description="Average latency in milliseconds")
    p50_ms: float = Field(default=0.0, description="50th percentile latency")
    p95_ms: float = Field(default=0.0, description="95th percentile latency")
    p99_ms: float = Field(default=0.0, description="99th percentile latency")
    std_dev_ms: float = Field(default=0.0, description="Standard deviation")

    @classmethod
    def from_samples(cls, samples_ms: list[float]) -> "LatencyStats":
        """Create LatencyStats from a list of latency samples."""
        if not samples_ms:
            return cls(min_ms=0, max_ms=0, avg_ms=0)

        import statistics

        sorted_samples = sorted(samples_ms)
        n = len(sorted_samples)

        return cls(
            min_ms=min(sorted_samples),
            max_ms=max(sorted_samples),
            avg_ms=statistics.mean(sorted_samples),
            p50_ms=sorted_samples[int(n * 0.5)] if n > 0 else 0,
            p95_ms=sorted_samples[int(n * 0.95)] if n > 0 else 0,
            p99_ms=sorted_samples[int(n * 0.99)] if n > 0 else 0,
            std_dev_ms=statistics.stdev(sorted_samples) if n > 1 else 0,
        )


class ThroughputMetrics(BaseModel):
    """Throughput metrics for operations."""

    operations_per_second: float = Field(description="Operations completed per second")
    vectors_per_second: float = Field(default=0.0, description="Vectors processed per second")
    bytes_per_second: float = Field(default=0.0, description="Bytes processed per second")
    total_operations: int = Field(default=0, description="Total number of operations")
    total_duration_ms: float = Field(default=0.0, description="Total duration in milliseconds")


class MemoryMetrics(BaseModel):
    """Memory usage metrics."""

    peak_memory_mb: float = Field(description="Peak memory usage in MB")
    avg_memory_mb: float = Field(default=0.0, description="Average memory usage in MB")
    memory_delta_mb: float = Field(default=0.0, description="Memory change during operation")
    gc_collections: int = Field(default=0, description="Number of garbage collections")


class BenchmarkMetrics(BaseModel):
    """Combined metrics for a benchmark operation."""

    latency: LatencyStats = Field(description="Latency statistics")
    throughput: ThroughputMetrics | None = Field(default=None, description="Throughput metrics")
    memory: MemoryMetrics | None = Field(default=None, description="Memory metrics")
    recall: float | None = Field(default=None, description="Recall rate for search operations")
    precision: float | None = Field(default=None, description="Precision for search operations")


class EnginePerformance(BaseModel):
    """Performance metrics for a specific storage engine."""

    engine_name: str = Field(description="Name of the storage engine")
    insert_metrics: BenchmarkMetrics | None = Field(
        default=None, description="Insert operation metrics"
    )
    search_metrics: BenchmarkMetrics | None = Field(
        default=None, description="Search operation metrics"
    )
    update_metrics: BenchmarkMetrics | None = Field(
        default=None, description="Update operation metrics"
    )
    delete_metrics: BenchmarkMetrics | None = Field(
        default=None, description="Delete operation metrics"
    )
    flush_time_ms: float | None = Field(default=None, description="Flush duration in ms")
    compaction_time_ms: float | None = Field(default=None, description="Compaction duration in ms")
    storage_size_mb: float | None = Field(default=None, description="Storage size in MB")


class BenchmarkResult(BaseModel):
    """Result of a benchmark run."""

    benchmark_name: str = Field(description="Name of the benchmark")
    timestamp: datetime = Field(default_factory=datetime.now, description="When benchmark ran")
    duration_seconds: float = Field(description="Total benchmark duration")
    vector_count: int = Field(description="Number of vectors tested")
    dimension: int = Field(description="Vector dimension")
    engine_results: list[EnginePerformance] = Field(
        default_factory=list, description="Per-engine results"
    )
    metadata: dict[str, Any] = Field(default_factory=dict, description="Additional metadata")
    success: bool = Field(default=True, description="Whether benchmark completed successfully")
    error_message: str | None = Field(default=None, description="Error message if failed")


class ValidationResult(BaseModel):
    """Result of a validation check."""

    check_name: str = Field(description="Name of the validation check")
    status: ValidationStatus = Field(description="Validation status")
    message: str = Field(description="Human-readable result message")
    expected_value: Any | None = Field(default=None, description="Expected value")
    actual_value: Any | None = Field(default=None, description="Actual value")
    threshold: float | None = Field(default=None, description="Threshold for pass/fail")
    details: dict[str, Any] = Field(default_factory=dict, description="Additional details")


class PerformanceSummary(BaseModel):
    """Summary of performance metrics across multiple runs or engines."""

    total_vectors_tested: int = Field(description="Total vectors across all tests")
    total_queries_executed: int = Field(default=0, description="Total search queries")
    avg_insert_latency_ms: float = Field(description="Average insert latency")
    avg_search_latency_ms: float = Field(description="Average search latency")
    best_engine_insert: str | None = Field(default=None, description="Best engine for inserts")
    best_engine_search: str | None = Field(default=None, description="Best engine for search")
    overall_recall: float | None = Field(default=None, description="Overall recall rate")
    recommendations: list[str] = Field(
        default_factory=list, description="Performance recommendations"
    )


class PerformanceReport(BaseModel):
    """Complete performance report with all metrics and validations."""

    report_id: str = Field(description="Unique report identifier")
    generated_at: datetime = Field(
        default_factory=datetime.now, description="Report generation time"
    )
    environment: dict[str, Any] = Field(
        default_factory=dict, description="Environment information"
    )
    benchmark_results: list[BenchmarkResult] = Field(
        default_factory=list, description="All benchmark results"
    )
    validation_results: list[ValidationResult] = Field(
        default_factory=list, description="All validation results"
    )
    summary: PerformanceSummary | None = Field(default=None, description="Performance summary")
    competitor_comparison: dict[str, Any] = Field(
        default_factory=dict, description="Comparison with competitor databases"
    )


# Helper functions for creating common metric combinations


def create_latency_stats(
    min_ms: float, max_ms: float, avg_ms: float, p99_ms: float | None = None
) -> LatencyStats:
    """Create LatencyStats with common parameters."""
    return LatencyStats(
        min_ms=min_ms,
        max_ms=max_ms,
        avg_ms=avg_ms,
        p50_ms=avg_ms,  # Approximate
        p95_ms=p99_ms or max_ms,
        p99_ms=p99_ms or max_ms,
    )


def create_throughput_metrics(
    ops_per_sec: float, total_ops: int, duration_ms: float
) -> ThroughputMetrics:
    """Create ThroughputMetrics with common parameters."""
    return ThroughputMetrics(
        operations_per_second=ops_per_sec,
        total_operations=total_ops,
        total_duration_ms=duration_ms,
    )


def create_validation_result(
    check_name: str,
    actual: Any,
    expected: Any,
    threshold: float | None = None,
    comparator: str = ">=",
) -> ValidationResult:
    """Create ValidationResult with automatic status determination."""
    if threshold is not None and isinstance(actual, (int, float)):
        if comparator == ">=":
            passed = actual >= threshold
        elif comparator == "<=":
            passed = actual <= threshold
        elif comparator == "==":
            passed = actual == threshold
        else:
            passed = True
        status = ValidationStatus.PASS if passed else ValidationStatus.FAIL
    else:
        status = ValidationStatus.PASS if actual == expected else ValidationStatus.FAIL

    return ValidationResult(
        check_name=check_name,
        status=status,
        message=f"{check_name}: actual={actual}, expected={expected}",
        expected_value=expected,
        actual_value=actual,
        threshold=threshold,
    )
