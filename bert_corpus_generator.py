#!/usr/bin/env python3
"""
BERT-based 10K Vector Corpus Generator
Creates realistic embeddings with metadata for comprehensive benchmarking
"""

import json
import numpy as np
import time
import random
from pathlib import Path
from typing import List, Dict, Any
import hashlib

class BERTCorpusGenerator:
    """Generate realistic BERT-like embeddings with rich metadata"""
    
    def __init__(self, dimension: int = 768, corpus_size: int = 10000):
        self.dimension = dimension  # BERT-base dimension
        self.corpus_size = corpus_size
        self.categories = [
            "technology", "science", "business", "health", "education", 
            "arts", "sports", "politics", "environment", "culture"
        ]
        self.languages = ["en", "es", "fr", "de", "it", "pt", "zh", "ja", "ko", "ru"]
        self.sources = [
            "wikipedia", "news", "academic", "blog", "documentation", 
            "review", "forum", "social", "book", "paper"
        ]
        
    def generate_realistic_bert_embedding(self, seed: int) -> List[float]:
        """Generate BERT-like embedding with realistic distribution"""
        np.random.seed(seed)
        
        # BERT embeddings typically have:
        # - Mean around 0, but with some bias
        # - Standard deviation around 0.1-0.3
        # - Some correlation between dimensions
        
        # Base embedding with realistic BERT characteristics
        embedding = np.random.normal(0.0, 0.15, self.dimension)
        
        # Add some structure (simulating semantic clusters)
        cluster_id = seed % 50  # 50 semantic clusters
        cluster_center = np.random.RandomState(cluster_id).normal(0, 0.05, self.dimension)
        embedding += cluster_center * 0.3
        
        # Normalize to unit vector (common in embeddings)
        norm = np.linalg.norm(embedding)
        if norm > 0:
            embedding = embedding / norm
        
        return embedding.tolist()
    
    def generate_metadata(self, index: int) -> Dict[str, Any]:
        """Generate rich, realistic metadata"""
        # Use index as seed for reproducible metadata
        random.seed(index)
        
        # Realistic content simulation
        categories = random.choices(self.categories, k=random.randint(1, 3))
        
        metadata = {
            "id": f"doc_{index:06d}",
            "title": f"Document {index}: {' '.join(categories).title()}",
            "category": categories[0],  # Primary category
            "tags": categories,  # All categories as tags
            "language": random.choice(self.languages),
            "source": random.choice(self.sources),
            "word_count": random.randint(50, 2000),
            "author": f"author_{random.randint(1, 1000)}",
            "publication_year": random.randint(2015, 2024),
            "quality_score": round(random.uniform(0.1, 1.0), 2),
            "view_count": random.randint(1, 100000),
            "sentiment": random.choice(["positive", "negative", "neutral"]),
            "is_verified": random.choice([True, False]),
            "region": random.choice(["us", "eu", "asia", "global"]),
            "content_type": random.choice(["article", "post", "paper", "comment", "review"]),
        }
        
        # Add synthetic text snippet for realism
        content_words = [
            "artificial", "intelligence", "machine", "learning", "data", "science",
            "technology", "innovation", "research", "development", "analysis", 
            "system", "algorithm", "model", "performance", "optimization"
        ]
        
        snippet_length = random.randint(10, 30)
        snippet = " ".join(random.choices(content_words, k=snippet_length))
        metadata["snippet"] = snippet
        
        return metadata
    
    def generate_corpus(self, output_file: str = "bert_10k_corpus.json") -> List[Dict]:
        """Generate the complete 10K vector corpus"""
        print(f"🧠 Generating {self.corpus_size} BERT embeddings (dimension: {self.dimension})")
        print("=" * 60)
        
        corpus = []
        start_time = time.time()
        
        for i in range(self.corpus_size):
            if i % 1000 == 0:
                elapsed = time.time() - start_time
                rate = i / elapsed if elapsed > 0 else 0
                print(f"📊 Progress: {i:,}/{self.corpus_size:,} ({i/self.corpus_size*100:.1f}%) - {rate:.0f} vectors/sec")
            
            # Generate embedding and metadata
            embedding = self.generate_realistic_bert_embedding(i)
            metadata = self.generate_metadata(i)
            
            # Create vector record
            vector_record = {
                "id": f"vec_{i:06d}",
                "vector": embedding,
                "metadata": metadata
            }
            
            corpus.append(vector_record)
        
        # Save corpus to file
        print(f"\n💾 Saving corpus to {output_file}...")
        with open(output_file, 'w') as f:
            json.dump(corpus, f, indent=2)
        
        # Generate statistics
        total_time = time.time() - start_time
        file_size = Path(output_file).stat().st_size / (1024 * 1024)  # MB
        
        print(f"\n✅ Corpus Generation Complete!")
        print(f"📊 Statistics:")
        print(f"   • Total vectors: {len(corpus):,}")
        print(f"   • Vector dimension: {self.dimension}")
        print(f"   • Generation time: {total_time:.2f}s")
        print(f"   • Generation rate: {len(corpus)/total_time:.0f} vectors/sec")
        print(f"   • File size: {file_size:.1f} MB")
        print(f"   • Categories: {len(self.categories)}")
        print(f"   • Languages: {len(self.languages)}")
        print(f"   • Sources: {len(self.sources)}")
        
        return corpus
    
    def analyze_corpus_statistics(self, corpus: List[Dict]) -> Dict:
        """Analyze corpus for distribution statistics"""
        print(f"\n📊 CORPUS ANALYSIS")
        print("=" * 20)
        
        # Metadata distribution analysis
        categories = {}
        languages = {}
        sources = {}
        years = {}
        
        for record in corpus:
            meta = record["metadata"]
            
            # Count distributions
            cat = meta["category"]
            categories[cat] = categories.get(cat, 0) + 1
            
            lang = meta["language"]
            languages[lang] = languages.get(lang, 0) + 1
            
            source = meta["source"]
            sources[source] = sources.get(source, 0) + 1
            
            year = meta["publication_year"]
            years[year] = years.get(year, 0) + 1
        
        # Vector statistics
        vectors = [record["vector"] for record in corpus]
        vector_array = np.array(vectors)
        
        stats = {
            "corpus_size": len(corpus),
            "vector_dimension": len(vectors[0]),
            "vector_stats": {
                "mean": float(np.mean(vector_array)),
                "std": float(np.std(vector_array)),
                "min": float(np.min(vector_array)),
                "max": float(np.max(vector_array))
            },
            "metadata_distributions": {
                "categories": dict(sorted(categories.items(), key=lambda x: x[1], reverse=True)),
                "languages": dict(sorted(languages.items(), key=lambda x: x[1], reverse=True)),
                "sources": dict(sorted(sources.items(), key=lambda x: x[1], reverse=True)),
                "years": dict(sorted(years.items()))
            }
        }
        
        # Print analysis
        print(f"📈 Vector Statistics:")
        print(f"   • Mean: {stats['vector_stats']['mean']:.6f}")
        print(f"   • Std Dev: {stats['vector_stats']['std']:.6f}")
        print(f"   • Range: [{stats['vector_stats']['min']:.6f}, {stats['vector_stats']['max']:.6f}]")
        
        print(f"\n📋 Top Categories:")
        for cat, count in list(stats['metadata_distributions']['categories'].items())[:5]:
            print(f"   • {cat}: {count:,} ({count/len(corpus)*100:.1f}%)")
        
        print(f"\n🌐 Language Distribution:")
        for lang, count in list(stats['metadata_distributions']['languages'].items())[:5]:
            print(f"   • {lang}: {count:,} ({count/len(corpus)*100:.1f}%)")
        
        return stats

def create_query_vectors(corpus: List[Dict], num_queries: int = 20) -> List[Dict]:
    """Create realistic query vectors from corpus"""
    print(f"\n🔍 Creating {num_queries} query vectors from corpus")
    
    # Select diverse queries
    query_indices = []
    categories_used = set()
    
    # Ensure coverage of different categories
    for i, record in enumerate(corpus):
        category = record["metadata"]["category"]
        if category not in categories_used and len(query_indices) < num_queries:
            query_indices.append(i)
            categories_used.add(category)
    
    # Fill remaining with random selection
    while len(query_indices) < num_queries:
        idx = random.randint(0, len(corpus) - 1)
        if idx not in query_indices:
            query_indices.append(idx)
    
    queries = []
    for i, idx in enumerate(query_indices):
        source_record = corpus[idx]
        query = {
            "query_id": f"query_{i:02d}",
            "vector": source_record["vector"],
            "expected_match_id": source_record["id"],
            "category": source_record["metadata"]["category"],
            "language": source_record["metadata"]["language"],
            "description": f"Query for {source_record['metadata']['category']} content"
        }
        queries.append(query)
    
    print(f"✅ Created {len(queries)} diverse query vectors")
    return queries

def main():
    print("🧠 BERT 10K Vector Corpus Generator")
    print("🎯 Creating realistic embeddings for comprehensive benchmarking")
    print("=" * 70)
    
    # Generate corpus
    generator = BERTCorpusGenerator(dimension=768, corpus_size=10000)
    corpus = generator.generate_corpus("bert_10k_corpus.json")
    
    # Analyze corpus
    stats = generator.analyze_corpus_statistics(corpus)
    
    # Create query vectors
    queries = create_query_vectors(corpus, num_queries=20)
    
    # Save queries
    with open("bert_queries.json", 'w') as f:
        json.dump(queries, f, indent=2)
    
    # Save statistics
    with open("corpus_stats.json", 'w') as f:
        json.dump(stats, f, indent=2)
    
    print(f"\n🎉 BERT Corpus Generation Complete!")
    print(f"📁 Files created:")
    print(f"   • bert_10k_corpus.json - Main corpus (10K vectors)")
    print(f"   • bert_queries.json - Query vectors (20 queries)")
    print(f"   • corpus_stats.json - Statistical analysis")
    
    print(f"\n🚀 Ready for ProximaDB benchmarking!")

if __name__ == "__main__":
    main()