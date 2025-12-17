"""
Multi-BERT Embedding Providers for ProximaDB

This module provides various BERT model sizes for different use cases:
- MiniLM: Fast, lightweight (384 dimensions)
- BERT Base: Balanced performance (768 dimensions)
- BERT Large: Maximum accuracy (1024 dimensions)
- RoBERTa: Robust performance (768/1024 dimensions)
- DeBERTa: State-of-the-art (768/1536 dimensions)
"""

import os
import logging
from typing import List, Dict, Any, Optional, Union
import numpy as np
from pathlib import Path
from enum import Enum

import torch
from transformers import AutoTokenizer, AutoModel
from sentence_transformers import SentenceTransformer
import torch.nn.functional as F

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class ModelSize(Enum):
    """Model size options"""
    MINI = "mini"          # ~20MB, 384 dims, fastest
    SMALL = "small"        # ~100MB, 512 dims
    BASE = "base"          # ~400MB, 768 dims, balanced
    LARGE = "large"        # ~1.3GB, 1024 dims
    XLARGE = "xlarge"      # ~3GB, 1536 dims, most accurate


class MultiBERTProvider(EmbeddingProvider):
    """
    Multi-size BERT embedding provider with various model options
    
    Features:
    - Multiple model sizes from Mini to XLarge
    - Automatic model selection based on requirements
    - GPU/CPU flexibility
    - Batch processing optimization
    - Memory-efficient options
    """
    
    # Available models by size and capability
    MODELS = {
        # Mini models (fastest, least memory)
        'minilm-l6': {
            'name': 'sentence-transformers/all-MiniLM-L6-v2',
            'dimension': 384,
            'size': ModelSize.MINI,
            'speed': 'fastest',
            'memory': '~22MB',
            'description': 'Lightweight, fast inference',
            'is_sentence_transformer': True
        },
        'minilm-l12': {
            'name': 'sentence-transformers/all-MiniLM-L12-v2',
            'dimension': 384,
            'size': ModelSize.MINI,
            'speed': 'very fast',
            'memory': '~40MB',
            'description': 'Better quality mini model',
            'is_sentence_transformer': True
        },
        
        # Small models
        'distilbert': {
            'name': 'sentence-transformers/all-distilroberta-v1',
            'dimension': 768,
            'size': ModelSize.SMALL,
            'speed': 'fast',
            'memory': '~100MB',
            'description': 'Distilled model, good balance',
            'is_sentence_transformer': True
        },
        
        # Base models (balanced)
        'bert-base': {
            'name': 'bert-base-uncased',
            'dimension': 768,
            'size': ModelSize.BASE,
            'speed': 'moderate',
            'memory': '~420MB',
            'description': 'Original BERT base',
            'is_sentence_transformer': False
        },
        'mpnet-base': {
            'name': 'sentence-transformers/all-mpnet-base-v2',
            'dimension': 768,
            'size': ModelSize.BASE,
            'speed': 'moderate',
            'memory': '~420MB',
            'description': 'High quality general purpose',
            'is_sentence_transformer': True
        },
        'roberta-base': {
            'name': 'roberta-base',
            'dimension': 768,
            'size': ModelSize.BASE,
            'speed': 'moderate',
            'memory': '~480MB',
            'description': 'Robust BERT variant',
            'is_sentence_transformer': False
        },
        
        # Large models (higher accuracy)
        'bert-large': {
            'name': 'bert-large-uncased',
            'dimension': 1024,
            'size': ModelSize.LARGE,
            'speed': 'slow',
            'memory': '~1.3GB',
            'description': 'Original BERT large',
            'is_sentence_transformer': False
        },
        'roberta-large': {
            'name': 'roberta-large',
            'dimension': 1024,
            'size': ModelSize.LARGE,
            'speed': 'slow',
            'memory': '~1.4GB',
            'description': 'Large robust model',
            'is_sentence_transformer': False
        },
        'e5-large': {
            'name': 'intfloat/e5-large-v2',
            'dimension': 1024,
            'size': ModelSize.LARGE,
            'speed': 'slow',
            'memory': '~1.3GB',
            'description': 'State-of-the-art embeddings',
            'is_sentence_transformer': True
        },
        
        # XLarge models (maximum accuracy)
        'deberta-large': {
            'name': 'microsoft/deberta-v3-large',
            'dimension': 1024,
            'size': ModelSize.XLARGE,
            'speed': 'very slow',
            'memory': '~1.5GB',
            'description': 'DeBERTa v3 large',
            'is_sentence_transformer': False
        },
        'deberta-xlarge': {
            'name': 'microsoft/deberta-v2-xlarge',
            'dimension': 1536,
            'size': ModelSize.XLARGE,
            'speed': 'slowest',
            'memory': '~3GB',
            'description': 'Maximum quality DeBERTa',
            'is_sentence_transformer': False
        }
    }
    
    def __init__(
        self,
        model_name: str = 'mpnet-base',
        size: Optional[ModelSize] = None,
        device: Optional[str] = None,
        cache_dir: Optional[str] = None,
        batch_size: int = 32,
        normalize: bool = True,
        pooling_strategy: str = 'mean',
        max_memory_gb: Optional[float] = None
    ):
        """
        Initialize Multi-BERT provider
        
        Args:
            model_name: Specific model name or None for auto-selection
            size: Preferred model size (mini, small, base, large, xlarge)
            device: Device to run on ('cuda', 'cpu', or None for auto)
            cache_dir: Directory to cache downloaded models
            batch_size: Batch size for encoding
            normalize: Whether to normalize embeddings
            pooling_strategy: How to pool token embeddings
            max_memory_gb: Maximum memory to use (helps select model)
        """
        # Auto-select model based on constraints
        if size and not model_name:
            model_name = self._select_model_by_size(size, max_memory_gb)
        elif not model_name:
            model_name = self._auto_select_model(max_memory_gb)
        
        if model_name not in self.MODELS:
            raise ValueError(f"Model {model_name} not found. Available: {list(self.MODELS.keys())}")
        
        self.model_config = self.MODELS[model_name]
        self.model_name = model_name
        self.batch_size = batch_size
        self.normalize = normalize
        self.pooling_strategy = pooling_strategy
        
        # Set device
        if device is None:
            self.device = 'cuda' if torch.cuda.is_available() else 'cpu'
            # Check if model fits in GPU memory
            if self.device == 'cuda' and self.model_config['size'] == ModelSize.XLARGE:
                gpu_memory = torch.cuda.get_device_properties(0).total_memory / 1e9
                if gpu_memory < 8:  # Less than 8GB GPU memory
                    logger.warning(f"GPU memory ({gpu_memory:.1f}GB) may be insufficient for {model_name}")
                    self.device = 'cpu'
        else:
            self.device = device
        
        # Set cache directory
        if cache_dir:
            self.cache_dir = Path(cache_dir)
        else:
            self.cache_dir = Path.home() / '.cache' / 'proximadb' / 'models'
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        os.environ['TRANSFORMERS_CACHE'] = str(self.cache_dir)
        
        # Initialize model
        self._load_model()
        
        # Text cache
        self._cache = {}
        
        logger.info(
            f"MultiBERT initialized: {model_name} "
            f"({self.model_config['memory']}, {self.model_config['dimension']}d) "
            f"on {self.device}"
        )
    
    def _auto_select_model(self, max_memory_gb: Optional[float] = None) -> str:
        """Auto-select best model based on available resources"""
        if torch.cuda.is_available():
            gpu_memory = torch.cuda.get_device_properties(0).total_memory / 1e9
            
            if gpu_memory >= 8:
                return 'e5-large'  # Best quality that fits
            elif gpu_memory >= 4:
                return 'mpnet-base'  # Good balance
            else:
                return 'minilm-l12'  # Lightweight
        else:
            # CPU mode - prefer smaller models
            import psutil
            ram_gb = psutil.virtual_memory().total / 1e9
            
            if max_memory_gb:
                ram_gb = min(ram_gb, max_memory_gb)
            
            if ram_gb >= 16:
                return 'mpnet-base'
            elif ram_gb >= 8:
                return 'distilbert'
            else:
                return 'minilm-l6'
    
    def _select_model_by_size(
        self,
        size: ModelSize,
        max_memory_gb: Optional[float] = None
    ) -> str:
        """Select model by size preference"""
        candidates = [
            name for name, config in self.MODELS.items()
            if config['size'] == size
        ]
        
        if not candidates:
            logger.warning(f"No models found for size {size}, using default")
            return 'mpnet-base'
        
        # Prefer sentence-transformers for ease of use
        st_models = [
            name for name in candidates
            if self.MODELS[name].get('is_sentence_transformer', False)
        ]
        
        return st_models[0] if st_models else candidates[0]
    
    def _load_model(self):
        """Load the selected model"""
        model_path = self.model_config['name']
        
        logger.info(f"Loading model: {model_path}")
        
        if self.model_config.get('is_sentence_transformer'):
            # Use sentence-transformers
            self.model = SentenceTransformer(
                model_path,
                device=self.device,
                cache_folder=str(self.cache_dir)
            )
            self.tokenizer = None
        else:
            # Use transformers
            self.tokenizer = AutoTokenizer.from_pretrained(
                model_path,
                cache_dir=self.cache_dir
            )
            self.model = AutoModel.from_pretrained(
                model_path,
                cache_dir=self.cache_dir
            ).to(self.device)
            self.model.eval()
        
        logger.info(f"Model loaded on {self.device}")
    
    def embed_text(self, text: str) -> np.ndarray:
        """Generate embedding for single text"""
        return self.embed_texts([text])[0]
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """Generate embeddings for multiple texts"""
        # Check cache
        uncached_texts = []
        uncached_indices = []
        cached_embeddings = {}
        
        for i, text in enumerate(texts):
            cache_key = f"{self.model_name}:{hash(text)}"
            if cache_key in self._cache:
                cached_embeddings[i] = self._cache[cache_key]
            else:
                uncached_texts.append(text)
                uncached_indices.append(i)
        
        # Process uncached
        if uncached_texts:
            if self.model_config.get('is_sentence_transformer'):
                # Sentence transformer encoding
                new_embeddings = self.model.encode(
                    uncached_texts,
                    batch_size=self.batch_size,
                    normalize_embeddings=self.normalize,
                    show_progress_bar=len(uncached_texts) > 100,
                    device=self.device
                )
            else:
                # Transformers encoding
                new_embeddings = self._encode_with_transformers(uncached_texts)
            
            # Cache results
            for text, embedding, idx in zip(uncached_texts, new_embeddings, uncached_indices):
                cache_key = f"{self.model_name}:{hash(text)}"
                self._cache[cache_key] = embedding
                cached_embeddings[idx] = embedding
        
        # Combine in order
        result = np.zeros((len(texts), self.model_config['dimension']))
        for i, embedding in cached_embeddings.items():
            result[i] = embedding
        
        return result
    
    def _encode_with_transformers(self, texts: List[str]) -> np.ndarray:
        """Encode using transformers library"""
        embeddings = []
        
        for i in range(0, len(texts), self.batch_size):
            batch = texts[i:i + self.batch_size]
            
            # Tokenize
            inputs = self.tokenizer(
                batch,
                padding=True,
                truncation=True,
                max_length=512,
                return_tensors='pt'
            ).to(self.device)
            
            # Generate embeddings
            with torch.no_grad():
                outputs = self.model(**inputs)
                
                # Pool based on strategy
                if self.pooling_strategy == 'cls':
                    batch_embeddings = outputs.last_hidden_state[:, 0, :]
                elif self.pooling_strategy == 'max':
                    batch_embeddings = outputs.last_hidden_state.max(dim=1)[0]
                else:  # mean
                    attention_mask = inputs['attention_mask'].unsqueeze(-1)
                    masked = outputs.last_hidden_state * attention_mask
                    summed = masked.sum(dim=1)
                    counts = attention_mask.sum(dim=1)
                    batch_embeddings = summed / counts
                
                # Normalize if requested
                if self.normalize:
                    batch_embeddings = F.normalize(batch_embeddings, p=2, dim=1)
                
                embeddings.append(batch_embeddings.cpu().numpy())
        
        return np.vstack(embeddings)
    
    def embed_documents(
        self,
        documents: List[Dict[str, Any]],
        text_field: str = 'text'
    ) -> np.ndarray:
        """Generate embeddings for documents"""
        texts = [doc.get(text_field, '') for doc in documents]
        return self.embed_texts(texts)
    
    def get_dimension(self) -> int:
        """Get embedding dimension"""
        return self.model_config['dimension']
    
    def get_model_info(self) -> Dict[str, Any]:
        """Get detailed model information"""
        return {
            'provider': 'MultiBERT',
            'model': self.model_name,
            'model_path': self.model_config['name'],
            'dimension': self.model_config['dimension'],
            'size': self.model_config['size'].value,
            'speed': self.model_config['speed'],
            'memory': self.model_config['memory'],
            'device': self.device,
            'description': self.model_config['description']
        }
    
    def benchmark(self, test_texts: List[str] = None) -> Dict[str, float]:
        """Benchmark model performance"""
        import time
        
        if test_texts is None:
            test_texts = [
                "This is a test sentence for benchmarking.",
                "The quick brown fox jumps over the lazy dog.",
                "Machine learning models require evaluation."
            ] * 10
        
        # Warmup
        _ = self.embed_texts(test_texts[:3])
        
        # Benchmark
        start = time.time()
        embeddings = self.embed_texts(test_texts)
        elapsed = time.time() - start
        
        return {
            'total_time': elapsed,
            'texts_per_second': len(test_texts) / elapsed,
            'ms_per_text': (elapsed * 1000) / len(test_texts),
            'batch_size': self.batch_size,
            'dimension': embeddings.shape[1],
            'device': self.device
        }
    
    @classmethod
    def compare_models(
        cls,
        texts: List[str],
        models: List[str] = None
    ) -> pd.DataFrame:
        """Compare different models on the same texts"""
        import pandas as pd
        import time
        
        if models is None:
            models = ['minilm-l6', 'distilbert', 'mpnet-base', 'e5-large']
        
        results = []
        
        for model_name in models:
            try:
                logger.info(f"Testing {model_name}...")
                provider = cls(model_name=model_name)
                
                # Time embedding generation
                start = time.time()
                embeddings = provider.embed_texts(texts)
                elapsed = time.time() - start
                
                # Get model info
                info = provider.get_model_info()
                
                results.append({
                    'model': model_name,
                    'dimension': info['dimension'],
                    'size': info['size'],
                    'memory': info['memory'],
                    'speed': info['speed'],
                    'time_seconds': elapsed,
                    'texts_per_sec': len(texts) / elapsed,
                    'device': info['device']
                })
                
                # Clear GPU memory
                if hasattr(provider, 'model'):
                    del provider.model
                torch.cuda.empty_cache()
                
            except Exception as e:
                logger.error(f"Failed to test {model_name}: {e}")
        
        return pd.DataFrame(results)


class AdaptiveBERTProvider(MultiBERTProvider):
    """
    Adaptive BERT provider that automatically selects model based on input
    
    Features:
    - Automatic model selection based on text length
    - Dynamic batching based on available memory
    - Fallback to smaller models on OOM
    """
    
    def __init__(
        self,
        prefer_speed: bool = False,
        prefer_accuracy: bool = False,
        max_memory_gb: Optional[float] = None,
        **kwargs
    ):
        """
        Initialize adaptive provider
        
        Args:
            prefer_speed: Prioritize speed over accuracy
            prefer_accuracy: Prioritize accuracy over speed
            max_memory_gb: Maximum memory to use
            **kwargs: Additional arguments for parent class
        """
        self.prefer_speed = prefer_speed
        self.prefer_accuracy = prefer_accuracy
        
        # Select initial model
        if prefer_speed:
            model_name = 'minilm-l12'
        elif prefer_accuracy:
            model_name = 'e5-large'
        else:
            model_name = None  # Auto-select
        
        super().__init__(
            model_name=model_name,
            max_memory_gb=max_memory_gb,
            **kwargs
        )
        
        # Track performance
        self.performance_stats = {
            'total_texts': 0,
            'total_time': 0,
            'model_switches': 0
        }
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """Adaptively embed texts with automatic model selection"""
        
        # Analyze input
        avg_length = np.mean([len(t) for t in texts])
        total_chars = sum(len(t) for t in texts)
        
        # Switch model if needed
        if self.prefer_speed and avg_length > 1000 and self.model_name != 'minilm-l6':
            logger.info("Switching to mini model for long texts")
            self._switch_model('minilm-l6')
        elif self.prefer_accuracy and avg_length < 200 and self.model_name != 'e5-large':
            logger.info("Switching to large model for short texts")
            self._switch_model('e5-large')
        
        # Try embedding with fallback
        try:
            embeddings = super().embed_texts(texts)
        except torch.cuda.OutOfMemoryError:
            logger.warning("GPU OOM, falling back to smaller model")
            self._switch_model('minilm-l6')
            torch.cuda.empty_cache()
            embeddings = super().embed_texts(texts)
        
        # Update stats
        self.performance_stats['total_texts'] += len(texts)
        
        return embeddings
    
    def _switch_model(self, new_model: str):
        """Switch to a different model"""
        if new_model != self.model_name:
            logger.info(f"Switching from {self.model_name} to {new_model}")
            
            # Clear current model
            if hasattr(self, 'model'):
                del self.model
            if hasattr(self, 'tokenizer'):
                del self.tokenizer
            torch.cuda.empty_cache()
            
            # Load new model
            self.model_name = new_model
            self.model_config = self.MODELS[new_model]
            self._load_model()
            
            self.performance_stats['model_switches'] += 1


def benchmark_all_models():
    """Benchmark all available models"""
    import pandas as pd
    
    test_texts = [
        "The company reported strong financial results for the quarter.",
        "Revenue increased by 15% year-over-year to $10 billion.",
        "Risk factors include market volatility and regulatory changes."
    ] * 10
    
    print("=" * 60)
    print("Benchmarking All BERT Models")
    print("=" * 60)
    
    # Test different model sizes
    models_to_test = [
        'minilm-l6',      # Mini
        'minilm-l12',     # Mini+
        'distilbert',     # Small
        'mpnet-base',     # Base
        'roberta-base',   # Base+
        'e5-large',       # Large
    ]
    
    results = MultiBERTProvider.compare_models(test_texts, models_to_test)
    
    print("\nResults:")
    print(results.to_string())
    
    # Recommendations
    print("\n" + "=" * 60)
    print("Recommendations:")
    print("=" * 60)
    print("🚀 For Speed: Use 'minilm-l6' (384d, 22MB)")
    print("⚖️  Balanced: Use 'mpnet-base' (768d, 420MB)")
    print("🎯 For Accuracy: Use 'e5-large' (1024d, 1.3GB)")
    print("💾 Low Memory: Use 'minilm-l12' (384d, 40MB)")
    
    return results


if __name__ == "__main__":
    # Run benchmarks when executed directly
    benchmark_all_models()