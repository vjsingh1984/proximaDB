# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Data collection components for performance reporting.

This module provides collectors for gathering benchmark data and
environment information for performance analysis.
"""

from __future__ import annotations

import glob
import json
import os
import platform
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class BenchmarkDataset:
    """Container for collected benchmark data."""

    qps_benchmarks: list[dict[str, Any]] = field(default_factory=list)
    multi_tenant_benchmarks: list[dict[str, Any]] = field(default_factory=list)
    cache_benchmarks: list[dict[str, Any]] = field(default_factory=list)
    sustained_load_results: list[dict[str, Any]] = field(default_factory=list)
    engine_benchmarks: list[dict[str, Any]] = field(default_factory=list)
    custom_benchmarks: dict[str, list[dict[str, Any]]] = field(default_factory=dict)

    def is_empty(self) -> bool:
        """Check if no benchmark data has been collected."""
        return (
            not self.qps_benchmarks
            and not self.multi_tenant_benchmarks
            and not self.cache_benchmarks
            and not self.sustained_load_results
            and not self.engine_benchmarks
            and not self.custom_benchmarks
        )

    def total_results(self) -> int:
        """Count total number of benchmark results."""
        custom_count = sum(len(v) for v in self.custom_benchmarks.values())
        return (
            len(self.qps_benchmarks)
            + len(self.multi_tenant_benchmarks)
            + len(self.cache_benchmarks)
            + len(self.sustained_load_results)
            + len(self.engine_benchmarks)
            + custom_count
        )


class BenchmarkDataCollector:
    """Collects benchmark data from various file sources."""

    def __init__(self, results_dir: str | Path):
        """
        Initialize the collector.

        Args:
            results_dir: Directory containing benchmark result files
        """
        self.results_dir = Path(results_dir)
        self._errors: list[str] = []

    @property
    def errors(self) -> list[str]:
        """Get list of errors encountered during collection."""
        return self._errors.copy()

    def collect(self) -> BenchmarkDataset:
        """
        Collect all benchmark data from the results directory.

        Returns:
            BenchmarkDataset containing all collected data
        """
        self._errors.clear()
        dataset = BenchmarkDataset()

        if not self.results_dir.exists():
            self._errors.append(f"Results directory not found: {self.results_dir}")
            return dataset

        # Collect each type of benchmark
        dataset.qps_benchmarks = self._collect_pattern("qps_*.json")
        dataset.multi_tenant_benchmarks = self._collect_pattern("multi_tenant_*.json")
        dataset.cache_benchmarks = self._collect_pattern("cache_*.json")
        dataset.sustained_load_results = self._collect_pattern("sustained_*.json")
        dataset.engine_benchmarks = self._collect_pattern("engine_*.json")

        return dataset

    def collect_pattern(self, pattern: str, category: str | None = None) -> list[dict[str, Any]]:
        """
        Collect benchmark data matching a specific pattern.

        Args:
            pattern: Glob pattern for matching files
            category: Optional category name for custom benchmarks

        Returns:
            List of benchmark results
        """
        return self._collect_pattern(pattern)

    def _collect_pattern(self, pattern: str) -> list[dict[str, Any]]:
        """Collect data from files matching pattern."""
        results: list[dict[str, Any]] = []
        search_pattern = str(self.results_dir / pattern)

        try:
            files = glob.glob(search_pattern)
            for file_path in files:
                try:
                    with open(file_path, "r") as f:
                        data = json.load(f)
                        if isinstance(data, list):
                            results.extend(data)
                        else:
                            results.append(data)
                except json.JSONDecodeError as e:
                    self._errors.append(f"Invalid JSON in {file_path}: {e}")
                except OSError as e:
                    self._errors.append(f"Error reading {file_path}: {e}")
        except Exception as e:
            self._errors.append(f"Error collecting pattern {pattern}: {e}")

        return results

    def collect_from_file(self, file_path: str | Path) -> list[dict[str, Any]]:
        """
        Collect benchmark data from a specific file.

        Args:
            file_path: Path to the benchmark file

        Returns:
            List of benchmark results from the file
        """
        file_path = Path(file_path)
        results: list[dict[str, Any]] = []

        try:
            with open(file_path, "r") as f:
                data = json.load(f)
                if isinstance(data, list):
                    results.extend(data)
                else:
                    results.append(data)
        except json.JSONDecodeError as e:
            self._errors.append(f"Invalid JSON in {file_path}: {e}")
        except OSError as e:
            self._errors.append(f"Error reading {file_path}: {e}")

        return results


@dataclass
class EnvironmentInfo:
    """Container for environment information."""

    platform: str = ""
    python_version: str = ""
    os_name: str = ""
    os_version: str = ""
    architecture: str = ""
    processor: str = ""
    cpu_count: int = 0
    memory_gb: float = 0.0
    hostname: str = ""
    rust_version: str | None = None
    proximadb_version: str | None = None
    custom_info: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for serialization."""
        result = {
            "platform": self.platform,
            "python_version": self.python_version,
            "os_name": self.os_name,
            "os_version": self.os_version,
            "architecture": self.architecture,
            "processor": self.processor,
            "cpu_count": self.cpu_count,
            "memory_gb": self.memory_gb,
            "hostname": self.hostname,
        }
        if self.rust_version:
            result["rust_version"] = self.rust_version
        if self.proximadb_version:
            result["proximadb_version"] = self.proximadb_version
        if self.custom_info:
            result.update(self.custom_info)
        return result


class EnvironmentCollector:
    """Collects environment and system information."""

    def collect(self) -> EnvironmentInfo:
        """
        Collect current environment information.

        Returns:
            EnvironmentInfo with system details
        """
        info = EnvironmentInfo(
            platform=platform.platform(),
            python_version=sys.version,
            os_name=platform.system(),
            os_version=platform.release(),
            architecture=platform.machine(),
            processor=platform.processor(),
            cpu_count=os.cpu_count() or 0,
            hostname=platform.node(),
        )

        # Try to get memory info
        info.memory_gb = self._get_memory_gb()

        # Try to get Rust version
        info.rust_version = self._get_rust_version()

        # Try to get ProximaDB version
        info.proximadb_version = self._get_proximadb_version()

        return info

    def _get_memory_gb(self) -> float:
        """Get total system memory in GB."""
        try:
            import resource

            # Try to get memory from resource module (Unix)
            mem_bytes = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
            # On macOS, ru_maxrss is in bytes; on Linux, it's in KB
            if platform.system() == "Darwin":
                return mem_bytes / (1024**3)
            return mem_bytes / (1024**2)
        except (ImportError, AttributeError):
            pass

        try:
            # Fallback: try psutil if available
            import psutil

            return psutil.virtual_memory().total / (1024**3)
        except ImportError:
            return 0.0

    def _get_rust_version(self) -> str | None:
        """Get Rust compiler version if available."""
        import subprocess

        try:
            result = subprocess.run(
                ["rustc", "--version"],
                capture_output=True,
                text=True,
                timeout=5,
            )
            if result.returncode == 0:
                return result.stdout.strip()
        except (subprocess.SubprocessError, FileNotFoundError):
            pass
        return None

    def _get_proximadb_version(self) -> str | None:
        """Get ProximaDB version if available."""
        try:
            # Try importing from SDK
            from proximadb_sdk import __version__

            return __version__
        except Exception:
            # Handle ImportError, protobuf version errors, etc.
            pass

        try:
            # Try reading from Cargo.toml
            cargo_path = Path(__file__).parent.parent.parent.parent / "Cargo.toml"
            if cargo_path.exists():
                with open(cargo_path, "r") as f:
                    for line in f:
                        if line.startswith("version"):
                            return line.split("=")[1].strip().strip('"')
        except Exception:
            pass

        return None

    def add_custom_info(self, info: EnvironmentInfo, key: str, value: Any) -> None:
        """Add custom information to environment info."""
        info.custom_info[key] = value
