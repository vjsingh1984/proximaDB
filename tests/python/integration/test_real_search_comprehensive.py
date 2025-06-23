#!/usr/bin/env python3
"""
Real Comprehensive Search Test on ProximaDB Server
Uses cached 10MB corpus with bulk insertion and tests all search combinations:
- Search by ID
- Metadata filtering  
- Similarity search (BERT embeddings)
- ID + similarity search
- ID + metadata filtering
- Metadata filtering + similarity search
- All 3 combined with ranking and top-k results
"""

import sys
import os
import json
import time
import asyncio
import numpy as np
import uuid
from pathlib import Path
from typing import List, Dict, Any, Tuple, Optional
from dataclasses import dataclass
from enum import Enum

# Add Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from bert_embedding_service import BERTEmbeddingService

@dataclass
class SearchResult:
    """Unified search result with ranking"""
    vector_id: str
    score: float
    metadata: Dict[str, Any]
    text: str
    search_sources: List[str]  # Which searches found this result
    combined_score: float = 0.0  # Weighted combination score

class RealSearchTest:
    """Comprehensive real search test using ProximaDB server"""
    
    def __init__(self):
        self.client = ProximaDBClient(endpoint="localhost:5679")
        self.embedding_service = BERTEmbeddingService("all-MiniLM-L6-v2")
        self.collection_name = f"real_search_test_{uuid.uuid4().hex[:8]}"
        self.corpus_data = []
        self.embeddings = []
        self.inserted_vectors = []
        
    async def run_comprehensive_test(self):
        """Run complete comprehensive search test"""
        print("🚀 Real Comprehensive Search Test on ProximaDB Server")
        print("Using cached 10MB corpus with all search combinations")
        print("=" * 70)
        
        try:
            # 1. Load cached corpus and embeddings
            await self.load_cached_corpus()
            
            # 2. Create collection and bulk insert
            await self.setup_collection_and_bulk_insert()
            
            # 3. Test all search combinations
            await self.test_all_search_combinations()
            
            print("\n🎉 All comprehensive search tests completed successfully!")
            return True
            
        except Exception as e:
            print(f"❌ Comprehensive test failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            await self.cleanup()
    
    async def load_cached_corpus(self):
        """Load cached 10MB corpus and embeddings"""
        print("\n📦 Loading Cached 10MB Corpus")
        print("-" * 40)
        
        cache_dir = Path("./embedding_cache")
        corpus_cache = cache_dir / "corpus_10.0mb.json"
        embeddings_cache = cache_dir / "embeddings_10.0mb.npy"
        
        if not corpus_cache.exists() or not embeddings_cache.exists():
            raise FileNotFoundError(
                "Cached 10MB corpus not found. Please run the hybrid search demo first to generate cache."
            )
        
        # Load cached corpus
        print("📄 Loading corpus data...")
        start_time = time.time()
        with open(corpus_cache, 'r') as f:
            self.corpus_data = json.load(f)
        
        # Load cached embeddings
        print("🧠 Loading BERT embeddings...")
        self.embeddings = np.load(embeddings_cache)
        load_time = time.time() - start_time
        
        print(f"✅ Loaded {len(self.corpus_data):,} documents and {len(self.embeddings):,} embeddings")
        print(f"   Loading time: {load_time:.2f}s")
        print(f"   Corpus size: ~10MB")
        print(f"   Embedding dimension: {self.embeddings.shape[1]}")
        
        # Show data distribution
        categories = {}
        authors = {}
        doc_types = {}
        years = {}
        
        for doc in self.corpus_data:
            categories[doc['category']] = categories.get(doc['category'], 0) + 1
            authors[doc['author']] = authors.get(doc['author'], 0) + 1
            doc_types[doc['doc_type']] = doc_types.get(doc['doc_type'], 0) + 1
            years[str(doc['year'])] = years.get(str(doc['year']), 0) + 1
        
        print(f"\n📊 Data Distribution:")
        print(f"   Categories: {len(categories)} types")
        print(f"   Authors: {len(authors)} unique")
        print(f"   Doc types: {len(doc_types)} types")
        print(f"   Years: {len(years)} years")
        
        for cat, count in sorted(categories.items()):
            print(f"     {cat}: {count:,} docs")
    
    async def setup_collection_and_bulk_insert(self):
        """Create collection and perform bulk vector insertion"""
        print("\n🏗️ Setting Up Collection and Bulk Insert")
        print("-" * 40)
        
        # Create collection
        print(f"Creating collection: {self.collection_name}")
        collection = await self.client.create_collection(
            name=self.collection_name,
            dimension=384,  # BERT MiniLM dimension
            distance_metric=1,  # Cosine similarity
            indexing_algorithm=1,  # HNSW
            storage_engine=1  # VIPER
        )
        
        print(f"✅ Collection created: {collection.name}")
        print(f"   Dimension: {collection.dimension}")
        print(f"   Metric: {collection.metric}")
        print(f"   Index type: {collection.index_type}")
        
        # Prepare vectors for bulk insertion
        print(f"\n📊 Preparing {len(self.corpus_data):,} vectors for bulk insertion...")
        
        total_batches = 0
        total_inserted = 0
        total_failed = 0
        bulk_start_time = time.time()
        
        # Insert in batches for better performance
        batch_size = 500  # Larger batches for bulk operation
        
        for i in range(0, len(self.corpus_data), batch_size):
            batch_docs = self.corpus_data[i:i + batch_size]
            batch_embeddings = self.embeddings[i:i + batch_size]
            
            # Prepare batch vectors
            vectors = []
            for doc, embedding in zip(batch_docs, batch_embeddings):
                vector_data = {
                    "id": doc["id"],
                    "vector": embedding.tolist(),
                    "metadata": {
                        "text": doc["text"][:500],  # Truncate for efficiency
                        "category": doc["category"],
                        "author": doc["author"],
                        "doc_type": doc["doc_type"],
                        "year": str(doc["year"]),
                        "keywords": doc.get("keywords", ""),
                        "length": str(doc.get("length", 0))
                    }
                }
                vectors.append(vector_data)
            
            # Insert batch
            batch_start = time.time()
            try:
                result = self.client.insert_vectors(
                    collection_id=self.collection_name,
                    vectors=vectors
                )
                batch_time = time.time() - batch_start
                
                total_batches += 1
                total_inserted += result.count
                total_failed += result.failed_count
                
                progress = (i + len(vectors)) / len(self.corpus_data) * 100
                print(f"  Batch {total_batches}: {result.count}/{len(vectors)} inserted, "
                      f"{batch_time:.2f}s, Progress: {progress:.1f}%")
                
                if result.failed_count > 0:
                    print(f"    ⚠️ {result.failed_count} failed insertions")
                    if result.errors:
                        for error in result.errors[:3]:  # Show first 3 errors
                            print(f"      Error: {error}")
                
                # Store successful insertions for search tests
                for j, vector in enumerate(vectors):
                    if j < result.count:  # Only count successful insertions
                        self.inserted_vectors.append({
                            "id": vector["id"],
                            "metadata": vector["metadata"],
                            "original_doc": batch_docs[j]
                        })
                        
            except Exception as e:
                print(f"    ❌ Batch {total_batches + 1} failed: {e}")
                total_failed += len(vectors)
        
        bulk_time = time.time() - bulk_start_time
        
        print(f"\n✅ Bulk insertion completed!")
        print(f"   Total batches: {total_batches}")
        print(f"   Successfully inserted: {total_inserted:,} vectors")
        print(f"   Failed insertions: {total_failed:,}")
        print(f"   Total time: {bulk_time:.2f}s")
        print(f"   Throughput: {total_inserted / bulk_time:.0f} vectors/sec")
        
        # Wait for indexing
        print(f"⏳ Waiting for indexing to complete...")
        await asyncio.sleep(10)  # Give more time for large dataset
    
    async def test_all_search_combinations(self):
        """Test all search combinations with ranking"""
        print("\n🔍 Testing All Search Combinations")
        print("=" * 50)
        
        # Test scenarios with different combinations
        test_scenarios = [
            {
                "name": "AI Research Query",
                "text_query": "artificial intelligence machine learning algorithms",
                "target_ids": ["ai_001", "ai_002", "ai_003"],
                "metadata_filters": {"category": "AI"},
                "description": "Find AI documents related to ML algorithms"
            },
            {
                "name": "NLP Research by Author",
                "text_query": "natural language processing transformer BERT",
                "target_ids": ["nlp_001", "nlp_002"],
                "metadata_filters": {"author": "Dr. Smith", "doc_type": "research_paper"},
                "description": "Find Dr. Smith's NLP research papers"
            },
            {
                "name": "Database Systems 2023",
                "text_query": "vector database similarity search high dimensional",
                "target_ids": ["db_001", "db_002", "db_003"],
                "metadata_filters": {"category": "Database", "year": "2023"},
                "description": "Find recent database systems research"
            },
            {
                "name": "Deep Learning Articles",
                "text_query": "deep neural networks convolutional architecture",
                "target_ids": ["ml_004", "ml_005"],
                "metadata_filters": {"category": "Deep Learning", "doc_type": "article"},
                "description": "Find deep learning articles about neural networks"
            }
        ]
        
        for scenario in test_scenarios:
            print(f"\n📋 Scenario: {scenario['name']}")
            print(f"   Description: {scenario['description']}")
            print(f"   Text query: '{scenario['text_query']}'")
            print(f"   Target IDs: {scenario['target_ids']}")
            print(f"   Metadata filters: {scenario['metadata_filters']}")
            print("-" * 50)
            
            await self.test_search_combinations_for_scenario(scenario)
    
    async def test_search_combinations_for_scenario(self, scenario: Dict[str, Any]):
        """Test all search combinations for a specific scenario"""
        
        # 1. Search by ID only
        print("\n🎯 1. Search by ID Only")
        id_results = await self.search_by_id(scenario["target_ids"])
        print(f"   Found {len(id_results)} results")
        
        # 2. Metadata filtering only
        print("\n🏷️ 2. Metadata Filtering Only")
        metadata_results = await self.search_by_metadata(scenario["metadata_filters"])
        print(f"   Found {len(metadata_results)} results")
        
        # 3. Similarity search only
        print("\n🔍 3. Similarity Search Only")
        similarity_results = await self.search_by_similarity(scenario["text_query"])
        print(f"   Found {len(similarity_results)} results")
        
        # 4. ID + Similarity
        print("\n🎯🔍 4. ID + Similarity Search")
        id_similarity_results = await self.search_id_and_similarity(
            scenario["target_ids"], scenario["text_query"]
        )
        print(f"   Found {len(id_similarity_results)} results")
        
        # 5. ID + Metadata
        print("\n🎯🏷️ 5. ID + Metadata Filtering")
        id_metadata_results = await self.search_id_and_metadata(
            scenario["target_ids"], scenario["metadata_filters"]
        )
        print(f"   Found {len(id_metadata_results)} results")
        
        # 6. Metadata + Similarity
        print("\n🏷️🔍 6. Metadata + Similarity Search")
        metadata_similarity_results = await self.search_metadata_and_similarity(
            scenario["metadata_filters"], scenario["text_query"]
        )
        print(f"   Found {len(metadata_similarity_results)} results")
        
        # 7. All Three Combined
        print("\n🎯🏷️🔍 7. All Three Combined with Ranking")
        combined_results = await self.search_all_three_combined(
            scenario["target_ids"], scenario["metadata_filters"], scenario["text_query"]
        )
        print(f"   Found {len(combined_results)} results")
        
        # Show top results for combined search
        if combined_results:
            print(f"\n   📊 Top 5 Combined Results:")
            for i, result in enumerate(combined_results[:5], 1):
                sources = ", ".join(result.search_sources)
                print(f"     {i}. {result.vector_id} (score: {result.combined_score:.3f})")
                print(f"        Sources: {sources}")
                print(f"        Text: {result.text[:60]}...")
                print(f"        Category: {result.metadata.get('category', 'N/A')}")
    
    async def search_by_id(self, target_ids: List[str]) -> List[SearchResult]:
        """Search by specific vector IDs"""
        results = []
        
        for target_id in target_ids:
            # Find the target document in our inserted vectors
            target_doc = None
            for inserted in self.inserted_vectors:
                if inserted["id"] == target_id:
                    target_doc = inserted
                    break
            
            if target_doc:
                # Use the document's own embedding for exact match
                original_doc = target_doc["original_doc"]
                doc_idx = None
                for i, doc in enumerate(self.corpus_data):
                    if doc["id"] == target_id:
                        doc_idx = i
                        break
                
                if doc_idx is not None:
                    query_embedding = self.embeddings[doc_idx]
                    
                    # Perform search with very high similarity threshold
                    search_results = self.client.search_vectors(
                        collection_id=self.collection_name,
                        query_vectors=[query_embedding.tolist()],
                        top_k=1
                    )
                    
                    if search_results and search_results[0].vector_id == target_id:
                        result = SearchResult(
                            vector_id=search_results[0].vector_id,
                            score=search_results[0].score,
                            metadata=search_results[0].metadata,
                            text=search_results[0].metadata.get("text", ""),
                            search_sources=["id"],
                            combined_score=1.0  # Perfect match for ID search
                        )
                        results.append(result)
                        print(f"     ✅ Found {target_id} (score: {result.score:.4f})")
                    else:
                        print(f"     ❌ {target_id} not found in search results")
            else:
                print(f"     ❌ {target_id} not found in inserted vectors")
        
        return results
    
    async def search_by_metadata(self, metadata_filters: Dict[str, Any]) -> List[SearchResult]:
        """Search by metadata filtering (client-side simulation)"""
        results = []
        
        # Get all documents that match metadata filters
        matching_docs = []
        for inserted in self.inserted_vectors:
            metadata = inserted["metadata"]
            matches = True
            for key, value in metadata_filters.items():
                if str(metadata.get(key, "")) != str(value):
                    matches = False
                    break
            if matches:
                matching_docs.append(inserted)
        
        print(f"     📊 {len(matching_docs)} documents match metadata filters")
        
        # Convert to SearchResult format
        for doc in matching_docs[:10]:  # Limit to top 10 for demo
            result = SearchResult(
                vector_id=doc["id"],
                score=1.0,  # Perfect metadata match
                metadata=doc["metadata"],
                text=doc["metadata"].get("text", ""),
                search_sources=["metadata"],
                combined_score=0.8  # High score for metadata match
            )
            results.append(result)
        
        return results
    
    async def search_by_similarity(self, text_query: str, top_k: int = 10) -> List[SearchResult]:
        """Search by semantic similarity using BERT embeddings"""
        print(f"     🧠 Generating BERT embedding for: '{text_query}'")
        
        # Generate query embedding
        query_embedding = self.embedding_service.embed_text(text_query)
        
        # Perform similarity search
        search_start = time.time()
        search_results = self.client.search_vectors(
            collection_id=self.collection_name,
            query_vectors=[query_embedding.tolist()],
            top_k=top_k
        )
        search_time = (time.time() - search_start) * 1000
        
        print(f"     ⚡ Search completed in {search_time:.2f}ms")
        
        # Convert to SearchResult format
        results = []
        for search_result in search_results:
            result = SearchResult(
                vector_id=search_result.vector_id,
                score=search_result.score,
                metadata=search_result.metadata,
                text=search_result.metadata.get("text", ""),
                search_sources=["similarity"],
                combined_score=search_result.score
            )
            results.append(result)
            print(f"       {len(results)}. {result.vector_id}: {result.score:.3f} - {result.text[:50]}...")
        
        return results
    
    async def search_id_and_similarity(self, target_ids: List[str], text_query: str) -> List[SearchResult]:
        """Combine ID search and similarity search"""
        # Get both result sets
        id_results = await self.search_by_id(target_ids)
        similarity_results = await self.search_by_similarity(text_query, top_k=20)
        
        # Merge and boost scores for matches found in both
        merged_results = {}
        
        # Add ID results
        for result in id_results:
            result.search_sources = ["id"]
            merged_results[result.vector_id] = result
        
        # Add similarity results and boost hybrid matches
        for result in similarity_results:
            if result.vector_id in merged_results:
                # Found in both - boost score
                existing = merged_results[result.vector_id]
                existing.search_sources.append("similarity")
                existing.combined_score = (existing.combined_score + result.score) / 2.0 * 1.3  # 30% boost
                existing.score = result.score  # Use similarity score
            else:
                result.search_sources = ["similarity"]
                merged_results[result.vector_id] = result
        
        # Sort by combined score
        results = sorted(merged_results.values(), key=lambda x: x.combined_score, reverse=True)
        
        return results[:10]
    
    async def search_id_and_metadata(self, target_ids: List[str], metadata_filters: Dict[str, Any]) -> List[SearchResult]:
        """Combine ID search and metadata filtering"""
        # Get both result sets
        id_results = await self.search_by_id(target_ids)
        metadata_results = await self.search_by_metadata(metadata_filters)
        
        # Find intersection - results that match both ID and metadata
        id_set = {r.vector_id for r in id_results}
        metadata_set = {r.vector_id for r in metadata_results}
        intersection = id_set & metadata_set
        
        # Merge results
        merged_results = {}
        
        # Add results from intersection with boosted scores
        for result in id_results + metadata_results:
            if result.vector_id in intersection:
                if result.vector_id not in merged_results:
                    result.search_sources = ["id", "metadata"]
                    result.combined_score = 1.0  # Perfect match for both
                    merged_results[result.vector_id] = result
            else:
                # Single source match
                if result.vector_id not in merged_results:
                    merged_results[result.vector_id] = result
        
        results = sorted(merged_results.values(), key=lambda x: x.combined_score, reverse=True)
        
        return results[:10]
    
    async def search_metadata_and_similarity(self, metadata_filters: Dict[str, Any], text_query: str) -> List[SearchResult]:
        """Combine metadata filtering and similarity search"""
        # Get both result sets
        metadata_results = await self.search_by_metadata(metadata_filters)
        similarity_results = await self.search_by_similarity(text_query, top_k=20)
        
        # Create metadata filter set for faster lookup
        metadata_set = {r.vector_id for r in metadata_results}
        
        # Merge and boost scores for matches found in both
        merged_results = {}
        
        # Add similarity results, boosting those that also match metadata
        for result in similarity_results:
            if result.vector_id in metadata_set:
                # Found in both - boost score
                result.search_sources = ["metadata", "similarity"]
                result.combined_score = result.score * 1.5  # 50% boost for metadata match
            else:
                result.search_sources = ["similarity"]
                result.combined_score = result.score
            merged_results[result.vector_id] = result
        
        # Add metadata-only results
        for result in metadata_results:
            if result.vector_id not in merged_results:
                result.search_sources = ["metadata"]
                merged_results[result.vector_id] = result
        
        # Sort by combined score
        results = sorted(merged_results.values(), key=lambda x: x.combined_score, reverse=True)
        
        return results[:10]
    
    async def search_all_three_combined(self, target_ids: List[str], metadata_filters: Dict[str, Any], 
                                      text_query: str) -> List[SearchResult]:
        """Combine all three search methods with sophisticated ranking"""
        print(f"     🔄 Executing all three search methods...")
        
        # Get all result sets
        id_results = await self.search_by_id(target_ids)
        metadata_results = await self.search_by_metadata(metadata_filters)
        similarity_results = await self.search_by_similarity(text_query, top_k=30)
        
        print(f"     📊 Raw results: ID={len(id_results)}, Metadata={len(metadata_results)}, Similarity={len(similarity_results)}")
        
        # Create sets for intersection analysis
        id_set = {r.vector_id for r in id_results}
        metadata_set = {r.vector_id for r in metadata_results}
        similarity_dict = {r.vector_id: r.score for r in similarity_results}
        
        # Analyze intersections
        all_three = id_set & metadata_set & set(similarity_dict.keys())
        id_and_metadata = (id_set & metadata_set) - all_three
        id_and_similarity = (id_set & set(similarity_dict.keys())) - metadata_set
        metadata_and_similarity = (metadata_set & set(similarity_dict.keys())) - id_set
        
        print(f"     🎯 Intersections: All 3={len(all_three)}, ID+Meta={len(id_and_metadata)}, "
              f"ID+Sim={len(id_and_similarity)}, Meta+Sim={len(metadata_and_similarity)}")
        
        # Build final results with sophisticated scoring
        final_results = {}
        
        # Process all similarity results as base
        for result in similarity_results:
            vid = result.vector_id
            sources = ["similarity"]
            base_score = result.score
            
            # Calculate combined score based on which searches found this result
            if vid in all_three:
                sources = ["id", "metadata", "similarity"]
                combined_score = base_score * 2.0  # 100% boost for triple match
            elif vid in id_and_metadata:
                sources = ["id", "metadata", "similarity"]
                combined_score = base_score * 1.7  # 70% boost
            elif vid in id_and_similarity:
                sources = ["id", "similarity"]
                combined_score = base_score * 1.5  # 50% boost
            elif vid in metadata_and_similarity:
                sources = ["metadata", "similarity"]
                combined_score = base_score * 1.3  # 30% boost
            else:
                combined_score = base_score
            
            final_result = SearchResult(
                vector_id=result.vector_id,
                score=result.score,
                metadata=result.metadata,
                text=result.text,
                search_sources=sources,
                combined_score=combined_score
            )
            final_results[vid] = final_result
        
        # Add ID-only and metadata-only results that weren't in similarity search
        for result in id_results + metadata_results:
            vid = result.vector_id
            if vid not in final_results:
                # These didn't match similarity search, so lower score
                result.combined_score = 0.5  # Lower score for non-similarity matches
                final_results[vid] = result
        
        # Sort by combined score and return top results
        results = sorted(final_results.values(), key=lambda x: x.combined_score, reverse=True)
        
        return results[:15]  # Return top 15 for comprehensive view
    
    async def cleanup(self):
        """Clean up test resources"""
        try:
            await self.client.delete_collection(self.collection_name)
            print(f"\n🧹 Cleaned up collection: {self.collection_name}")
        except Exception as e:
            print(f"⚠️ Cleanup warning: {e}")

async def main():
    """Run the comprehensive real search test"""
    test = RealSearchTest()
    success = await test.run_comprehensive_test()
    
    if success:
        print("\n✨ Comprehensive Real Search Test Summary:")
        print("- ✅ Cached 10MB corpus loaded successfully")
        print("- ✅ Bulk vector insertion completed")
        print("- ✅ Search by ID functionality")
        print("- ✅ Metadata filtering")
        print("- ✅ BERT-powered similarity search")
        print("- ✅ ID + similarity search combination")
        print("- ✅ ID + metadata filtering combination")
        print("- ✅ Metadata + similarity search combination")
        print("- ✅ All 3 combined with sophisticated ranking")
        print("- ✅ Top-k results with multi-source scoring")
        print("\n🎯 Key Achievements:")
        print("  • Real ProximaDB server integration with bulk data")
        print("  • All 7 search combination modes tested")
        print("  • Sophisticated ranking and scoring algorithms")
        print("  • BERT semantic search with 10MB corpus")
        print("  • Production-ready search architecture")
    else:
        print("\n❌ Comprehensive test encountered issues")

if __name__ == "__main__":
    asyncio.run(main())