"""
Insert Builder

Fluent interface for building batch insert operations.
"""

from typing import Any, Dict, List, Optional, Union
import numpy as np

from ..models import VectorRecord, MetadataDict


class InsertBuilder:
    """
    Fluent interface for building batch insert operations.

    Examples:
        # Simple batch insert
        insert = (InsertBuilder()
            .add_vector("vec1", [0.1, 0.2, 0.3])
            .add_vector("vec2", [0.4, 0.5, 0.6], {"category": "test"})
            .batch_size(1000)
            .build())

        # From existing data
        vectors = [...]  # List of VectorRecord objects
        insert = (InsertBuilder()
            .add_vectors(vectors)
            .overwrite_existing()
            .async_mode()
            .build())

        # From numpy arrays
        ids = ["vec1", "vec2", "vec3"]
        embeddings = np.array([[0.1, 0.2], [0.3, 0.4], [0.5, 0.6]])
        metadata = [{"label": "A"}, {"label": "B"}, {"label": "C"}]

        insert = (InsertBuilder()
            .from_arrays(ids, embeddings, metadata)
            .validate_vectors()
            .build())
    """

    def __init__(self):
        """Initialize insert builder"""
        self.vectors: List[VectorRecord] = []
        self._batch_size = 1000
        self._overwrite = False
        self._validate_vectors = True
        self._async_mode = False

    def add_vector(
        self,
        vector_id: str,
        vector: List[float],
        metadata: Optional[MetadataDict] = None,
    ) -> "InsertBuilder":
        """Add a single vector"""
        record = VectorRecord(id=vector_id, vector=vector, metadata=metadata or {})
        self.vectors.append(record)
        return self

    def add_vectors(self, vectors: List[VectorRecord]) -> "InsertBuilder":
        """Add multiple VectorRecord objects"""
        self.vectors.extend(vectors)
        return self

    def add_records(self, records: List[Dict[str, Any]]) -> "InsertBuilder":
        """Add vectors from dictionary records"""
        for record in records:
            vector_record = VectorRecord(
                id=record["id"],
                vector=record["vector"],
                metadata=record.get("metadata", {}),
            )
            self.vectors.append(vector_record)
        return self

    def from_arrays(
        self,
        ids: List[str],
        vectors: Union[List[List[float]], np.ndarray],
        metadata: Optional[List[MetadataDict]] = None,
    ) -> "InsertBuilder":
        """Add vectors from arrays"""
        if isinstance(vectors, np.ndarray):
            vectors = vectors.tolist()

        if len(ids) != len(vectors):
            raise ValueError("IDs and vectors must have the same length")

        if metadata and len(metadata) != len(ids):
            raise ValueError("Metadata list must have the same length as IDs")

        for i, (vector_id, vector) in enumerate(zip(ids, vectors)):
            record = VectorRecord(
                id=vector_id, vector=vector, metadata=metadata[i] if metadata else {}
            )
            self.vectors.append(record)

        return self

    def from_dataframe(
        self,
        df,
        id_col: str,
        vector_col: str,
        metadata_cols: Optional[List[str]] = None,
    ) -> "InsertBuilder":
        """Add vectors from pandas DataFrame"""
        try:
            import pandas as pd
        except ImportError:
            raise ImportError("pandas is required for from_dataframe method")

        if not isinstance(df, pd.DataFrame):
            raise ValueError("Input must be a pandas DataFrame")

        for _, row in df.iterrows():
            vector_id = str(row[id_col])
            vector = row[vector_col]

            # Handle different vector formats
            if isinstance(vector, str):
                # Parse string representation
                vector = eval(vector)  # Simple parsing - could be improved
            elif hasattr(vector, "tolist"):
                # numpy array or similar
                vector = vector.tolist()

            # Build metadata
            metadata = {}
            if metadata_cols:
                for col in metadata_cols:
                    if col in row:
                        metadata[col] = row[col]

            record = VectorRecord(id=vector_id, vector=vector, metadata=metadata)
            self.vectors.append(record)

        return self

    def batch_size(self, size: int) -> "InsertBuilder":
        """Set batch size for processing"""
        if size <= 0:
            raise ValueError("Batch size must be positive")
        if size > 10000:
            raise ValueError("Batch size cannot exceed 10000")
        self._batch_size = size
        return self

    def overwrite_existing(self, overwrite: bool = True) -> "InsertBuilder":
        """Enable/disable overwriting existing vectors"""
        self._overwrite = overwrite
        return self

    def validate_vectors(self, validate: bool = True) -> "InsertBuilder":
        """Enable/disable vector validation"""
        self._validate_vectors = validate
        return self

    def async_mode(self, async_mode: bool = True) -> "InsertBuilder":
        """Enable/disable async processing"""
        self._async_mode = async_mode
        return self

    def clear(self) -> "InsertBuilder":
        """Clear all vectors"""
        self.vectors.clear()
        return self

    def filter_duplicates(self) -> "InsertBuilder":
        """Remove duplicate vector IDs (keeps first occurrence)"""
        seen_ids = set()
        filtered_vectors = []

        for vector in self.vectors:
            if vector.id not in seen_ids:
                seen_ids.add(vector.id)
                filtered_vectors.append(vector)

        self.vectors = filtered_vectors
        return self

    def validate_dimensions(self, expected_dimension: int) -> "InsertBuilder":
        """Validate that all vectors have the expected dimension"""
        for i, vector in enumerate(self.vectors):
            if len(vector.vector) != expected_dimension:
                raise ValueError(
                    f"Vector {i} (id: {vector.id}) has dimension {len(vector.vector)}, "
                    f"expected {expected_dimension}"
                )
        return self

    def normalize_vectors(self) -> "InsertBuilder":
        """L2 normalize all vectors"""
        for vector in self.vectors:
            norm = sum(x * x for x in vector.vector) ** 0.5
            if norm > 0:
                vector.vector = [x / norm for x in vector.vector]
        return self

    def add_metadata_field(self, key: str, value: Any) -> "InsertBuilder":
        """Add a metadata field to all vectors"""
        for vector in self.vectors:
            vector.metadata[key] = value
        return self

    def transform_metadata(self, transformer) -> "InsertBuilder":
        """Apply transformation function to all metadata"""
        for vector in self.vectors:
            vector.metadata = transformer(vector.metadata)
        return self

    def build(self) -> tuple[List[VectorRecord], Dict[str, Any]]:
        """Build vectors list and insert options"""
        options = {
            "batch_size": self._batch_size,
            "overwrite": self._overwrite,
            "validate_vectors": self._validate_vectors,
            "async_mode": self._async_mode,
        }
        return self.vectors.copy(), options

    def build_vectors(self) -> List[VectorRecord]:
        """Build just the vectors list"""
        return self.vectors.copy()

    def build_options(self) -> Dict[str, Any]:
        """Build just the insert options"""
        return {
            "batch_size": self._batch_size,
            "overwrite": self._overwrite,
            "validate_vectors": self._validate_vectors,
            "async_mode": self._async_mode,
        }

    def count(self) -> int:
        """Get number of vectors"""
        return len(self.vectors)

    def is_empty(self) -> bool:
        """Check if no vectors are added"""
        return len(self.vectors) == 0

    def get_vector_ids(self) -> List[str]:
        """Get list of all vector IDs"""
        return [v.id for v in self.vectors]

    def get_dimensions(self) -> List[int]:
        """Get dimensions of all vectors"""
        return [len(v.vector) for v in self.vectors]

    def summary(self) -> Dict[str, Any]:
        """Get summary statistics"""
        if not self.vectors:
            return {"count": 0, "dimensions": [], "has_metadata": False}

        dimensions = self.get_dimensions()
        metadata_counts = sum(1 for v in self.vectors if v.metadata)

        return {
            "count": len(self.vectors),
            "dimensions": {
                "min": min(dimensions),
                "max": max(dimensions),
                "unique": list(set(dimensions)),
            },
            "has_metadata": metadata_counts > 0,
            "metadata_coverage": metadata_counts / len(self.vectors),
            "duplicate_ids": len(self.vectors) - len(set(self.get_vector_ids())),
        }


# Convenience functions
def insert() -> InsertBuilder:
    """Create a new InsertBuilder"""
    return InsertBuilder()


def batch_insert(
    vectors: List[VectorRecord], batch_size: int = 1000
) -> tuple[List[VectorRecord], Dict[str, Any]]:
    """Create simple batch insert"""
    return InsertBuilder().add_vectors(vectors).batch_size(batch_size).build()


def from_numpy(
    ids: List[str],
    vectors: np.ndarray,
    metadata: Optional[List[MetadataDict]] = None,
    batch_size: int = 1000,
) -> tuple[List[VectorRecord], Dict[str, Any]]:
    """Create batch insert from numpy array"""
    return (
        InsertBuilder()
        .from_arrays(ids, vectors, metadata)
        .batch_size(batch_size)
        .build()
    )
