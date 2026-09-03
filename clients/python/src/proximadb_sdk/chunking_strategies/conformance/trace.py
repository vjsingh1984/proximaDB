"""Per-document chunking cost trace (TD-CHUNK-1 deliverable 3).

Chunking is the write-side cost allocator: chunk count decides how many vectors
exist, which fixes spend in embedding (paid once) and storage (paid forever), and
sets the vector count that drives ANN probe and GET counts at read time. Per the
co-design mandate a component that moves a dimensional cost term must make that
term *observable*, and today none of the ecosystem's chunkers can report their own
coverage.

Every defect ADR-091 catalogues is a field in here:

* ``units_covered < units_in``      -> the silent text-loss bug
* ``spans_dropped > 0``             -> a span was discarded rather than merged
* ``units_duplicated`` unexplained  -> overlap that nobody asked for
* ``max_chunk_units > budget``      -> the unenforced size cap

The trace is also the pre-embedding cost estimate, so ingest spend is predictable
*before* it is incurred rather than reconcilable afterwards.

Units, not bytes
----------------
Offsets in this SDK are *character* offsets on the text strategies and *byte*
offsets on the code path — one field, two bases, no marker (ADR-091 axiom 2;
resolving that is TD-CHUNK-2's job, guarded by the ``offset_basis`` marker
TD-CG2 introduces). This module therefore measures in whichever unit the spans
are expressed in and records which via :attr:`ChunkTrace.offset_basis`, rather
than asserting a basis that does not yet hold. The filed TD used provisional
``bytes_*`` field names; the precise ``units_*`` naming is used here so the
ambiguity is not baked into a new contract.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any

#: Basis in which chunk offsets are expressed.
BASIS_CHAR = "char"
BASIS_BYTE = "byte"


@dataclass(frozen=True)
class Gap:
    """A maximal run of input units covered by no chunk."""

    start: int
    end: int
    preview: str

    @property
    def length(self) -> int:
        return self.end - self.start


@dataclass(frozen=True)
class ChunkTrace:
    """Observable cost and coverage of one chunking call over one document."""

    units_in: int
    units_covered: int
    units_duplicated: int
    chunk_count: int
    spans_dropped: int
    min_chunk_units: int
    max_chunk_units: int
    offset_basis: str = BASIS_CHAR
    #: What ``*_chunk_units`` are counted in. A trace that reports a size
    #: without naming its unit is unreadable the moment a second measure
    #: exists: "max 512" is fine against a 512-CHARACTER budget and a serious
    #: overflow against a 512-TOKEN one, and nothing in the number says which.
    size_unit: str = "char"
    tokens_out: int | None = None
    boundary_sources: dict[str, int] = field(default_factory=dict)
    gaps: tuple[Gap, ...] = ()

    @property
    def is_total(self) -> bool:
        """True when every non-whitespace input unit landed in some chunk.

        ADR-091 axiom 1. This is the single property whose violation is always a
        data-loss bug rather than a tuning choice.
        """
        return self.units_covered >= self.units_in and self.spans_dropped == 0

    @property
    def coverage_ratio(self) -> float:
        if self.units_in == 0:
            return 1.0
        return self.units_covered / self.units_in

    @property
    def duplication_ratio(self) -> float:
        if self.units_in == 0:
            return 0.0
        return self.units_duplicated / self.units_in

    def summary(self) -> str:
        """One-line, log-friendly rendering."""
        lost = self.units_in - self.units_covered
        return (
            f"chunks={self.chunk_count} basis={self.offset_basis} "
            f"in={self.units_in} covered={self.units_covered} "
            f"lost={lost} dup={self.units_duplicated} "
            f"dropped_spans={self.spans_dropped} "
            f"size[min/max]={self.min_chunk_units}/{self.max_chunk_units}"
            f" ({self.size_unit})"
        )


def _merge_spans(
    spans: Sequence[tuple[int, int]], limit: int
) -> tuple[list[tuple[int, int]], int]:
    """Union the spans, clamped to ``[0, limit]``; return (merged, duplicated).

    ``duplicated`` counts units covered more than once, which is the overlap the
    caller is paying for twice in embedding and storage.
    """
    clamped = sorted(
        (max(0, s), min(limit, e)) for s, e in spans if min(limit, e) > max(0, s)
    )
    merged: list[tuple[int, int]] = []
    duplicated = 0
    for start, end in clamped:
        if merged and start < merged[-1][1]:
            duplicated += min(end, merged[-1][1]) - start
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        elif merged and start == merged[-1][1]:
            merged[-1] = (merged[-1][0], end)
        else:
            merged.append((start, end))
    return merged, duplicated


def compute_trace(
    source: str,
    chunks: Sequence[Any],
    *,
    offset_basis: str = BASIS_CHAR,
    token_counter: Callable[[str], int] | None = None,
    measure: Any | None = None,
    source_label: str = "chunking_strategy",
    max_gaps: int = 16,
) -> ChunkTrace:
    """Derive a :class:`ChunkTrace` from a source document and its chunks.

    Coverage is measured over **non-whitespace** units only. Whitespace between
    chunks is a legitimate casualty of most segmentation schemes (and of
    tokenizers that exclude it from offsets), whereas a dropped non-whitespace
    unit is always lost content.

    ``chunks`` may be any objects exposing ``text``/``start_pos``/``end_pos``;
    ``metadata`` is read when present to build the boundary-source histogram.
    """
    unit_count = len(source)
    significant = [i for i, ch in enumerate(source) if not ch.isspace()]

    spans = [
        (int(getattr(c, "start_pos", 0)), int(getattr(c, "end_pos", 0))) for c in chunks
    ]
    merged, duplicated = _merge_spans(spans, unit_count)

    covered_flags = bytearray(unit_count)
    for start, end in merged:
        covered_flags[start:end] = b"\x01" * (end - start)

    covered = sum(1 for i in significant if covered_flags[i])

    # One pass: count every dropped span, preview the first ``max_gaps`` of them.
    gaps: list[Gap] = []
    dropped = 0
    cursor = 0
    while cursor < unit_count:
        if covered_flags[cursor]:
            cursor += 1
            continue
        run_end = cursor
        while run_end < unit_count and not covered_flags[run_end]:
            run_end += 1
        fragment = source[cursor:run_end]
        if fragment.strip():
            dropped += 1
            if len(gaps) < max_gaps:
                gaps.append(Gap(cursor, run_end, fragment[:60]))
        cursor = run_end

    # Sizes are reported in the MEASURE's units, not characters. Hardcoding
    # len() while advertising these as the cap check means that under a token
    # measure the comparison a reader would naturally make is nonsense -- and
    # nothing fails, because a plausible number is still a number. Measuring
    # the span in the source is also the more faithful question than measuring
    # the rebuilt text.
    if measure is None:
        sizes = [len(getattr(c, "text", "")) for c in chunks]
        size_unit = BASIS_CHAR
    else:
        sizes = [
            measure.size(
                source, int(getattr(c, "start_pos", 0)), int(getattr(c, "end_pos", 0))
            )
            for c in chunks
        ]
        size_unit = str(getattr(measure, "name", "custom"))
    histogram: dict[str, int] = {}
    for chunk in chunks:
        metadata = getattr(chunk, "metadata", None)
        if isinstance(metadata, dict):
            key = str(metadata.get(source_label, "unknown"))
            histogram[key] = histogram.get(key, 0) + 1

    tokens_out = None
    if token_counter is not None:
        tokens_out = sum(token_counter(getattr(c, "text", "")) for c in chunks)

    return ChunkTrace(
        units_in=len(significant),
        units_covered=covered,
        units_duplicated=duplicated,
        chunk_count=len(chunks),
        spans_dropped=dropped,
        min_chunk_units=min(sizes) if sizes else 0,
        max_chunk_units=max(sizes) if sizes else 0,
        offset_basis=offset_basis,
        size_unit=size_unit,
        tokens_out=tokens_out,
        boundary_sources=histogram,
        gaps=tuple(gaps),
    )
