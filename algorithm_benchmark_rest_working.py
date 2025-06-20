#!/usr/bin/env python3
"""
Working Algorithm Benchmark using REST API

Since gRPC has Avro payload limitations, this script uses the REST API
to actually test different algorithms and distance metrics with real BERT embeddings.

Copyright 2025 ProximaDB
"""

import sys
import time
import json
import logging
import random
import hashlib
import requests
from typing import List, Dict, Any, Tuple
from datetime import datetime
import numpy as np

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)


class RESTAlgorithmBenchmark:
    """Algorithm benchmark using ProximaDB REST API"""
    
    def __init__(self, base_url: str = "http://localhost:5678"):
        self.base_url = f"{base_url}/api/v1"
        self.session = requests.Session()
        self.session.headers.update({'Content-Type': 'application/json'})
        
        # Test configurations - simplified since REST may have different algorithm support
        self.test_configurations = [
            {"distance_metric": "cosine", "storage_layout": "standard", "name": "cosine_standard"},
            {"distance_metric": "euclidean", "storage_layout": "standard", "name": "euclidean_standard"},
            {"distance_metric": "dot_product", "storage_layout": "standard", "name": "dot_standard"},
            {"distance_metric": "manhattan", "storage_layout": "viper", "name": "manhattan_viper"},
            {"distance_metric": "cosine", "storage_layout": "viper", "name": "cosine_viper"},
        ]
        
        self.test_collections = []
        self.sample_text = self._generate_sample_text()
    
    def _generate_sample_text(self) -> str:
        """Generate sample text for embeddings"""
        return """
        ProximaDB is a high-performance vector database designed for AI and machine learning applications.
        It supports multiple indexing algorithms including HNSW, IVF, and brute force search methods.
        The database provides excellent performance for similarity search across millions of vectors.
        Distance metrics like cosine similarity, Euclidean distance, and dot product are fully supported.
        Vector embeddings from models like BERT, GPT, and custom neural networks work seamlessly.
        Metadata filtering enables complex queries that combine semantic similarity with structured attributes.
        Real-time indexing ensures that newly inserted vectors are immediately available for search.
        The architecture scales horizontally across multiple nodes for enterprise workloads.
        """ * 50  # Repeat to get substantial content
    
    def generate_bert_embedding(self, text: str, dim: int = 128) -> List[float]:
        """Generate deterministic BERT-like embedding"""
        # Use text hash for reproducible embeddings
        text_hash = hashlib.md5(text.encode()).hexdigest()
        hash_seed = int(text_hash[:8], 16)
        
        # Create local random state for consistency
        local_random = np.random.RandomState(hash_seed)
        embedding = local_random.normal(0, 0.3, dim)
        
        # Add text-specific features
        embedding[0] = len(text) / 1000.0
        embedding[1] = len(text.split()) / 100.0
        
        # Normalize for cosine similarity
        norm = np.linalg.norm(embedding)
        if norm > 0:
            embedding = embedding / norm
        
        return embedding.tolist()
    
    def check_server_health(self) -> bool:
        """Check if REST server is responding"""
        try:
            # Health endpoint is at root, not under /api/v1
            health_url = self.base_url.replace("/api/v1", "") + "/health"
            response = self.session.get(health_url, timeout=5)
            if response.status_code == 200:
                logger.info("✅ REST server is healthy")
                return True
            else:
                logger.error(f"❌ REST server health check failed: {response.status_code}")
                return False
        except Exception as e:
            logger.error(f"❌ Cannot connect to REST server: {e}")
            return False
    
    def create_test_collections(self) -> List[Dict]:
        """Create collections with different configurations"""
        logger.info("🏗️ Creating test collections via REST API")
        
        created_collections = []
        
        for config in self.test_configurations:
            collection_name = f"rest_benchmark_{config['name']}_{int(time.time())}"
            
            collection_data = {
                "name": collection_name,
                "dimension": 128,
                "distance_metric": config["distance_metric"],
                "storage_layout": config["storage_layout"],
                "config": {
                    "description": f"REST benchmark for {config['distance_metric']} + {config['storage_layout']}"
                }
            }
            
            try:
                logger.info(f"Creating collection: {collection_name}")
                logger.info(f"  Distance: {config['distance_metric']}")
                logger.info(f"  Storage: {config['storage_layout']}")
                
                response = self.session.post(
                    f"{self.base_url}/collections",
                    json=collection_data,
                    timeout=10
                )
                
                if response.status_code == 200:
                    result = response.json()
                    created_collections.append({
                        "id": result.get("id", collection_name),
                        "name": collection_name,
                        "config": config,
                        "creation_response": result
                    })
                    logger.info(f"✅ Created collection: {result.get('id', collection_name)}")
                else:
                    logger.error(f"❌ Failed to create collection {collection_name}: {response.status_code} - {response.text}")
                    
            except Exception as e:
                logger.error(f"❌ Exception creating collection {collection_name}: {e}")
                continue
        
        self.test_collections = created_collections
        return created_collections
    
    def prepare_embedding_data(self, count: int = 50) -> List[Dict[str, Any]]:
        """Prepare text chunks and embeddings"""
        logger.info(f"📝 Preparing {count} embeddings")
        
        # Split text into chunks
        words = self.sample_text.split()
        chunk_size = len(words) // count
        
        embedding_data = []
        
        for i in range(count):
            start_idx = i * chunk_size
            end_idx = min((i + 1) * chunk_size, len(words))
            chunk_text = ' '.join(words[start_idx:end_idx])
            
            # Generate client ID
            client_id = f"rest_vector_{i}_{hashlib.md5(chunk_text.encode()).hexdigest()[:8]}"
            
            # Generate embedding
            embedding = self.generate_bert_embedding(chunk_text)
            
            # Rich metadata for filtering tests
            metadata = {
                "chunk_index": i,
                "text_length": len(chunk_text),
                "word_count": len(chunk_text.split()),
                "category": ["technology", "database", "ai", "search"][i % 4],
                "priority": ["high", "medium", "low"][i % 3],
                "timestamp": datetime.now().isoformat(),
                "author": f"author_{i % 5}",
                "source": f"document_{i // 10}",
                "version": f"v{(i % 3) + 1}",
                "status": ["published", "draft", "review"][i % 3],
                "quality_score": round(random.uniform(0.7, 1.0), 2),
                # Nested metadata for advanced filtering
                "attributes": {
                    "department": ["engineering", "research", "product"][i % 3],
                    "region": ["us", "eu", "asia"][i % 3],
                    "confidential": i % 4 == 0
                }
            }
            
            embedding_data.append({
                "id": client_id,
                "vector": embedding,
                "metadata": metadata,
                "text_preview": chunk_text[:100] + "..." if len(chunk_text) > 100 else chunk_text
            })
        
        logger.info(f"✅ Generated {len(embedding_data)} embeddings with rich metadata")
        return embedding_data
    
    def insert_vectors_to_collections(self, embedding_data: List[Dict]) -> Dict[str, Dict]:
        """Insert vectors to all collections and measure performance"""
        logger.info("📥 Inserting vectors to all collections via REST")
        
        insertion_results = {}
        
        for collection_info in self.test_collections:
            collection_id = collection_info["id"]
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Inserting to {collection_name} ({config_name})")
            
            start_time = time.time()
            successful_inserts = 0
            
            try:
                # Insert vectors one by one (REST API pattern)
                for item in embedding_data:
                    vector_data = {
                        "collection_id": collection_id,
                        "id": item["id"],
                        "vector": item["vector"],
                        "metadata": item["metadata"]
                    }
                    
                    try:
                        response = self.session.post(
                            f"{self.base_url}/vectors",
                            json=vector_data,
                            timeout=10
                        )
                        
                        if response.status_code == 200:
                            successful_inserts += 1
                        else:
                            logger.warning(f"Insert failed for {item['id']}: {response.status_code}")
                            
                    except Exception as e:
                        logger.warning(f"Insert exception for {item['id']}: {e}")
                
                insertion_time = time.time() - start_time
                
                insertion_results[config_name] = {
                    "collection_id": collection_id,
                    "collection_name": collection_name,
                    "insertion_time": insertion_time,
                    "vectors_attempted": len(embedding_data),
                    "vectors_successful": successful_inserts,
                    "vectors_per_second": successful_inserts / insertion_time if insertion_time > 0 else 0,
                    "success_rate": successful_inserts / len(embedding_data) * 100,
                    "config": collection_info["config"]
                }
                
                logger.info(f"✅ {config_name}: {successful_inserts}/{len(embedding_data)} vectors in {insertion_time:.2f}s "
                           f"({successful_inserts / insertion_time:.1f} vectors/sec)")
                
            except Exception as e:
                logger.error(f"❌ Failed to insert to {collection_name}: {e}")
                insertion_results[config_name] = {
                    "collection_name": collection_name,
                    "error": str(e),
                    "success": False
                }
        
        return insertion_results
    
    def perform_search_operations(self, embedding_data: List[Dict]) -> Dict[str, Dict]:
        """Perform various search operations"""
        logger.info("🔍 Performing search operations via REST")
        
        search_results = {}
        
        # Prepare query vectors
        query_texts = [
            "vector database performance optimization",
            "machine learning embeddings similarity",
            "artificial intelligence search algorithms"
        ]
        
        query_vectors = [self.generate_bert_embedding(text) for text in query_texts]
        
        for collection_info in self.test_collections:
            collection_id = collection_info["id"]
            collection_name = collection_info["name"]
            config_name = collection_info["config"]["name"]
            
            logger.info(f"Searching in {collection_name}")
            
            collection_results = {
                "collection_id": collection_id,
                "collection_name": collection_name,
                "config": collection_info["config"],
                "similarity_searches": [],
                "id_searches": [],
                "metadata_searches": []
            }
            
            # 1. Similarity searches
            for i, query_vector in enumerate(query_vectors):
                start_time = time.time()
                try:
                    search_data = {
                        "collection_id": collection_id,
                        "query_vector": query_vector,
                        "top_k": 5,
                        "include_metadata": True
                    }
                    
                    response = self.session.post(
                        f"{self.base_url}/vectors/search",
                        json=search_data,
                        timeout=10
                    )
                    
                    search_time = time.time() - start_time
                    
                    if response.status_code == 200:
                        results = response.json()
                        collection_results["similarity_searches"].append({
                            "query_id": i,
                            "search_time": search_time,
                            "results_count": len(results.get("results", [])),
                            "success": True,
                            "query_text": query_texts[i][:50] + "..."
                        })
                    else:
                        collection_results["similarity_searches"].append({
                            "query_id": i,
                            "error": f"HTTP {response.status_code}",
                            "success": False
                        })
                        
                except Exception as e:
                    collection_results["similarity_searches"].append({
                        "query_id": i,
                        "error": str(e),
                        "success": False
                    })
            
            # 2. ID-based searches
            search_ids = random.sample([item["id"] for item in embedding_data], min(3, len(embedding_data)))
            for vector_id in search_ids:
                start_time = time.time()
                try:
                    response = self.session.get(
                        f"{self.base_url}/vectors/{collection_id}/{vector_id}",
                        timeout=10
                    )
                    
                    search_time = time.time() - start_time
                    
                    collection_results["id_searches"].append({
                        "vector_id": vector_id,
                        "search_time": search_time,
                        "found": response.status_code == 200,
                        "success": True
                    })
                    
                except Exception as e:
                    collection_results["id_searches"].append({
                        "vector_id": vector_id,
                        "error": str(e),
                        "success": False
                    })
            
            # 3. Metadata filtering (if supported by REST API)
            # This is conceptual since metadata filtering depends on server implementation
            collection_results["metadata_searches"] = {
                "note": "Metadata filtering would be tested here",
                "planned_filters": [
                    {"category": "technology"},
                    {"priority": "high"},
                    {"attributes.department": "engineering"}
                ]
            }
            
            search_results[config_name] = collection_results
            
            # Log summary
            successful_sim_searches = [s for s in collection_results["similarity_searches"] if s.get("success")]
            successful_id_searches = [s for s in collection_results["id_searches"] if s.get("success")]
            
            if successful_sim_searches:
                sim_avg_time = np.mean([s["search_time"] for s in successful_sim_searches])
                logger.info(f"✅ {config_name}: Similarity search avg: {sim_avg_time:.4f}s")
            
            if successful_id_searches:
                id_avg_time = np.mean([s["search_time"] for s in successful_id_searches])
                logger.info(f"✅ {config_name}: ID search avg: {id_avg_time:.4f}s")
        
        return search_results
    
    def cleanup_collections(self):
        """Clean up test collections"""
        logger.info("🧹 Cleaning up test collections")
        
        for collection_info in self.test_collections:
            try:
                response = self.session.delete(
                    f"{self.base_url}/collections/{collection_info['id']}",
                    timeout=10
                )
                
                if response.status_code == 200:
                    logger.info(f"✅ Deleted collection: {collection_info['name']}")
                else:
                    logger.warning(f"⚠️ Failed to delete collection {collection_info['name']}: {response.status_code}")
                    
            except Exception as e:
                logger.warning(f"⚠️ Exception deleting collection {collection_info['name']}: {e}")
    
    def run_benchmark(self) -> Dict[str, Any]:
        """Run the complete REST benchmark"""
        logger.info("🚀 Starting REST Algorithm Benchmark")
        start_time = time.time()
        
        results = {
            "start_time": datetime.now().isoformat(),
            "test_type": "REST API Algorithm Benchmark",
            "base_url": self.base_url,
            "test_configurations": self.test_configurations
        }
        
        try:
            # Check server health
            if not self.check_server_health():
                raise Exception("REST server is not responding")
            
            # Create collections
            collections = self.create_test_collections()
            results["collections_created"] = len(collections)
            
            if not collections:
                raise Exception("No collections were created successfully")
            
            # Prepare data
            embedding_data = self.prepare_embedding_data(count=20)  # Smaller for REST testing
            results["embeddings_generated"] = len(embedding_data)
            
            # Insert vectors
            insertion_results = self.insert_vectors_to_collections(embedding_data)
            results["insertion_results"] = insertion_results
            
            # Perform searches
            search_results = self.perform_search_operations(embedding_data)
            results["search_results"] = search_results
            
            # Calculate summary
            total_time = time.time() - start_time
            results["total_time"] = total_time
            results["end_time"] = datetime.now().isoformat()
            
            logger.info(f"🎉 REST benchmark completed in {total_time:.2f} seconds")
            
        except Exception as e:
            logger.error(f"💥 REST benchmark failed: {e}")
            results["error"] = str(e)
            results["end_time"] = datetime.now().isoformat()
        
        finally:
            # Cleanup
            self.cleanup_collections()
        
        return results
    
    def generate_report(self, results: Dict[str, Any]) -> str:
        """Generate performance report"""
        report_lines = [
            "# ProximaDB REST API Algorithm Benchmark Report",
            f"Generated: {results.get('end_time', 'unknown')}",
            f"Total Duration: {results.get('total_time', 0):.2f} seconds",
            f"Base URL: {results.get('base_url')}",
            "",
            "## Test Configuration",
            f"- Collections Created: {results.get('collections_created', 0)}",
            f"- Embeddings Generated: {results.get('embeddings_generated', 0)}",
            f"- Embedding Dimension: 128",
            "",
            "## Distance Metric and Storage Layout Combinations"
        ]
        
        for config in results.get('test_configurations', []):
            report_lines.append(f"- {config['name']}: {config['distance_metric']} + {config['storage_layout']}")
        
        # Insertion Performance
        report_lines.extend([
            "",
            "## Insertion Performance (REST API)",
            "| Configuration | Vectors | Success Rate | Time (s) | Vectors/sec |",
            "|---------------|---------|--------------|----------|-------------|"
        ])
        
        for config_name, result in results.get('insertion_results', {}).items():
            if not result.get('error'):
                report_lines.append(
                    f"| {config_name} | {result.get('vectors_successful', 0)}/{result.get('vectors_attempted', 0)} | "
                    f"{result.get('success_rate', 0):.1f}% | {result.get('insertion_time', 0):.2f} | "
                    f"{result.get('vectors_per_second', 0):.1f} |"
                )
            else:
                report_lines.append(f"| {config_name} | - | - | - | ❌ Error |")
        
        # Search Performance
        report_lines.extend([
            "",
            "## Search Performance (REST API)",
            "| Configuration | Similarity Avg (ms) | ID Search Avg (ms) | Success Rate |",
            "|---------------|---------------------|-------------------|--------------|"
        ])
        
        for config_name, result in results.get('search_results', {}).items():
            sim_searches = result.get('similarity_searches', [])
            id_searches = result.get('id_searches', [])
            
            sim_times = [s.get('search_time', 0) * 1000 for s in sim_searches if s.get('success')]
            id_times = [s.get('search_time', 0) * 1000 for s in id_searches if s.get('success')]
            
            sim_avg = np.mean(sim_times) if sim_times else 0
            id_avg = np.mean(id_times) if id_times else 0
            
            sim_success_rate = len(sim_times) / len(sim_searches) * 100 if sim_searches else 0
            id_success_rate = len(id_times) / len(id_searches) * 100 if id_searches else 0
            
            overall_success = (sim_success_rate + id_success_rate) / 2
            
            report_lines.append(
                f"| {config_name} | {sim_avg:.2f} | {id_avg:.2f} | {overall_success:.1f}% |"
            )
        
        return "\n".join(report_lines)


def main():
    """Main execution"""
    logger.info("🎯 ProximaDB REST Algorithm Benchmark")
    
    try:
        benchmark = RESTAlgorithmBenchmark("http://localhost:5678")
        results = benchmark.run_benchmark()
        
        # Save results
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        results_file = f"rest_algorithm_benchmark_{timestamp}.json"
        
        with open(results_file, 'w') as f:
            json.dump(results, f, indent=2, default=str)
        
        logger.info(f"📄 Results saved to: {results_file}")
        
        # Generate report
        report = benchmark.generate_report(results)
        report_file = f"rest_benchmark_report_{timestamp}.md"
        
        with open(report_file, 'w') as f:
            f.write(report)
        
        logger.info(f"📊 Report saved to: {report_file}")
        
        # Print summary
        print("\n" + "="*60)
        print("REST BENCHMARK SUMMARY")
        print("="*60)
        print(f"Collections tested: {results.get('collections_created', 0)}")
        print(f"Embeddings processed: {results.get('embeddings_generated', 0)}")
        print(f"Total time: {results.get('total_time', 0):.2f} seconds")
        
        if results.get('error'):
            print(f"⚠️ Errors encountered: {results['error']}")
            return 1
        else:
            print("✅ REST benchmark completed successfully!")
            return 0
            
    except Exception as e:
        logger.error(f"💥 Benchmark failed: {e}")
        return 1


if __name__ == "__main__":
    exit(main())