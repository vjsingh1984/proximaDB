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
