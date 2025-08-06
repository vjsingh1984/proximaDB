"""
OpenAI embedding provider

Uses OpenAI's embedding API. 
WARNING: Requires API key and incurs costs per token.
"""

import numpy as np
from typing import List, Optional, Dict, Any
import logging
import os
import warnings

from .base import EmbeddingProvider, EmbeddingConfig

logger = logging.getLogger(__name__)


class OpenAIProvider(EmbeddingProvider):
    """
    Embedding provider using OpenAI's API
    
    ⚠️ WARNING: This provider requires an OpenAI API key and will incur costs!
    
    Pricing (as of 2024):
    - text-embedding-ada-002: ~$0.0001 per 1K tokens
    - text-embedding-3-small: ~$0.00002 per 1K tokens  
    - text-embedding-3-large: ~$0.00013 per 1K tokens
    
    Models:
    - text-embedding-ada-002: 1536 dims, legacy model
    - text-embedding-3-small: 1536 dims, newer, cheaper
    - text-embedding-3-large: 3072 dims, highest quality
    
    Set API key via:
    - Environment variable: OPENAI_API_KEY
    - Config parameter: api_key
    """
    
    MODEL_DIMENSIONS = {
        "text-embedding-ada-002": 1536,
        "text-embedding-3-small": 1536,
        "text-embedding-3-large": 3072,
    }
    
    def _get_default_config(self) -> EmbeddingConfig:
        """Get default configuration"""
        return EmbeddingConfig(
            model_name="text-embedding-3-small",  # Cheaper option
            dimension=1536,
            batch_size=100,  # OpenAI supports large batches
            normalize=False,  # OpenAI embeddings are already normalized
            cache_embeddings=True,  # Cache to reduce costs
            device=None,
            extra_params={
                "api_key": None,
                "organization": None,
                "api_base": None,
                "api_version": None,
                "max_retries": 3,
                "timeout": 60.0,
                "show_cost_warnings": True,
            }
        )
    
    def _initialize(self) -> None:
        """Initialize the OpenAI client"""
        try:
            import openai
            
            # Get API key
            api_key = self.config.extra_params.get("api_key") or os.getenv("OPENAI_API_KEY")
            
            if not api_key:
                self._available = False
                logger.error("OpenAI API key not found. Set OPENAI_API_KEY environment variable or pass api_key in config.")
                return
            
            # Show cost warning
            if self.config.extra_params.get("show_cost_warnings", True):
                warnings.warn(
                    f"⚠️  OpenAI embeddings will incur costs! Model '{self.config.model_name}' charges per token. "
                    f"Consider using free alternatives like SentenceTransformer or FastEmbed for development.",
                    UserWarning,
                    stacklevel=2
                )
            
            # Configure client
            openai.api_key = api_key
            
            if self.config.extra_params.get("organization"):
                openai.organization = self.config.extra_params["organization"]
            
            if self.config.extra_params.get("api_base"):
                openai.api_base = self.config.extra_params["api_base"]
            
            if self.config.extra_params.get("api_version"):
                openai.api_version = self.config.extra_params["api_version"]
            
            self.client = openai
            
            # Update dimension
            if self.config.model_name in self.MODEL_DIMENSIONS:
                self.config.dimension = self.MODEL_DIMENSIONS[self.config.model_name]
            
            self._available = True
            self._token_count = 0  # Track usage
            
            logger.info(f"Initialized OpenAI provider with model: {self.config.model_name} "
                       f"(dimension: {self.config.dimension})")
            logger.warning("Remember: OpenAI embeddings incur costs per token!")
            
        except ImportError:
            self._available = False
            logger.warning("openai not installed. Install with: pip install openai")
        except Exception as e:
            self._available = False
            logger.error(f"Failed to initialize OpenAI: {e}")
    
    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts
        
        Args:
            texts: List of texts to embed
            
        Returns:
            Array of embeddings with shape (len(texts), dimension)
        """
        if not self._available:
            raise RuntimeError("OpenAI not available. Check API key and installation.")
        
        if not texts:
            return np.array([])
        
        all_embeddings = []
        
        # Estimate tokens (rough estimate: 1 token ≈ 4 characters)
        estimated_tokens = sum(len(text) for text in texts) // 4
        self._token_count += estimated_tokens
        
        if self.config.extra_params.get("show_cost_warnings", True) and estimated_tokens > 10000:
            warnings.warn(
                f"⚠️  About to process ~{estimated_tokens:,} tokens with {self.config.model_name}. "
                f"Estimated cost: ${self._estimate_cost(estimated_tokens):.4f}",
                UserWarning
            )
        
        # Process in batches
        for i in range(0, len(texts), self.config.batch_size):
            batch = texts[i:i + self.config.batch_size]
            
            try:
                response = self.client.Embedding.create(
                    model=self.config.model_name,
                    input=batch,
                    encoding_format="float"  # Get as floats, not base64
                )
                
                # Extract embeddings
                batch_embeddings = [item["embedding"] for item in response["data"]]
                all_embeddings.extend(batch_embeddings)
                
                # Log usage if available
                if "usage" in response:
                    actual_tokens = response["usage"]["total_tokens"]
                    logger.info(f"OpenAI API used {actual_tokens} tokens for {len(batch)} texts")
                
            except Exception as e:
                logger.error(f"OpenAI API error: {e}")
                raise RuntimeError(f"Failed to generate embeddings: {e}")
        
        embeddings = np.array(all_embeddings)
        
        # OpenAI embeddings are already normalized, but normalize if requested
        if self.config.normalize:
            norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
            embeddings = embeddings / norms
        
        return embeddings
    
    def _estimate_cost(self, tokens: int) -> float:
        """Estimate cost based on token count"""
        # Rough pricing estimates (check OpenAI for current prices)
        cost_per_1k = {
            "text-embedding-ada-002": 0.0001,
            "text-embedding-3-small": 0.00002,
            "text-embedding-3-large": 0.00013,
        }
        
        rate = cost_per_1k.get(self.config.model_name, 0.0001)
        return (tokens / 1000) * rate
    
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
            "text-embedding-3-small": {
                "dimension": 1536,
                "description": "Newest small model, very cost-effective",
                "cost_per_1k_tokens": "$0.00002",
                "max_tokens": 8191
            },
            "text-embedding-3-large": {
                "dimension": 3072,
                "description": "Highest quality, more expensive",
                "cost_per_1k_tokens": "$0.00013",
                "max_tokens": 8191
            },
            "text-embedding-ada-002": {
                "dimension": 1536,
                "description": "Legacy model, being phased out",
                "cost_per_1k_tokens": "$0.0001",
                "max_tokens": 8191
            }
        }