"""
ProximaDB Embedded - In-process vector database for Python

This package provides a zero-overhead embedded mode for ProximaDB,
enabling direct in-process access to the high-performance Rust core
without network communication.

Features:
- Zero network overhead - direct in-process calls
- Multi-disk support with weighted distribution
- SIMD-accelerated vector operations
- Full WAL persistence and crash recovery
- NumPy zero-copy vector transfer

Example:
    >>> from proximadb_embedded import ProximaDB, DiskConfig
    >>> import numpy as np
    >>>
    >>> # Configure multi-disk storage
    >>> disks = [
    ...     DiskConfig("/nvme/data", weight=2),  # Fast SSD
    ...     DiskConfig("/hdd/data", weight=1),   # Slower HDD
    ... ]
    >>>
    >>> # Create embedded database
    >>> db = ProximaDB(data_dirs=disks, cache_size_mb=1024)
    >>>
    >>> # Create collection
    >>> db.create_collection("embeddings", dimension=768)
    >>>
    >>> # Insert vectors (direct NumPy support)
    >>> vectors = np.random.rand(1000, 768).astype(np.float32)
    >>> db.insert("embeddings", ids=[f"v{i}" for i in range(1000)], vectors=vectors)
    >>>
    >>> # Search
    >>> results = db.search("embeddings", query=vectors[0], top_k=10)
"""

from ._proximadb_embedded import (
    ProximaDB,
    DiskConfig,
    SearchResult,
    CollectionInfo,
    StorageStats,
)

__version__ = "0.1.5"
__all__ = [
    "ProximaDB",
    "DiskConfig",
    "SearchResult",
    "CollectionInfo",
    "StorageStats",
    "__version__",
]


def open(
    path: str = "./data",
    cache_size_mb: int = 512,
    default_engine: str = "sst",
) -> ProximaDB:
    """
    Open an embedded ProximaDB database with simple configuration.

    This is a convenience function for quick setup. For multi-disk
    configuration, use the ProximaDB class directly.

    Args:
        path: Path to the data directory
        cache_size_mb: Cache size in megabytes
        default_engine: Default storage engine type

    Returns:
        ProximaDB instance

    Example:
        >>> import proximadb_embedded as pdb
        >>> db = pdb.open("./my_database")
        >>> db.create_collection("vectors", dimension=128)
    """
    return ProximaDB(
        data_dirs=path,
        cache_size_mb=cache_size_mb,
        default_engine=default_engine,
    )
