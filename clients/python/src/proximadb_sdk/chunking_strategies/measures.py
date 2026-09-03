"""Measures — what "size 512" actually counts.

One of the three decisions a chunker makes (ADR-091 and the decoupling plan):

1. the **grid** — where may a cut land, and where is it forbidden?
2. the **measure** — how big is a candidate span?          <- this module
3. the **budget** — window, overlap, floor, cap, in the measure's unit.

They are orthogonal, and the useful pairings cross: measure in *tokens*, cut at
*sentence* boundaries is the correct default and is what ``TokenBudgetStrategy``
already does. Fusing measure and grid into one "unit" enum would make that
inexpressible — the same category error as ``SLIDING_WINDOW`` (a segmenter
config) sitting as a peer of ``SENTENCE`` (a boundary source).

This widens an existing protocol rather than adding one
-------------------------------------------------------
``contracts.TokenCounter`` is already a measure in all but name: it has a stable
identity, a scalar ``count`` of a string, and an optional decomposition into
monotone character spans (``content_offsets``), whose ``| None`` return already
encodes "I can count, but I cannot give you a grid". ``TokenMeasure`` adapts it;
``TokenCounter`` keeps working unchanged.

Additivity is the crux
----------------------
``TokenCounter.count()`` measures **rendered** text (role prefix + model special
tokens) while ``content_offsets()`` excludes them, so
``count(text) != len(content_offsets(text))`` *by design*. A measure is therefore
not necessarily additive over its own units, and that is precisely why
``TokenBudgetStrategy`` binary-searches for a fitting end instead of computing
``start + target``.

Rather than assume additivity, measures **declare** it:

* **additive** (characters): ``size`` and ``advance`` are exact arithmetic.
* **non-additive** (rendered tokens): ``advance`` yields a *candidate* that an
  authoritative fit check must confirm, so the search costs O(log budget).

One algorithm either way; additivity only decides whether the verify loop runs
once or ``log n`` times.
"""

from __future__ import annotations

import bisect
from collections.abc import Callable, Sequence
from typing import Any, Protocol, runtime_checkable

from .spans import Span

#: Resolves a span to its text.
Slicer = Callable[[int, int], str]

#: A source is the document text, or a slicer for it. The grouping loops are
#: shared between the batch and streaming paths, and streaming holds only a
#: bounded buffer — never the whole document — so a measure must accept either.
Source = str | Slicer


def materialise(source: Any, start: int, end: int) -> str:
    """Get the text of ``[start, end)`` from a str or a slicer.

    Only non-character measures pay this: the character measure never calls it,
    which is what keeps the default path allocation-free.
    """
    if isinstance(source, str):
        return source[start:end]
    return source(start, end)


@runtime_checkable
class Measure(Protocol):
    """How a span is counted."""

    #: Stable identity — folded into the chunker-pool key, so two measures can
    #: never share a pooled chunker.
    name: str

    #: True when the size of a span equals the sum of its units' sizes, so
    #: ``size`` and ``advance`` are exact and no fit search is needed.
    is_additive: bool

    #: True when sizing requires the whole document, not just the span. A
    #: tokenizer's output depends on preceding context, so a token measure
    #: cannot be computed from a streaming buffer -- and measuring the buffer
    #: as if it were the document yields a plausible, wrong number. Streaming
    #: paths refuse such a measure up front rather than degrade.
    needs_document: bool

    def size(self, source: Any, start: int, end: int) -> int:
        """Units in ``[start, end)``."""
        ...

    def advance(self, source: Any, start: int, units: int) -> int:
        """Character offset ``units`` units after ``start``.

        For an additive measure this is exact. For a non-additive one it is a
        candidate the caller must verify against the real budget.
        """
        ...

    def unit_spans(self, text: str) -> Sequence[Span] | None:
        """The measure's grid, or ``None`` when it imposes none.

        ``None`` means "any character position is a legal cut" (the character
        measure) — it does **not** mean "cannot measure". A measure that can
        count but genuinely cannot decompose must say so by raising from
        :meth:`advance`, so the failure is loud rather than a silent mis-cut.
        """
        ...


class CharMeasure:
    """The character measure: span extent IS the size.

    The default, and deliberately the cheap path — no prefix array, no
    materialisation, no allocation. Every strategy behaved exactly this way
    before measures existed, so injecting this explicitly must reproduce the
    default byte-for-byte; the conformance suite asserts that equivalence.
    """

    name = "char"
    is_additive = True
    needs_document = False

    __slots__ = ()

    def size(self, source: Any, start: int, end: int) -> int:  # noqa: ARG002
        return end - start

    def advance(self, source: Any, start: int, units: int) -> int:  # noqa: ARG002
        return start + units

    def unit_spans(self, text: str) -> Sequence[Span] | None:  # noqa: ARG002
        # No grid: a character measure permits a cut anywhere.
        return None

    def __repr__(self) -> str:
        return "CharMeasure()"


#: The default. Module-level and immutable (``__slots__``, no state), so every
#: strategy can share one instance.
CHAR_MEASURE = CharMeasure()


def resolve_document(source: Any) -> str:
    """Recover the whole document behind ``source``, or fail with a reason.

    A whole-document measure cannot work from a window: the same characters
    tokenize differently depending on what precedes them. So it needs the
    document, and the two things it can be handed are a ``str`` (which is the
    document) and a slicer (which may or may not be able to produce it).

    Raising here rather than degrading is the point. Measuring a buffer fragment
    as if it were the document produces token counts that are *plausible* and
    *wrong* — chunks that overflow the model budget with no signal, which is the
    exact failure this whole line of work exists to remove.
    """
    if isinstance(source, str):
        return source
    document = getattr(source, "document", None)
    if isinstance(document, str):
        return document
    raise TypeError(
        "this measure needs the whole document, but was given a windowed "
        f"source ({type(source).__name__}). Streaming holds only a bounded "
        "buffer, so a token measure cannot be used on the streaming path; "
        "chunk the text in one call, or use the character measure."
    )


class TokenMeasure:
    """Counts a span in a tokenizer's **content** tokens.

    Adapts the existing :class:`~.contracts.TokenCounter` — its
    ``content_offsets`` already returns exactly the monotone character grid a
    measure needs. Nothing about ``TokenCounter`` changes; this is the adapter
    that lets any boundary strategy size itself in tokens, which previously
    required a whole separate strategy (``TokenBudgetStrategy``).

    Content tokens, not rendered tokens
    -----------------------------------
    This measures the *source* token grid, excluding role prefixes and special
    tokens, and it is therefore **additive over its own grid**: for offsets
    ``a <= b <= c`` that all land on token boundaries,
    ``size(a, c) == size(a, b) + size(b, c)``. (Off-grid offsets are not a
    counter-example but a definition: a token straddling ``b`` is fully inside
    neither half, so it is counted in neither.)

    The genuinely non-additive quantity is the **rendered** budget --
    ``counter.count()`` includes special tokens that no span owns -- and that
    stays where it already is, in ``TokenBudgetStrategy``, which binary-searches
    against an authoritative fit check. ``is_additive = False`` is reserved for
    measures of that shape; the caller-side backstop that makes such a measure
    safe is ``ChunkingStrategyInterface._fit_end``.

    Caching
    -------
    Tokenizing is O(document) and every size comparison would otherwise repeat
    it, so one document's grid is cached. The cache holds a single entry and is
    guarded by identity against the text it was built from, so a miss costs a
    recompute and can never return another document's grid.
    """

    is_additive = True
    needs_document = True

    __slots__ = ("_counter", "_cache", "name")

    def __init__(self, counter: Any, *, name: str | None = None) -> None:
        self._counter = counter
        self._cache: tuple[str, tuple[Span, ...], tuple[int, ...], tuple[int, ...]] | (
            None
        ) = None
        self.name = name or f"token:{getattr(counter, 'name', 'unknown')}"

    @property
    def counter(self) -> Any:
        return self._counter

    def _grid(self, document: str) -> tuple[tuple[Span, ...], tuple[int, ...], ...]:
        """``(spans, starts, ends)`` for ``document``, tokenizing at most once."""
        cached = self._cache
        # Identity, not equality: a hit must be free, and a miss is only ever a
        # recompute. Read into a local first so a concurrent replacement cannot
        # tear the check away from the use.
        if cached is not None and cached[0] is document:
            return cached[1], cached[2], cached[3]

        raw = self._counter.content_offsets(document)
        if raw is None:
            raise ValueError(
                f"token counter {getattr(self._counter, 'name', '?')} cannot "
                "provide source offsets, so it can count but cannot say where "
                "its units begin and end. A measure without a grid cannot be "
                "used to cut text; use it as a validator instead."
            )
        spans = tuple((int(a), int(b)) for a, b in raw)

        # Monotonicity is assumed by every bisect below and is never checked by
        # the tokenizers themselves. An out-of-order grid does not raise inside
        # bisect -- it silently returns a wrong index, so chunks come out
        # mis-cut with no error anywhere. Check it once, here, per document.
        previous_end = -1
        for index, (start, end) in enumerate(spans):
            if start > end or start < previous_end:
                raise ValueError(
                    f"token counter {getattr(self._counter, 'name', '?')} "
                    f"returned a non-monotone offset at index {index}: "
                    f"({start}, {end}) after end {previous_end}. Offsets must "
                    "be ordered and non-overlapping for a span to be countable."
                )
            previous_end = end

        starts = tuple(span[0] for span in spans)
        ends = tuple(span[1] for span in spans)
        self._cache = (document, spans, starts, ends)
        return spans, starts, ends

    def _first_index_at(self, starts: Sequence[int], start: int) -> int:
        """Index of the first token beginning at or after ``start``."""
        return bisect.bisect_left(starts, start)

    def size(self, source: Any, start: int, end: int) -> int:
        """Tokens lying wholly inside ``[start, end)``.

        Wholly-inside is the only definition that keeps the count additive and
        keeps a cut from being credited to two chunks at once. A token
        straddling a boundary belongs to neither side.
        """
        if end <= start:
            return 0
        document = resolve_document(source)
        _, starts, ends = self._grid(document)
        low = self._first_index_at(starts, start)
        high = bisect.bisect_right(ends, end)
        return max(0, high - low)

    def advance(self, source: Any, start: int, units: int) -> int:
        """Character offset of the end of the ``units``-th token after ``start``.

        Returns the document length when fewer than ``units`` tokens remain, so
        a windowing loop naturally terminates on the tail rather than stalling.
        """
        if units <= 0:
            return start
        document = resolve_document(source)
        _, starts, ends = self._grid(document)
        low = self._first_index_at(starts, start)
        target = low + units
        if target > len(ends):
            # Fewer than `units` tokens remain: run to the end of the document
            # so the tail (trailing punctuation, whitespace) stays covered.
            return len(document)
        return ends[target - 1]

    def unit_spans(self, text: str) -> Sequence[Span] | None:
        spans, _, _ = self._grid(text)
        return spans

    def __repr__(self) -> str:
        return f"TokenMeasure({self.name!r})"
