# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Test utilities package for ProximaDB Python SDK tests.

This package provides decoupled test helpers and utilities that can be used
across different test modules without tight SDK coupling.
"""

from .test_helpers import (
    CollectionInfo,
    InsertResult,
    SearchResult,
    ProximaDBTestError,
    assert_proximadb_error,
)

__all__ = [
    "CollectionInfo",
    "InsertResult",
    "SearchResult",
    "ProximaDBTestError",
    "assert_proximadb_error",
]
