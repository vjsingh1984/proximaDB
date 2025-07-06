#!/usr/bin/env python3
"""
BERT Embedding Service for ProximaDB
Generates 384, 768, or 1024 dimensional embeddings from text using sentence-transformers
"""

import sys
import os
import json
import time
import hashlib
import pickle
from pathlib import Path
from typing import List, Dict, Optional, Tuple
import numpy as np

try:
    from sentence_transformers import SentenceTransformer
    SENTENCE_TRANSFORMERS_AVAILABLE = True
except ImportError:
    SENTENCE_TRANSFORMERS_AVAILABLE = False
    print("⚠️ Warning: sentence-transformers not available. Using simulated embeddings.")

class BERTEmbeddingService:
    """Service for generating BERT embeddings from text"""
    
    def __init__(self, model_name: str = "all-MiniLM-L6-v2", cache_dir: Optional[str] = None):
        """
        Initialize BERT embedding service
        
        Args:
            model_name: HuggingFace model name
                - "all-MiniLM-L6-v2": 384 dimensions (fast, good quality)
                - "all-mpnet-base-v2": 768 dimensions (best quality)
                - "all-MiniLM-L12-v2": 384 dimensions (balanced)
            cache_dir: Directory to cache embeddings
        """
        self.model_name = model_name
        self.cache_dir = Path(cache_dir) if cache_dir else Path("./embedding_cache")
        self.cache_dir.mkdir(exist_ok=True)
        
        # Load model if available
        if SENTENCE_TRANSFORMERS_AVAILABLE:
            print(f"🤖 Loading BERT model: {model_name}")
            self.model = SentenceTransformer(model_name)
            self.dimension = self.model.get_sentence_embedding_dimension()
            print(f"✅ Model loaded with {self.dimension} dimensions")
        else:
            # Fallback dimensions for common models
            dimensions = {
                "all-MiniLM-L6-v2": 384,
                "all-mpnet-base-v2": 768,
                "all-MiniLM-L12-v2": 384,
                "all-distilroberta-v1": 768
            }
            self.dimension = dimensions.get(model_name, 384)
            self.model = None
            print(f"⚠️ Using simulated {self.dimension}D embeddings (install sentence-transformers for real BERT)")
    
    def get_cache_path(self, text: str) -> Path:
        """Get cache file path for text"""
        text_hash = hashlib.md5(text.encode('utf-8')).hexdigest()
        return self.cache_dir / f"{self.model_name}_{text_hash}.pkl"
    
    def embed_text(self, text: str, use_cache: bool = True) -> np.ndarray:
        """
        Generate embedding for single text
        
        Args:
            text: Input text to embed
            use_cache: Whether to use cached embeddings
            
        Returns:
            Embedding vector as numpy array
        """
        if use_cache:
            cache_path = self.get_cache_path(text)
            if cache_path.exists():
                with open(cache_path, 'rb') as f:
                    return pickle.load(f)
        
        if self.model:
            # Real BERT embedding
            embedding = self.model.encode(text, convert_to_numpy=True)
        else:
            # Simulated embedding based on text content
            embedding = self._simulate_embedding(text)
        
        # Cache the result
        if use_cache:
            cache_path = self.get_cache_path(text)
            with open(cache_path, 'wb') as f:
                pickle.dump(embedding, f)
        
        return embedding
    
    def embed_texts(self, texts: List[str], batch_size: int = 32, show_progress: bool = True) -> List[np.ndarray]:
        """
        Generate embeddings for multiple texts
        
        Args:
            texts: List of input texts
            batch_size: Batch size for processing
            show_progress: Whether to show progress
            
        Returns:
            List of embedding vectors
        """
        embeddings = []
        total = len(texts)
        
        for i in range(0, total, batch_size):
            batch = texts[i:i + batch_size]
            
            if self.model:
                # Real BERT embeddings
                batch_embeddings = self.model.encode(batch, convert_to_numpy=True, show_progress_bar=False)
            else:
                # Simulated embeddings
                batch_embeddings = [self._simulate_embedding(text) for text in batch]
            
            embeddings.extend(batch_embeddings)
            
            if show_progress:
                progress = (i + len(batch)) / total * 100
                print(f"\r🔄 Embedding progress: {progress:.1f}% ({i + len(batch)}/{total})", end="", flush=True)
        
        if show_progress:
            print()  # New line after progress
        
        return embeddings
    
    def _simulate_embedding(self, text: str) -> np.ndarray:
        """Generate simulated embedding based on text characteristics"""
        # Use text characteristics to create a deterministic but meaningful embedding
        words = text.lower().split()
        
        # Initialize with zeros
        embedding = np.zeros(self.dimension)
        
        # Add features based on text characteristics
        for i, word in enumerate(words[:min(len(words), self.dimension // 4)]):
            # Use word hash to distribute across dimensions
            word_hash = hash(word) % self.dimension
            embedding[word_hash] += 1.0 / (i + 1)  # Decay by position
        
        # Add text length features
        text_len_feature = min(len(text) / 1000.0, 1.0)  # Normalize to 0-1
        embedding[0] = text_len_feature
        
        # Add vocabulary richness
        vocab_richness = len(set(words)) / max(len(words), 1)
        embedding[1] = vocab_richness
        
        # Add character-level features
        for i, char in enumerate(text[:50]):  # First 50 characters
            if i < self.dimension - 10:
                embedding[i + 10] += ord(char) / 1000.0
        
        # Normalize to unit vector (important for cosine similarity)
        norm = np.linalg.norm(embedding)
        if norm > 0:
            embedding = embedding / norm
        
        return embedding.astype(np.float32)
    
    def similarity(self, embedding1: np.ndarray, embedding2: np.ndarray) -> float:
        """Calculate cosine similarity between two embeddings"""
        return float(np.dot(embedding1, embedding2) / (np.linalg.norm(embedding1) * np.linalg.norm(embedding2)))
    
    def find_similar(self, query_embedding: np.ndarray, corpus_embeddings: List[np.ndarray], 
                    top_k: int = 10) -> List[Tuple[int, float]]:
        """
        Find most similar embeddings to query
        
        Args:
            query_embedding: Query vector
            corpus_embeddings: List of corpus embeddings
            top_k: Number of results to return
            
        Returns:
            List of (index, similarity_score) tuples
        """
        similarities = []
        
        for i, corpus_emb in enumerate(corpus_embeddings):
            sim = self.similarity(query_embedding, corpus_emb)
            similarities.append((i, sim))
        
        # Sort by similarity (descending)
        similarities.sort(key=lambda x: x[1], reverse=True)
        
        return similarities[:top_k]

def create_sample_corpus(size_mb: float = 10.0) -> List[Dict[str, str]]:
    """
    Create a sample text corpus of approximately the specified size
    
    Args:
        size_mb: Target size in megabytes
        
    Returns:
        List of documents with text and metadata
    """
    print(f"📚 Creating {size_mb}MB sample corpus...")
    
    # Sample texts (these will be repeated and varied to reach target size)
    base_texts = [
        "Machine learning is a subset of artificial intelligence that focuses on algorithms that can learn from data.",
        "Deep learning neural networks have revolutionized computer vision and natural language processing.",
        "Vector databases are specialized databases designed to store and query high-dimensional vectors efficiently.",
        "BERT (Bidirectional Encoder Representations from Transformers) has transformed natural language understanding.",
        "Similarity search in high-dimensional spaces is a fundamental problem in information retrieval.",
        "Embeddings capture semantic meaning of text in dense vector representations.",
        "Cosine similarity is a common metric for measuring similarity between vectors.",
        "Retrieval-augmented generation combines information retrieval with language generation.",
        "Transformer architectures have become the dominant paradigm in modern NLP.",
        "Few-shot learning allows models to adapt to new tasks with minimal training data.",
        "Self-attention mechanisms enable models to focus on relevant parts of the input.",
        "Pre-trained language models can be fine-tuned for specific downstream tasks.",
        "Vector search enables semantic similarity rather than keyword matching.",
        "Dimensionality reduction techniques help visualize high-dimensional data.",
        "Attention mechanisms allow models to dynamically focus on important information.",
        "Transfer learning leverages knowledge from pre-trained models for new tasks.",
        "Gradient descent is the fundamental optimization algorithm for training neural networks.",
        "Regularization techniques prevent overfitting in machine learning models.",
        "Cross-validation helps estimate model performance on unseen data.",
        "Feature engineering involves creating meaningful inputs for machine learning algorithms."
    ]
    
    categories = ["AI", "ML", "NLP", "Database", "Vector Search", "Deep Learning", "Research"]
    authors = ["Dr. Smith", "Prof. Johnson", "Alice Chen", "Bob Wilson", "Carol Davis", "David Brown"]
    
    corpus = []
    total_size = 0
    target_size = size_mb * 1024 * 1024  # Convert to bytes
    doc_id = 0
    
    while total_size < target_size:
        # Create variations of base texts
        base_text = base_texts[doc_id % len(base_texts)]
        
        # Add variations to make text unique
        variations = [
            f"Introduction: {base_text}",
            f"Research shows that {base_text.lower()}",
            f"Recent advances in {base_text.lower()}",
            f"The field of study reveals that {base_text.lower()}",
            f"Comprehensive analysis indicates {base_text.lower()}",
            f"Experimental results suggest {base_text.lower()}",
            f"Contemporary research in {base_text.lower()}",
            f"Advanced techniques show {base_text.lower()}"
        ]
        
        variation = variations[doc_id % len(variations)]
        
        # Add some padding text to reach target size
        padding_words = ["Furthermore", "Moreover", "Additionally", "In conclusion", "Subsequently", 
                        "Consequently", "Nevertheless", "Therefore", "However", "Meanwhile"]
        padding = " ".join(padding_words[:(doc_id % 5) + 1])
        
        full_text = f"{variation} {padding}. " * ((doc_id % 3) + 1)
        
        doc = {
            "id": f"doc_{doc_id:06d}",
            "text": full_text,
            "category": categories[doc_id % len(categories)],
            "author": authors[doc_id % len(authors)],
            "length": len(full_text),
            "doc_type": "research_paper" if doc_id % 3 == 0 else "article",
            "year": 2020 + (doc_id % 4),
            "keywords": f"ai,ml,{categories[doc_id % len(categories)].lower().replace(' ', '_')}"
        }
        
        corpus.append(doc)
        total_size += len(json.dumps(doc).encode('utf-8'))
        doc_id += 1
        
        if doc_id % 1000 == 0:
            print(f"Generated {doc_id} documents, {total_size / (1024*1024):.1f}MB", end="\r")
    
    actual_size = total_size / (1024 * 1024)
    print(f"\n✅ Created corpus: {len(corpus)} documents, {actual_size:.1f}MB")
    
    return corpus

if __name__ == "__main__":
    # Demo the embedding service
    service = BERTEmbeddingService("all-MiniLM-L6-v2")
    
    # Test embedding
    sample_text = "This is a test document about machine learning and AI."
    embedding = service.embed_text(sample_text)
    print(f"Sample embedding shape: {embedding.shape}")
    print(f"Sample embedding (first 10 dims): {embedding[:10]}")
    
    # Create sample corpus
    corpus = create_sample_corpus(1.0)  # 1MB for demo
    print(f"Sample document: {corpus[0]}")