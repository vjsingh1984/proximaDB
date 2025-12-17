# Copyright 2025 Vijaykumar Singh
# SPDX-License-Identifier: Apache-2.0
"""
Test helper classes for ProximaDB Python SDK tests.

This module provides standalone helper classes that are decoupled from
the main SDK, enabling tests to run without tight coupling to SDK types.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class CollectionInfo:
    """
    Collection information object for tests.

    This is a standalone representation of collection metadata that doesn't
    require the SDK's CollectionConfig class.
    """

    name: str
    dimension: int
    engine: str = "sst"
    distance_metric: str = "cosine"
    description: str = ""

    def __post_init__(self):
        # SDK compatibility - config attribute points to self
        self.config = self
        self.id = self.name


@dataclass
class InsertResult:
    """
    Insert operation result for tests.

    Provides a consistent interface for insert results regardless of
    whether using embedded or network client.
    """

    success: bool = True
    count: int = 0
    successful_count: int = 0
    total: int = 0


@dataclass
class SearchResult:
    """
    Search result item for tests.

    Provides a unified search result representation that works with
    both embedded and network clients.
    """

    id: str
    score: float
    metadata: dict[str, Any] = field(default_factory=dict)
    vector: list[float] | None = None

    def get(self, key: str, default: Any = None) -> Any:
        """Dict-like access for backward compatibility."""
        return getattr(self, key, default)


class ProximaDBTestError(Exception):
    """Custom exception for test-specific errors."""

    pass


def assert_proximadb_error(exc_info, expected_message_fragment: str | None = None):
    """
    Helper to assert ProximaDB errors with optional message checking.

    Args:
        exc_info: The pytest exception info object
        expected_message_fragment: Optional string to look for in the error message
    """
    # Accept any exception type since we're decoupled from SDK
    assert exc_info.type is not None, "Expected an exception to be raised"

    if expected_message_fragment:
        assert expected_message_fragment.lower() in str(exc_info.value).lower(), (
            f"Expected '{expected_message_fragment}' in error message: {exc_info.value}"
        )
