# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Performance reporting components package.

This package provides modular components for collecting, processing,
and reporting performance data, improving cohesion and maintainability.
"""

from .collectors import BenchmarkDataCollector, EnvironmentCollector
from .processors import MetricsProcessor, ComparisonProcessor
from .reporters import HTMLReporter, ConsoleReporter, JSONReporter

__all__ = [
    "BenchmarkDataCollector",
    "EnvironmentCollector",
    "MetricsProcessor",
    "ComparisonProcessor",
    "HTMLReporter",
    "ConsoleReporter",
    "JSONReporter",
]
