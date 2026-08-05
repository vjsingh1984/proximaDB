#!/usr/bin/env python3
"""Local dataset IO for the context-corridor benchmark (TD-CTXCORR-1 Slice 2b).

SIFT-family ``.fvecs`` / ``.ivecs`` readers and checksum helpers. Benchmark
vectors are kept **local and gitignored** and verified by SHA256 — the checksum
becomes the report ``dataset_hash`` (satisfies ``require_dataset_hash``). These
are the file-format + fetch foundation the SIFT1M / Wikipedia providers
(Slice 2b-ii) build on; nothing here downloads by itself except the injectable
``fetch_corpus`` helper.
"""

from __future__ import annotations

import hashlib
import struct
import urllib.request
from collections.abc import Callable, Iterator
from pathlib import Path

_HEADER = struct.Struct("<i")


def _read_vecs(path: Path | str, code: str) -> Iterator[list[float]]:
    # .fvecs/.ivecs layout: for each vector, an int32 dimension followed by
    # `dimension` little-endian values of the given struct code ('f' or 'i').
    with open(path, "rb") as handle:
        while True:
            header = handle.read(4)
            if not header:
                return
            if len(header) != 4:
                raise ValueError(f"truncated .{code}vecs header in {path}")
            (dimension,) = _HEADER.unpack(header)
            if dimension < 0:
                raise ValueError(f"invalid .{code}vecs dimension {dimension} in {path}")
            body = handle.read(4 * dimension)
            if len(body) != 4 * dimension:
                raise ValueError(f"truncated .{code}vecs vector body in {path}")
            yield list(struct.unpack(f"<{dimension}{code}", body))


def read_fvecs(path: Path | str) -> Iterator[list[float]]:
    """Stream float32 vectors from a ``.fvecs`` file (SIFT base/query vectors)."""
    return _read_vecs(path, "f")


def read_ivecs(path: Path | str) -> Iterator[list[int]]:
    """Stream int32 vectors from an ``.ivecs`` file (SIFT ground-truth neighbours)."""
    return (
        [int(value) for value in vector] for vector in _read_vecs(path, "i")
    )


def sha256_file(path: Path | str, *, chunk_size: int = 1 << 20) -> str:
    hasher = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(chunk_size), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def verify_checksum(path: Path | str, expected: str) -> str:
    actual = sha256_file(path)
    if actual != expected:
        raise ValueError(
            f"checksum mismatch for {path}: expected {expected}, got {actual}"
        )
    return actual


def fetch_corpus(
    url: str,
    dest: Path | str,
    expected_sha256: str,
    *,
    download: Callable[[str, str], object] | None = None,
) -> str:
    """Fetch ``url`` to ``dest`` and verify its SHA256; return the checksum.

    Idempotent: an existing ``dest`` with the right checksum is reused (no
    download). Downloads to a ``.part`` sidecar and only promotes it on a
    checksum match, so a mismatch never leaves a corrupt corpus in place. The
    downloader is injectable for testing.
    """
    download = download or urllib.request.urlretrieve
    dest = Path(dest)
    if dest.exists():
        actual = sha256_file(dest)
        if actual == expected_sha256:
            return actual
        raise ValueError(
            f"{dest} exists but its checksum {actual} != expected "
            f"{expected_sha256}; delete it to re-fetch"
        )
    dest.parent.mkdir(parents=True, exist_ok=True)
    partial = dest.with_name(dest.name + ".part")
    try:
        download(url, str(partial))
        actual = sha256_file(partial)
        if actual != expected_sha256:
            raise ValueError(
                f"checksum mismatch for {url}: expected {expected_sha256}, "
                f"got {actual}"
            )
        partial.replace(dest)
    finally:
        if partial.exists():
            partial.unlink()
    return actual
