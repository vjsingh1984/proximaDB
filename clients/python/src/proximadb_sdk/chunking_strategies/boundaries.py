"""Boundary sources — *proposing* cuts, separately from *choosing* them.

ADR-091 D2. Today's eight "strategies" conflate two responsibilities, and that
conflation is why the invariants kept getting re-broken: a component that both
proposes boundaries and enforces a budget has to get both right every time, in
every strategy, forever.

Split, they are:

* a **boundary source** proposes candidate cuts and says what each one *means*;
* a **segmenter** consumes candidates plus a budget and returns a total
  partition of exact spans.

The second is where totality, overlap and the size cap live, and there should be
exactly one of it. The first is where document knowledge lives, and there can be
many — which is the capability this module adds, because a document has more
than one kind of structure at once. A markdown file has headings *and*
paragraphs *and* fenced code blocks, and today a caller must pick one strategy
and lose the other two.

This generalises what already works
-----------------------------------
``ChunkingStrategyInterface.preferred_boundaries()`` is already the proposal
seam, and ``TokenBudgetStrategy`` is already the segmenter — it is the layer that
survived the ADR-091 audit with exact offsets, precisely because it slices the
original text instead of rejoining pieces. So this is not a new idea being
introduced; it is the shape the code already has, made explicit and composable.

What ``meaning`` is for
-----------------------
A cut position alone is lossy. "This is the end of the section titled
*Installation > Docker*" is the part a retrieval system wants, and the part every
implementation in the ADR-091 census threw away. ``meaning`` carries it — the
heading path, the table identity, the JSON pointer — so a boundary can enrich the
chunk it produces instead of merely ending it.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol, runtime_checkable

from .structure import HeadingOutline, protected_spans


class BoundaryKind(str, Enum):
    """What kind of structure proposed a cut.

    Ordered loosely from strongest to weakest as a *document* division, which is
    the order :func:`merge_boundaries` uses to break ties. A heading is a more
    meaningful place to end a chunk than a sentence, so when both land on the
    same offset the heading's meaning is the one worth keeping.
    """

    DOCUMENT = "document"
    HEADING = "heading"
    SECTION = "section"
    CODE_BLOCK = "code_block"
    TABLE = "table"
    PARAGRAPH = "paragraph"
    SENTENCE = "sentence"
    #: A proposal with no structural claim — a fixed window, a fallback cut.
    ARBITRARY = "arbitrary"


#: Strength order for tie-breaking. Explicit rather than derived from the enum's
#: declaration order, so reordering the enum for readability cannot silently
#: change segmentation.
_KIND_STRENGTH: dict[BoundaryKind, int] = {
    BoundaryKind.DOCUMENT: 70,
    BoundaryKind.HEADING: 60,
    BoundaryKind.SECTION: 50,
    BoundaryKind.CODE_BLOCK: 40,
    BoundaryKind.TABLE: 40,
    BoundaryKind.PARAGRAPH: 30,
    BoundaryKind.SENTENCE: 20,
    BoundaryKind.ARBITRARY: 0,
}


@dataclass(frozen=True)
class Boundary:
    """One proposed cut: where it is, what kind it is, and what it means.

    ``end`` is an *end* offset — the position a chunk may finish at — because
    that is what a segmenter needs and what ``preferred_boundaries`` already
    returns. Expressing it as a start would require every consumer to convert.
    """

    end: int
    kind: BoundaryKind = BoundaryKind.ARBITRARY
    #: Structural context this cut carries (heading path, JSON pointer, table
    #: id). Deliberately an open mapping: the set of useful keys is
    #: document-type-specific and should not require changing this class.
    meaning: Mapping[str, Any] = field(default_factory=dict)

    @property
    def strength(self) -> int:
        return _KIND_STRENGTH.get(self.kind, 0)


@runtime_checkable
class BoundarySource(Protocol):
    """Proposes candidate cuts over a document. Composable; never total.

    A source is explicitly **not** required to cover the document, respect a
    budget, or avoid overlap. Those are the segmenter's obligations, and keeping
    them out of here is the entire point of the split — it is what lets a source
    be written by someone who knows about markdown but nothing about token
    budgets.
    """

    name: str

    def boundaries(
        self,
        text: str,
        *,
        source_id: str = "doc",
        base_metadata: dict[str, Any] | None = None,
    ) -> Sequence[Boundary]: ...


class StrategyBoundarySource:
    """Adapts any existing strategy into a source. The compatibility facade.

    Every strategy already answers ``preferred_boundaries``, so each one becomes
    a source without being rewritten — which is what lets the enum keep working
    while the architecture underneath it changes (ADR-091 D2's "the enum
    survives as a compatibility facade so no caller breaks").

    The strategy's own name and a declared kind are attached, so a composite can
    tell a sentence proposal from a paragraph one even though the underlying
    call returns bare integers.
    """

    __slots__ = ("_strategy", "kind", "name")

    def __init__(
        self,
        strategy: Any,
        *,
        kind: BoundaryKind = BoundaryKind.ARBITRARY,
        name: str | None = None,
    ) -> None:
        self._strategy = strategy
        self.kind = kind
        configured = getattr(getattr(strategy, "config", None), "strategy", None)
        self.name = name or str(getattr(configured, "value", type(strategy).__name__))

    def boundaries(
        self,
        text: str,
        *,
        source_id: str = "doc",
        base_metadata: dict[str, Any] | None = None,
    ) -> Sequence[Boundary]:
        ends = self._strategy.preferred_boundaries(text, source_id, base_metadata)
        return tuple(
            Boundary(end=end, kind=self.kind, meaning={"source": self.name})
            for end in ends
            if 0 < end <= len(text)
        )

    def __repr__(self) -> str:
        return f"StrategyBoundarySource({self.name!r}, kind={self.kind.value})"


def merge_boundaries(groups: Sequence[Sequence[Boundary]]) -> tuple[Boundary, ...]:
    """Union several sources' proposals into one ordered, deduplicated list.

    Two sources agreeing on an offset is the common case, not the exception —
    a heading is also a paragraph break. Keeping both would double-count that
    position and let an arbitrary one win by ordering; keeping the **strongest**
    preserves the most informative ``meaning``, which is the thing sources exist
    to carry.

    Merging is deliberately plain code rather than a source that wraps sources:
    it needs every group at once, so there is nothing to gain from deferring it,
    and a function is far easier to reason about than a recursive composite.
    """
    strongest: dict[int, Boundary] = {}
    for group in groups:
        for boundary in group:
            existing = strongest.get(boundary.end)
            if existing is None or boundary.strength > existing.strength:
                strongest[boundary.end] = boundary
    return tuple(strongest[end] for end in sorted(strongest))


class CompositeBoundarySource:
    """Several sources over one document, merged.

    The capability that did not exist before the split. A markdown file has
    headings, paragraphs and fenced code blocks simultaneously; previously a
    caller chose one strategy and lost the rest.
    """

    __slots__ = ("name", "sources")

    def __init__(self, *sources: BoundarySource, name: str = "composite") -> None:
        self.sources = sources
        self.name = name

    def boundaries(
        self,
        text: str,
        *,
        source_id: str = "doc",
        base_metadata: dict[str, Any] | None = None,
    ) -> Sequence[Boundary]:
        return merge_boundaries(
            [
                source.boundaries(
                    text, source_id=source_id, base_metadata=base_metadata
                )
                for source in self.sources
            ]
        )

    def __repr__(self) -> str:
        names = ", ".join(s.name for s in self.sources)
        return f"CompositeBoundarySource({names})"


class HeadingBoundarySource:
    """Proposes a cut where each section begins, carrying its hierarchy path.

    The first source that is not an adapter over an existing strategy, and the
    thing the SDK lacked entirely (ADR-091 D2: ``HEADING`` "which the SDK lacks
    entirely" becomes a source carrying the hierarchy path).

    Where the value actually is
    ---------------------------
    Not in the cut positions. Measured while landing the port: a heading START
    and the preceding paragraph END routinely fall within one token of each
    other, and the segmenter resolves candidates on the token grid, so a heading
    source frequently moves no cut at all. Its worth is the ``meaning`` it
    carries — which is why :func:`annotate_heading_paths` exists and is the
    recommended way to use this.

    Each boundary describes the section it **closes**, because the segmenter
    attaches a boundary's meaning to the chunk that ENDS there.
    """

    __slots__ = ("name", "_barriers_enabled")

    def __init__(self, *, name: str = "heading", respect_code_blocks: bool = True):
        self.name = name
        self._barriers_enabled = respect_code_blocks

    def boundaries(
        self,
        text: str,
        *,
        source_id: str = "doc",
        base_metadata: dict[str, Any] | None = None,
    ) -> Sequence[Boundary]:
        outline = HeadingOutline.build(
            text,
            barriers=protected_spans(text) if self._barriers_enabled else [],
        )
        proposals: list[Boundary] = []
        for heading in outline.headings:
            if heading.start <= 0:
                continue  # a cut at 0 divides nothing
            # The section CLOSING here is the one containing the character
            # before this heading.
            proposals.append(
                Boundary(
                    end=heading.start,
                    kind=BoundaryKind.HEADING,
                    meaning=outline.meaning_at(heading.start - 1),
                )
            )
        if text:
            proposals.append(
                Boundary(
                    end=len(text),
                    kind=BoundaryKind.DOCUMENT,
                    meaning=outline.meaning_at(max(0, len(text) - 1)),
                )
            )
        return tuple(proposals)

    def __repr__(self) -> str:
        return f"HeadingBoundarySource({self.name!r})"


def annotate_heading_paths(
    text: str,
    chunks: Sequence[Any],
    *,
    respect_code_blocks: bool = True,
    key: str = "heading_path",
) -> Sequence[Any]:
    """Label every chunk with the heading path it lives under. In place.

    A post-segmentation pass rather than a boundary source, and deliberately so.
    A source can only describe chunks that happen to END on one of its
    proposals; a chunk that ended mid-section because the budget ran out gets
    nothing. Most chunks are that kind. Labelling by LOOKUP instead of by
    coincidence is what makes this work for every chunk, under every strategy,
    including ones with no structural awareness at all.

    Co-design: this is metadata on an existing chunk. No extra vectors, so no
    KSU or KEU delta -- strictly free retrieval quality, which is why TD-CHUNK-3
    sequences it first.

    Chunks before the first heading are left unlabelled rather than given an
    invented "Introduction", which would put a title in the index that the
    document never contained.
    """
    outline = HeadingOutline.build(
        text, barriers=protected_spans(text) if respect_code_blocks else []
    )
    if not outline.headings:
        return chunks
    for chunk in chunks:
        start = int(getattr(chunk, "start_pos", 0))
        meaning = outline.meaning_at(start)
        if not meaning:
            continue
        metadata = getattr(chunk, "metadata", None)
        if isinstance(metadata, dict):
            metadata[key] = meaning["heading_path"]
            metadata["heading_title"] = meaning["heading_title"]
            metadata["heading_level"] = meaning["heading_level"]
    return chunks
