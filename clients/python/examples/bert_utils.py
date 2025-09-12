#!/usr/bin/env python3
"""
BERT Embedding Utilities for ProximaDB Python SDK Examples

This module provides real BERT embeddings for examples and demonstrations,
replacing random/simulated vectors with semantically meaningful embeddings.
"""

from typing import List, Dict, Optional
import logging

# Import sentence-transformers at top level to fail fast if not available
from sentence_transformers import SentenceTransformer

logger = logging.getLogger(__name__)

# Global model instance for reuse
_bert_model: Optional[SentenceTransformer] = None

def get_bert_model(model_name: str = 'all-MiniLM-L6-v2') -> SentenceTransformer:
    """Get or create BERT model instance (384 dimensions)"""
    global _bert_model
    if _bert_model is None:
        logger.info(f"Loading BERT model: {model_name}")
        _bert_model = SentenceTransformer(model_name)
        logger.info(f"BERT model loaded - dimension: {_bert_model.get_sentence_embedding_dimension()}")
    return _bert_model

def generate_text_embeddings(texts: List[str], model_name: str = 'all-MiniLM-L6-v2') -> List[List[float]]:
    """Generate real BERT embeddings from text strings"""
    if not texts:
        return []
    
    model = get_bert_model(model_name)
    embeddings = model.encode(texts, convert_to_tensor=False).tolist()
    logger.info(f"Generated {len(embeddings)} BERT embeddings from text")
    return embeddings

def generate_sample_documents(num_docs: int = 10) -> List[Dict[str, any]]:
    """Generate sample documents with realistic content for vector database demos"""
    
    sample_texts = [
        "Machine learning algorithms enable computers to learn patterns from data automatically.",
        "Vector databases store high-dimensional embeddings for efficient similarity search operations.",
        "Natural language processing transforms human language into computer-readable representations.",
        "Deep learning neural networks can extract meaningful features from complex data structures.",
        "Artificial intelligence systems demonstrate human-like reasoning and decision-making capabilities.",
        "Data science combines statistics, programming, and domain expertise to extract insights.",
        "Computer vision algorithms process and analyze visual information from images and videos.",
        "Information retrieval systems help users find relevant content from large document collections.",
        "Semantic search understands query intent rather than just matching keywords exactly.",
        "Knowledge graphs represent relationships between entities in structured formats.",
        "Recommendation systems suggest relevant items based on user preferences and behavior patterns.",
        "Text classification algorithms categorize documents into predefined topic categories automatically.",
        "Sentiment analysis determines emotional tone and opinion from written text content.",
        "Named entity recognition identifies people, places, and organizations in text documents.",
        "Question answering systems provide direct responses to user queries from knowledge bases.",
        "Document clustering groups similar texts together without predefined category labels.",
        "Feature extraction transforms raw data into meaningful numerical representations for analysis.",
        "Dimensionality reduction techniques compress high-dimensional data while preserving important information.",
        "Unsupervised learning discovers hidden patterns in data without labeled training examples.",
        "Transfer learning applies knowledge from one domain to solve problems in related domains."
    ]
    
    # Select texts based on requested number
    selected_texts = sample_texts[:num_docs] if num_docs <= len(sample_texts) else sample_texts * ((num_docs // len(sample_texts)) + 1)
    selected_texts = selected_texts[:num_docs]
    
    # Generate embeddings
    embeddings = generate_text_embeddings(selected_texts)
    
    # Create documents with metadata
    documents = []
    for i, (text, embedding) in enumerate(zip(selected_texts, embeddings)):
        doc = {
            'id': f'doc_{i:03d}',
            'text': text,
            'embedding': embedding,
            'metadata': {
                'category': 'ai_ml' if any(term in text.lower() for term in ['machine learning', 'neural', 'deep learning']) else 'data_science',
                'word_count': len(text.split()),
                'document_type': 'article',
                'indexed_at': f'2025-01-{(i % 30) + 1:02d}T10:00:00Z'
            }
        }
        documents.append(doc)
    
    return documents

def generate_sample_products(num_products: int = 10) -> List[Dict[str, any]]:
    """Generate sample e-commerce products with BERT embeddings"""
    
    product_descriptions = [
        "High-performance gaming laptop with RTX 4080 graphics card and Intel i9 processor",
        "Wireless noise-canceling headphones with premium sound quality and long battery life",
        "Professional DSLR camera with 24-megapixel sensor and multiple lens compatibility",
        "Ergonomic office chair with lumbar support and adjustable height mechanism",
        "Smart fitness tracker with heart rate monitoring and GPS navigation features",
        "Stainless steel coffee maker with programmable brewing and thermal carafe",
        "Mechanical keyboard with RGB backlighting and tactile switches for gaming",
        "4K ultra-wide monitor with HDR support and USB-C connectivity hub",
        "Portable bluetooth speaker with waterproof design and 360-degree sound",
        "Smart home security camera with motion detection and night vision capabilities",
        "Premium mattress with memory foam layers and temperature regulation technology",
        "Electric standing desk with height adjustment and built-in wireless charging",
        "High-capacity power bank with fast charging and multiple device support",
        "Professional drone with 4K camera stabilization and intelligent flight modes",
        "Smart thermostat with learning algorithms and energy efficiency optimization"
    ]
    
    # Select descriptions based on requested number
    selected_descriptions = product_descriptions[:num_products] if num_products <= len(product_descriptions) else product_descriptions * ((num_products // len(product_descriptions)) + 1)
    selected_descriptions = selected_descriptions[:num_products]
    
    # Generate embeddings
    embeddings = generate_text_embeddings(selected_descriptions)
    
    # Create products with metadata
    products = []
    for i, (description, embedding) in enumerate(zip(selected_descriptions, embeddings)):
        product = {
            'id': f'prod_{i:03d}',
            'description': description,
            'embedding': embedding,
            'metadata': {
                'brand': ['TechCorp', 'EliteGear', 'ProDevice', 'SmartTech', 'PremiumBrand'][i % 5],
                'price': round(99.99 + (i * 50.0) + (i * i * 10.0), 2),
                'category': 'electronics',
                'in_stock': i % 3 != 0,  # Most products in stock
                'rating': round(3.5 + (i % 3) * 0.5, 1),
                'launch_date': f'2024-{(i % 12) + 1:02d}-15'
            }
        }
        products.append(product)
    
    return products

def generate_query_embedding(query_text: str, model_name: str = 'all-MiniLM-L6-v2') -> List[float]:
    """Generate BERT embedding for a single query string"""
    return generate_text_embeddings([query_text], model_name)[0]

def get_sample_queries() -> List[str]:
    """Get sample search queries for demonstrations"""
    return [
        "machine learning algorithms for data analysis",
        "computer vision and image processing",
        "natural language understanding systems",
        "vector database similarity search",
        "artificial intelligence applications",
        "data science and analytics tools",
        "deep learning neural networks",
        "recommendation systems design"
    ]

def get_sample_product_queries() -> List[str]:
    """Get sample product search queries"""
    return [
        "gaming laptop with high performance graphics",
        "wireless headphones with noise canceling",
        "professional camera for photography",
        "ergonomic office furniture setup",
        "smart fitness and health tracking",
        "kitchen appliances for coffee brewing",
        "mechanical keyboard for gaming setup",
        "4K monitor with wide screen display"
    ]

# Cache for repeated use in examples
_sample_documents_cache: Optional[List[Dict]] = None
_sample_products_cache: Optional[List[Dict]] = None

def get_cached_sample_documents(num_docs: int = 10) -> List[Dict[str, any]]:
    """Get cached sample documents (generates once, reuses for performance)"""
    global _sample_documents_cache
    if _sample_documents_cache is None or len(_sample_documents_cache) < num_docs:
        _sample_documents_cache = generate_sample_documents(max(num_docs, 20))
    return _sample_documents_cache[:num_docs]

def get_cached_sample_products(num_products: int = 10) -> List[Dict[str, any]]:
    """Get cached sample products (generates once, reuses for performance)"""  
    global _sample_products_cache
    if _sample_products_cache is None or len(_sample_products_cache) < num_products:
        _sample_products_cache = generate_sample_products(max(num_products, 15))
    return _sample_products_cache[:num_products]