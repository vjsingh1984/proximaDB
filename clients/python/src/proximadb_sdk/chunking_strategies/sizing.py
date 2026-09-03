"""Sizing — how a budget is *expressed*, resolved once into absolute units.

The third of the three decisions (ADR-091): grid, measure, **budget**. This is
the budget, and its whole job is to be declarative at the edge and absolute
everywhere else.

Why resolution belongs at the wrapper
-------------------------------------
A fraction is a function of the *resolved window*, so it cannot be evaluated
until the window is known, and it must be evaluated in a strict order (window ->
cap -> floor -> overlap). Doing that lazily, at each of the ~30 places a size is
read, means re-deriving it ~30 times and inviting two of them to disagree.

So it happens exactly once, at construction, and every inner method keeps taking
plain absolute integers. That is deliberately the *smallest* change that adds a
declarative dialect: nothing downstream of ``ChunkingConfig`` learns that
fractions exist.

One quantity had four dialects
------------------------------
"How big is a chunk" is already expressed as wire ``size_tokens`` +
``overlap_pct``, SDK ``chunk_size`` + ``chunk_overlap`` characters,
``TokenBudget.target_tokens``, and anvaiops tokens-then-``* 4`` -- while the code
that *consumes* overlap is one subtraction. A declarative front door with an
absolute core is what collapses that: dialects are added here, not in the
algorithms.

A fraction names its referent
-----------------------------
``Fraction(0.10)`` is ambiguous on its own -- ten percent of *what*. It matters
because there is already a divergent reading in this system:
``proximadb-embedding/src/chunker.rs`` treats the wire ``overlap_pct`` as a
fraction of a *character* window approximated as ``size_tokens * 4``, not of the
resolved window in the resolved measure. Rather than pick one silently,
:class:`Of` names the referent, and a second member can be added if the wire
semantics ever need reproducing exactly. Reconciling the two is a TD, not a
default -- matching an approximation by accident is precisely the failure this
module exists to prevent.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Any

from .measures import CHAR_MEASURE


class Of(Enum):
    """What a :class:`Fraction` is a fraction *of*."""

    #: The resolved window, in the resolved measure's units. The only reading
    #: that stays correct when the measure changes, and therefore the default.
    WINDOW = "window"


@dataclass(frozen=True)
class Absolute:
    """A literal count, in the measure's units."""

    value: int

    def resolve(self, window: int) -> int:  # noqa: ARG002 - window unused by design
        return self.value


@dataclass(frozen=True)
class Fraction:
    """A proportion of :attr:`of`, floored to a whole unit."""

    value: float
    of: Of = Of.WINDOW

    def __post_init__(self) -> None:
        if not 0.0 <= self.value < 1.0:
            raise ValueError(
                f"fraction must be in [0.0, 1.0), got {self.value}. A fraction "
                "of 1.0 or more would make a step of zero and never terminate."
            )

    def resolve(self, window: int) -> int:
        return int(window * self.value)


#: Either dialect. Typed loosely so ``base`` stays dependency-light.
Size = Any


@dataclass(frozen=True)
class ResolvedSizing:
    """A budget reduced to absolute units, with the invariants enforced once.

    ``__post_init__`` is the single home for the sizing invariants, which were
    otherwise spread across ``ChunkingConfig.__post_init__``, ``validate_config``
    and a scattering of ``max(1, ...)`` at call sites -- three places that could
    each clamp differently.
    """

    window: int
    overlap: int
    minimum: int
    maximum: int
    measure: Any = CHAR_MEASURE

    def __post_init__(self) -> None:
        if self.window <= 0:
            raise ValueError(f"window must be positive, got {self.window}")
        if self.overlap < 0:
            raise ValueError(f"overlap cannot be negative, got {self.overlap}")
        if self.overlap >= self.window:
            raise ValueError(
                f"overlap {self.overlap} must be less than window {self.window}; "
                "an overlap at or above the window makes no forward progress"
            )
        if self.minimum < 0:
            raise ValueError(f"minimum cannot be negative, got {self.minimum}")
        if self.minimum > self.window:
            raise ValueError(
                f"minimum {self.minimum} exceeds window {self.window}: every "
                "full chunk would be below the floor"
            )
        if self.maximum < self.window:
            raise ValueError(f"maximum {self.maximum} is below window {self.window}")

    @property
    def step(self) -> int:
        """Units of new content per chunk. Never zero -- that is non-termination."""
        return max(1, self.window - self.overlap)


@dataclass(frozen=True)
class SizingPolicy:
    """The declarative front door: window plus three derived bounds.

    ``window`` must be :class:`Absolute` -- it is the referent everything else is
    resolved against, so a fraction of it would be circular.
    """

    window: Size
    overlap: Size = Absolute(0)
    minimum: Size = Absolute(0)
    maximum: Size | None = None
    measure: Any = CHAR_MEASURE

    def resolve(self) -> ResolvedSizing:
        """Reduce to absolutes, in dependency order: window, cap, floor, overlap."""
        if not isinstance(self.window, Absolute):
            raise TypeError(
                "window must be Absolute: it is the referent a Fraction resolves "
                "against, so expressing it as a fraction would be circular"
            )
        window = self.window.resolve(0)
        maximum = self.maximum.resolve(window) if self.maximum is not None else window
        return ResolvedSizing(
            window=window,
            overlap=self.overlap.resolve(window),
            minimum=self.minimum.resolve(window),
            maximum=maximum,
            measure=self.measure,
        )
