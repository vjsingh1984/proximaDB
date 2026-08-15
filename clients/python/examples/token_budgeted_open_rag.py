#!/usr/bin/env python3
"""Chunk and embed a document with any catalogued open-weight model.

Examples:
  python token_budgeted_open_rag.py --list-models
  python token_budgeted_open_rag.py --model nomic-ai/nomic-embed-text-v1.5 README.md
  python token_budgeted_open_rag.py --model google/embeddinggemma-300m --dimension 256 README.md
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from proximadb_sdk.chunking import TextChunker
from proximadb_sdk.chunking_strategies import (
    ChunkingConfig,
    ChunkingStrategy,
    InputRole,
    TokenBudget,
)
from proximadb_sdk.embedding_providers import get_open_embedding_model
from proximadb_sdk.embedding_providers.catalog import list_open_models


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path, nargs="?")
    parser.add_argument("--model", default="sentence-transformers/all-MiniLM-L6-v2")
    parser.add_argument("--dimension", type=int)
    parser.add_argument("--target-tokens", type=int)
    parser.add_argument("--overlap-percent", type=float, default=15.0)
    parser.add_argument("--list-models", action="store_true")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.list_models:
        for spec in list_open_models():
            model = spec.metadata
            print(
                f"{model.name}\t{model.dimension}d\t{model.max_length} tokens\t"
                f"{model.license_id}\t{model.access}"
            )
        return
    if args.path is None:
        raise SystemExit("path is required unless --list-models is used")

    provider = get_open_embedding_model(args.model, dimension=args.dimension)
    contract = provider.get_input_contract()
    target = args.target_tokens or min(480, contract.effective_context_limit)
    overlap = round(target * args.overlap_percent / 100)
    chunker = TextChunker(
        ChunkingConfig(
            strategy=ChunkingStrategy.RECURSIVE,
            chunk_size=4096,
            chunk_overlap=0,
            min_chunk_size=1,
            token_budget=TokenBudget(
                target_tokens=target,
                overlap_tokens=overlap,
                min_content_tokens=16,
            ),
            input_contract=contract,
            input_role=InputRole.DOCUMENT,
        )
    )
    text = args.path.read_text(encoding="utf-8")
    chunks = chunker.chunk_text(text, source_id=str(args.path))
    embeddings = provider.embed_passages([chunk.text for chunk in chunks])
    token_counts = [contract.count(chunk.text, InputRole.DOCUMENT) for chunk in chunks]
    dimension = provider.get_dimension()

    print(
        json.dumps(
            {
                "model_contract": contract.to_manifest(),
                "chunks": len(chunks),
                "token_min": min(token_counts, default=0),
                "token_max": max(token_counts, default=0),
                "truncated_chunks": sum(
                    count > contract.effective_context_limit for count in token_counts
                ),
                "embedding_shape": list(embeddings.shape),
                "float32_vector_bytes": len(chunks) * dimension * 4,
            },
            indent=2,
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
