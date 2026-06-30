"""
FinBERT Embedding Provider for ProximaDB

FinBERT embeddings for financial-text analysis, ported onto
:class:`core.BaseEmbeddingProvider` (TD-126 System-B collapse). Self-registers
under ``finbert``; :class:`SECBERTProvider` registers under ``sec-bert``.

Two backends are supported per model:
- sentence-transformers checkpoints (``provider_type="sentence-transformer"``)
- raw transformers models with configurable token pooling (mean/max/cls)

The heavy ``torch`` / ``transformers`` / ``sentence_transformers`` imports are
performed lazily inside :meth:`_load_model` so importing this module (e.g. at
registry-discovery time) pulls no model dependencies.
"""

import logging
from typing import Any

import numpy as np

from .core.base import BaseEmbeddingProvider
from .core.config import ModelMetadata, ProviderConfig
from .core.device import resolve_device
from .core.registry import ProviderRegistry

logger = logging.getLogger(__name__)


FINBERT_MODELS = {
    "ProsusAI/finbert": ModelMetadata(
        name="ProsusAI/finbert",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="General financial text understanding (FinBERT)",
        use_case="Financial document/sentence embeddings",
    ),
    "yiyanghkust/finbert-tone": ModelMetadata(
        name="yiyanghkust/finbert-tone",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="FinBERT trained for financial sentiment/tone",
        use_case="Financial sentiment analysis",
    ),
    "ahmedrachid/FinancialBERT": ModelMetadata(
        name="ahmedrachid/FinancialBERT",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="Trained on financial documents",
        use_case="Financial document understanding",
    ),
    "sentence-transformers/paraphrase-mpnet-base-v2": ModelMetadata(
        name="sentence-transformers/paraphrase-mpnet-base-v2",
        dimension=768,
        max_length=512,
        provider_type="sentence-transformer",
        languages="en",
        description="Sentence-transformer baseline for finance paraphrase",
        use_case="Drop-in sentence-transformer alternative",
    ),
}

SECBERT_MODELS = {
    "nlpaueb/sec-bert-base": ModelMetadata(
        name="nlpaueb/sec-bert-base",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="BERT trained on SEC filings",
        use_case="SEC filing embeddings",
    ),
    "nlpaueb/sec-bert-shape": ModelMetadata(
        name="nlpaueb/sec-bert-shape",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="SEC-BERT specialised for document structure",
        use_case="Structural SEC-filing understanding",
    ),
    "nlpaueb/sec-bert-num": ModelMetadata(
        name="nlpaueb/sec-bert-num",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="SEC-BERT specialised for numerical understanding",
        use_case="Numerical SEC-filing understanding",
    ),
    "nlpaueb/legal-bert-base-uncased": ModelMetadata(
        name="nlpaueb/legal-bert-base-uncased",
        dimension=768,
        max_length=512,
        provider_type="transformers",
        languages="en",
        description="Legal BERT for regulatory text",
        use_case="Regulatory / legal text embeddings",
    ),
}


@ProviderRegistry.register(
    name="finbert",
    models=FINBERT_MODELS,
    aliases=["financial-bert", "prosus-finbert"],
    description="FinBERT financial-text embeddings (transformers / sentence-transformers)",
)
class FinBERTProvider(BaseEmbeddingProvider):
    """
    FinBERT embedding provider for financial text.

    Features:
    - General + tone + document FinBERT variants
    - Configurable token pooling for the transformers backend
      (``extra["pooling_strategy"]`` in {"mean", "max", "cls"}, default "mean")
    - Financial-term preprocessing helper (:meth:`preprocess_financial_text`)
    """

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=FINBERT_MODELS["ProsusAI/finbert"],
            batch_size=32,
            normalize=True,
            extra={"pooling_strategy": "mean"},
        )

    @property
    def pooling_strategy(self) -> str:
        return self.config.extra.get("pooling_strategy", "mean")

    def _is_sentence_transformer(self) -> bool:
        return self.config.model.provider_type == "sentence-transformer"

    def _load_model(self) -> Any:
        """Load the model lazily (transformers tuple or sentence-transformer)."""
        from .core.cache import ModelCache

        device = resolve_device(self.config.device) or "cpu"
        cache = ModelCache()
        model_path = self.config.model.name

        if self._is_sentence_transformer():
            cache_key = f"finbert_st_{model_path}_{device}"

            def st_loader():
                from sentence_transformers import SentenceTransformer

                logger.info("Loading FinBERT sentence-transformer: %s", model_path)
                return SentenceTransformer(
                    model_path, device=device, cache_folder=self.config.cache_dir
                )

            return cache.get_or_load(cache_key, st_loader)

        cache_key = f"finbert_hf_{model_path}_{device}"

        def hf_loader():
            from transformers import AutoModel, AutoTokenizer

            logger.info("Loading FinBERT transformers model: %s", model_path)
            tokenizer = AutoTokenizer.from_pretrained(
                model_path, cache_dir=self.config.cache_dir
            )
            model = AutoModel.from_pretrained(
                model_path, cache_dir=self.config.cache_dir
            ).to(device)
            model.eval()
            return (tokenizer, model, device)

        return cache.get_or_load(cache_key, hf_loader)

    def embed(self, texts: list[str]) -> np.ndarray:
        if not texts:
            return np.array([])

        self.ensure_initialized()

        if self._is_sentence_transformer():
            return self._model.encode(
                texts,
                batch_size=self.config.batch_size,
                normalize_embeddings=self.config.normalize,
                show_progress_bar=False,
                convert_to_numpy=True,
            )
        return self._encode_with_transformers(texts)

    def _encode_with_transformers(self, texts: list[str]) -> np.ndarray:
        """Encode via the raw transformers backend with the configured pooling."""
        import torch

        tokenizer, model, device = self._model
        max_length = self.config.model.max_length
        embeddings = []

        for i in range(0, len(texts), self.config.batch_size):
            batch_texts = texts[i : i + self.config.batch_size]
            inputs = tokenizer(
                batch_texts,
                padding=True,
                truncation=True,
                max_length=max_length,
                return_tensors="pt",
            ).to(device)

            with torch.no_grad():
                outputs = model(**inputs)

                if self.pooling_strategy == "cls":
                    batch_embeddings = outputs.last_hidden_state[:, 0, :]
                elif self.pooling_strategy == "max":
                    batch_embeddings = outputs.last_hidden_state.max(dim=1)[0]
                else:  # mean
                    attention_mask = inputs["attention_mask"].unsqueeze(-1)
                    masked = outputs.last_hidden_state * attention_mask
                    summed = masked.sum(dim=1)
                    counts = attention_mask.sum(dim=1)
                    batch_embeddings = summed / counts

                if self.config.normalize:
                    batch_embeddings = torch.nn.functional.normalize(
                        batch_embeddings, p=2, dim=1
                    )

                embeddings.append(batch_embeddings.cpu().numpy())

        return np.vstack(embeddings)

    def embed_documents(
        self, documents: list[dict[str, Any]], text_field: str = "text"
    ) -> np.ndarray:
        """Embed documents, extracting ``text_field`` from each."""
        texts = [doc.get(text_field, "") for doc in documents]
        return self.embed(texts)

    def preprocess_financial_text(self, text: str) -> str:
        """Normalise financial text (money/percent/dates/periods) for embedding."""
        import re

        text = re.sub(r"\$[\d,]+\.?\d*[BMK]?", "[MONEY]", text)
        text = re.sub(r"\d+\.?\d*%", "[PERCENT]", text)
        text = re.sub(r"\b\d{4}-\d{2}-\d{2}\b", "[DATE]", text)
        text = re.sub(r"\b(?:Q[1-4]|FY)\s*\d{4}\b", "[PERIOD]", text)

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
        for term in important_terms:
            text = re.sub(f"\\b{term}\\b", term.upper(), text, flags=re.IGNORECASE)

        return text


@ProviderRegistry.register(
    name="sec-bert",
    models=SECBERT_MODELS,
    aliases=["secbert", "legal-bert"],
    description="SEC-BERT embeddings for SEC filings / regulatory text",
)
class SECBERTProvider(FinBERTProvider):
    """
    SEC-BERT provider trained on SEC filings.

    Extends :class:`FinBERTProvider` with SEC-specific default models and
    SEC-aware preprocessing (form references, CIK/accession normalisation).
    """

    SEC_TERMS = {
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

    def default_config(self) -> ProviderConfig:
        return ProviderConfig(
            model=SECBERT_MODELS["nlpaueb/sec-bert-base"],
            batch_size=32,
            normalize=True,
            extra={"pooling_strategy": "mean"},
        )

    def preprocess_financial_text(self, text: str) -> str:
        """Apply general financial preprocessing plus SEC-specific normalisation."""
        import re

        text = super().preprocess_financial_text(text)

        # CIK numbers / accession numbers.
        text = re.sub(r"\b\d{10}-\d{2}-\d{6}\b", "[ACCESSION]", text)
        text = re.sub(r"\b\d{10}\b", "[CIK]", text)

        for term, replacement in self.SEC_TERMS.items():
            text = text.replace(term, replacement)

        text = re.sub(r"<[^>]+>", "", text)  # strip XML/HTML/XBRL tags
        text = re.sub(r"See Table \d+", "[TABLE_REF]", text)
        text = re.sub(r"See Note \d+", "[NOTE_REF]", text)

        return text
