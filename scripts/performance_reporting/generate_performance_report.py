#!/usr/bin/env python3
# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Performance Report Generator for ProximaDB

Orchestrates the generation of comprehensive performance reports from benchmark
results using modular components for data collection, processing, and reporting.

This script follows the design specified in task_3_performance_validation_design.adoc
and uses the component-based architecture for improved maintainability.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import TYPE_CHECKING

# Import modular components
from components import (
    BenchmarkDataCollector,
    ComparisonProcessor,
    ConsoleReporter,
    EnvironmentCollector,
    HTMLReporter,
    JSONReporter,
    MetricsProcessor,
)

if TYPE_CHECKING:
    from components.collectors import BenchmarkDataset
    from components.processors import ProcessedMetrics


class PerformanceReportOrchestrator:
    """
    Orchestrates the performance report generation workflow.

    This class coordinates the data collection, processing, and reporting
    phases using the modular component architecture.
    """

    def __init__(
        self,
        results_dir: str | Path,
        verbose: bool = False,
        include_environment: bool = True,
    ):
        """
        Initialize the orchestrator.

        Args:
            results_dir: Directory containing benchmark result files
            verbose: Enable verbose output during processing
            include_environment: Include environment information in reports
        """
        self.results_dir = Path(results_dir)
        self.verbose = verbose
        self.include_environment = include_environment

        # Initialize components
        self._data_collector = BenchmarkDataCollector(self.results_dir)
        self._env_collector = EnvironmentCollector()

        # Lazy-initialized after data collection
        self._dataset: BenchmarkDataset | None = None
        self._metrics: ProcessedMetrics | None = None

    def collect_data(self) -> "BenchmarkDataset":
        """
        Collect benchmark data from the results directory.

        Returns:
            BenchmarkDataset containing all collected benchmark results
        """
        if self._log("Collecting benchmark data..."):
            pass

        self._dataset = self._data_collector.collect()

        # Log collection results
        if self._dataset.is_empty():
            self._log("No benchmark data found in results directory.")
        else:
            self._log(f"Collected {self._dataset.total_results()} benchmark results:")
            if self._dataset.qps_benchmarks:
                self._log(f"  - QPS benchmarks: {len(self._dataset.qps_benchmarks)}")
            if self._dataset.multi_tenant_benchmarks:
                self._log(
                    f"  - Multi-tenant benchmarks: {len(self._dataset.multi_tenant_benchmarks)}"
                )
            if self._dataset.cache_benchmarks:
                self._log(f"  - Cache benchmarks: {len(self._dataset.cache_benchmarks)}")
            if self._dataset.sustained_load_results:
                self._log(
                    f"  - Sustained load results: {len(self._dataset.sustained_load_results)}"
                )
            if self._dataset.engine_benchmarks:
                self._log(
                    f"  - Engine benchmarks: {len(self._dataset.engine_benchmarks)}"
                )

        # Log any collection errors
        for error in self._data_collector.errors:
            self._log(f"Warning: {error}")

        return self._dataset

    def process_metrics(self) -> "ProcessedMetrics":
        """
        Process collected data into aggregated metrics.

        Returns:
            ProcessedMetrics containing analyzed benchmark results

        Raises:
            RuntimeError: If data has not been collected yet
        """
        if self._dataset is None:
            raise RuntimeError("Must call collect_data() before process_metrics()")

        self._log("Processing metrics...")

        processor = MetricsProcessor(self._dataset)
        self._metrics = processor.process()

        # Log processing results
        if self._metrics.executive_summary:
            summary = self._metrics.executive_summary
            self._log(f"Performance rating: {summary.performance_rating.value}")
            self._log(f"Enterprise readiness: {summary.enterprise_readiness.value}")
            if summary.peak_qps > 0:
                self._log(f"Peak QPS: {summary.peak_qps:.0f}")

        return self._metrics

    def generate_comparison(self, baseline_dir: str | Path | None = None) -> dict:
        """
        Generate comparison metrics against a baseline.

        Args:
            baseline_dir: Optional directory containing baseline benchmark results

        Returns:
            Dictionary containing comparison results
        """
        if self._dataset is None:
            raise RuntimeError("Must call collect_data() before generate_comparison()")

        comparison_processor = ComparisonProcessor(self._dataset)

        if baseline_dir:
            baseline_collector = BenchmarkDataCollector(baseline_dir)
            baseline_dataset = baseline_collector.collect()
            comparison_processor = ComparisonProcessor(self._dataset, baseline_dataset)

        return {
            "qps_comparison": [r.__dict__ for r in comparison_processor.compare_qps()],
            "engine_comparison": {
                engine: [r.__dict__ for r in results]
                for engine, results in comparison_processor.compare_engines().items()
            },
        }

    def generate_report(
        self,
        output_path: str | Path,
        format: str = "html",
        title: str | None = None,
    ) -> None:
        """
        Generate a performance report in the specified format.

        Args:
            output_path: Path for the output report file
            format: Report format ('html', 'json', 'console')
            title: Optional custom title for the report
        """
        if self._metrics is None:
            raise RuntimeError("Must call process_metrics() before generate_report()")

        output_path = Path(output_path)
        self._log(f"Generating {format.upper()} report...")

        # Add environment info if requested
        if self.include_environment:
            env_info = self._env_collector.collect()
            self._metrics.environment_info = env_info.to_dict()

        # Select and use appropriate reporter
        if format == "html":
            reporter = HTMLReporter(
                title=title or "ProximaDB Performance Validation Report"
            )
            reporter.generate(self._metrics, output_path)
        elif format == "json":
            reporter = JSONReporter(pretty=True)
            reporter.generate(self._metrics, output_path)
        elif format == "console":
            reporter = ConsoleReporter(use_colors=True)
            reporter.generate(self._metrics, sys.stdout)
        else:
            raise ValueError(f"Unsupported format: {format}")

        if format != "console":
            self._log(f"Report generated: {output_path}")

    def run_full_pipeline(
        self,
        output_path: str | Path,
        format: str = "html",
        title: str | None = None,
    ) -> "ProcessedMetrics":
        """
        Run the complete report generation pipeline.

        This is a convenience method that runs all phases in sequence:
        1. Collect data
        2. Process metrics
        3. Generate report

        Args:
            output_path: Path for the output report file
            format: Report format ('html', 'json', 'console')
            title: Optional custom title for the report

        Returns:
            ProcessedMetrics from the processing phase
        """
        self.collect_data()
        self.process_metrics()
        self.generate_report(output_path, format, title)
        return self._metrics

    def get_execution_summary(self) -> str:
        """
        Get a summary of what benchmarks were executed.

        Returns:
            Human-readable summary string
        """
        if self._dataset is None:
            return "No benchmark data collected yet."

        if self._dataset.is_empty():
            return (
                "No benchmark results found - execute performance validation "
                "benchmarks to generate metrics."
            )

        lines = ["Benchmark Execution Status:"]

        if self._dataset.qps_benchmarks:
            lines.append(f"  QPS Benchmarks: {len(self._dataset.qps_benchmarks)} results")

        if self._dataset.multi_tenant_benchmarks:
            lines.append(
                f"  Multi-Tenant Benchmarks: {len(self._dataset.multi_tenant_benchmarks)} results"
            )

        if self._dataset.cache_benchmarks:
            lines.append(
                f"  Cache Benchmarks: {len(self._dataset.cache_benchmarks)} results"
            )

        if self._dataset.sustained_load_results:
            lines.append(
                f"  Sustained Load Results: {len(self._dataset.sustained_load_results)} results"
            )

        if self._dataset.engine_benchmarks:
            lines.append(
                f"  Engine Benchmarks: {len(self._dataset.engine_benchmarks)} results"
            )

        return "\n".join(lines)

    def _log(self, message: str) -> bool:
        """Log a message if verbose mode is enabled."""
        if self.verbose:
            print(message)
        return True


def main():
    """Main entry point for the performance report generator."""
    parser = argparse.ArgumentParser(
        description="Generate ProximaDB performance validation report",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s --results-dir benchmark_results --output report.html
  %(prog)s --results-dir benchmark_results --format json --output metrics.json
  %(prog)s --results-dir benchmark_results --format console
        """,
    )
    parser.add_argument(
        "--results-dir",
        default="benchmark_results",
        help="Directory containing benchmark results (default: benchmark_results)",
    )
    parser.add_argument(
        "--output",
        default="performance_report.html",
        help="Output file path (default: performance_report.html)",
    )
    parser.add_argument(
        "--format",
        choices=["html", "json", "console"],
        default="html",
        help="Output format (default: html)",
    )
    parser.add_argument(
        "--title",
        help="Custom title for the report",
    )
    parser.add_argument(
        "--baseline-dir",
        help="Directory containing baseline benchmark results for comparison",
    )
    parser.add_argument(
        "--no-environment",
        action="store_true",
        help="Exclude environment information from the report",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Enable verbose output",
    )

    args = parser.parse_args()

    print("ProximaDB Performance Report Generator")
    print("=" * 40)

    # Check if results directory exists
    results_path = Path(args.results_dir)
    if not results_path.exists():
        print(f"\nWarning: Results directory not found: {args.results_dir}")
        print("\nTo generate benchmark data, run:")
        print("  cargo bench --bench comprehensive_qps_benchmark")
        print("  cargo bench --bench multi_tenant_isolation_benchmark")
        return 1

    # Create orchestrator and run pipeline
    orchestrator = PerformanceReportOrchestrator(
        results_dir=args.results_dir,
        verbose=args.verbose,
        include_environment=not args.no_environment,
    )

    try:
        # Run the full pipeline
        metrics = orchestrator.run_full_pipeline(
            output_path=args.output,
            format=args.format,
            title=args.title,
        )

        # Print summary
        print(f"\n{orchestrator.get_execution_summary()}")

        if args.format != "console":
            print(f"\nReport generated: {args.output}")

        # Print comparison if baseline provided
        if args.baseline_dir:
            comparison = orchestrator.generate_comparison(args.baseline_dir)
            print(f"\nComparison with baseline: {len(comparison['qps_comparison'])} QPS comparisons")

        return 0

    except Exception as e:
        print(f"\nError generating report: {e}")
        if args.verbose:
            import traceback

            traceback.print_exc()
        return 1


if __name__ == "__main__":
    sys.exit(main())
