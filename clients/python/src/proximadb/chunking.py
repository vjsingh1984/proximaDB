"""
Text chunking strategies for ProximaDB

This module provides various text chunking strategies for preparing documents
for vectorization. Supports sentence, paragraph, sliding window, and semantic chunking.
"""

import re
from typing import List, Dict, Any, Optional, Callable, Tuple
from dataclasses import dataclass
from enum import Enum
import numpy as np


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


class ChunkingConfig:
    """Configuration for text chunking"""
    
    def __init__(
        self,
        strategy: ChunkingStrategy = ChunkingStrategy.SLIDING_WINDOW,
        chunk_size: int = 512,
        chunk_overlap: int = 128,
        min_chunk_size: int = 100,
        max_chunk_size: int = 2048,
        separator: str = "\n",
        preserve_sentences: bool = True,
        preserve_paragraphs: bool = False,
        add_context: bool = True,
        context_size: int = 50
    ):
        self.strategy = strategy
        self.chunk_size = chunk_size
        self.chunk_overlap = chunk_overlap
        self.min_chunk_size = min_chunk_size
        self.max_chunk_size = max_chunk_size
        self.separator = separator
        self.preserve_sentences = preserve_sentences
        self.preserve_paragraphs = preserve_paragraphs
        self.add_context = add_context
        self.context_size = context_size


class TextChunker:
    """Main text chunking class with multiple strategies"""
    
    def __init__(self, config: ChunkingConfig = None):
        self.config = config or ChunkingConfig()
        
        # Sentence detection patterns
        self.sentence_endings = re.compile(r'[.!?]+[\s\n]+')
        self.paragraph_separator = re.compile(r'\n\s*\n')
        
    def chunk_text(
        self,
        text: str,
        document_id: str = "doc",
        metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """
        Chunk text using the configured strategy
        
        Args:
            text: The text to chunk
            document_id: ID for the source document
            metadata: Additional metadata to attach to chunks
            
        Returns:
            List of TextChunk objects
        """
        if not text:
            return []
            
        metadata = metadata or {}
        
        if self.config.strategy == ChunkingStrategy.SENTENCE:
            return self._chunk_by_sentences(text, document_id, metadata)
        elif self.config.strategy == ChunkingStrategy.PARAGRAPH:
            return self._chunk_by_paragraphs(text, document_id, metadata)
        elif self.config.strategy == ChunkingStrategy.SLIDING_WINDOW:
            return self._chunk_sliding_window(text, document_id, metadata)
        elif self.config.strategy == ChunkingStrategy.SEMANTIC:
            return self._chunk_semantic(text, document_id, metadata)
        elif self.config.strategy == ChunkingStrategy.FIXED_SIZE:
            return self._chunk_fixed_size(text, document_id, metadata)
        elif self.config.strategy == ChunkingStrategy.RECURSIVE:
            return self._chunk_recursive(text, document_id, metadata)
        else:
            raise ValueError(f"Unknown chunking strategy: {self.config.strategy}")
    
    def _chunk_by_sentences(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """Chunk text by sentences"""
        chunks = []
        sentences = self._split_into_sentences(text)
        current_chunk = []
        current_length = 0
        start_pos = 0
        
        for i, sentence in enumerate(sentences):
            sentence_length = len(sentence)
            
            # Check if adding this sentence would exceed chunk size
            if current_length + sentence_length > self.config.chunk_size and current_chunk:
                # Create chunk from accumulated sentences
                chunk_text = " ".join(current_chunk)
                chunk = TextChunk(
                    text=chunk_text.strip(),
                    start_pos=start_pos,
                    end_pos=start_pos + len(chunk_text),
                    chunk_id=f"{document_id}_chunk_{len(chunks)}",
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
                current_length += sentence_length + 1  # +1 for space
        
        # Handle remaining sentences
        if current_chunk:
            chunk_text = " ".join(current_chunk)
            chunk = TextChunk(
                text=chunk_text.strip(),
                start_pos=start_pos,
                end_pos=len(text),
                chunk_id=f"{document_id}_chunk_{len(chunks)}",
                metadata={
                    **metadata,
                    "chunk_type": "sentence",
                    "sentence_count": len(current_chunk),
                    "chunk_index": len(chunks)
                }
            )
            chunks.append(chunk)
        
        return chunks
    
    def _chunk_by_paragraphs(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """Chunk text by paragraphs"""
        chunks = []
        paragraphs = self.paragraph_separator.split(text)
        position = 0
        
        for i, paragraph in enumerate(paragraphs):
            paragraph = paragraph.strip()
            if not paragraph or len(paragraph) < self.config.min_chunk_size:
                position += len(paragraph) + 2  # Account for \n\n
                continue
                
            # Split large paragraphs if needed
            if len(paragraph) > self.config.max_chunk_size:
                # Use sentence chunking for large paragraphs
                sub_chunks = self._chunk_by_sentences(
                    paragraph,
                    f"{document_id}_p{i}",
                    metadata
                )
                for sub_chunk in sub_chunks:
                    sub_chunk.start_pos += position
                    sub_chunk.end_pos += position
                    sub_chunk.metadata["paragraph_index"] = i
                    chunks.append(sub_chunk)
            else:
                chunk = TextChunk(
                    text=paragraph,
                    start_pos=position,
                    end_pos=position + len(paragraph),
                    chunk_id=f"{document_id}_chunk_{len(chunks)}",
                    metadata={
                        **metadata,
                        "chunk_type": "paragraph",
                        "paragraph_index": i,
                        "chunk_index": len(chunks)
                    }
                )
                chunks.append(chunk)
            
            position += len(paragraph) + 2  # Account for \n\n
        
        return chunks
    
    def _chunk_sliding_window(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """Chunk text using sliding window approach"""
        chunks = []
        text_length = len(text)
        
        # Calculate stride (chunk_size - overlap)
        stride = max(1, self.config.chunk_size - self.config.chunk_overlap)
        
        for start in range(0, text_length, stride):
            end = min(start + self.config.chunk_size, text_length)
            chunk_text = text[start:end]
            
            # Adjust boundaries to preserve sentences if configured
            if self.config.preserve_sentences and start > 0:
                # Find sentence boundary at the start
                sentence_start = self._find_sentence_boundary(text, start, direction="backward")
                if sentence_start != start:
                    start = sentence_start
                    chunk_text = text[start:end]
            
            if self.config.preserve_sentences and end < text_length:
                # Find sentence boundary at the end
                sentence_end = self._find_sentence_boundary(text, end, direction="forward")
                if sentence_end != end:
                    end = sentence_end
                    chunk_text = text[start:end]
            
            # Skip chunks that are too small
            if len(chunk_text.strip()) < self.config.min_chunk_size:
                continue
            
            chunk = TextChunk(
                text=chunk_text.strip(),
                start_pos=start,
                end_pos=end,
                chunk_id=f"{document_id}_chunk_{len(chunks)}",
                metadata={
                    **metadata,
                    "chunk_type": "sliding_window",
                    "window_size": self.config.chunk_size,
                    "overlap": self.config.chunk_overlap,
                    "chunk_index": len(chunks)
                }
            )
            chunks.append(chunk)
            
            # Break if we've reached the end
            if end >= text_length:
                break
        
        return chunks
    
    def _chunk_semantic(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """
        Chunk text based on semantic boundaries (topics/sections)
        This is a simplified version - production would use NLP models
        """
        chunks = []
        
        # Look for semantic markers (headers, topic changes)
        section_markers = [
            r'^#{1,6}\s+.*$',  # Markdown headers
            r'^[A-Z][^.!?]*:$',  # Title-like lines
            r'^\d+\.\s+.*$',  # Numbered sections
            r'^[A-Z]{2,}.*$',  # All caps headers
        ]
        
        combined_pattern = '|'.join(f'({pattern})' for pattern in section_markers)
        section_regex = re.compile(combined_pattern, re.MULTILINE)
        
        # Find all section boundaries
        matches = list(section_regex.finditer(text))
        
        if not matches:
            # No sections found, use paragraph chunking
            return self._chunk_by_paragraphs(text, document_id, metadata)
        
        # Process sections
        for i, match in enumerate(matches):
            start = match.start()
            end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
            
            section_text = text[start:end].strip()
            
            # Split large sections
            if len(section_text) > self.config.max_chunk_size:
                sub_chunks = self._chunk_sliding_window(
                    section_text,
                    f"{document_id}_s{i}",
                    metadata
                )
                for sub_chunk in sub_chunks:
                    sub_chunk.start_pos += start
                    sub_chunk.end_pos += start
                    sub_chunk.metadata["section_index"] = i
                    chunks.append(sub_chunk)
            else:
                chunk = TextChunk(
                    text=section_text,
                    start_pos=start,
                    end_pos=end,
                    chunk_id=f"{document_id}_chunk_{len(chunks)}",
                    metadata={
                        **metadata,
                        "chunk_type": "semantic",
                        "section_index": i,
                        "section_header": match.group(0),
                        "chunk_index": len(chunks)
                    }
                )
                chunks.append(chunk)
        
        return chunks
    
    def _chunk_fixed_size(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """Chunk text into fixed-size pieces"""
        chunks = []
        
        for i in range(0, len(text), self.config.chunk_size):
            chunk_text = text[i:i + self.config.chunk_size]
            
            if len(chunk_text.strip()) < self.config.min_chunk_size:
                continue
                
            chunk = TextChunk(
                text=chunk_text.strip(),
                start_pos=i,
                end_pos=min(i + self.config.chunk_size, len(text)),
                chunk_id=f"{document_id}_chunk_{len(chunks)}",
                metadata={
                    **metadata,
                    "chunk_type": "fixed_size",
                    "chunk_size": self.config.chunk_size,
                    "chunk_index": len(chunks)
                }
            )
            chunks.append(chunk)
        
        return chunks
    
    def _chunk_recursive(
        self,
        text: str,
        document_id: str,
        metadata: Dict[str, Any]
    ) -> List[TextChunk]:
        """
        Recursively chunk text using multiple separators
        Similar to LangChain's RecursiveCharacterTextSplitter
        """
        separators = ["\n\n", "\n", ". ", " ", ""]
        chunks = []
        
        def _split_recursive(
            text: str,
            separators: List[str],
            chunk_size: int
        ) -> List[str]:
            if not text or not separators:
                return [text]
            
            separator = separators[0]
            splits = text.split(separator) if separator else list(text)
            
            final_chunks = []
            current_chunk = []
            current_size = 0
            
            for split in splits:
                split_size = len(split)
                
                if current_size + split_size + len(separator) > chunk_size and current_chunk:
                    # Join current chunk
                    final_chunks.append(separator.join(current_chunk))
                    current_chunk = [split]
                    current_size = split_size
                else:
                    current_chunk.append(split)
                    current_size += split_size + len(separator)
            
            if current_chunk:
                final_chunks.append(separator.join(current_chunk))
            
            # Recursively split chunks that are still too large
            result = []
            for chunk in final_chunks:
                if len(chunk) > chunk_size and len(separators) > 1:
                    result.extend(_split_recursive(chunk, separators[1:], chunk_size))
                else:
                    result.append(chunk)
            
            return result
        
        # Get recursive chunks
        text_chunks = _split_recursive(text, separators, self.config.chunk_size)
        
        # Convert to TextChunk objects
        position = 0
        for i, chunk_text in enumerate(text_chunks):
            if len(chunk_text.strip()) < self.config.min_chunk_size:
                position += len(chunk_text)
                continue
                
            chunk = TextChunk(
                text=chunk_text.strip(),
                start_pos=position,
                end_pos=position + len(chunk_text),
                chunk_id=f"{document_id}_chunk_{len(chunks)}",
                metadata={
                    **metadata,
                    "chunk_type": "recursive",
                    "chunk_index": len(chunks)
                }
            )
            chunks.append(chunk)
            position += len(chunk_text)
        
        return chunks
    
    def _split_into_sentences(self, text: str) -> List[str]:
        """Split text into sentences"""
        # Handle common abbreviations
        text = re.sub(r'\b(Dr|Mr|Mrs|Ms|Prof|Sr|Jr)\.\s*', r'\1<PERIOD> ', text)
        
        # Split by sentence endings
        sentences = self.sentence_endings.split(text)
        
        # Restore periods
        sentences = [s.replace('<PERIOD>', '.') for s in sentences]
        
        # Filter empty sentences
        return [s.strip() for s in sentences if s.strip()]
    
    def _find_sentence_boundary(
        self,
        text: str,
        position: int,
        direction: str = "forward"
    ) -> int:
        """Find the nearest sentence boundary"""
        if direction == "forward":
            # Look for next sentence ending
            match = self.sentence_endings.search(text, position)
            if match:
                return match.end()
            return len(text)
        else:  # backward
            # Look for previous sentence ending
            for i in range(position, -1, -1):
                if i == 0:
                    return 0
                if text[i-1:i+1] in ['. ', '! ', '? ']:
                    return i + 1
            return 0
    
    def add_context_to_chunks(
        self,
        chunks: List[TextChunk],
        context_size: int = None
    ) -> List[TextChunk]:
        """
        Add surrounding context to each chunk
        
        Args:
            chunks: List of chunks to add context to
            context_size: Number of characters of context (uses config if None)
            
        Returns:
            List of chunks with context added to metadata
        """
        context_size = context_size or self.config.context_size
        
        for i, chunk in enumerate(chunks):
            # Previous context
            if i > 0:
                prev_chunk = chunks[i-1]
                prev_context = prev_chunk.text[-context_size:].strip()
                chunk.metadata["prev_context"] = prev_context
            
            # Next context
            if i < len(chunks) - 1:
                next_chunk = chunks[i+1]
                next_context = next_chunk.text[:context_size].strip()
                chunk.metadata["next_context"] = next_context
        
        return chunks


def create_chunker(strategy: str, **kwargs) -> TextChunker:
    """
    Factory function to create a text chunker with specified strategy
    
    Args:
        strategy: Name of the chunking strategy
        **kwargs: Additional configuration parameters
        
    Returns:
        Configured TextChunker instance
    """
    strategy_enum = ChunkingStrategy(strategy.lower())
    config = ChunkingConfig(strategy=strategy_enum, **kwargs)
    return TextChunker(config)


# Convenience functions for common use cases
def chunk_by_sentences(
    text: str,
    chunk_size: int = 512,
    document_id: str = "doc",
    metadata: Dict[str, Any] = None
) -> List[TextChunk]:
    """Chunk text by sentences"""
    chunker = create_chunker("sentence", chunk_size=chunk_size)
    return chunker.chunk_text(text, document_id, metadata)


def chunk_by_paragraphs(
    text: str,
    max_size: int = 1024,
    document_id: str = "doc",
    metadata: Dict[str, Any] = None
) -> List[TextChunk]:
    """Chunk text by paragraphs"""
    chunker = create_chunker("paragraph", max_chunk_size=max_size)
    return chunker.chunk_text(text, document_id, metadata)


def chunk_sliding_window(
    text: str,
    window_size: int = 512,
    overlap: int = 128,
    document_id: str = "doc",
    metadata: Dict[str, Any] = None
) -> List[TextChunk]:
    """Chunk text using sliding window"""
    chunker = create_chunker(
        "sliding_window",
        chunk_size=window_size,
        chunk_overlap=overlap
    )
    return chunker.chunk_text(text, document_id, metadata)