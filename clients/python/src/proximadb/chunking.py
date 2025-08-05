"""
Text chunking strategies for ProximaDB SDK

This module provides core text chunking functionality with clean separation of concerns:
- Text chunking strategies (no network operations)
- Metadata preparation and separation
- VectorRecord conversion for gRPC/REST protocols

Usage:
    # Basic chunking
    chunker = TextChunker(ChunkingConfig(strategy=ChunkingStrategy.SLIDING_WINDOW))
    chunks = chunker.chunk_text("Your text here", source_id="doc_1")
    
    # Convert to VectorRecord format (requires embeddings from external service)
    records = chunks_to_vector_records(chunks, embeddings, source_metadata={"author": "John"})
"""

import re
import time
from typing import List, Dict, Any, Optional, Callable, Union
from dataclasses import dataclass
from enum import Enum


class ChunkingStrategy(Enum):
    """Available chunking strategies"""
    SENTENCE = "sentence"
    PARAGRAPH = "paragraph" 
    SLIDING_WINDOW = "sliding_window"
    SEMANTIC = "semantic"
    FIXED_SIZE = "fixed_size"
    RECURSIVE = "recursive"


@dataclass
class TextChunk:
    """Represents a chunk of text with metadata"""
    text: str
    start_pos: int
    end_pos: int
    chunk_id: str
    metadata: Dict[str, Any]
    
    @property
    def length(self) -> int:
        """Get the length of the chunk text"""
        return len(self.text)


@dataclass
class ChunkingConfig:
    """Configuration for text chunking"""
    strategy: ChunkingStrategy = ChunkingStrategy.SLIDING_WINDOW
    chunk_size: int = 512
    chunk_overlap: int = 128
    min_chunk_size: int = 100
    max_chunk_size: int = 2048
    separator: str = "\n"
    preserve_sentences: bool = True
    preserve_paragraphs: bool = False
    add_context: bool = True
    context_size: int = 50


class TextChunker:
    """Core text chunking class - no network operations"""
    
    def __init__(self, config: ChunkingConfig = None):
        self.config = config or ChunkingConfig()
        
        # Sentence detection patterns
        self.sentence_endings = re.compile(r'[.!?]+[\s\n]+')
        self.paragraph_separator = re.compile(r'\n\s*\n')
        
    def chunk_text(
        self,
        text: str,
        source_id: str = "doc",
        base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """
        Chunk text using configured strategy (core chunking logic only)
        
        Args:
            text: The text to chunk
            source_id: ID for the source document
            base_metadata: Base metadata to attach to all chunks
            
        Returns:
            List of TextChunk objects
        """
        if not text:
            return []
            
        base_metadata = base_metadata or {}
        
        if self.config.strategy == ChunkingStrategy.SENTENCE:
            return self._chunk_by_sentences(text, source_id, base_metadata)
        elif self.config.strategy == ChunkingStrategy.PARAGRAPH:
            return self._chunk_by_paragraphs(text, source_id, base_metadata)
        elif self.config.strategy == ChunkingStrategy.SLIDING_WINDOW:
            return self._chunk_sliding_window(text, source_id, base_metadata)
        elif self.config.strategy == ChunkingStrategy.FIXED_SIZE:
            return self._chunk_fixed_size(text, source_id, base_metadata)
        else:
            raise ValueError(f"Unsupported chunking strategy: {self.config.strategy}")
    
    def _chunk_by_sentences(self, text: str, source_id: str, metadata: Dict[str, Any]) -> List[TextChunk]:
        """Chunk text by sentences"""
        chunks = []
        sentences = self._split_into_sentences(text)
        current_chunk = []
        current_length = 0
        start_pos = 0
        
        for i, sentence in enumerate(sentences):
            sentence_length = len(sentence)
            
            if current_length + sentence_length > self.config.chunk_size and current_chunk:
                # Create chunk from accumulated sentences
                chunk_text = " ".join(current_chunk)
                chunk = TextChunk(
                    text=chunk_text.strip(),
                    start_pos=start_pos,
                    end_pos=start_pos + len(chunk_text),
                    chunk_id=f"{source_id}_chunk_{len(chunks)}",
                    metadata={
                        **metadata,
                        "chunk_type": "sentence",
                        "sentence_count": len(current_chunk),
                        "chunk_index": len(chunks)
                    }
                )
                chunks.append(chunk)
                
                # Reset for next chunk
                current_chunk = [sentence]
                current_length = sentence_length
                start_pos += len(chunk_text) + 1
            else:
                current_chunk.append(sentence)
                current_length += sentence_length + 1
        
        # Handle remaining sentences
        if current_chunk:
            chunk_text = " ".join(current_chunk)
            chunk = TextChunk(
                text=chunk_text.strip(),
                start_pos=start_pos,
                end_pos=len(text),
                chunk_id=f"{source_id}_chunk_{len(chunks)}",
                metadata={
                    **metadata,
                    "chunk_type": "sentence",
                    "sentence_count": len(current_chunk),
                    "chunk_index": len(chunks)
                }
            )
            chunks.append(chunk)
        
        return chunks
    
    def _chunk_by_paragraphs(self, text: str, source_id: str, metadata: Dict[str, Any]) -> List[TextChunk]:
        """Chunk text by paragraphs"""
        chunks = []
        paragraphs = self.paragraph_separator.split(text)
        position = 0
        
        for i, paragraph in enumerate(paragraphs):
            paragraph = paragraph.strip()
            if not paragraph or len(paragraph) < self.config.min_chunk_size:
                position += len(paragraph) + 2
                continue
                
            if len(paragraph) > self.config.max_chunk_size:
                # Split large paragraphs using sentence chunking
                config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE, 
                                      chunk_size=self.config.chunk_size)
                sub_chunker = TextChunker(config)
                sub_chunks = sub_chunker.chunk_text(paragraph, f"{source_id}_p{i}", metadata)
                chunks.extend(sub_chunks)
            else:
                chunk = TextChunk(
                    text=paragraph,
                    start_pos=position,
                    end_pos=position + len(paragraph),
                    chunk_id=f"{source_id}_chunk_{len(chunks)}",
                    metadata={
                        **metadata,
                        "chunk_type": "paragraph",
                        "paragraph_index": i,
                        "chunk_index": len(chunks)
                    }
                )
                chunks.append(chunk)
            
            position += len(paragraph) + 2
        
        return chunks
    
    def _chunk_sliding_window(self, text: str, source_id: str, metadata: Dict[str, Any]) -> List[TextChunk]:
        """Sliding window chunking"""
        chunks = []
        text_length = len(text)
        start = 0
        chunk_index = 0
        
        while start < text_length:
            end = min(start + self.config.chunk_size, text_length)
            
            # Try to break at word boundary if preserve_sentences is True
            if self.config.preserve_sentences and end < text_length:
                # Look for sentence ending within context_size characters
                context_end = min(end + self.config.context_size, text_length)
                sentence_break = self.sentence_endings.search(text, end, context_end)
                if sentence_break:
                    end = sentence_break.end()
            
            chunk_text = text[start:end].strip()
            if len(chunk_text) >= self.config.min_chunk_size:
                chunk = TextChunk(
                    text=chunk_text,
                    start_pos=start,
                    end_pos=end,
                    chunk_id=f"{source_id}_chunk_{chunk_index}",
                    metadata={
                        **metadata,
                        "chunk_type": "sliding_window",
                        "chunk_index": chunk_index,
                        "overlap_size": self.config.chunk_overlap
                    }
                )
                chunks.append(chunk)
                chunk_index += 1
            
            # Move start position (with overlap)
            start = max(start + self.config.chunk_size - self.config.chunk_overlap, start + 1)
            
            # Prevent infinite loop
            if start >= end:
                break
        
        return chunks
    
    def _chunk_fixed_size(self, text: str, source_id: str, metadata: Dict[str, Any]) -> List[TextChunk]:
        """Fixed size chunking (no overlap)"""
        chunks = []
        text_length = len(text)
        
        for i in range(0, text_length, self.config.chunk_size):
            end = min(i + self.config.chunk_size, text_length)
            chunk_text = text[i:end].strip()
            
            if len(chunk_text) >= self.config.min_chunk_size:
                chunk = TextChunk(
                    text=chunk_text,
                    start_pos=i,
                    end_pos=end,
                    chunk_id=f"{source_id}_chunk_{len(chunks)}",
                    metadata={
                        **metadata,
                        "chunk_type": "fixed_size",
                        "chunk_index": len(chunks)
                    }
                )
                chunks.append(chunk)
        
        return chunks
    
    def _split_into_sentences(self, text: str) -> List[str]:
        """Split text into sentences"""
        sentences = self.sentence_endings.split(text)
        return [s.strip() for s in sentences if s.strip()]


# Metadata separation strategy constants
DEFAULT_FILTERABLE_FIELDS = {
    "text", "chunk_index", "source_type", "category", "author", 
    "title", "tags", "topic", "section", "brand", "sku", "price"
}

NEVER_FILTERABLE_FIELDS = {
    "source_id", "embedding_model", "embedding_dimension",
    "chunk_strategy", "chunk_size", "chunk_overlap", 
    "created_at", "indexed_at", "start_pos", "end_pos"
}


def chunks_to_vector_records(
    chunks: List[TextChunk],
    embeddings: List[List[float]],
    source_type: str = "document",
    source_metadata: Optional[Dict[str, Any]] = None,
    chunk_metadata_fn: Optional[Callable[[TextChunk, int], Dict[str, Any]]] = None,
    filterable_fields: Optional[List[str]] = None,
    include_positions: bool = False
) -> List["VectorRecord"]:
    """
    Convert TextChunk objects and embeddings to VectorRecord format
    
    This function implements smart metadata separation for optimal performance:
    - High-cardinality fields → filterable (indexed for queries)
    - Low-cardinality fields → non-filterable (stored but not indexed)
    
    Args:
        chunks: List of TextChunk objects
        embeddings: List of embedding vectors (must match chunks length)
        source_type: Type of source (document, product, article, etc.)
        source_metadata: Additional source metadata
        chunk_metadata_fn: Function to generate chunk-specific metadata
        filterable_fields: Additional fields to make filterable
        include_positions: Whether to include start_pos/end_pos in metadata
        
    Returns:
        List of VectorRecord objects ready for ProximaDB insertion
        
    Example:
        chunker = TextChunker(ChunkingConfig(strategy=ChunkingStrategy.SLIDING_WINDOW))
        chunks = chunker.chunk_text("Product description", "PROD-123")
        
        # Get embeddings from external service (user's responsibility)
        embeddings = get_embeddings_from_service([chunk.text for chunk in chunks])
        
        # Convert to VectorRecords
        records = chunks_to_vector_records(
            chunks, 
            embeddings,
            source_type="product",
            source_metadata={"brand": "TechCorp", "price": 299.99},
            filterable_fields=["brand", "price"]
        )
    """
    from .models import VectorRecord
    
    if len(chunks) != len(embeddings):
        raise ValueError(f"Chunks ({len(chunks)}) and embeddings ({len(embeddings)}) length mismatch")
    
    if not chunks:
        return []
    
    # Determine filterable fields
    if filterable_fields:
        filterable_set = DEFAULT_FILTERABLE_FIELDS.union(set(filterable_fields))
    else:
        filterable_set = DEFAULT_FILTERABLE_FIELDS
    
    vector_records = []
    timestamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    source_metadata = source_metadata or {}
    
    for i, (chunk, embedding) in enumerate(zip(chunks, embeddings)):
        # Start with chunk's existing metadata
        combined_metadata = dict(chunk.metadata)
        
        # Add essential filterable metadata
        combined_metadata.update({
            "text": chunk.text,
            "chunk_index": i,
            "source_type": source_type,
        })
        
        # Add source metadata (filtered by filterable fields)
        for key, value in source_metadata.items():
            if key in filterable_set and key not in NEVER_FILTERABLE_FIELDS:
                combined_metadata[key] = value
            else:
                combined_metadata[f"source_{key}"] = value
        
        # Add embedding metadata (non-filterable)
        combined_metadata.update({
            "embedding_dimension": len(embedding),
            "created_at": timestamp,
            "total_chunks": len(chunks)
        })
        
        # Add position information if requested
        if include_positions:
            combined_metadata.update({
                "start_pos": chunk.start_pos,
                "end_pos": chunk.end_pos
            })
        
        # Apply custom chunk metadata function
        if chunk_metadata_fn:
            try:
                custom_metadata = chunk_metadata_fn(chunk, i)
                if isinstance(custom_metadata, dict):
                    for key, value in custom_metadata.items():
                        if key in filterable_set and key not in NEVER_FILTERABLE_FIELDS:
                            combined_metadata[key] = value
                        else:
                            combined_metadata[f"custom_{key}"] = value
            except Exception as e:
                import logging
                logging.warning(f"chunk_metadata_fn failed for chunk {i}: {e}")
        
        # Create VectorRecord
        vector_record = VectorRecord(
            id=chunk.chunk_id,
            vector=embedding,
            metadata=combined_metadata
        )
        vector_records.append(vector_record)
    
    return vector_records


# Convenience functions for common chunking patterns
def create_chunker(strategy: str = "sliding_window", **kwargs) -> TextChunker:
    """Create a TextChunker with specified strategy"""
    strategy_enum = ChunkingStrategy(strategy.lower())
    config = ChunkingConfig(strategy=strategy_enum, **kwargs)
    return TextChunker(config)


def chunk_by_sentences(
    text: str,
    source_id: str = "doc",
    chunk_size: int = 512,
    metadata: Optional[Dict[str, Any]] = None
) -> List[TextChunk]:
    """Convenience function for sentence-based chunking"""
    config = ChunkingConfig(strategy=ChunkingStrategy.SENTENCE, chunk_size=chunk_size)
    chunker = TextChunker(config)
    return chunker.chunk_text(text, source_id, metadata)


def chunk_by_paragraphs(
    text: str,
    source_id: str = "doc", 
    metadata: Optional[Dict[str, Any]] = None
) -> List[TextChunk]:
    """Convenience function for paragraph-based chunking"""
    config = ChunkingConfig(strategy=ChunkingStrategy.PARAGRAPH)
    chunker = TextChunker(config)
    return chunker.chunk_text(text, source_id, metadata)


def chunk_sliding_window(
    text: str,
    source_id: str = "doc",
    chunk_size: int = 512,
    overlap: int = 128,
    metadata: Optional[Dict[str, Any]] = None
) -> List[TextChunk]:
    """Convenience function for sliding window chunking"""
    config = ChunkingConfig(
        strategy=ChunkingStrategy.SLIDING_WINDOW,
        chunk_size=chunk_size,
        chunk_overlap=overlap
    )
    chunker = TextChunker(config)
    return chunker.chunk_text(text, source_id, metadata)


# Legacy compatibility wrapper (for tests that expect old interface)
def prepare_vector_records(
    embedding_response: Dict[str, Any],
    source_id: str,
    source_type: str = "document",
    source_metadata: Optional[Dict[str, Any]] = None,
    chunk_metadata_fn: Optional[Callable[[Dict[str, Any], int], Dict[str, Any]]] = None,
    preserve_embedding_metadata: bool = True,
    filterable_fields: Optional[List[str]] = None
) -> List["VectorRecord"]:
    """
    Legacy compatibility wrapper for prepare_vector_records
    
    This maintains compatibility with existing tests while using the new chunking architecture.
    New code should use: chunk_text() → get_embeddings() → chunks_to_vector_records()
    """
    from .models import VectorRecord
    
    chunks_data = embedding_response.get("chunks", [])
    if not chunks_data:
        raise ValueError("No chunks found in embedding service response")
    
    # Convert embedding service format to TextChunk objects
    chunks = []
    embeddings = []
    
    for i, chunk_data in enumerate(chunks_data):
        chunk_id = chunk_data.get("id", f"{source_id}_chunk_{i}")
        chunk_text = chunk_data.get("text", "")
        chunk_embedding = chunk_data.get("embedding", [])
        
        if not chunk_embedding:
            raise ValueError(f"Chunk {i} missing embedding")
        
        # Create TextChunk object
        chunk = TextChunk(
            text=chunk_text,
            start_pos=chunk_data.get("start_pos", i * 400),
            end_pos=chunk_data.get("end_pos", (i + 1) * 400),
            chunk_id=chunk_id,
            metadata={}
        )
        
        chunks.append(chunk)
        embeddings.append(chunk_embedding)
    
    # Use new chunks_to_vector_records function
    return chunks_to_vector_records(
        chunks=chunks,
        embeddings=embeddings,
        source_type=source_type,
        source_metadata=source_metadata,
        chunk_metadata_fn=lambda chunk, idx: chunk_metadata_fn(chunks_data[idx], idx) if chunk_metadata_fn else {},
        filterable_fields=filterable_fields,
        include_positions=True
    )