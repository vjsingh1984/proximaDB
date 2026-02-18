# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Report generation components for performance reporting.

This module provides reporters for outputting performance data in
various formats (HTML, console, JSON).
"""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from datetime import datetime
from pathlib import Path
from typing import Any, TextIO

from .collectors import BenchmarkDataset, EnvironmentInfo
from .processors import (
    EngineMetrics,
    ExecutiveSummary,
    MultiTenantMetrics,
    PerformanceRating,
    ProcessedMetrics,
)


class BaseReporter(ABC):
    """Abstract base class for reporters."""

    @abstractmethod
    def generate(self, metrics: ProcessedMetrics, output: str | Path | TextIO) -> None:
        """
        Generate a report from processed metrics.

        Args:
            metrics: Processed benchmark metrics
            output: Output destination (file path or stream)
        """
        pass


class HTMLReporter(BaseReporter):
    """Generates HTML performance reports."""

    def __init__(
        self,
        title: str = "ProximaDB Performance Validation Report",
        environment: EnvironmentInfo | None = None,
    ):
        """
        Initialize the HTML reporter.

        Args:
            title: Report title
            environment: Optional environment information
        """
        self.title = title
        self.environment = environment

    def generate(self, metrics: ProcessedMetrics, output: str | Path | TextIO) -> None:
        """Generate HTML report."""
        html_content = self._build_html(metrics)

        if isinstance(output, (str, Path)):
            with open(output, "w") as f:
                f.write(html_content)
        else:
            output.write(html_content)

    def _build_html(self, metrics: ProcessedMetrics) -> str:
        """Build complete HTML document."""
        summary = metrics.summary
        timestamp = datetime.now()

        html = f"""<!DOCTYPE html>
<html>
<head>
    <title>{self.title}</title>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
{self._get_styles()}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>{self.title}</h1>
            <p>Comprehensive Performance Analysis | Generated on {timestamp.strftime('%B %d, %Y at %H:%M UTC')}</p>
        </div>

{self._build_summary_section(summary)}
{self._build_engine_section(metrics.engine_metrics)}
{self._build_multi_tenant_section(metrics.multi_tenant_metrics)}
{self._build_recommendations_section(metrics.recommendations)}

        <div class="footer">
            <p>ProximaDB Performance Validation Report | {timestamp.strftime('%Y-%m-%d %H:%M:%S UTC')}</p>
            <p>Enterprise Readiness: {summary.enterprise_readiness.value}</p>
        </div>
    </div>
</body>
</html>"""
        return html

    def _get_styles(self) -> str:
        """Return CSS styles for the report."""
        return """        body {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            margin: 0;
            padding: 20px;
            background-color: #f5f5f5;
        }
        .container {
            max-width: 1200px;
            margin: 0 auto;
            background: white;
            border-radius: 8px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
        }
        .header {
            background: linear-gradient(135deg, #1e88e5, #42a5f5);
            color: white;
            padding: 30px;
            text-align: center;
            border-radius: 8px 8px 0 0;
        }
        .header h1 {
            margin: 0;
            font-size: 2.5em;
            font-weight: 300;
        }
        .header p {
            margin: 10px 0 0 0;
            opacity: 0.9;
        }
        .summary {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 20px;
            padding: 30px;
            background: #f8f9fa;
        }
        .metric {
            text-align: center;
            padding: 20px;
            background: white;
            border-radius: 6px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }
        .metric-value {
            font-size: 2.5em;
            font-weight: bold;
            color: #1e88e5;
            margin: 0;
        }
        .metric-label {
            font-size: 0.9em;
            color: #666;
            margin: 10px 0 0 0;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }
        .section {
            margin: 0;
            padding: 30px;
            border-top: 1px solid #eee;
        }
        .section h2 {
            color: #333;
            margin: 0 0 20px 0;
            font-size: 1.8em;
            font-weight: 400;
        }
        .benchmark-table {
            width: 100%;
            border-collapse: collapse;
            margin: 20px 0;
            font-size: 0.9em;
        }
        .benchmark-table th {
            background: #1e88e5;
            color: white;
            padding: 12px;
            text-align: left;
            font-weight: 500;
        }
        .benchmark-table td {
            border: 1px solid #ddd;
            padding: 12px;
            text-align: left;
        }
        .benchmark-table tr:nth-child(even) {
            background: #f8f9fa;
        }
        .status-excellent { color: #4caf50; font-weight: bold; }
        .status-good { color: #2196f3; font-weight: bold; }
        .status-acceptable { color: #ff9800; font-weight: bold; }
        .status-needs-improvement { color: #f44336; font-weight: bold; }
        .footer {
            background: #333;
            color: white;
            text-align: center;
            padding: 20px;
            border-radius: 0 0 8px 8px;
        }
        .recommendations ul {
            line-height: 1.8;
            padding-left: 20px;
        }
        .recommendations li {
            margin: 10px 0;
        }"""

    def _build_summary_section(self, summary: ExecutiveSummary) -> str:
        """Build the executive summary section."""
        rating_class = f"status-{summary.performance_rating.value.lower().replace(' ', '-')}"

        return f"""        <div class="summary">
            <div class="metric">
                <div class="metric-value">{summary.peak_qps:.0f}</div>
                <div class="metric-label">Peak QPS</div>
            </div>
            <div class="metric">
                <div class="metric-value">{summary.average_qps:.0f}</div>
                <div class="metric-label">Average QPS</div>
            </div>
            <div class="metric">
                <div class="metric-value">{summary.multi_tenant_qps_per_tenant:.0f}</div>
                <div class="metric-label">QPS per Tenant</div>
            </div>
            <div class="metric">
                <div class="metric-value">{summary.cache_hit_rate:.1f}%</div>
                <div class="metric-label">Cache Hit Rate</div>
            </div>
            <div class="metric">
                <div class="metric-value">{summary.recommended_production_capacity}</div>
                <div class="metric-label">Production QPS<br>Recommendation</div>
            </div>
            <div class="metric">
                <div class="metric-value {rating_class}">{summary.performance_rating.value}</div>
                <div class="metric-label">Performance Rating</div>
            </div>
        </div>"""

    def _build_engine_section(self, engine_metrics: dict[str, EngineMetrics]) -> str:
        """Build the storage engine performance section."""
        if not engine_metrics:
            return """        <div class="section">
            <h2>QPS Performance by Storage Engine</h2>
            <p>No storage engine benchmark data available.</p>
        </div>"""

        rows = ""
        for engine, metrics in engine_metrics.items():
            status_class = f"status-{metrics.status.value.lower().replace(' ', '-')}"
            latency = (
                f"{metrics.best_latency_p95_ms:.1f}ms"
                if metrics.best_latency_p95_ms is not None
                else "N/A"
            )

            rows += f"""                <tr>
                    <td>{engine}</td>
                    <td>{metrics.peak_qps:.0f}</td>
                    <td>{metrics.avg_qps:.0f}</td>
                    <td>{latency}</td>
                    <td>{metrics.optimal_dataset_size:,}</td>
                    <td><span class="{status_class}">{metrics.status.value}</span></td>
                </tr>
"""

        return f"""        <div class="section">
            <h2>QPS Performance by Storage Engine</h2>
            <table class="benchmark-table">
                <tr>
                    <th>Storage Engine</th>
                    <th>Peak QPS</th>
                    <th>Avg QPS</th>
                    <th>Best Latency (P95)</th>
                    <th>Optimal Dataset Size</th>
                    <th>Status</th>
                </tr>
{rows}            </table>
        </div>"""

    def _build_multi_tenant_section(
        self, multi_tenant_metrics: list[MultiTenantMetrics]
    ) -> str:
        """Build the multi-tenant performance section."""
        if not multi_tenant_metrics:
            return """        <div class="section">
            <h2>Multi-Tenant Performance</h2>
            <p>No multi-tenant benchmark data available.</p>
        </div>"""

        rows = ""
        for metrics in multi_tenant_metrics:
            isolation_status = (
                '<span class="status-excellent">Maintained</span>'
                if metrics.isolation_maintained
                else '<span class="status-needs-improvement">Violated</span>'
            )

            rows += f"""                <tr>
                    <td>{metrics.tenant_count}</td>
                    <td>{metrics.total_qps:.0f}</td>
                    <td>{metrics.avg_qps_per_tenant:.0f}</td>
                    <td>{metrics.min_qps_per_tenant:.0f}</td>
                    <td>{metrics.max_qps_per_tenant:.0f}</td>
                    <td>{isolation_status}</td>
                </tr>
"""

        return f"""        <div class="section">
            <h2>Multi-Tenant Performance Isolation</h2>
            <table class="benchmark-table">
                <tr>
                    <th>Tenant Count</th>
                    <th>Total QPS</th>
                    <th>Avg QPS/Tenant</th>
                    <th>Min QPS/Tenant</th>
                    <th>Max QPS/Tenant</th>
                    <th>Isolation Status</th>
                </tr>
{rows}            </table>
        </div>"""

    def _build_recommendations_section(self, recommendations: list[str]) -> str:
        """Build the recommendations section."""
        if not recommendations:
            return """        <div class="section recommendations">
            <h2>Performance Recommendations</h2>
            <p>No recommendations available.</p>
        </div>"""

        items = "\n".join(f"                <li>{rec}</li>" for rec in recommendations)

        return f"""        <div class="section recommendations">
            <h2>Performance Recommendations</h2>
            <ul>
{items}
            </ul>
        </div>"""


class ConsoleReporter(BaseReporter):
    """Generates console-friendly text reports."""

    def __init__(self, use_colors: bool = True, width: int = 80):
        """
        Initialize the console reporter.

        Args:
            use_colors: Whether to use ANSI color codes
            width: Maximum line width
        """
        self.use_colors = use_colors
        self.width = width

    def generate(self, metrics: ProcessedMetrics, output: str | Path | TextIO) -> None:
        """Generate console report."""
        report = self._build_report(metrics)

        if isinstance(output, (str, Path)):
            with open(output, "w") as f:
                f.write(report)
        else:
            output.write(report)

    def _build_report(self, metrics: ProcessedMetrics) -> str:
        """Build complete text report."""
        lines = []
        summary = metrics.summary

        # Header
        lines.append(self._header("PROXIMADB PERFORMANCE REPORT"))
        lines.append(f"Generated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        lines.append("")

        # Executive Summary
        lines.append(self._header("EXECUTIVE SUMMARY"))
        lines.append(f"  Peak QPS:              {summary.peak_qps:,.0f}")
        lines.append(f"  Average QPS:           {summary.average_qps:,.0f}")
        lines.append(f"  Multi-Tenant QPS:      {summary.multi_tenant_qps_per_tenant:,.0f} per tenant")
        lines.append(f"  Cache Hit Rate:        {summary.cache_hit_rate:.1f}%")
        lines.append(f"  Performance Rating:    {self._colorize(summary.performance_rating)}")
        lines.append(f"  Enterprise Readiness:  {summary.enterprise_readiness.value}")
        lines.append(f"  Production Capacity:   {summary.recommended_production_capacity} QPS")
        lines.append("")

        # Engine Performance
        if metrics.engine_metrics:
            lines.append(self._header("STORAGE ENGINE PERFORMANCE"))
            lines.append(
                f"{'Engine':<15} {'Peak QPS':>10} {'Avg QPS':>10} {'P95 (ms)':>10} {'Status':<15}"
            )
            lines.append("-" * 60)
            for engine, m in metrics.engine_metrics.items():
                latency = (
                    f"{m.best_latency_p95_ms:.1f}" if m.best_latency_p95_ms else "N/A"
                )
                status = self._colorize(m.status)
                lines.append(
                    f"{engine:<15} {m.peak_qps:>10,.0f} {m.avg_qps:>10,.0f} {latency:>10} {status:<15}"
                )
            lines.append("")

        # Multi-Tenant
        if metrics.multi_tenant_metrics:
            lines.append(self._header("MULTI-TENANT PERFORMANCE"))
            lines.append(
                f"{'Tenants':>8} {'Total QPS':>12} {'Avg/Tenant':>12} {'Isolation':<15}"
            )
            lines.append("-" * 50)
            for m in metrics.multi_tenant_metrics:
                isolation = self._color("green", "OK") if m.isolation_maintained else self._color("red", "VIOLATED")
                lines.append(
                    f"{m.tenant_count:>8} {m.total_qps:>12,.0f} {m.avg_qps_per_tenant:>12,.0f} {isolation:<15}"
                )
            lines.append("")

        # Recommendations
        if metrics.recommendations:
            lines.append(self._header("RECOMMENDATIONS"))
            for i, rec in enumerate(metrics.recommendations, 1):
                lines.append(f"  {i}. {rec}")
            lines.append("")

        lines.append("=" * self.width)
        return "\n".join(lines)

    def _header(self, text: str) -> str:
        """Create a header line."""
        return f"\n{'=' * self.width}\n{text:^{self.width}}\n{'=' * self.width}"

    def _colorize(self, rating: PerformanceRating) -> str:
        """Apply color to rating based on level."""
        colors = {
            PerformanceRating.EXCELLENT: "green",
            PerformanceRating.GOOD: "blue",
            PerformanceRating.ACCEPTABLE: "yellow",
            PerformanceRating.NEEDS_IMPROVEMENT: "red",
            PerformanceRating.UNKNOWN: "white",
        }
        return self._color(colors.get(rating, "white"), rating.value)

    def _color(self, color: str, text: str) -> str:
        """Apply ANSI color code if colors enabled."""
        if not self.use_colors:
            return text

        codes = {
            "green": "\033[92m",
            "blue": "\033[94m",
            "yellow": "\033[93m",
            "red": "\033[91m",
            "white": "\033[97m",
        }
        reset = "\033[0m"
        return f"{codes.get(color, '')}{text}{reset}"


class JSONReporter(BaseReporter):
    """Generates JSON performance reports."""

    def __init__(self, pretty: bool = True):
        """
        Initialize the JSON reporter.

        Args:
            pretty: Whether to pretty-print JSON
        """
        self.pretty = pretty

    def generate(self, metrics: ProcessedMetrics, output: str | Path | TextIO) -> None:
        """Generate JSON report."""
        data = self._to_dict(metrics)

        if isinstance(output, (str, Path)):
            with open(output, "w") as f:
                json.dump(data, f, indent=2 if self.pretty else None, default=str)
        else:
            json.dump(data, output, indent=2 if self.pretty else None, default=str)

    def _to_dict(self, metrics: ProcessedMetrics) -> dict[str, Any]:
        """Convert metrics to dictionary."""
        return {
            "generated_at": datetime.now().isoformat(),
            "summary": metrics.summary.to_dict(),
            "engine_metrics": {
                name: {
                    "engine_name": m.engine_name,
                    "peak_qps": m.peak_qps,
                    "avg_qps": m.avg_qps,
                    "best_latency_p95_ms": m.best_latency_p95_ms,
                    "optimal_dataset_size": m.optimal_dataset_size,
                    "total_benchmarks": m.total_benchmarks,
                    "status": m.status.value,
                }
                for name, m in metrics.engine_metrics.items()
            },
            "multi_tenant_metrics": [
                {
                    "tenant_count": m.tenant_count,
                    "total_qps": m.total_qps,
                    "avg_qps_per_tenant": m.avg_qps_per_tenant,
                    "min_qps_per_tenant": m.min_qps_per_tenant,
                    "max_qps_per_tenant": m.max_qps_per_tenant,
                    "isolation_maintained": m.isolation_maintained,
                }
                for m in metrics.multi_tenant_metrics
            ],
            "recommendations": metrics.recommendations,
        }
