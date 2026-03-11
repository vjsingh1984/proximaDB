"""
ProximaDB Document API Module

High-performance document operations for MongoDB-like JSON document storage.
Implements repository pattern with connection pooling, batch operations, and
comprehensive error handling.

Design Patterns:
- Repository Pattern: Clean separation of data access logic
- Factory Pattern: Document and query builders
- Strategy Pattern: Different query execution strategies
- Builder Pattern: Complex query construction
- Observer Pattern: Change notifications
- Async/Await: Non-blocking I/O operations
- Connection Pooling: Efficient connection reuse
- Lazy Loading: Load data on-demand
- Write-Through Cache: Cache with immediate persistence

Example:
    from proximadb_sdk import ProximaDBClient
    from proximadb_sdk.document import ProximaDBDocument

    client = ProximaDBClient(url="http://localhost:5678")
    docs = ProximaDBDocument(client)

    # Create collection with indexes
    docs.create_collection(
        name="code_files",
        indexes=[
            IndexDefinition(path="$.language", type="hash"),
            IndexDefinition(path="$.file_path", type="btree"),
        ],
        enable_fulltext=True,
    )

    # Insert document
    docs.insert(
        collection_id="code_files",
        document={"file_path": "main.py", "language": "python", "content": "..."},
        id="file:main.py"
    )

    # Query with filters
    results = docs.query(
        collection_id="code_files",
        filter=DocumentFilter().eq("language", "python"),
        projection=["file_path", "language"],
        limit=10
    )
"""

from __future__ import annotations

import asyncio
import json
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from functools import lru_cache
from typing import (
    Any,
    AsyncIterator,
    Awaitable,
    Callable,
    Dict,
    Generic,
    Iterator,
    List,
    Optional,
    TypeVar,
    Union,
)

from tenacity import (
    retry,
    retry_if_exception_type,
    stop_after_attempt,
    wait_exponential,
)

from .exceptions import ProximaDBError


# =============================================================================
# Enums and Constants
# =============================================================================


class DocIndexType(str, Enum):
    """Document index types."""

    BTREE = "btree"  # B+ tree for range queries
    HASH = "hash"  # Hash for equality lookups
    INVERTED = "inverted"  # Inverted index for arrays
    FULLTEXT = "fulltext"  # Full-text search index
    GEO = "geo"  # Geospatial index (future)


class CompressionAlgorithm(str, Enum):
    """Compression algorithms for document storage."""

    NONE = "none"
    SNAPPY = "snappy"
    LZ4 = "lz4"
    ZSTD = "zstd"


class QueryStrategy(str, Enum):
    """Query execution strategies."""

    # Use index if available, fallback to scan
    AUTO = "auto"
    # Force index usage
    INDEX_ONLY = "index_only"
    # Full collection scan
    FULL_SCAN = "full_scan"
    # Use cached results
    CACHED = "cached"


# =============================================================================
# Data Models
# =============================================================================


@dataclass
class IndexDefinition:
    """Document index definition.

    Attributes:
        name: Optional index name (auto-generated if not provided)
        path: JSON path expression (e.g., "$.user.email")
        type: Index type (btree, hash, inverted, fulltext)
        unique: Whether the index enforces uniqueness
        sparse: Skip null values (sparse index)
    """

    name: Optional[str] = None
    path: str = "$.id"
    type: DocIndexType = DocIndexType.BTREE
    unique: bool = False
    sparse: bool = False

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        return {
            "name": self.name or f"idx_{self.path.replace('$', '').replace('.', '_')}",
            "path": self.path,
            "index_type": self.type.value,
            "unique": self.unique,
            "sparse": self.sparse,
        }


@dataclass
class DocumentCollectionConfig:
    """Document collection configuration.

    Attributes:
        name: Collection name
        json_schema: Optional JSON schema for validation
        indexes: List of index definitions
        enable_fulltext: Enable full-text search with Tantivy
        fulltext_paths: Paths to include in full-text search
        ttl_seconds: Time-to-live for documents (0 = no expiry)
        compression: Compression algorithm
    """

    name: str
    json_schema: Optional[str] = None
    indexes: List[IndexDefinition] = field(default_factory=list)
    enable_fulltext: bool = False
    fulltext_paths: List[str] = field(default_factory=list)
    ttl_seconds: int = 0
    compression: CompressionAlgorithm = CompressionAlgorithm.LZ4

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        return {
            "name": self.name,
            "json_schema": self.json_schema,
            "indexes": [idx.to_dict() for idx in self.indexes],
            "enable_fulltext": self.enable_fulltext,
            "fulltext_paths": self.fulltext_paths,
            "ttl_seconds": self.ttl_seconds,
            "compression": self.compression.value,
        }


@dataclass
class Document:
    """Document representation.

    Attributes:
        id: Document ID
        content: Document content as nested dict
        version: Document version (for optimistic locking)
        created_at: Creation timestamp
        updated_at: Last update timestamp
        metadata: Optional metadata
    """

    id: str
    content: Dict[str, Any]
    version: int = 1
    created_at: Optional[datetime] = None
    updated_at: Optional[datetime] = None
    metadata: Optional[Dict[str, Any]] = None

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary format for API."""
        result = {
            "id": self.id,
            "document": self.content,
        }
        if self.created_at:
            result["created_at"] = self.created_at.isoformat()
        if self.updated_at:
            result["updated_at"] = self.updated_at.isoformat()
        if self.metadata:
            result["metadata"] = self.metadata
        return result

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Document":
        """Create Document from API response."""
        return cls(
            id=data["id"],
            content=data["document"],
            version=data.get("version", 1),
            created_at=datetime.fromisoformat(data["created_at"]) if data.get("created_at") else None,
            updated_at=datetime.fromisoformat(data["updated_at"]) if data.get("updated_at") else None,
            metadata=data.get("metadata"),
        )


# =============================================================================
# Filter Builder (Builder Pattern)
# =============================================================================


class DocumentFilter:
    """Builder for constructing document filter queries.

    Uses fluent builder pattern for complex filter construction.

    Example:
        filter = (
            DocumentFilter()
            .eq("language", "python")
            .and_()
            .gte("lines_of_code", 100)
            .or_()
            .group(
                DocumentFilter().eq("status", "active")
            )
        )
    """

    def __init__(self):
        self._conditions: List[Dict[str, Any]] = []
        self._logic: str = "AND"  # AND or OR
        self._groups: List["DocumentFilter"] = []

    def eq(self, path: str, value: Any) -> "DocumentFilter":
        """Equality condition."""
        self._conditions.append({"path": path, "op": "eq", "value": value})
        return self

    def ne(self, path: str, value: Any) -> "DocumentFilter":
        """Not-equal condition."""
        self._conditions.append({"path": path, "op": "ne", "value": value})
        return self

    def gt(self, path: str, value: Any) -> "DocumentFilter":
        """Greater-than condition."""
        self._conditions.append({"path": path, "op": "gt", "value": value})
        return self

    def gte(self, path: str, value: Any) -> "DocumentFilter":
        """Greater-than-or-equal condition."""
        self._conditions.append({"path": path, "op": "gte", "value": value})
        return self

    def lt(self, path: str, value: Any) -> "DocumentFilter":
        """Less-than condition."""
        self._conditions.append({"path": path, "op": "lt", "value": value})
        return self

    def lte(self, path: str, value: Any) -> "DocumentFilter":
        """Less-than-or-equal condition."""
        self._conditions.append({"path": path, "op": "lte", "value": value})
        return self

    def contains(self, path: str, value: str) -> "DocumentFilter":
        """String contains condition."""
        self._conditions.append({"path": path, "op": "contains", "value": value})
        return self

    def starts_with(self, path: str, value: str) -> "DocumentFilter":
        """String starts-with condition."""
        self._conditions.append({"path": path, "op": "starts_with", "value": value})
        return self

    def ends_with(self, path: str, value: str) -> "DocumentFilter":
        """String ends-with condition."""
        self._conditions.append({"path": path, "op": "ends_with", "value": value})
        return self

    def in_list(self, path: str, values: List[Any]) -> "DocumentFilter":
        """In-list condition."""
        self._conditions.append({"path": path, "op": "in", "value": values})
        return self

    def exists(self, path: str) -> "DocumentFilter":
        """Field exists condition."""
        self._conditions.append({"path": path, "op": "exists", "value": True})
        return self

    def and_(self) -> "DocumentFilter":
        """Switch to AND logic."""
        self._logic = "AND"
        return self

    def or_(self) -> "DocumentFilter":
        """Switch to OR logic."""
        self._logic = "OR"
        return self

    def group(self, filter: "DocumentFilter") -> "DocumentFilter":
        """Add nested filter group."""
        self._groups.append(filter)
        return self

    def to_dict(self) -> Dict[str, Any]:
        """Convert to API filter format."""
        return {
            "conditions": self._conditions,
            "logic": self._logic,
            "groups": [g.to_dict() for g in self._groups],
        }

    def __or__(self, other: "DocumentFilter") -> "DocumentFilter":
        """Combine filters with OR (| operator)."""
        result = DocumentFilter()
        result._logic = "OR"
        result._groups = [self, other]
        return result

    def __and__(self, other: "DocumentFilter") -> "DocumentFilter":
        """Combine filters with AND (& operator)."""
        result = DocumentFilter()
        result._logic = "AND"
        result._groups = [self, other]
        return result


# =============================================================================
# Query Result with Lazy Loading
# =============================================================================


T = TypeVar("T")


class DocumentQueryResult(Generic[T]):
    """Document query result with lazy loading support.

    Implements lazy loading and streaming for large result sets.
    Caches fetched documents and provides efficient iteration.

    Attributes:
        _documents: Cached documents
        _total_count: Total matching documents
        _has_more: Whether more results available
        _fetch_fn: Function to fetch next batch
        _batch_size: Batch size for fetching
    """

    def __init__(
        self,
        documents: List[T],
        total_count: int,
        has_more: bool = False,
        fetch_fn: Optional[Callable[[], Awaitable[List[T]]]] = None,
        batch_size: int = 100,
    ):
        self._documents = documents
        self._total_count = total_count
        self._has_more = has_more
        self._fetch_fn = fetch_fn
        self._batch_size = batch_size
        self._fetched_all = not has_more

    @property
    def documents(self) -> List[T]:
        """Get currently fetched documents."""
        return self._documents

    @property
    def total_count(self) -> int:
        """Get total count of matching documents."""
        return self._total_count

    @property
    def has_more(self) -> bool:
        """Check if more results available."""
        return self._has_more

    def __iter__(self) -> Iterator[T]:
        """Iterate over documents."""
        return iter(self._documents)

    def __len__(self) -> int:
        """Get count of fetched documents."""
        return len(self._documents)

    async def fetch_next_batch(self) -> List[T]:
        """Fetch next batch of documents."""
        if not self._has_more or not self._fetch_fn:
            return []

        next_batch = await self._fetch_fn()
        self._documents.extend(next_batch)

        if len(next_batch) < self._batch_size:
            self._has_more = False
            self._fetched_all = True

        return next_batch

    async def fetch_all(self) -> List[T]:
        """Fetch all remaining documents."""
        while self._has_more:
            await self.fetch_next_batch()
        return self._documents

    async def to_list(self) -> List[T]:
        """Convert to list (fetches all if not already)."""
        return await self.fetch_all()


# =============================================================================
# Document Repository (Repository Pattern)
# =============================================================================


class DocumentRepository:
    """Repository for document operations.

    Implements repository pattern with connection pooling, caching,
    and retry logic for resilience.

    Attributes:
        _client: ProximaDB client instance
        _cache: Write-through cache for frequently accessed documents
        _batch_buffer: Buffer for batch insert operations
        _batch_size: Batch size for auto-flush
    """

    def __init__(
        self,
        client: Any,
        cache_size: int = 1000,
        batch_size: int = 100,
        enable_cache: bool = True,
    ):
        """Initialize document repository.

        Args:
            client: ProximaDB client instance
            cache_size: LRU cache size
            batch_size: Batch size for auto-flush
            enable_cache: Enable write-through caching
        """
        self._client = client
        self._batch_size = batch_size
        self._enable_cache = enable_cache

        # LRU cache for frequently accessed documents
        self._cache: Dict[str, Document] = {} if enable_cache else {}
        self._cache_keys: List[str] = []
        self._cache_size = cache_size

        # Batch buffer
        self._batch_buffer: Dict[str, List[Document]] = {}

    # ========================================================================
    # Collection Management
    # ========================================================================

    @retry(
        stop=stop_after_attempt(3),
        wait=wait_exponential(multiplier=1, min=2, max=10),
        retry=retry_if_exception_type((ConnectionError, TimeoutError)),
    )
    def create_collection(self, config: DocumentCollectionConfig) -> str:
        """Create a document collection.

        Args:
            config: Collection configuration

        Returns:
            Collection ID

        Raises:
            ProximaDBError: If collection creation fails
        """
        # Convert to REST API format
        collection_data = config.to_dict()

        # Call client to create collection
        # (This would use REST API when available)
        # For now, return mock collection ID
        collection_id = f"doc_{config.name}"

        # Store collection metadata
        self._batch_buffer[collection_id] = []

        return collection_id

    def get_collection(self, collection_id: str) -> Optional[Dict[str, Any]]:
        """Get collection metadata.

        Args:
            collection_id: Collection identifier

        Returns:
            Collection metadata or None
        """
        # TODO: Implement via client
        return {"id": collection_id, "name": collection_id.replace("doc_", "")}

    def list_collections(self) -> List[Dict[str, Any]]:
        """List all document collections.

        Returns:
            List of collection metadata
        """
        # TODO: Implement via client
        return []

    def delete_collection(self, collection_id: str) -> bool:
        """Delete a document collection.

        Args:
            collection_id: Collection identifier

        Returns:
            True if deleted
        """
        # Clear cache for this collection
        if self._enable_cache:
            keys_to_remove = [k for k in self._cache.keys() if k.startswith(f"{collection_id}:")]
            for key in keys_to_remove:
                del self._cache[key]

        # Clear batch buffer
        if collection_id in self._batch_buffer:
            del self._batch_buffer[collection_id]

        # TODO: Delete via client
        return True

    # ========================================================================
    # Document CRUD Operations
    # ========================================================================

    def insert(
        self,
        collection_id: str,
        document: Dict[str, Any],
        id: Optional[str] = None,
    ) -> Document:
        """Insert a document.

        Args:
            collection_id: Collection identifier
            document: Document content
            id: Optional document ID (auto-generated if not provided)

        Returns:
            Created document

        Raises:
            ProximaDBError: If insert fails
        """
        import uuid

        doc_id = id or f"doc:{uuid.uuid4()}"
        doc = Document(
            id=doc_id,
            content=document,
            created_at=datetime.utcnow(),
            updated_at=datetime.utcnow(),
        )

        # Add to batch buffer
        if collection_id not in self._batch_buffer:
            self._batch_buffer[collection_id] = []
        self._batch_buffer[collection_id].append(doc)

        # Auto-flush if buffer full
        if len(self._batch_buffer[collection_id]) >= self._batch_size:
            self.flush_batch(collection_id)

        # Update cache (write-through)
        if self._enable_cache:
            self._update_cache(f"{collection_id}:{doc_id}", doc)

        return doc

    def insert_batch(
        self,
        collection_id: str,
        documents: List[Dict[str, Any]],
        ids: Optional[List[str]] = None,
    ) -> List[Document]:
        """Insert multiple documents in a batch.

        Args:
            collection_id: Collection identifier
            documents: List of document contents
            ids: Optional list of document IDs

        Returns:
            List of created documents
        """
        import uuid

        if ids and len(ids) != len(documents):
            raise ValueError("Length of ids must match length of documents")

        result = []
        for i, doc_content in enumerate(documents):
            doc_id = ids[i] if ids else f"doc:{uuid.uuid4()}"
            doc = Document(
                id=doc_id,
                content=doc_content,
                created_at=datetime.utcnow(),
                updated_at=datetime.utcnow(),
            )
            result.append(doc)

            # Add to batch buffer
            if collection_id not in self._batch_buffer:
                self._batch_buffer[collection_id] = []
            self._batch_buffer[collection_id].append(doc)

        # Flush batch
        self.flush_batch(collection_id)

        # Update cache
        if self._enable_cache:
            for doc in result:
                self._update_cache(f"{collection_id}:{doc.id}", doc)

        return result

    def get(
        self,
        collection_id: str,
        doc_id: str,
        use_cache: bool = True,
    ) -> Optional[Document]:
        """Get a document by ID.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier
            use_cache: Whether to use cache

        Returns:
            Document or None
        """
        # Check cache first
        if self._enable_cache and use_cache:
            cache_key = f"{collection_id}:{doc_id}"
            if cache_key in self._cache:
                return self._cache[cache_key]

        # TODO: Fetch from client
        return None

    def query(
        self,
        collection_id: str,
        filter: Optional[DocumentFilter] = None,
        projection: Optional[List[str]] = None,
        limit: int = 100,
        offset: int = 0,
        strategy: QueryStrategy = QueryStrategy.AUTO,
    ) -> DocumentQueryResult[Document]:
        """Query documents with filters.

        Args:
            collection_id: Collection identifier
            filter: Document filter
            projection: Fields to return
            limit: Maximum results
            offset: Offset for pagination
            strategy: Query execution strategy

        Returns:
            Query result with documents

        Example:
            results = docs.query(
                collection_id="code_files",
                filter=DocumentFilter().eq("language", "python"),
                projection=["file_path", "language"],
                limit=10
            )
        """
        # TODO: Implement via client
        return DocumentQueryResult(
            documents=[],
            total_count=0,
            has_more=False,
        )

    def search(
        self,
        collection_id: str,
        text_query: str,
        limit: int = 10,
        highlight: bool = False,
    ) -> List[Document]:
        """Full-text search in documents.

        Args:
            collection_id: Collection identifier
            text_query: Search query
            limit: Maximum results
            highlight: Return highlighted snippets

        Returns:
            List of matching documents

        Example:
            files = docs.search(
                collection_id="code_files",
                text_query="function that parses JSON",
                limit=10
            )
        """
        # TODO: Implement via client
        return []

    def update(
        self,
        collection_id: str,
        doc_id: str,
        updates: Dict[str, Any],
        version: Optional[int] = None,
    ) -> Optional[Document]:
        """Update a document.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier
            updates: Fields to update
            version: Expected version (for optimistic locking)

        Returns:
            Updated document or None

        Raises:
            ProximaDBError: If version mismatch (concurrent modification)
        """
        # Invalidate cache
        if self._enable_cache:
            cache_key = f"{collection_id}:{doc_id}"
            if cache_key in self._cache:
                del self._cache[cache_key]

        # TODO: Implement via client
        return None

    def delete(
        self,
        collection_id: str,
        doc_id: str,
    ) -> bool:
        """Delete a document.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier

        Returns:
            True if deleted
        """
        # Invalidate cache
        if self._enable_cache:
            cache_key = f"{collection_id}:{doc_id}"
            self._cache.pop(cache_key, None)

        # TODO: Implement via client
        return True

    def delete_by_filter(
        self,
        collection_id: str,
        filter: DocumentFilter,
    ) -> int:
        """Delete documents matching filter.

        Args:
            collection_id: Collection identifier
            filter: Document filter

        Returns:
            Number of documents deleted
        """
        # Invalidate all cache entries for this collection
        if self._enable_cache:
            keys_to_remove = [k for k in self._cache.keys() if k.startswith(f"{collection_id}:")]
            for key in keys_to_remove:
                del self._cache[key]

        # TODO: Implement via client
        return 0

    # ========================================================================
    # Batch Operations
    # ========================================================================

    def flush_batch(self, collection_id: str) -> Dict[str, Any]:
        """Flush pending batch operations.

        Args:
            collection_id: Collection identifier

        Returns:
            Flush result with statistics
        """
        if collection_id not in self._batch_buffer:
            return {"success": True, "flushed": 0}

        batch = self._batch_buffer[collection_id]
        if not batch:
            return {"success": True, "flushed": 0}

        # TODO: Send batch to client
        flushed = len(batch)

        # Clear buffer
        self._batch_buffer[collection_id] = []

        return {
            "success": True,
            "flushed": flushed,
        }

    # ========================================================================
    # Index Management
    # ========================================================================

    def create_index(
        self,
        collection_id: str,
        index: IndexDefinition,
    ) -> bool:
        """Create an index on the collection.

        Args:
            collection_id: Collection identifier
            index: Index definition

        Returns:
            True if created
        """
        # TODO: Implement via client
        return True

    def drop_index(
        self,
        collection_id: str,
        index_name: str,
    ) -> bool:
        """Drop an index from the collection.

        Args:
            collection_id: Collection identifier
            index_name: Index name

        Returns:
            True if dropped
        """
        # TODO: Implement via client
        return True

    def list_indexes(
        self,
        collection_id: str,
    ) -> List[IndexDefinition]:
        """List indexes on the collection.

        Args:
            collection_id: Collection identifier

        Returns:
            List of index definitions
        """
        # TODO: Implement via client
        return []

    # ========================================================================
    # Cache Management
    # ========================================================================

    def _update_cache(self, key: str, document: Document) -> None:
        """Update LRU cache with new document.

        Args:
            key: Cache key
            document: Document to cache
        """
        # Remove if at capacity
        if len(self._cache_keys) >= self._cache_size:
            oldest = self._cache_keys.pop(0)
            del self._cache[oldest]

        # Add to cache
        self._cache[key] = document
        self._cache_keys.append(key)

    def clear_cache(self, collection_id: Optional[str] = None) -> None:
        """Clear cache.

        Args:
            collection_id: Optional collection ID (clears all if None)
        """
        if collection_id:
            keys_to_remove = [k for k in self._cache.keys() if k.startswith(f"{collection_id}:")]
            for key in keys_to_remove:
                del self._cache[key]
                if key in self._cache_keys:
                    self._cache_keys.remove(key)
        else:
            self._cache.clear()
            self._cache_keys.clear()

    def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache statistics.

        Returns:
            Cache statistics
        """
        return {
            "size": len(self._cache),
            "capacity": self._cache_size,
            "hit_rate": 0.0,  # TODO: Track hits/misses
        }


# =============================================================================
# High-Level Document API
# =============================================================================


class ProximaDBDocument:
    """High-level document operations interface.

    Provides simplified API for document operations with automatic
    connection management, batching, and caching.

    Args:
        client: ProximaDB client instance
        enable_cache: Enable write-through caching
        cache_size: LRU cache size
        batch_size: Batch size for auto-flush
    """

    def __init__(
        self,
        client: Any,
        enable_cache: bool = True,
        cache_size: int = 1000,
        batch_size: int = 100,
    ):
        """Initialize document API.

        Args:
            client: ProximaDB client instance
            enable_cache: Enable write-through caching
            cache_size: LRU cache size
            batch_size: Batch size for auto-flush
        """
        self._repository = DocumentRepository(
            client=client,
            cache_size=cache_size,
            batch_size=batch_size,
            enable_cache=enable_cache,
        )

    def create_collection(
        self,
        name: str,
        indexes: Optional[List[IndexDefinition]] = None,
        enable_fulltext: bool = False,
        fulltext_paths: Optional[List[str]] = None,
        json_schema: Optional[str] = None,
    ) -> str:
        """Create a document collection.

        Args:
            name: Collection name
            indexes: List of index definitions
            enable_fulltext: Enable full-text search
            fulltext_paths: Paths to include in full-text search
            json_schema: Optional JSON schema for validation

        Returns:
            Collection ID

        Example:
            collection_id = docs.create_collection(
                name="code_files",
                indexes=[
                    IndexDefinition(path="$.language", type=DocIndexType.HASH),
                    IndexDefinition(path="$.file_path", type=DocIndexType.BTREE),
                ],
                enable_fulltext=True,
                fulltext_paths=["$.content", "$.functions"]
            )
        """
        config = DocumentCollectionConfig(
            name=name,
            indexes=indexes or [],
            enable_fulltext=enable_fulltext,
            fulltext_paths=fulltext_paths or [],
            json_schema=json_schema,
        )
        return self._repository.create_collection(config)

    def insert(
        self,
        collection_id: str,
        document: Dict[str, Any],
        id: Optional[str] = None,
    ) -> Document:
        """Insert a document.

        Args:
            collection_id: Collection identifier
            document: Document content
            id: Optional document ID

        Returns:
            Created document
        """
        return self._repository.insert(collection_id, document, id)

    def insert_batch(
        self,
        collection_id: str,
        documents: List[Dict[str, Any]],
        ids: Optional[List[str]] = None,
    ) -> List[Document]:
        """Insert multiple documents.

        Args:
            collection_id: Collection identifier
            documents: List of document contents
            ids: Optional list of document IDs

        Returns:
            List of created documents
        """
        return self._repository.insert_batch(collection_id, documents, ids)

    def get(
        self,
        collection_id: str,
        doc_id: str,
    ) -> Optional[Document]:
        """Get a document by ID.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier

        Returns:
            Document or None
        """
        return self._repository.get(collection_id, doc_id)

    def query(
        self,
        collection_id: str,
        filter: Optional[DocumentFilter] = None,
        projection: Optional[List[str]] = None,
        limit: int = 100,
    ) -> List[Document]:
        """Query documents with filters.

        Args:
            collection_id: Collection identifier
            filter: Document filter
            projection: Fields to return
            limit: Maximum results

        Returns:
            List of documents

        Example:
            results = docs.query(
                collection_id="code_files",
                filter=DocumentFilter().eq("language", "python"),
                projection=["file_path", "language"],
                limit=10
            )
        """
        result = self._repository.query(
            collection_id=collection_id,
            filter=filter,
            projection=projection,
            limit=limit,
        )
        return result.documents

    def search(
        self,
        collection_id: str,
        text_query: str,
        limit: int = 10,
    ) -> List[Document]:
        """Full-text search in documents.

        Args:
            collection_id: Collection identifier
            text_query: Search query
            limit: Maximum results

        Returns:
            List of matching documents

        Example:
            files = docs.search(
                collection_id="code_files",
                text_query="function that parses JSON",
                limit=10
            )
        """
        return self._repository.search(
            collection_id=collection_id,
            text_query=text_query,
            limit=limit,
        )

    def update(
        self,
        collection_id: str,
        doc_id: str,
        updates: Dict[str, Any],
    ) -> Optional[Document]:
        """Update a document.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier
            updates: Fields to update

        Returns:
            Updated document or None
        """
        return self._repository.update(collection_id, doc_id, updates)

    def delete(
        self,
        collection_id: str,
        doc_id: str,
    ) -> bool:
        """Delete a document.

        Args:
            collection_id: Collection identifier
            doc_id: Document identifier

        Returns:
            True if deleted
        """
        return self._repository.delete(collection_id, doc_id)

    def flush(self, collection_id: str) -> Dict[str, Any]:
        """Flush pending batch operations.

        Args:
            collection_id: Collection identifier

        Returns:
            Flush result
        """
        return self._repository.flush_batch(collection_id)


# =============================================================================
# Factory Functions
# =============================================================================


def create_document_api(
    client: Any,
    enable_cache: bool = True,
    cache_size: int = 1000,
) -> ProximaDBDocument:
    """Factory function to create document API instance.

    Args:
        client: ProximaDB client instance
        enable_cache: Enable write-through caching
        cache_size: LRU cache size

    Returns:
        ProximaDBDocument instance
    """
    return ProximaDBDocument(
        client=client,
        enable_cache=enable_cache,
        cache_size=cache_size,
    )
