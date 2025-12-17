"""
Cohere embedding provider

Uses Cohere's embedding API.
WARNING: Requires API key and incurs costs per token.
"""

import numpy as np
from typing import List, Optional, Dict, Any, Literal
import logging
import os
import warnings

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class CohereProvider(EmbeddingProvider):
    """
    Embedding provider using Cohere's API
    
    ⚠️ WARNING: This provider requires a Cohere API key and will incur costs!
    
    Pricing (as of 2024):
    - embed-english-v3.0: ~$0.1 per 1M tokens
    - embed-multilingual-v3.0: ~$0.1 per 1M tokens
    - embed-english-light-v3.0: ~$0.02 per 1M tokens
    
    Models:
    - embed-english-v3.0: 1024 dims, best for English
    - embed-multilingual-v3.0: 1024 dims, supports 100+ languages
    - embed-english-light-v3.0: 384 dims, faster and cheaper
    - embed-english-v2.0: 4096 dims, legacy model
    
    Set API key via:
    - Environment variable: COHERE_API_KEY
    - Config parameter: api_key
    
    Special features:
    - Input types: "search_document", "search_query", "classification", "clustering"
    - Compression support for reduced dimensions
    """
    
    MODEL_DIMENSIONS = {
        "embed-english-v3.0": 1024,
        "embed-multilingual-v3.0": 1024,
        "embed-english-light-v3.0": 384,
        "embed-english-v2.0": 4096,
        "embed-multilingual-v2.0": 768,
    }
    
    # Supported input types for different use cases
    INPUT_TYPES = ["search_document", "search_query", "classification", "clustering"]
    
    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="embed-english-light-v3.0",  # Cheaper option
            dimension=384,
            batch_size=96,  # Cohere max batch size
            normalize=True,
            cache_embeddings=True,  # Cache to reduce costs
            device=None,
            extra_params={
                "api_key": None,
                "input_type": "search_document",
                "truncate": "END",  # How to handle long texts
                "compress": False,  # Whether to use compression
                "compression_codebook": None,
                "show_cost_warnings": True,
                "timeout": 60.0,
            }
        )
    
    def _initialize(self) -> None:
        """Initialize the Cohere client"""
        try:
            import cohere
            
            # Get API key
            api_key = self.config.extra_params.get("api_key") or os.getenv("COHERE_API_KEY")
            
            if not api_key:
                self._available = False
                logger.error("Cohere API key not found. Set COHERE_API_KEY environment variable or pass api_key in config.")
                return
            
            # Show cost warning
            if self.config.extra_params.get("show_cost_warnings", True):
                warnings.warn(
                    f"⚠️  Cohere embeddings will incur costs! Model '{self.config.model_name}' charges per token. "
                    f"Consider using free alternatives like SentenceTransformer or FastEmbed for development.",
                    UserWarning,
                    stacklevel=2
                )
            
            # Initialize client
            self.client = cohere.Client(api_key=api_key)
            
            # Update dimension
            if self.config.model_name in self.MODEL_DIMENSIONS:
                self.config.dimension = self.MODEL_DIMENSIONS[self.config.model_name]
            
            self._available = True
            self._token_count = 0  # Track usage
            
            logger.info(f"Initialized Cohere provider with model: {self.config.model_name} "
                       f"(dimension: {self.config.dimension})")
            logger.warning("Remember: Cohere embeddings incur costs per token!")
            
        except ImportError:
            self._available = False
            logger.warning("cohere not installed. Install with: pip install cohere")
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize Cohere: {e}")
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts
        
        Args:
            texts: List of texts to embed
            
        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError("Cohere not available. Check API key and installation.")
        
        if not texts:
            return np.array([])
        
        all_embeddings = []
        
        # Estimate tokens (rough estimate for Cohere)
        estimated_tokens = sum(len(text.split()) for text in texts)
        self._token_count += estimated_tokens
        
        if self.config.extra_params.get("show_cost_warnings", True) and estimated_tokens > 100000:
            warnings.warn(
                f"⚠️  About to process ~{estimated_tokens:,} tokens with {self.config.model_name}. "
                f"Estimated cost: ${self._estimate_cost(estimated_tokens):.4f}",
                UserWarning
            )
        
        # Process in batches (Cohere max batch size is 96)
        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i:i + self.config.batch_size]
            
            try:
                response = self.client.embed(
                    texts=batch,
                    model=self.config.model_name,
                    input_type=self.config.extra_params.get("input_type", "search_document"),
                    truncate=self.config.extra_params.get("truncate", "END"),
                    compress=self.config.extra_params.get("compress", False),
                    compression_codebook=self.config.extra_params.get("compression_codebook"),
                )
                
                # Extract embeddings
                batch_embeddings = response.embeddings
                all_embeddings.extend(batch_embeddings)
                
                # Log token usage if available
                if hasattr(response, 'meta') and hasattr(response.meta, 'billed_units'):
                    tokens = response.meta.billed_units.input_tokens
                    logger.info(f"Cohere API used {tokens} tokens for {len(batch)} texts")
                
            except Exception as e:
                logger.error(f"Cohere API error: {e}")
                raise RuntimeError(f"Failed to generate embeddings: {e}")
        
        embeddings = np.array(all_embeddings)
        
        # Normalize if requested
        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            norms[norms == 0] = 1  # Avoid division by zero
            embeddings = embeddings / norms
        
        return embeddings
    
    def embed_with_type(
        self,
        texts: List[str],
        input_type: Literal["search_document", "search_query", "classification", "clustering"]
    ) -> np.ndarray:
        """
        Generate embeddings with specific input type
        
        Args:
            texts: List of texts to embed
            input_type: Type of input for optimization
            
        Returns:
            Array of embeddings
        """
        # Temporarily override input type
        original_type = self.config.extra_params.get("input_type")
        self.config.extra_params["input_type"] = input_type
        
        try:
            embeddings = self.embed_texts(texts)
        finally:
            # Restore original type
            self.config.extra_params["input_type"] = original_type
        
        return embeddings
    
    def _estimate_cost(self, tokens: int) -> float:
        """Estimate cost based on token count"""
        # Cohere pricing per 1M tokens
        cost_per_1m = {
            "embed-english-v3.0": 0.10,
            "embed-multilingual-v3.0": 0.10,
            "embed-english-light-v3.0": 0.02,
            "embed-english-v2.0": 0.10,
            "embed-multilingual-v2.0": 0.10,
        }
        
        rate = cost_per_1m.get(self.config.model_name, 0.10)
        return (tokens / 1_000_000) * rate
    
    @property
    def dimension(self) -> int:
        """Get embedding dimension"""
        return self.config.dimension
    
    @property
    def model_name(self) -> str:
        """Get model name"""
        return self.config.model_name
    
    def is_available(self) -> bool:
        """Check if provider is available"""
        if self._available is None:
            self._initialize()
        return self._available
    
    def get_token_usage(self) -> Dict[str, Any]:
        """Get token usage statistics"""
        return {
            "estimated_tokens": self._token_count,
            "estimated_cost": self._estimate_cost(self._token_count),
            "model": self.config.model_name
        }
    
    @classmethod
    def list_models(cls) -> Dict[str, Dict[str, Any]]:
        """List available models with details"""
        return {
            "embed-english-light-v3.0": {
                "dimension": 384,
                "description": "Lightweight English model, very cost-effective",
                "cost_per_1m_tokens": "$0.02",
                "max_tokens": 512,
                "languages": ["en"]
            },
            "embed-english-v3.0": {
                "dimension": 1024,
                "description": "High-quality English embeddings",
                "cost_per_1m_tokens": "$0.10",
                "max_tokens": 512,
                "languages": ["en"]
            },
            "embed-multilingual-v3.0": {
                "dimension": 1024,
                "description": "Supports 100+ languages",
                "cost_per_1m_tokens": "$0.10",
                "max_tokens": 512,
                "languages": ["100+ languages"]
            },
            "embed-english-v2.0": {
                "dimension": 4096,
                "description": "Legacy model with larger dimensions",
                "cost_per_1m_tokens": "$0.10",
                "max_tokens": 512,
                "languages": ["en"]
            }
        }
    
    @classmethod
    def create_for_search(
        cls,
        model_name: str = "embed-english-light-v3.0",
        **kwargs
    ) -> "CohereProvider":
        """
        Create provider optimized for search
        
        Returns separate providers for documents and queries
        """
        # Document provider
        doc_config = EmbeddingConfig(
            model_name=model_name,
            dimension=cls.MODEL_DIMENSIONS.get(model_name, 384),
            extra_params={
                "input_type": "search_document",
                **kwargs
            }
        )
        
        return cls(doc_config)