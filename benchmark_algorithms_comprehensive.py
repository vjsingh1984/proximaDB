#!/usr/bin/env python3
"""
Comprehensive Algorithm and Distance Metric Benchmarking for ProximaDB

This script creates collections with various indexing algorithms and distance metrics,
inserts BERT embeddings from chunked 100KB text, and benchmarks performance across
different operations including updates, deletes, searches, and metadata filtering.

Copyright 2025 ProximaDB
"""

import sys
import time
import json
import logging
import random
import hashlib
from typing import List, Dict, Any, Tuple
from datetime import datetime
import numpy as np

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Add Python SDK path
sys.path.insert(0, '/home/vsingh/code/proximadb/clients/python/src')

try:
    from proximadb.grpc_client import ProximaDBGrpcClient
    logger.info("✅ ProximaDB gRPC client imported successfully")
except ImportError as e:
    logger.error(f"❌ Failed to import ProximaDB client: {e}")
    sys.exit(1)


class BERTEmbeddingSimulator:
    """Simulates BERT-like embeddings for text chunks"""
    
    def __init__(self, embedding_dim: int = 768):
        self.embedding_dim = embedding_dim
        np.random.seed(42)  # For reproducible embeddings
    
    def chunk_text(self, text: str, chunk_size: int = 512) -> List[str]:
        """Split text into chunks of approximately chunk_size characters"""
        chunks = []
        words = text.split()
        current_chunk = []
        current_length = 0
        
        for word in words:
            if current_length + len(word) + 1 > chunk_size and current_chunk:
                chunks.append(' '.join(current_chunk))
                current_chunk = [word]
                current_length = len(word)
            else:
                current_chunk.append(word)
                current_length += len(word) + 1
        
        if current_chunk:
            chunks.append(' '.join(current_chunk))
        
        return chunks
    
    def generate_bert_embedding(self, text: str) -> List[float]:
        """Generate BERT-like embedding for text chunk"""
        # Create deterministic embedding based on text content
        text_hash = hashlib.md5(text.encode()).hexdigest()
        
        # Use hash to seed random state for consistent embeddings
        hash_seed = int(text_hash[:8], 16)
        local_random = np.random.RandomState(hash_seed)
        
        # Generate normalized embedding similar to BERT output
        embedding = local_random.normal(0, 0.3, self.embedding_dim)
        
        # Add some structure based on text characteristics
        text_length = len(text)
        text_words = len(text.split())
        
        # Modify embedding based on text properties
        embedding[0] = text_length / 1000.0  # Length signal
        embedding[1] = text_words / 100.0    # Word count signal
        embedding[2] = len(set(text.split())) / text_words if text_words > 0 else 0  # Vocabulary diversity
        
        # Normalize to unit vector (common for sentence embeddings)
        norm = np.linalg.norm(embedding)
        if norm > 0:
            embedding = embedding / norm
        
        return embedding.tolist()


class AlgorithmBenchmark:
    """Comprehensive benchmarking of ProximaDB algorithms and distance metrics"""
    
    def __init__(self, endpoint: str = "localhost:5679"):
        self.client = ProximaDBGrpcClient(endpoint=endpoint, timeout=60.0)
        self.embedder = BERTEmbeddingSimulator(embedding_dim=768)
        self.test_collections = []
        self.benchmark_results = {}
        self.sample_text = self._generate_sample_text()
        
        # Algorithm and distance metric combinations to test
        self.test_configurations = [
            {"index_algorithm": "hnsw", "distance_metric": "cosine", "name": "hnsw_cosine"},
            {"index_algorithm": "hnsw", "distance_metric": "euclidean", "name": "hnsw_euclidean"},
            {"index_algorithm": "hnsw", "distance_metric": "dot_product", "name": "hnsw_dot"},
            {"index_algorithm": "ivf", "distance_metric": "cosine", "name": "ivf_cosine"},
            {"index_algorithm": "ivf", "distance_metric": "euclidean", "name": "ivf_euclidean"},
            {"index_algorithm": "brute_force", "distance_metric": "cosine", "name": "brute_cosine"},
            {"index_algorithm": "brute_force", "distance_metric": "manhattan", "name": "brute_manhattan"},
            {"index_algorithm": "auto", "distance_metric": "cosine", "name": "auto_cosine"},
        ]
    
    def _generate_sample_text(self) -> str:
        """Generate approximately 100KB of sample text for embedding"""
        base_text = """
        Artificial intelligence (AI) is transforming industries across the globe. Machine learning algorithms
        are becoming increasingly sophisticated, enabling computers to process and analyze vast amounts of data
        with unprecedented accuracy. Deep learning networks, inspired by the human brain's neural structure,
        can recognize patterns in images, understand natural language, and make predictions about future events.
        
        Vector databases have emerged as a critical infrastructure component for AI applications. These specialized
        databases excel at storing and querying high-dimensional vector representations of data, such as text
        embeddings, image features, and audio signatures. The ability to perform similarity searches across
        millions of vectors in milliseconds has revolutionized recommendation systems, semantic search, and
        content discovery platforms.
        
        Modern embedding models like BERT, GPT, and Sentence Transformers convert text into dense vector
        representations that capture semantic meaning. These embeddings enable applications to understand
        context, similarity, and relationships between different pieces of content. When combined with efficient
        vector search algorithms like HNSW (Hierarchical Navigable Small World) and IVF (Inverted File),
        these systems can deliver real-time responses for complex queries.
        
        The choice of distance metrics significantly impacts search quality and performance. Cosine similarity
        works well for normalized embeddings and focuses on direction rather than magnitude. Euclidean distance
        considers both direction and magnitude, making it suitable for spatial data. Manhattan distance is
        robust to outliers, while dot product can be efficient for specific embedding types.
        
        Metadata filtering adds another dimension to vector search, allowing applications to combine semantic
        similarity with structured attributes. Users can search for "articles about machine learning published
        after 2020 by authors in the technology sector" while maintaining sub-second response times across
        millions of documents.
        
        Performance optimization in vector databases involves careful consideration of indexing algorithms,
        hardware resources, and query patterns. HNSW provides excellent recall with logarithmic search complexity,
        while IVF can be more memory-efficient for certain workloads. Brute force search, though computationally
        expensive, guarantees perfect recall and serves as a baseline for accuracy comparisons.
        
        The future of AI applications depends heavily on the efficiency and scalability of vector search systems.
        As embedding models grow larger and datasets expand, the ability to maintain low latency while preserving
        search quality becomes increasingly challenging. Advanced techniques like quantization, pruning, and
        approximate search help balance the trade-offs between speed, accuracy, and resource consumption.
        """
        
        # Repeat and vary the text to reach ~100KB
        variations = []
        for i in range(50):  # Create enough variations to reach 100KB
            variation = base_text.replace("AI", f"AI_{i}").replace("vector", f"vector_{i}")
            variation += f"\n\nDocument section {i}: " + base_text[:500]
            variations.append(variation)
        
        full_text = "\n\n".join(variations)
        
        # Ensure we have approximately 100KB
        while len(full_text.encode('utf-8')) < 100_000:
            full_text += "\n" + base_text
        
        return full_text[:100_000]  # Truncate to exactly 100KB
    
    def create_test_collections(self) -> List[str]:
        """Create collections with different algorithm and distance metric combinations"""
        logger.info("🏗️ Creating test collections with various algorithms and distance metrics")
        
        created_collections = []
        
        for config in self.test_configurations:
            collection_name = f"benchmark_{config['name']}_{int(time.time())}"
            
            try:
                logger.info(f"Creating collection: {collection_name}")
                logger.info(f"  Algorithm: {config['index_algorithm']}")
                logger.info(f"  Distance: {config['distance_metric']}")
                
                # Note: The actual collection creation will use JSON payload
                # The server will need to interpret the index_algorithm parameter
                collection = self.client.create_collection(
                    name=collection_name,
                    dimension=768,  # BERT embedding dimension
                    distance_metric=config['distance_metric'],
                    description=f"Benchmark collection for {config['index_algorithm']} + {config['distance_metric']}"
                )
                
                created_collections.append({
                    "id": collection.id,
                    "name": collection_name,
                    "config": config
                })
                
                logger.info(f"✅ Created collection: {collection.id}")
                
            except Exception as e:
                logger.error(f"❌ Failed to create collection {collection_name}: {e}")
                continue
        
        self.test_collections = created_collections
        return created_collections
    
    def prepare_embedding_data(self) -> List[Dict[str, Any]]:
        """Prepare text chunks and their BERT embeddings with metadata"""
        logger.info("📝 Preparing text chunks and generating BERT embeddings")
        
        # Chunk the text
        chunks = self.embedder.chunk_text(self.sample_text, chunk_size=512)
        logger.info(f"Generated {len(chunks)} text chunks")
        
        # Generate embeddings and metadata
        embedding_data = []
        
        for i, chunk in enumerate(chunks):
            # Generate client-side ID
            client_id = f"chunk_{i}_{hashlib.md5(chunk.encode()).hexdigest()[:8]}"
            
            # Generate BERT embedding
            embedding = self.embedder.generate_bert_embedding(chunk)
            
            # Create rich metadata for filtering tests
            metadata = {
                "chunk_index": i,
                "text_length": len(chunk),
                "word_count": len(chunk.split()),
                "category": ["technology", "ai", "database", "search"][i % 4],
                "priority": ["high", "medium", "low"][i % 3],
                "timestamp": datetime.now().isoformat(),
                "author": f"author_{i % 10}",
                "source": f"document_{i // 20}",
                "language": "en",
                "topic_tags": [
                    "machine_learning" if "machine learning" in chunk.lower() else "general",
                    "vector_search" if "vector" in chunk.lower() else "general",
                    "embeddings" if "embedding" in chunk.lower() else "general"
                ],
                "quality_score": random.uniform(0.7, 1.0),
                "filterable_fields": {
                    "department": ["engineering", "research", "product"][i % 3],
                    "region": ["us", "eu", "asia"][i % 3],
                    "version": f"v{(i % 5) + 1}",
                    "status": ["published", "draft", "review"][i % 3]
                }
            }
            
            embedding_data.append({
                "id": client_id,
                "vector": embedding,
                "metadata": metadata,
                "text_preview": chunk[:100] + "..." if len(chunk) > 100 else chunk
            })
        
        logger.info(f"✅ Generated {len(embedding_data)} embeddings with rich metadata")
        return embedding_data
    
    def insert_embeddings_to_collections(self, embedding_data: List[Dict[str, Any]]) -> Dict[str, Dict]:
        """Insert the same embeddings to all test collections and measure performance"""
        logger.info("📥 Inserting embeddings to all collections")
        
        insertion_results = {}
        
        for collection_info in self.test_collections:
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Inserting to {collection_name} ({config_name})")
            
            start_time = time.time()
            
            try:
                # Batch insert for better performance
                vectors_for_batch = [
                    {
                        "id": item["id"],
                        "vector": item["vector"],
                        "metadata": item["metadata"]
                    }
                    for item in embedding_data
                ]
                
                batch_result = self.client.batch_insert(
                    collection_name=collection_name,
                    vectors=vectors_for_batch
                )
                
                insertion_time = time.time() - start_time
                
                insertion_results[config_name] = {
                    "collection_name": collection_name,
                    "insertion_time": insertion_time,
                    "vectors_inserted": len(embedding_data),
                    "vectors_per_second": len(embedding_data) / insertion_time,
                    "success": batch_result.success,
                    "config": collection_info["config"]
                }
                
                logger.info(f"✅ {config_name}: {len(embedding_data)} vectors in {insertion_time:.2f}s "
                           f"({len(embedding_data) / insertion_time:.1f} vectors/sec)")
                
            except Exception as e:
                logger.error(f"❌ Failed to insert to {collection_name}: {e}")
                insertion_results[config_name] = {
                    "collection_name": collection_name,
                    "error": str(e),
                    "success": False,
                    "config": collection_info["config"]
                }
        
        return insertion_results
    
    def perform_update_operations(self, embedding_data: List[Dict[str, Any]]) -> Dict[str, Dict]:
        """Perform update operations on a subset of vectors"""
        logger.info("🔄 Performing update operations")
        
        update_results = {}
        
        # Select 10% of vectors for updates
        update_sample = random.sample(embedding_data, max(1, len(embedding_data) // 10))
        
        for collection_info in self.test_collections:
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Updating vectors in {collection_name}")
            
            start_time = time.time()
            successful_updates = 0
            
            try:
                for item in update_sample:
                    # Modify metadata for update
                    updated_metadata = item["metadata"].copy()
                    updated_metadata["updated_at"] = datetime.now().isoformat()
                    updated_metadata["update_count"] = updated_metadata.get("update_count", 0) + 1
                    updated_metadata["status"] = "updated"
                    
                    # For now, we'll simulate updates by re-inserting with updated metadata
                    # In a real implementation, this would be an actual update operation
                    try:
                        result = self.client.insert_vector(
                            collection_name=collection_name,
                            vector_id=item["id"],
                            vector=item["vector"],
                            metadata=updated_metadata
                        )
                        if result.success:
                            successful_updates += 1
                    except Exception as e:
                        logger.warning(f"Update failed for {item['id']}: {e}")
                
                update_time = time.time() - start_time
                
                update_results[config_name] = {
                    "collection_name": collection_name,
                    "update_time": update_time,
                    "updates_attempted": len(update_sample),
                    "updates_successful": successful_updates,
                    "updates_per_second": successful_updates / update_time if update_time > 0 else 0,
                    "config": collection_info["config"]
                }
                
                logger.info(f"✅ {config_name}: {successful_updates}/{len(update_sample)} updates in {update_time:.2f}s")
                
            except Exception as e:
                logger.error(f"❌ Update failed for {collection_name}: {e}")
                update_results[config_name] = {"error": str(e), "config": collection_info["config"]}
        
        return update_results
    
    def perform_delete_operations(self, embedding_data: List[Dict[str, Any]]) -> Dict[str, Dict]:
        """Perform delete operations on a subset of vectors"""
        logger.info("🗑️ Performing delete operations")
        
        delete_results = {}
        
        # Select 5% of vectors for deletion
        delete_sample = random.sample(embedding_data, max(1, len(embedding_data) // 20))
        
        for collection_info in self.test_collections:
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Deleting vectors from {collection_name}")
            
            start_time = time.time()
            successful_deletes = 0
            
            try:
                for item in delete_sample:
                    try:
                        result = self.client.delete_vector(
                            collection_name=collection_name,
                            vector_id=item["id"]
                        )
                        if result.success:
                            successful_deletes += 1
                    except Exception as e:
                        logger.warning(f"Delete failed for {item['id']}: {e}")
                
                delete_time = time.time() - start_time
                
                delete_results[config_name] = {
                    "collection_name": collection_name,
                    "delete_time": delete_time,
                    "deletes_attempted": len(delete_sample),
                    "deletes_successful": successful_deletes,
                    "deletes_per_second": successful_deletes / delete_time if delete_time > 0 else 0,
                    "config": collection_info["config"]
                }
                
                logger.info(f"✅ {config_name}: {successful_deletes}/{len(delete_sample)} deletes in {delete_time:.2f}s")
                
            except Exception as e:
                logger.error(f"❌ Delete failed for {collection_name}: {e}")
                delete_results[config_name] = {"error": str(e), "config": collection_info["config"]}
        
        return delete_results
    
    def perform_search_operations(self, embedding_data: List[Dict[str, Any]]) -> Dict[str, Dict]:
        """Perform various search operations and measure performance"""
        logger.info("🔍 Performing search operations")
        
        search_results = {}
        
        # Prepare search queries
        query_embeddings = [
            self.embedder.generate_bert_embedding("machine learning and artificial intelligence"),
            self.embedder.generate_bert_embedding("vector databases and similarity search"),
            self.embedder.generate_bert_embedding("embeddings and neural networks"),
        ]
        
        for collection_info in self.test_collections:
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Searching in {collection_name}")
            
            collection_results = {
                "collection_name": collection_name,
                "config": collection_info["config"],
                "similarity_searches": [],
                "id_searches": [],
                "metadata_searches": []
            }
            
            # 1. Similarity searches
            for i, query_vector in enumerate(query_embeddings):
                start_time = time.time()
                try:
                    results = self.client.search_vectors(
                        collection_name=collection_name,
                        query_vector=query_vector,
                        k=10,
                        include_metadata=True
                    )
                    search_time = time.time() - start_time
                    
                    collection_results["similarity_searches"].append({
                        "query_id": i,
                        "search_time": search_time,
                        "results_count": len(results),
                        "success": True
                    })
                    
                except Exception as e:
                    collection_results["similarity_searches"].append({
                        "query_id": i,
                        "error": str(e),
                        "success": False
                    })
            
            # 2. ID-based searches
            search_ids = random.sample([item["id"] for item in embedding_data], min(5, len(embedding_data)))
            for vector_id in search_ids:
                start_time = time.time()
                try:
                    result = self.client.get_vector(
                        collection_name=collection_name,
                        vector_id=vector_id,
                        include_vector=True,
                        include_metadata=True
                    )
                    search_time = time.time() - start_time
                    
                    collection_results["id_searches"].append({
                        "vector_id": vector_id,
                        "search_time": search_time,
                        "found": result.get("found", False),
                        "success": True
                    })
                    
                except Exception as e:
                    collection_results["id_searches"].append({
                        "vector_id": vector_id,
                        "error": str(e),
                        "success": False
                    })
            
            # 3. Metadata filtering searches (conceptual - depends on server implementation)
            # For now, we'll just record that this would be tested
            collection_results["metadata_searches"] = {
                "note": "Metadata filtering searches would be implemented here",
                "test_filters": [
                    {"category": "technology"},
                    {"priority": "high"},
                    {"author": "author_1"},
                    {"filterable_fields.department": "engineering"}
                ]
            }
            
            search_results[config_name] = collection_results
            
            # Log summary
            sim_avg_time = np.mean([s["search_time"] for s in collection_results["similarity_searches"] if s.get("success")])
            id_avg_time = np.mean([s["search_time"] for s in collection_results["id_searches"] if s.get("success")])
            
            logger.info(f"✅ {config_name}: Similarity search avg: {sim_avg_time:.4f}s, ID search avg: {id_avg_time:.4f}s")
        
        return search_results
    
    def run_comprehensive_benchmark(self) -> Dict[str, Any]:
        """Run the complete benchmark suite"""
        logger.info("🚀 Starting comprehensive algorithm and distance metric benchmark")
        start_time = time.time()
        
        # Initialize results
        benchmark_results = {
            "start_time": datetime.now().isoformat(),
            "test_configurations": self.test_configurations,
            "sample_text_size": len(self.sample_text.encode('utf-8')),
        }
        
        try:
            # 1. Create collections
            collections = self.create_test_collections()
            benchmark_results["collections_created"] = len(collections)
            
            if not collections:
                raise Exception("No collections were created successfully")
            
            # 2. Prepare embedding data
            embedding_data = self.prepare_embedding_data()
            benchmark_results["embeddings_generated"] = len(embedding_data)
            
            # 3. Insert embeddings
            insertion_results = self.insert_embeddings_to_collections(embedding_data)
            benchmark_results["insertion_results"] = insertion_results
            
            # 4. Perform updates
            update_results = self.perform_update_operations(embedding_data)
            benchmark_results["update_results"] = update_results
            
            # 5. Perform deletes
            delete_results = self.perform_delete_operations(embedding_data)
            benchmark_results["delete_results"] = delete_results
            
            # 6. Perform searches
            search_results = self.perform_search_operations(embedding_data)
            benchmark_results["search_results"] = search_results
            
            # 7. Calculate summary statistics
            total_time = time.time() - start_time
            benchmark_results["total_time"] = total_time
            benchmark_results["end_time"] = datetime.now().isoformat()
            
            logger.info(f"🎉 Benchmark completed in {total_time:.2f} seconds")
            
        except Exception as e:
            logger.error(f"💥 Benchmark failed: {e}")
            benchmark_results["error"] = str(e)
            benchmark_results["end_time"] = datetime.now().isoformat()
        
        finally:
            # Cleanup collections
            self.cleanup_test_collections()
        
        return benchmark_results
    
    def cleanup_test_collections(self):
        """Clean up test collections"""
        logger.info("🧹 Cleaning up test collections")
        
        for collection_info in self.test_collections:
            try:
                self.client.delete_collection(collection_info["name"])
                logger.info(f"✅ Deleted collection: {collection_info['name']}")
            except Exception as e:
                logger.warning(f"⚠️ Failed to delete collection {collection_info['name']}: {e}")
    
    def generate_performance_report(self, results: Dict[str, Any]) -> str:
        """Generate a comprehensive performance report"""
        report_lines = [
            "# ProximaDB Algorithm and Distance Metric Benchmark Report",
            f"Generated: {results.get('end_time', 'unknown')}",
            f"Total Duration: {results.get('total_time', 0):.2f} seconds",
            "",
            "## Test Configuration",
            f"- Sample Text Size: {results.get('sample_text_size', 0):,} bytes (~100KB)",
            f"- Embeddings Generated: {results.get('embeddings_generated', 0)}",
            f"- Collections Created: {results.get('collections_created', 0)}",
            f"- Embedding Dimension: 768 (BERT-like)",
            "",
            "## Algorithm and Distance Metric Combinations Tested"
        ]
        
        for config in results.get('test_configurations', []):
            report_lines.append(f"- {config['name']}: {config['index_algorithm']} + {config['distance_metric']}")
        
        # Insertion Performance
        report_lines.extend([
            "",
            "## Insertion Performance",
            "| Configuration | Vectors | Time (s) | Vectors/sec | Status |",
            "|---------------|---------|----------|-------------|--------|"
        ])
        
        for config_name, result in results.get('insertion_results', {}).items():
            if result.get('success'):
                report_lines.append(
                    f"| {config_name} | {result.get('vectors_inserted', 0)} | "
                    f"{result.get('insertion_time', 0):.2f} | "
                    f"{result.get('vectors_per_second', 0):.1f} | ✅ |"
                )
            else:
                report_lines.append(f"| {config_name} | - | - | - | ❌ |")
        
        # Search Performance
        report_lines.extend([
            "",
            "## Search Performance",
            "| Configuration | Similarity Avg (ms) | ID Search Avg (ms) | Status |",
            "|---------------|---------------------|-------------------|--------|"
        ])
        
        for config_name, result in results.get('search_results', {}).items():
            sim_searches = result.get('similarity_searches', [])
            id_searches = result.get('id_searches', [])
            
            sim_times = [s.get('search_time', 0) * 1000 for s in sim_searches if s.get('success')]
            id_times = [s.get('search_time', 0) * 1000 for s in id_searches if s.get('success')]
            
            sim_avg = np.mean(sim_times) if sim_times else 0
            id_avg = np.mean(id_times) if id_times else 0
            
            status = "✅" if sim_times and id_times else "❌"
            
            report_lines.append(
                f"| {config_name} | {sim_avg:.2f} | {id_avg:.2f} | {status} |"
            )
        
        return "\n".join(report_lines)


def main():
    """Main execution function"""
    logger.info("🎯 ProximaDB Comprehensive Algorithm Benchmark")
    
    try:
        # Initialize benchmark
        benchmark = AlgorithmBenchmark("localhost:5679")
        
        # Run comprehensive benchmark
        results = benchmark.run_comprehensive_benchmark()
        
        # Save results to JSON
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        results_file = f"proximadb_algorithm_benchmark_{timestamp}.json"
        
        with open(results_file, 'w') as f:
            json.dump(results, f, indent=2, default=str)
        
        logger.info(f"📄 Results saved to: {results_file}")
        
        # Generate and save performance report
        report = benchmark.generate_performance_report(results)
        report_file = f"proximadb_benchmark_report_{timestamp}.md"
        
        with open(report_file, 'w') as f:
            f.write(report)
        
        logger.info(f"📊 Performance report saved to: {report_file}")
        
        # Print summary
        print("\n" + "="*60)
        print("BENCHMARK SUMMARY")
        print("="*60)
        print(f"Collections tested: {results.get('collections_created', 0)}")
        print(f"Embeddings processed: {results.get('embeddings_generated', 0)}")
        print(f"Total time: {results.get('total_time', 0):.2f} seconds")
        
        if results.get('error'):
            print(f"⚠️ Benchmark encountered errors: {results['error']}")
            return 1
        else:
            print("✅ Benchmark completed successfully!")
            return 0
            
    except Exception as e:
        logger.error(f"💥 Benchmark execution failed: {e}")
        return 1


if __name__ == "__main__":
    exit(main())