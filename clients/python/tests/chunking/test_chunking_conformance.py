"""Chunking conformance sweep — the TD-CHUNK-1 baseline, landed RED-as-recorded.

This is the specification ADR-091 says the ecosystem never had, made enforceable.
It deliberately does **not** fix anything: TD-CHUNK-1's acceptance is the suite
plus the recorded baseline, and TD-CHUNK-2 onward turns the recorded violations
green.

Why a recorded baseline instead of ``xfail``
--------------------------------------------
The baseline is a **bidirectional ratchet**. Each (strategy, corpus) case asserts
its violation set *exactly*:

* a new violation fails -> regressions are caught;
* a **fixed** violation also fails, until the baseline is updated -> nobody
  repairs a defect without recording that they did, and nobody can quietly let a
  fix rot back.

``xfail(strict=True)`` gives the second property but scatters the state across 60
markers. A single reviewable mapping is the state of truth, and it is generated
from a real sweep (``format_baseline``) rather than hand-asserted — so it records
what the code *does*, not what anyone believes it does. That distinction is the
whole lesson of the audit that produced ADR-091: the pre-existing suite had 426
passing tests and caught none of these defects.

Baseline recorded 2026-08-17 against develop @482075bc1:
**60 cases, 22 clean, 38 violating, 67 violations, 0 errors.**

Note on running locally
-----------------------
The repo's conftests rely on the *installed* package. If your editable install
points at a different worktree you will silently test that one, so run this with
``PYTHONPATH=clients/python/src`` to pin the tree under test.
"""

from __future__ import annotations

import pytest

from proximadb_sdk.chunking_strategies import (
    ChunkingConfig,
    ChunkingStrategy,
    ChunkingStrategyFactory,
)
from proximadb_sdk.chunking_strategies.conformance import (
    BASIS_BYTE,
    BASIS_CHAR,
    ChunkerAdapter,
    Invariant,
    by_name,
    check_config_safety,
    check_exactness,
    check_non_empty,
    compute_trace,
    evaluate,
    evaluate_all,
    format_baseline,
    standard_corpus,
)
from proximadb_sdk.chunking_strategies.conformance.invariants import (
    _wall_clock_budget,
)

BUDGET = 2048
DEFAULTS = {
    "chunk_size": 512,
    "chunk_overlap": 50,
    "min_chunk_size": 100,
    "max_chunk_size": BUDGET,
}

SWEPT_STRATEGIES = (
    ChunkingStrategy.SLIDING_WINDOW,
    ChunkingStrategy.FIXED_SIZE,
    ChunkingStrategy.SENTENCE,
    ChunkingStrategy.PARAGRAPH,
    ChunkingStrategy.SEMANTIC,
    ChunkingStrategy.RECURSIVE,
)
# SEMANTIC_EMBEDDING needs an injected provider and CODE needs tree-sitter; both
# are swept separately once TD-CG2 settles the code path's offset basis.


def _v(*names: str) -> frozenset[Invariant]:
    return frozenset(Invariant(n) for n in names)


#: Recorded state of today's code. Generated, not hand-written.
#: ``sliding_window`` is absent because it is clean on all ten entries — it was
#: the only strategy whose offsets survived the audit.
BASELINE: dict[tuple[str, str], frozenset[Invariant]] = {
    ("fixed_size", "prose"): _v("exactness"),
    ("fixed_size", "header_dense_markdown"): _v("exactness"),
    ("fixed_size", "cjk_emoji"): _v("exactness"),
    ("fixed_size", "whitespace_heavy"): _v("exactness"),
    ("fixed_size", "sub_minimum"): _v("non_empty", "totality"),
    ("fixed_size", "table_markdown"): _v("exactness"),
    ("fixed_size", "code_fenced_markdown"): _v("exactness"),
    ("fixed_size", "html"): _v("exactness"),
    ("fixed_size", "json_doc"): _v("exactness"),
    ("sentence", "prose"): _v("exactness", "totality"),
    ("sentence", "header_dense_markdown"): _v("exactness", "totality"),
    ("sentence", "cjk_emoji"): _v("cap", "stream_equivalence"),
    ("sentence", "boundary_free"): _v("cap"),
    ("sentence", "whitespace_heavy"): _v("cap", "exactness", "totality"),
    ("sentence", "table_markdown"): _v("exactness", "stream_equivalence", "totality"),
    ("sentence", "code_fenced_markdown"): _v(
        "exactness", "stream_equivalence", "totality"
    ),
    ("sentence", "html"): _v("stream_equivalence"),
    ("sentence", "json_doc"): _v("cap", "stream_equivalence"),
    ("paragraph", "prose"): _v("exactness", "totality"),
    ("paragraph", "header_dense_markdown"): _v("exactness", "totality"),
    ("paragraph", "cjk_emoji"): _v("cap"),
    ("paragraph", "boundary_free"): _v("cap"),
    ("paragraph", "table_markdown"): _v("exactness", "totality"),
    ("paragraph", "code_fenced_markdown"): _v("exactness", "totality"),
    ("paragraph", "json_doc"): _v("cap"),
    ("semantic", "prose"): _v("cap", "exactness", "no_containment", "totality"),
    # The defect that decided ADR-091: ZERO chunks on header-dense Markdown,
    # reached independently by victor-rag's unrelated implementation.
    ("semantic", "header_dense_markdown"): _v("non_empty", "totality"),
    ("semantic", "cjk_emoji"): _v("cap"),
    ("semantic", "boundary_free"): _v("cap"),
    ("semantic", "whitespace_heavy"): _v("exactness", "totality"),
    ("semantic", "sub_minimum"): _v("non_empty", "totality"),
    ("semantic", "table_markdown"): _v("exactness", "no_containment", "totality"),
    ("semantic", "code_fenced_markdown"): _v("exactness", "no_containment", "totality"),
    ("semantic", "json_doc"): _v("cap"),
    ("recursive", "prose"): _v("exactness", "totality"),
    ("recursive", "header_dense_markdown"): _v("exactness", "totality"),
    ("recursive", "table_markdown"): _v("exactness", "totality"),
    ("recursive", "code_fenced_markdown"): _v("exactness", "totality"),
}

#: Aggregate ratchet. Clean cases may only increase; violations may only fall.
CLEAN_FLOOR = 22
VIOLATION_CEILING = 67


def _adapter(strategy: ChunkingStrategy) -> ChunkerAdapter:
    config = ChunkingConfig(strategy=strategy, **DEFAULTS)
    strat = ChunkingStrategyFactory.create_strategy(strategy, config)

    stream = None
    if getattr(strat, "supports_streaming", False):

        def stream(pieces):  # type: ignore[misc]
            return list(strat.chunk_stream(pieces, source_id="doc"))

    def hostile():
        """A legal-but-hostile config: overlap close to size, tiny minimum."""
        hostile_config = ChunkingConfig(
            strategy=strategy,
            chunk_size=100,
            chunk_overlap=90,
            min_chunk_size=10,
            max_chunk_size=200,
        )
        hostile_strategy = ChunkingStrategyFactory.create_strategy(
            strategy, hostile_config
        )
        return hostile_strategy.chunk("word " * 4000, "doc")

    return ChunkerAdapter(
        name=strategy.value,
        chunk=lambda text: strat.chunk(text, "doc"),
        budget=BUDGET,
        chunk_stream=stream,
        rechunk=lambda text: strat.chunk(text, "doc"),
        hostile=hostile,
    )


CASES = [
    pytest.param(strategy, entry, id=f"{strategy.value}-{entry.name}")
    for strategy in SWEPT_STRATEGIES
    for entry in standard_corpus()
]


@pytest.mark.parametrize("strategy,entry", CASES)
def test_conformance_matches_recorded_baseline(strategy, entry):
    """Each case's violation set must equal the recorded baseline, exactly."""
    result = evaluate(_adapter(strategy), entry)
    assert (
        result.error is None
    ), f"{result.chunker} raised on {entry.name}: {result.error}"

    expected = BASELINE.get((strategy.value, entry.name), frozenset())
    actual = result.violated

    if actual != expected:
        fixed = sorted(i.value for i in expected - actual)
        broke = sorted(i.value for i in actual - expected)
        raise AssertionError(
            "conformance baseline drift for "
            f"{strategy.value} x {entry.name}\n"
            f"  newly VIOLATED (regression): {broke or 'none'}\n"
            f"  no longer violated (fixed):  {fixed or 'none'}\n"
            f"  -> if this is a fix, update BASELINE and lower "
            f"VIOLATION_CEILING / raise CLEAN_FLOOR in the same commit.\n\n"
            f"{result.render()}\n\n"
            f"corpus entry earned its place by catching: {entry.caught}"
        )


def test_aggregate_ratchet_does_not_regress():
    """The single number a reviewer can watch: violations may only fall."""
    results = [
        evaluate(_adapter(strategy), entry)
        for strategy in SWEPT_STRATEGIES
        for entry in standard_corpus()
    ]
    clean = sum(1 for r in results if not r.violated and r.error is None)
    violations = sum(len(r.violations) for r in results)
    errors = [r.render() for r in results if r.error]

    assert not errors, "chunking raised on: " + "; ".join(errors)
    assert (
        clean >= CLEAN_FLOOR
    ), f"clean cases fell to {clean}, floor is {CLEAN_FLOOR} — a regression"
    assert (
        violations <= VIOLATION_CEILING
    ), f"violations rose to {violations}, ceiling is {VIOLATION_CEILING}"


def test_sliding_window_is_clean_across_the_whole_corpus():
    """The one strategy that already satisfies the specification.

    Pinned separately so that "at least one implementation is correct" cannot
    silently stop being true — it is the reference the others are migrating to.
    """
    adapter = _adapter(ChunkingStrategy.SLIDING_WINDOW)
    failures = []
    for entry in standard_corpus():
        result = evaluate(adapter, entry)
        if result.violated or result.error:
            failures.append(result.render())
    assert not failures, "sliding_window regressed:\n" + "\n".join(failures)


class TestTrace:
    """The trace is new production code; it needs its own teeth."""

    def test_detects_total_coverage(self):
        source = "alpha beta gamma"

        class C:
            text = source
            start_pos = 0
            end_pos = len(source)
            metadata = {"chunking_strategy": "unit"}

        trace = compute_trace(source, [C()])
        assert trace.is_total
        assert trace.units_covered == trace.units_in
        assert trace.spans_dropped == 0
        assert trace.boundary_sources == {"unit": 1}

    def test_detects_a_dropped_span(self):
        source = "keep this DROPPED keep that"

        class C:
            text = "keep this"
            start_pos = 0
            end_pos = 9
            metadata: dict = {}

        trace = compute_trace(source, [C()])
        assert not trace.is_total
        assert trace.spans_dropped == 1
        assert trace.units_covered < trace.units_in
        assert "DROPPED" in trace.gaps[0].preview

    def test_counts_duplication_from_overlap(self):
        source = "abcdefghij"

        def chunk(start, end):
            obj = type("C", (), {})()
            obj.text = source[start:end]
            obj.start_pos = start
            obj.end_pos = end
            obj.metadata = {}
            return obj

        trace = compute_trace(source, [chunk(0, 6), chunk(4, 10)])
        assert trace.is_total
        assert trace.units_duplicated == 2
        assert trace.duplication_ratio == pytest.approx(0.2)

    def test_empty_input_is_total(self):
        trace = compute_trace("", [])
        assert trace.is_total
        assert trace.coverage_ratio == 1.0


class TestRunner:
    """The runner is the surface a foreign consumer drives, so it is tested as one.

    victor-rag and the anvaiops connector SDK are meant to run this bed against
    their own chunkers (ADR-091 D4); these tests pin the contract they will code
    against, using a deliberately trivial chunker rather than a ProximaDB one.
    """

    @staticmethod
    def _whole_document_chunker(text):
        obj = type("C", (), {})()
        obj.text = text
        obj.start_pos = 0
        obj.end_pos = len(text)
        obj.metadata = {"chunking_strategy": "whole"}
        return [obj]

    def _adapter(self, **overrides):
        base = {
            "name": "whole",
            "chunk": self._whole_document_chunker,
            "budget": 10**9,
        }
        base.update(overrides)
        return ChunkerAdapter(**base)

    def test_a_trivially_correct_chunker_is_clean(self):
        entry = by_name("prose")
        result = evaluate(self._adapter(), entry)
        assert result.error is None
        assert result.violated == frozenset()
        assert result.trace is not None and result.trace.is_total

    def test_unmeasured_invariants_are_reported_as_skipped_not_passed(self):
        """A green result must never be mistaken for full coverage."""
        result = evaluate(self._adapter(), by_name("prose"))
        assert set(result.skipped) == {
            Invariant.STREAM_EQUIVALENCE,
            Invariant.IDEMPOTENCE,
            Invariant.CONFIG_SAFETY,
        }
        assert "skipped" in result.render()

    def test_a_raising_chunker_is_recorded_not_propagated(self):
        """One throwing case must not hide the other 69."""

        def boom(_text):
            raise ValueError("synthetic failure")

        result = evaluate(self._adapter(chunk=boom), by_name("prose"))
        assert result.error is not None
        assert "ValueError: synthetic failure" in result.error
        assert "ERROR" in result.render()

    def test_budget_violation_is_detected(self):
        result = evaluate(self._adapter(budget=10), by_name("prose"))
        assert Invariant.CAP in result.violated

    def test_evaluate_all_sweeps_in_stable_order(self):
        entries = standard_corpus()[:3]
        results = evaluate_all([self._adapter()], entries)
        assert [r.corpus for r in results] == [e.name for e in entries]

    def test_format_baseline_omits_clean_cases(self):
        """The emitted baseline records only what actually violates."""
        rendered = format_baseline(evaluate_all([self._adapter()], standard_corpus()))
        assert rendered.startswith("BASELINE")
        assert "prose" not in rendered  # clean under a whole-document chunker

    def test_format_baseline_records_violations(self):
        rendered = format_baseline(
            evaluate_all([self._adapter(budget=10)], standard_corpus()[:1])
        )
        assert "Invariant.CAP" in rendered


class TestInvariantMechanics:
    """The checks themselves need teeth — especially the hang detector."""

    def test_config_safety_actually_catches_a_hang(self):
        """CONFIG_SAFETY is worthless if it cannot fire.

        victor-rag's ``_chunk_text`` infinite-loops on a legal config; this proves
        the detector would catch that rather than hanging CI forever.
        """
        if not _wall_clock_budget.supported:
            pytest.skip("SIGALRM unavailable on this platform")

        def spin():
            while True:
                pass

        violation = check_config_safety(spin, seconds=1)
        assert violation is not None
        assert violation.invariant is Invariant.CONFIG_SAFETY
        assert "did not terminate" in violation.detail

    def test_config_safety_tolerates_a_raise(self):
        """Raising on a hostile config is acceptable; hanging is not."""

        def boom():
            raise ValueError("rejected")

        assert check_config_safety(boom, seconds=1) is None

    def test_exactness_understands_a_byte_basis(self):
        """Byte offsets on non-ASCII must validate under BASIS_BYTE, not char."""
        source = "café résumé"
        encoded = source.encode("utf-8")
        chunk = type("C", (), {})()
        chunk.text = "café"
        chunk.start_pos = 0
        chunk.end_pos = len("café".encode())

        assert check_exactness(source, [chunk], basis=BASIS_BYTE) is None
        # The same span read as characters is wrong — which is the whole reason
        # ADR-091 axiom 2 demands the basis be declared.
        assert check_exactness(source, [chunk], basis=BASIS_CHAR) is not None
        assert len(encoded) != len(source)

    def test_non_empty_ignores_whitespace_only_input(self):
        assert check_non_empty("   \n\t ", []) is None
        assert check_non_empty("content", []) is not None


class TestCorpus:
    def test_is_deterministic(self):
        """Two builds must be byte-identical (repo determinism mandate)."""
        first = {e.name: e.text for e in standard_corpus()}
        second = {e.name: e.text for e in standard_corpus()}
        assert first == second

    def test_every_entry_records_the_defect_it_caught(self):
        """Admission rule: no decorative corpus entries."""
        for entry in standard_corpus():
            assert entry.text, f"{entry.name} is empty"
            assert (
                len(entry.caught) > 30
            ), f"{entry.name} must record which defect earned its place"

    def test_names_are_unique(self):
        names = [e.name for e in standard_corpus()]
        assert len(names) == len(set(names))
