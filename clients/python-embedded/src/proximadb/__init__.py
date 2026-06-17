"""Compatibility shim for legacy embedded imports.

Canonical embedded builds should import `proximadb_embedded`.
This alias exists only to keep older local benchmarks and examples working
while the repo migrates to the explicit embedded package name.
"""

from proximadb_embedded import (  # noqa: F401
    CollectionInfo,
    DiskConfig,
    ProximaRecord,
    ProximaDB,
    ProximaValue,
    SearchResult,
    StorageStats,
    __version__,
    insert_proxima_records,
    insert_records,
    open,
    proxima_value,
    upsert_proxima_records,
    upsert_records,
)

__all__ = [
    "ProximaDB",
    "DiskConfig",
    "ProximaRecord",
    "ProximaValue",
    "SearchResult",
    "CollectionInfo",
    "StorageStats",
    "insert_records",
    "insert_proxima_records",
    "upsert_records",
    "upsert_proxima_records",
    "proxima_value",
    "__version__",
    "open",
]
