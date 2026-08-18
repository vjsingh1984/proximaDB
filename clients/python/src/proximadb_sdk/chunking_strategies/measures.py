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
