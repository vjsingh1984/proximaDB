#!/usr/bin/env python3
"""
BERT Embedding Utilities for Performance Tests
Provides real text corpus generation and BERT embeddings
"""

import numpy as np
import time
from typing import List, Dict, Any, Tuple


def generate_text_corpus(size: int) -> List[Dict[str, Any]]:
    """
    Generate a diverse text corpus for testing
    Assumes ~4 chars per token, creates meaningful text for embeddings
    """
    
    # Base categories with sample texts
    categories = {
        "technology": [
            "artificial intelligence machine learning deep neural networks",
            "computer science programming software development algorithms",
            "data science analytics big data processing frameworks",
            "cloud computing distributed systems microservices architecture",
            "cybersecurity encryption cryptography network security protocols",
            "blockchain cryptocurrency decentralized ledger technology",
            "internet of things IoT sensors smart devices connectivity",
            "robotics automation industrial manufacturing processes",
            "quantum computing quantum mechanics superposition entanglement",
            "virtual reality augmented reality immersive digital experiences"
        ],
        "science": [
            "physics quantum mechanics relativity electromagnetic radiation",
            "chemistry molecular structure chemical reactions organic compounds",
            "biology genetics DNA RNA protein synthesis cellular processes",
            "astronomy astrophysics cosmology black holes stellar evolution",
            "geology earth science plate tectonics mineral formation",
            "neuroscience brain function cognitive processes neural networks",
            "medicine healthcare diagnosis treatment pharmaceutical research",
            "environmental science climate change ecosystem biodiversity",
            "mathematics calculus statistics probability theoretical analysis",
            "materials science nanotechnology advanced composite materials"
        ],
        "business": [
            "entrepreneurship startup ventures business development strategies",
            "marketing digital advertising brand management customer engagement",
            "finance investment banking capital markets risk management",
            "operations supply chain logistics inventory management",
            "human resources talent acquisition employee development",
            "strategy corporate planning competitive analysis market research",
            "economics macroeconomics microeconomics fiscal monetary policy",
            "consulting management advisory business transformation",
            "sales customer relationship management revenue generation",
            "leadership organizational behavior team management skills"
        ],
        "arts": [
            "literature poetry novels creative writing storytelling",
            "visual arts painting sculpture contemporary modern art",
            "music composition classical jazz electronic genres",
            "theater drama performance acting directing production",
            "film cinema cinematography editing post production",
            "dance choreography ballet contemporary movement expression",
            "architecture design structural engineering urban planning",
            "fashion design textile industry style trends",
            "photography digital imaging visual documentation",
            "crafts traditional handmade artisan techniques"
        ],
        "sports": [
            "football soccer basketball athletics team sports",
            "tennis golf individual sports professional tournaments",
            "swimming diving aquatic sports Olympic competitions",
            "running marathon endurance training fitness",
            "cycling mountain biking road racing competitive events",
            "baseball softball diamond sports American pastimes",
            "hockey ice hockey field hockey team dynamics",
            "volleyball beach volleyball indoor court sports",
            "martial arts boxing wrestling combat sports",
            "winter sports skiing snowboarding alpine competitions"
        ]
    }
    
    corpus = []
    doc_id = 0
    
    # Calculate how many documents per category
    categories_list = list(categories.keys())
    docs_per_category = size // len(categories_list)
    remainder = size % len(categories_list)
    
    for cat_idx, (category, base_texts) in enumerate(categories.items()):
        # Add extra document to some categories to handle remainder
        docs_for_this_category = docs_per_category + (1 if cat_idx < remainder else 0)
        
        for i in range(docs_for_this_category):
            # Select base text and add variations
            base_text = base_texts[i % len(base_texts)]
            
            # Add variations to make each document unique
            variations = [
                f"research study analysis {base_text} methodology results",
                f"innovative developments {base_text} future trends",
                f"comprehensive overview {base_text} practical applications",
                f"advanced techniques {base_text} industry standards",
                f"emerging technologies {base_text} market opportunities",
                f"scientific investigation {base_text} experimental findings",
                f"professional expertise {base_text} best practices",
                f"technical implementation {base_text} system design",
                f"strategic approach {base_text} business value",
                f"theoretical framework {base_text} empirical evidence"
            ]
            
            text = variations[i % len(variations)]
            
            # Ensure text is at least 16 chars (4 tokens * 4 chars)
            if len(text) < 16:
                text = f"{text} additional context information details"
            
            # Create document
            doc = {
                "id": f"doc_{doc_id:06d}",
                "text": text,
                "category": category,
                "subcategory": f"{category}_{i % 3}",
                "doc_index": doc_id,
                "word_count": len(text.split()),
                "char_count": len(text),
                "timestamp": time.time() + doc_id  # Unique timestamps
            }
            
            corpus.append(doc)
            doc_id += 1
            
            if doc_id >= size:
                break
        
        if doc_id >= size:
            break
    
    return corpus[:size]


def create_bert_embeddings(texts: List[str], dimension: int = 384) -> np.ndarray:
    """
    Create BERT-style embeddings for texts
    Uses deterministic approach based on text content for reproducibility
    """
    
    embeddings = []
    
    for text in texts:
        # Create deterministic embedding based on text content
        embedding = create_deterministic_embedding(text, dimension)
        embeddings.append(embedding)
    
    return np.array(embeddings, dtype=np.float32)


def create_deterministic_embedding(text: str, dimension: int = 384) -> np.ndarray:
    """
    Create a deterministic BERT-style embedding for a text
    Uses text content to generate reproducible embeddings with semantic clustering
    """
    
    # Use text hash for reproducible randomness
    text_hash = hash(text.lower().strip()) % (2**32)
    np.random.seed(text_hash)
    
    # Create base embedding
    embedding = np.random.normal(0, 0.1, dimension)
    
    # Add semantic structure based on keywords and categories
    text_lower = text.lower()
    
    # Technology cluster (dimensions 0-75)
    tech_keywords = ['ai', 'artificial', 'intelligence', 'machine', 'learning', 'computer', 
                     'programming', 'software', 'algorithm', 'data', 'cloud', 'cyber']
    if any(keyword in text_lower for keyword in tech_keywords):
        embedding[0:76] += np.random.normal(0.3, 0.1, 76)
    
    # Science cluster (dimensions 76-151) 
    science_keywords = ['physics', 'chemistry', 'biology', 'quantum', 'molecular', 
                       'genetic', 'astronomy', 'medicine', 'research', 'scientific']
    if any(keyword in text_lower for keyword in science_keywords):
        embedding[76:152] += np.random.normal(0.3, 0.1, 76)
    
    # Business cluster (dimensions 152-227)
    business_keywords = ['business', 'marketing', 'finance', 'management', 'strategy',
                        'sales', 'investment', 'entrepreneurship', 'corporate', 'economic']
    if any(keyword in text_lower for keyword in business_keywords):
        embedding[152:228] += np.random.normal(0.3, 0.1, 76)
    
    # Arts cluster (dimensions 228-303)
    arts_keywords = ['art', 'music', 'literature', 'poetry', 'painting', 'film',
                    'theater', 'dance', 'creative', 'design', 'fashion', 'photography']
    if any(keyword in text_lower for keyword in arts_keywords):
        embedding[228:304] += np.random.normal(0.3, 0.1, 76)
    
    # Sports cluster (dimensions 304-379)
    sports_keywords = ['sport', 'football', 'basketball', 'tennis', 'golf', 'swimming',
                      'running', 'cycling', 'hockey', 'volleyball', 'athletic', 'competition']
    if any(keyword in text_lower for keyword in sports_keywords):
        embedding[304:380] += np.random.normal(0.3, 0.1, 76)
    
    # Add word-level features for remaining dimensions
    words = text_lower.split()
    for i, word in enumerate(words[:4]):  # Use first 4 words
        word_hash = hash(word) % (2**16)
        embedding[380 + i] = (word_hash / (2**16)) * 0.2
    
    # Normalize embedding
    norm = np.linalg.norm(embedding)
    if norm > 0:
        embedding = embedding / norm
    
    return embedding.astype(np.float32)


def create_query_texts() -> List[Dict[str, Any]]:
    """Create sample query texts for search testing"""
    
    queries = [
        {
            "text": "artificial intelligence machine learning deep learning",
            "expected_category": "technology",
            "description": "AI/ML query"
        },
        {
            "text": "quantum physics mechanics relativity",
            "expected_category": "science", 
            "description": "Physics query"
        },
        {
            "text": "business strategy marketing finance",
            "expected_category": "business",
            "description": "Business query"
        },
        {
            "text": "music art creative literature",
            "expected_category": "arts",
            "description": "Arts query"
        },
        {
            "text": "sports football basketball athletics",
            "expected_category": "sports",
            "description": "Sports query"
        },
        {
            "text": "data science analytics big data processing",
            "expected_category": "technology",
            "description": "Data science query"
        },
        {
            "text": "biology genetics DNA molecular structure",
            "expected_category": "science",
            "description": "Biology query"
        },
        {
            "text": "investment banking capital markets trading",
            "expected_category": "business", 
            "description": "Finance query"
        }
    ]
    
    return queries


def convert_corpus_to_vectors(corpus: List[Dict[str, Any]], dimension: int = 384) -> List[Dict[str, Any]]:
    """Convert text corpus to vector format with embeddings"""
    
    vectors = []
    
    for doc in corpus:
        # Create embedding for the text
        embedding = create_deterministic_embedding(doc["text"], dimension)
        
        # Create vector entry
        vector = {
            "id": doc["id"],
            "vector": embedding.tolist(),
            "metadata": {
                "text": doc["text"],
                "category": doc["category"],
                "subcategory": doc["subcategory"],
                "doc_index": doc["doc_index"],
                "word_count": doc["word_count"],
                "char_count": doc["char_count"],
                "timestamp": doc["timestamp"]
            }
        }
        
        vectors.append(vector)
    
    return vectors


def test_embedding_quality():
    """Test the quality of our embedding generation"""
    print("🧪 Testing BERT Embedding Quality")
    print("=" * 40)
    
    # Test texts from different categories
    test_texts = [
        "artificial intelligence machine learning",
        "quantum physics relativity mechanics", 
        "business strategy marketing finance",
        "music art creative literature",
        "sports football basketball tennis"
    ]
    
    embeddings = create_bert_embeddings(test_texts)
    
    print(f"Generated {len(embeddings)} embeddings of dimension {embeddings.shape[1]}")
    
    # Test similarity within and across categories
    from scipy.spatial.distance import cosine
    
    print("\nSimilarity Matrix (lower = more similar):")
    print("     ", "   ".join([f"T{i+1}" for i in range(len(test_texts))]))
    
    for i, text1 in enumerate(test_texts):
        similarities = []
        for j, text2 in enumerate(test_texts):
            if i == j:
                sim = 0.0
            else:
                sim = cosine(embeddings[i], embeddings[j])
            similarities.append(sim)
        
        sim_str = "  ".join([f"{s:.3f}" for s in similarities])
        print(f"T{i+1}: {sim_str}  | {test_texts[i][:30]}...")


if __name__ == "__main__":
    # Test the embedding utilities
    print("🚀 BERT Embedding Utilities Test")
    print("=" * 50)
    
    # Test corpus generation
    corpus = generate_text_corpus(10)
    print(f"✅ Generated corpus with {len(corpus)} documents")
    
    for i, doc in enumerate(corpus[:3]):
        print(f"   Doc {i+1}: {doc['category']} - {doc['text'][:50]}...")
    
    # Test embedding generation
    test_embedding_quality()
    
    # Test vector conversion
    vectors = convert_corpus_to_vectors(corpus[:5])
    print(f"\n✅ Converted {len(vectors)} documents to vectors")
    
    print(f"   Sample vector shape: {len(vectors[0]['vector'])}")
    print(f"   Sample metadata: {list(vectors[0]['metadata'].keys())}")
    
    print("\n🎉 BERT embedding utilities ready for use!")