"""
ProximaDB SDK v2 Models - ProximaRecord and typed schema support

This module provides Pydantic models for the v2 API with full type support.
ProximaRecord replaces VectorRecord with rich typed columns and dedicated TEXT storage.

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0

Example:
    from proximadb_sdk import ProximaRecord, TypedValue, TextField, FilterBuilder

    # Create a record with typed fields
    record = ProximaRecord(
        id="doc_123",
        vector=[0.1, 0.2, 0.3],
        typed_fields={
            "category": TypedValue.text("technology"),
            "price": TypedValue.float_(29.99),
            "in_stock": TypedValue.boolean(True),
        },
        text_fields=[
            TextField(name="content", content="Full article text...")
        ]
    )

    # Build typed filters
    filters = (FilterBuilder("price")
        .gte(10.0)
        .and_("category")
        .eq("electronics")
        .build())
"""

import re
import time
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional, Union

from pydantic import BaseModel, ConfigDict, Field, field_validator

# ============================================================================
# COLUMN DATA TYPES
# ============================================================================


class ColumnDataType(str, Enum):
    """Supported column data types for ProximaRecord.

    These types align with common database column types and provide
    type safety for metadata fields in ProximaDB.

    Example:
        schema = RecordSchema()
        schema.add_column("name", ColumnDataType.TEXT)
        schema.add_column("price", ColumnDataType.FLOAT)
    """

    TEXT = "text"
    TEXT_LARGE = "text_large"
    INTEGER = "integer"
    FLOAT = "float"
    DECIMAL = "decimal"
    BOOLEAN = "boolean"
    TIMESTAMP = "timestamp"
    TIMESTAMP_TZ = "timestamp_tz"
    DATE = "date"
    TIME = "time"
    UUID = "uuid"
    BINARY = "binary"
    JSON = "json"
    ARRAY_TEXT = "array_text"
    ARRAY_INTEGER = "array_integer"
    ARRAY_FLOAT = "array_float"
    MAP_STRING_STRING = "map_string_string"
    MAP_STRING_ANY = "map_string_any"


# ============================================================================
# TEXT STORAGE
# ============================================================================


class TextStorageStrategy(str, Enum):
    """Storage strategy for TEXT columns.

    Different storage strategies optimize for different text sizes:
    - INLINE: Best for short text (<4KB), stores directly in main column
    - CHUNKED: For medium text (4KB-1MB), splits into chunks with embeddings
    - SIDECAR: For large text (>1MB), uses separate sidecar files
    - ADAPTIVE: Auto-selects strategy based on content size

    Example:
        field = TextField(
            name="article",
            content="Long article text...",
            storage_hint=TextStorageStrategy.CHUNKED
        )
    """

    INLINE = "inline"  # < 4KB, store in main column
    CHUNKED = "chunked"  # 4KB - 1MB, split into chunks with embeddings
    SIDECAR = "sidecar"  # > 1MB, separate sidecar file
    ADAPTIVE = "adaptive"  # Auto-select based on size


class TextField(BaseModel):
    """Text field with storage hint for ProximaRecord.

    TextField provides dedicated storage for text content that may need
    special handling (chunking, embedding generation, etc.).

    Attributes:
        name: Field name (must be unique within a record)
        content: The text content (max 10MB)
        storage_hint: Strategy for storing the text

    Example:
        text_field = TextField(
            name="article_body",
            content="This is the full article content...",
            storage_hint=TextStorageStrategy.ADAPTIVE
        )
    """

    model_config = ConfigDict(populate_by_name=True)

    name: str = Field(..., min_length=1, description="Field name")
    content: str = Field(..., description="Text content")
    storage_hint: TextStorageStrategy = Field(
        default=TextStorageStrategy.ADAPTIVE, description="Storage strategy hint"
    )

    @field_validator("content")
    @classmethod
    def validate_content(cls, v: str) -> str:
        """Validate text content size (max 10MB)."""
        if len(v) > 10 * 1024 * 1024:  # 10MB max
            raise ValueError("Text content exceeds 10MB limit")
        return v

    @field_validator("name")
    @classmethod
    def validate_name(cls, v: str) -> str:
        """Validate field name is not empty."""
        if not v or not v.strip():
            raise ValueError("Field name cannot be empty")
        return v.strip()


class TextColumnConfig(BaseModel):
    """Configuration for TEXT columns in a collection schema.

    TextColumnConfig defines how TEXT columns should be stored and indexed
    in ProximaDB. Different strategies optimize for different use cases:

    - INLINE: Best for short text (<4KB), stores directly in the main column.
      Fast reads, minimal overhead, but not suitable for large documents.

    - CHUNKED: Ideal for RAG (Retrieval-Augmented Generation) workloads.
      Splits text into chunks (4KB-1MB), generates embeddings per chunk,
      enables semantic search within documents.

    - SIDECAR: For large documents (>1MB) like PDFs or books.
      Stores text in separate sidecar files, reduces main table bloat.

    - ADAPTIVE: Auto-selects strategy based on content size at write time.
      Good default for mixed-size content.

    Attributes:
        column_name: Name of the TEXT column (must be unique within collection)
        strategy: Storage strategy for the text content
        chunk_size: Target chunk size in tokens (for CHUNKED strategy, default 512)
        chunk_overlap: Overlap between chunks in tokens (for CHUNKED, default 50)
        enable_ngram_bloom: Enable n-gram bloom filter for fast substring search
        ngram_size: N-gram size for bloom filter (default 3)
        enable_full_text_search: Enable full-text search indexing
        embedding_model: Optional embedding model name for CHUNKED strategy
        max_content_size_bytes: Maximum allowed content size (default 10MB)
        compression_enabled: Enable compression for stored text
        language: Language hint for text processing (e.g., "en", "es", "zh")

    Example:
        # Configure a TEXT column for RAG workloads
        from proximadb_sdk import TextColumnConfig, TextStorageStrategy

        text_config = TextColumnConfig(
            column_name="content",
            strategy=TextStorageStrategy.CHUNKED,
            chunk_size=512,
            chunk_overlap=50,
            enable_ngram_bloom=True
        )

        # Configure for large documents
        large_doc_config = TextColumnConfig(
            column_name="document_body",
            strategy=TextStorageStrategy.SIDECAR,
            compression_enabled=True
        )

        # Auto-adaptive configuration
        adaptive_config = TextColumnConfig(
            column_name="description",
            strategy=TextStorageStrategy.ADAPTIVE,
            enable_full_text_search=True
        )
    """

    model_config = ConfigDict(populate_by_name=True)

    column_name: str = Field(
        ..., min_length=1, max_length=256, description="Name of the TEXT column"
    )
    strategy: TextStorageStrategy = Field(
        default=TextStorageStrategy.ADAPTIVE,
        description="Storage strategy for text content",
    )
    chunk_size: int = Field(
        default=512,
        ge=64,
        le=8192,
        description="Target chunk size in tokens (for CHUNKED strategy)",
    )
    chunk_overlap: int = Field(
        default=50,
        ge=0,
        le=1024,
        description="Overlap between chunks in tokens (for CHUNKED strategy)",
    )
    enable_ngram_bloom: bool = Field(
        default=False,
        description="Enable n-gram bloom filter for fast substring search",
    )
    ngram_size: int = Field(
        default=3, ge=2, le=8, description="N-gram size for bloom filter"
    )
    enable_full_text_search: bool = Field(
        default=False, description="Enable full-text search indexing"
    )
    embedding_model: Optional[str] = Field(
        default=None,
        description="Embedding model name for CHUNKED strategy (e.g., 'text-embedding-3-small')",
    )
    max_content_size_bytes: int = Field(
        default=10 * 1024 * 1024,  # 10MB
        ge=1024,
        le=100 * 1024 * 1024,  # 100MB max
        description="Maximum allowed content size in bytes",
    )
    compression_enabled: bool = Field(
        default=False, description="Enable compression for stored text"
    )
    language: Optional[str] = Field(
        default=None, description="Language hint for text processing (ISO 639-1 code)"
    )

    @field_validator("column_name")
    @classmethod
    def validate_column_name(cls, v: str) -> str:
        """Validate column name format."""
        if not v or not v.strip():
            raise ValueError("Column name cannot be empty")
        v = v.strip()
        # Validate column name format (alphanumeric, underscores, no leading digits)
        if not v[0].isalpha() and v[0] != "_":
            raise ValueError("Column name must start with a letter or underscore")
        if not all(c.isalnum() or c == "_" for c in v):
            raise ValueError(
                "Column name can only contain letters, numbers, and underscores"
            )
        return v

    @field_validator("chunk_overlap")
    @classmethod
    def validate_chunk_overlap(cls, v: int, info) -> int:
        """Validate chunk overlap is less than chunk size."""
        # Access chunk_size from the data being validated
        if hasattr(info, "data") and "chunk_size" in info.data:
            chunk_size = info.data["chunk_size"]
            if v >= chunk_size:
                raise ValueError(
                    f"Chunk overlap ({v}) must be less than chunk size ({chunk_size})"
                )
        return v

    def to_dict(self) -> dict:
        """Convert to dictionary for API serialization.

        Returns:
            Dictionary representation with None values excluded
        """
        return self.model_dump(exclude_none=True)

    @classmethod
    def for_rag(
        cls,
        column_name: str,
        chunk_size: int = 512,
        chunk_overlap: int = 50,
        embedding_model: Optional[str] = None,
        enable_ngram_bloom: bool = True,
    ) -> "TextColumnConfig":
        """Create a TEXT column configuration optimized for RAG workloads.

        This factory method creates a configuration suitable for
        Retrieval-Augmented Generation (RAG) applications, with
        chunking and optional n-gram bloom filters for hybrid search.

        Args:
            column_name: Name of the TEXT column
            chunk_size: Target chunk size in tokens (default 512)
            chunk_overlap: Overlap between chunks in tokens (default 50)
            embedding_model: Optional embedding model name
            enable_ngram_bloom: Enable n-gram bloom filter (default True)

        Returns:
            TextColumnConfig configured for RAG

        Example:
            config = TextColumnConfig.for_rag(
                "document_content",
                chunk_size=512,
                embedding_model="text-embedding-3-small"
            )
        """
        return cls(
            column_name=column_name,
            strategy=TextStorageStrategy.CHUNKED,
            chunk_size=chunk_size,
            chunk_overlap=chunk_overlap,
            enable_ngram_bloom=enable_ngram_bloom,
            embedding_model=embedding_model,
        )

    @classmethod
    def for_short_text(
        cls, column_name: str, enable_full_text_search: bool = False
    ) -> "TextColumnConfig":
        """Create a TEXT column configuration for short text content.

        This factory method creates a configuration suitable for
        short text fields like titles, descriptions, or tags.
        Uses INLINE storage for minimal overhead.

        Args:
            column_name: Name of the TEXT column
            enable_full_text_search: Enable full-text search indexing

        Returns:
            TextColumnConfig configured for short text

        Example:
            title_config = TextColumnConfig.for_short_text("title")
            description_config = TextColumnConfig.for_short_text(
                "description",
                enable_full_text_search=True
            )
        """
        return cls(
            column_name=column_name,
            strategy=TextStorageStrategy.INLINE,
            enable_full_text_search=enable_full_text_search,
        )

    @classmethod
    def for_large_documents(
        cls,
        column_name: str,
        compression_enabled: bool = True,
        language: Optional[str] = None,
    ) -> "TextColumnConfig":
        """Create a TEXT column configuration for large documents.

        This factory method creates a configuration suitable for
        large documents like PDFs, books, or research papers.
        Uses SIDECAR storage to keep the main table compact.

        Args:
            column_name: Name of the TEXT column
            compression_enabled: Enable compression (default True)
            language: Language hint for text processing

        Returns:
            TextColumnConfig configured for large documents

        Example:
            pdf_config = TextColumnConfig.for_large_documents(
                "pdf_content",
                language="en"
            )
        """
        return cls(
            column_name=column_name,
            strategy=TextStorageStrategy.SIDECAR,
            compression_enabled=compression_enabled,
            language=language,
            max_content_size_bytes=100 * 1024 * 1024,  # 100MB for large docs
        )

    @classmethod
    def for_hybrid_search(
        cls, column_name: str, chunk_size: int = 512, ngram_size: int = 3
    ) -> "TextColumnConfig":
        """Create a TEXT column configuration for hybrid search.

        This factory method creates a configuration suitable for
        hybrid search combining vector similarity with keyword matching.
        Enables both chunked storage and n-gram bloom filters.

        Args:
            column_name: Name of the TEXT column
            chunk_size: Target chunk size in tokens (default 512)
            ngram_size: N-gram size for bloom filter (default 3)

        Returns:
            TextColumnConfig configured for hybrid search

        Example:
            hybrid_config = TextColumnConfig.for_hybrid_search(
                "article_content",
                chunk_size=256,
                ngram_size=4
            )
        """
        return cls(
            column_name=column_name,
            strategy=TextStorageStrategy.CHUNKED,
            chunk_size=chunk_size,
            enable_ngram_bloom=True,
            ngram_size=ngram_size,
            enable_full_text_search=True,
        )


# ============================================================================
# SCHEMA ENFORCEMENT
# ============================================================================


class SchemaEnforcement(str, Enum):
    """Schema enforcement mode for collections.

    Controls how strictly the schema is enforced for records:
    - STRICT: All columns must match schema exactly
    - FLEXIBLE: Schema on read, no enforcement on write
    - HYBRID: Core columns enforced, additional fields allowed

    Example:
        schema = RecordSchema(
            enforcement=SchemaEnforcement.HYBRID,
            allow_additional_fields=True
        )
    """

    STRICT = "strict"  # All columns must match schema
    FLEXIBLE = "flexible"  # Schema on read
    HYBRID = "hybrid"  # Core columns enforced, extras allowed


# ============================================================================
# TYPED VALUES
# ============================================================================


class TypedValue(BaseModel):
    """Union type for typed field values.

    TypedValue wraps a value with its type information, enabling
    type-safe metadata storage and filtering in ProximaDB.

    Use the factory methods to create typed values:

    Example:
        # Using factory methods (recommended)
        name = TypedValue.text("John Doe")
        age = TypedValue.integer(30)
        price = TypedValue.float_(29.99)
        active = TypedValue.boolean(True)

        # Using constructor directly
        value = TypedValue(value_type=ColumnDataType.TEXT, value="hello")
    """

    model_config = ConfigDict(populate_by_name=True)

    value_type: ColumnDataType = Field(..., description="The data type of the value")
    value: Any = Field(..., description="The actual value")

    @classmethod
    def text(cls, value: str) -> "TypedValue":
        """Create a TEXT typed value.

        Args:
            value: String value

        Returns:
            TypedValue with TEXT type

        Example:
            name = TypedValue.text("Product Name")
        """
        return cls(value_type=ColumnDataType.TEXT, value=value)

    @classmethod
    def text_large(cls, value: str) -> "TypedValue":
        """Create a TEXT_LARGE typed value for longer text content.

        Args:
            value: String value (can be longer than standard TEXT)

        Returns:
            TypedValue with TEXT_LARGE type
        """
        return cls(value_type=ColumnDataType.TEXT_LARGE, value=value)

    @classmethod
    def integer(cls, value: int) -> "TypedValue":
        """Create an INTEGER typed value.

        Args:
            value: Integer value

        Returns:
            TypedValue with INTEGER type

        Example:
            count = TypedValue.integer(42)
        """
        return cls(value_type=ColumnDataType.INTEGER, value=value)

    @classmethod
    def float_(cls, value: float) -> "TypedValue":
        """Create a FLOAT typed value.

        Note: Named float_ to avoid shadowing Python's built-in float.

        Args:
            value: Float value

        Returns:
            TypedValue with FLOAT type

        Example:
            price = TypedValue.float_(29.99)
        """
        return cls(value_type=ColumnDataType.FLOAT, value=value)

    @classmethod
    def decimal(cls, value: Union[float, str]) -> "TypedValue":
        """Create a DECIMAL typed value for precise decimal numbers.

        Args:
            value: Decimal value (as float or string for precision)

        Returns:
            TypedValue with DECIMAL type
        """
        return cls(value_type=ColumnDataType.DECIMAL, value=value)

    @classmethod
    def boolean(cls, value: bool) -> "TypedValue":
        """Create a BOOLEAN typed value.

        Args:
            value: Boolean value

        Returns:
            TypedValue with BOOLEAN type

        Example:
            active = TypedValue.boolean(True)
        """
        return cls(value_type=ColumnDataType.BOOLEAN, value=value)

    @classmethod
    def uuid(cls, value: str) -> "TypedValue":
        """Create a UUID typed value.

        Validates that the value is a properly formatted UUID.

        Args:
            value: UUID string (e.g., "550e8400-e29b-41d4-a716-446655440000")

        Returns:
            TypedValue with UUID type

        Raises:
            ValueError: If UUID format is invalid

        Example:
            id = TypedValue.uuid("550e8400-e29b-41d4-a716-446655440000")
        """
        uuid_pattern = r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
        if not re.match(uuid_pattern, value):
            raise ValueError(f"Invalid UUID format: {value}")
        return cls(value_type=ColumnDataType.UUID, value=value)

    @classmethod
    def timestamp(cls, value: Union[datetime, int]) -> "TypedValue":
        """Create a TIMESTAMP typed value.

        Accepts either a datetime object or milliseconds since epoch.

        Args:
            value: datetime object or int (milliseconds since epoch)

        Returns:
            TypedValue with TIMESTAMP type (stored as milliseconds)

        Example:
            now = TypedValue.timestamp(datetime.now())
            explicit = TypedValue.timestamp(1704067200000)
        """
        if isinstance(value, datetime):
            value = int(value.timestamp() * 1000)
        return cls(value_type=ColumnDataType.TIMESTAMP, value=value)

    @classmethod
    def timestamp_tz(
        cls, value: Union[datetime, int], timezone: Optional[str] = None
    ) -> "TypedValue":
        """Create a TIMESTAMP_TZ typed value with timezone.

        Args:
            value: datetime object or int (milliseconds since epoch)
            timezone: Optional timezone string (e.g., "UTC", "America/New_York")

        Returns:
            TypedValue with TIMESTAMP_TZ type
        """
        if isinstance(value, datetime):
            value = int(value.timestamp() * 1000)
        return cls(
            value_type=ColumnDataType.TIMESTAMP_TZ,
            value={"timestamp": value, "timezone": timezone},
        )

    @classmethod
    def date(cls, value: Union[datetime, str]) -> "TypedValue":
        """Create a DATE typed value (no time component).

        Args:
            value: datetime object or ISO date string (YYYY-MM-DD)

        Returns:
            TypedValue with DATE type
        """
        if isinstance(value, datetime):
            value = value.strftime("%Y-%m-%d")
        return cls(value_type=ColumnDataType.DATE, value=value)

    @classmethod
    def time_(cls, value: Union[datetime, str]) -> "TypedValue":
        """Create a TIME typed value (no date component).

        Args:
            value: datetime object or ISO time string (HH:MM:SS)

        Returns:
            TypedValue with TIME type
        """
        if isinstance(value, datetime):
            value = value.strftime("%H:%M:%S")
        return cls(value_type=ColumnDataType.TIME, value=value)

    @classmethod
    def binary(cls, value: bytes) -> "TypedValue":
        """Create a BINARY typed value.

        Args:
            value: Bytes value

        Returns:
            TypedValue with BINARY type (base64 encoded for serialization)
        """
        import base64

        encoded = base64.b64encode(value).decode("ascii")
        return cls(value_type=ColumnDataType.BINARY, value=encoded)

    @classmethod
    def json_(cls, value: dict) -> "TypedValue":
        """Create a JSON typed value.

        Args:
            value: Dictionary to store as JSON

        Returns:
            TypedValue with JSON type

        Example:
            config = TypedValue.json_({"key": "value", "nested": {"a": 1}})
        """
        return cls(value_type=ColumnDataType.JSON, value=value)

    @classmethod
    def array_text(cls, value: List[str]) -> "TypedValue":
        """Create an ARRAY_TEXT typed value.

        Args:
            value: List of strings

        Returns:
            TypedValue with ARRAY_TEXT type

        Example:
            tags = TypedValue.array_text(["python", "database", "vector"])
        """
        return cls(value_type=ColumnDataType.ARRAY_TEXT, value=value)

    @classmethod
    def array_integer(cls, value: List[int]) -> "TypedValue":
        """Create an ARRAY_INTEGER typed value.

        Args:
            value: List of integers

        Returns:
            TypedValue with ARRAY_INTEGER type
        """
        return cls(value_type=ColumnDataType.ARRAY_INTEGER, value=value)

    @classmethod
    def array_float(cls, value: List[float]) -> "TypedValue":
        """Create an ARRAY_FLOAT typed value.

        Args:
            value: List of floats

        Returns:
            TypedValue with ARRAY_FLOAT type

        Example:
            scores = TypedValue.array_float([0.95, 0.87, 0.92])
        """
        return cls(value_type=ColumnDataType.ARRAY_FLOAT, value=value)

    @classmethod
    def map_string_string(cls, value: Dict[str, str]) -> "TypedValue":
        """Create a MAP_STRING_STRING typed value.

        Args:
            value: Dictionary with string keys and string values

        Returns:
            TypedValue with MAP_STRING_STRING type
        """
        return cls(value_type=ColumnDataType.MAP_STRING_STRING, value=value)

    @classmethod
    def map_string_any(cls, value: Dict[str, Any]) -> "TypedValue":
        """Create a MAP_STRING_ANY typed value.

        Args:
            value: Dictionary with string keys and any values

        Returns:
            TypedValue with MAP_STRING_ANY type
        """
        return cls(value_type=ColumnDataType.MAP_STRING_ANY, value=value)


# ============================================================================
# PROXIMA RECORD
# ============================================================================


class ProximaRecord(BaseModel):
    """ProximaRecord - the new unified record type for ProximaDB v2.

    Replaces VectorRecord with rich typed columns and dedicated TEXT storage.
    ProximaRecord provides:
    - Strong typing for metadata fields
    - Dedicated text field storage with chunking support
    - Flexible schema with optional strict enforcement
    - TTL support via expires_at_ms
    - Version tracking for optimistic concurrency

    Attributes:
        id: Optional record ID (auto-generated if not provided)
        vector: The embedding vector (required, non-empty)
        vector_dimension: Optional dimension hint for validation
        typed_fields: Dictionary of strongly-typed field values
        flexible_fields: Dictionary of untyped field values (for HYBRID mode)
        text_fields: List of text fields with storage hints
        timestamp_ms: Creation timestamp in milliseconds
        updated_at_ms: Last update timestamp in milliseconds
        expires_at_ms: TTL expiration timestamp in milliseconds
        version: Version number for optimistic concurrency
        source: Original content that generated this vector
        schema_id: Optional schema ID for validation

    Example:
        record = ProximaRecord(
            id="doc_123",
            vector=[0.1, 0.2, 0.3],
            typed_fields={
                "category": TypedValue.text("technology"),
                "price": TypedValue.float_(29.99),
                "in_stock": TypedValue.boolean(True),
            },
            text_fields=[
                TextField(name="content", content="Full article text...")
            ]
        )

        # Fluent API
        record = (ProximaRecord(id="doc_456", vector=[0.1, 0.2, 0.3])
            .set_typed("category", TypedValue.text("science"))
            .add_text("abstract", "Research paper abstract..."))
    """

    model_config = ConfigDict(populate_by_name=True)

    id: Optional[str] = Field(
        default=None, description="Record ID (auto-generated if not provided)"
    )
    vector: List[float] = Field(..., description="Embedding vector (required)")
    vector_dimension: Optional[int] = Field(
        default=None, description="Vector dimension hint"
    )
    typed_fields: Dict[str, TypedValue] = Field(
        default_factory=dict, description="Strongly-typed field values"
    )
    flexible_fields: Dict[str, Any] = Field(
        default_factory=dict,
        description="Untyped field values (for HYBRID schema mode)",
    )
    text_fields: List[TextField] = Field(
        default_factory=list, description="Text fields with storage hints"
    )
    timestamp_ms: int = Field(
        default_factory=lambda: int(time.time() * 1000),
        description="Creation timestamp in milliseconds",
    )
    updated_at_ms: Optional[int] = Field(
        default=None, description="Last update timestamp in milliseconds"
    )
    expires_at_ms: Optional[int] = Field(
        default=None, description="TTL expiration timestamp in milliseconds"
    )
    version: Optional[int] = Field(
        default=None, description="Version number for optimistic concurrency"
    )
    source: Optional[str] = Field(
        default=None, description="Original content that generated this vector"
    )
    schema_id: Optional[str] = Field(
        default=None, description="Schema ID for validation"
    )

    @field_validator("vector")
    @classmethod
    def validate_vector(cls, v: List[float]) -> List[float]:
        """Validate that vector is non-empty."""
        if not v:
            raise ValueError("Vector cannot be empty")
        return v

    def add_text(
        self,
        name: str,
        content: str,
        storage_hint: TextStorageStrategy = TextStorageStrategy.ADAPTIVE,
    ) -> "ProximaRecord":
        """Add a text field to the record.

        Args:
            name: Field name
            content: Text content
            storage_hint: Storage strategy hint

        Returns:
            Self for method chaining

        Example:
            record.add_text("description", "Product description text")
        """
        self.text_fields.append(
            TextField(name=name, content=content, storage_hint=storage_hint)
        )
        return self

    def set_typed(self, name: str, value: TypedValue) -> "ProximaRecord":
        """Set a typed field value.

        Args:
            name: Field name
            value: TypedValue instance

        Returns:
            Self for method chaining

        Example:
            record.set_typed("price", TypedValue.float_(29.99))
        """
        self.typed_fields[name] = value
        return self

    def set_flexible(self, name: str, value: Any) -> "ProximaRecord":
        """Set a flexible (untyped) field value.

        Args:
            name: Field name
            value: Any value (will be serialized as-is)

        Returns:
            Self for method chaining
        """
        self.flexible_fields[name] = value
        return self

    def with_ttl(self, ttl_seconds: int) -> "ProximaRecord":
        """Set TTL (time-to-live) for this record.

        Args:
            ttl_seconds: Seconds until record expires

        Returns:
            Self for method chaining

        Example:
            record.with_ttl(3600)  # Expires in 1 hour
        """
        self.expires_at_ms = int(time.time() * 1000) + (ttl_seconds * 1000)
        return self

    def with_version(self, version: int) -> "ProximaRecord":
        """Set version for optimistic concurrency control.

        Args:
            version: Version number

        Returns:
            Self for method chaining
        """
        self.version = version
        return self

    def to_dict(self) -> dict:
        """Convert to dictionary for API serialization.

        Returns:
            Dictionary representation with None values excluded
        """
        return self.model_dump(exclude_none=True)

    # Backward compatibility properties
    @property
    def timestamp(self) -> int:
        """Backward compatibility: timestamp in seconds."""
        return self.timestamp_ms // 1000

    @timestamp.setter
    def timestamp(self, value: int) -> None:
        """Backward compatibility: set timestamp in seconds."""
        self.timestamp_ms = value * 1000

    @property
    def metadata(self) -> Dict[str, Any]:
        """Backward compatibility: get metadata as dict.

        Combines typed_fields and flexible_fields into a single dict.
        """
        result = {}
        for name, typed_value in self.typed_fields.items():
            result[name] = typed_value.value
        result.update(self.flexible_fields)
        return result


# ============================================================================
# SCHEMA DEFINITION
# ============================================================================


class ColumnDefinition(BaseModel):
    """Column definition for schema.

    Defines a column's properties including type, constraints, and indexing options.

    Attributes:
        name: Column name
        data_type: Column data type
        nullable: Whether NULL values are allowed
        indexed: Whether column is indexed for fast lookups
        filterable: Whether column supports filtering in queries
        max_length: Maximum length for TEXT columns
        min_value: Minimum value for numeric columns
        max_value: Maximum value for numeric columns
        regex_pattern: Regex pattern for TEXT validation
        default_value: Default value if not provided

    Example:
        column = ColumnDefinition(
            name="price",
            data_type=ColumnDataType.FLOAT,
            nullable=False,
            filterable=True,
            min_value=0.0
        )
    """

    model_config = ConfigDict(populate_by_name=True)

    name: str = Field(..., min_length=1, description="Column name")
    data_type: ColumnDataType = Field(..., description="Column data type")
    nullable: bool = Field(default=True, description="Whether NULL is allowed")
    indexed: bool = Field(default=False, description="Whether column is indexed")
    filterable: bool = Field(default=True, description="Whether filtering is supported")
    max_length: Optional[int] = Field(default=None, description="Max length for TEXT")
    min_value: Optional[float] = Field(
        default=None, description="Min value for numerics"
    )
    max_value: Optional[float] = Field(
        default=None, description="Max value for numerics"
    )
    regex_pattern: Optional[str] = Field(
        default=None, description="Regex for TEXT validation"
    )
    default_value: Optional[Any] = Field(default=None, description="Default value")


class RecordSchema(BaseModel):
    """Schema definition for a collection.

    RecordSchema defines the structure and constraints for ProximaRecords
    in a collection. Supports STRICT, FLEXIBLE, and HYBRID enforcement modes.

    Attributes:
        schema_id: Optional unique schema identifier
        schema_version: Schema version string
        columns: List of column definitions
        text_columns: List of TEXT column configurations with storage strategies
        enforcement: Schema enforcement mode
        allow_additional_fields: Whether extra fields are allowed (HYBRID mode)

    Example:
        schema = (RecordSchema()
            .add_text_column("title", max_length=256, indexed=True)
            .add_integer_column("year", min_value=1900, max_value=2100)
            .add_column("price", ColumnDataType.FLOAT, nullable=False)
            .add_text_column_config(TextColumnConfig.for_rag("content")))
    """

    model_config = ConfigDict(populate_by_name=True)

    schema_id: Optional[str] = Field(
        default=None, description="Unique schema identifier"
    )
    schema_version: str = Field(default="1.0", description="Schema version")
    columns: List[ColumnDefinition] = Field(
        default_factory=list, description="Column definitions"
    )
    text_columns: List["TextColumnConfig"] = Field(
        default_factory=list,
        description="TEXT column configurations with storage strategies",
    )
    enforcement: SchemaEnforcement = Field(
        default=SchemaEnforcement.HYBRID, description="Schema enforcement mode"
    )
    allow_additional_fields: bool = Field(
        default=True, description="Allow fields not in schema (HYBRID mode)"
    )

    def add_column(
        self, name: str, data_type: ColumnDataType, **kwargs: Any
    ) -> "RecordSchema":
        """Add a column to the schema.

        Args:
            name: Column name
            data_type: Column data type
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining

        Example:
            schema.add_column("status", ColumnDataType.TEXT, indexed=True)
        """
        self.columns.append(ColumnDefinition(name=name, data_type=data_type, **kwargs))
        return self

    def add_text_column(
        self, name: str, max_length: int = 65536, **kwargs: Any
    ) -> "RecordSchema":
        """Add a TEXT column with default settings.

        Args:
            name: Column name
            max_length: Maximum text length (default 64KB)
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(
            name, ColumnDataType.TEXT, max_length=max_length, **kwargs
        )

    def add_integer_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add an INTEGER column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.INTEGER, **kwargs)

    def add_float_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add a FLOAT column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.FLOAT, **kwargs)

    def add_boolean_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add a BOOLEAN column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.BOOLEAN, **kwargs)

    def add_timestamp_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add a TIMESTAMP column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.TIMESTAMP, **kwargs)

    def add_json_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add a JSON column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.JSON, **kwargs)

    def add_uuid_column(self, name: str, **kwargs: Any) -> "RecordSchema":
        """Add a UUID column.

        Args:
            name: Column name
            **kwargs: Additional ColumnDefinition options

        Returns:
            Self for method chaining
        """
        return self.add_column(name, ColumnDataType.UUID, **kwargs)

    def add_text_column_config(self, config: "TextColumnConfig") -> "RecordSchema":
        """Add a TEXT column with advanced storage configuration.

        This method adds a TEXT column using a TextColumnConfig object
        that specifies storage strategy, chunking parameters, and other
        advanced options for TEXT storage.

        Args:
            config: TextColumnConfig specifying column name and storage options

        Returns:
            Self for method chaining

        Example:
            # Add a TEXT column for RAG workloads
            schema.add_text_column_config(
                TextColumnConfig.for_rag("content", chunk_size=512)
            )

            # Add multiple TEXT columns with different strategies
            schema.add_text_column_config(
                TextColumnConfig.for_short_text("title")
            ).add_text_column_config(
                TextColumnConfig.for_large_documents("body")
            )
        """
        # Also add to columns list for schema validation
        self.columns.append(
            ColumnDefinition(
                name=config.column_name,
                data_type=(
                    ColumnDataType.TEXT_LARGE
                    if config.strategy != TextStorageStrategy.INLINE
                    else ColumnDataType.TEXT
                ),
                nullable=True,
                filterable=config.enable_full_text_search,
            )
        )
        # Store the full configuration
        self.text_columns.append(config)
        return self

    def add_rag_text_column(
        self,
        name: str,
        chunk_size: int = 512,
        chunk_overlap: int = 50,
        embedding_model: Optional[str] = None,
        enable_ngram_bloom: bool = True,
    ) -> "RecordSchema":
        """Add a TEXT column optimized for RAG (Retrieval-Augmented Generation).

        Convenience method that creates a TextColumnConfig with CHUNKED
        storage strategy, suitable for semantic search and RAG applications.

        Args:
            name: Column name
            chunk_size: Target chunk size in tokens (default 512)
            chunk_overlap: Overlap between chunks in tokens (default 50)
            embedding_model: Optional embedding model name
            enable_ngram_bloom: Enable n-gram bloom filter for hybrid search

        Returns:
            Self for method chaining

        Example:
            schema = (RecordSchema()
                .add_rag_text_column("document_content", chunk_size=256)
                .add_text_column("title", max_length=256))
        """
        config = TextColumnConfig.for_rag(
            column_name=name,
            chunk_size=chunk_size,
            chunk_overlap=chunk_overlap,
            embedding_model=embedding_model,
            enable_ngram_bloom=enable_ngram_bloom,
        )
        return self.add_text_column_config(config)

    def add_large_text_column(
        self,
        name: str,
        compression_enabled: bool = True,
        language: Optional[str] = None,
    ) -> "RecordSchema":
        """Add a TEXT column for large documents (SIDECAR storage).

        Convenience method that creates a TextColumnConfig with SIDECAR
        storage strategy, suitable for large documents like PDFs or books.

        Args:
            name: Column name
            compression_enabled: Enable compression (default True)
            language: Language hint for text processing

        Returns:
            Self for method chaining

        Example:
            schema = (RecordSchema()
                .add_large_text_column("pdf_content", language="en")
                .add_text_column("filename", max_length=256))
        """
        config = TextColumnConfig.for_large_documents(
            column_name=name, compression_enabled=compression_enabled, language=language
        )
        return self.add_text_column_config(config)

    def get_text_column_config(self, name: str) -> Optional["TextColumnConfig"]:
        """Get a TEXT column configuration by name.

        Args:
            name: Column name to find

        Returns:
            TextColumnConfig if found, None otherwise
        """
        for config in self.text_columns:
            if config.column_name == name:
                return config
        return None

    def get_column(self, name: str) -> Optional[ColumnDefinition]:
        """Get a column definition by name.

        Args:
            name: Column name to find

        Returns:
            ColumnDefinition if found, None otherwise
        """
        for col in self.columns:
            if col.name == name:
                return col
        return None

    def validate_record(self, record: ProximaRecord) -> List[str]:
        """Validate a record against this schema.

        Args:
            record: ProximaRecord to validate

        Returns:
            List of validation error messages (empty if valid)
        """
        errors = []

        if self.enforcement == SchemaEnforcement.FLEXIBLE:
            return errors  # No validation in flexible mode

        # Check required columns
        for col in self.columns:
            if not col.nullable:
                if col.name not in record.typed_fields:
                    if col.name not in record.flexible_fields:
                        errors.append(f"Required column '{col.name}' is missing")

        # Check typed field types
        for name, typed_value in record.typed_fields.items():
            col = self.get_column(name)
            if col is not None:
                if typed_value.value_type != col.data_type:
                    errors.append(
                        f"Column '{name}' has type {typed_value.value_type.value}, "
                        f"expected {col.data_type.value}"
                    )
            elif self.enforcement == SchemaEnforcement.STRICT:
                errors.append(f"Unknown column '{name}' in strict mode")

        # Check flexible fields in strict mode
        if self.enforcement == SchemaEnforcement.STRICT:
            for name in record.flexible_fields:
                if self.get_column(name) is None:
                    errors.append(f"Unknown column '{name}' in strict mode")

        return errors


# ============================================================================
# FILTER DSL
# ============================================================================


class FilterOperator(str, Enum):
    """Filter comparison operators for v2 typed filters.

    Example:
        condition = TypedFilterCondition(
            field_name="price",
            operator=FilterOperator.GTE,
            value=10.0
        )
    """

    EQ = "eq"
    NE = "ne"
    GT = "gt"
    GTE = "gte"
    LT = "lt"
    LTE = "lte"
    CONTAINS = "contains"
    STARTS_WITH = "starts_with"
    ENDS_WITH = "ends_with"
    BETWEEN = "between"
    IN = "in"
    IS_NULL = "is_null"
    IS_NOT_NULL = "is_not_null"


class TypedFilterCondition(BaseModel):
    """A single typed filter condition.

    Represents a filter condition with field name, operator, and value(s).

    Attributes:
        field_name: Name of the field to filter on
        operator: Comparison operator
        value: Value to compare against
        value_upper: Upper bound for BETWEEN operator

    Example:
        # Simple equality
        condition = TypedFilterCondition(
            field_name="category",
            operator=FilterOperator.EQ,
            value="electronics"
        )

        # Range filter
        condition = TypedFilterCondition(
            field_name="price",
            operator=FilterOperator.BETWEEN,
            value=10.0,
            value_upper=100.0
        )
    """

    model_config = ConfigDict(populate_by_name=True)

    field_name: str = Field(..., description="Field name to filter on")
    operator: FilterOperator = Field(..., description="Comparison operator")
    value: Any = Field(..., description="Value to compare against")
    value_upper: Optional[Any] = Field(
        default=None, description="Upper bound for BETWEEN"
    )


class FilterBuilderV2:
    """Fluent builder for typed filters (v2 API).

    Provides a chainable API for building complex filter expressions
    with type-safe operators.

    Example:
        # Simple filter
        filters = FilterBuilderV2("category").eq("electronics").build()

        # Multiple conditions
        filters = (FilterBuilderV2("price")
            .gte(10.0)
            .and_("category")
            .eq("electronics")
            .and_("in_stock")
            .eq(True)
            .build())

        # Range filter
        filters = FilterBuilderV2("price").between(10.0, 100.0).build()

        # IN filter
        filters = FilterBuilderV2("status").in_(["active", "pending"]).build()
    """

    def __init__(self, field_name: str):
        """Initialize filter builder with first field name.

        Args:
            field_name: Initial field to filter on
        """
        self._conditions: List[TypedFilterCondition] = []
        self._current_field = field_name

    def eq(self, value: Any) -> "FilterBuilderV2":
        """Add equality condition.

        Args:
            value: Value to compare for equality

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.EQ, value=value
            )
        )
        return self

    def ne(self, value: Any) -> "FilterBuilderV2":
        """Add not-equal condition.

        Args:
            value: Value to compare for inequality

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.NE, value=value
            )
        )
        return self

    def gt(self, value: Any) -> "FilterBuilderV2":
        """Add greater-than condition.

        Args:
            value: Value to compare against

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.GT, value=value
            )
        )
        return self

    def gte(self, value: Any) -> "FilterBuilderV2":
        """Add greater-than-or-equal condition.

        Args:
            value: Value to compare against

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.GTE, value=value
            )
        )
        return self

    def lt(self, value: Any) -> "FilterBuilderV2":
        """Add less-than condition.

        Args:
            value: Value to compare against

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.LT, value=value
            )
        )
        return self

    def lte(self, value: Any) -> "FilterBuilderV2":
        """Add less-than-or-equal condition.

        Args:
            value: Value to compare against

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.LTE, value=value
            )
        )
        return self

    def contains(self, value: str) -> "FilterBuilderV2":
        """Add contains condition for text fields.

        Args:
            value: Substring to search for

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.CONTAINS,
                value=value,
            )
        )
        return self

    def starts_with(self, value: str) -> "FilterBuilderV2":
        """Add starts-with condition for text fields.

        Args:
            value: Prefix to match

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.STARTS_WITH,
                value=value,
            )
        )
        return self

    def ends_with(self, value: str) -> "FilterBuilderV2":
        """Add ends-with condition for text fields.

        Args:
            value: Suffix to match

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.ENDS_WITH,
                value=value,
            )
        )
        return self

    def between(self, lower: Any, upper: Any) -> "FilterBuilderV2":
        """Add between condition (inclusive).

        Args:
            lower: Lower bound (inclusive)
            upper: Upper bound (inclusive)

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.BETWEEN,
                value=lower,
                value_upper=upper,
            )
        )
        return self

    def in_(self, values: List[Any]) -> "FilterBuilderV2":
        """Add IN condition.

        Args:
            values: List of values to match

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field, operator=FilterOperator.IN, value=values
            )
        )
        return self

    def is_null(self) -> "FilterBuilderV2":
        """Add IS NULL condition.

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.IS_NULL,
                value=None,
            )
        )
        return self

    def is_not_null(self) -> "FilterBuilderV2":
        """Add IS NOT NULL condition.

        Returns:
            Self for method chaining
        """
        self._conditions.append(
            TypedFilterCondition(
                field_name=self._current_field,
                operator=FilterOperator.IS_NOT_NULL,
                value=None,
            )
        )
        return self

    def and_(self, field_name: str) -> "FilterBuilderV2":
        """Chain to next field (AND logic).

        Args:
            field_name: Next field to filter on

        Returns:
            Self for method chaining
        """
        self._current_field = field_name
        return self

    def build(self) -> List[TypedFilterCondition]:
        """Build and return the filter conditions.

        Returns:
            List of TypedFilterCondition objects
        """
        return self._conditions

    def to_dict(self) -> List[Dict[str, Any]]:
        """Convert to list of dictionaries for API serialization.

        Returns:
            List of condition dictionaries
        """
        return [c.model_dump(exclude_none=True) for c in self._conditions]


# ============================================================================
# SEARCH REQUEST
# ============================================================================


class SearchRequestV2(BaseModel):
    """Search request for v2 API with typed filters.

    Provides a structured search request with support for typed filters,
    text inclusion, and performance hints.

    Attributes:
        vector: Query vector
        top_k: Number of results to return
        filters: List of typed filter conditions
        include_text: Whether to include text fields in results
        include_vectors: Whether to include vectors in results
        ef_search: HNSW ef_search parameter for accuracy/speed tradeoff

    Example:
        # Simple search
        request = SearchRequestV2.create([0.1, 0.2, 0.3], top_k=10)

        # Search with filters
        request = (SearchRequestV2.create([0.1, 0.2, 0.3], top_k=10)
            .with_filter(FilterBuilderV2("category").eq("electronics"))
            .with_text())
    """

    model_config = ConfigDict(populate_by_name=True)

    vector: List[float] = Field(..., description="Query vector")
    top_k: int = Field(default=10, ge=1, description="Number of results to return")
    filters: List[TypedFilterCondition] = Field(
        default_factory=list, description="Typed filter conditions"
    )
    include_text: bool = Field(
        default=False, description="Include text fields in results"
    )
    include_vectors: bool = Field(
        default=False, description="Include vectors in results"
    )
    ef_search: Optional[int] = Field(
        default=None, ge=1, description="HNSW ef_search parameter"
    )

    @classmethod
    def create(cls, vector: List[float], top_k: int = 10) -> "SearchRequestV2":
        """Factory method to create a search request.

        Args:
            vector: Query vector
            top_k: Number of results (default 10)

        Returns:
            New SearchRequestV2 instance
        """
        return cls(vector=vector, top_k=top_k)

    def with_filter(self, builder: FilterBuilderV2) -> "SearchRequestV2":
        """Add filters from a FilterBuilderV2.

        Args:
            builder: FilterBuilderV2 with conditions

        Returns:
            Self for method chaining
        """
        self.filters.extend(builder.build())
        return self

    def with_filters(self, conditions: List[TypedFilterCondition]) -> "SearchRequestV2":
        """Add filter conditions directly.

        Args:
            conditions: List of TypedFilterCondition

        Returns:
            Self for method chaining
        """
        self.filters.extend(conditions)
        return self

    def with_text(self) -> "SearchRequestV2":
        """Include text fields in results.

        Returns:
            Self for method chaining
        """
        self.include_text = True
        return self

    def with_vectors(self) -> "SearchRequestV2":
        """Include vectors in results.

        Returns:
            Self for method chaining
        """
        self.include_vectors = True
        return self

    def with_ef_search(self, ef_search: int) -> "SearchRequestV2":
        """Set HNSW ef_search parameter.

        Higher values give better accuracy but slower search.

        Args:
            ef_search: ef_search parameter (typically 10-500)

        Returns:
            Self for method chaining
        """
        self.ef_search = ef_search
        return self


# Backward compatibility alias
FilterBuilder = FilterBuilderV2


# ============================================================================
# CONVENIENCE FUNCTIONS FOR TEXT COLUMNS
# ============================================================================


def create_text_column_schema(
    text_columns: List[TextColumnConfig],
    additional_columns: Optional[List[ColumnDefinition]] = None,
    enforcement: SchemaEnforcement = SchemaEnforcement.HYBRID,
) -> RecordSchema:
    """Create a RecordSchema with TEXT column configurations.

    Convenience function to create a schema with TEXT columns that have
    specific storage strategies (INLINE, CHUNKED, SIDECAR, ADAPTIVE).

    Args:
        text_columns: List of TextColumnConfig objects defining TEXT columns
        additional_columns: Optional list of additional ColumnDefinition objects
        enforcement: Schema enforcement mode (default HYBRID)

    Returns:
        RecordSchema with the specified TEXT columns

    Example:
        from proximadb_sdk import (
            create_text_column_schema,
            TextColumnConfig,
            TextStorageStrategy
        )

        # Create a schema for a document collection
        schema = create_text_column_schema([
            TextColumnConfig.for_rag("content", chunk_size=512),
            TextColumnConfig.for_short_text("title"),
            TextColumnConfig.for_large_documents("full_pdf")
        ])

        # Use with collection creation
        client.create_collection(
            "documents",
            dimension=768,
            schema=schema
        )
    """
    schema = RecordSchema(enforcement=enforcement)

    # Add additional columns first
    if additional_columns:
        for col in additional_columns:
            schema.columns.append(col)

    # Add TEXT column configurations
    for text_config in text_columns:
        schema.add_text_column_config(text_config)

    return schema


def text_column(
    name: str, strategy: TextStorageStrategy = TextStorageStrategy.ADAPTIVE, **kwargs
) -> TextColumnConfig:
    """Create a TEXT column configuration with a simple function call.

    This is a convenience function that provides a simpler alternative
    to the TextColumnConfig constructor for common use cases.

    Args:
        name: Column name
        strategy: Storage strategy (default ADAPTIVE)
        **kwargs: Additional TextColumnConfig options

    Returns:
        TextColumnConfig object

    Example:
        from proximadb_sdk import text_column, TextStorageStrategy

        # Simple adaptive column
        config = text_column("description")

        # Chunked column for RAG
        config = text_column(
            "content",
            strategy=TextStorageStrategy.CHUNKED,
            chunk_size=512,
            enable_ngram_bloom=True
        )
    """
    return TextColumnConfig(column_name=name, strategy=strategy, **kwargs)
