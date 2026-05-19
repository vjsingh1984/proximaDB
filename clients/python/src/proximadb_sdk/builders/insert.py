"""
Insert Builder

Fluent interface for building batch insert operations.
"""

from typing import Any, Dict, List, Optional, Union

import numpy as np

from ..models import MetadataDict, VectorRecord
from ..models_v2 import ProximaRecord


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
        records = [...]  # List of ProximaRecord objects or record-shaped dicts
        insert = (InsertBuilder()
            .add_records(records)
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
        self.records: List[Dict[str, Any]] = []
        self._batch_size = 1000
        self._overwrite = False
        self._validate_vectors = True
        self._async_mode = False

    @staticmethod
    def _normalize_record(record: Union[ProximaRecord, VectorRecord, Dict[str, Any]]) -> Dict[str, Any]:
        if hasattr(record, "model_dump"):
            record = record.model_dump(exclude_none=True)
        elif hasattr(record, "dict"):
            record = record.dict(exclude_none=True)
        if not isinstance(record, dict):
            raise TypeError(f"Unsupported record type: {type(record)!r}")

        props = {}
        for source in ("props", "metadata", "flexible_fields"):
            values = record.get(source)
            if isinstance(values, dict):
                props.update(values)

        normalized = {
            "id": record.get("id"),
            "vector": list(record.get("vector") or []),
            "props": props,
        }
        for field in (
            "typed_fields",
            "text_fields",
            "timestamp_ms",
            "updated_at_ms",
            "expires_at_ms",
            "version",
            "source",
            "source_type",
            "schema_id",
        ):
            if record.get(field) is not None:
                normalized[field] = record[field]
        return normalized

    def add_record(
        self,
        record: Union[ProximaRecord, Dict[str, Any]],
    ) -> "InsertBuilder":
        """Add a single ProximaRecord-shaped record."""
        self.records.append(self._normalize_record(record))
        return self

    def add_records(
        self,
        records: List[Union[ProximaRecord, Dict[str, Any]]],
    ) -> "InsertBuilder":
        """Add ProximaRecord-shaped records."""
        for record in records:
            self.add_record(record)
        return self

    def add_vector(
        self,
        vector_id: str,
        vector: List[float],
        metadata: Optional[MetadataDict] = None,
    ) -> "InsertBuilder":
        """Compatibility alias for adding one vector-bearing record."""
        self.add_record({"id": vector_id, "vector": vector, "props": metadata or {}})
        return self

    def add_vectors(self, vectors: List[VectorRecord]) -> "InsertBuilder":
        """Compatibility alias for adding multiple vector records."""
        for vector in vectors:
            self.add_record(vector)
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
            self.add_record(
                {
                    "id": vector_id,
                    "vector": vector,
                    "props": metadata[i] if metadata else {},
                }
            )

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

            self.add_record({"id": vector_id, "vector": vector, "props": metadata})

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
        """Clear all records."""
        self.records.clear()
        return self

    def filter_duplicates(self) -> "InsertBuilder":
        """Remove duplicate record IDs (keeps first occurrence)"""
        seen_ids = set()
        filtered_records = []

        for record in self.records:
            if record.get("id") not in seen_ids:
                seen_ids.add(record.get("id"))
                filtered_records.append(record)

        self.records = filtered_records
        return self

    def validate_dimensions(self, expected_dimension: int) -> "InsertBuilder":
        """Validate that all vectors have the expected dimension."""
        for i, record in enumerate(self.records):
            vector = record.get("vector") or []
            if len(vector) != expected_dimension:
                raise ValueError(
                    f"Record {i} (id: {record.get('id')}) has dimension {len(vector)}, "
                    f"expected {expected_dimension}"
                )
        return self

    def normalize_vectors(self) -> "InsertBuilder":
        """L2 normalize all record vectors."""
        for record in self.records:
            vector = record.get("vector") or []
            norm = sum(x * x for x in vector) ** 0.5
            if norm > 0:
                record["vector"] = [x / norm for x in vector]
        return self

    def add_metadata_field(self, key: str, value: Any) -> "InsertBuilder":
        """Add a property field to all records."""
        for record in self.records:
            record.setdefault("props", {})[key] = value
        return self

    def transform_metadata(self, transformer) -> "InsertBuilder":
        """Apply transformation function to all record properties."""
        for record in self.records:
            record["props"] = transformer(record.get("props", {}))
        return self

    def build(self) -> tuple[List[Dict[str, Any]], Dict[str, Any]]:
        """Build records list and insert options."""
        options = {
            "batch_size": self._batch_size,
            "overwrite": self._overwrite,
            "validate_vectors": self._validate_vectors,
            "async_mode": self._async_mode,
        }
        return self.build_records(), options

    def build_records(self) -> List[Dict[str, Any]]:
        """Build just the record list."""
        return [record.copy() for record in self.records]

    def build_vectors(self) -> List[VectorRecord]:
        """Compatibility builder returning legacy VectorRecord objects."""
        return [
            VectorRecord(
                id=record.get("id"),
                vector=record.get("vector") or [],
                metadata=record.get("props") or {},
            )
            for record in self.records
        ]

    def build_options(self) -> Dict[str, Any]:
        """Build just the insert options"""
        return {
            "batch_size": self._batch_size,
            "overwrite": self._overwrite,
            "validate_vectors": self._validate_vectors,
            "async_mode": self._async_mode,
        }

    def count(self) -> int:
        """Get number of records."""
        return len(self.records)

    def is_empty(self) -> bool:
        """Check if no records are added."""
        return len(self.records) == 0

    def get_vector_ids(self) -> List[str]:
        """Get list of all record IDs."""
        return [record.get("id") for record in self.records]

    def get_dimensions(self) -> List[int]:
        """Get dimensions of all record vectors."""
        return [len(record.get("vector") or []) for record in self.records]

    def summary(self) -> Dict[str, Any]:
        """Get summary statistics"""
        if not self.records:
            return {"count": 0, "dimensions": [], "has_metadata": False}

        dimensions = self.get_dimensions()
        metadata_counts = sum(1 for record in self.records if record.get("props"))

        return {
            "count": len(self.records),
            "dimensions": {
                "min": min(dimensions),
                "max": max(dimensions),
                "unique": list(set(dimensions)),
            },
            "has_metadata": metadata_counts > 0,
            "metadata_coverage": metadata_counts / len(self.records),
            "duplicate_ids": len(self.records) - len(set(self.get_vector_ids())),
        }


# Convenience functions
def insert() -> InsertBuilder:
    """Create a new InsertBuilder"""
    return InsertBuilder()


def batch_insert(
    records: List[Union[ProximaRecord, Dict[str, Any]]], batch_size: int = 1000
) -> tuple[List[Dict[str, Any]], Dict[str, Any]]:
    """Create simple record batch insert."""
    return InsertBuilder().add_records(records).batch_size(batch_size).build()


def from_numpy(
    ids: List[str],
    vectors: np.ndarray,
    metadata: Optional[List[MetadataDict]] = None,
    batch_size: int = 1000,
) -> tuple[List[Dict[str, Any]], Dict[str, Any]]:
    """Create record batch insert from numpy array."""
    return (
        InsertBuilder()
        .from_arrays(ids, vectors, metadata)
        .batch_size(batch_size)
        .build()
    )
