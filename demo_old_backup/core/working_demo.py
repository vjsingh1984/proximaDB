#!/usr/bin/env python3
"""
ProximaDB Working Demo Script

This script demonstrates ProximaDB functionality using the working Python SDK:
- Vector operations (insert, search, delete)
- Collection management
- Performance benchmarking
- Metadata filtering
"""

import time
import random
import logging
import numpy as np
import sys
import os
from pathlib import Path

# Import ProximaDB SDK (requires PYTHONPATH to include clients/python/src)
from proximadb import (
    connect_rest, CollectionConfig, DistanceMetric,
    chunk_by_sentences, chunk_sliding_window, ChunkingStrategy
)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class WorkingProximaDBDemo:
    """Working demo class showcasing ProximaDB features"""
    
    def __init__(self, server_url="http://localhost:5678"):
        self.server_url = server_url
        self.client = None
        self.collection_name = f"working_demo_{int(time.time())}"
        self.demo_vectors = []
        self.demo_metadata = []
        self.categories = ["electronics", "books", "clothing", "sports", "home", "toys"]
        
    def step_1_connect_to_server(self):
        """Step 1: Connect to ProximaDB server"""
        print("🔗 Step 1: Connecting to ProximaDB...")
        
        try:
            self.client = connect_rest(self.server_url)
            logger.info("✅ Connected to ProximaDB successfully")
            
            # Try health check if available
            try:
                health = self.client.health()
                logger.info(f"💚 Server health: {health}")
            except:
                logger.info("ℹ️  Health endpoint not available (using gRPC interface)")
            
            return True
        except Exception as e:
            logger.error(f"❌ Failed to connect: {e}")
            return False
    
    def step_2_create_collection(self):
        """Step 2: Create a demo collection"""
        print(f"\n📁 Step 2: Creating collection '{self.collection_name}'...")
        
        try:
            config = CollectionConfig(
                dimension=256,
                distance_metric=DistanceMetric.COSINE,
                description="Working demo collection showcasing ProximaDB features"
            )
            
            collection = self.client.create_collection(self.collection_name, config)
            logger.info(f"✅ Collection created: {collection.name}")
            logger.info(f"   📏 Dimension: {collection.dimension}")
            logger.info(f"   📐 Distance metric: {collection.metric}")
            
            return True
        except Exception as e:
            logger.error(f"❌ Failed to create collection: {e}")
            return False
    
    def step_3_generate_demo_data(self, num_vectors=100):
        """Step 3: Generate realistic demo data"""
        print(f"\n🎲 Step 3: Generating {num_vectors} demo vectors...")
        
        self.demo_vectors = []
        self.demo_metadata = []
        vector_ids = []
        
        for i in range(num_vectors):
            # Generate vector with category-specific patterns
            category = random.choice(self.categories)
            category_seed = hash(category) % 1000
            
            vector = np.random.randn(256).astype(np.float32)
            # Add category signature
            vector[:10] += (category_seed / 1000.0)
            vector = vector / np.linalg.norm(vector)  # Normalize
            
            # Generate metadata
            metadata = {
                "category": category,
                "price": round(random.uniform(10.0, 2000.0), 2),
                "rating": round(random.uniform(1.0, 5.0), 1),
                "brand": f"Brand_{chr(65 + random.randint(0, 4))}",
                "name": f"{category.title()} Product {i}",
                "in_stock": random.choice([True, False]),
                "reviews": random.randint(0, 1000)
            }
            
            self.demo_vectors.append(vector.tolist())
            self.demo_metadata.append(metadata)
            vector_ids.append(f"demo_vec_{i}")
        
        self.vector_ids = vector_ids
        
        logger.info(f"✅ Generated {len(self.demo_vectors)} vectors")
        logger.info(f"📊 Categories: {list(set(m['category'] for m in self.demo_metadata))}")
        
        return True
    
    def step_4_insert_vectors(self):
        """Step 4: Insert vectors in batches"""
        print(f"\n📤 Step 4: Inserting {len(self.demo_vectors)} vectors...")
        
        try:
            batch_size = 25
            total_inserted = 0
            start_time = time.time()
            
            for i in range(0, len(self.demo_vectors), batch_size):
                batch_vectors = self.demo_vectors[i:i+batch_size]
                batch_ids = self.vector_ids[i:i+batch_size]
                batch_metadata = self.demo_metadata[i:i+batch_size]
                
                result = self.client.insert_vectors(
                    self.collection_name,
                    batch_vectors,
                    batch_ids,
                    metadata=batch_metadata
                )
                
                total_inserted += result.successful_count
                
                if (i // batch_size + 1) % 5 == 0:
                    progress = min(100, ((i + batch_size) / len(self.demo_vectors)) * 100)
                    logger.info(f"📈 Progress: {progress:.1f}% ({total_inserted} vectors inserted)")
            
            duration = time.time() - start_time
            logger.info(f"✅ Inserted {total_inserted} vectors in {duration:.2f}s")
            logger.info(f"⚡ Throughput: {total_inserted/duration:.0f} vectors/second")
            
            return True
        except Exception as e:
            logger.error(f"❌ Failed to insert vectors: {e}")
            return False
    
    def step_5_basic_search(self):
        """Step 5: Perform basic similarity search"""
        print("\n🔍 Step 5: Performing basic similarity search...")
        
        try:
            # Use a random vector as query
            query_idx = random.randint(0, len(self.demo_vectors) - 1)
            query_vector = self.demo_vectors[query_idx]
            query_metadata = self.demo_metadata[query_idx]
            
            logger.info(f"🎯 Searching for vectors similar to '{query_metadata['name']}'")
            logger.info(f"   Category: {query_metadata['category']}")
            
            start_time = time.time()
            results = self.client.search(self.collection_name, query_vector, k=10)
            search_time = time.time() - start_time
            
            logger.info(f"✅ Found {len(results)} results in {search_time*1000:.2f}ms")
            
            print("\n🎯 Top 5 search results:")
            for i, result in enumerate(results[:5]):
                metadata = getattr(result, 'metadata', {}) or {}
                print(f"   {i+1}. {result.id}")
                print(f"      📊 Score: {result.score:.4f}")
                print(f"      🏷️  Category: {metadata.get('category', 'N/A')}")
                print(f"      💰 Price: ${metadata.get('price', 'N/A')}")
                print(f"      ⭐ Rating: {metadata.get('rating', 'N/A')}/5.0")
                print()
            
            return True
        except Exception as e:
            logger.error(f"❌ Basic search failed: {e}")
            return False
    
    def step_6_category_analysis(self):
        """Step 6: Analyze data by categories"""
        print("\n📊 Step 6: Analyzing data by categories...")
        
        try:
            # Analyze category distribution
            category_counts = {}
            category_prices = {}
            
            for metadata in self.demo_metadata:
                category = metadata['category']
                price = metadata['price']
                
                category_counts[category] = category_counts.get(category, 0) + 1
                if category not in category_prices:
                    category_prices[category] = []
                category_prices[category].append(price)
            
            print("📈 Category Analysis:")
            for category in sorted(category_counts.keys()):
                count = category_counts[category]
                prices = category_prices[category]
                avg_price = sum(prices) / len(prices)
                min_price = min(prices)
                max_price = max(prices)
                
                print(f"   {category.title()}:")
                print(f"     📦 Items: {count}")
                print(f"     💰 Avg Price: ${avg_price:.2f}")
                print(f"     💵 Price Range: ${min_price:.2f} - ${max_price:.2f}")
                print()
            
            return True
        except Exception as e:
            logger.error(f"❌ Category analysis failed: {e}")
            return False
    
    def step_7_performance_benchmark(self):
        """Step 7: Run performance benchmarks"""
        print("\n⚡ Step 7: Running performance benchmarks...")
        
        try:
            # Test different k values
            k_values = [1, 5, 10, 20]
            benchmark_results = {}
            
            print("📊 Search performance with different k values:")
            for k in k_values:
                times = []
                query_vector = random.choice(self.demo_vectors)
                
                # Run multiple searches
                for _ in range(3):
                    start_time = time.time()
                    results = self.client.search(self.collection_name, query_vector, k=k)
                    search_time = time.time() - start_time
                    times.append(search_time * 1000)
                
                avg_time = sum(times) / len(times)
                benchmark_results[k] = avg_time
                print(f"   k={k:2d}: {avg_time:6.2f}ms average")
            
            # Batch query test
            print(f"\n🚀 Batch query performance:")
            num_queries = 20
            start_time = time.time()
            
            for i in range(num_queries):
                query_vector = self.demo_vectors[i % len(self.demo_vectors)]
                results = self.client.search(self.collection_name, query_vector, k=5)
            
            total_time = time.time() - start_time
            qps = num_queries / total_time
            
            logger.info(f"   Executed {num_queries} queries in {total_time:.2f}s")
            logger.info(f"   ⚡ Throughput: {qps:.1f} QPS")
            logger.info(f"   📊 Avg latency: {(total_time/num_queries)*1000:.2f}ms")
            
            return True
        except Exception as e:
            logger.error(f"❌ Performance benchmark failed: {e}")
            return False
    
    def step_8_text_chunking_demo(self):
        """Step 8: Demonstrate text chunking strategies"""
        print("\n📚 Step 8: Text Chunking Demonstration...")
        
        try:
            # Sample text document
            sample_text = """
            ProximaDB is a high-performance vector database designed for AI applications. 
            It provides efficient storage and retrieval of high-dimensional vectors. 
            The system supports multiple indexing algorithms including HNSW and IVF.
            
            With ProximaDB, you can build semantic search applications that understand meaning.
            The database handles billions of vectors while maintaining sub-second query times.
            It's perfect for recommendation systems, similarity search, and RAG applications.
            """
            
            # Demonstrate sentence chunking
            print("📝 Sentence-based chunking:")
            sentence_chunks = chunk_by_sentences(sample_text, chunk_size=150)
            print(f"   Created {len(sentence_chunks)} sentence chunks")
            for i, chunk in enumerate(sentence_chunks[:2]):
                print(f"   Chunk {i+1}: {chunk.text[:60]}...")
            
            # Demonstrate sliding window chunking
            print("\n🔄 Sliding window chunking:")
            window_chunks = chunk_sliding_window(sample_text, window_size=100, overlap=20)
            print(f"   Created {len(window_chunks)} sliding window chunks")
            print(f"   Window size: 100 chars, Overlap: 20 chars")
            
            # Store chunks as vectors (mock embeddings for demo)
            print("\n💾 Storing text chunks as vectors...")
            chunk_vectors = []
            chunk_ids = []
            chunk_metadata = []
            
            for i, chunk in enumerate(sentence_chunks[:5]):
                # Mock embedding (in production, use real embeddings)
                vector = np.random.randn(256).astype(np.float32)
                vector = vector / np.linalg.norm(vector)
                
                chunk_vectors.append(vector.tolist())
                chunk_ids.append(f"text_chunk_{i}")
                chunk_metadata.append({
                    "chunk_text": chunk.text[:100],
                    "chunk_type": "sentence",
                    "position": f"{chunk.start_pos}-{chunk.end_pos}"
                })
            
            # Insert text chunks
            result = self.client.insert_vectors(
                self.collection_name,
                chunk_vectors,
                chunk_ids,
                metadata=chunk_metadata
            )
            
            logger.info(f"✅ Stored {result.successful_count} text chunks as vectors")
            
            return True
        except Exception as e:
            logger.error(f"❌ Text chunking demo failed: {e}")
            return False
    
    def step_9_collection_info(self):
        """Step 9: Get collection information"""
        print("\n📋 Step 9: Getting collection information...")
        
        try:
            collection_info = self.client.get_collection(self.collection_name)
            
            print("📊 Collection Information:")
            print(f"   🆔 ID: {collection_info.id}")
            print(f"   📝 Name: {collection_info.name}")
            print(f"   📏 Dimension: {collection_info.dimension}")
            print(f"   📐 Distance Metric: {collection_info.metric}")
            print(f"   📊 Status: {collection_info.status}")
            
            # Dataset statistics
            print(f"\n📈 Dataset Statistics:")
            print(f"   📦 Total vectors: {len(self.demo_vectors)}")
            print(f"   🏷️  Categories: {len(set(m['category'] for m in self.demo_metadata))}")
            
            prices = [m['price'] for m in self.demo_metadata]
            print(f"   💰 Price range: ${min(prices):.2f} - ${max(prices):.2f}")
            print(f"   💵 Average price: ${sum(prices)/len(prices):.2f}")
            
            return True
        except Exception as e:
            logger.error(f"❌ Collection info failed: {e}")
            return False
    
    def cleanup(self):
        """Clean up demo resources"""
        print(f"\n🧹 Cleaning up...")
        
        try:
            self.client.delete_collection(self.collection_name)
            logger.info(f"✅ Deleted collection '{self.collection_name}'")
            return True
        except Exception as e:
            logger.warning(f"⚠️  Cleanup failed: {e}")
            return False
    
    def run_full_demo(self):
        """Run the complete demo"""
        print("🎭 ProximaDB Working Demo")
        print("=" * 60)
        print("This demo showcases:")
        print("• Vector database operations")
        print("• Similarity search")
        print("• Data analysis")
        print("• Performance benchmarking")
        print("• Text chunking strategies")
        print("=" * 60)
        
        steps = [
            self.step_1_connect_to_server,
            self.step_2_create_collection,
            lambda: self.step_3_generate_demo_data(100),
            self.step_4_insert_vectors,
            self.step_5_basic_search,
            self.step_6_category_analysis,
            self.step_7_performance_benchmark,
            self.step_8_text_chunking_demo,
            self.step_9_collection_info
        ]
        
        success_count = 0
        
        for i, step in enumerate(steps, 1):
            print(f"\n{'='*60}")
            try:
                success = step()
                if success:
                    success_count += 1
                    logger.info(f"✅ Step {i} completed successfully")
                else:
                    logger.error(f"❌ Step {i} failed")
                    break
            except Exception as e:
                logger.error(f"💥 Step {i} crashed: {e}")
                break
            
            # Brief pause between steps
            if i < len(steps):
                time.sleep(1)
        
        # Cleanup
        self.cleanup()
        
        print(f"\n{'='*60}")
        print(f"🎉 Demo Summary")
        logger.info(f"✅ Completed {success_count}/{len(steps)} steps successfully")
        
        if success_count == len(steps):
            print(f"🏆 ProximaDB demo completed successfully!")
            print(f"💡 Demonstrated features:")
            print(f"   • Collection creation and management")
            print(f"   • Vector insertion with metadata")
            print(f"   • Similarity search operations")
            print(f"   • Data analysis and categorization")
            print(f"   • Performance benchmarking")
        else:
            print(f"⚠️  Demo partially completed ({success_count}/{len(steps)} steps)")
        
        return success_count == len(steps)

def main():
    """Main entry point"""
    print("🚀 Starting ProximaDB Working Demo...")
    
    demo = WorkingProximaDBDemo()
    success = demo.run_full_demo()
    
    print(f"\n{'='*60}")
    if success:
        print(f"🎊 ProximaDB demo completed successfully!")
        print(f"✨ All features demonstrated and working!")
    else:
        print(f"😞 Demo encountered issues")
    
    return success

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)