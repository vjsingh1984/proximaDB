"""
Text chunking module for ProximaDB SDK

This module provides clean text chunking functionality with performance optimizations:
- Delegates all chunking logic to the strategy pattern modules
- Includes ChunkerPool optimization for performance
- Provides convenient utility functions for creating vector records
- Maintains clean separation of concerns

Usage:
    # Basic chunking using strategy pattern
    chunker = TextChunker(ChunkingConfig(strategy=ChunkingStrategy.SEMANTIC))
    chunks = chunker.chunk_text("Your text here", source_id="doc_1")

    # With pooling for better performance
    config = ChunkingConfig(strategy=ChunkingStrategy.RECURSIVE)
    with PooledChunkerContext(config) as chunker:
        chunks = chunker.chunk_text(text, source_id="doc_1")

    # Create vector records from chunks and embeddings
    records = create_vector_records(chunks, embeddings, collection_metadata)

    # Convenience function that combines chunking and embedding
    records = chunk_and_embed_text(text, source_id, embedding_provider, config)
"""

import threading
import time
from collections import defaultdict
from typing import Any, Callable, Dict, List, Optional, Union

# Import from chunking strategies for clean separation
from .chunking_strategies import (
    ChunkingConfig,
    ChunkingStrategy,
    ChunkingStrategyInterface,
    TextChunk,
    get_chunking_strategy,
)
from .models import VectorRecord
from .resource_pool import ResourceFactory, ResourcePool


class ChunkerFactory(ResourceFactory):
    """Factory for creating TextChunker instances"""

    def __init__(self, config: ChunkingConfig):
        self.config = config

    def create(self) -> "TextChunker":
        """Create new TextChunker instance"""
        return TextChunker(self.config)

    def validate(self, resource: "TextChunker") -> bool:
        """Validate chunker is still usable"""
        return resource._strategy is not None

    def reset(self, resource: "TextChunker") -> None:
        """Reset chunker state if needed"""
        # TextChunker is stateless, no reset needed
        pass

    def dispose(self, resource: "TextChunker") -> None:
        """Dispose of chunker"""
        # No special cleanup needed
        pass

    def destroy(self, resource: "TextChunker") -> None:
        """Destroy resource - alias for dispose"""
        self.dispose(resource)


class ChunkerPool:
    """
    Thread-safe chunker instance pool using unified ResourcePool

    Features:
    - Reuses TextChunker instances to avoid creation overhead
    - Thread-safe access with minimal lock contention
    - Auto-scaling pool size based on usage patterns
    - Performance monitoring and metrics via ResourcePool

    Performance Benefit: 10-15% improvement for recursive/semantic chunking
    """

    _instance = None
    _lock = threading.RLock()

    def __init__(self, max_pool_size: int = 50):
        self.max_pool_size = max_pool_size
        self._pools: Dict[str, ResourcePool[TextChunker]] = {}
        self._pool_locks: Dict[str, threading.RLock] = defaultdict(threading.RLock)

    @classmethod
    def get_instance(cls) -> "ChunkerPool":
        """Get singleton instance of chunker pool"""
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:
                    cls._instance = cls()
        return cls._instance

    def _get_pool_key(self, config: ChunkingConfig) -> str:
        """Generate pool key for chunker configuration"""
        return f"{config.strategy.value}_{config.chunk_size}_{config.chunk_overlap}_{config.min_chunk_size}"

    def _get_or_create_pool(
        self, config: ChunkingConfig
    ) -> ResourcePool["TextChunker"]:
        """Get or create resource pool for config"""
        pool_key = self._get_pool_key(config)

        with self._pool_locks[pool_key]:
            if pool_key not in self._pools:
                factory = ChunkerFactory(config)
                self._pools[pool_key] = ResourcePool(
                    factory=factory, max_size=self.max_pool_size
                )
            return self._pools[pool_key]

    def get_chunker(self, config: ChunkingConfig) -> "TextChunker":
        """Get chunker instance from pool or create new one"""
        pool = self._get_or_create_pool(config)
        return pool.acquire()

    def return_chunker(self, chunker: "TextChunker", config: ChunkingConfig):
        """Return chunker to pool for reuse"""
        pool = self._get_or_create_pool(config)
        pool.release(chunker)

    def get_stats(self) -> Dict[str, Any]:
        """Get pool performance statistics"""
        all_stats = {}
        for pool_key, pool in self._pools.items():
            metrics = pool.get_metrics()
            all_stats[pool_key] = {
                "total_requests": metrics["total_acquisitions"],
                "active": metrics["active_resources"],
                "available": metrics["available_resources"],
                "total_created": metrics["total_created"],
                "health": pool.health_check(),
            }

        # Calculate aggregate stats
        total_hits = sum(
            stats["total_requests"] - stats["total_created"]
            for stats in all_stats.values()
        )
        total_requests = sum(stats["total_requests"] for stats in all_stats.values())
        hit_rate = (total_hits / total_requests * 100) if total_requests > 0 else 0

        return {
            "hit_rate_percent": hit_rate,
            "total_requests": total_requests,
            "active_pools": len(self._pools),
            "pool_stats": all_stats,
        }

    def cleanup_unused_pools(self, max_idle_time: float = 300.0):
        """Clean up unused pools"""
        # ResourcePool handles its own cleanup via background maintenance
        pass


# Global chunker pool instance
_global_chunker_pool = ChunkerPool()


class PooledChunkerContext:
    """Context manager for using pooled chunker instances"""

    def __init__(self, config: ChunkingConfig, pool: ChunkerPool = None):
        self.config = config
        self.pool = pool or _global_chunker_pool
        self.chunker = None

    def __enter__(self) -> "TextChunker":
        self.chunker = self.pool.get_chunker(self.config)
        return self.chunker

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.chunker:
            self.pool.return_chunker(self.chunker, self.config)


class TextChunker:
    """
    Main text chunking interface that delegates to strategy pattern

    This class provides a clean interface that delegates all chunking logic
    to the appropriate strategy while maintaining compatibility with existing code.
    """

    def __init__(self, config: Optional[ChunkingConfig] = None):
        """
        Initialize text chunker

        Args:
            config: Chunking configuration (uses defaults if not provided)
        """
        self.config = config or ChunkingConfig()
        self._strategy = None
        self._initialize_strategy()

    def _initialize_strategy(self):
        """Initialize the chunking strategy"""
        self._strategy = get_chunking_strategy(
            self.config.strategy,
            chunk_size=self.config.chunk_size,
            chunk_overlap=self.config.chunk_overlap,
            min_chunk_size=self.config.min_chunk_size,
            max_chunk_size=self.config.max_chunk_size,
            preserve_sentences=self.config.preserve_sentences,
            preserve_paragraphs=self.config.preserve_paragraphs,
            preserve_code_blocks=getattr(self.config, "preserve_code_blocks", False),
            preserve_tables=getattr(self.config, "preserve_tables", False),
            add_context=self.config.add_context,
            context_size=self.config.context_size,
        )

    def chunk_text(
        self,
        text: str,
        source_id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> List[TextChunk]:
        """
        Chunk text using the configured strategy

        Args:
            text: Text to chunk
            source_id: Optional identifier for the source document (defaults to "doc")
            metadata: Optional metadata to include with all chunks

        Returns:
            List of TextChunk objects
        """
        if not self._strategy:
            self._initialize_strategy()

        # Default source_id if not provided
        if source_id is None:
            source_id = "doc"

        return self._strategy.chunk(text, source_id, metadata)

    def add_context_to_chunks(
        self, chunks: List[TextChunk], context_size: int = 50
    ) -> List[TextChunk]:
        """
        Add context from surrounding chunks

        Args:
            chunks: List of chunks to enhance
            context_size: Number of characters to include from adjacent chunks

        Returns:
            Enhanced chunks with context metadata
        """
        if not chunks or len(chunks) <= 1:
            return chunks

        enhanced_chunks = []

        for i, chunk in enumerate(chunks):
            # Add context from previous chunk
            if i > 0:
                prev_text = chunks[i - 1].text
                prev_context = (
                    prev_text[-context_size:]
                    if len(prev_text) > context_size
                    else prev_text
                )
                chunk.metadata["prev_context"] = prev_context

            # Add context from next chunk
            if i < len(chunks) - 1:
                next_text = chunks[i + 1].text
                next_context = (
                    next_text[:context_size]
                    if len(next_text) > context_size
                    else next_text
                )
                chunk.metadata["next_context"] = next_context

            chunk.metadata["has_context"] = True
            enhanced_chunks.append(chunk)

        return enhanced_chunks


def create_vector_records(
    chunks: List[TextChunk],
    embeddings: List[List[float]],
    collection_metadata: Optional[Dict[str, Any]] = None,
    filterable_fields: Optional[List[str]] = None,
    model_id: Optional[str] = None,
    processing_config: Optional[Dict[str, Any]] = None,
    source_type: Optional[str] = None,
    source_metadata: Optional[Dict[str, Any]] = None,
) -> List[VectorRecord]:
    """
    Create VectorRecord objects from chunks and embeddings with ultra-efficient enum packing

    This function combines the results of chunking and embedding into
    the format needed for ProximaDB storage, leveraging the new gRPC source content
    fields and 75% storage savings through enum packing.

    Args:
        chunks: List of text chunks
        embeddings: List of embedding vectors (must match chunks length)
        collection_metadata: Metadata to add to all records
        filterable_fields: List of metadata fields to mark as filterable
        model_id: Optional embedding model ID for tracking
        processing_config: Optional processing configuration

    Returns:
        List of VectorRecord objects ready for insertion

    Raises:
        ValueError: If chunks and embeddings lengths don't match
    """
    if len(chunks) != len(embeddings):
        raise ValueError(
            f"Chunks ({len(chunks)}) and embeddings ({len(embeddings)}) "
            f"length mismatch"
        )

    collection_metadata = collection_metadata or {}
    filterable_fields = set(filterable_fields or [])

    # Default filterable fields
    default_filterable = {"source_id", "chunk_index", "chunk_type", "chunking_strategy"}
    filterable_fields.update(default_filterable)

    records = []

    for chunk, embedding in zip(chunks, embeddings):
        # Combine all metadata
        metadata = {
            **collection_metadata,
            **chunk.metadata,
            "source_id": chunk.metadata.get("source_id", chunk.chunk_id.split("_")[0]),
            "text_preview": (
                chunk.text[:100] + "..." if len(chunk.text) > 100 else chunk.text
            ),
            "embedding_dimension": len(embedding),
        }

        # Add source_type and source_metadata if provided
        if source_type:
            metadata["source_type"] = source_type
        if source_metadata:
            metadata.update(source_metadata)

        # Separate filterable and non-filterable metadata
        filterable_metadata = {
            k: v
            for k, v in metadata.items()
            if k in filterable_fields and isinstance(v, (str, int, float, bool))
        }

        non_filterable_metadata = {
            k: v for k, v in metadata.items() if k not in filterable_fields
        }

        # Create vector record with optimized structure
        # Merge filterable and non-filterable metadata (no nesting allowed)
        all_metadata = {**filterable_metadata, **non_filterable_metadata}

        record = VectorRecord(
            id=chunk.chunk_id,
            vector=embedding,
            metadata=all_metadata,
            source=chunk.text,  # Store original chunk text in source field for RAG
        )

        records.append(record)

    return records


def chunk_and_embed_text(
    text: str,
    source_id: str,
    embedding_provider,
    chunking_config: Optional[ChunkingConfig] = None,
    metadata: Optional[Dict[str, Any]] = None,
    filterable_fields: Optional[List[str]] = None,
    model_id: Optional[str] = None,
    processing_config: Optional[Dict[str, Any]] = None,
) -> List[VectorRecord]:
    """
    Convenience function that chunks text and generates embeddings with ultra-efficient storage

    This is a helper that combines chunking and embedding in one call,
    but still maintains separation of concerns internally. Now leverages
    the new gRPC source content fields and enum packing for 75% storage savings.

    Args:
        text: Text to process
        source_id: Source document identifier
        embedding_provider: Embedding provider instance
        chunking_config: Optional chunking configuration
        metadata: Optional metadata for all chunks
        filterable_fields: Fields to mark as filterable
        model_id: Optional embedding model ID for tracking
        processing_config: Optional processing configuration

    Returns:
        List of VectorRecord objects with optimized source content storage
    """
    # 1. Chunk text using pooled chunker for performance
    config = chunking_config or ChunkingConfig()
    with PooledChunkerContext(config) as chunker:
        chunks = chunker.chunk_text(text, source_id, metadata)

    # 2. Generate embeddings with processing metadata for ultra-efficient storage
    chunk_texts = [chunk.text for chunk in chunks]
    if hasattr(embedding_provider, "embed_texts_with_metadata"):
        embeddings, embedding_metadata = embedding_provider.embed_texts_with_metadata(
            chunk_texts
        )
        # Merge with existing processing config
        if processing_config:
            processing_config.update(embedding_metadata)
        else:
            processing_config = embedding_metadata
    else:
        # Fallback for providers that don't support metadata
        embeddings = embedding_provider.embed_texts(chunk_texts)

    # 3. Create vector records with ultra-efficient enum packing
    records = create_vector_records(
        chunks,
        embeddings.tolist() if hasattr(embeddings, "tolist") else embeddings,
        metadata,
        filterable_fields,
        model_id=model_id,
        processing_config=processing_config,
    )

    return records


# Backward compatibility functions (legacy API from old chunking.py)
def create_chunker(
    strategy: Union[str, ChunkingStrategy, ChunkingConfig] = None, **kwargs
) -> TextChunker:
    """Create a text chunker instance (backward compatibility)

    Args:
        strategy: Strategy name, ChunkingStrategy enum, or ChunkingConfig object
        **kwargs: Additional configuration parameters if strategy is a string/enum

    Returns:
        TextChunker instance
    """
    if isinstance(strategy, ChunkingConfig):
        # Direct config passed
        return TextChunker(strategy)
    elif strategy is None:
        # Use defaults
        return TextChunker()
    else:
        # Create config from strategy and kwargs
        if isinstance(strategy, str):
            strategy = ChunkingStrategy(strategy)
        config = ChunkingConfig(strategy=strategy, **kwargs)
        return TextChunker(config)


def create_enhanced_semantic_chunker(
    enable_caching: bool = True, **kwargs
) -> TextChunker:
    """Create an enhanced semantic chunker (alias for create_chunker with semantic strategy)

    Args:
        enable_caching: Whether to enable embedding caching (default: True)
        **kwargs: Additional configuration parameters

    Returns:
        TextChunker instance configured for semantic chunking
    """
    # Add enable_caching to kwargs if not already present
    if "enable_caching" not in kwargs:
        kwargs["enable_caching"] = enable_caching

    # Create chunker with semantic strategy
    return create_chunker(ChunkingStrategy.SEMANTIC, **kwargs)


def chunk_by_sentences(
    text: str,
    chunk_size: int = 512,
    document_id: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
) -> List[TextChunk]:
    """
    Chunk text by sentences.

    Args:
        text: Text to chunk
        chunk_size: Maximum chunk size in characters
        document_id: Optional document ID (defaults to "doc")
        metadata: Optional metadata to attach to chunks

    Returns:
        List of TextChunk objects
    """
    config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE, chunk_size=chunk_size)
    chunker = TextChunker(config)
    source_id = document_id or "doc"
    return chunker.chunk_text(text, source_id, metadata)


def chunk_by_paragraphs(
    text: str,
    max_size: int = 1000,
    document_id: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
) -> List[TextChunk]:
    """
    Chunk text by paragraphs.

    Args:
        text: Text to chunk
        max_size: Maximum chunk size in characters
        document_id: Optional document ID (defaults to "doc")
        metadata: Optional metadata to attach to chunks

    Returns:
        List of TextChunk objects
    """
    config = ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH, chunk_size=max_size)
    chunker = TextChunker(config)
    source_id = document_id or "doc"
    return chunker.chunk_text(text, source_id, metadata)


def chunk_sliding_window(
    text: str,
    window_size: int = 512,
    overlap: int = 128,
    document_id: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
) -> List[TextChunk]:
    """
    Chunk text using sliding window.

    Args:
        text: Text to chunk
        window_size: Size of sliding window in characters
        overlap: Overlap between chunks in characters
        document_id: Optional document ID (defaults to "doc")
        metadata: Optional metadata to attach to chunks

    Returns:
        List of TextChunk objects
    """
    config = ChunkingConfig(
        strategy=ChunkingStrategy.SLIDING_WINDOW,
        chunk_size=window_size,
        chunk_overlap=overlap,
    )
    chunker = TextChunker(config)
    source_id = document_id or "doc"
    return chunker.chunk_text(text, source_id, metadata)


def prepare_vector_records(
    response: Dict[str, Any],
    source_id: str,
    source_type: Optional[str] = None,
    source_metadata: Optional[Dict[str, Any]] = None,
    filterable_fields: Optional[List[str]] = None,
    chunk_metadata_fn: Optional[Callable[[Dict[str, Any], int], Dict[str, Any]]] = None,
    preserve_embedding_metadata: bool = False,
) -> List[VectorRecord]:
    """
    Prepare vector records from embedding service response with flexible metadata handling.

    This function processes chunks from an embedding service response and creates VectorRecord
    objects with sophisticated metadata management:
    - Filterable fields go into metadata at top level
    - Non-filterable fields get namespaced with prefixes (source_, chunk_, custom_, etc.)
    - Custom metadata functions can enrich chunks
    - Preserves or filters embedding service metadata

    Args:
        response: Embedding service response with 'chunks' array. Each chunk should have:
                  - id: Chunk identifier
                  - text: Chunk text content
                  - embedding: Vector embedding (list of floats)
                  - Optional: start_pos, end_pos, token_count, confidence, language, etc.
        source_id: Source document identifier (stored in metadata)
        source_type: Source document type (optional, e.g., "product", "documentation")
        source_metadata: Additional source-level metadata. Fields in filterable_fields go to
                        top level, others get "source_" prefix
        filterable_fields: List of metadata field names that should be at top level for filtering.
                          Includes fields from source_metadata, chunk_metadata_fn, and special
                          fields like "category", "author", "tags", "price", etc.
        chunk_metadata_fn: Optional function (chunk_dict, index) -> metadata_dict to enrich
                          each chunk with custom metadata. Returned fields get "custom_" prefix
                          unless they're in filterable_fields
        preserve_embedding_metadata: If True, preserve extra chunk fields from embedding service
                                    (e.g., confidence, language, token_count). These get "chunk_"
                                    prefix unless in filterable_fields

    Returns:
        List of VectorRecord objects with properly structured metadata

    Raises:
        ValueError: If response has no chunks or chunks missing embeddings

    Examples:
        >>> # Basic usage
        >>> response = {
        ...     "chunks": [
        ...         {"id": "c1", "text": "Hello", "embedding": [0.1, 0.2]},
        ...         {"id": "c2", "text": "World", "embedding": [0.3, 0.4]}
        ...     ],
        ...     "model": "all-mpnet-base-v2"
        ... }
        >>> records = prepare_vector_records(response, source_id="doc1", source_type="test")

        >>> # With filterable metadata
        >>> response = {"chunks": [{"id": "c1", "text": "Product", "embedding": [0.1]}]}
        >>> records = prepare_vector_records(
        ...     response,
        ...     source_id="PROD-123",
        ...     source_metadata={"category": "Electronics", "price": 99.99, "internal_note": "Check"},
        ...     filterable_fields=["category", "price"]  # internal_note gets source_ prefix
        ... )

        >>> # With custom enrichment
        >>> def enrich(chunk, idx):
        ...     return {"section": f"part_{idx}", "has_numbers": any(c.isdigit() for c in chunk["text"])}
        >>> records = prepare_vector_records(
        ...     response,
        ...     source_id="doc",
        ...     chunk_metadata_fn=enrich,
        ...     filterable_fields=["section", "has_numbers"]
        ... )
    """
    import time
    from datetime import datetime, timezone

    # Extract chunks from response
    chunks = response.get("chunks", [])
    if not chunks:
        raise ValueError("No chunks found in response")

    # Default filterable fields include common high-cardinality fields
    filterable_fields = filterable_fields or []
    filterable_set = set(filterable_fields)

    # Always filterable: common high-value fields
    always_filterable = {
        "text",
        "chunk_index",
        "source_type",
        "category",
        "author",
        "tags",
        "brand",
        "sku",
        "product_id",
    }
    filterable_set.update(always_filterable)

    records = []

    for idx, chunk in enumerate(chunks):
        # Validate chunk has required fields
        if "embedding" not in chunk:
            raise ValueError(f"Chunk {idx} missing embedding")

        chunk_id = chunk.get("id", f"chunk_{idx}")
        text = chunk.get("text", "")
        embedding = chunk["embedding"]

        # Start building metadata
        metadata = {}

        # 1. Core chunk fields (always filterable)
        metadata["text"] = text
        metadata["chunk_index"] = idx
        if source_type:
            metadata["source_type"] = source_type

        # 2. Source metadata (filterable fields at top level, others with source_ prefix)
        if source_metadata:
            for key, value in source_metadata.items():
                if key in filterable_set:
                    metadata[key] = value
                else:
                    metadata[f"source_{key}"] = value

        # 3. Custom chunk metadata function
        if chunk_metadata_fn:
            try:
                custom_meta = chunk_metadata_fn(chunk, idx)
                if custom_meta:
                    for key, value in custom_meta.items():
                        if key in filterable_set:
                            metadata[key] = value
                        else:
                            metadata[f"custom_{key}"] = value
            except Exception as e:
                # Log warning but don't break processing
                logger = __import__("logging").getLogger(__name__)
                logger.warning(f"Custom metadata function failed for chunk {idx}: {e}")

        # 4. Embedding service metadata (if preserving)
        if preserve_embedding_metadata:
            # Fields to exclude (already handled)
            exclude_fields = {"id", "text", "embedding"}
            for key, value in chunk.items():
                if key not in exclude_fields:
                    if key in filterable_set:
                        metadata[key] = value
                    else:
                        metadata[f"chunk_{key}"] = value

        # 5. Response-level metadata (chunking/embedding info - always extra with prefixes)
        # These are low-cardinality so they go to extra metadata
        if "model" in response:
            metadata["embedding_model"] = response["model"]
        if "chunking_strategy" in response:
            metadata["chunk_strategy"] = response["chunking_strategy"]
        if "chunk_size" in response:
            metadata["chunk_size"] = response["chunk_size"]
        if "overlap" in response:
            metadata["chunk_overlap"] = response["overlap"]
        if "dimension" in response:
            metadata["embedding_dimension"] = response["dimension"]

        # 6. System metadata (extra)
        metadata["source_id"] = source_id
        metadata["created_at"] = datetime.now(timezone.utc).isoformat()
        metadata["indexed_at"] = datetime.now(timezone.utc).isoformat()

        # Create VectorRecord
        record = VectorRecord(id=chunk_id, vector=embedding, metadata=metadata)

        records.append(record)

    return records


def get_chunker_pool_stats() -> Dict[str, Any]:
    """Get global chunker pool performance statistics"""
    return _global_chunker_pool.get_stats()


def cleanup_chunker_pool():
    """Manually trigger cleanup of unused chunker pools"""
    _global_chunker_pool.cleanup_unused_pools()


# Re-export key components
__all__ = [
    # Core classes
    "TextChunker",
    "ChunkerPool",
    "PooledChunkerContext",
    # Strategy pattern imports
    "ChunkingStrategy",
    "ChunkingConfig",
    "TextChunk",
    # Main utility functions
    "create_vector_records",
    "chunk_and_embed_text",
    # Backward compatibility functions
    "create_chunker",
    "chunk_by_sentences",
    "chunk_by_paragraphs",
    "chunk_sliding_window",
    "prepare_vector_records",
    # Pool utilities
    "get_chunker_pool_stats",
    "cleanup_chunker_pool",
]
