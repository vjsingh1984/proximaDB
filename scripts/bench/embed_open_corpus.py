#!/usr/bin/env python3
"""Resumably embed a token-budget corpus with a pinned open-model contract.

The corpus manifest is authoritative for text and input-contract identity. This
transport owns embedding shards and benchmark binary emission; it contains no
chunking logic.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import tempfile
import time
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import numpy as np
from proximadb_sdk.embedding_providers.providers.local.open_weights import (
    create_open_model_provider,
)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _atomic_json(value: Any, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=f"{path.name}.", suffix=".tmp", dir=path.parent)
    os.close(fd)
    temporary = Path(name)
    try:
        with temporary.open("w", encoding="utf-8") as handle:
            json.dump(value, handle, indent=2, sort_keys=True)
            handle.write("\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def _contract_for_model(
    corpus_manifest: dict[str, Any], model_id: str
) -> dict[str, Any]:
    contracts = corpus_manifest.get("input_contract", {}).get("contracts", [])
    matches = [item for item in contracts if item.get("model_id") == model_id]
    if len(matches) != 1:
        raise ValueError(
            f"corpus manifest must contain exactly one contract for {model_id}"
        )
    return matches[0]


def prepare_shards(
    *,
    output_dir: Path,
    corpus_sha256: str,
    model_id: str,
    revision: str,
    contract_fingerprint: str,
    dimension: int,
    shard_size: int,
    rows: int,
) -> list[Path]:
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "--", model_id).strip("-")
    shard_dir = (
        output_dir
        / "shards"
        / f"{corpus_sha256[:16]}-{slug}-{revision[:12]}-{dimension}d"
    )
    shard_dir.mkdir(parents=True, exist_ok=True)
    identity = {
        "schema_version": 1,
        "corpus_sha256": corpus_sha256,
        "model_id": model_id,
        "model_revision": revision,
        "contract_fingerprint": contract_fingerprint,
        "dimension": dimension,
        "normalize_embeddings": True,
        "shard_size": shard_size,
        "rows": rows,
    }
    identity_path = shard_dir / "manifest.json"
    if identity_path.exists():
        if json.loads(identity_path.read_text(encoding="utf-8")) != identity:
            raise RuntimeError(f"embedding shard identity mismatch in {shard_dir}")
    else:
        _atomic_json(identity, identity_path)
    return [shard_dir / f"{start:08d}.npy" for start in range(0, rows, shard_size)]


def pending_shards(
    paths: Sequence[Path], *, rows: int, dimension: int, shard_size: int
) -> list[tuple[int, Path]]:
    pending = []
    for start, path in zip(range(0, rows, shard_size), paths, strict=True):
        expected = (min(shard_size, rows - start), dimension)
        if path.exists():
            saved = np.load(path, mmap_mode="r")
            if saved.shape != expected or saved.dtype != np.float32:
                raise RuntimeError(
                    f"invalid cached shard {path}: {saved.shape} {saved.dtype}; expected {expected} float32"
                )
        else:
            pending.append((start, path))
    return pending


def _atomic_numpy(array: np.ndarray, path: Path) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    try:
        with temporary.open("wb") as handle:
            np.save(handle, array)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def finalize_embeddings(
    shard_paths: Sequence[Path],
    *,
    output_dir: Path,
    prefix: str,
    dimension: int,
    query_rows: int,
) -> dict[str, Any]:
    rows = 0
    value_min = float("inf")
    value_max = float("-inf")
    sum_x = np.zeros(dimension, dtype=np.float64)
    sum_xx = np.zeros((dimension, dimension), dtype=np.float64)
    for path in shard_paths:
        embeddings = np.load(path)
        if embeddings.ndim != 2 or embeddings.shape[1] != dimension:
            raise RuntimeError(f"invalid embedding shape in {path}: {embeddings.shape}")
        rows += embeddings.shape[0]
        value_min = min(value_min, float(embeddings.min()))
        value_max = max(value_max, float(embeddings.max()))
        values = embeddings.astype(np.float64)
        sum_x += values.sum(axis=0)
        sum_xx += values.T @ values
    if rows <= query_rows:
        raise ValueError(f"need more than {query_rows} rows, got {rows}")
    if not value_max > value_min:
        raise ValueError("embedding quantization range is empty")

    output_dir.mkdir(parents=True, exist_ok=True)
    query_path = output_dir / f"{prefix}_query.u8bin"
    base_path = output_dir / f"{prefix}_base.u8bin"
    query_temporary = query_path.with_name(f".{query_path.name}.tmp")
    base_temporary = base_path.with_name(f".{base_path.name}.tmp")
    position = 0
    try:
        with query_temporary.open("wb") as query, base_temporary.open("wb") as base:
            np.asarray([query_rows, dimension], dtype="<i4").tofile(query)
            np.asarray([rows - query_rows, dimension], dtype="<i4").tofile(base)
            for path in shard_paths:
                embeddings = np.load(path)
                quantized = np.clip(
                    np.rint((embeddings - value_min) / (value_max - value_min) * 255.0),
                    0,
                    255,
                ).astype(np.uint8)
                query_end = min(quantized.shape[0], max(query_rows - position, 0))
                quantized[:query_end].tofile(query)
                quantized[query_end:].tofile(base)
                position += quantized.shape[0]
        os.replace(query_temporary, query_path)
        os.replace(base_temporary, base_path)
    finally:
        query_temporary.unlink(missing_ok=True)
        base_temporary.unlink(missing_ok=True)

    mean = sum_x / rows
    covariance = sum_xx / rows - np.outer(mean, mean)
    eigenvalues = np.linalg.eigvalsh(covariance)[::-1]
    cumulative = np.cumsum(eigenvalues)
    total_variance = cumulative[-1]
    if not np.isfinite(total_variance) or total_variance <= 0:
        raise ValueError("embedding covariance has no positive variance")
    cumulative /= total_variance
    spectrum = {
        str(threshold): int(np.searchsorted(cumulative, threshold) + 1)
        for threshold in (0.70, 0.79, 0.85, 0.90, 0.95)
    }
    return {
        "rows": rows,
        "base_rows": rows - query_rows,
        "query_rows": query_rows,
        "dimension": dimension,
        "quantization_min": value_min,
        "quantization_max": value_max,
        "spectrum_full": spectrum,
        "base_path": str(base_path),
        "query_path": str(query_path),
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    for name in ("dimension", "shard_size", "query_rows", "batch_size"):
        if getattr(args, name) <= 0:
            raise ValueError(f"{name} must be positive")
    if re.fullmatch(r"[0-9a-f]{40}", args.revision) is None:
        raise ValueError("revision must be an immutable 40-character commit SHA")
    texts_sha256 = _sha256(args.texts)
    corpus_manifest = json.loads(args.corpus_manifest.read_text(encoding="utf-8"))
    if corpus_manifest.get("texts_sha256") != texts_sha256:
        raise ValueError("texts.json does not match the corpus manifest")
    declared_contract = _contract_for_model(corpus_manifest, args.model)
    if declared_contract.get("model_revision") != args.revision:
        raise ValueError("requested model revision does not match the corpus contract")
    if declared_contract.get("output_dimension") != args.dimension:
        raise ValueError("requested dimension does not match the corpus contract")
    texts = json.loads(args.texts.read_text(encoding="utf-8"))
    if not isinstance(texts, list) or not all(isinstance(text, str) for text in texts):
        raise ValueError("texts.json must be an array of strings")

    provider = create_open_model_provider(
        args.model,
        revision=args.revision,
        dimension=args.dimension,
        device=args.device,
        batch_size=args.batch_size,
        normalize=True,
    )
    resolved_contract = provider.get_input_contract()
    if resolved_contract.fingerprint != declared_contract.get("contract_fingerprint"):
        raise ValueError("loaded runtime contract does not match the corpus contract")
    if provider.get_dimension() != args.dimension:
        raise ValueError(
            "loaded runtime dimension does not match the requested dimension"
        )

    shard_paths = prepare_shards(
        output_dir=args.output_dir,
        corpus_sha256=texts_sha256,
        model_id=args.model,
        revision=args.revision,
        contract_fingerprint=resolved_contract.fingerprint,
        dimension=args.dimension,
        shard_size=args.shard_size,
        rows=len(texts),
    )
    pending = pending_shards(
        shard_paths,
        rows=len(texts),
        dimension=args.dimension,
        shard_size=args.shard_size,
    )
    for index, (start, path) in enumerate(pending, 1):
        started = time.monotonic()
        embeddings = np.asarray(
            provider.embed_passages(texts[start : start + args.shard_size]),
            dtype=np.float32,
        )
        expected = (min(args.shard_size, len(texts) - start), args.dimension)
        if embeddings.shape != expected:
            raise RuntimeError(
                f"runtime emitted {embeddings.shape} for shard {start}; expected {expected}"
            )
        _atomic_numpy(embeddings, path)
        elapsed = time.monotonic() - started
        remaining = len(pending) - index
        print(
            f"shard {start:>8,}: {elapsed:.1f}s ({index}/{len(pending)}, eta {remaining * elapsed / 60:.1f}m)",
            flush=True,
        )

    result = finalize_embeddings(
        shard_paths,
        output_dir=args.output_dir,
        prefix=args.prefix,
        dimension=args.dimension,
        query_rows=args.query_rows,
    )
    manifest = {
        "schema_version": 1,
        "model_id": args.model,
        "model_revision": args.revision,
        "contract_fingerprint": resolved_contract.fingerprint,
        "corpus_sha256": texts_sha256,
        "shard_size": args.shard_size,
        "normalize_embeddings": True,
        **result,
    }
    _atomic_json(manifest, args.output_dir / f"{args.prefix}.embedding.manifest.json")
    return manifest


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--texts", type=Path, required=True)
    parser.add_argument("--corpus-manifest", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--dimension", type=int, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--shard-size", type=int, default=20_000)
    parser.add_argument("--query-rows", type=int, default=1_000)
    parser.add_argument("--batch-size", type=int, default=64)
    parser.add_argument("--device")
    return parser.parse_args()


def main() -> None:
    print(json.dumps(run(parse_args()), indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
