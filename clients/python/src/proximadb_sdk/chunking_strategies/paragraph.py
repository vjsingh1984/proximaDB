"""
Paragraph-based chunking strategy

Chunks text at paragraph boundaries while respecting size constraints.
"""

import re
from typing import Any, Dict, List, Optional, Tuple

from .base import ChunkingConfig, ChunkingStrategyInterface, TextChunk


class ParagraphStrategy(ChunkingStrategyInterface):
    """
    Paragraph-based chunking that preserves paragraph structure

    Keeps paragraphs together when possible, splits large paragraphs if needed
    """

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile regex patterns for paragraph detection"""
        # Pattern for paragraph boundaries
        self.paragraph_pattern = re.compile(r"\n\s*\n+")

        # Pattern for list items
        self.list_pattern = re.compile(r"^\s*[\-\*\+•]\s+", re.MULTILINE)

        # Pattern for numbered lists
        self.numbered_list_pattern = re.compile(r"^\s*\d+[\.\)]\s+", re.MULTILINE)

    def _split_into_paragraphs(self, text: str) -> List[Tuple[str, int]]:
        """Split text into paragraphs with positions"""
        paragraphs = []

        # Split by paragraph breaks
        parts = self.paragraph_pattern.split(text)
        current_pos = 0

        for part in parts:
            part = part.strip()
            if part:
                # Find actual position in original text
                start_pos = text.find(part, current_pos)
                paragraphs.append((part, start_pos))
                current_pos = start_pos + len(part)

        return paragraphs

    def _is_list_paragraph(self, text: str) -> bool:
        """Check if paragraph is a list"""
        lines = text.strip().split("\n")
        if len(lines) < 2:
            return False

        # Check if most lines are list items
        list_lines = sum(
            1
            for line in lines
            if self.list_pattern.match(line) or self.numbered_list_pattern.match(line)
        )

        return list_lines >= len(lines) * 0.7

    def _split_large_paragraph(self, text: str, max_size: int) -> List[str]:
        """Split a large paragraph into smaller chunks"""
        if len(text) <= max_size:
            return [text]

        # Try to split at sentence boundaries first
        sentence_endings = "|".join(re.escape(e) for e in self.config.sentence_endings)
        sentence_pattern = re.compile(rf"(?<=[{sentence_endings}])\s+")

        sentences = sentence_pattern.split(text)

        # Group sentences into chunks
        chunks = []
        current_chunk = []
        current_length = 0

        for sentence in sentences:
            sentence_length = len(sentence)

            if current_chunk and current_length + sentence_length + 1 > max_size:
                chunks.append(" ".join(current_chunk))
                current_chunk = []
                current_length = 0

            current_chunk.append(sentence)
            current_length += sentence_length + (1 if current_chunk else 0)

        if current_chunk:
            chunks.append(" ".join(current_chunk))

        return chunks

    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """Create chunks at paragraph boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}
        chunks = []

        # Split into paragraphs
        paragraphs = self._split_into_paragraphs(text)
        if not paragraphs:
            return []

        # Process paragraphs
        chunk_index = 0
        current_chunk_paras = []
        current_chunk_length = 0
        current_chunk_start = 0

        for para_text, para_start in paragraphs:
            para_length = len(para_text)
            is_list = self._is_list_paragraph(para_text)

            # Check if paragraph itself is too large
            if para_length > self.config.max_chunk_size:
                # First, create chunk from accumulated paragraphs
                if current_chunk_paras:
                    chunk_text = "\n\n".join(current_chunk_paras)
                    if len(chunk_text) >= self.config.min_chunk_size:
                        chunks.append(
                            self._create_chunk(
                                chunk_text,
                                current_chunk_start,
                                chunk_index,
                                source_id,
                                base_metadata,
                                len(current_chunk_paras),
                            )
                        )
                        chunk_index += 1
                    current_chunk_paras = []
                    current_chunk_length = 0

                # Split large paragraph
                sub_chunks = self._split_large_paragraph(
                    para_text, self.config.chunk_size
                )
                for sub_chunk in sub_chunks:
                    chunks.append(
                        self._create_chunk(
                            sub_chunk,
                            para_start,
                            chunk_index,
                            source_id,
                            base_metadata,
                            1,
                            is_list,
                        )
                    )
                    chunk_index += 1
                    para_start += len(sub_chunk) + 1

                current_chunk_start = para_start
                continue

            # Check if adding this paragraph exceeds chunk size
            separator_length = 2 if current_chunk_paras else 0  # "\n\n"
            if (
                current_chunk_paras
                and current_chunk_length + separator_length + para_length
                > self.config.chunk_size
            ):
                # Create chunk from accumulated paragraphs
                chunk_text = "\n\n".join(current_chunk_paras)
                if len(chunk_text) >= self.config.min_chunk_size:
                    chunks.append(
                        self._create_chunk(
                            chunk_text,
                            current_chunk_start,
                            chunk_index,
                            source_id,
                            base_metadata,
                            len(current_chunk_paras),
                        )
                    )
                    chunk_index += 1

                # Start new chunk
                current_chunk_paras = [para_text]
                current_chunk_length = para_length
                current_chunk_start = para_start
            else:
                # Add to current chunk
                current_chunk_paras.append(para_text)
                current_chunk_length += separator_length + para_length

        # Handle remaining paragraphs
        if current_chunk_paras:
            chunk_text = "\n\n".join(current_chunk_paras)
            if len(chunk_text) >= self.config.min_chunk_size or not chunks:
                chunks.append(
                    self._create_chunk(
                        chunk_text,
                        current_chunk_start,
                        chunk_index,
                        source_id,
                        base_metadata,
                        len(current_chunk_paras),
                    )
                )

        # Update total chunks count
        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def _create_chunk(
        self,
        text: str,
        start_pos: int,
        chunk_index: int,
        source_id: str,
        base_metadata: Dict[str, Any],
        paragraph_count: int,
        is_list: bool = False,
    ) -> TextChunk:
        """Create a chunk with metadata"""
        chunk_metadata = {
            **base_metadata,
            "chunk_type": "paragraph",
            "paragraph_count": paragraph_count,
            "is_list": is_list,
            "first_line": (
                text.split("\n")[0][:50] + "..."
                if len(text.split("\n")[0]) > 50
                else text.split("\n")[0]
            ),
        }

        chunk = TextChunk(
            text=text,
            start_pos=start_pos,
            end_pos=start_pos + len(text),
            chunk_id=f"{source_id}_chunk_{chunk_index}",
            metadata=chunk_metadata,
        )

        self.add_chunk_metadata(chunk, chunk_index, -1, "paragraph")
        return chunk

    def __repr__(self) -> str:
        return f"ParagraphStrategy(chunk_size={self.config.chunk_size})"
