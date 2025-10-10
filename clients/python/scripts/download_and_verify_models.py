#!/usr/bin/env python3
"""
Download and Verify Embedding Models

This script downloads all recommended embedding models from HuggingFace
and verifies they load correctly.

Models to download:
1. SFR: Salesforce/SFR-Embedding-2_R (4096 dims, ~14GB)
2. BGE: BAAI/bge-large-en-v1.5 (1024 dims, ~1.3GB)
3. BGE: BAAI/bge-base-en-v1.5 (768 dims, ~438MB)
4. BGE: BAAI/bge-small-en-v1.5 (384 dims, ~134MB)
5. E5: intfloat/e5-large-v2 (1024 dims, ~1.3GB)
6. E5: intfloat/e5-base-v2 (768 dims, ~438MB)
7. Sentence-Transformers: all-mpnet-base-v2 (768 dims, ~438MB)
8. Sentence-Transformers: all-MiniLM-L6-v2 (384 dims, ~91MB)

Total download size: ~18GB
"""

import sys
import time
from typing import List, Dict, Any
import numpy as np

# Add src to path
sys.path.insert(0, 'src')

from proximadb.embedding_providers import get_provider
from proximadb.embedding_providers.core import ProviderConfig, ModelMetadata
from proximadb.embedding_providers.providers.local.gte_qwen import GTEQwenProvider, GTE_QWEN_MODELS
from proximadb.embedding_providers.providers.local.sfr import SFRProvider, SFR_MODELS
from proximadb.embedding_providers.providers.local.bge import BGEProvider, BGE_MODELS
from proximadb.embedding_providers.providers.local.e5 import E5Provider, E5_MODELS
from proximadb.embedding_providers.providers.local.sentence_transformer import (
    SentenceTransformerProvider,
    SENTENCE_TRANSFORMER_MODELS
)


class ModelDownloader:
    """Download and verify embedding models"""

    def __init__(self):
        self.results = []

    def download_and_verify(
        self,
        provider_class,
        config: ProviderConfig,
        provider_name: str,
        test_texts: List[str]
    ) -> Dict[str, Any]:
        """Download model and verify it works"""
        print(f"\n{'='*80}")
        print(f"Testing: {provider_name}")
        print(f"Model: {config.model.name}")
        print(f"Expected dimension: {config.model.dimension}")
        print(f"{'='*80}")

        result = {
            "provider": provider_name,
            "model": config.model.name,
            "dimension": config.model.dimension,
            "success": False,
            "error": None,
            "download_time": 0,
            "embedding_time": 0,
            "actual_dimension": None
        }

        try:
            # Download and initialize
            print(f"Initializing provider (will download if needed)...")
            start_time = time.time()
            provider = provider_class(config)
            provider.ensure_initialized()  # Trigger model loading
            download_time = time.time() - start_time
            result["download_time"] = download_time
            print(f"✓ Provider initialized in {download_time:.2f}s")

            # Verify initialization
            if not provider._initialized:
                raise RuntimeError("Provider failed to initialize")
            print(f"✓ Provider initialized")

            # Get model info
            print(f"✓ Model config: {config.model.name}, dimension={config.model.dimension}")

            # Test embedding generation
            print(f"Generating embeddings for {len(test_texts)} test texts...")
            start_time = time.time()
            embeddings = provider.embed(test_texts)
            embedding_time = time.time() - start_time
            result["embedding_time"] = embedding_time
            print(f"✓ Generated embeddings in {embedding_time:.2f}s")

            # Verify dimensions
            actual_dim = embeddings.shape[1]
            result["actual_dimension"] = actual_dim
            print(f"✓ Embedding shape: {embeddings.shape}")

            if actual_dim != config.model.dimension:
                raise ValueError(f"Dimension mismatch: expected {config.model.dimension}, got {actual_dim}")
            print(f"✓ Dimension matches expected: {actual_dim}")

            # Test query embedding (if provider supports it)
            if hasattr(provider, 'embed_query'):
                query_emb = provider.embed_query("test query")
                print(f"✓ Query embedding shape: {query_emb.shape}")

            # Test document embedding
            if hasattr(provider, 'embed_documents'):
                docs = [{"text": text} for text in test_texts]
                doc_embs = provider.embed_documents(docs)
                print(f"✓ Document embeddings shape: {doc_embs.shape}")

            # Verify normalization
            if config.normalize:
                norms = np.linalg.norm(embeddings, axis=1)
                avg_norm = np.mean(norms)
                print(f"✓ Average L2 norm: {avg_norm:.4f} (should be ~1.0 if normalized)")

            result["success"] = True
            print(f"\n✅ SUCCESS: {provider_name} ({config.model.name})")

        except Exception as e:
            result["error"] = str(e)
            print(f"\n❌ FAILED: {provider_name}")
            print(f"Error: {e}")
            import traceback
            traceback.print_exc()

        self.results.append(result)
        return result

    def print_summary(self):
        """Print summary of all downloads"""
        print("\n" + "="*80)
        print("DOWNLOAD AND VERIFICATION SUMMARY")
        print("="*80)

        total = len(self.results)
        successful = sum(1 for r in self.results if r["success"])
        failed = total - successful

        print(f"\nTotal models tested: {total}")
        print(f"Successful: {successful}")
        print(f"Failed: {failed}")

        print("\nDetailed Results:")
        print("┌" + "─"*40 + "┬" + "─"*10 + "┬" + "─"*12 + "┬" + "─"*12 + "┐")
        print("│ Model" + " "*35 + "│ Dims     │ Download (s) │ Embed (s)    │")
        print("├" + "─"*40 + "┼" + "─"*10 + "┼" + "─"*12 + "┼" + "─"*12 + "┤")

        for r in self.results:
            status = "✓" if r["success"] else "✗"
            model_short = r["model"].split("/")[-1][:38]
            dim_str = str(r["actual_dimension"] or "N/A")
            download_str = f"{r['download_time']:.2f}" if r["download_time"] > 0 else "N/A"
            embed_str = f"{r['embedding_time']:.2f}" if r["embedding_time"] > 0 else "N/A"

            print(f"│ {status} {model_short:<37}│ {dim_str:<8} │ {download_str:<10} │ {embed_str:<10} │")

        print("└" + "─"*40 + "┴" + "─"*10 + "┴" + "─"*12 + "┴" + "─"*12 + "┘")

        if failed > 0:
            print("\nFailed models:")
            for r in self.results:
                if not r["success"]:
                    print(f"  ✗ {r['model']}: {r['error']}")

        return successful, failed


def main():
    """Download and verify all models"""
    print("="*80)
    print("EMBEDDING MODEL DOWNLOAD AND VERIFICATION")
    print("="*80)
    print("\nThis script will download and verify the following models:")
    print("1. gte-Qwen2-1.5B-instruct (1536 dims, ~3GB)")
    print("2. SFR-Embedding-2_R (4096 dims, ~14GB)")
    print("3. bge-large-en-v1.5 (1024 dims, ~1.3GB)")
    print("4. bge-base-en-v1.5 (768 dims, ~438MB)")
    print("5. bge-small-en-v1.5 (384 dims, ~134MB)")
    print("6. e5-large-v2 (1024 dims, ~1.3GB)")
    print("7. e5-base-v2 (768 dims, ~438MB)")
    print("8. all-mpnet-base-v2 (768 dims, ~438MB)")
    print("9. all-MiniLM-L6-v2 (384 dims, ~91MB)")
    print("\nTotal download size: ~21GB")
    print("Models will be cached in: ~/.cache/huggingface/")

    response = input("\nContinue? [y/N]: ")
    if response.lower() != 'y':
        print("Aborted.")
        return

    downloader = ModelDownloader()

    # Test texts
    test_texts = [
        "This is a test sentence about machine learning.",
        "Financial markets experienced significant volatility today.",
        "The Court finds that the defendant's motion is denied.",
        "Patient presents with acute myocardial infarction.",
        "We propose a novel attention mechanism for neural networks."
    ]

    # 1. gte-Qwen Provider - #1 MTEB multilingual
    print("\n" + "="*80)
    print("DOWNLOADING GTE-QWEN MODELS (#1 MTEB multilingual)")
    print("="*80)

    downloader.download_and_verify(
        GTEQwenProvider,
        ProviderConfig(
            model=GTE_QWEN_MODELS["Alibaba-NLP/gte-Qwen2-1.5B-instruct"],
            batch_size=16,
            normalize=True,
            trust_remote_code=False  # Set to False for compatibility
        ),
        "gte-Qwen 1.5B (#1 MTEB)",
        test_texts
    )

    # 2. SFR Provider - Top accuracy (English focus)
    print("\n" + "="*80)
    print("DOWNLOADING SFR MODELS (Large downloads)")
    print("="*80)

    downloader.download_and_verify(
        SFRProvider,
        ProviderConfig(
            model=SFR_MODELS["Salesforce/SFR-Embedding-2_R"],
            batch_size=16,
            normalize=True
        ),
        "SFR (Top English Accuracy)",
        test_texts
    )

    # 2. BGE Providers - Top retrieval
    print("\n" + "="*80)
    print("DOWNLOADING BGE MODELS")
    print("="*80)

    for model_name, desc in [
        ("BAAI/bge-large-en-v1.5", "BGE Large"),
        ("BAAI/bge-base-en-v1.5", "BGE Base"),
        ("BAAI/bge-small-en-v1.5", "BGE Small")
    ]:
        downloader.download_and_verify(
            BGEProvider,
            ProviderConfig(
                model=BGE_MODELS[model_name],
                batch_size=32,
                normalize=True
            ),
            desc,
            test_texts
        )

    # 3. E5 Providers - General purpose
    print("\n" + "="*80)
    print("DOWNLOADING E5 MODELS")
    print("="*80)

    for model_name, desc in [
        ("intfloat/e5-large-v2", "E5 Large"),
        ("intfloat/e5-base-v2", "E5 Base")
    ]:
        downloader.download_and_verify(
            E5Provider,
            ProviderConfig(
                model=E5_MODELS[model_name],
                batch_size=32,
                normalize=True
            ),
            desc,
            test_texts
        )

    # 4. Sentence-Transformers - Versatile
    print("\n" + "="*80)
    print("DOWNLOADING SENTENCE-TRANSFORMERS MODELS")
    print("="*80)

    for model_name, desc in [
        ("all-mpnet-base-v2", "MPNet (Quality)"),
        ("all-MiniLM-L6-v2", "MiniLM (Speed)")
    ]:
        downloader.download_and_verify(
            SentenceTransformerProvider,
            ProviderConfig(
                model=SENTENCE_TRANSFORMER_MODELS[model_name],
                batch_size=64,
                normalize=True
            ),
            desc,
            test_texts
        )

    # Print summary
    successful, failed = downloader.print_summary()

    # Exit with appropriate code
    if failed > 0:
        print(f"\n⚠️  {failed} model(s) failed to download or verify")
        sys.exit(1)
    else:
        print(f"\n✅ All {successful} models downloaded and verified successfully!")
        sys.exit(0)


if __name__ == "__main__":
    main()
