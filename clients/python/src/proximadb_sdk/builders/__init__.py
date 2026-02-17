"""
ProximaDB Builder Patterns

Fluent interfaces for building complex operations and configurations.
"""

from .collection import CollectionBuilder
from .insert import InsertBuilder
from .search import SearchBuilder

__all__ = [
    "SearchBuilder",
    "CollectionBuilder",
    "InsertBuilder",
]
