#!/usr/bin/env python3
"""
ProximaDB Hybrid Search Comprehensive Demo
Complete demonstration of semantic search with metadata filtering

Features:
- BERT embeddings for semantic search
- Metadata filtering capabilities
- All 7 search combinations
- Query planning and cost optimization
- Production workflow demonstration
- Integration with demo logger
"""

import sys
import time
import asyncio
import numpy as np
import json
from pathlib import Path
from typing import List, Dict, Any, Tuple, Optional
from dataclasses import dataclass
from enum import Enum
import uuid

# Add parent directory for utils
sys.path.append(str(Path(__file__).parent))

from proximadb import ProximaDBClient, Protocol
from utils.bert_embedding_service import BERTEmbeddingService
from utils.demo_logger import DemoLogger


class SearchStrategy(Enum):
    """Search strategy options"""
    VECTOR_FIRST = "vector_first"
    METADATA_FIRST = "metadata_first"
    PARALLEL_HYBRID = "parallel_hybrid"
    ADAPTIVE = "adaptive"


@dataclass
class SearchResult:
    """Unified search result"""
    vector_id: str
    score: float
    metadata: Dict[str, Any]
    text: str
    search_sources: List[str]
    combined_score: float = 0.0


@dataclass
class SearchCost:
    """Search cost estimation"""
    vector_search_cost: float
    metadata_filter_cost: float
    merge_cost: float
    total_cost: float
    estimated_result_count: int
    strategy: SearchStrategy


class HybridSearchComprehensiveDemo:
    """Comprehensive hybrid search demonstration"""
    
    def __init__(self):
        self.logger = DemoLogger("hybrid_search_comprehensive")
        self.client = ProximaDBClient(endpoint="localhost:5679")
        self.embedding_service = BERTEmbeddingService("all-MiniLM-L6-v2")
        self.collection_name = f"hybrid_demo_{uuid.uuid4().hex[:8]}"
        self.corpus_data = []
        self.embeddings = []
        self.inserted_vectors = []
        
    async def setup(self):
        """Setup collection and data"""
        self.logger.section("Hybrid Search Demo Setup")
        
        try:
            # Create collection
            collection = await self.client.create_collection(
                name=self.collection_name,
                dimension=384,
                distance_metric=1,  # Cosine
                indexing_algorithm=1,  # HNSW
                storage_engine=1,  # VIPER
                filterable_metadata_fields=["category", "author", "doc_type", "year"]
            )
            self.logger.success(f"Created collection: {self.collection_name}")
            
            # Load or generate corpus
            await self.load_corpus()
            
            return True
            
        except Exception as e:
            self.logger.error("Setup failed", e)
            return False
    
    async def load_corpus(self):
        """Load corpus with BERT embeddings"""
        self.logger.log("Loading corpus data...")
        
        # Check for cached data
        cache_dir = Path("./embedding_cache")
        corpus_cache = cache_dir / "corpus_10mb.json"
        embeddings_cache = cache_dir / "embeddings_10mb.npy"
        
        if corpus_cache.exists() and embeddings_cache.exists():
            self.logger.log("Loading cached corpus and embeddings...")
            
            with open(corpus_cache, 'r') as f:
                self.corpus_data = json.load(f)
            
            self.embeddings = np.load(embeddings_cache)
            self.logger.success(f"Loaded {len(self.corpus_data)} documents from cache")
        else:
            # Generate sample corpus
            self.logger.log("Generating sample corpus...")
            
            categories = ["AI", "ML", "NLP", "Database", "Vector Search", "Deep Learning"]
            authors = ["Dr. Smith", "Prof. Johnson", "Dr. Chen", "Prof. Williams", "Dr. Brown"]
            
            for i in range(100):
                doc = {
                    "id": f"doc_{i}",
                    "text": f"Document {i} about {categories[i % len(categories)]} research",
                    "metadata": {
                        "category": categories[i % len(categories)],
                        "author": authors[i % len(authors)],
                        "doc_type": "research_paper" if i % 3 == 0 else "article",
                        "year": 2020 + (i % 5)
                    }
                }
                self.corpus_data.append(doc)
            
            # Generate embeddings
            texts = [doc["text"] for doc in self.corpus_data]
            self.embeddings = await self.embedding_service.embed_batch(texts)
            
            self.logger.success(f"Generated {len(self.corpus_data)} documents with embeddings")
    
    async def insert_vectors(self):
        """Insert vectors with optimal batching"""
        self.logger.section("Vector Insertion")
        
        batch_size = 100
        total_inserted = 0
        
        self.logger.log(f"Inserting {len(self.corpus_data)} vectors in batches of {batch_size}")
        
        start_time = time.time()
        
        for i in range(0, len(self.corpus_data), batch_size):
            batch_docs = self.corpus_data[i:i+batch_size]
            batch_embeddings = self.embeddings[i:i+batch_size]
            
            vectors = []
            for doc, embedding in zip(batch_docs, batch_embeddings):
                vectors.append({
                    "id": doc["id"],
                    "vector": embedding.tolist(),
                    "metadata": doc["metadata"]
                })
            
            try:
                result = await self.client.insert_vectors(
                    collection_name=self.collection_name,
                    vectors=vectors
                )
                total_inserted += len(vectors)
                self.inserted_vectors.extend(vectors)
            except Exception as e:
                self.logger.warning(f"Batch insert failed: {e}")
        
        insert_time = time.time() - start_time
        throughput = total_inserted / insert_time
        
        self.logger.success(f"Inserted {total_inserted} vectors")
        self.logger.metric("Insert throughput", throughput, "vec/s")
        self.logger.metric("Total insert time", insert_time, "s")
    
    async def demonstrate_all_search_modes(self):
        """Demonstrate all 7 search combinations"""
        self.logger.section("All 7 Search Combinations")
        
        # Test scenario
        query_text = "artificial intelligence machine learning algorithms"
        target_ids = ["doc_1", "doc_5", "doc_10"]
        metadata_filter = {"category": "AI", "doc_type": "research_paper"}
        
        self.logger.log(f"Query: '{query_text}'")
        self.logger.log(f"Target IDs: {target_ids}")
        self.logger.log(f"Metadata filter: {metadata_filter}")
        
        # 1. Search by ID only
        self.logger.log("\n1️⃣ Search by ID only")
        start_time = time.time()
        id_results = await self.search_by_id(target_ids)
        id_time = (time.time() - start_time) * 1000
        self.logger.metric("ID search time", id_time, "ms")
        self.logger.log(f"   Found {len(id_results)} results")
        
        # 2. Metadata filtering only
        self.logger.log("\n2️⃣ Metadata filtering only")
        start_time = time.time()
        metadata_results = await self.search_by_metadata(metadata_filter)
        metadata_time = (time.time() - start_time) * 1000
        self.logger.metric("Metadata filter time", metadata_time, "ms")
        self.logger.log(f"   Found {len(metadata_results)} results")
        
        # 3. Similarity search only
        self.logger.log("\n3️⃣ Similarity search only")
        query_embedding = await self.embedding_service.embed(query_text)
        
        start_time = time.time()
        similarity_results = await self.search_by_similarity(query_embedding.tolist())
        similarity_time = (time.time() - start_time) * 1000
        self.logger.metric("Similarity search time", similarity_time, "ms")
        self.logger.log(f"   Found {len(similarity_results)} results")
        
        # 4. ID + Similarity
        self.logger.log("\n4️⃣ ID + Similarity search")
        start_time = time.time()
        id_sim_results = await self.combine_id_similarity(target_ids, query_embedding.tolist())
        id_sim_time = (time.time() - start_time) * 1000
        self.logger.metric("ID+Similarity time", id_sim_time, "ms")
        self.logger.log(f"   Found {len(id_sim_results)} results")
        
        # 5. ID + Metadata
        self.logger.log("\n5️⃣ ID + Metadata filtering")
        start_time = time.time()
        id_meta_results = await self.combine_id_metadata(target_ids, metadata_filter)
        id_meta_time = (time.time() - start_time) * 1000
        self.logger.metric("ID+Metadata time", id_meta_time, "ms")
        self.logger.log(f"   Found {len(id_meta_results)} results")
        
        # 6. Metadata + Similarity
        self.logger.log("\n6️⃣ Metadata + Similarity search")
        start_time = time.time()
        meta_sim_results = await self.combine_metadata_similarity(metadata_filter, query_embedding.tolist())
        meta_sim_time = (time.time() - start_time) * 1000
        self.logger.metric("Metadata+Similarity time", meta_sim_time, "ms")
        self.logger.log(f"   Found {len(meta_sim_results)} results")
        
        # 7. All three combined
        self.logger.log("\n7️⃣ All three combined (Ultimate search)")
        start_time = time.time()
        all_results = await self.combine_all_three(target_ids, metadata_filter, query_embedding.tolist())
        all_time = (time.time() - start_time) * 1000
        self.logger.metric("All combined time", all_time, "ms")
        self.logger.log(f"   Found {len(all_results)} results")
    
    async def demonstrate_query_planning(self):
        """Demonstrate query planning and cost optimization"""
        self.logger.section("Query Planning and Cost Optimization")
        
        # Estimate collection statistics
        collection_size = len(self.inserted_vectors)
        
        # Test different selectivity scenarios
        scenarios = [
            {
                "name": "High selectivity",
                "filter": {"category": "AI", "year": 2024},
                "expected_selectivity": 0.05
            },
            {
                "name": "Medium selectivity",
                "filter": {"category": "ML"},
                "expected_selectivity": 0.17
            },
            {
                "name": "Low selectivity",
                "filter": {"doc_type": "article"},
                "expected_selectivity": 0.67
            }
        ]
        
        for scenario in scenarios:
            self.logger.log(f"\n{scenario['name']} scenario:")
            self.logger.log(f"Filter: {scenario['filter']}")
            
            # Estimate costs for different strategies
            vector_search_cost = np.log2(collection_size) * 12.0  # HNSW complexity
            metadata_filter_cost = collection_size * 0.003  # Linear scan
            filtered_size = int(collection_size * scenario['expected_selectivity'])
            
            # Strategy costs
            vector_first_cost = vector_search_cost + (10 * 0.1)  # Filter top-k results
            metadata_first_cost = metadata_filter_cost + np.log2(max(filtered_size, 1)) * 12.0
            parallel_cost = max(vector_search_cost, metadata_filter_cost) + 2.5
            
            costs = {
                "vector_first": vector_first_cost,
                "metadata_first": metadata_first_cost,
                "parallel": parallel_cost
            }
            
            optimal = min(costs.items(), key=lambda x: x[1])
            
            self.logger.log(f"Expected selectivity: {scenario['expected_selectivity']:.1%}")
            self.logger.log(f"Estimated costs:")
            for strategy, cost in costs.items():
                marker = "✅" if strategy == optimal[0] else "  "
                self.logger.log(f"  {marker} {strategy}: {cost:.1f}ms")
            
            self.logger.success(f"Optimal strategy: {optimal[0]}")
    
    # Search implementation methods
    async def search_by_id(self, ids: List[str]) -> List[Dict]:
        """Search by vector IDs"""
        results = []
        for id in ids:
            try:
                vector = await self.client.get_vector(self.collection_name, id)
                if vector:
                    results.append(vector)
            except:
                pass
        return results
    
    async def search_by_metadata(self, filter: Dict) -> List[Dict]:
        """Search by metadata filter"""
        # Use a dummy vector for metadata-only search
        dummy_vector = np.zeros(384).tolist()
        
        results = await self.client.search_vectors(
            collection_name=self.collection_name,
            vector=dummy_vector,
            top_k=100,
            metadata_filter=filter
        )
        
        return [r for r in results.results if r.metadata]
    
    async def search_by_similarity(self, query_vector: List[float]) -> List[Dict]:
        """Pure similarity search"""
        results = await self.client.search_vectors(
            collection_name=self.collection_name,
            vector=query_vector,
            top_k=10
        )
        
        return results.results
    
    async def combine_id_similarity(self, ids: List[str], query_vector: List[float]) -> List[Dict]:
        """Combine ID and similarity search"""
        # Get vectors by ID first
        id_vectors = await self.search_by_id(ids)
        
        # Then do similarity search and boost scores for matching IDs
        sim_results = await self.search_by_similarity(query_vector)
        
        # Combine results
        combined = []
        id_set = set(ids)
        
        for result in sim_results:
            if result.id in id_set:
                result.score *= 1.5  # Boost score for ID matches
            combined.append(result)
        
        return sorted(combined, key=lambda x: x.score, reverse=True)[:10]
    
    async def combine_id_metadata(self, ids: List[str], filter: Dict) -> List[Dict]:
        """Combine ID and metadata filtering"""
        # Get both ID results and metadata results
        id_results = await self.search_by_id(ids)
        meta_results = await self.search_by_metadata(filter)
        
        # Find intersection
        id_set = set(r.get("id") for r in id_results)
        combined = [r for r in meta_results if r.id in id_set]
        
        return combined
    
    async def combine_metadata_similarity(self, filter: Dict, query_vector: List[float]) -> List[Dict]:
        """Combine metadata filtering and similarity search"""
        results = await self.client.search_vectors(
            collection_name=self.collection_name,
            vector=query_vector,
            top_k=10,
            metadata_filter=filter
        )
        
        return results.results
    
    async def combine_all_three(self, ids: List[str], filter: Dict, query_vector: List[float]) -> List[Dict]:
        """Combine all three search methods"""
        # Search with metadata filter
        filtered_results = await self.combine_metadata_similarity(filter, query_vector)
        
        # Boost scores for ID matches
        id_set = set(ids)
        for result in filtered_results:
            if result.id in id_set:
                result.score *= 2.0  # Double boost for ID match
        
        return sorted(filtered_results, key=lambda x: x.score, reverse=True)[:10]
    
    async def cleanup(self):
        """Clean up test collection"""
        try:
            await self.client.delete_collection(self.collection_name)
            self.logger.success("Cleaned up test collection")
        except Exception as e:
            self.logger.warning(f"Cleanup failed: {e}")
    
    async def run_demo(self):
        """Run complete hybrid search demo"""
        self.logger.section("ProximaDB Hybrid Search Comprehensive Demo")
        
        if not await self.setup():
            return False
        
        try:
            await self.insert_vectors()
            await self.demonstrate_all_search_modes()
            await self.demonstrate_query_planning()
            
            self.logger.success("Hybrid search demo completed successfully!")
            
            self.logger.section("Key Insights")
            self.logger.log("• All 7 search combinations provide flexibility")
            self.logger.log("• Query planning optimizes performance")
            self.logger.log("• BERT embeddings enable semantic understanding")
            self.logger.log("• Metadata filtering narrows results effectively")
            self.logger.log("• Combined approaches yield best results")
            
            return True
            
        except Exception as e:
            self.logger.error("Demo failed", e)
            return False
        finally:
            await self.cleanup()


async def main():
    """Run hybrid search comprehensive demo"""
    demo = HybridSearchComprehensiveDemo()
    
    with demo.logger:
        success = await demo.run_demo()
        return 0 if success else 1


if __name__ == "__main__":
    exit_code = asyncio.run(main())
    sys.exit(exit_code)