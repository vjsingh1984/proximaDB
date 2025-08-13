#!/usr/bin/env python3
"""
Download and setup financial embedding models for ProximaDB

This script downloads FinBERT and SEC-BERT models and sets them up
for use with the ProximaDB SDK.
"""

import os
import sys
import time
from pathlib import Path

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent / "clients" / "python" / "src"))

def download_models():
    """Download and initialize financial embedding models"""
    
    print("=" * 60)
    print("ProximaDB Financial Model Setup")
    print("=" * 60)
    
    try:
        # Import the providers
        from proximadb.embedding_providers.finbert_provider import (
            FinBERTProvider,
            SECBERTProvider
        )
        
        # Create model cache directory
        cache_dir = Path.home() / '.cache' / 'proximadb' / 'models'
        cache_dir.mkdir(parents=True, exist_ok=True)
        print(f"\n📁 Model cache directory: {cache_dir}")
        
        # Download FinBERT models
        print("\n📥 Downloading FinBERT models...")
        print("-" * 40)
        
        models_to_download = [
            ('finbert-general', 'ProsusAI/finbert'),
            ('finbert-tone', 'yiyanghkust/finbert-tone'),
        ]
        
        for model_name, model_path in models_to_download:
            print(f"\n⏳ Downloading {model_name} ({model_path})...")
            start_time = time.time()
            
            try:
                provider = FinBERTProvider(
                    model_name=model_name,
                    cache_dir=str(cache_dir)
                )
                
                # Test the model
                test_text = "The company reported strong financial results."
                embedding = provider.embed_text(test_text)
                
                elapsed = time.time() - start_time
                print(f"✅ {model_name} downloaded successfully")
                print(f"   - Dimension: {len(embedding)}")
                print(f"   - Download time: {elapsed:.1f}s")
                print(f"   - Device: {provider.device}")
                
            except Exception as e:
                print(f"❌ Failed to download {model_name}: {e}")
        
        # Download SEC-BERT models
        print("\n📥 Downloading SEC-BERT models...")
        print("-" * 40)
        
        sec_models = [
            ('sec-bert-base', 'nlpaueb/sec-bert-base'),
        ]
        
        for model_name, model_path in sec_models:
            print(f"\n⏳ Downloading {model_name} ({model_path})...")
            start_time = time.time()
            
            try:
                provider = SECBERTProvider(
                    model_name=model_name,
                    cache_dir=str(cache_dir)
                )
                
                # Test the model
                test_text = "Item 1A. Risk Factors include market volatility."
                embedding = provider.embed_text(test_text)
                
                elapsed = time.time() - start_time
                print(f"✅ {model_name} downloaded successfully")
                print(f"   - Dimension: {len(embedding)}")
                print(f"   - Download time: {elapsed:.1f}s")
                print(f"   - Device: {provider.device}")
                
            except Exception as e:
                print(f"❌ Failed to download {model_name}: {e}")
        
        # Print summary
        print("\n" + "=" * 60)
        print("✅ Model Setup Complete!")
        print("=" * 60)
        print("\nModels are cached at:")
        print(f"  {cache_dir}")
        print("\nYou can now use these models with ProximaDB:")
        print("  - FinBERT for financial text embeddings")
        print("  - SEC-BERT for SEC filing embeddings")
        
        return True
        
    except ImportError as e:
        print(f"\n❌ Error: Missing dependencies")
        print(f"   {e}")
        print("\n📦 Please install required packages:")
        print("   pip install torch transformers sentence-transformers")
        return False
    
    except Exception as e:
        print(f"\n❌ Unexpected error: {e}")
        return False


if __name__ == "__main__":
    success = download_models()
    sys.exit(0 if success else 1)