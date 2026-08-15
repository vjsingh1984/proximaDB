"""
Instruction mixin

Provides query instruction support for retrieval-optimized models.
"""

import logging

import numpy as np

logger = logging.getLogger(__name__)


class InstructionMixin:
    """
    Mixin for providers with query instruction support

    Many retrieval-optimized models (BGE, E5, SFR, gte-Qwen) require
    special instruction prefixes for optimal search performance.

    This mixin provides:
    - Automatic instruction application
    - Separate methods for queries vs passages
    - Configurable instruction templates

    Usage:
        class MyProvider(InstructionMixin, SentenceTransformerMixin, BaseEmbeddingProvider):
            def default_config(self) -> ProviderConfig:
                return ProviderConfig(
                    model=ModelMetadata(
                        requires_instruction=True,
                        instruction_template="Query: {query}"
                    )
                )

    Note:
        This mixin assumes the provider has a `config` attribute with a `model`
        attribute that has `requires_instruction` and `instruction_template` fields.
    """

    def apply_instruction(self, text: str, is_query: bool = True) -> str:
        """
        Apply instruction template to text

        Args:
            text: Input text
            is_query: Whether this is a query (vs a passage/document)

        Returns:
            Text with instruction prefix (if applicable)

        Example:
            >>> mixin = InstructionMixin()
            >>> result = mixin.apply_instruction("machine learning", is_query=True)
            >>> # Returns: "Instruct: Given a query, retrieve relevant passages\nQuery: machine learning"
        """
        if is_query:
            template = self.config.model.query_template
            if template is None and self.config.model.requires_instruction:
                template = self.config.model.instruction_template
        else:
            template = self.config.model.document_template
        if not template:
            return text

        try:
            return template.format(query=text, text=text)
        except KeyError as e:
            logger.error(
                f"Invalid instruction template: {template}. Missing placeholder: {e}"
            )
            return text

    def embed_query(self, query: str) -> np.ndarray:
        """
        Embed query with automatic instruction

        Args:
            query: Query text

        Returns:
            1D NumPy array of shape (dimension,)

        Example:
            >>> provider = MyProvider()
            >>> query_emb = provider.embed_query("What is machine learning?")
            >>> print(query_emb.shape)
            (1536,)
        """
        role_encoder = getattr(self, "_encode_with_prompt", None)
        if callable(role_encoder):
            return role_encoder([query], "query")[0]
        instructed_query = self.apply_instruction(query, is_query=True)
        return self.embed([instructed_query])[0]

    def embed_queries(self, queries: list[str]) -> np.ndarray:
        """
        Embed multiple queries with automatic instructions

        Args:
            queries: List of query strings

        Returns:
            2D NumPy array of shape (len(queries), dimension)

        Example:
            >>> provider = MyProvider()
            >>> queries = ["What is ML?", "What is AI?"]
            >>> query_embs = provider.embed_queries(queries)
            >>> print(query_embs.shape)
            (2, 1536)
        """
        role_encoder = getattr(self, "_encode_with_prompt", None)
        if callable(role_encoder):
            return role_encoder(queries, "query")
        instructed_queries = [self.apply_instruction(q, is_query=True) for q in queries]
        return self.embed(instructed_queries)

    def embed_passages(self, passages: list[str]) -> np.ndarray:
        """
        Embed passages without instructions

        Args:
            passages: List of passage/document strings

        Returns:
            2D NumPy array of shape (len(passages), dimension)

        Example:
            >>> provider = MyProvider()
            >>> passages = ["ML is a subset of AI", "AI enables machines to learn"]
            >>> passage_embs = provider.embed_passages(passages)
            >>> print(passage_embs.shape)
            (2, 1536)
        """
        role_encoder = getattr(self, "_encode_with_prompt", None)
        if callable(role_encoder):
            return role_encoder(passages, "document")
        rendered = [self.apply_instruction(text, is_query=False) for text in passages]
        return self.embed(rendered)

    def embed_documents(self, documents: list[dict]) -> np.ndarray:
        """
        Embed documents (extracts text field)

        Args:
            documents: List of dicts with 'text' field

        Returns:
            2D NumPy array of embeddings

        Example:
            >>> provider = MyProvider()
            >>> docs = [{"text": "ML is great", "id": "1"}]
            >>> doc_embs = provider.embed_documents(docs)
        """
        texts = [
            doc.get("text", "") if isinstance(doc, dict) else str(doc)
            for doc in documents
        ]
        return self.embed_passages(texts)
