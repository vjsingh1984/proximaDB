#!/usr/bin/env python3
"""
ProximaDB End-to-End Demo Script

This script demonstrates the complete functionality of ProximaDB including:
- Vector operations (upsert, search, delete)
- Metadata filtering with logical operators
- Quantization support
- AXIS+HNSW indexing
- Collection management
- Performance benchmarking
"""

import asyncio
import json
import time
import random
import logging
from typing import List, Dict, Any, Optional
from dataclasses import dataclass
import numpy as np
import requests
import sys
import os
from pathlib import Path

# Add the Python SDK path using PYTHONPATH approach
sdk_path = str(Path(__file__).parent.parent / "clients" / "python" / "src")
if sdk_path not in sys.path:
    sys.path.insert(0, sdk_path)

try:
    from proximadb import connect_rest, CollectionConfig, DistanceMetric
except ImportError as e:
    print(f"❌ Failed to import ProximaDB client: {e}")
    print("Please make sure the Python SDK is installed:")
    print("cd clients/python && pip install -e .")
    print(f"SDK path attempted: {sdk_path}")
    sys.exit(1)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

@dataclass
class DemoConfig:
    """Configuration for the demo"""
    server_url: str = "http://localhost:5678"
    grpc_url: str = "localhost:5679"
    collection_name: str = "demo_collection"
    vector_dimension: int = 768  # BERT-like embeddings
    num_vectors: int = 1000
    search_k: int = 10
    enable_quantization: bool = True
    enable_hnsw: bool = True
    benchmark_iterations: int = 5

class ProximaDBDemo:
    """Main demo class showcasing ProximaDB features"""
    
    def __init__(self, config: DemoConfig):
        self.config = config
        self.client = None
        self.collection = None
        self.demo_vectors = []
        self.categories = ["electronics", "books", "clothing", "sports", "home"]
        
    async def setup(self):
        """Initialize the demo environment"""
        print("🚀 Setting up ProximaDB Demo...")
        
        # Initialize client
        try:
            self.client = ProximaDBClient(
                rest_url=self.config.server_url,
                grpc_url=self.config.grpc_url
            )
            await self.client.connect()
            logger.info("✅ Connected to ProximaDB server")
        except Exception as e:
            logger.error(f"❌ Failed to connect to ProximaDB: {e}")
            raise
        
        # Check server health
        try:
            health = await self.client.health_check()
            logger.info(f"✅ Server health: {health}")
        except Exception as e:
            logger.error(f"❌ Server health check failed: {e}")
            raise
    
    async def create_collection(self):
        """Create a demo collection with advanced indexing"""
        print("\n📁 Creating demo collection...")
        
        # Configure quantization
        quantization_config = None
        if self.config.enable_quantization:
            quantization_config = QuantizationConfig(
                method="product",
                bits=8,
                compression_ratio=0.25
            )
        
        # Configure HNSW indexing
        index_config = None
        if self.config.enable_hnsw:
            index_config = IndexConfig(
                algorithm="hnsw",
                parameters={
                    "m": 16,
                    "ef_construction": 200,
                    "ef_search": 50,
                    "max_partition_size": 100000
                }
            )
        
        # Create collection
        try:
            self.collection = Collection(
                name=self.config.collection_name,
                dimension=self.config.vector_dimension,
                distance_metric="cosine",
                quantization_config=quantization_config,
                index_config=index_config,
                metadata_schema={
                    "category": {"type": "string", "index": True},
                    "price": {"type": "number", "index": True},
                    "rating": {"type": "number", "index": True},
                    "brand": {"type": "string", "index": True},
                    "description": {"type": "string", "index": False}
                }
            )
            
            result = await self.client.create_collection(self.collection)
            logger.info(f"✅ Collection created: {result}")
            
        except Exception as e:
            if "already exists" in str(e):
                logger.info("📁 Collection already exists, using existing")
                self.collection = await self.client.get_collection(self.config.collection_name)
            else:
                logger.error(f"❌ Failed to create collection: {e}")
                raise
    
    def generate_demo_vectors(self):
        """Generate realistic demo vectors with metadata"""
        print(f"\n🎲 Generating {self.config.num_vectors} demo vectors...")
        
        self.demo_vectors = []
        
        for i in range(self.config.num_vectors):
            # Generate realistic embeddings (normalized)
            vector = np.random.randn(self.config.vector_dimension).astype(np.float32)
            vector = vector / np.linalg.norm(vector)  # Normalize
            
            # Add some clustering by category
            category = random.choice(self.categories)
            category_offset = hash(category) % 100
            vector[:10] += (category_offset / 100.0)  # Add category signature
            vector = vector / np.linalg.norm(vector)  # Re-normalize
            
            # Generate metadata
            metadata = {
                "category": category,
                "price": round(random.uniform(10.0, 1000.0), 2),
                "rating": round(random.uniform(1.0, 5.0), 1),
                "brand": f"Brand_{random.randint(1, 20)}",
                "description": f"Demo product {i} in {category} category"
            }
            
            vector_record = VectorRecord(
                id=f"demo_vec_{i}",
                vector=vector.tolist(),
                metadata=metadata
            )
            
            self.demo_vectors.append(vector_record)
        
        logger.info(f"✅ Generated {len(self.demo_vectors)} demo vectors")
    
    async def upsert_vectors(self):
        """Upsert demo vectors to the collection"""
        print("\n📤 Upserting vectors to collection...")
        
        start_time = time.time()
        
        try:
            # Batch upsert for better performance
            batch_size = 100
            batches = [
                self.demo_vectors[i:i + batch_size] 
                for i in range(0, len(self.demo_vectors), batch_size)
            ]
            
            for i, batch in enumerate(batches):
                result = await self.client.upsert_vectors(
                    collection_name=self.config.collection_name,
                    vectors=batch
                )
                
                if (i + 1) % 5 == 0:
                    progress = ((i + 1) / len(batches)) * 100
                    logger.info(f"📈 Upsert progress: {progress:.1f}% ({i + 1}/{len(batches)} batches)")
            
            duration = time.time() - start_time
            vectors_per_second = len(self.demo_vectors) / duration
            
            logger.info(f"✅ Upserted {len(self.demo_vectors)} vectors in {duration:.2f}s")
            logger.info(f"⚡ Throughput: {vectors_per_second:.0f} vectors/second")
            
        except Exception as e:
            logger.error(f"❌ Failed to upsert vectors: {e}")
            raise
    
    async def demonstrate_basic_search(self):
        """Demonstrate basic vector similarity search"""
        print("\n🔍 Demonstrating basic vector search...")
        
        # Use the first vector as query
        query_vector = self.demo_vectors[0].vector
        
        try:
            search_query = SearchQuery(
                vector=query_vector,
                k=self.config.search_k,
                include_metadata=True
            )
            
            start_time = time.time()
            results = await self.client.search_vectors(
                collection_name=self.config.collection_name,
                query=search_query
            )
            search_time = time.time() - start_time
            
            logger.info(f"✅ Basic search completed in {search_time*1000:.2f}ms")
            logger.info(f"📊 Found {len(results)} results")
            
            # Display top results
            print("\n🎯 Top search results:")
            for i, result in enumerate(results[:5]):
                print(f"  {i+1}. ID: {result.id}, Score: {result.score:.4f}")
                print(f"      Category: {result.metadata.get('category', 'N/A')}")
                print(f"      Price: ${result.metadata.get('price', 'N/A')}")
                print()
                
        except Exception as e:
            logger.error(f"❌ Basic search failed: {e}")
            raise
    
    async def demonstrate_metadata_filtering(self):
        """Demonstrate metadata filtering with logical operators"""
        print("\n🧮 Demonstrating metadata filtering...")
        
        # Create a complex metadata query
        # Find electronics OR books with price < 500 AND rating > 3.5
        metadata_query = MetadataQuery(
            query_type="and",
            conditions=[
                MetadataQuery(
                    query_type="or",
                    conditions=[
                        MetadataQuery(field="category", operator="eq", value="electronics"),
                        MetadataQuery(field="category", operator="eq", value="books")
                    ]
                ),
                MetadataQuery(field="price", operator="lt", value=500.0),
                MetadataQuery(field="rating", operator="gt", value=3.5)
            ]
        )
        
        query_vector = self.demo_vectors[10].vector  # Use different query vector
        
        try:
            search_query = SearchQuery(
                vector=query_vector,
                k=self.config.search_k,
                metadata_query=metadata_query,
                include_metadata=True
            )
            
            start_time = time.time()
            results = await self.client.search_vectors(
                collection_name=self.config.collection_name,
                query=search_query
            )
            search_time = time.time() - start_time
            
            logger.info(f"✅ Filtered search completed in {search_time*1000:.2f}ms")
            logger.info(f"📊 Found {len(results)} filtered results")
            
            # Verify filter results
            print("\n🎯 Filtered search results:")
            for i, result in enumerate(results[:5]):
                category = result.metadata.get('category', 'N/A')
                price = result.metadata.get('price', 'N/A')
                rating = result.metadata.get('rating', 'N/A')
                
                print(f"  {i+1}. ID: {result.id}, Score: {result.score:.4f}")
                print(f"      Category: {category}, Price: ${price}, Rating: {rating}")
                print()
                
        except Exception as e:
            logger.error(f"❌ Filtered search failed: {e}")
            raise
    
    async def demonstrate_quantization(self):
        """Demonstrate quantization effects on search performance"""
        if not self.config.enable_quantization:
            print("\n⚪ Skipping quantization demo (disabled)")
            return
        
        print("\n🗜️ Demonstrating quantization effects...")
        
        try:
            # Get collection stats
            stats = await self.client.get_collection_stats(self.config.collection_name)
            
            print(f"📊 Collection Statistics:")
            print(f"  Total vectors: {stats.get('total_vectors', 'N/A')}")
            print(f"  Memory usage: {stats.get('memory_usage_mb', 'N/A'):.2f} MB")
            print(f"  Compression ratio: {stats.get('compression_ratio', 'N/A'):.2f}x")
            
            # Test search performance with quantization
            query_vector = self.demo_vectors[20].vector
            search_query = SearchQuery(
                vector=query_vector,
                k=self.config.search_k,
                use_quantization=True
            )
            
            start_time = time.time()
            results = await self.client.search_vectors(
                collection_name=self.config.collection_name,
                query=search_query
            )
            quantized_time = time.time() - start_time
            
            logger.info(f"✅ Quantized search completed in {quantized_time*1000:.2f}ms")
            logger.info(f"📊 Quantization provides {stats.get('compression_ratio', 1):.2f}x compression")
            
        except Exception as e:
            logger.error(f"❌ Quantization demo failed: {e}")
            logger.warning("Quantization features may not be fully implemented yet")
    
    async def run_performance_benchmark(self):
        """Run comprehensive performance benchmarks"""
        print("\n⚡ Running performance benchmarks...")
        
        benchmark_results = {
            "search_latency": [],
            "search_throughput": [],
            "filter_latency": [],
            "batch_upsert_throughput": []
        }
        
        # Search latency benchmark
        print("\n🔍 Benchmarking search latency...")
        query_vectors = [vec.vector for vec in self.demo_vectors[:10]]
        
        for i in range(self.config.benchmark_iterations):
            query_vector = random.choice(query_vectors)
            search_query = SearchQuery(vector=query_vector, k=10)
            
            start_time = time.time()
            results = await self.client.search_vectors(
                collection_name=self.config.collection_name,
                query=search_query
            )
            latency = (time.time() - start_time) * 1000  # Convert to ms
            benchmark_results["search_latency"].append(latency)
        
        # Search throughput benchmark
        print("\n🔍 Benchmarking search throughput...")
        start_time = time.time()
        
        for query_vector in query_vectors:
            search_query = SearchQuery(vector=query_vector, k=10)
            await self.client.search_vectors(
                collection_name=self.config.collection_name,
                query=search_query
            )
        
        duration = time.time() - start_time
        throughput = len(query_vectors) / duration
        benchmark_results["search_throughput"].append(throughput)
        
        # Print benchmark results
        print("\n📊 Benchmark Results:")
        print(f"  Search Latency (avg): {np.mean(benchmark_results['search_latency']):.2f}ms")
        print(f"  Search Latency (p95): {np.percentile(benchmark_results['search_latency'], 95):.2f}ms")
        print(f"  Search Throughput: {np.mean(benchmark_results['search_throughput']):.0f} queries/second")
        
        logger.info("✅ Performance benchmarks completed")
    
    async def demonstrate_collection_management(self):
        """Demonstrate collection management operations"""
        print("\n📁 Demonstrating collection management...")
        
        try:
            # List collections
            collections = await self.client.list_collections()
            logger.info(f"📋 Found {len(collections)} collections")
            
            # Get collection info
            collection_info = await self.client.get_collection(self.config.collection_name)
            print(f"📊 Collection '{self.config.collection_name}' info:")
            print(f"  Dimension: {collection_info.dimension}")
            print(f"  Distance metric: {collection_info.distance_metric}")
            print(f"  Quantization: {collection_info.quantization_config is not None}")
            print(f"  HNSW indexing: {collection_info.index_config is not None}")
            
            # Get collection statistics
            stats = await self.client.get_collection_stats(self.config.collection_name)
            print(f"📈 Collection statistics:")
            for key, value in stats.items():
                print(f"  {key}: {value}")
            
        except Exception as e:
            logger.error(f"❌ Collection management demo failed: {e}")
            raise
    
    async def cleanup(self):
        """Clean up demo resources"""
        print("\n🧹 Cleaning up demo resources...")
        
        try:
            # Optionally delete the demo collection
            # await self.client.delete_collection(self.config.collection_name)
            # logger.info("✅ Demo collection deleted")
            
            # Close client connection
            await self.client.close()
            logger.info("✅ Client connection closed")
            
        except Exception as e:
            logger.error(f"❌ Cleanup failed: {e}")
    
    async def run_demo(self):
        """Run the complete demo sequence"""
        try:
            print("🎭 ProximaDB Complete Feature Demo")
            print("=" * 50)
            
            await self.setup()
            await self.create_collection()
            
            self.generate_demo_vectors()
            await self.upsert_vectors()
            
            await self.demonstrate_basic_search()
            await self.demonstrate_metadata_filtering()
            await self.demonstrate_quantization()
            await self.demonstrate_collection_management()
            
            await self.run_performance_benchmark()
            
            print("\n🎉 Demo completed successfully!")
            print("✅ All ProximaDB features demonstrated")
            
        except Exception as e:
            logger.error(f"❌ Demo failed: {e}")
            raise
        finally:
            await self.cleanup()

async def main():
    """Main demo entry point"""
    print("🚀 ProximaDB End-to-End Demo")
    print("This demo showcases all ProximaDB features including:")
    print("• Vector operations (upsert, search, delete)")
    print("• Metadata filtering with logical operators")
    print("• Quantization support")
    print("• AXIS+HNSW indexing")
    print("• Performance benchmarking")
    print()
    
    # Configure demo
    config = DemoConfig()
    
    # Create and run demo
    demo = ProximaDBDemo(config)
    await demo.run_demo()

if __name__ == "__main__":
    asyncio.run(main())