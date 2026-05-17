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
    CollectionInfo,
    DiskConfig,
    GraphEdge,
    GraphNode,
    GraphStats,
    ProximaDB,
    SearchResult,
    SearchStreamIterator,
    StorageStats,
    StreamingSearchResult,
    init_logging,
)

__version__ = "0.2.0"
__all__ = [
    "ProximaDB",
    "DiskConfig",
    "SearchResult",
    "StreamingSearchResult",
    "SearchStreamIterator",
    "CollectionInfo",
    "StorageStats",
    "GraphNode",
    "GraphEdge",
    "GraphStats",
    "init_logging",
    "insert_arrow",
    "upsert_arrow",
    "insert_pandas",
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


def _arrow_source_to_ipc_bytes(source) -> bytes:
    """Convert pyarrow/pandas/IPC-byte inputs to Arrow IPC stream bytes."""
    if isinstance(source, bytes):
        return source
    if isinstance(source, bytearray):
        return bytes(source)
    if isinstance(source, memoryview):
        return source.tobytes()

    try:
        import pyarrow as pa
        import pyarrow.ipc as ipc
    except ImportError as exc:
        raise ImportError(
            "insert_arrow/insert_pandas require pyarrow. Install with "
            "`pip install proximadb_embedded[arrow]` or pass Arrow IPC bytes."
        ) from exc

    if isinstance(source, pa.RecordBatch):
        table = pa.Table.from_batches([source])
    elif isinstance(source, pa.Table):
        table = source
    else:
        try:
            table = pa.Table.from_pandas(source, preserve_index=False)
        except Exception as exc:
            raise TypeError(
                "Expected pyarrow.Table, pyarrow.RecordBatch, pandas.DataFrame, "
                "or Arrow IPC bytes"
            ) from exc

    sink = pa.BufferOutputStream()
    with ipc.new_stream(sink, table.schema) as writer:
        writer.write_table(table)
    return sink.getvalue().to_pybytes()


def insert_arrow(
    db: ProximaDB,
    collection: str,
    source,
    *,
    mode: str = "insert",
    tenant_id=None,
) -> int:
    """Insert/upsert a pyarrow Table/RecordBatch, pandas DataFrame, or IPC bytes."""
    return db.insert_arrow_ipc(
        collection,
        _arrow_source_to_ipc_bytes(source),
        mode,
        tenant_id,
    )


def upsert_arrow(
    db: ProximaDB,
    collection: str,
    source,
    *,
    tenant_id=None,
) -> int:
    """Upsert a pyarrow Table/RecordBatch, pandas DataFrame, or IPC bytes."""
    return db.upsert_arrow_ipc(
        collection,
        _arrow_source_to_ipc_bytes(source),
        tenant_id,
    )


def insert_pandas(
    db: ProximaDB,
    collection: str,
    dataframe,
    *,
    mode: str = "insert",
    tenant_id=None,
) -> int:
    """Insert/upsert a pandas DataFrame through the embedded Arrow batch path."""
    return insert_arrow(
        db,
        collection,
        dataframe,
        mode=mode,
        tenant_id=tenant_id,
    )


try:
    ProximaDB.insert_arrow = lambda self, collection, source, **kwargs: insert_arrow(
        self,
        collection,
        source,
        **kwargs,
    )
    ProximaDB.upsert_arrow = lambda self, collection, source, **kwargs: upsert_arrow(
        self,
        collection,
        source,
        **kwargs,
    )
    ProximaDB.insert_pandas = lambda self, collection, dataframe, **kwargs: insert_pandas(
        self,
        collection,
        dataframe,
        **kwargs,
    )
except (AttributeError, TypeError):
    # Some Python extension class builds reject monkey-patching. The module-level
    # helpers above remain available and route to the same native IPC methods.
    pass
