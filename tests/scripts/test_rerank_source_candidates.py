from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "rerank_source_candidates", ROOT / "scripts/bench/rerank_source_candidates.py"
)
assert SPEC is not None and SPEC.loader is not None
RERANK = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RERANK
SPEC.loader.exec_module(RERANK)


class WhitespaceTokenizer:
    def encode(self, text: str, *, add_special_tokens: bool) -> list[int]:
        del add_special_tokens
        return list(range(len(text.split())))

    def decode(self, token_ids, *, skip_special_tokens: bool) -> str:
        del skip_special_tokens
        return " ".join(f"w{token_id}" for token_id in token_ids)

    def num_special_tokens_to_add(self, *, pair: bool) -> int:
        assert pair
        return 3


class LengthScorer:
    tokenizer = WhitespaceTokenizer()

    def predict(self, pairs, *, batch_size: int):
        assert batch_size > 0
        return [float(len(document.split())) for _, document in pairs]


def test_pair_windows_cover_long_body_without_truncation_and_propagate_title():
    windows = RERANK.build_pair_windows(
        WhitespaceTokenizer(),
        query="two query",
        title="useful title",
        body=" ".join(f"source{i}" for i in range(12)),
        max_length=10,
        overlap_tokens=1,
    )

    assert len(windows) == 6
    assert windows[0].body_token_start == 0
    assert windows[-1].body_token_end == 12
    assert all(window.text.startswith("useful title\n\n") for window in windows)
    assert all(window.input_tokens <= 10 for window in windows)
    assert all(
        right.body_token_start == left.body_token_end - 1
        for left, right in zip(windows, windows[1:])
    )


def test_pair_windows_reject_title_that_would_silently_displace_body():
    with pytest.raises(ValueError, match="title context requires"):
        RERANK.build_pair_windows(
            WhitespaceTokenizer(),
            query="query",
            title="one two three four five",
            body="body",
            max_length=9,
            overlap_tokens=0,
        )


def test_score_query_max_reduces_windows_and_records_token_economics():
    result = RERANK.score_query(
        LengthScorer(),
        query_id="q1",
        query="short query",
        candidates=[{"source_id": "d1", "baseline_rank": 1, "baseline_score": 4.0}],
        documents={
            "d1": {
                "title": "title",
                "body": " ".join(f"body{i}" for i in range(10)),
            }
        },
        max_length=9,
        overlap_tokens=1,
        batch_size=2,
        contract_fingerprint="test-contract",
    )

    assert result["zero_truncation_asserted"] is True
    assert result["contract_fingerprint"] == "test-contract"
    assert result["window_count"] > 1
    candidate = result["candidates"][0]
    assert candidate["rerank_score"] > 0
    assert candidate["input_token_count"] >= candidate["max_input_tokens"]
    assert candidate["winning_window"]["input_tokens"] <= 9


def test_materialize_reranks_only_prefix_and_preserves_full_candidate_set():
    baseline = {
        "q1": [
            {"source_id": "a", "baseline_rank": 1, "baseline_score": 30.0},
            {"source_id": "b", "baseline_rank": 2, "baseline_score": 20.0},
            {"source_id": "c", "baseline_rank": 3, "baseline_score": 10.0},
            {"source_id": "d", "baseline_rank": 4, "baseline_score": 5.0},
        ]
    }
    cached = {
        "q1": {
            "candidates": [
                {
                    "source_id": "a",
                    "baseline_rank": 1,
                    "baseline_score": 30.0,
                    "rerank_score": -3.0,
                },
                {
                    "source_id": "b",
                    "baseline_rank": 2,
                    "baseline_score": 20.0,
                    "rerank_score": 4.0,
                },
                {
                    "source_id": "c",
                    "baseline_rank": 3,
                    "baseline_score": 10.0,
                    "rerank_score": 2.0,
                },
            ]
        }
    }

    output = RERANK.materialize_run(baseline, cached, rerank_count=3)

    assert [row["source_id"] for row in output] == ["b", "c", "a", "d"]
    assert [row["rank"] for row in output] == [1, 2, 3, 4]
    assert [row["score"] for row in output] == [4.0, 3.0, 2.0, 1.0]


def test_materialize_rejects_a_cache_from_a_different_candidate_prefix():
    baseline = {"q1": [{"source_id": "a", "baseline_rank": 1, "baseline_score": 1.0}]}
    cached = {
        "q1": {
            "candidates": [
                {
                    "source_id": "other",
                    "baseline_rank": 1,
                    "baseline_score": 1.0,
                    "rerank_score": 2.0,
                }
            ]
        }
    }

    with pytest.raises(ValueError, match="does not match baseline prefix"):
        RERANK.materialize_run(baseline, cached, rerank_count=1)
