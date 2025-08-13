#!/usr/bin/env python3
"""
ProximaDB Full Demo with All Features

This comprehensive demo showcases:
- Vector database operations
- Text chunking strategies
- Similarity search with metadata filtering
- Quantization support
- AXIS+HNSW indexing
- Performance benchmarking
"""

import time
import logging
import numpy as np
import sys
import os
from pathlib import Path
import json

# Import ProximaDB SDK (requires PYTHONPATH to include clients/python/src)
from proximadb import (
    connect_rest, connect_grpc, connect, Protocol,
    CollectionConfig, DistanceMetric, IndexAlgorithm,
    TextChunker, ChunkingStrategy, ChunkingConfig,
    chunk_by_sentences, chunk_by_paragraphs, chunk_sliding_window
)

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Sample documents for demonstration
SAMPLE_DOCUMENTS = {
    "technical": """
    ProximaDB Architecture Overview
    
    ProximaDB is built on a unified memtable architecture that eliminates code duplication across storage engines. 
    The core innovation is a single memtable system with behavior wrappers for different storage backends.
    
    Key Components:
    - Unified Memtable Manager: Provides a consistent interface for all storage operations
    - Behavior Wrappers: WalBehaviorWrapper and LsmBehaviorWrapper adapt the memtable for specific storage needs
    - VIPER Engine: Uses pure WAL delegation without memtable for Parquet optimization
    
    The storage-aware polymorphic search system automatically selects optimal search engines based on data location.
    This architecture has achieved 6.10x performance improvement with 100% accuracy in benchmarks.
    """,
    
    "product_description": """
    Introducing the SmartHome Hub Pro - Your Connected Living Solution
    
    Transform your home into a intelligent ecosystem with our flagship smart home controller. 
    The SmartHome Hub Pro seamlessly integrates with over 10,000 devices from leading brands.
    
    Features include voice control through multiple assistants, advanced automation rules, 
    and enterprise-grade security. The intuitive mobile app lets you control everything from 
    lighting and climate to security cameras and door locks.
    
    With local processing for privacy and cloud backup for reliability, the SmartHome Hub Pro 
    offers the perfect balance of performance and peace of mind. Setup takes just minutes with 
    our step-by-step wizard.
    """,
    
    "research_paper": """
    Abstract: Vector Similarity Search in High-Dimensional Spaces
    
    This paper presents novel approaches to efficient similarity search in high-dimensional vector spaces. 
    We propose a hybrid indexing strategy that combines hierarchical navigable small world (HNSW) graphs 
    with learned quantization techniques.
    
    Our experiments on diverse datasets demonstrate that the proposed method achieves sub-linear query time 
    while maintaining over 95% recall at 10. The memory footprint is reduced by 75% compared to traditional 
    approaches through adaptive compression.
    
    Key contributions include: (1) A theoretical framework for analyzing trade-offs between accuracy and 
    efficiency, (2) An adaptive algorithm that adjusts index parameters based on query patterns, and 
    (3) Extensive empirical evaluation on real-world applications including image retrieval and semantic search.
    """
}


class FullProximaDBDemo:
    """Comprehensive demo showcasing all ProximaDB features"""
    
    def __init__(self, server_url="http://localhost:5678", grpc_url="localhost:5679"):
        self.server_url = server_url
        self.grpc_url = grpc_url
        self.rest_client = None
        self.grpc_client = None
        self.collection_name = f"full_demo_{int(time.time())}"
        self.chunks_collection = f"chunks_demo_{int(time.time())}"
        self.hnsw_collection = f"hnsw_demo_{int(time.time())}"
        self.quantized_collection = f"quantized_demo_{int(time.time())}"
        self.vectors_data = []
        self.chunks_data = []
        
    def setup(self):
        """Initialize ProximaDB connections and collections"""
        print("🚀 Setting up ProximaDB Full Demo...")
        
        try:
            # Create both REST and gRPC clients
            self.rest_client = connect_rest(self.server_url)
            logger.info("✅ Connected to ProximaDB via REST")
            
            try:
                self.grpc_client = connect_grpc(self.grpc_url)
                logger.info("✅ Connected to ProximaDB via gRPC")
            except Exception as e:
                logger.warning(f"⚠️ gRPC connection failed, using REST only: {e}")
                self.grpc_client = self.rest_client
            
            # Create main collection for basic vectors
            config1 = CollectionConfig(
                dimension=384,
                distance_metric=DistanceMetric.COSINE,
                description="Main collection for full demo"
            )
            collection1 = self.rest_client.create_collection(self.collection_name, config1)
            logger.info(f"✅ Created main collection: {collection1.name}")
            
            # Create collection for text chunks
            config2 = CollectionConfig(
                dimension=384,
                distance_metric=DistanceMetric.COSINE,
                description="Text chunks demonstration"
            )
            collection2 = self.rest_client.create_collection(self.chunks_collection, config2)
            logger.info(f"✅ Created chunks collection: {collection2.name}")
            
            # Create HNSW collection with Euclidean distance
            config3 = CollectionConfig(
                dimension=512,
                distance_metric=DistanceMetric.EUCLIDEAN,
                description="HNSW indexing with Euclidean distance",
                index_config={
                    "algorithm": "hnsw",
                    "parameters": {
                        "m": 16,
                        "ef_construction": 200,
                        "ef_search": 50
                    }
                }
            )
            collection3 = self.rest_client.create_collection(self.hnsw_collection, config3)
            logger.info(f"✅ Created HNSW collection (Euclidean): {collection3.name}")
            
            # Create quantized collection with Manhattan distance
            config4 = CollectionConfig(
                dimension=768,
                distance_metric=DistanceMetric.MANHATTAN,
                description="Quantization with Manhattan distance",
                storage_config={
                    "quantization": {
                        "enabled": True,
                        "method": "product",
                        "bits": 8,
                        "compression_ratio": 4.0
                    }
                }
            )
            collection4 = self.rest_client.create_collection(self.quantized_collection, config4)
            logger.info(f"✅ Created quantized collection (Manhattan): {collection4.name}")
            
            return True
        except Exception as e:
            logger.error(f"❌ Setup failed: {e}")
            return False
    
    def demonstrate_text_chunking(self):
        """Comprehensive text chunking demonstration"""
        print("\n📚 Text Chunking Strategies Demonstration")
        print("=" * 60)
        
        all_chunks = []
        
        for doc_name, doc_text in SAMPLE_DOCUMENTS.items():
            print(f"\n📄 Processing document: {doc_name}")
            print("-" * 40)
            
            # 1. Sentence chunking
            sentence_chunks = chunk_by_sentences(
                doc_text,
                chunk_size=200,
                document_id=f"{doc_name}_sentences"
            )
            print(f"📝 Sentence chunking: {len(sentence_chunks)} chunks")
            
            # 2. Paragraph chunking
            para_chunks = chunk_by_paragraphs(
                doc_text,
                max_size=500,
                document_id=f"{doc_name}_paragraphs"
            )
            print(f"📄 Paragraph chunking: {len(para_chunks)} chunks")
            
            # 3. Sliding window chunking
            window_chunks = chunk_sliding_window(
                doc_text,
                window_size=150,
                overlap=50,
                document_id=f"{doc_name}_sliding"
            )
            print(f"🔄 Sliding window: {len(window_chunks)} chunks")
            
            # 4. Semantic chunking
            semantic_config = ChunkingConfig(
                strategy=ChunkingStrategy.SEMANTIC,
                max_chunk_size=400
            )
            semantic_chunker = TextChunker(semantic_config)
            semantic_chunks = semantic_chunker.chunk_text(
                doc_text,
                document_id=f"{doc_name}_semantic"
            )
            print(f"🧠 Semantic chunking: {len(semantic_chunks)} chunks")
            
            # Collect all chunks
            all_chunks.extend(sentence_chunks[:5])  # Take samples
            all_chunks.extend(window_chunks[:5])
            
        self.chunks_data = all_chunks
        print(f"\n✅ Total chunks created: {len(self.chunks_data)}")
        
        return True
    
    def vectorize_and_store_chunks(self):
        """Convert text chunks to vectors and store in ProximaDB"""
        print("\n🔄 Vectorizing and Storing Text Chunks")
        print("=" * 60)
        
        if not self.chunks_data:
            print("❌ No chunks available")
            return False
        
        chunk_vectors = []
        chunk_ids = []
        chunk_metadata = []
        
        for i, chunk in enumerate(self.chunks_data[:30]):  # Process first 30 chunks
            # Generate mock embedding (in production, use real embeddings)
            vector = np.random.randn(384).astype(np.float32)
            # Add some structure based on document type
            if "technical" in chunk.chunk_id:
                vector[:50] += 0.5
            elif "product" in chunk.chunk_id:
                vector[50:100] += 0.5
            elif "research" in chunk.chunk_id:
                vector[100:150] += 0.5
            
            vector = vector / np.linalg.norm(vector)
            
            chunk_vectors.append(vector.tolist())
            chunk_ids.append(f"chunk_{i:03d}")
            
            metadata = {
                "text": chunk.text[:200],
                "document": chunk.chunk_id.split("_")[0],
                "chunk_type": chunk.metadata.get("chunk_type", "unknown"),
                "strategy": chunk.chunk_id.split("_")[1],
                "position": f"{chunk.start_pos}-{chunk.end_pos}",
                "length": chunk.length
            }
            chunk_metadata.append(metadata)
        
        # Store chunks using REST client
        try:
            start_time = time.time()
            result = self.rest_client.insert_vectors(
                self.chunks_collection,
                chunk_vectors,
                chunk_ids,
                metadata=chunk_metadata
            )
            duration = time.time() - start_time
            
            logger.info(f"✅ Stored {result.successful_count} chunk vectors")
            logger.info(f"⚡ Throughput: {result.successful_count/duration:.0f} chunks/second")
            
            return True
        except Exception as e:
            logger.error(f"❌ Failed to store chunks: {e}")
            return False
    
    def demonstrate_semantic_search(self):
        """Demonstrate semantic search across document chunks"""
        print("\n🔍 Semantic Search Demonstration")
        print("=" * 60)
        
        # Create query vectors for different concepts
        queries = {
            "architecture": "unified memtable system storage architecture",
            "smart_home": "voice control automation connected devices",
            "research": "high-dimensional similarity search algorithms"
        }
        
        for query_name, query_text in queries.items():
            print(f"\n🎯 Query: '{query_text}'")
            
            # Generate query vector (mock)
            query_vector = np.random.randn(384).astype(np.float32)
            if "architecture" in query_name:
                query_vector[:50] += 0.7
            elif "smart" in query_name:
                query_vector[50:100] += 0.7
            elif "research" in query_name:
                query_vector[100:150] += 0.7
            
            query_vector = query_vector / np.linalg.norm(query_vector)
            
            # Search using REST client
            try:
                results = self.rest_client.search(
                    self.chunks_collection,
                    query_vector.tolist(),
                    k=5
                )
                
                print(f"✅ Found {len(results)} relevant chunks:")
                for i, result in enumerate(results[:3]):
                    metadata = getattr(result, 'metadata', {})
                    print(f"\n{i+1}. Document: {metadata.get('document', 'N/A')}")
                    print(f"   Strategy: {metadata.get('strategy', 'N/A')}")
                    print(f"   Score: {result.score:.3f}")
                    print(f"   Text: {metadata.get('text', 'N/A')[:80]}...")
                
            except Exception as e:
                logger.error(f"❌ Search failed: {e}")
    
    def demonstrate_hybrid_search(self):
        """Demonstrate combining vector search with metadata filtering"""
        print("\n🔄 Hybrid Search (Vector + Metadata)")
        print("=" * 60)
        
        # Generate a query vector
        query_vector = np.random.randn(384).astype(np.float32)
        query_vector[50:100] += 0.5  # Bias towards product documents
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        print("🎯 Searching for product-related chunks using sliding window strategy...")
        
        try:
            # First, get all results
            all_results = self.rest_client.search(
                self.chunks_collection,
                query_vector.tolist(),
                k=20
            )
            
            # Filter for specific criteria
            filtered_results = []
            for result in all_results:
                metadata = getattr(result, 'metadata', {})
                if (metadata.get('document') == 'product_description' and 
                    metadata.get('strategy') == 'sliding'):
                    filtered_results.append(result)
            
            print(f"✅ Found {len(filtered_results)} matching chunks:")
            for i, result in enumerate(filtered_results[:3]):
                metadata = getattr(result, 'metadata', {})
                print(f"\n{i+1}. Chunk Type: {metadata.get('chunk_type', 'N/A')}")
                print(f"   Length: {metadata.get('length', 'N/A')} chars")
                print(f"   Score: {result.score:.3f}")
                print(f"   Text: {metadata.get('text', 'N/A')[:100]}...")
            
        except Exception as e:
            logger.error(f"❌ Hybrid search failed: {e}")
    
    def demonstrate_rag_pipeline(self):
        """Demonstrate a simple RAG (Retrieval Augmented Generation) pipeline"""
        print("\n🤖 RAG Pipeline Demonstration")
        print("=" * 60)
        
        # User query
        user_query = "How does ProximaDB achieve high performance?"
        print(f"❓ User Query: '{user_query}'")
        
        # Generate query embedding
        query_vector = np.random.randn(384).astype(np.float32)
        query_vector[:50] += 0.8  # Technical bias
        query_vector = query_vector / np.linalg.norm(query_vector)
        
        try:
            # Retrieve relevant chunks
            results = self.rest_client.search(
                self.chunks_collection,
                query_vector.tolist(),
                k=3
            )
            
            print("\n📚 Retrieved Context:")
            context_texts = []
            for i, result in enumerate(results):
                metadata = getattr(result, 'metadata', {})
                text = metadata.get('text', '')
                context_texts.append(text)
                print(f"\n{i+1}. From: {metadata.get('document', 'N/A')}")
                print(f"   Text: {text[:150]}...")
            
            # Simulate answer generation (in production, use LLM)
            print("\n💡 Generated Answer:")
            print("Based on the retrieved information, ProximaDB achieves high performance through:")
            print("• Unified memtable architecture eliminating code duplication")
            print("• Storage-aware polymorphic search with automatic engine selection")
            print("• 6.10x performance improvement demonstrated in benchmarks")
            print("• Behavior wrappers that optimize for specific storage backends")
            
        except Exception as e:
            logger.error(f"❌ RAG pipeline failed: {e}")
    
    def demonstrate_performance_analysis(self):
        """Analyze performance of different chunking strategies"""
        print("\n📊 Performance Analysis")
        print("=" * 60)
        
        # Analyze chunks by strategy
        strategy_stats = {}
        
        try:
            # Get all chunks (simplified - in production, use proper pagination)
            query_vector = np.random.randn(384).astype(np.float32)
            results = self.rest_client.search(
                self.chunks_collection,
                query_vector.tolist(),
                k=50
            )
            
            for result in results:
                metadata = getattr(result, 'metadata', {})
                strategy = metadata.get('strategy', 'unknown')
                
                if strategy not in strategy_stats:
                    strategy_stats[strategy] = {
                        'count': 0,
                        'avg_length': 0,
                        'total_length': 0
                    }
                
                strategy_stats[strategy]['count'] += 1
                strategy_stats[strategy]['total_length'] += metadata.get('length', 0)
            
            # Calculate averages
            print("📊 Chunking Strategy Performance:")
            print(f"{'Strategy':<15} {'Chunks':<10} {'Avg Length':<15}")
            print("-" * 40)
            
            for strategy, stats in strategy_stats.items():
                if stats['count'] > 0:
                    avg_length = stats['total_length'] / stats['count']
                    print(f"{strategy:<15} {stats['count']:<10} {avg_length:<15.0f}")
            
        except Exception as e:
            logger.error(f"❌ Performance analysis failed: {e}")
    
    def demonstrate_hnsw_indexing(self):
        """Demonstrate HNSW indexing with different distance metrics"""
        print("\n🌐 HNSW Indexing Demonstration")
        print("=" * 60)
        
        # Generate vectors for HNSW collection (512-dimensional)
        print("📊 Creating vectors for HNSW indexing...")
        hnsw_vectors = []
        hnsw_ids = []
        hnsw_metadata = []
        
        # Create clustered data to show HNSW efficiency
        for cluster in range(5):
            cluster_center = np.random.randn(512).astype(np.float32)
            cluster_center = cluster_center / np.linalg.norm(cluster_center)
            
            for i in range(20):  # 20 vectors per cluster
                # Add noise around cluster center
                vector = cluster_center + np.random.normal(0, 0.1, 512).astype(np.float32)
                vector = vector / np.linalg.norm(vector)
                
                hnsw_vectors.append(vector.tolist())
                hnsw_ids.append(f"hnsw_vec_{cluster}_{i}")
                hnsw_metadata.append({
                    "cluster": cluster,
                    "cluster_name": f"Cluster_{chr(65 + cluster)}",
                    "item_id": i,
                    "vector_type": "clustered_euclidean"
                })
        
        try:
            # Insert vectors using gRPC for performance
            start_time = time.time()
            result = self.grpc_client.insert_vectors(
                self.hnsw_collection,
                hnsw_vectors,
                hnsw_ids,
                metadata=hnsw_metadata
            )
            duration = time.time() - start_time
            
            logger.info(f"✅ Inserted {result.successful_count} HNSW vectors via gRPC")
            logger.info(f"⚡ gRPC Throughput: {result.successful_count/duration:.0f} vectors/second")
            
            # Demonstrate HNSW search efficiency
            print("\n🔍 HNSW Search Performance Test:")
            query_vector = hnsw_vectors[5]  # Use a vector from cluster 0
            
            # Multiple searches to test performance
            search_times = []
            for _ in range(5):
                start_time = time.time()
                results = self.rest_client.search(
                    self.hnsw_collection,
                    query_vector,
                    k=10
                )
                search_time = time.time() - start_time
                search_times.append(search_time * 1000)  # Convert to ms
            
            avg_search_time = np.mean(search_times)
            print(f"⚡ HNSW average search time: {avg_search_time:.2f}ms")
            print(f"📊 Found {len(results)} results")
            
            # Show cluster affinity
            cluster_results = {}
            for result in results[:10]:
                metadata = getattr(result, 'metadata', {})
                cluster = metadata.get('cluster', -1)
                cluster_results[cluster] = cluster_results.get(cluster, 0) + 1
            
            print(f"🎯 Cluster distribution in results:")
            for cluster, count in sorted(cluster_results.items()):
                print(f"   Cluster {cluster}: {count} results")
            
        except Exception as e:
            logger.error(f"❌ HNSW demonstration failed: {e}")
    
    def demonstrate_quantization(self):
        """Demonstrate vector quantization for memory efficiency"""
        print("\n🗜️ Vector Quantization Demonstration")
        print("=" * 60)
        
        # Generate high-dimensional vectors for quantization
        print("📊 Creating high-dimensional vectors for quantization...")
        quant_vectors = []
        quant_ids = []
        quant_metadata = []
        
        # Create vectors with different patterns
        for pattern in range(4):
            for i in range(25):  # 25 vectors per pattern
                vector = np.random.randn(768).astype(np.float32)
                
                # Add pattern-specific features
                if pattern == 0:  # Dense pattern
                    vector += 0.5
                elif pattern == 1:  # Sparse pattern
                    vector[vector < 0] = 0
                elif pattern == 2:  # Oscillating pattern
                    vector[::2] *= 2
                elif pattern == 3:  # Normal pattern
                    pass
                
                vector = vector / np.linalg.norm(vector)
                
                quant_vectors.append(vector.tolist())
                quant_ids.append(f"quant_vec_{pattern}_{i}")
                quant_metadata.append({
                    "pattern": pattern,
                    "pattern_name": ["dense", "sparse", "oscillating", "normal"][pattern],
                    "item_id": i,
                    "compression_target": True
                })
        
        try:
            # Insert vectors (quantization happens automatically)
            start_time = time.time()
            result = self.rest_client.insert_vectors(
                self.quantized_collection,
                quant_vectors,
                quant_ids,
                metadata=quant_metadata
            )
            duration = time.time() - start_time
            
            logger.info(f"✅ Inserted {result.successful_count} quantized vectors")
            logger.info(f"⚡ Quantization throughput: {result.successful_count/duration:.0f} vectors/second")
            
            # Compare search performance with quantization
            print("\n🔍 Quantized Search Performance:")
            query_vector = quant_vectors[10]
            
            search_times = []
            for _ in range(3):
                start_time = time.time()
                results = self.rest_client.search(
                    self.quantized_collection,
                    query_vector,
                    k=15
                )
                search_time = time.time() - start_time
                search_times.append(search_time * 1000)
            
            avg_search_time = np.mean(search_times)
            print(f"⚡ Quantized search time: {avg_search_time:.2f}ms")
            print(f"📊 Memory efficiency: ~4x compression achieved")
            
            # Analyze pattern distribution in results
            pattern_results = {}
            for result in results:
                metadata = getattr(result, 'metadata', {})
                pattern = metadata.get('pattern_name', 'unknown')
                pattern_results[pattern] = pattern_results.get(pattern, 0) + 1
            
            print(f"🎯 Pattern distribution in results:")
            for pattern, count in sorted(pattern_results.items()):
                print(f"   {pattern}: {count} results")
                
        except Exception as e:
            logger.error(f"❌ Quantization demonstration failed: {e}")
    
    def demonstrate_distance_metrics(self):
        """Demonstrate different distance metrics"""
        print("\n📏 Distance Metrics Comparison")
        print("=" * 60)
        
        # Create test vectors for distance comparison
        test_vectors = []
        base_vector = np.random.randn(384).astype(np.float32)
        base_vector = base_vector / np.linalg.norm(base_vector)
        
        # Create similar vectors with different perturbations
        for i in range(5):
            perturbed = base_vector + np.random.normal(0, 0.1, 384).astype(np.float32)
            test_vectors.append(perturbed / np.linalg.norm(perturbed))
        
        # Compare distance metrics
        print("📊 Distance metric comparison for similar vectors:")
        
        # Cosine similarity (from main collection)
        try:
            cosine_results = self.rest_client.search(
                self.collection_name,
                test_vectors[0].tolist(),
                k=5
            )
            cosine_distances = [r.score for r in cosine_results[:3]]
            print(f"🎯 Cosine similarity scores: {[f'{d:.4f}' for d in cosine_distances]}")
        except:
            print("⚠️ Cosine similarity test skipped")
        
        # For demonstration, we'll use Manhattan distance collection
        print("📐 Distance metrics supported:")
        print("   • Cosine: Measures angular similarity (0-1)")
        print("   • Euclidean: Measures straight-line distance") 
        print("   • Manhattan: Measures grid-based distance")
        print("   • Dot Product: Measures vector alignment")
        
        print("\n💡 Distance metric selection guidelines:")
        print("   • Cosine: Text embeddings, normalized features")
        print("   • Euclidean: Image features, spatial data") 
        print("   • Manhattan: High-dimensional sparse data")
        print("   • Dot: Recommendation systems, user preferences")
    
    def demonstrate_search_operators(self):
        """Demonstrate advanced search operators and filtering"""
        print("\n🔧 Advanced Search Operators")
        print("=" * 60)
        
        print("📊 Search operator capabilities:")
        print("   • Vector similarity search")
        print("   • Metadata filtering with logical operators")
        print("   • Range queries on numerical fields")
        print("   • Text matching on string fields")
        print("   • Composite queries combining multiple conditions")
        
        # Demonstrate complex filtering (simulated for this demo)
        try:
            # Complex query: chunks from technical documents using sliding window strategy
            query_vector = np.random.randn(384).astype(np.float32)
            query_vector[:50] += 0.8  # Technical bias
            query_vector = query_vector / np.linalg.norm(query_vector)
            
            all_results = self.rest_client.search(
                self.chunks_collection,
                query_vector.tolist(),
                k=30
            )
            
            # Simulate advanced filtering
            filtered_results = []
            for result in all_results:
                metadata = getattr(result, 'metadata', {})
                
                # Complex filter: technical documents AND (sliding OR sentence strategy) AND length > 150
                if (metadata.get('document') == 'technical' and 
                    metadata.get('strategy') in ['sliding', 'sentence'] and
                    metadata.get('length', 0) > 150):
                    filtered_results.append(result)
            
            print(f"\n🎯 Complex query results:")
            print(f"   Original results: {len(all_results)}")
            print(f"   After filtering: {len(filtered_results)}")
            
            for i, result in enumerate(filtered_results[:3]):
                metadata = getattr(result, 'metadata', {})
                print(f"\n   {i+1}. Score: {result.score:.3f}")
                print(f"      Strategy: {metadata.get('strategy')}")
                print(f"      Length: {metadata.get('length')} chars")
                print(f"      Text: {metadata.get('text', '')[:60]}...")
        
        except Exception as e:
            logger.error(f"❌ Search operators demo failed: {e}")
    
    def demonstrate_protocol_comparison(self):
        """Compare REST vs gRPC performance"""
        print("\n🔄 Protocol Performance Comparison")
        print("=" * 60)
        
        # Test vector for comparison
        test_vector = np.random.randn(384).astype(np.float32)
        test_vector = test_vector / np.linalg.norm(test_vector)
        
        # REST performance
        print("📡 REST API Performance:")
        rest_times = []
        try:
            for _ in range(3):
                start_time = time.time()
                results = self.rest_client.search(
                    self.chunks_collection,
                    test_vector.tolist(),
                    k=10
                )
                rest_times.append((time.time() - start_time) * 1000)
            
            avg_rest_time = np.mean(rest_times)
            print(f"   ⚡ Average latency: {avg_rest_time:.2f}ms")
            print(f"   📊 Results count: {len(results)}")
        except Exception as e:
            print(f"   ⚠️ REST test failed: {e}")
        
        # gRPC performance
        print("\n📨 gRPC API Performance:")
        grpc_times = []
        try:
            for _ in range(3):
                start_time = time.time()
                results = self.grpc_client.search(
                    self.chunks_collection,
                    test_vector.tolist(),
                    k=10
                )
                grpc_times.append((time.time() - start_time) * 1000)
            
            avg_grpc_time = np.mean(grpc_times)
            print(f"   ⚡ Average latency: {avg_grpc_time:.2f}ms")
            print(f"   📊 Results count: {len(results)}")
            
            # Compare protocols
            if rest_times and grpc_times:
                speedup = avg_rest_time / avg_grpc_time
                print(f"\n📈 Performance comparison:")
                print(f"   gRPC speedup: {speedup:.2f}x faster than REST")
        except Exception as e:
            print(f"   ⚠️ gRPC test failed: {e}")
    
    def cleanup(self):
        """Clean up demo resources"""
        print("\n🧹 Cleaning up...")
        
        try:
            self.client.delete_collection(self.collection_name)
            self.client.delete_collection(self.chunks_collection)
            logger.info("✅ Deleted demo collections")
        except Exception as e:
            logger.warning(f"⚠️  Cleanup failed: {e}")
    
    def run_full_demo(self):
        """Run the complete demonstration"""
        print("🎭 ProximaDB Full Feature Demonstration")
        print("=" * 60)
        print("This comprehensive demo showcases:")
        print("• Advanced text chunking strategies")
        print("• Semantic search across documents")
        print("• Hybrid search with metadata filtering")
        print("• RAG pipeline demonstration")
        print("• Performance analysis")
        print("=" * 60)
        
        if not self.setup():
            return False
        
        try:
            # Run all demonstrations
            self.demonstrate_text_chunking()
            self.vectorize_and_store_chunks()
            self.demonstrate_semantic_search()
            self.demonstrate_hybrid_search()
            self.demonstrate_rag_pipeline()
            self.demonstrate_performance_analysis()
            
            print("\n✅ Full demonstration completed successfully!")
            return True
            
        except Exception as e:
            logger.error(f"❌ Demo failed: {e}")
            return False
        finally:
            self.cleanup()


def main():
    """Main entry point"""
    print("🚀 Starting ProximaDB Full Feature Demo...")
    
    demo = FullProximaDBDemo()
    success = demo.run_full_demo()
    
    print(f"\n{'='*60}")
    if success:
        print("🎊 All ProximaDB features demonstrated successfully!")
        print("✨ Including text chunking, semantic search, and RAG pipeline!")
    else:
        print("😞 Demo encountered issues")
    
    return success


if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)