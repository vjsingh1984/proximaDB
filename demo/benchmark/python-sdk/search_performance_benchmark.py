#!/usr/bin/env python3
"""
ProximaDB Python SDK - Search Performance Benchmark
Extracted from test suite to run as standalone performance benchmark
"""

import time
import sys
import os
import numpy as np
from typing import List, Dict, Any

# Add SDK to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', '..', '..', 'clients', 'python', 'src'))

from proximadb import ProximaDBClient, connect_rest, connect_grpc
from proximadb.models import CollectionConfig, DistanceMetric

class SearchPerformanceBenchmark:
    """Standalone search performance benchmark"""
    
    def __init__(self, rest_url="http://localhost:5678", grpc_url="http://localhost:5679"):
        self.rest_client = connect_rest(rest_url)
        self.grpc_client = connect_grpc(grpc_url)
        self.results = {}
        
    def setup_test_collection(self, name: str, dimension: int = 384) -> str:
        """Create test collection for benchmarking"""
        config = CollectionConfig(
            name=name,
            dimension=dimension,
            distance_metric="cosine",
            description=f"Performance benchmark collection - {dimension}D"
        )
        
        collection = self.rest_client.create_collection(name, config)
        return name
    
    def populate_collection(self, collection_name: str, vector_count: int, dimension: int):
        """Populate collection with test vectors"""
        print(f"Populating {collection_name} with {vector_count} vectors...")
        
        batch_size = 100
        start_time = time.time()
        
        for i in range(0, vector_count, batch_size):
            batch_end = min(i + batch_size, vector_count)
            vectors = []
            ids = []
            metadata = []
            
            for j in range(i, batch_end):
                vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
                vectors.append(vector)
                ids.append(f"perf_vector_{j}")
                metadata.append({
                    "index": j,
                    "category": f"group_{j % 10}",
                    "batch": i // batch_size
                })
            
            self.rest_client.insert_vectors(collection_name, vectors, ids, metadata)
        
        populate_time = time.time() - start_time
        print(f"✅ Populated {vector_count} vectors in {populate_time:.2f}s ({vector_count/populate_time:.0f} vectors/s)")
        
    def benchmark_search_latency(self, collection_name: str, dimension: int, num_queries: int = 100):
        """Benchmark search latency for both REST and gRPC"""
        print(f"\n🔍 Search Latency Benchmark ({num_queries} queries)")
        
        # Generate random query vectors
        query_vectors = [
            np.random.normal(0, 1, dimension).astype(np.float32).tolist()
            for _ in range(num_queries)
        ]
        
        # Benchmark REST
        rest_times = []
        for i, query_vector in enumerate(query_vectors):
            start_time = time.time()
            results = self.rest_client.search_vectors(collection_name, query_vector, top_k=10)
            rest_times.append(time.time() - start_time)
            
            if i % 20 == 0:
                print(f"  REST progress: {i+1}/{num_queries}")
        
        # Benchmark gRPC
        grpc_times = []
        for i, query_vector in enumerate(query_vectors):
            start_time = time.time()
            results = self.grpc_client.search_vectors(collection_name, query_vector, top_k=10)
            grpc_times.append(time.time() - start_time)
            
            if i % 20 == 0:
                print(f"  gRPC progress: {i+1}/{num_queries}")
        
        # Calculate statistics
        def calc_stats(times):
            times_ms = [t * 1000 for t in times]
            return {
                'mean': np.mean(times_ms),
                'median': np.median(times_ms),
                'p95': np.percentile(times_ms, 95),
                'p99': np.percentile(times_ms, 99),
                'min': np.min(times_ms),
                'max': np.max(times_ms)
            }
        
        rest_stats = calc_stats(rest_times)
        grpc_stats = calc_stats(grpc_times)
        
        print(f"\n📊 Search Latency Results:")
        print(f"REST API:")
        print(f"  Mean: {rest_stats['mean']:.2f}ms")
        print(f"  Median: {rest_stats['median']:.2f}ms") 
        print(f"  P95: {rest_stats['p95']:.2f}ms")
        print(f"  P99: {rest_stats['p99']:.2f}ms")
        print(f"  Range: {rest_stats['min']:.2f}ms - {rest_stats['max']:.2f}ms")
        
        print(f"gRPC API:")
        print(f"  Mean: {grpc_stats['mean']:.2f}ms")
        print(f"  Median: {grpc_stats['median']:.2f}ms")
        print(f"  P95: {grpc_stats['p95']:.2f}ms") 
        print(f"  P99: {grpc_stats['p99']:.2f}ms")
        print(f"  Range: {grpc_stats['min']:.2f}ms - {grpc_stats['max']:.2f}ms")
        
        speedup = rest_stats['mean'] / grpc_stats['mean']
        print(f"\n🚀 gRPC is {speedup:.2f}x faster than REST (mean latency)")
        
        return {
            'rest': rest_stats,
            'grpc': grpc_stats,
            'speedup': speedup
        }
    
    def benchmark_throughput(self, collection_name: str, dimension: int, duration_seconds: int = 30):
        """Benchmark search throughput"""
        print(f"\n⚡ Search Throughput Benchmark ({duration_seconds}s duration)")
        
        # REST throughput
        rest_count = 0
        start_time = time.time()
        while time.time() - start_time < duration_seconds:
            query_vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
            self.rest_client.search_vectors(collection_name, query_vector, top_k=5)
            rest_count += 1
        rest_throughput = rest_count / duration_seconds
        
        # gRPC throughput  
        grpc_count = 0
        start_time = time.time()
        while time.time() - start_time < duration_seconds:
            query_vector = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
            self.grpc_client.search_vectors(collection_name, query_vector, top_k=5)
            grpc_count += 1
        grpc_throughput = grpc_count / duration_seconds
        
        print(f"📈 Throughput Results:")
        print(f"  REST: {rest_throughput:.1f} queries/second")
        print(f"  gRPC: {grpc_throughput:.1f} queries/second")
        print(f"  gRPC throughput advantage: {grpc_throughput/rest_throughput:.2f}x")
        
        return {
            'rest_qps': rest_throughput,
            'grpc_qps': grpc_throughput,
            'grpc_advantage': grpc_throughput/rest_throughput
        }
    
    def cleanup(self, collection_name: str):
        """Clean up test collection"""
        try:
            self.rest_client.delete_collection(collection_name)
            print(f"🧹 Cleaned up collection: {collection_name}")
        except Exception as e:
            print(f"⚠️ Cleanup warning: {e}")
    
    def run_full_benchmark(self):
        """Run complete search performance benchmark"""
        print("🚀 ProximaDB Python SDK - Search Performance Benchmark")
        print("=" * 60)
        
        collection_name = f"search_perf_benchmark_{int(time.time())}"
        dimension = 384
        vector_count = 1000
        
        try:
            # Setup
            self.setup_test_collection(collection_name, dimension)
            self.populate_collection(collection_name, vector_count, dimension)
            
            # Warm up
            print("\n🔥 Warming up...")
            for _ in range(10):
                query = np.random.normal(0, 1, dimension).astype(np.float32).tolist()
                self.rest_client.search_vectors(collection_name, query, top_k=5)
                self.grpc_client.search_vectors(collection_name, query, top_k=5)
            
            # Run benchmarks
            latency_results = self.benchmark_search_latency(collection_name, dimension, num_queries=50)
            throughput_results = self.benchmark_throughput(collection_name, dimension, duration_seconds=15)
            
            # Summary
            print(f"\n🎯 Benchmark Summary:")
            print(f"Collection: {vector_count} vectors, {dimension}D")
            print(f"gRPC mean latency: {latency_results['grpc']['mean']:.2f}ms")
            print(f"gRPC throughput: {throughput_results['grpc_qps']:.1f} QPS")
            print(f"Overall gRPC advantage: {latency_results['speedup']:.2f}x latency, {throughput_results['grpc_advantage']:.2f}x throughput")
            
        finally:
            self.cleanup(collection_name)

if __name__ == "__main__":
    try:
        benchmark = SearchPerformanceBenchmark()
        benchmark.run_full_benchmark()
    except KeyboardInterrupt:
        print("\n⏹️ Benchmark interrupted by user")
    except Exception as e:
        print(f"❌ Benchmark failed: {e}")
        import traceback
        traceback.print_exc()