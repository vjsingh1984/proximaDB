"""
Semantic chunking strategy

Chunks text based on semantic boundaries like sections, topics, and content structure.
This strategy focuses on text-based semantic analysis without embeddings.
"""

import re
from typing import List, Dict, Any, Optional, Tuple
from .base import ChunkingStrategyInterface, TextChunk, ChunkingConfig


class SemanticStrategy(ChunkingStrategyInterface):
    """
    Semantic chunking based on content structure and topic boundaries

    Uses text-based analysis to identify semantic boundaries:
    - Section headers (Markdown, HTML, etc.)
    - Topic transitions
    - Content type changes
    - Structural elements (code blocks, tables, etc.)

    Note: This does NOT use embeddings - that's a separate concern
    """

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile regex patterns for semantic analysis"""
        # Markdown headers
        self.markdown_header_pattern = re.compile(r"^(#{1,6})\s+(.+)$", re.MULTILINE)

        # HTML headers
        self.html_header_pattern = re.compile(r"<h([1-6])>(.*?)</h\1>", re.IGNORECASE)

        # Code blocks
        self.code_block_pattern = re.compile(r"```[\s\S]*?```|~~~[\s\S]*?~~~")

        # Tables (simple markdown)
        self.table_pattern = re.compile(r"^\|.*\|$[\s\S]*?(?=\n\n|\Z)", re.MULTILINE)

        # Topic transition indicators
        self.transition_patterns = [
            re.compile(
                r"^(however|moreover|furthermore|additionally|consequently|therefore|thus)",
                re.IGNORECASE | re.MULTILINE,
            ),
            re.compile(
                r"^(in conclusion|in summary|to summarize|finally)",
                re.IGNORECASE | re.MULTILINE,
            ),
            re.compile(
                r"^(first|second|third|next|then|lastly)", re.IGNORECASE | re.MULTILINE
            ),
        ]

        # Section breaks
        self.section_break_pattern = re.compile(r"^[\-\*_]{3,}$", re.MULTILINE)

    def _identify_sections(
        self, text: str
    ) -> List[Tuple[str, int, int, Dict[str, Any]]]:
        """Identify semantic sections in the text"""
        sections = []

        # Find all headers
        headers = []

        # Markdown headers
        for match in self.markdown_header_pattern.finditer(text):
            level = len(match.group(1))
            title = match.group(2).strip()
            headers.append(
                {
                    "start": match.start(),
                    "end": match.end(),
                    "level": level,
                    "title": title,
                    "type": "markdown",
                }
            )

        # HTML headers
        for match in self.html_header_pattern.finditer(text):
            level = int(match.group(1))
            title = match.group(2).strip()
            headers.append(
                {
                    "start": match.start(),
                    "end": match.end(),
                    "level": level,
                    "title": title,
                    "type": "html",
                }
            )

        # Sort headers by position
        headers.sort(key=lambda h: h["start"])

        # Create sections based on headers
        if headers:
            # First section (before first header)
            if headers[0]["start"] > 0:
                sections.append(
                    (
                        text[: headers[0]["start"]].strip(),
                        0,
                        headers[0]["start"],
                        {"section_type": "introduction", "has_header": False},
                    )
                )

            # Sections with headers
            for i, header in enumerate(headers):
                section_start = header["end"]
                section_end = (
                    headers[i + 1]["start"] if i + 1 < len(headers) else len(text)
                )

                section_text = text[section_start:section_end].strip()
                if section_text:
                    sections.append(
                        (
                            section_text,
                            section_start,
                            section_end,
                            {
                                "section_type": "content",
                                "has_header": True,
                                "header_level": header["level"],
                                "header_title": header["title"],
                                "header_type": header["type"],
                            },
                        )
                    )
        else:
            # No headers found - use other semantic boundaries
            sections = self._identify_topic_sections(text)

        return sections

    def _identify_topic_sections(
        self, text: str
    ) -> List[Tuple[str, int, int, Dict[str, Any]]]:
        """Identify sections based on topic transitions"""
        sections = []

        # Find section breaks
        breaks = []
        for match in self.section_break_pattern.finditer(text):
            breaks.append(match.start())

        # Find topic transitions
        transitions = []
        for pattern in self.transition_patterns:
            for match in pattern.finditer(text):
                # Find the start of the paragraph containing the transition
                para_start = text.rfind("\n\n", 0, match.start()) + 2
                if para_start == 1:  # No paragraph break found
                    para_start = 0
                transitions.append(para_start)

        # Combine and sort boundaries
        boundaries = sorted(set([0] + breaks + transitions + [len(text)]))

        # Create sections
        for i in range(len(boundaries) - 1):
            section_text = text[boundaries[i] : boundaries[i + 1]].strip()
            if section_text:
                sections.append(
                    (
                        section_text,
                        boundaries[i],
                        boundaries[i + 1],
                        {
                            "section_type": "topic_based",
                            "has_header": False,
                            "boundary_type": (
                                "topic_transition"
                                if boundaries[i] in transitions
                                else "section_break"
                            ),
                        },
                    )
                )

        return (
            sections
            if sections
            else [(text, 0, len(text), {"section_type": "single", "has_header": False})]
        )

    def _preserve_special_blocks(self, text: str) -> Tuple[str, List[Dict[str, Any]]]:
        """Extract and preserve special blocks (code, tables)"""
        preserved_blocks = []

        # Extract code blocks
        for match in self.code_block_pattern.finditer(text):
            block_id = f"<<CODE_BLOCK_{len(preserved_blocks)}>>"
            preserved_blocks.append(
                {
                    "id": block_id,
                    "content": match.group(0),
                    "type": "code",
                    "start": match.start(),
                    "end": match.end(),
                }
            )
            text = text[: match.start()] + block_id + text[match.end() :]

        # Extract tables
        for match in self.table_pattern.finditer(text):
            block_id = f"<<TABLE_BLOCK_{len(preserved_blocks)}>>"
            preserved_blocks.append(
                {
                    "id": block_id,
                    "content": match.group(0),
                    "type": "table",
                    "start": match.start(),
                    "end": match.end(),
                }
            )
            text = text[: match.start()] + block_id + text[match.end() :]

        return text, preserved_blocks

    def _restore_special_blocks(
        self, text: str, preserved_blocks: List[Dict[str, Any]]
    ) -> str:
        """Restore preserved special blocks"""
        for block in preserved_blocks:
            text = text.replace(block["id"], block["content"])
        return text

    def chunk(
        self, text: str, source_id: str, base_metadata: Optional[Dict[str, Any]] = None
    ) -> List[TextChunk]:
        """Create chunks based on semantic boundaries"""
        self.validate_config()

        if not text:
            return []

        base_metadata = base_metadata or {}
        chunks = []

        # Preserve special blocks if configured
        preserved_blocks = []
        if self.config.preserve_code_blocks or self.config.preserve_tables:
            text, preserved_blocks = self._preserve_special_blocks(text)

        # Identify semantic sections
        sections = self._identify_sections(text)

        # Process sections into chunks
        chunk_index = 0

        for section_text, start_pos, end_pos, section_metadata in sections:
            # Restore special blocks in section
            if preserved_blocks:
                section_text = self._restore_special_blocks(
                    section_text, preserved_blocks
                )

            # Check section size
            if len(section_text) <= self.config.chunk_size:
                # Section fits in one chunk
                if len(section_text) >= self.config.min_chunk_size:
                    chunk_metadata = {
                        **base_metadata,
                        **section_metadata,
                        "chunk_type": "semantic",
                    }

                    chunk = TextChunk(
                        text=section_text,
                        start_pos=start_pos,
                        end_pos=end_pos,
                        chunk_id=f"{source_id}_chunk_{chunk_index}",
                        metadata=chunk_metadata,
                    )

                    self.add_chunk_metadata(chunk, chunk_index, -1, "semantic")
                    chunks.append(chunk)
                    chunk_index += 1
            else:
                # Section too large - split it
                sub_chunks = self._split_large_section(
                    section_text,
                    start_pos,
                    source_id,
                    chunk_index,
                    base_metadata,
                    section_metadata,
                )
                chunks.extend(sub_chunks)
                chunk_index += len(sub_chunks)

        # Update total chunks count
        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)

        return chunks

    def _split_large_section(
        self,
        text: str,
        start_pos: int,
        source_id: str,
        chunk_index: int,
        base_metadata: Dict[str, Any],
        section_metadata: Dict[str, Any],
    ) -> List[TextChunk]:
        """Split a large section into smaller chunks"""
        chunks = []

        # Try paragraph-based splitting first
        paragraphs = re.split(r"\n\s*\n+", text)

        current_chunk_text = ""
        current_chunk_start = start_pos
        sub_index = 0

        for para in paragraphs:
            para = para.strip()
            if not para:
                continue

            # Check if adding paragraph exceeds size
            separator = "\n\n" if current_chunk_text else ""
            if (
                current_chunk_text
                and len(current_chunk_text) + len(separator) + len(para)
                > self.config.chunk_size
            ):
                # Create chunk
                if len(current_chunk_text) >= self.config.min_chunk_size:
                    chunk_metadata = {
                        **base_metadata,
                        **section_metadata,
                        "chunk_type": "semantic_split",
                        "parent_section": section_metadata.get(
                            "header_title", "untitled"
                        ),
                        "sub_index": sub_index,
                    }

                    chunk = TextChunk(
                        text=current_chunk_text,
                        start_pos=current_chunk_start,
                        end_pos=current_chunk_start + len(current_chunk_text),
                        chunk_id=f"{source_id}_chunk_{chunk_index + sub_index}",
                        metadata=chunk_metadata,
                    )

                    self.add_chunk_metadata(
                        chunk, chunk_index + sub_index, -1, "semantic"
                    )
                    chunks.append(chunk)
                    sub_index += 1

                # Start new chunk
                current_chunk_text = para
                current_chunk_start += len(current_chunk_text) + len(separator)
            else:
                # Add to current chunk
                current_chunk_text += separator + para

        # Handle remaining text
        if current_chunk_text and (
            len(current_chunk_text) >= self.config.min_chunk_size or not chunks
        ):
            chunk_metadata = {
                **base_metadata,
                **section_metadata,
                "chunk_type": "semantic_split",
                "parent_section": section_metadata.get("header_title", "untitled"),
                "sub_index": sub_index,
            }

            chunk = TextChunk(
                text=current_chunk_text,
                start_pos=current_chunk_start,
                end_pos=current_chunk_start + len(current_chunk_text),
                chunk_id=f"{source_id}_chunk_{chunk_index + sub_index}",
                metadata=chunk_metadata,
            )

            self.add_chunk_metadata(chunk, chunk_index + sub_index, -1, "semantic")
            chunks.append(chunk)

        return chunks

    def __repr__(self) -> str:
        return f"SemanticStrategy(chunk_size={self.config.chunk_size})"
