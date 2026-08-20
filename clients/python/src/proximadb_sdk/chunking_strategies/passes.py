"""Chunk passes — the second seam, for capabilities that act *after* the partition.

ADR-091 decomposed chunking into **grid x measure x budget**, which together
produce one total, order-preserving partition of the document into spans. That
covers every capability that decides *where a cut lands*.

It does not cover the rest of TD-CHUNK-3's inventory, and trying to force those
into the grid is what makes them look like unrelated features. Context
enrichment, near-duplicate elision and small-to-big retrieval move no cut at
all. They are transformations *of* the partition, and each one is a deliberate
divergence between three things a chunk conflates today:

============  ================================================  ================
Face          What it is                                        Cost term
============  ================================================  ================
``span``      which part of the document this chunk IS          -- (the axiom)
``embedded``  the text sent to the model to make the vector     **KEU**, once
``retrieved`` the text a caller gets back for generation        read cost, recall
============  ================================================  ================

Every remaining capability is exactly one such divergence:

* **heading paths** -- neither; pure metadata, which is why it is free.
* **dedup** -- whether the span is materialised at all (**KEU** and **KSU** down).
* **context enrichment** -- ``embedded != span`` (**KEU** up, recall up).
* **small-to-big** -- ``retrieved != embedded`` (**KSU** up, read cost down).

So one port covers all four, and the ordering that used to live in comments
becomes a property of the faces. That matters because the alternative was
already visible: two boolean flags inlined in ``TextDocumentProcessor.chunk``
with a paragraph each explaining why they run in that order, and two more
capabilities to add. Four flags have twenty-four orderings and no rule.

Why the face is *declared and then checked*
-------------------------------------------
A pass that declares ``METADATA`` and quietly drops a chunk is a KSU bug that no
test of that pass would catch, because the pass's own tests assert what it
means to do. :func:`run_passes` verifies the face after every pass, which makes
the abstraction load-bearing rather than decorative — the same move
``ChunkingStrategyInterface.__init_subclass__`` makes for ``max_chunk_size``.

Why edges rather than copied text
---------------------------------
Small-to-big needs a parent that is bigger than the child. Writing the parent's
text onto every child would multiply KSU by the fan-out — the exact cost this
program keeps finding. Passes therefore emit :class:`ChunkEdge` *intents* and
leave materialisation to the caller, which is also what keeps this module free
of any client, transport or graph dependency (ORION edges in the server, plain
metadata in an embedded test, whatever anvaiops needs).
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol, runtime_checkable


class Face(str, Enum):
    """Which of a chunk's three faces a pass is allowed to touch.

    Ordered by the only sequence that composes; see :data:`FACE_ORDER`.
    """

    #: Adds or edits ``chunk.metadata``. Must not change the chunk set, any
    #: span, or any text.
    METADATA = "metadata"

    #: May REMOVE chunks. Must not add chunks, and must not alter a surviving
    #: chunk's span or text.
    SELECTION = "selection"

    #: Sets what gets embedded, via ``metadata["embedded_text"]``. Must leave
    #: the chunk set, spans and ``text`` untouched.
    EMBEDDED = "embedded"

    #: Declares where a caller's returned text comes from -- by reference, via
    #: edges and ``metadata["parent_id"]``. May emit new PARENT chunks, and
    #: must not alter existing chunks' spans or text.
    RETRIEVED = "retrieved"


#: The order passes run in, regardless of the order they were configured in.
#:
#: Each adjacency is forced, not conventional:
#:
#: * METADATA before SELECTION -- a survivor must keep the heading path of the
#:   duplicates it absorbed. Measured during TD-CHUNK-3 item 2: reversing this
#:   buys the cost saving and pays for it in retrieval quality.
#: * SELECTION before EMBEDDED -- enriching a chunk that is about to be dropped
#:   spends KEU on nothing. This is the whole reason dedup must be lexical:
#:   the decision has to precede the spend.
#: * EMBEDDED before RETRIEVED -- linkage refers to the chunks that survive and
#:   to what they will actually be embedded as.
FACE_ORDER: tuple[Face, ...] = (
    Face.METADATA,
    Face.SELECTION,
    Face.EMBEDDED,
    Face.RETRIEVED,
)

#: Metadata key carrying the text to embed, when it differs from ``chunk.text``.
#: Absence means "embed the span", which is the legacy behaviour and the default
#: -- the same absence-means-legacy rule the offset markers use.
EMBEDDED_TEXT_KEY = "embedded_text"

#: Metadata key linking a child chunk to the larger chunk a caller should
#: RETURN when this one matches. A reference, never a copy.
PARENT_ID_KEY = "parent_id"

#: ``chunk_type`` marking a chunk that exists to be returned, not embedded.
CHUNK_TYPE_PARENT = "parent"


@dataclass(frozen=True)
class ChunkEdge:
    """A relationship between two chunks, for a caller to materialise.

    Deliberately not written anywhere by this module. The same edge becomes an
    ORION graph edge through a client, a metadata pair in an embedded test, or
    nothing at all if the caller only wants the chunks -- and the chunking layer
    stays a pure function of text, as its package docstring promises.
    """

    source_id: str
    target_id: str
    relation: str
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class PassResult:
    """What one pass produced."""

    chunks: list[Any]
    edges: tuple[ChunkEdge, ...] = ()
    #: Free-form, surfaced in :attr:`PassPipelineResult.stats` under the pass name.
    stats: dict[str, Any] = field(default_factory=dict)


@runtime_checkable
class ChunkPass(Protocol):
    """A transformation of a finished partition."""

    #: Stable identity; keys this pass's entry in the pipeline stats.
    name: str

    #: The single face this pass is permitted to touch.
    face: Face

    def apply(self, document: str, chunks: list[Any]) -> PassResult:
        """Transform ``chunks``. May mutate in place and return them."""
        ...


@dataclass
class PassPipelineResult:
    """The partition after every pass, plus what it cost."""

    chunks: list[Any]
    edges: tuple[ChunkEdge, ...] = ()
    stats: dict[str, Any] = field(default_factory=dict)

    @property
    def span_chars(self) -> int:
        """Characters covered by the surviving spans -- what is stored."""
        return sum(
            int(getattr(c, "end_pos", 0)) - int(getattr(c, "start_pos", 0))
            for c in self.chunks
            if getattr(c, "metadata", {}).get("chunk_type") != CHUNK_TYPE_PARENT
        )

    @property
    def embedded_chars(self) -> int:
        """Characters actually sent to the embedding model -- what KEU bills.

        The instrument this seam exists to expose. Before it, "does enrichment
        pay for itself" could not even be *asked* in the pipeline, because
        there was one text and it was both the span and the payload.
        """
        total = 0
        for chunk in self.chunks:
            metadata = getattr(chunk, "metadata", None) or {}
            if metadata.get("chunk_type") == CHUNK_TYPE_PARENT:
                continue  # parents are returned, not embedded
            total += len(embedded_text_of(chunk))
        return total

    @property
    def enrichment_tax(self) -> float:
        """``embedded_chars / span_chars`` -- 1.0 when nothing was enriched.

        Quoted as the KEU multiplier a configuration costs, so the decision to
        enable enrichment is made against a number rather than an intuition.
        """
        span = self.span_chars
        return (self.embedded_chars / span) if span else 1.0


def embedded_text_of(chunk: Any) -> str:
    """What to embed for ``chunk``: the declared text, else its own.

    One accessor so no caller re-derives the absence rule. Consumers that
    embed (``DocumentProcessor.prepare_for_embedding``, batch ingest, anvaiops)
    route through this and therefore cannot miss an enrichment.
    """
    metadata = getattr(chunk, "metadata", None) or {}
    declared = metadata.get(EMBEDDED_TEXT_KEY)
    if isinstance(declared, str) and declared:
        return declared
    return getattr(chunk, "text", "")


def _fingerprint(chunks: Sequence[Any]) -> list[tuple[str, int, int, str]]:
    return [
        (
            str(getattr(c, "chunk_id", "")),
            int(getattr(c, "start_pos", 0)),
            int(getattr(c, "end_pos", 0)),
            getattr(c, "text", ""),
        )
        for c in chunks
    ]


def _check_face(pass_: ChunkPass, before: Sequence[Any], after: Sequence[Any]) -> None:
    """Fail loudly when a pass exceeded the face it declared.

    Loudly rather than defensively: a pass that removes chunks while claiming
    METADATA is silently deleting a tenant's content, and the only safe moment
    to notice is the one where it happens.
    """
    face = pass_.face
    old = _fingerprint(before)
    new = _fingerprint(after)
    old_by_id = {row[0]: row for row in old}

    if face is Face.METADATA:
        if new != old:
            raise ValueError(
                f"pass {pass_.name!r} declares face={face.value} but changed the "
                "chunk set, a span or a text. A metadata pass must be a no-op "
                "on everything a reader can slice."
            )
        return

    if face is Face.SELECTION:
        if len(new) > len(old):
            raise ValueError(
                f"pass {pass_.name!r} declares face={face.value} but ADDED "
                f"chunks ({len(old)} -> {len(new)}). Selection may only remove."
            )
        for row in new:
            if old_by_id.get(row[0]) != row:
                raise ValueError(
                    f"pass {pass_.name!r} declares face={face.value} but altered "
                    f"surviving chunk {row[0]!r}. Selection chooses; it does not edit."
                )
        return

    if face is Face.EMBEDDED:
        if new != old:
            raise ValueError(
                f"pass {pass_.name!r} declares face={face.value} but changed a "
                "span, a text or the chunk set. Enrichment changes what is SENT "
                "to the model, never what the chunk IS -- otherwise the offsets "
                "stop describing the document."
            )
        return

    # RETRIEVED: may append parents; existing chunks must survive untouched.
    added = [row for row in new if row[0] not in old_by_id]
    for row in new:
        if row[0] in old_by_id and old_by_id[row[0]] != row:
            raise ValueError(
                f"pass {pass_.name!r} declares face={face.value} but altered "
                f"existing chunk {row[0]!r}."
            )
    if len(new) - len(added) != len(old):
        raise ValueError(
            f"pass {pass_.name!r} declares face={face.value} but DROPPED chunks. "
            "Linkage adds a coarser view; it never removes the fine one."
        )


def run_passes(
    document: str, chunks: list[Any], passes: Sequence[ChunkPass]
) -> PassPipelineResult:
    """Run ``passes`` in :data:`FACE_ORDER`, verifying each stayed in its face.

    Configuration order is deliberately ignored. Ordering here is a correctness
    property derived from the faces, not a caller preference -- a caller that
    could reorder these could buy a cost saving and silently pay for it in
    retrieval quality, which is precisely the mistake the ordering encodes
    against.
    """
    ordered = sorted(passes, key=lambda p: FACE_ORDER.index(p.face))
    edges: list[ChunkEdge] = []
    stats: dict[str, Any] = {}
    current = list(chunks)

    for pass_ in ordered:
        before = list(current)
        result = pass_.apply(document, current)
        if not isinstance(result, PassResult):  # tolerate a bare list
            result = PassResult(chunks=list(result))
        _check_face(pass_, before, result.chunks)
        current = list(result.chunks)
        edges.extend(result.edges)
        if result.stats:
            stats[pass_.name] = result.stats

    return PassPipelineResult(chunks=current, edges=tuple(edges), stats=stats)
