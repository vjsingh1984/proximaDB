"""
Embedded ProximaDB client adapter

Provides a client-like interface to the embedded ProximaDB module,
allowing tests written for the SDK client to work with embedded mode.

Note: The SDK is now named `proximadb_sdk` to avoid conflict with the
native `proximadb` module (PyO3/maturin-based embedded database).
"""

import tempfile
import shutil
import uuid
from typing import Optional, List, Dict, Any, Union
from pathlib import Path
from dataclasses import dataclass, field
import numpy as np

# Import native ProximaDB module directly
# SDK is now proximadb_sdk, so no namespace conflict
try:
    from proximadb import ProximaDB
except ImportError as e:
    raise ImportError(
        "Native ProximaDB module not found. "
        "Install with: pip install target/wheels/proximadb-*.whl"
    ) from e


@dataclass
class InsertResponse:
    """Response from insert operation"""

    success: bool = True
    inserted: int = 0
    errors: List[str] = field(default_factory=list)


@dataclass
class SearchResult:
    """Search result item"""

    id: str
    score: float
    metadata: Dict[str, Any] = field(default_factory=dict)
    vector: Optional[List[float]] = None


@dataclass
class CollectionInfo:
    """Collection information"""

    name: str
    dimension: int
    engine: str = "sst"
    vector_count: int = 0


class EmbeddedClientAdapter:
    """
    Adapter that wraps embedded ProximaDB with a client-like interface.

    This allows tests written for ProximaDBClient to work with
    the embedded database without modification.
    """

    def __init__(
        self,
        data_dir: Optional[str] = None,
        cache_size_mb: int = 256,
        default_engine: str = "sst",
    ):
        """
        Initialize embedded client adapter.

        Args:
            data_dir: Data directory (temp dir created if None)
            cache_size_mb: Cache size in MB
            default_engine: Default storage engine
        """
        self._temp_dir = None
        if data_dir is None:
            self._temp_dir = tempfile.mkdtemp(prefix="proximadb_adapter_")
            data_dir = self._temp_dir

        self._db = ProximaDB(
            data_dirs=data_dir,
            cache_size_mb=cache_size_mb,
            default_engine=default_engine,
            enable_wal=True,
        )
        self._default_engine = default_engine

    def close(self):
        """Close the embedded database and clean up"""
        if self._db is not None:
            try:
                self._db.flush()
            except Exception:
                pass
            self._db = None

        if self._temp_dir and Path(self._temp_dir).exists():
            try:
                shutil.rmtree(self._temp_dir)
            except Exception:
                pass

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()

    # Collection operations

    def create_collection(
        self, name: str, dimension: int, engine: Optional[str] = None, **kwargs
    ) -> CollectionInfo:
        """Create a collection"""
        engine = engine or self._default_engine
        self._db.create_collection(name, dimension, engine)
        return CollectionInfo(name=name, dimension=dimension, engine=engine)

    def delete_collection(self, name: str) -> bool:
        """Delete a collection"""
        try:
            self._db.delete_collection(name)
            return True
        except Exception:
            return False

    def get_collection(self, name: str) -> Optional[CollectionInfo]:
        """Get collection info"""
        try:
            info = self._db.get_collection(name)
            if info:
                return CollectionInfo(
                    name=info.name if hasattr(info, "name") else name,
                    dimension=info.dimension if hasattr(info, "dimension") else 0,
                    engine=info.engine if hasattr(info, "engine") else "sst",
                    vector_count=(
                        info.vector_count if hasattr(info, "vector_count") else 0
                    ),
                )
            return None
        except Exception:
            return None

    def list_collections(self) -> List[CollectionInfo]:
        """List all collections"""
        try:
            collections = self._db.list_collections()
            return [
                CollectionInfo(
                    name=c.name if hasattr(c, "name") else str(c),
                    dimension=c.dimension if hasattr(c, "dimension") else 0,
                    engine=c.engine if hasattr(c, "engine") else "sst",
                    vector_count=c.vector_count if hasattr(c, "vector_count") else 0,
                )
                for c in collections
            ]
        except Exception:
            return []

    # Vector operations

    def insert_vectors(
        self,
        collection_id: str,
        records: Optional[List] = None,
        vectors: Optional[List] = None,
        ids: Optional[List[str]] = None,
        metadata: Optional[List[Dict]] = None,
        **kwargs,
    ) -> InsertResponse:
        """
        Insert vectors into a collection.

        Supports both record-based and array-based insertion.
        """
        try:
            # Handle record-based input (from VectorRecord objects)
            if records is not None:
                ids = [r.id for r in records]
                vectors = [
                    (
                        np.array(r.vector, dtype=np.float32)
                        if not isinstance(r.vector, np.ndarray)
                        else r.vector
                    )
                    for r in records
                ]
                metadata = [
                    r.metadata if hasattr(r, "metadata") else {} for r in records
                ]

                # Stack vectors into 2D array
                vectors = np.vstack(vectors) if len(vectors) > 0 else np.array([])

            # Handle array-based input
            elif vectors is not None:
                if not isinstance(vectors, np.ndarray):
                    vectors = np.array(vectors, dtype=np.float32)

                if ids is None:
                    ids = [f"vec_{uuid.uuid4().hex[:8]}" for _ in range(len(vectors))]

            else:
                return InsertResponse(success=False, errors=["No vectors provided"])

            # Insert into embedded database
            count = self._db.insert(collection_id, ids, vectors, metadata)

            return InsertResponse(success=True, inserted=count)

        except Exception as e:
            return InsertResponse(success=False, errors=[str(e)])

    def search(
        self,
        collection_id: str,
        vector: Union[List[float], np.ndarray],
        top_k: int = 10,
        filter: Optional[str] = None,
        **kwargs,
    ) -> List[SearchResult]:
        """Search for similar vectors"""
        try:
            if not isinstance(vector, np.ndarray):
                vector = np.array(vector, dtype=np.float32)

            results = self._db.search(collection_id, vector, top_k=top_k, filter=filter)

            return [
                SearchResult(
                    id=r.id,
                    score=r.score,
                    metadata=r.metadata if hasattr(r, "metadata") else {},
                    vector=r.vector if hasattr(r, "vector") else None,
                )
                for r in results
            ]

        except Exception as e:
            print(f"Search error: {e}")
            return []

    def get_vector(self, collection_id: str, vector_id: str) -> Optional[SearchResult]:
        """Get a specific vector by ID"""
        # Not directly supported, use search as fallback
        return None

    def delete_vectors(self, collection_id: str, ids: List[str]) -> bool:
        """Delete vectors by IDs"""
        # Not directly implemented in embedded module
        return False

    # Utility methods

    def flush(self):
        """Flush pending writes"""
        self._db.flush()

    def stats(self):
        """Get storage statistics"""
        return self._db.stats()


# Convenience function
def create_embedded_client(**kwargs) -> EmbeddedClientAdapter:
    """Create an embedded client adapter"""
    return EmbeddedClientAdapter(**kwargs)
