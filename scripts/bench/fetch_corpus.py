#!/usr/bin/env python3
"""Fetch + SHA256-verify a context-corridor benchmark corpus (TD-CTXCORR-1 Slice 2b).

Vectors are kept local and gitignored under benches/context-corridor/data/; the
printed checksum is the report ``dataset_hash``. Idempotent — an existing file
with the right checksum is reused.

Example (SIFT1M base vectors)::

    python3 scripts/bench/fetch_corpus.py \\
      --url http://<mirror>/sift/sift_base.fvecs \\
      --dest benches/context-corridor/data/sift/sift_base.fvecs \\
      --sha256 <expected-sha256>
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import corpus_io  # noqa: E402  (path-injected sibling module)

DATA_DIR = Path(__file__).resolve().parents[2] / "benches/context-corridor/data"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--dest", type=Path, required=True)
    parser.add_argument("--sha256", required=True, help="expected SHA256 of the file")
    args = parser.parse_args()
    try:
        checksum = corpus_io.fetch_corpus(args.url, args.dest, args.sha256)
    except (OSError, ValueError) as error:
        print(f"fetch_corpus failed: {error}", file=sys.stderr)
        return 1
    print(checksum)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
