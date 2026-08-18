"""Drive the invariant suite over any chunker (TD-CHUNK-1).

The checks in :mod:`.invariants` are deliberately structural, so the only
ProximaDB-specific knowledge needed to run them is "how do I chunk a string with
this implementation". :class:`ChunkerAdapter` captures exactly that and nothing
else, which is what lets victor-rag and the anvaiops connector SDK run the same
bed against their own code (ADR-091 D4).

A consumer supplies the callables it can and omits the rest; omitted capabilities
are *skipped*, never silently reported as passing — an unmeasured invariant is
recorded in :attr:`Evaluation.skipped` so a green result can never be mistaken for
full coverage.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass, field
from typing import Any

from .corpus import CorpusEntry
from .invariants import (
    Invariant,
    Violation,
    check_cap,
    check_config_safety,
    check_exactness,
    check_idempotence,
    check_no_containment,
    check_non_empty,
    check_stream_equivalence,
    check_totality,
)
from .trace import BASIS_CHAR, ChunkTrace, compute_trace


@dataclass
class ChunkerAdapter:
    """Everything the suite needs to know about one chunker configuration."""

    name: str
    chunk: Callable[[str], Sequence[Any]]
    budget: int
    basis: str = BASIS_CHAR
    #: Streaming entry point; ``None`` means the implementation does not claim to
    #: stream, so STREAM_EQUIVALENCE is skipped rather than failed.
    chunk_stream: Callable[[Iterator[str]], Sequence[Any]] | None = None
    #: Re-chunk a single chunk's text at the same budget (IDEMPOTENCE).
    rechunk: Callable[[str], Sequence[Any]] | None = None
    #: Build and run a deliberately hostile-but-legal config (CONFIG_SAFETY).
    hostile: Callable[[], Sequence[Any]] | None = None
    #: Optional token counter; when absent CAP is measured in characters.
    token_counter: Callable[[str], int] | None = None
    #: The measure this chunker sizes itself in. Supplied so the trace reports
    #: sizes in the chunker's own units rather than silently in characters.
    measure: Any | None = None


@dataclass
class Evaluation:
    """Result of running the suite for one (chunker, corpus entry) pair."""

    chunker: str
    corpus: str
    violations: tuple[Violation, ...] = ()
    skipped: tuple[Invariant, ...] = ()
    trace: ChunkTrace | None = None
    error: str | None = None
    notes: list[str] = field(default_factory=list)

    @property
    def violated(self) -> frozenset[Invariant]:
        """The comparable summary — this is what a baseline records."""
        return frozenset(v.invariant for v in self.violations)

    def render(self) -> str:
        head = f"{self.chunker} x {self.corpus}"
        if self.error:
            return f"{head}: ERROR {self.error}"
        body = "\n".join(f"    - {v}" for v in self.violations) or "    (clean)"
        skipped = (
            f"\n    skipped: {', '.join(i.value for i in self.skipped)}"
            if self.skipped
            else ""
        )
        trace = f"\n    trace: {self.trace.summary()}" if self.trace else ""
        return f"{head}\n{body}{skipped}{trace}"


def evaluate(adapter: ChunkerAdapter, entry: CorpusEntry) -> Evaluation:
    """Run every applicable invariant for one chunker over one corpus entry.

    A raise from the chunker itself is recorded as ``error`` rather than being
    allowed to abort the sweep: "this configuration throws on this input" is a
    finding, and one throwing case must not hide the other 69.
    """
    result = Evaluation(chunker=adapter.name, corpus=entry.name)
    try:
        chunks = list(adapter.chunk(entry.text))
    except Exception as exc:  # noqa: BLE001 - recorded, not swallowed
        result.error = f"{type(exc).__name__}: {exc}"
        return result

    result.trace = compute_trace(
        entry.text,
        chunks,
        offset_basis=adapter.basis,
        token_counter=adapter.token_counter,
        measure=adapter.measure,
    )

    violations: list[Violation] = []
    skipped: list[Invariant] = []

    for check in (
        check_non_empty(entry.text, chunks),
        check_totality(entry.text, chunks),
        check_exactness(entry.text, chunks, basis=adapter.basis),
        check_cap(
            chunks,
            budget=adapter.budget,
            token_counter=adapter.token_counter,
        ),
        check_no_containment(chunks),
    ):
        if check is not None:
            violations.append(check)

    if adapter.chunk_stream is None:
        skipped.append(Invariant.STREAM_EQUIVALENCE)
    else:
        outcome = check_stream_equivalence(
            entry.text, adapter.chunk, adapter.chunk_stream
        )
        if outcome is not None:
            violations.append(outcome)

    if adapter.rechunk is None:
        skipped.append(Invariant.IDEMPOTENCE)
    else:
        outcome = check_idempotence(chunks, adapter.rechunk)
        if outcome is not None:
            violations.append(outcome)

    if adapter.hostile is None:
        skipped.append(Invariant.CONFIG_SAFETY)
    else:
        outcome = check_config_safety(adapter.hostile)
        if outcome is not None:
            violations.append(outcome)

    result.violations = tuple(violations)
    result.skipped = tuple(skipped)
    return result


def evaluate_all(
    adapters: Sequence[ChunkerAdapter], entries: Sequence[CorpusEntry]
) -> list[Evaluation]:
    """Full sweep, adapters x corpus, in a stable order."""
    return [evaluate(a, e) for a in adapters for e in entries]


def format_baseline(evaluations: Sequence[Evaluation]) -> str:
    """Render a sweep as the literal baseline mapping, for review as data.

    Emitting the baseline rather than hand-writing it keeps the recorded state
    honest: it is whatever the code actually does today, not what someone
    believed it did.
    """
    lines = ["BASELINE: dict[tuple[str, str], frozenset[Invariant]] = {"]
    for ev in evaluations:
        if ev.error:
            lines.append(f'    ("{ev.chunker}", "{ev.corpus}"): ERROR,  # {ev.error}')
            continue
        if not ev.violated:
            continue
        members = ", ".join(
            f"Invariant.{i.name}" for i in sorted(ev.violated, key=lambda x: x.name)
        )
        lines.append(f'    ("{ev.chunker}", "{ev.corpus}"): frozenset({{{members}}}),')
    lines.append("}")
    return "\n".join(lines)
