"""Chunking evals — the rubric for a quality that has no right answer.

The tests-vs-evals split (CLAUDE.md mandate 13): tests cover what is
deterministic, evals cover what is not. The invariant suite covers the
deterministic half of chunking — coverage, exactness, the size cap — and every
one of those has a right answer.

**Boundary quality does not.** Whether a cut lands in a good place is a matter of
degree, judged against what retrieval needs. So it gets a rubric with thresholds,
not an assertion.

The first principle: this must not become an eval of the embedding model
--------------------------------------------------------------------------
The tempting design is to embed chunks, run queries, and score recall@k. It is
also wrong for this harness. Recall@k moves when the *model* changes, when the
*index* changes, and when the *chunker* changes — so a chunking gate built on it
fires for reasons that have nothing to do with chunking, and the day it goes red
nobody can say which layer moved. A gate that cannot attribute its own failure
gets muted.

So every metric here is **model-independent** and measures only what the chunker
actually decides: given that the answer to a question occupies span ``[a, b)`` of
the source, what did the chunker do to it? That question has a defensible answer
without embedding anything, and it is the *precondition* for retrieval working at
all — if the answer is split across two chunks, no model and no index can return
it whole.

Retrieval-level recall@k against the f32 baseline still belongs in the ANN
harnesses, where the model is the thing under test. This measures the layer
underneath, which those harnesses assume and never check.

Output and trajectory
---------------------
The mandate asks for both, and they are genuinely different failures:

* **Output** — is the answer *findable*: contained whole in one chunk, and not
  buried in so much unrelated text that the chunk's embedding is about something
  else.
* **Trajectory** — was the route *sound*: did the chunker respect the document's
  own structure (never cut through a fenced code block or a table row), and does
  the chunk carry the context needed to attribute the answer to its section.

A chunker can score well on output by luck — a large enough chunk contains
everything — which is exactly why dilution and trajectory are scored too. Pass
the output metrics by making chunks huge and the dilution metric fails.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

from .corpus import by_name

# ---------------------------------------------------------------------------
# The oracle's own structure detection — deliberately INDEPENDENT of
# ``structure.py``, which the chunkers use.
#
# A rubric that shares an implementation with the thing it scores cannot detect
# that implementation being wrong: change the shared definition and the chunker
# stops avoiding fences at the same instant the metric stops looking for them.
# The failure is silent and total, and it is a property of the coupling rather
# than of any particular bug.
#
# Duplication is therefore the correct answer here and nowhere else in this
# package: an oracle has to be a second opinion, not the same opinion. (Stated
# as the design rule it is — the two definitions currently agree on every corpus
# entry, so this is prevention, not a repair.)
# ---------------------------------------------------------------------------

_ORACLE_FENCE = re.compile(r"```[\s\S]*?```|~~~[\s\S]*?~~~")
_ORACLE_TABLE = re.compile(r"^\|.*\|$(?:\n^\|.*\|$)*", re.MULTILINE)


def _oracle_protected(text: str) -> list[tuple[int, int]]:
    """Regions a chunk boundary must not fall inside, per the oracle."""
    found = [(m.start(), m.end()) for m in _ORACLE_FENCE.finditer(text)]
    found += [(m.start(), m.end()) for m in _ORACLE_TABLE.finditer(text)]
    return sorted(found)


@dataclass(frozen=True)
class EvalCase:
    """One question, expressed as the span that answers it.

    The question is recorded in prose for the reader, but the *machine-checkable*
    form is ``needle`` — the literal text a correct answer must contain. Located
    by search rather than stored as offsets, so editing the corpus cannot
    silently point a case at the wrong span.
    """

    name: str
    corpus: str
    question: str
    needle: str
    #: What the chunker should have respected to answer this well. Documents the
    #: intent of the case; not used for scoring.
    tests: str = ""

    def span(self) -> tuple[int, int]:
        text = by_name(self.corpus).text
        index = text.find(self.needle)
        if index < 0:
            raise ValueError(
                f"eval case {self.name!r}: needle not found in corpus entry "
                f"{self.corpus!r}. The corpus changed under the case; fix the "
                "case rather than loosening the needle."
            )
        return index, index + len(self.needle)


@dataclass
class CaseScore:
    """What one chunker did to one answer span."""

    case: str
    #: OUTPUT: the answer is wholly inside a single chunk.
    contained: bool
    #: OUTPUT: how many chunks the answer is spread across (1 is ideal).
    fragments: int
    #: OUTPUT: answer length / containing chunk length. 1.0 is a chunk that is
    #: exactly the answer; near 0 is an answer lost in unrelated text.
    density: float
    #: TRAJECTORY: the chunker did not cut through a protected region.
    structure_intact: bool
    #: TRAJECTORY: the containing chunk knows which section it came from.
    attributable: bool

    def as_row(self) -> str:
        return (
            f"{self.case:34s} contained={int(self.contained)} "
            f"fragments={self.fragments:2d} density={self.density:.3f} "
            f"structure={int(self.structure_intact)} attrib={int(self.attributable)}"
        )


@dataclass
class EvalReport:
    chunker: str
    scores: list[CaseScore] = field(default_factory=list)

    @property
    def containment(self) -> float:
        """Fraction of answers a retriever could return whole. The headline."""
        return self._mean(s.contained for s in self.scores)

    @property
    def mean_density(self) -> float:
        return self._mean(s.density for s in self.scores)

    @property
    def structural_integrity(self) -> float:
        return self._mean(s.structure_intact for s in self.scores)

    @property
    def attributability(self) -> float:
        return self._mean(s.attributable for s in self.scores)

    def _mean(self, values) -> float:
        collected = [float(v) for v in values]
        return sum(collected) / len(collected) if collected else 0.0

    def render(self) -> str:
        head = (
            f"{self.chunker}: containment={self.containment:.2f} "
            f"density={self.mean_density:.3f} "
            f"structure={self.structural_integrity:.2f} "
            f"attributable={self.attributability:.2f}"
        )
        return "\n".join([head] + [f"    {s.as_row()}" for s in self.scores])


def _covering(chunks: Sequence[Any], start: int, end: int) -> list[Any]:
    """Chunks overlapping ``[start, end)``, in order."""
    return [
        chunk
        for chunk in chunks
        if int(getattr(chunk, "end_pos", 0)) > start
        and int(getattr(chunk, "start_pos", 0)) < end
    ]


def _cuts_through_protected(text: str, chunks: Sequence[Any]) -> bool:
    """True if any chunk boundary falls strictly inside a protected region.

    Cutting a fenced code block leaves both halves syntactically broken, and
    cutting a table row leaves a fragment that reads as prose. Neither is
    recoverable downstream, which is what makes this a trajectory failure rather
    than a matter of taste.
    """
    barriers = _oracle_protected(text)
    if not barriers:
        return False
    for chunk in chunks:
        for edge in (
            int(getattr(chunk, "start_pos", 0)),
            int(getattr(chunk, "end_pos", 0)),
        ):
            for start, end in barriers:
                if start < edge < end:
                    return True
    return False


def score_case(case: EvalCase, chunks: Sequence[Any]) -> CaseScore:
    text = by_name(case.corpus).text
    start, end = case.span()
    touching = _covering(chunks, start, end)

    contained = any(
        int(getattr(c, "start_pos", 0)) <= start
        and int(getattr(c, "end_pos", 0)) >= end
        for c in touching
    )

    holder = next(
        (
            c
            for c in touching
            if int(getattr(c, "start_pos", 0)) <= start
            and int(getattr(c, "end_pos", 0)) >= end
        ),
        None,
    )
    if holder is None:
        density = 0.0
        attributable = False
    else:
        holder_length = max(
            1, int(getattr(holder, "end_pos", 0)) - int(getattr(holder, "start_pos", 0))
        )
        density = (end - start) / holder_length
        metadata = getattr(holder, "metadata", {}) or {}
        attributable = bool(
            metadata.get("heading_path") or metadata.get("header_title")
        )

    return CaseScore(
        case=case.name,
        contained=contained,
        fragments=len(touching),
        density=density,
        structure_intact=not _cuts_through_protected(text, chunks),
        attributable=attributable,
    )


def run_eval(
    name: str, chunk: Any, cases: Sequence[EvalCase] | None = None
) -> EvalReport:
    """Score one chunker (a ``text -> chunks`` callable) across the suite."""
    report = EvalReport(chunker=name)
    for case in cases or STANDARD_CASES:
        chunks = list(chunk(by_name(case.corpus).text))
        report.scores.append(score_case(case, chunks))
    return report


#: The suite. Each case names a real retrieval question and the literal span
#: that answers it. Kept small and legible on purpose: an eval nobody reads is
#: an eval nobody trusts, and every case here earns its place by testing a
#: distinct way chunking can make an answer unfindable.
STANDARD_CASES: tuple[EvalCase, ...] = (
    EvalCase(
        name="section_answer_is_whole",
        corpus="header_dense_markdown",
        question="What is the answer in Section 3?",
        needle="Vector answer number 3.",
        tests="A section's answer must not be split from itself.",
    ),
    EvalCase(
        name="section_answer_keeps_its_heading",
        corpus="header_dense_markdown",
        question="Which section does 'Query answer number 1' belong to?",
        needle="Query answer number 1.",
        tests="Attribution: the chunk should carry its heading path.",
    ),
    EvalCase(
        name="table_header_binds_to_its_rows",
        corpus="table_markdown",
        question="What is the recall for coll_0_1?",
        needle=(
            "| collection | vectors | recall | latency_ms |\n"
            "|------------|---------|--------|------------|\n"
            "| coll_0_0 | 1000 | 0.90 | 3 |"
        ),
        tests=(
            "A row separated from its header row is unreadable: 0.91 means "
            "nothing without the column that names it."
        ),
    ),
    EvalCase(
        name="code_block_stays_runnable",
        corpus="code_fenced_markdown",
        question="How do I call search in step 0?",
        needle=(
            "```python\ndef step_0(client):\n"
            "    result = client.search(vector=[0.1, 0.2], k=10)\n\n"
            "    return [hit.id for hit in result]"
        ),
        tests="A fence cut in half yields two syntactically broken halves.",
    ),
    EvalCase(
        name="html_paragraph_is_whole",
        corpus="html",
        question="What does the first body paragraph say?",
        needle="Paragraph 0 of body copy that must be retrievable.",
        tests="HTML must be chunked by its own structure, not by luck.",
    ),
    EvalCase(
        name="json_record_is_whole",
        corpus="json_doc",
        question="What is the body of rec-2?",
        needle=('{"id": "rec-2", "body": "Record 2 narrative text.", "score": 0.02}'),
        tests="A record split mid-object cannot be parsed by a consumer.",
    ),
    EvalCase(
        name="cjk_sentence_is_whole",
        corpus="cjk_emoji",
        question="What do deep learning models convert text into?",
        needle="深度学习模型将文本转换为向量表示。",
        tests="CJK has no spaces; a naive cut lands mid-sentence.",
    ),
    EvalCase(
        name="short_document_survives",
        corpus="sub_minimum",
        question="What is the product title?",
        needle="A 39-char product title goes right here",
        tests="A document below the minimum must not be dropped.",
    ),
)
