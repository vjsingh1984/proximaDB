"""Compatibility shim for legacy embedded imports.

Canonical embedded builds should import `proximadb_embedded`.
This alias exists only to keep older local benchmarks and examples working
while the repo migrates to the explicit embedded package name.
"""

from proximadb_embedded import (  # noqa: F401
    CollectionInfo,
    DiskConfig,
    ProximaDB,
    SearchResult,
    StorageStats,
    __version__,
    open,
)

__all__ = [
    "ProximaDB",
    "DiskConfig",
    "SearchResult",
    "CollectionInfo",
    "StorageStats",
    "__version__",
    "open",
]
