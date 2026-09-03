"""
Semantic chunking strategy

Chunks text based on semantic boundaries like sections, topics, and content
structure. This strategy focuses on text-based semantic analysis without
embeddings.

Rewritten span-first (ADR-091 axiom 1/2). The previous implementation had four
compounding defects that made it the worst strategy in the audit:

* ``_preserve_special_blocks`` substituted ``<<CODE_BLOCK_N>>`` placeholders into
  the text while iterating ``finditer`` matches captured from the PRE-mutation
  string, so every match after the first spliced at stale offsets and destroyed
  content. Both ``preserve_code_blocks`` and ``preserve_tables`` default to true,
  so this was the default path. It also meant chunk spans indexed the substituted
  string while chunk text was the restored one — two different strings.
* Sections began at ``header["end"]``, so the header line itself belonged to no
  chunk. A document that is only headers produced nothing at all.
* Sections below ``min_chunk_size`` were dropped with no ``else``, so 40 short
  sections returned ZERO chunks — the defect ADR-091 cites as decisive.
* ``max_chunk_size`` was never referenced anywhere in the file.

The replacement is a **contiguous span partition** of the document, which makes
totality and non-overlap structural rather than checked afterwards, plus
**protected spans**: a code fence or table is declared atomic and no boundary may
land inside it. Same config flags, honest mechanism — ``preserve_code_blocks``
now means "a code block is indivisible" rather than "a code block is temporarily
replaced by a placeholder".
"""

import re
from typing import Any

from .base import (
    OFFSET_CONTRACT_EXACT,
    ChunkingConfig,
    ChunkingStrategyInterface,
    TextChunk,
)
from .spans import Span, hard_split, is_empty, merge_spans, strip_span
from .structure import (
    CODE_BLOCK,
    HTML_HEADING,
    MARKDOWN_HEADING,
    TABLE,
    protected_spans,
    protecting_span,
)

#: A section: raw span plus the metadata it contributes to its chunks.
Section = tuple[int, int, dict[str, Any]]


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

    #: Span-first: every chunk is a verbatim slice of the source.
    _offset_contract = OFFSET_CONTRACT_EXACT

    def __init__(self, config: ChunkingConfig):
        super().__init__(config)
        self._compile_patterns()

    def _compile_patterns(self):
        """Compile regex patterns for semantic analysis"""
        # Structural patterns come from `structure.py`, which owns the single
        # implementation. This module had the only correct one and kept it
        # private, which is exactly how the ADR-091 census's five forks
        # happened; a heading source needed the same rules, so they moved out
        # rather than being copied. Bound as attributes because they are part of
        # this class's existing surface.
        self.markdown_header_pattern = MARKDOWN_HEADING
        self.html_header_pattern = HTML_HEADING
        self.code_block_pattern = CODE_BLOCK
        self.table_pattern = TABLE

        # Paragraph boundaries within a section
        self.paragraph_pattern = re.compile(r"\n\s*\n+")

        # Topic transition indicators. `\b` matters: without it "Firstly",
        # "Secondary", "Nextcloud" and "Thence" all registered as transitions.
        self.transition_patterns = [
            re.compile(
                r"^(?:however|moreover|furthermore|additionally|consequently"
                r"|therefore|thus)\b",
                re.IGNORECASE | re.MULTILINE,
            ),
            re.compile(
                r"^(?:in conclusion|in summary|to summarize|finally)\b",
                re.IGNORECASE | re.MULTILINE,
            ),
            re.compile(
                r"^(?:first|second|third|next|then|lastly)\b",
                re.IGNORECASE | re.MULTILINE,
            ),
        ]

        # Section breaks
        self.section_break_pattern = re.compile(r"^[\-\*_]{3,}$", re.MULTILINE)

    # ------------------------------------------------------------------
    # Protected spans (the honest replacement for placeholder substitution)
    # ------------------------------------------------------------------

    def _protected_spans(self, text: str) -> list[Span]:
        """Spans no boundary may land inside, merged and disjoint.

        Replaces the substitute-then-restore round trip entirely: the text is
        never rewritten, so offsets stay native and nothing can be spliced away.
        """
        return protected_spans(
            text,
            code_blocks=self.config.preserve_code_blocks,
            tables=self.config.preserve_tables,
        )

    @staticmethod
    def _protects(barriers: list[Span], position: int) -> Span | None:
        """The barrier strictly containing ``position``, if any."""
        return protecting_span(barriers, position)

    # ------------------------------------------------------------------
    # Sections — a contiguous partition of [0, len(text))
    # ------------------------------------------------------------------

    def _section_spans(self, text: str) -> list[Section]:
        """Partition the document into sections, contiguously.

        Contiguity is what makes totality structural: sections tile the whole
        document, so no span can go missing between them and none can nest.
        """
        barriers = self._protected_spans(text)
        headers: list[dict[str, Any]] = []
        for match in self.markdown_header_pattern.finditer(text):
            if self._protects(barriers, match.start()):
                continue  # a '#' comment inside a fenced block is not a heading
            headers.append(
                {
                    "start": match.start(),
                    "end": match.end(),
                    "level": len(match.group(1)),
                    "title": match.group(2).strip(),
                    "type": "markdown",
                }
            )
        for match in self.html_header_pattern.finditer(text):
            if self._protects(barriers, match.start()):
                continue
            headers.append(
                {
                    "start": match.start(),
                    "end": match.end(),
                    "level": int(match.group(1)),
                    "title": re.sub(r"<[^>]+>", "", match.group(2)).strip(),
                    "type": "html",
                }
            )
        headers.sort(key=lambda h: h["start"])

        if not headers:
            return self._topic_section_spans(text)

        sections: list[Section] = []
        if headers[0]["start"] > 0:
            sections.append(
                (
                    0,
                    headers[0]["start"],
                    {"section_type": "introduction", "has_header": False},
                )
            )
        for index, header in enumerate(headers):
            # Starts at the header's START, not its end: the heading line is the
            # most retrieval-valuable line in the section, and excluding it also
            # punched a hole in the partition.
            section_end = (
                headers[index + 1]["start"] if index + 1 < len(headers) else len(text)
            )
            sections.append(
                (
                    header["start"],
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
        return sections

    def _topic_section_spans(self, text: str) -> list[Section]:
        """Sections from topic transitions when the document has no headers."""
        breaks = {m.start() for m in self.section_break_pattern.finditer(text)}
        transitions: set[int] = set()
        for pattern in self.transition_patterns:
            for match in pattern.finditer(text):
                para_start = text.rfind("\n\n", 0, match.start())
                transitions.add(0 if para_start < 0 else para_start + 2)

        cuts = sorted({0, len(text)} | breaks | transitions)
        sections: list[Section] = []
        for start, end in zip(cuts, cuts[1:], strict=False):
            if end <= start:
                continue
            sections.append(
                (
                    start,
                    end,
                    {
                        "section_type": "topic_based",
                        "has_header": False,
                        "boundary_type": (
                            "topic_transition"
                            if start in transitions
                            else "section_break"
                        ),
                    },
                )
            )
        if not sections:
            sections = [(0, len(text), {"section_type": "single", "has_header": False})]
        return sections

    def _merge_undersized(self, sections: list[Section], text: str) -> list[Section]:
        """Fold sections below the floor into a neighbour instead of dropping.

        A header-bearing section merges FORWARD, into the body it introduces: a
        lone ``## Section 3`` chunk is retrievable but useless, while the heading
        plus its answer is the unit a reader actually wants. Everything else
        merges backward. Dropping — the old behaviour — is the one option with no
        correct reading, and on header-dense Markdown it returned zero chunks.
        """
        if not sections:
            return sections
        floor = self.config.min_chunk_size
        cap = self.config.max_chunk_size

        out: list[Section] = []
        # A section held back to be merged FORWARD into the next one. Its
        # metadata is kept as the base so a heading's title/level survives into
        # the chunk that ends up carrying its body.
        pending: Section | None = None

        for start, end, meta in sections:
            if pending is not None:
                start = pending[0]
                meta = {**pending[2], **meta}
                pending = None

            span = strip_span(text, start, end)
            if is_empty(span):
                # Whitespace only: extend the previous section so the partition
                # stays contiguous, or hold it for the next one.
                if out:
                    out[-1] = (out[-1][0], end, out[-1][2])
                else:
                    pending = (start, end, meta)
                continue

            if self._size_of_span(text, span) >= floor:
                out.append((start, end, meta))
                continue

            if meta.get("has_header"):
                pending = (start, end, meta)  # merge forward, into its body
            elif out and self._size(text, out[-1][0], end) <= cap:
                out[-1] = (out[-1][0], end, out[-1][2])  # merge backward
            else:
                pending = (start, end, meta)

        if pending is not None:
            if out and self._size(text, out[-1][0], pending[1]) <= cap:
                out[-1] = (out[-1][0], pending[1], out[-1][2])
            else:
                out.append(pending)  # emit short rather than drop
        return out

    # ------------------------------------------------------------------
    # Splitting a section to the budget
    # ------------------------------------------------------------------

    def _section_units(self, text: str, start: int, end: int, barriers: list[Span]):
        """Paragraph-ish units within a section, never cutting inside a barrier."""
        units: list[Span] = []
        cursor = start
        for match in self.paragraph_pattern.finditer(text, start, end):
            if self._protects(barriers, match.start()):
                continue
            unit = strip_span(text, cursor, match.start())
            cursor = match.end()
            if not is_empty(unit):
                units.append(unit)
        tail = strip_span(text, cursor, end)
        if not is_empty(tail):
            units.append(tail)
        return units

    def _split_section(
        self, text: str, start: int, end: int, barriers: list[Span]
    ) -> list[tuple[Span, bool]]:
        """Split one section into (span, forced) at or under the cap."""
        span = strip_span(text, start, end)
        if is_empty(span):
            return []
        if self._size_of_span(text, span) <= self.config.chunk_size:
            return [(span, False)]

        units = self._section_units(text, span[0], span[1], barriers)
        if not units:
            units = [span]

        out: list[tuple[Span, bool]] = []
        group: list[Span] = []
        for unit in units:
            if group:
                group_start = group[0][0]
                if self._size(text, group_start, unit[1]) > self.config.chunk_size:
                    current = self._size(text, group_start, group[-1][1])
                    fits_cap = (
                        self._size(text, group_start, unit[1])
                        <= self.config.max_chunk_size
                    )
                    if current >= self.config.min_chunk_size or not fits_cap:
                        out.append((merge_spans(group), False))
                        group = []
            group.append(unit)
        if group:
            out.append((merge_spans(group), False))

        # Cap backstop. A single unit — a giant table, a fence, minified content —
        # can still exceed it, and emitting over the cap is worse than an ugly
        # cut: the provider truncates silently or rejects the call.
        capped: list[tuple[Span, bool]] = []
        for (piece_start, piece_end), _forced in out:
            if self._size(text, piece_start, piece_end) <= self.config.max_chunk_size:
                capped.append(((piece_start, piece_end), False))
                continue
            for cut in hard_split(
                text, piece_start, piece_end, self.config.max_chunk_size
            ):
                capped.append((cut, True))
        return capped

    # ------------------------------------------------------------------
    # Public surface
    # ------------------------------------------------------------------

    def chunk(
        self, text: str, source_id: str, base_metadata: dict[str, Any] | None = None
    ) -> list[TextChunk]:
        """Create chunks based on semantic boundaries"""
        self.validate_config()

        if not text or not text.strip():
            return []

        base_metadata = base_metadata or {}
        barriers = self._protected_spans(text)
        sections = self._merge_undersized(self._section_spans(text), text)

        chunks: list[TextChunk] = []
        for start, end, section_metadata in sections:
            pieces = self._split_section(text, start, end, barriers)
            multi = len(pieces) > 1
            for sub_index, (span, forced) in enumerate(pieces):
                metadata = {
                    **base_metadata,
                    **section_metadata,
                    "chunk_type": "semantic_split" if multi else "semantic",
                    "forced_split": forced,
                }
                if multi:
                    metadata["parent_section"] = section_metadata.get(
                        "header_title", "untitled"
                    )
                    metadata["sub_index"] = sub_index
                chunk = TextChunk(
                    text=text[span[0] : span[1]],
                    start_pos=span[0],
                    end_pos=span[1],
                    chunk_id=f"{source_id}_chunk_{len(chunks)}",
                    metadata=metadata,
                )
                self.add_chunk_metadata(chunk, len(chunks), -1, "semantic")
                chunks.append(chunk)

        for chunk in chunks:
            chunk.metadata["total_chunks"] = len(chunks)
        return chunks

    def preferred_boundaries(
        self,
        text: str,
        source_id: str,
        base_metadata: dict[str, Any] | None = None,
    ) -> list[int]:
        """Expose section and paragraph ends, not legacy character grouping.

        Without this override the budgeter fell back to the base default, which
        derives boundaries from ``chunk().end_pos`` — i.e. from output rather
        than from structure.
        """
        barriers = self._protected_spans(text)
        boundaries: set[int] = {len(text)}
        for start, end, _meta in self._section_spans(text):
            span = strip_span(text, start, end)
            if not is_empty(span):
                boundaries.add(span[1])
            for unit in self._section_units(text, start, end, barriers):
                boundaries.add(unit[1])
        return sorted(b for b in boundaries if 0 < b <= len(text))

    def __repr__(self) -> str:
        return f"SemanticStrategy(chunk_size={self.config.chunk_size})"
