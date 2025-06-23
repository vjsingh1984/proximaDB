#!/usr/bin/env python3
"""
Advanced Hybrid Search with Query Planner and Cost Calculator
Demonstrates similarity search + metadata filtering on 10MB BERT corpus
Includes query optimization and execution cost analysis
"""

import sys
import os
import json
import time
import asyncio
import numpy as np
from pathlib import Path
from typing import List, Dict, Any, Tuple, Optional
import uuid
from dataclasses import dataclass
from enum import Enum

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from bert_embedding_service import BERTEmbeddingService, create_sample_corpus

class SearchStrategy(Enum):
    """Available search strategies"""
    VECTOR_FIRST = "vector_first"       # Vector search → metadata filter
    METADATA_FIRST = "metadata_first"   # Metadata filter → vector search  
    PARALLEL_HYBRID = "parallel_hybrid" # Parallel execution + merge
    ADAPTIVE = "adaptive"               # Cost-based selection

@dataclass
class SearchCost:
    """Cost estimation for search operations"""
    vector_search_cost: float          # Cost for vector similarity search
    metadata_filter_cost: float        # Cost for metadata filtering
    merge_cost: float                   # Cost for result merging
    total_cost: float                   # Total estimated cost
    estimated_result_count: int         # Expected result count
    strategy: SearchStrategy            # Chosen strategy

@dataclass
class HybridSearchQuery:
    """Hybrid search query combining vector and metadata"""
    query_text: str                     # Text for similarity search
    metadata_filters: Dict[str, Any]    # Metadata filtering criteria
    top_k: int = 10                     # Number of results
    similarity_threshold: float = 0.0   # Minimum similarity score
    include_vectors: bool = False       # Include vector data in results

class QueryPlanner:
    """Query planner with cost-based optimization"""
    
    def __init__(self, collection_stats: Dict[str, Any]):
        self.stats = collection_stats
        self.vector_search_base_cost = 10.0      # ms per 1K vectors
        self.metadata_filter_base_cost = 2.0     # ms per 1K vectors
        self.merge_cost_per_result = 0.1         # ms per result merged
        
    def estimate_metadata_selectivity(self, filters: Dict[str, Any]) -> float:
        """Estimate what fraction of documents match metadata filters"""
        if not filters:
            return 1.0
            
        # Simulate metadata statistics (in real system, this would use actual stats)
        selectivity = 1.0
        for key, value in filters.items():
            if key == "category":
                # Assume 7 categories, uniform distribution
                selectivity *= 1.0 / 7.0
            elif key == "author":
                # Assume 6 authors, uniform distribution
                selectivity *= 1.0 / 6.0
            elif key == "doc_type":
                # Assume 2 doc types (research_paper, article)
                selectivity *= 0.6 if value == "research_paper" else 0.4
            elif key == "year":
                # Assume 4 years (2020-2024), uniform distribution
                selectivity *= 1.0 / 4.0
            else:
                # Default selectivity for unknown fields
                selectivity *= 0.5
                
        return max(selectivity, 0.001)  # Minimum 0.1% selectivity
    
    def estimate_vector_search_cost(self, collection_size: int, top_k: int) -> float:
        """Estimate cost of vector similarity search"""
        # HNSW search complexity is roughly O(log(n) * ef_construction)
        # For simplicity, we use linear approximation
        search_cost = (collection_size / 1000.0) * self.vector_search_base_cost
        return search_cost * (1.0 + top_k / 100.0)  # Slightly higher cost for larger k
    
    def estimate_metadata_filter_cost(self, collection_size: int, selectivity: float) -> float:
        """Estimate cost of metadata filtering"""
        # Linear scan through metadata
        scan_cost = (collection_size / 1000.0) * self.metadata_filter_base_cost
        return scan_cost
    
    def plan_hybrid_search(self, query: HybridSearchQuery) -> SearchCost:
        """Generate optimal execution plan for hybrid search"""
        collection_size = self.stats.get("total_vectors", 10000)
        
        # Estimate metadata selectivity
        metadata_selectivity = self.estimate_metadata_selectivity(query.metadata_filters)
        filtered_count = int(collection_size * metadata_selectivity)
        
        print(f"🧮 Query Planning Analysis:")
        print(f"   Collection size: {collection_size:,} vectors")
        print(f"   Metadata filters: {query.metadata_filters}")
        print(f"   Estimated selectivity: {metadata_selectivity:.3f} ({filtered_count:,} vectors)")
        
        # Strategy 1: Vector-first (vector search → metadata filter)
        vector_first_cost = (
            self.estimate_vector_search_cost(collection_size, query.top_k * 3) +  # Search more to account for filtering
            self.metadata_filter_base_cost +  # Filter vector results
            query.top_k * self.merge_cost_per_result
        )
        
        # Strategy 2: Metadata-first (metadata filter → vector search)
        metadata_first_cost = (
            self.estimate_metadata_filter_cost(collection_size, metadata_selectivity) +
            self.estimate_vector_search_cost(filtered_count, query.top_k) +
            query.top_k * self.merge_cost_per_result
        )
        
        # Strategy 3: Parallel hybrid (parallel execution + merge)
        parallel_cost = max(
            self.estimate_vector_search_cost(collection_size, query.top_k * 2),
            self.estimate_metadata_filter_cost(collection_size, metadata_selectivity)
        ) + query.top_k * self.merge_cost_per_result * 2  # Higher merge cost
        
        # Choose optimal strategy based on cost
        costs = {
            SearchStrategy.VECTOR_FIRST: vector_first_cost,
            SearchStrategy.METADATA_FIRST: metadata_first_cost,
            SearchStrategy.PARALLEL_HYBRID: parallel_cost
        }
        
        optimal_strategy = min(costs.keys(), key=lambda s: costs[s])
        optimal_cost = costs[optimal_strategy]
        
        print(f"   Cost Analysis:")
        print(f"     Vector-first: {vector_first_cost:.2f}ms")
        print(f"     Metadata-first: {metadata_first_cost:.2f}ms") 
        print(f"     Parallel hybrid: {parallel_cost:.2f}ms")
        print(f"   ✅ Optimal strategy: {optimal_strategy.value} ({optimal_cost:.2f}ms)")
        
        return SearchCost(
            vector_search_cost=self.estimate_vector_search_cost(collection_size, query.top_k),
            metadata_filter_cost=self.estimate_metadata_filter_cost(collection_size, metadata_selectivity),
            merge_cost=query.top_k * self.merge_cost_per_result,
            total_cost=optimal_cost,
            estimated_result_count=min(query.top_k, filtered_count),
            strategy=optimal_strategy
        )

class HybridSearchEngine:
    """Advanced hybrid search engine with query planning"""
    
    def __init__(self, proximadb_endpoint: str = "localhost:5679"):
        self.client = ProximaDBClient(endpoint=proximadb_endpoint)
        self.embedding_service = BERTEmbeddingService("all-MiniLM-L6-v2")
        self.collection_name = f"hybrid_search_{uuid.uuid4().hex[:8]}"
        self.corpus_data = []
        self.embeddings = []
        self.collection_stats = {}
        
    async def setup_collection_with_large_corpus(self, corpus_size_mb: float = 10.0):
        """Setup collection with large BERT-embedded corpus"""
        print("🚀 Advanced Hybrid Search with Query Planner")
        print("=" * 60)
        
        # Create collection
        print(f"🏗️ Creating collection: {self.collection_name}")
        collection = await self.client.create_collection(
            name=self.collection_name,
            dimension=384,
            distance_metric=1,  # Cosine similarity
            indexing_algorithm=1,  # HNSW
            storage_engine=1  # VIPER
        )
        
        # Check for cached corpus and embeddings
        cache_dir = Path("./embedding_cache")
        corpus_cache = cache_dir / f"corpus_{corpus_size_mb}mb.json"
        embeddings_cache = cache_dir / f"embeddings_{corpus_size_mb}mb.npy"
        
        if corpus_cache.exists() and embeddings_cache.exists():
            print(f"📦 Loading cached corpus and embeddings...")
            # Load cached corpus
            with open(corpus_cache, 'r') as f:
                self.corpus_data = json.load(f)
            
            # Load cached embeddings
            self.embeddings = np.load(embeddings_cache)
            print(f"✅ Loaded {len(self.corpus_data)} documents and {len(self.embeddings)} embeddings from cache")
        else:
            # Generate new corpus and embeddings
            print(f"📚 Generating {corpus_size_mb}MB corpus...")
            self.corpus_data = create_sample_corpus(corpus_size_mb)
            
            print("🤖 Creating BERT embeddings...")
            start_time = time.time()
            texts = [doc["text"] for doc in self.corpus_data]
            self.embeddings = self.embedding_service.embed_texts(texts, batch_size=64, show_progress=True)
            embedding_time = time.time() - start_time
            
            print(f"✅ Generated {len(self.embeddings)} embeddings in {embedding_time:.1f}s")
            print(f"   Avg: {embedding_time/len(self.embeddings)*1000:.1f}ms per document")
            
            # Cache for future use
            cache_dir.mkdir(exist_ok=True)
            with open(corpus_cache, 'w') as f:
                json.dump(self.corpus_data, f)
            np.save(embeddings_cache, self.embeddings)
            print(f"💾 Cached corpus and embeddings for future use")
        
        # Insert vectors in batches
        print(f"\n📊 Inserting {len(self.corpus_data)} vectors...")
        await self.batch_insert_vectors()
        
        # Build collection statistics
        self.collection_stats = {
            "total_vectors": len(self.corpus_data),
            "corpus_size_mb": corpus_size_mb,
            "embedding_dimension": 384,
            "categories": len(set(doc["category"] for doc in self.corpus_data)),
            "authors": len(set(doc["author"] for doc in self.corpus_data)),
            "doc_types": len(set(doc["doc_type"] for doc in self.corpus_data)),
            "years": len(set(doc["year"] for doc in self.corpus_data))
        }
        
        print(f"📈 Collection Statistics:")
        for key, value in self.collection_stats.items():
            print(f"   {key}: {value}")
            
    async def batch_insert_vectors(self):
        """Insert vectors in optimized batches"""
        batch_size = 200
        total_time = 0
        
        for i in range(0, len(self.corpus_data), batch_size):
            batch_docs = self.corpus_data[i:i + batch_size]
            batch_embeddings = self.embeddings[i:i + batch_size]
            
            vectors = []
            for doc, embedding in zip(batch_docs, batch_embeddings):
                vector_data = {
                    "id": doc["id"],
                    "vector": embedding.tolist(),
                    "metadata": {
                        "text": doc["text"][:300],  # Truncate for efficiency
                        "category": doc["category"],
                        "author": doc["author"],
                        "doc_type": doc["doc_type"],
                        "year": str(doc["year"]),
                        "keywords": doc["keywords"],
                        "length": str(doc["length"])
                    }
                }
                vectors.append(vector_data)
            
            start = time.time()
            result = self.client.insert_vectors(
                collection_id=self.collection_name,
                vectors=vectors
            )
            batch_time = time.time() - start
            total_time += batch_time
            
            progress = (i + len(vectors)) / len(self.corpus_data) * 100
            print(f"  Progress: {progress:.1f}% ({i + len(vectors):,}/{len(self.corpus_data):,})", end="\r")
        
        throughput = len(self.corpus_data) / total_time
        print(f"\n✅ Inserted {len(self.corpus_data):,} vectors in {total_time:.1f}s")
        print(f"   Throughput: {throughput:.0f} vectors/sec")
    
    async def execute_hybrid_search(self, query: HybridSearchQuery) -> Tuple[List[Dict], SearchCost, float]:
        """Execute hybrid search with query planning"""
        print(f"\n🔍 Executing Hybrid Search")
        print(f"   Query: '{query.query_text[:60]}...'")
        print(f"   Filters: {query.metadata_filters}")
        print(f"   Top-k: {query.top_k}")
        
        # Plan the query
        planner = QueryPlanner(self.collection_stats)
        search_cost = planner.plan_hybrid_search(query)
        
        # Execute based on optimal strategy
        start_time = time.time()
        
        if search_cost.strategy == SearchStrategy.METADATA_FIRST:
            results = await self.execute_metadata_first(query)
        elif search_cost.strategy == SearchStrategy.VECTOR_FIRST:
            results = await self.execute_vector_first(query)
        else:  # PARALLEL_HYBRID
            results = await self.execute_parallel_hybrid(query)
            
        actual_time = (time.time() - start_time) * 1000  # Convert to ms
        
        print(f"⚡ Execution Results:")
        print(f"   Strategy used: {search_cost.strategy.value}")
        print(f"   Estimated cost: {search_cost.total_cost:.2f}ms")
        print(f"   Actual time: {actual_time:.2f}ms")
        print(f"   Results found: {len(results)}")
        print(f"   Cost accuracy: {abs(actual_time - search_cost.total_cost) / search_cost.total_cost * 100:.1f}% error")
        
        return results, search_cost, actual_time
    
    async def execute_metadata_first(self, query: HybridSearchQuery) -> List[Dict]:
        """Execute metadata-first strategy"""
        print("   📋 Strategy: Metadata filtering → Vector search")
        
        # Step 1: Filter by metadata
        metadata_matches = []
        for doc in self.corpus_data:
            if self.matches_metadata_filters(doc, query.metadata_filters):
                metadata_matches.append(doc)
        
        if not metadata_matches:
            return []
        
        # Step 2: Vector search on filtered set
        query_embedding = self.embedding_service.embed_text(query.query_text)
        
        # Get embeddings for filtered documents
        filtered_embeddings = []
        filtered_docs = []
        for doc in metadata_matches:
            doc_idx = next(i for i, d in enumerate(self.corpus_data) if d["id"] == doc["id"])
            filtered_embeddings.append(self.embeddings[doc_idx])
            filtered_docs.append(doc)
        
        # Find similar vectors in filtered set
        similar_indices = self.embedding_service.find_similar(
            query_embedding, filtered_embeddings, top_k=query.top_k
        )
        
        # Return results
        results = []
        for idx, score in similar_indices:
            if score >= query.similarity_threshold:
                doc = filtered_docs[idx]
                result = {
                    "id": doc["id"],
                    "score": score,
                    "metadata": {k: v for k, v in doc.items() if k not in ["embedding"]},
                    "vector": doc.get("embedding") if query.include_vectors else None
                }
                results.append(result)
        
        return results
    
    async def execute_vector_first(self, query: HybridSearchQuery) -> List[Dict]:
        """Execute vector-first strategy"""
        print("   🎯 Strategy: Vector search → Metadata filtering")
        
        # Step 1: Vector similarity search
        query_embedding = self.embedding_service.embed_text(query.query_text)
        
        # Search larger set to account for metadata filtering
        search_k = min(query.top_k * 5, len(self.embeddings))
        similar_indices = self.embedding_service.find_similar(
            query_embedding, self.embeddings, top_k=search_k
        )
        
        # Step 2: Filter by metadata and similarity threshold
        results = []
        for idx, score in similar_indices:
            if score >= query.similarity_threshold:
                doc = self.corpus_data[idx]
                if self.matches_metadata_filters(doc, query.metadata_filters):
                    result = {
                        "id": doc["id"],
                        "score": score,
                        "metadata": {k: v for k, v in doc.items() if k not in ["embedding"]},
                        "vector": doc.get("embedding") if query.include_vectors else None
                    }
                    results.append(result)
                    
                    if len(results) >= query.top_k:
                        break
        
        return results
    
    async def execute_parallel_hybrid(self, query: HybridSearchQuery) -> List[Dict]:
        """Execute parallel hybrid strategy"""
        print("   🔄 Strategy: Parallel execution + merge")
        
        # Execute both searches in parallel (simulated)
        vector_results = await self.execute_vector_first(query)
        metadata_results = await self.execute_metadata_first(query)
        
        # Merge and deduplicate results
        merged_results = {}
        
        for result in vector_results:
            merged_results[result["id"]] = result
            
        for result in metadata_results:
            if result["id"] in merged_results:
                # Boost score for items found in both searches
                merged_results[result["id"]]["score"] = (
                    merged_results[result["id"]]["score"] + result["score"]
                ) / 2.0 * 1.1  # 10% boost for hybrid match
            else:
                merged_results[result["id"]] = result
        
        # Sort by score and return top-k
        final_results = sorted(merged_results.values(), key=lambda x: x["score"], reverse=True)
        return final_results[:query.top_k]
    
    def matches_metadata_filters(self, doc: Dict, filters: Dict[str, Any]) -> bool:
        """Check if document matches metadata filters"""
        for key, value in filters.items():
            if str(doc.get(key, "")) != str(value):
                return False
        return True
    
    async def run_comprehensive_test(self):
        """Run comprehensive hybrid search tests"""
        await self.setup_collection_with_large_corpus(10.0)  # 10MB corpus
        
        # Wait for indexing
        print("\n⏳ Waiting for indexing...")
        await asyncio.sleep(5)
        
        # Test different hybrid search scenarios
        test_queries = [
            HybridSearchQuery(
                query_text="machine learning algorithms and artificial intelligence",
                metadata_filters={"category": "AI"},
                top_k=5
            ),
            HybridSearchQuery(
                query_text="deep neural networks and transformers",
                metadata_filters={"author": "Dr. Smith", "doc_type": "research_paper"},
                top_k=8
            ),
            HybridSearchQuery(
                query_text="vector databases and similarity search",
                metadata_filters={"year": "2023"},
                top_k=10
            ),
            HybridSearchQuery(
                query_text="natural language processing",
                metadata_filters={"category": "NLP", "doc_type": "article"},
                top_k=6
            )
        ]
        
        print(f"\n🧪 Running {len(test_queries)} Hybrid Search Tests")
        print("=" * 60)
        
        total_estimated_cost = 0
        total_actual_time = 0
        
        for i, query in enumerate(test_queries, 1):
            print(f"\n🔍 Test {i}/{len(test_queries)}")
            results, cost, actual_time = await self.execute_hybrid_search(query)
            
            total_estimated_cost += cost.total_cost
            total_actual_time += actual_time
            
            # Show sample results
            print(f"   📋 Sample Results:")
            for j, result in enumerate(results[:3]):
                print(f"     {j+1}. {result['id']}: {result['metadata']['text'][:60]}... (score: {result['score']:.3f})")
        
        # Performance summary
        print(f"\n⚡ Overall Performance Summary")
        print("=" * 50)
        print(f"Total queries: {len(test_queries)}")
        print(f"Total estimated cost: {total_estimated_cost:.2f}ms")
        print(f"Total actual time: {total_actual_time:.2f}ms")
        print(f"Average cost accuracy: {abs(total_actual_time - total_estimated_cost) / total_estimated_cost * 100:.1f}% error")
        print(f"Throughput: {len(test_queries) / (total_actual_time / 1000):.1f} queries/sec")
        
        # Cleanup
        await self.cleanup()
    
    async def cleanup(self):
        """Clean up resources"""
        try:
            await self.client.delete_collection(self.collection_name)
            print(f"\n🧹 Cleaned up collection: {self.collection_name}")
        except Exception as e:
            print(f"⚠️ Cleanup warning: {e}")

async def main():
    """Run the advanced hybrid search demonstration"""
    engine = HybridSearchEngine()
    
    try:
        await engine.run_comprehensive_test()
        print("\n🎉 Advanced hybrid search testing completed successfully!")
    except Exception as e:
        print(f"❌ Test failed: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    asyncio.run(main())