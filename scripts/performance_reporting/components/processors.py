# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Data processing components for performance reporting.

This module provides processors for analyzing, aggregating, and
comparing benchmark metrics.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any

from .collectors import BenchmarkDataset


class PerformanceRating(str, Enum):
    """Performance rating levels."""

    EXCELLENT = "Excellent"
    GOOD = "Good"
    ACCEPTABLE = "Acceptable"
    NEEDS_IMPROVEMENT = "Needs Improvement"
    UNKNOWN = "Unknown"


class EnterpriseReadiness(str, Enum):
    """Enterprise readiness levels."""

    PRODUCTION_READY = "Production Ready"
    ENTERPRISE_SUITABLE = "Enterprise Suitable"
    DEVELOPMENT_READY = "Development Ready"
    OPTIMIZATION_REQUIRED = "Optimization Required"
    UNKNOWN = "Unknown"


@dataclass
class ExecutiveSummary:
    """Executive-level performance summary."""

    peak_qps: float = 0.0
    average_qps: float = 0.0
    multi_tenant_qps_per_tenant: float = 0.0
    cache_hit_rate: float = 0.0
    sustained_load_stability: str = "Not Tested"
    recommended_production_capacity: int | str = "Unknown"
    performance_rating: PerformanceRating = PerformanceRating.UNKNOWN
    enterprise_readiness: EnterpriseReadiness = EnterpriseReadiness.UNKNOWN

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        return {
            "peak_qps": self.peak_qps,
            "average_qps": self.average_qps,
            "multi_tenant_qps_per_tenant": self.multi_tenant_qps_per_tenant,
            "cache_hit_rate": self.cache_hit_rate,
            "sustained_load_stability": self.sustained_load_stability,
            "recommended_production_capacity": self.recommended_production_capacity,
            "performance_rating": self.performance_rating.value,
            "enterprise_readiness": self.enterprise_readiness.value,
        }


@dataclass
class EngineMetrics:
    """Aggregated metrics for a storage engine."""

    engine_name: str
    peak_qps: float = 0.0
    avg_qps: float = 0.0
    best_latency_p95_ms: float | None = None
    optimal_dataset_size: int | str = "Unknown"
    total_benchmarks: int = 0
    status: PerformanceRating = PerformanceRating.UNKNOWN


@dataclass
class MultiTenantMetrics:
    """Metrics for multi-tenant performance."""

    tenant_count: int = 0
    total_qps: float = 0.0
    avg_qps_per_tenant: float = 0.0
    min_qps_per_tenant: float = 0.0
    max_qps_per_tenant: float = 0.0
    isolation_maintained: bool = False


@dataclass
class ProcessedMetrics:
    """Container for all processed metrics."""

    summary: ExecutiveSummary = field(default_factory=ExecutiveSummary)
    engine_metrics: dict[str, EngineMetrics] = field(default_factory=dict)
    multi_tenant_metrics: list[MultiTenantMetrics] = field(default_factory=list)
    recommendations: list[str] = field(default_factory=list)
    environment_info: dict[str, Any] = field(default_factory=dict)

    @property
    def executive_summary(self) -> ExecutiveSummary:
        """Alias for summary for backwards compatibility."""
        return self.summary


class MetricsProcessor:
    """Processes raw benchmark data into aggregated metrics."""

    # QPS thresholds for performance ratings
    QPS_EXCELLENT_THRESHOLD = 5000
    QPS_GOOD_THRESHOLD = 2000
    QPS_ACCEPTABLE_THRESHOLD = 1000

    # Production safety margin (70%)
    PRODUCTION_SAFETY_MARGIN = 0.7

    def __init__(self, dataset: BenchmarkDataset):
        """
        Initialize the processor.

        Args:
            dataset: Collected benchmark data
        """
        self.dataset = dataset

    def process(self) -> ProcessedMetrics:
        """
        Process all benchmark data.

        Returns:
            ProcessedMetrics containing aggregated results
        """
        result = ProcessedMetrics()
        result.summary = self._generate_executive_summary()
        result.engine_metrics = self._process_engine_metrics()
        result.multi_tenant_metrics = self._process_multi_tenant_metrics()
        result.recommendations = self._generate_recommendations(result.summary)
        return result

    def _generate_executive_summary(self) -> ExecutiveSummary:
        """Generate executive-level performance summary."""
        summary = ExecutiveSummary()

        # Analyze QPS benchmarks
        if self.dataset.qps_benchmarks:
            qps_values = [
                b["qps"] for b in self.dataset.qps_benchmarks if "qps" in b
            ]
            if qps_values:
                summary.peak_qps = max(qps_values)
                summary.average_qps = sum(qps_values) / len(qps_values)

                # Determine performance rating
                summary.performance_rating, summary.enterprise_readiness = (
                    self._rate_performance(summary.peak_qps)
                )

        # Analyze multi-tenant performance
        if self.dataset.multi_tenant_benchmarks:
            avg_per_tenant = [
                b["avg_qps_per_tenant"]
                for b in self.dataset.multi_tenant_benchmarks
                if "avg_qps_per_tenant" in b
            ]
            if avg_per_tenant:
                summary.multi_tenant_qps_per_tenant = sum(avg_per_tenant) / len(
                    avg_per_tenant
                )

        # Analyze cache performance
        if self.dataset.cache_benchmarks:
            cache_rates = [
                b["hit_rate"]
                for b in self.dataset.cache_benchmarks
                if "hit_rate" in b
            ]
            if cache_rates:
                summary.cache_hit_rate = sum(cache_rates) / len(cache_rates)

        # Calculate recommended production capacity
        if summary.average_qps > 0:
            summary.recommended_production_capacity = int(
                summary.peak_qps * self.PRODUCTION_SAFETY_MARGIN
            )

        return summary

    def _rate_performance(
        self, peak_qps: float
    ) -> tuple[PerformanceRating, EnterpriseReadiness]:
        """Determine performance rating based on QPS."""
        if peak_qps > self.QPS_EXCELLENT_THRESHOLD:
            return PerformanceRating.EXCELLENT, EnterpriseReadiness.PRODUCTION_READY
        elif peak_qps > self.QPS_GOOD_THRESHOLD:
            return PerformanceRating.GOOD, EnterpriseReadiness.ENTERPRISE_SUITABLE
        elif peak_qps > self.QPS_ACCEPTABLE_THRESHOLD:
            return PerformanceRating.ACCEPTABLE, EnterpriseReadiness.DEVELOPMENT_READY
        else:
            return (
                PerformanceRating.NEEDS_IMPROVEMENT,
                EnterpriseReadiness.OPTIMIZATION_REQUIRED,
            )

    def _process_engine_metrics(self) -> dict[str, EngineMetrics]:
        """Process QPS benchmarks grouped by storage engine."""
        engine_results: dict[str, list[dict[str, Any]]] = {}

        for result in self.dataset.qps_benchmarks:
            engine = result.get("storage_engine", "Unknown")
            if engine not in engine_results:
                engine_results[engine] = []
            engine_results[engine].append(result)

        processed: dict[str, EngineMetrics] = {}
        for engine, results in engine_results.items():
            qps_values = [r["qps"] for r in results if "qps" in r]
            if not qps_values:
                continue

            metrics = EngineMetrics(
                engine_name=engine,
                peak_qps=max(qps_values),
                avg_qps=sum(qps_values) / len(qps_values),
                total_benchmarks=len(results),
            )

            # Find best latency
            latencies = [
                r.get("p95_latency_ms", float("inf"))
                for r in results
                if "p95_latency_ms" in r
            ]
            if latencies:
                metrics.best_latency_p95_ms = min(latencies)

            # Find optimal dataset size
            best_result = max(results, key=lambda r: r.get("qps", 0))
            metrics.optimal_dataset_size = best_result.get("dataset_size", "Unknown")

            # Determine status
            metrics.status = self._rate_performance(metrics.peak_qps)[0]

            processed[engine] = metrics

        return processed

    def _process_multi_tenant_metrics(self) -> list[MultiTenantMetrics]:
        """Process multi-tenant benchmark data."""
        metrics_list: list[MultiTenantMetrics] = []

        for result in self.dataset.multi_tenant_benchmarks:
            metrics = MultiTenantMetrics(
                tenant_count=result.get("tenant_count", 0),
                total_qps=result.get("total_qps", 0),
                avg_qps_per_tenant=result.get("avg_qps_per_tenant", 0),
                min_qps_per_tenant=result.get("min_qps_per_tenant", 0),
                max_qps_per_tenant=result.get("max_qps_per_tenant", 0),
                isolation_maintained=result.get("isolation_maintained", False),
            )
            metrics_list.append(metrics)

        return metrics_list

    def _generate_recommendations(self, summary: ExecutiveSummary) -> list[str]:
        """Generate performance recommendations based on metrics."""
        recommendations: list[str] = []

        # QPS-based recommendations
        if summary.peak_qps > self.QPS_EXCELLENT_THRESHOLD:
            recommendations.append(
                "Excellent Performance: ProximaDB delivers outstanding query "
                "throughput suitable for high-scale enterprise deployments."
            )
        elif summary.peak_qps > self.QPS_GOOD_THRESHOLD:
            recommendations.append(
                "Enterprise-Ready Performance: Query throughput meets enterprise "
                "requirements for most workloads."
            )
        elif summary.peak_qps > self.QPS_ACCEPTABLE_THRESHOLD:
            recommendations.append(
                "Optimization Opportunity: Performance is acceptable but could "
                "benefit from optimization for large-scale deployments."
            )
        elif summary.peak_qps > 0:
            recommendations.append(
                "Performance Optimization Required: Query throughput below "
                "enterprise expectations - optimization critical."
            )

        # Cache performance recommendations
        if summary.cache_hit_rate > 90:
            recommendations.append(
                "Excellent Cache Performance: Cache hit rate exceeds 90% - "
                "optimal for production workloads."
            )
        elif summary.cache_hit_rate > 80:
            recommendations.append(
                "Good Cache Performance: Cache efficiency is acceptable for "
                "production deployment."
            )
        elif summary.cache_hit_rate > 0:
            recommendations.append(
                "Cache Optimization Needed: Cache hit rate below optimal - "
                "consider cache tuning."
            )
        else:
            recommendations.append(
                "Cache Performance Validation Needed: Execute cache benchmarks "
                "to measure efficiency."
            )

        # Multi-tenant recommendations
        if summary.multi_tenant_qps_per_tenant > 1000:
            recommendations.append(
                "Multi-Tenant Performance Excellent: Per-tenant QPS exceeds 1000 - "
                "meets enterprise SLA requirements."
            )
        elif summary.multi_tenant_qps_per_tenant > 500:
            recommendations.append(
                "Multi-Tenant Performance Good: Per-tenant performance suitable "
                "for most enterprise deployments."
            )
        elif summary.multi_tenant_qps_per_tenant > 0:
            recommendations.append(
                "Multi-Tenant Optimization Needed: Per-tenant QPS below optimal "
                "for enterprise SLAs."
            )
        else:
            recommendations.append(
                "Multi-Tenant Validation Needed: Execute multi-tenant benchmarks "
                "to validate isolation performance."
            )

        # Enterprise readiness assessment
        recommendations.append(
            f"Enterprise Readiness Assessment: {summary.enterprise_readiness.value}"
        )

        # Production capacity recommendation
        if isinstance(summary.recommended_production_capacity, int):
            recommendations.append(
                f"Production Capacity Recommendation: "
                f"{summary.recommended_production_capacity} QPS for production "
                f"deployment with 30% safety margin."
            )

        return recommendations


@dataclass
class ComparisonResult:
    """Result of comparing two benchmark datasets or metrics."""

    baseline_name: str
    comparison_name: str
    metric_name: str
    baseline_value: float
    comparison_value: float
    absolute_diff: float
    percentage_diff: float
    improvement: bool


class ComparisonProcessor:
    """Compares benchmark results across different runs or configurations."""

    def __init__(
        self,
        current: ProcessedMetrics | BenchmarkDataset,
        baseline: ProcessedMetrics | BenchmarkDataset | None = None,
    ):
        """
        Initialize the comparison processor.

        Args:
            current: Current metrics or dataset
            baseline: Optional baseline metrics or dataset for comparison
        """
        self.comparison = (
            current
            if isinstance(current, ProcessedMetrics)
            else MetricsProcessor(current).process()
        )
        self.baseline = (
            baseline
            if baseline is None or isinstance(baseline, ProcessedMetrics)
            else MetricsProcessor(baseline).process()
        )

    def compare_qps(self) -> list[ComparisonResult]:
        """Compare QPS metrics between baseline and comparison."""
        if self.baseline is None:
            return []

        results: list[ComparisonResult] = []

        # Compare peak QPS
        results.append(
            self._compare_metric(
                "Peak QPS",
                self.baseline.summary.peak_qps,
                self.comparison.summary.peak_qps,
            )
        )

        # Compare average QPS
        results.append(
            self._compare_metric(
                "Average QPS",
                self.baseline.summary.average_qps,
                self.comparison.summary.average_qps,
            )
        )

        return results

    def compare_engines(self) -> dict[str, list[ComparisonResult]]:
        """Compare engine-specific metrics."""
        if self.baseline is None:
            return {}

        results: dict[str, list[ComparisonResult]] = {}

        # Find common engines
        common_engines = set(self.baseline.engine_metrics.keys()) & set(
            self.comparison.engine_metrics.keys()
        )

        for engine in common_engines:
            baseline_metrics = self.baseline.engine_metrics[engine]
            comparison_metrics = self.comparison.engine_metrics[engine]

            engine_results: list[ComparisonResult] = []

            engine_results.append(
                self._compare_metric(
                    f"{engine} Peak QPS",
                    baseline_metrics.peak_qps,
                    comparison_metrics.peak_qps,
                )
            )

            engine_results.append(
                self._compare_metric(
                    f"{engine} Avg QPS",
                    baseline_metrics.avg_qps,
                    comparison_metrics.avg_qps,
                )
            )

            if (
                baseline_metrics.best_latency_p95_ms is not None
                and comparison_metrics.best_latency_p95_ms is not None
            ):
                # For latency, lower is better
                engine_results.append(
                    self._compare_metric(
                        f"{engine} P95 Latency",
                        baseline_metrics.best_latency_p95_ms,
                        comparison_metrics.best_latency_p95_ms,
                        lower_is_better=True,
                    )
                )

            results[engine] = engine_results

        return results

    def compare_multi_tenant(self) -> list[ComparisonResult]:
        """Compare multi-tenant metrics."""
        if self.baseline is None:
            return []

        results: list[ComparisonResult] = []

        results.append(
            self._compare_metric(
                "Multi-Tenant QPS/Tenant",
                self.baseline.summary.multi_tenant_qps_per_tenant,
                self.comparison.summary.multi_tenant_qps_per_tenant,
            )
        )

        return results

    def _compare_metric(
        self,
        metric_name: str,
        baseline_value: float,
        comparison_value: float,
        lower_is_better: bool = False,
    ) -> ComparisonResult:
        """Compare a single metric between baseline and comparison."""
        absolute_diff = comparison_value - baseline_value

        if baseline_value != 0:
            percentage_diff = (absolute_diff / baseline_value) * 100
        else:
            percentage_diff = 0.0 if comparison_value == 0 else float("inf")

        # Determine if this is an improvement
        if lower_is_better:
            improvement = comparison_value < baseline_value
        else:
            improvement = comparison_value > baseline_value

        return ComparisonResult(
            baseline_name="Baseline",
            comparison_name="Comparison",
            metric_name=metric_name,
            baseline_value=baseline_value,
            comparison_value=comparison_value,
            absolute_diff=absolute_diff,
            percentage_diff=percentage_diff,
            improvement=improvement,
        )

    def generate_comparison_summary(self) -> dict[str, Any]:
        """Generate a summary of all comparisons."""
        qps_comparisons = self.compare_qps()
        engine_comparisons = self.compare_engines()
        mt_comparisons = self.compare_multi_tenant()

        total_improvements = 0
        total_regressions = 0

        all_results = (
            qps_comparisons
            + mt_comparisons
            + [r for results in engine_comparisons.values() for r in results]
        )

        for result in all_results:
            if result.improvement:
                total_improvements += 1
            elif result.absolute_diff < 0:  # Regression (not just equal)
                total_regressions += 1

        return {
            "qps_comparisons": [
                {
                    "metric": r.metric_name,
                    "baseline": r.baseline_value,
                    "comparison": r.comparison_value,
                    "diff_pct": r.percentage_diff,
                    "improved": r.improvement,
                }
                for r in qps_comparisons
            ],
            "engine_comparisons": {
                engine: [
                    {
                        "metric": r.metric_name,
                        "baseline": r.baseline_value,
                        "comparison": r.comparison_value,
                        "diff_pct": r.percentage_diff,
                        "improved": r.improvement,
                    }
                    for r in results
                ]
                for engine, results in engine_comparisons.items()
            },
            "multi_tenant_comparisons": [
                {
                    "metric": r.metric_name,
                    "baseline": r.baseline_value,
                    "comparison": r.comparison_value,
                    "diff_pct": r.percentage_diff,
                    "improved": r.improvement,
                }
                for r in mt_comparisons
            ],
            "summary": {
                "total_metrics_compared": len(all_results),
                "improvements": total_improvements,
                "regressions": total_regressions,
                "unchanged": len(all_results) - total_improvements - total_regressions,
            },
        }
