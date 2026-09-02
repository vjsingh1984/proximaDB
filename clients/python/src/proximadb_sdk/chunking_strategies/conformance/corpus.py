"""The shared chunking conformance corpus (TD-CHUNK-1 deliverable 2).

Part of the *contract*, not of any one implementation. ADR-091 D4: a port that is
not jointly tested is a suggestion, and every suite in the ecosystem was green
while broken because each chose inputs flattering to its own implementation. So
this corpus ships inside the package, importable by every consumer — ProximaDB,
victor-rag, anvaiops — and no implementation may substitute its own.

Admission rule: **an entry earns its place by having caught a real defect.**
Each :class:`CorpusEntry` records which one in ``caught``, so the corpus cannot
silently accumulate decorative cases.

Determinism
-----------
Every generator is seeded and pure; no wall-clock, no ambient randomness (repo
determinism mandate). Sizes are deliberately modest — this is a correctness bed
run on every PR, not a benchmark. ``SCALE_LARGE`` variants exist for the
superlinear-cost probes and are opt-in.
"""

from __future__ import annotations

import random
import string
from dataclasses import dataclass, field

_WORDS = (
    "retrieval",
    "vector",
    "database",
    "chunk",
    "semantic",
    "index",
    "graph",
    "tenant",
    "latency",
    "storage",
    "query",
    "embedding",
)


@dataclass(frozen=True)
class CorpusEntry:
    """One conformance document plus the defect that earned it a place."""

    name: str
    text: str
    caught: str
    tags: frozenset[str] = field(default_factory=frozenset)

    @property
    def size(self) -> int:
        return len(self.text)

    def __repr__(self) -> str:  # keeps pytest ids readable
        return f"<CorpusEntry {self.name} ({self.size} chars)>"


def _prose(target_chars: int, *, seed: int = 42) -> str:
    rng = random.Random(seed)
    out: list[str] = []
    total = 0
    while total < target_chars:
        length = rng.randint(5, 25)
        sentence = " ".join(rng.choice(_WORDS) for _ in range(length))
        piece = sentence.capitalize() + rng.choice([". ", "! ", "? "])
        out.append(piece)
        total += len(piece)
        if rng.random() < 0.12:
            out.append("\n\n")
            total += 2
    return "".join(out)


def _header_dense_markdown(sections: int = 40, *, seed: int = 7) -> str:
    rng = random.Random(seed)
    parts: list[str] = []
    for i in range(sections):
        parts.append(f"\n## Section {i}: {rng.choice(_WORDS).title()}\n\n")
        # Deliberately SHORT bodies: this is what makes the entry lethal.
        parts.append(f"{rng.choice(_WORDS).capitalize()} answer number {i}.\n")
    return "".join(parts)


def _cjk(reps: int = 220) -> str:
    base = "深度学习模型将文本转换为向量表示。这些向量捕获语义信息！检索系统使用余弦相似度匹配？"
    return (base + "性能很好 🚀🔥 结果准确 ✅。") * reps


def _boundary_free(size: int = 24_000, *, seed: int = 11) -> str:
    rng = random.Random(seed)
    return "".join(rng.choice(string.ascii_lowercase) for _ in range(size))


def _whitespace_heavy(reps: int = 120) -> str:
    return ("word  \t " * 40 + "\n\n\n\n") * reps


def _table_markdown(tables: int = 12) -> str:
    parts: list[str] = []
    for i in range(tables):
        parts.append(f"\n### Metrics table {i}\n\n")
        parts.append("| collection | vectors | recall | latency_ms |\n")
        parts.append("|------------|---------|--------|------------|\n")
        for row in range(6):
            parts.append(f"| coll_{i}_{row} | {1000 + row} | 0.9{row} | {row + 3} |\n")
        parts.append("\nThe table above reports steady-state numbers.\n")
    return "".join(parts)


def _code_fenced_markdown(blocks: int = 10) -> str:
    parts: list[str] = ["# Guide\n\nIntroduction paragraph explaining the setup.\n\n"]
    for i in range(blocks):
        parts.append(f"## Step {i}\n\nRun the snippet below, then verify output.\n\n")
        parts.append(
            "```python\n"
            f"def step_{i}(client):\n"
            "    result = client.search(vector=[0.1, 0.2], k=10)\n"
            "\n"
            "    return [hit.id for hit in result]\n"
            "```\n\n"
        )
        parts.append(f"Paragraph {i} after the fence, which must survive.\n\n")
    return "".join(parts)


def _html(sections: int = 10) -> str:
    parts = ["<html><body>\n"]
    for i in range(sections):
        parts.append(f'<h2 class="hdr">Heading {i}</h2>\n')
        parts.append(f"<p>Paragraph {i} of body copy that must be retrievable.</p>\n")
        parts.append("<table><tr><td>left</td><td>right</td></tr></table>\n")
    parts.append("</body></html>\n")
    return "".join(parts)


def _json_doc(records: int = 40) -> str:
    rows = ",\n".join(
        f'    {{"id": "rec-{i}", "body": "Record {i} narrative text.", '
        f'"score": 0.{i % 100:02d}}}'
        for i in range(records)
    )
    return '{\n  "records": [\n' + rows + "\n  ]\n}\n"


def standard_corpus() -> tuple[CorpusEntry, ...]:
    """The conformance bed every consumer runs on every PR."""
    return (
        CorpusEntry(
            name="prose",
            text=_prose(60_000),
            caught=(
                "SEMANTIC lost 29% of non-whitespace characters on paragraph-"
                "structured prose (un-headed bodies fall between markers); "
                "sentence.py's accumulator is quadratic"
            ),
            tags=frozenset({"text"}),
        ),
        CorpusEntry(
            name="header_dense_markdown",
            text=_header_dense_markdown(),
            caught=(
                "ZERO chunks — 100% silent loss — independently in BOTH the "
                "ProximaDB SDK and victor-rag: a section below min_chunk_size is "
                "dropped rather than merged. The defect that proves this is a "
                "missing specification, not a coding error"
            ),
            tags=frozenset({"markdown", "structure"}),
        ),
        CorpusEntry(
            name="cjk_emoji",
            text=_cjk(),
            caught=(
                "sentence regex requires an ASCII capital after the terminator, "
                "so the 。！？ endings shipped in the default config can never "
                "fire; 41 KB of Chinese returned as one chunk"
            ),
            tags=frozenset({"text", "unicode"}),
        ),
        CorpusEntry(
            name="boundary_free",
            text=_boundary_free(),
            caught=(
                "max_chunk_size never enforced — a 100 KB boundary-free input "
                "came back as one 100,000-char chunk, a 48x overrun"
            ),
            tags=frozenset({"text", "adversarial"}),
        ),
        CorpusEntry(
            name="whitespace_heavy",
            text=_whitespace_heavy(),
            caught=(
                "quadratic accumulator: 518 KB took 17.5 s (0.03 MB/s against a "
                "13.5 MB/s prose baseline). Shape of scraped HTML and PDF text"
            ),
            tags=frozenset({"text", "adversarial", "cost"}),
        ),
        CorpusEntry(
            name="sub_minimum",
            text="A 39-char product title goes right here",
            caught=(
                "documents below the default min_chunk_size=100 are silently "
                "discarded by fixed_size and semantic; victor-rag drops "
                "everything under 200 chars while still counting the document"
            ),
            tags=frozenset({"text", "edge"}),
        ),
        CorpusEntry(
            name="table_markdown",
            text=_table_markdown(),
            caught=(
                "preserve_tables is a config flag no strategy honours; no "
                "implementation repeats header rows across a split table, so "
                "wide tables lose their column names"
            ),
            tags=frozenset({"markdown", "structure"}),
        ),
        CorpusEntry(
            name="code_fenced_markdown",
            text=_code_fenced_markdown(),
            caught=(
                "_preserve_special_blocks mutates the string while iterating "
                "finditer matches taken from the pre-mutation string, splicing "
                "at stale offsets and destroying content outright"
            ),
            tags=frozenset({"markdown", "structure"}),
        ),
        CorpusEntry(
            name="html",
            text=_html(),
            caught=(
                "victor-rag's HTML offsets are a synthetic cursor over "
                "re-serialised text — 100% fabricated; ProximaDB's HTML header "
                "regex requires a bare tag so <h2 class=...> never matches"
            ),
            tags=frozenset({"markup", "structure"}),
        ),
        CorpusEntry(
            name="json_doc",
            text=_json_doc(),
            caught=(
                "victor-rag's JSON offsets are likewise fabricated; no "
                "record-oriented boundary source exists in the SDK at all"
            ),
            tags=frozenset({"markup", "structure"}),
        ),
    )


def scale_corpus(target_chars: int = 512_000) -> tuple[CorpusEntry, ...]:
    """Opt-in larger inputs for superlinear-cost probes.

    Kept out of :func:`standard_corpus` so the per-PR bed stays fast; the
    quadratic paths this exercises take tens of seconds by construction.
    """
    return (
        CorpusEntry(
            name="prose_large",
            text=_prose(target_chars),
            caught="quadratic accumulator in sentence.py at scale",
            tags=frozenset({"text", "cost", "slow"}),
        ),
        CorpusEntry(
            name="whitespace_heavy_large",
            text=_whitespace_heavy(reps=max(1, target_chars // 340)),
            caught="0.03 MB/s at 518 KB — 4.9x time for 2x input",
            tags=frozenset({"text", "cost", "slow", "adversarial"}),
        ),
    )


def by_name(name: str) -> CorpusEntry:
    """Look up one standard entry, for focused reproduction."""
    for entry in standard_corpus():
        if entry.name == name:
            return entry
    available = ", ".join(e.name for e in standard_corpus())
    raise KeyError(f"unknown corpus entry {name!r}; available: {available}")
