#!/usr/bin/env python3
"""Validate real tokenizer/rendering contracts for the open-model catalog."""

from __future__ import annotations

import argparse
import json

from proximadb_sdk.chunking_strategies import HuggingFaceTokenCounter
from proximadb_sdk.embedding_providers.catalog import (
    OPEN_MODEL_CATALOG,
    get_open_model_spec,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", action="append", dest="models")
    parser.add_argument("--local-files-only", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    model_ids = args.models or sorted(OPEN_MODEL_CATALOG)
    results = []
    failed = False
    for model_id in model_ids:
        spec = get_open_model_spec(model_id)
        metadata = spec.metadata
        try:
            counter = HuggingFaceTokenCounter.from_pretrained(
                metadata.tokenizer_name or metadata.name,
                revision=metadata.revision,
                trust_remote_code=spec.trust_remote_code,
                local_files_only=args.local_files_only,
            )
            document = metadata.document_template.format(text="alpha beta gamma")
            query_template = metadata.query_template or "{text}"
            query = query_template.format(text="alpha beta gamma")
            offsets = counter.content_offsets("alpha beta gamma")
            result = {
                "model_id": model_id,
                "status": "ok",
                "declared_context": metadata.max_length,
                "tokenizer_context": counter.advertised_limit,
                "document_tokens": counter.count(document),
                "query_tokens": counter.count(query),
                "offset_count": len(offsets or ()),
                "tokenizer_fingerprint": counter.fingerprint,
            }
            if counter.advertised_limit is not None:
                if metadata.max_length > counter.advertised_limit:
                    raise ValueError("declared model context exceeds tokenizer context")
            if not offsets:
                raise ValueError("tokenizer returned no source offsets")
        except Exception as exc:
            failed = True
            result = {
                "model_id": model_id,
                "status": "error",
                "error": f"{type(exc).__name__}: {exc}",
            }
        results.append(result)
        print(json.dumps(result, sort_keys=True))
    print(
        json.dumps(
            {
                "models": len(results),
                "passed": sum(row["status"] == "ok" for row in results),
                "failed": sum(row["status"] != "ok" for row in results),
            },
            sort_keys=True,
        )
    )
    if failed:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
