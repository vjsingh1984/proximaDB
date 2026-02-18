"""
FinBERT Embedding Provider for ProximaDB

This module provides FinBERT embeddings for financial text analysis,
implementing the ProximaDB embedding provider interface.

FinBERT models available:
1. yiyanghkust/finbert-tone - Sentiment analysis focused
2. ProsusAI/finbert - General financial understanding
3. ahmedrachid/FinancialBERT - Financial document focused
"""

import logging
import os
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

import numpy as np
import torch
from sentence_transformers import SentenceTransformer
from transformers import AutoModel, AutoTokenizer

from .base import EmbeddingConfig, EmbeddingProvider

logger = logging.getLogger(__name__)


class FinBERTProvider(EmbeddingProvider):
    """
    FinBERT embedding provider for financial text

    Features:
    - Multiple FinBERT model variants
    - Batch processing optimization
    - GPU acceleration support
    - Caching for repeated texts
    - Financial term awareness
    """

    # Available FinBERT models with their characteristics
    MODELS = {
        "finbert-tone": {
            "name": "yiyanghkust/finbert-tone",
            "dimension": 768,
            "max_length": 512,
            "description": "FinBERT trained for sentiment analysis",
        },
        "finbert-general": {
            "name": "ProsusAI/finbert",
            "dimension": 768,
            "max_length": 512,
            "description": "General financial text understanding",
        },
        "financial-bert": {
            "name": "ahmedrachid/FinancialBERT",
            "dimension": 768,
            "max_length": 512,
            "description": "Trained on financial documents",
        },
        "finbert-sentence": {
            "name": "sentence-transformers/paraphrase-mpnet-base-v2",
            "dimension": 768,
            "max_length": 512,
            "description": "Sentence transformer fine-tuned for finance",
            "is_sentence_transformer": True,
        },
    }

    def __init__(
        self,
        model_name: str = "finbert-general",
        device: Optional[str] = None,
        cache_dir: Optional[str] = None,
        batch_size: int = 32,
        normalize: bool = True,
        pooling_strategy: str = "mean",
    ):
        """
        Initialize FinBERT provider

        Args:
            model_name: Which FinBERT variant to use
            device: Device to run on ('cuda', 'cpu', or None for auto)
            cache_dir: Directory to cache downloaded models
            batch_size: Batch size for encoding
            normalize: Whether to normalize embeddings
            pooling_strategy: How to pool token embeddings ('mean', 'max', 'cls')
        """
        if model_name not in self.MODELS:
            raise ValueError(
                f"Model {model_name} not found. Available: {list(self.MODELS.keys())}"
            )

        self.model_config = self.MODELS[model_name]
        self.model_name = model_name
        self.batch_size = batch_size
        self.normalize = normalize
        self.pooling_strategy = pooling_strategy

        # Set device
        if device is None:
            self.device = "cuda" if torch.cuda.is_available() else "cpu"
        else:
            self.device = device

        # Set cache directory
        if cache_dir:
            self.cache_dir = Path(cache_dir)
            self.cache_dir.mkdir(parents=True, exist_ok=True)
            os.environ["TRANSFORMERS_CACHE"] = str(self.cache_dir)
        else:
            self.cache_dir = Path.home() / ".cache" / "proximadb" / "models"
            self.cache_dir.mkdir(parents=True, exist_ok=True)

        # Initialize model
        self._load_model()

        # Text cache for repeated embeddings
        self._cache = {}

        logger.info(f"FinBERT provider initialized with {model_name} on {self.device}")

    def _load_model(self):
        """Download and load the FinBERT model"""
        model_path = self.model_config["name"]

        logger.info(f"Loading FinBERT model: {model_path}")

        if self.model_config.get("is_sentence_transformer"):
            # Use sentence-transformers for certain models
            self.model = SentenceTransformer(
                model_path, device=self.device, cache_folder=str(self.cache_dir)
            )
            self.tokenizer = None  # Sentence transformer handles tokenization
        else:
            # Use transformers library
            self.tokenizer = AutoTokenizer.from_pretrained(
                model_path, cache_dir=self.cache_dir
            )
            self.model = AutoModel.from_pretrained(
                model_path, cache_dir=self.cache_dir
            ).to(self.device)
            self.model.eval()

        logger.info(f"Model loaded successfully on {self.device}")

    def embed_text(self, text: str) -> np.ndarray:
        """
        Generate embedding for a single text

        Args:
            text: Input text

        Returns:
            Embedding vector
        """
        return self.embed_texts([text])[0]

    def embed_texts(self, texts: List[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts

        Args:
            texts: List of input texts

        Returns:
            Array of embedding vectors
        """
        # Check cache
        uncached_texts = []
        uncached_indices = []
        cached_embeddings = {}

        for i, text in enumerate(texts):
            cache_key = f"{self.model_name}:{text[:100]}"  # Use first 100 chars as key
            if cache_key in self._cache:
                cached_embeddings[i] = self._cache[cache_key]
            else:
                uncached_texts.append(text)
                uncached_indices.append(i)

        # Process uncached texts
        if uncached_texts:
            if self.model_config.get("is_sentence_transformer"):
                # Use sentence transformer encoding
                new_embeddings = self.model.encode(
                    uncached_texts,
                    batch_size=self.batch_size,
                    normalize_embeddings=self.normalize,
                    show_progress_bar=len(uncached_texts) > 100,
                )
            else:
                # Use transformers encoding
                new_embeddings = self._encode_with_transformers(uncached_texts)

            # Add to cache
            for text, embedding, idx in zip(
                uncached_texts, new_embeddings, uncached_indices
            ):
                cache_key = f"{self.model_name}:{text[:100]}"
                self._cache[cache_key] = embedding
                cached_embeddings[idx] = embedding

        # Combine results in original order
        result = np.zeros((len(texts), self.model_config["dimension"]))
        for i, embedding in cached_embeddings.items():
            result[i] = embedding

        return result

    def _encode_with_transformers(self, texts: List[str]) -> np.ndarray:
        """Encode texts using transformers library"""
        embeddings = []

        # Process in batches
        for i in range(0, len(texts), self.batch_size):
            batch_texts = texts[i : i + self.batch_size]

            # Tokenize
            inputs = self.tokenizer(
                batch_texts,
                padding=True,
                truncation=True,
                max_length=self.model_config["max_length"],
                return_tensors="pt",
            ).to(self.device)

            # Generate embeddings
            with torch.no_grad():
                outputs = self.model(**inputs)

                # Pool embeddings based on strategy
                if self.pooling_strategy == "cls":
                    batch_embeddings = outputs.last_hidden_state[:, 0, :]
                elif self.pooling_strategy == "max":
                    batch_embeddings = outputs.last_hidden_state.max(dim=1)[0]
                else:  # mean
                    attention_mask = inputs["attention_mask"].unsqueeze(-1)
                    masked_embeddings = outputs.last_hidden_state * attention_mask
                    sum_embeddings = masked_embeddings.sum(dim=1)
                    sum_mask = attention_mask.sum(dim=1)
                    batch_embeddings = sum_embeddings / sum_mask

                # Normalize if requested
                if self.normalize:
                    batch_embeddings = torch.nn.functional.normalize(
                        batch_embeddings, p=2, dim=1
                    )

                embeddings.append(batch_embeddings.cpu().numpy())

        return np.vstack(embeddings)

    def embed_documents(
        self, documents: List[Dict[str, Any]], text_field: str = "text"
    ) -> np.ndarray:
        """
        Generate embeddings for documents

        Args:
            documents: List of document dictionaries
            text_field: Field containing text to embed

        Returns:
            Array of embedding vectors
        """
        texts = [doc.get(text_field, "") for doc in documents]
        return self.embed_texts(texts)

    def get_dimension(self) -> int:
        """Get embedding dimension"""
        return self.model_config["dimension"]

    def get_model_info(self) -> Dict[str, Any]:
        """Get model information"""
        return {
            "provider": "FinBERT",
            "model": self.model_name,
            "model_path": self.model_config["name"],
            "dimension": self.model_config["dimension"],
            "max_length": self.model_config["max_length"],
            "device": self.device,
            "description": self.model_config["description"],
        }

    def preprocess_financial_text(self, text: str) -> str:
        """
        Preprocess financial text for better embeddings

        Args:
            text: Raw financial text

        Returns:
            Preprocessed text
        """
        import re

        # Normalize financial numbers
        text = re.sub(r"\$[\d,]+\.?\d*[BMK]?", "[MONEY]", text)
        text = re.sub(r"\d+\.?\d*%", "[PERCENT]", text)

        # Normalize dates
        text = re.sub(r"\b\d{4}-\d{2}-\d{2}\b", "[DATE]", text)
        text = re.sub(r"\b(?:Q[1-4]|FY)\s*\d{4}\b", "[PERIOD]", text)

        # Keep important financial terms
        important_terms = [
            "revenue",
            "earnings",
            "EBITDA",
            "margin",
            "growth",
            "debt",
            "asset",
            "liability",
            "equity",
            "cash flow",
        ]

        # Ensure important terms are preserved
        for term in important_terms:
            text = re.sub(f"\\b{term}\\b", term.upper(), text, flags=re.IGNORECASE)

        return text

    def clear_cache(self):
        """Clear the embedding cache"""
        self._cache.clear()
        logger.info("Embedding cache cleared")


class SECBERTProvider(FinBERTProvider):
    """
    SEC-BERT embedding provider specifically trained on SEC filings

    This extends FinBERT with SEC-specific models and preprocessing
    """

    MODELS = {
        "sec-bert-base": {
            "name": "nlpaueb/sec-bert-base",
            "dimension": 768,
            "max_length": 512,
            "description": "BERT trained on SEC filings",
        },
        "sec-bert-shape": {
            "name": "nlpaueb/sec-bert-shape",
            "dimension": 768,
            "max_length": 512,
            "description": "SEC-BERT for document structure",
        },
        "sec-bert-num": {
            "name": "nlpaueb/sec-bert-num",
            "dimension": 768,
            "max_length": 512,
            "description": "SEC-BERT for numerical understanding",
        },
        "legal-bert": {
            "name": "nlpaueb/legal-bert-base-uncased",
            "dimension": 768,
            "max_length": 512,
            "description": "Legal BERT for regulatory text",
        },
    }

    def __init__(self, model_name: str = "sec-bert-base", **kwargs):
        """
        Initialize SEC-BERT provider

        Args:
            model_name: Which SEC-BERT variant to use
            **kwargs: Additional arguments for parent class
        """
        super().__init__(model_name=model_name, **kwargs)

        # SEC-specific preprocessing
        self.sec_terms = {
            "10-K": "FORM_10K",
            "10-Q": "FORM_10Q",
            "8-K": "FORM_8K",
            "DEF 14A": "FORM_DEF14A",
            "S-1": "FORM_S1",
            "Item 1": "SECTION_BUSINESS",
            "Item 1A": "SECTION_RISK_FACTORS",
            "Item 7": "SECTION_MDA",
            "Item 8": "SECTION_FINANCIAL_STATEMENTS",
        }

    def preprocess_financial_text(self, text: str) -> str:
        """
        Preprocess SEC filing text with SEC-specific normalization

        Args:
            text: Raw SEC filing text

        Returns:
            Preprocessed text
        """
        # Apply general financial preprocessing
        text = super().preprocess_financial_text(text)

        # SEC-specific preprocessing
        import re

        # Normalize CIK numbers
        text = re.sub(r"\b\d{10}\b", "[CIK]", text)

        # Normalize accession numbers
        text = re.sub(r"\b\d{10}-\d{2}-\d{6}\b", "[ACCESSION]", text)

        # Replace SEC form references
        for term, replacement in self.sec_terms.items():
            text = text.replace(term, replacement)

        # Normalize XBRL tags
        text = re.sub(r"<[^>]+>", "", text)  # Remove XML/HTML tags

        # Normalize table references
        text = re.sub(r"See Table \d+", "[TABLE_REF]", text)
        text = re.sub(r"See Note \d+", "[NOTE_REF]", text)

        return text


def download_and_test_models():
    """
    Download and test FinBERT and SEC-BERT models
    """
    import time

    print("Downloading and testing financial embedding models...")

    # Test FinBERT
    print("\n1. Testing FinBERT...")
    finbert = FinBERTProvider(model_name="finbert-general")

    test_texts = [
        "The company reported strong revenue growth in Q3 2024.",
        "Risk factors include market volatility and regulatory changes.",
        "Net income increased by 15% year-over-year.",
    ]

    start = time.time()
    embeddings = finbert.embed_texts(test_texts)
    elapsed = time.time() - start

    print(f"   - Model: {finbert.get_model_info()['model_path']}")
    print(f"   - Dimension: {embeddings.shape}")
    print(f"   - Time: {elapsed:.2f}s")
    print(f"   - Device: {finbert.device}")

    # Test SEC-BERT
    print("\n2. Testing SEC-BERT...")
    secbert = SECBERTProvider(model_name="sec-bert-base")

    sec_texts = [
        "Item 1A. Risk Factors - The company faces significant competition.",
        "Form 10-K filed with accession number 0000320193-24-000123.",
        "See Note 12 to the consolidated financial statements.",
    ]

    start = time.time()
    embeddings = secbert.embed_texts(sec_texts)
    elapsed = time.time() - start

    print(f"   - Model: {secbert.get_model_info()['model_path']}")
    print(f"   - Dimension: {embeddings.shape}")
    print(f"   - Time: {elapsed:.2f}s")
    print(f"   - Device: {secbert.device}")

    # Test preprocessing
    print("\n3. Testing SEC preprocessing...")
    raw_text = "Form 10-K for Apple Inc. (CIK: 0000320193) filed on 2024-11-01"
    processed = secbert.preprocess_financial_text(raw_text)
    print(f"   - Original: {raw_text}")
    print(f"   - Processed: {processed}")

    print("\n✅ Models downloaded and ready for use!")

    return finbert, secbert


if __name__ == "__main__":
    # Download and test models when run directly
    download_and_test_models()
