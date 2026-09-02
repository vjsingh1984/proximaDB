from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "fuse_source_runs", ROOT / "scripts/bench/fuse_source_runs.py"
)
assert SPEC is not None and SPEC.loader is not None
FUSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FUSION)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _write_run(path: Path, rankings: dict[str, tuple[str, ...]]) -> None:
    rows = [
        {
            "query_id": query_id,
            "source_id": source_id,
            "rank": rank,
            "score": float(len(source_ids) - rank + 1),
        }
        for query_id, source_ids in rankings.items()
        for rank, source_id in enumerate(source_ids, 1)
    ]
    path.write_text(
        "".join(json.dumps(row, sort_keys=True) + "\n" for row in rows),
        encoding="utf-8",
    )


def _write_manifest(path: Path, run: Path, *, query_count: int, top_k: int) -> None:
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "candidate_granularity": "source",
                "query_count": query_count,
                "run_path": str(run.resolve()),
                "run_row_count": sum(1 for _ in run.open(encoding="utf-8")),
                "run_sha256": _sha256(run),
                "top_k": top_k,
            }
        )
        + "\n",
        encoding="utf-8",
    )


def test_fusion_validates_sealed_inputs_and_records_candidate_depth(tmp_path: Path):
    dense = tmp_path / "dense.jsonl"
    lexical = tmp_path / "lexical.jsonl"
    _write_run(dense, {"q1": ("a", "b", "c"), "q2": ("d", "e", "f")})
    _write_run(lexical, {"q1": ("b", "d", "a"), "q2": ("e", "g", "d")})
    dense_manifest = tmp_path / "dense.manifest.json"
    lexical_manifest = tmp_path / "lexical.manifest.json"
    _write_manifest(dense_manifest, dense, query_count=2, top_k=3)
    _write_manifest(lexical_manifest, lexical, query_count=2, top_k=3)
    output = tmp_path / "fused.jsonl"

    manifest = FUSION.fuse_source_runs(
        (dense, lexical),
        output,
        labels=("dense", "lexical"),
        top_k=4,
        manifest_paths=(dense_manifest, lexical_manifest),
        require_complete_top_k=True,
    )

    assert manifest["input_manifests_validated"] is True
    assert manifest["candidate_count"] == {
        "max": 4,
        "min": 4,
        "p50": 4,
        "p90": 4,
        "p99": 4,
    }
    assert manifest["complete_query_count"] == 2
    assert all(row["manifest_sha256"] for row in manifest["inputs"])


def test_fusion_rejects_a_manifest_for_different_run_bytes(tmp_path: Path):
    first = tmp_path / "first.jsonl"
    second = tmp_path / "second.jsonl"
    _write_run(first, {"q1": ("a", "b")})
    _write_run(second, {"q1": ("b", "c")})
    first_manifest = tmp_path / "first.manifest.json"
    second_manifest = tmp_path / "second.manifest.json"
    _write_manifest(first_manifest, first, query_count=1, top_k=2)
    _write_manifest(second_manifest, second, query_count=1, top_k=2)
    first.write_text(first.read_text(encoding="utf-8") + "\n", encoding="utf-8")

    with pytest.raises(ValueError, match="run_sha256 does not match"):
        FUSION.fuse_source_runs(
            (first, second),
            tmp_path / "fused.jsonl",
            labels=("first", "second"),
            top_k=2,
            manifest_paths=(first_manifest, second_manifest),
        )


def test_fusion_fails_before_writing_an_incomplete_required_depth(tmp_path: Path):
    first = tmp_path / "first.jsonl"
    second = tmp_path / "second.jsonl"
    _write_run(first, {"q1": ("a", "b")})
    _write_run(second, {"q1": ("b", "a")})
    output = tmp_path / "fused.jsonl"

    with pytest.raises(ValueError, match="fewer than required top_k=3"):
        FUSION.fuse_source_runs(
            (first, second),
            output,
            labels=("first", "second"),
            top_k=3,
            require_complete_top_k=True,
        )

    assert not output.exists()
