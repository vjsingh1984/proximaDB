"""Document structure — heading outlines and the spans a cut may not enter.

Extracted from ``SemanticStrategy``, which had the only correct implementation
of both and kept it private. The ADR-091 census found the same five algorithms
forked across five codebases because the good one was never reachable; forking
it a sixth time inside this package, to build a heading source, would have been
the same mistake at smaller scale.

So there is one implementation. ``SemanticStrategy`` keeps its behaviour exactly
(the golden snapshot asserts it) and now shares this code rather than owning it.

What was actually missing
-------------------------
Heading *detection* was never the gap — ``semantic.py`` already finds markdown
and HTML headings and already excludes a ``#`` inside a fenced code block. What
it discards is the **hierarchy**: it records that a chunk sits under a level-2
heading titled "Docker", but not that the full path is
``Installation > Docker``. The level and title alone do not identify a section —
half a dozen documents have a "Configuration" heading — whereas the path does,
which is what makes it worth carrying into retrieval.
"""

from __future__ import annotations

import bisect
import re
from dataclasses import dataclass, field
from typing import Any

from .spans import Span

#: Markdown ATX headings.
MARKDOWN_HEADING = re.compile(r"^(#{1,6})\s+(.+)$", re.MULTILINE)

#: HTML headings. The attribute-tolerant form matters: a bare ``<h([1-6])>``
#: never matched ordinary markup like ``<h2 class="hdr">``, so real HTML was
#: never split by headings at all.
HTML_HEADING = re.compile(r"<h([1-6])\b[^>]*>(.*?)</h\1>", re.IGNORECASE | re.DOTALL)

#: Fenced code, either fence style.
CODE_BLOCK = re.compile(r"```[\s\S]*?```|~~~[\s\S]*?~~~")

#: Simple markdown tables.
TABLE = re.compile(r"^\|.*\|$[\s\S]*?(?=\n\n|\Z)", re.MULTILINE)


def merge_disjoint(spans: list[Span]) -> list[Span]:
    """Sort and union overlapping spans into a disjoint, ordered list."""
    if not spans:
        return []
    spans = sorted(spans)
    merged: list[Span] = [spans[0]]
    for start, end in spans[1:]:
        if start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def protected_spans(
    text: str, *, code_blocks: bool = True, tables: bool = True
) -> list[Span]:
    """Spans no boundary may land inside, merged and disjoint.

    Never rewrites the text. The substitute-then-restore round trip this
    replaced is what destroyed content and shifted every offset after it.
    """
    raw: list[Span] = []
    if code_blocks:
        raw += [(m.start(), m.end()) for m in CODE_BLOCK.finditer(text)]
    if tables:
        raw += [(m.start(), m.end()) for m in TABLE.finditer(text)]
    return merge_disjoint(raw)


def protecting_span(barriers: list[Span], position: int) -> Span | None:
    """The barrier *strictly* containing ``position``, if any."""
    if not barriers:
        return None
    index = bisect.bisect_right([b[0] for b in barriers], position) - 1
    if index < 0:
        return None
    start, end = barriers[index]
    return (start, end) if start < position < end else None


@dataclass(frozen=True)
class Heading:
    """One heading: where it is, how deep, and what it says."""

    start: int
    end: int
    level: int
    title: str
    kind: str = "markdown"


def find_headings(text: str, *, barriers: list[Span] | None = None) -> list[Heading]:
    """Every markdown and HTML heading, in document order.

    Headings inside a protected span are skipped: a ``#`` on a line of shell in
    a fenced block is a comment, not a section.
    """
    if barriers is None:
        barriers = protected_spans(text)

    found: list[Heading] = []
    for match in MARKDOWN_HEADING.finditer(text):
        if protecting_span(barriers, match.start()):
            continue
        found.append(
            Heading(
                start=match.start(),
                end=match.end(),
                level=len(match.group(1)),
                title=match.group(2).strip(),
                kind="markdown",
            )
        )
    for match in HTML_HEADING.finditer(text):
        if protecting_span(barriers, match.start()):
            continue
        found.append(
            Heading(
                start=match.start(),
                end=match.end(),
                level=int(match.group(1)),
                title=re.sub(r"<[^>]+>", "", match.group(2)).strip(),
                kind="html",
            )
        )
    found.sort(key=lambda h: h.start)
    return found


@dataclass
class HeadingOutline:
    """The heading hierarchy of one document, queryable by offset.

    The point of the outline, as opposed to a flat list, is
    :meth:`path_at` — "which section is offset N in", answered as the full
    ancestor chain rather than just the nearest heading.
    """

    headings: list[Heading] = field(default_factory=list)
    #: Ancestor chain for each heading, parallel to :attr:`headings`.
    paths: list[tuple[str, ...]] = field(default_factory=list)
    #: Heading start offsets, for bisect. Built once: `annotate_heading_paths`
    #: resolves one offset PER CHUNK, so rebuilding this list per lookup would
    #: make a feature justified as "no extra cost" quietly O(headings x chunks).
    _starts: list[int] = field(default_factory=list, repr=False, compare=False)

    @classmethod
    def build(cls, text: str, *, barriers: list[Span] | None = None) -> HeadingOutline:
        headings = find_headings(text, barriers=barriers)
        paths: list[tuple[str, ...]] = []
        stack: list[Heading] = []
        for heading in headings:
            # Pop to the first strictly-shallower heading. Using `>=` keeps a
            # sibling from being recorded as its own ancestor.
            while stack and stack[-1].level >= heading.level:
                stack.pop()
            stack.append(heading)
            paths.append(tuple(h.title for h in stack))
        return cls(headings=headings, paths=paths, _starts=[h.start for h in headings])

    def index_at(self, offset: int) -> int:
        """Index of the heading whose section contains ``offset``; -1 if none.

        A section runs from its heading's START (the heading line is part of the
        section it names, and the most retrieval-valuable line in it) to the
        next heading's start.
        """
        if not self.headings:
            return -1
        if len(self._starts) != len(self.headings):  # directly constructed
            self._starts = [h.start for h in self.headings]
        return bisect.bisect_right(self._starts, offset) - 1

    def path_at(self, offset: int) -> tuple[str, ...]:
        """Full ancestor chain for the section containing ``offset``.

        Empty before the first heading — content there belongs to no section,
        and inventing one ("Introduction") would put a title in the index that
        the document never contained.
        """
        index = self.index_at(offset)
        return self.paths[index] if index >= 0 else ()

    def heading_at(self, offset: int) -> Heading | None:
        index = self.index_at(offset)
        return self.headings[index] if index >= 0 else None

    def meaning_at(self, offset: int) -> dict[str, Any]:
        """Retrieval-facing description of the section containing ``offset``."""
        heading = self.heading_at(offset)
        if heading is None:
            return {}
        return {
            "heading_path": list(self.path_at(offset)),
            "heading_title": heading.title,
            "heading_level": heading.level,
            "heading_kind": heading.kind,
        }
