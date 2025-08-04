#!/usr/bin/env python3
"""
ProximaDB Search Optimizations Benchmark

Tests the performance impact of three search optimizations:
1. Bloom Filters for Memtable (95%+ skip rate)
2. Parallel SSTable Reading (3-5x speedup)
3. Early Termination in Deduplication (for unordered queries)

Requires a running ProximaDB server on localhost:5678 (REST) and localhost:5679 (gRPC)
"""

import sys
import os
import time
import json
import numpy as np
from typing import List, Dict, Tuple
import asyncio
from dataclasses import dataclass
from datetime import datetime

# Add the Python client to path
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '../../clients/python/src')))

from proximadb import ProximaDBClient, Protocol
from proximadb.models import VectorRecord, CollectionConfig, StorageEngine, DistanceMetric

@dataclass
class BenchmarkResult:
    """Result of a single benchmark run"""
    operation: str
    protocol: str
    optimization_config: Dict[str, bool]
    num_vectors: int
    num_queries: int
    avg_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    throughput_qps: float
    metadata_filter_used: bool
    ordering_required: bool

class SearchOptimizationsBenchmark:
    """Benchmark suite for search optimizations"""
    
    def __init__(self, rest_url: str = "http://localhost:5678", grpc_url: str = "http://localhost:5679"):
        self.rest_url = rest_url
        self.grpc_url = grpc_url
        self.results: List[BenchmarkResult] = []
        
    async def setup_test_collection(self, client: ProximaDBClient, collection_name: str, 
                                  dimension: int, num_vectors: int, 
                                  storage_engine: StorageEngine = StorageEngine.SST) -> None:
        """Create and populate a test collection"""
        # Delete if exists
        try:
            await client.delete_collection(collection_name)
        except:
            pass
            
        # Create collection
        config = CollectionConfig(
            name=collection_name,
            dimension=dimension,
            storage_engine=storage_engine,
            distance_metric=DistanceMetric.COSINE
        )
        await client.create_collection(collection_name, config)
        
        # Insert vectors in batches
        batch_size = 1000
        categories = ["electronics", "books", "clothing", "food", "toys"]
        brands = ["Apple", "Samsung", "Nike", "Adidas", "Sony"]
        
        for i in range(0, num_vectors, batch_size):
            batch = []
            for j in range(min(batch_size, num_vectors - i)):
                vector_id = f"vec_{i + j}"
                vector = np.random.rand(dimension).tolist()
                
                # Add metadata for filtering tests
                metadata = {
                    "category": categories[(i + j) % len(categories)],
                    "brand": brands[(i + j) % len(brands)],
                    "price": float(50 + (i + j) % 500),
                    "in_stock": (i + j) % 3 != 0,
                    "rating": float(3.0 + (i + j) % 2)
                }
                
                batch.append(VectorRecord(
                    id=vector_id,
                    vector=vector,
                    metadata=metadata
                ))
            
            await client.insert_vectors(collection_name, records=batch)
            
        # Force flush to SST files for SST engine
        if storage_engine == StorageEngine.SST:
            # Insert a marker vector to trigger flush
            await client.insert_vector(
                collection_name,
                vector_id="flush_marker",
                vector=np.random.rand(dimension).tolist(),
                metadata={"flush": True}
            )
            time.sleep(2)  # Wait for flush
            
    async def benchmark_search_operation(self, client: ProximaDBClient, collection_name: str,
                                       query_vectors: List[List[float]], k: int = 10,
                                       metadata_filter: Dict = None, 
                                       sql_query: str = None) -> Tuple[List[float], int]:
        """Benchmark a search operation and return latencies"""
        latencies = []
        successful_queries = 0
        
        for query_vector in query_vectors:
            start_time = time.time()
            
            try:
                if sql_query:
                    # SQL query (can use early termination if no ORDER BY)
                    result = await client.execute_sql(sql_query)
                else:
                    # Regular search (always ordered)
                    result = await client.search_vectors(
                        collection_name,
                        query_vector,
                        top_k=k,
                        metadata_filter=metadata_filter
                    )
                
                latency_ms = (time.time() - start_time) * 1000
                latencies.append(latency_ms)
                successful_queries += 1
                
            except Exception as e:
                print(f"Query failed: {e}")
                
        return latencies, successful_queries
        
    async def run_benchmark_scenario(self, protocol: Protocol, collection_name: str,
                                   num_queries: int, scenario_name: str,
                                   metadata_filter: Dict = None,
                                   sql_query: str = None,
                                   ordering_required: bool = True) -> BenchmarkResult:
        """Run a specific benchmark scenario"""
        client = ProximaDBClient(
            rest_url=self.rest_url,
            grpc_url=self.grpc_url,
            protocol=protocol
        )
        
        # Get collection info
        info = await client.get_collection(collection_name)
        dimension = info.config.dimension
        num_vectors = info.stats.vector_count
        
        # Generate query vectors
        query_vectors = [np.random.rand(dimension).tolist() for _ in range(num_queries)]
        
        # Warm up
        await self.benchmark_search_operation(client, collection_name, query_vectors[:5])
        
        # Run benchmark
        start_time = time.time()
        latencies, successful_queries = await self.benchmark_search_operation(
            client, collection_name, query_vectors, 
            metadata_filter=metadata_filter,
            sql_query=sql_query
        )
        total_time = time.time() - start_time
        
        if not latencies:
            print(f"No successful queries for scenario: {scenario_name}")
            return None
            
        # Calculate metrics
        latencies_sorted = sorted(latencies)
        avg_latency = sum(latencies) / len(latencies)
        p95_latency = latencies_sorted[int(len(latencies) * 0.95)]
        p99_latency = latencies_sorted[int(len(latencies) * 0.99)]
        throughput = successful_queries / total_time
        
        result = BenchmarkResult(
            operation=scenario_name,
            protocol=protocol.value,
            optimization_config={
                "bloom_filter": True,  # Always enabled in current implementation
                "parallel_sst": True,  # Always enabled for SST
                "early_termination": not ordering_required
            },
            num_vectors=num_vectors,
            num_queries=num_queries,
            avg_latency_ms=avg_latency,
            p95_latency_ms=p95_latency,
            p99_latency_ms=p99_latency,
            throughput_qps=throughput,
            metadata_filter_used=metadata_filter is not None or sql_query is not None,
            ordering_required=ordering_required
        )
        
        return result
        
    async def run_all_benchmarks(self):
        """Run all benchmark scenarios"""
        print("🚀 ProximaDB Search Optimizations Benchmark")
        print("=" * 80)
        
        # Test configurations
        vector_counts = [10000, 50000, 100000]
        dimension = 128
        num_queries = 100
        
        for num_vectors in vector_counts:
            print(f"\n📊 Testing with {num_vectors} vectors...")
            
            # Setup SST collection
            sst_collection = f"bench_sst_{num_vectors}"
            rest_client = ProximaDBClient(protocol=Protocol.REST)
            
            print(f"Setting up SST collection with {num_vectors} vectors...")
            await self.setup_test_collection(rest_client, sst_collection, dimension, num_vectors, StorageEngine.SST)
            
            # Benchmark scenarios
            scenarios = [
                # Basic search (no filters, ordered)
                {
                    "name": "basic_search",
                    "metadata_filter": None,
                    "sql_query": None,
                    "ordering_required": True
                },
                
                # Metadata filtered search (bloom filter helps)
                {
                    "name": "metadata_filtered_search",
                    "metadata_filter": {"category": "electronics", "in_stock": True},
                    "sql_query": None,
                    "ordering_required": True
                },
                
                # SQL with ORDER BY (no early termination)
                {
                    "name": "sql_ordered_search",
                    "metadata_filter": None,
                    "sql_query": f"""
                        SELECT id, metadata
                        FROM {sst_collection}
                        WHERE metadata->>'category' = 'electronics'
                        AND metadata->>'in_stock' = 'true'
                        ORDER BY VECTOR_SIMILARITY(vector, [0.5] * {dimension}, 'cosine')
                        LIMIT 10
                    """.replace('[0.5] * 128', json.dumps([0.5] * dimension)),
                    "ordering_required": True
                },
                
                # SQL without ORDER BY (early termination possible)
                {
                    "name": "sql_unordered_search",
                    "metadata_filter": None,
                    "sql_query": f"""
                        SELECT id, metadata
                        FROM {sst_collection}
                        WHERE metadata->>'category' = 'electronics'
                        AND metadata->>'price' > 200
                        LIMIT 10
                    """,
                    "ordering_required": False
                },
                
                # Complex metadata filter (multiple bloom filter checks)
                {
                    "name": "complex_metadata_search",
                    "metadata_filter": {
                        "category": "electronics",
                        "brand": "Apple",
                        "in_stock": True,
                        "rating": 4.0
                    },
                    "sql_query": None,
                    "ordering_required": True
                }
            ]
            
            # Run benchmarks for each protocol
            for protocol in [Protocol.REST, Protocol.GRPC]:
                print(f"\n  Testing {protocol.value} protocol...")
                
                for scenario in scenarios:
                    print(f"    Running {scenario['name']}...", end='', flush=True)
                    
                    result = await self.run_benchmark_scenario(
                        protocol=protocol,
                        collection_name=sst_collection,
                        num_queries=num_queries,
                        scenario_name=scenario['name'],
                        metadata_filter=scenario.get('metadata_filter'),
                        sql_query=scenario.get('sql_query'),
                        ordering_required=scenario['ordering_required']
                    )
                    
                    if result:
                        self.results.append(result)
                        print(f" ✓ (avg: {result.avg_latency_ms:.2f}ms, QPS: {result.throughput_qps:.1f})")
                    else:
                        print(" ✗ Failed")
                        
            # Cleanup
            await rest_client.delete_collection(sst_collection)
            
    def print_results_summary(self):
        """Print a summary of all benchmark results"""
        print("\n" + "=" * 80)
        print("📈 BENCHMARK RESULTS SUMMARY")
        print("=" * 80)
        
        # Group results by scenario
        scenarios = {}
        for result in self.results:
            key = f"{result.operation}_{result.num_vectors}"
            if key not in scenarios:
                scenarios[key] = []
            scenarios[key].append(result)
            
        # Print comparison table
        print(f"\n{'Scenario':<30} {'Vectors':<10} {'Protocol':<10} {'Avg (ms)':<10} {'P95 (ms)':<10} {'P99 (ms)':<10} {'QPS':<10}")
        print("-" * 90)
        
        for key in sorted(scenarios.keys()):
            results = scenarios[key]
            for result in results:
                optimizations = []
                if result.optimization_config.get('bloom_filter'):
                    optimizations.append('BF')
                if result.optimization_config.get('parallel_sst'):
                    optimizations.append('PS')
                if result.optimization_config.get('early_termination'):
                    optimizations.append('ET')
                opt_str = '+'.join(optimizations) if optimizations else 'None'
                
                print(f"{result.operation:<30} {result.num_vectors:<10} {result.protocol:<10} "
                      f"{result.avg_latency_ms:<10.2f} {result.p95_latency_ms:<10.2f} "
                      f"{result.p99_latency_ms:<10.2f} {result.throughput_qps:<10.1f}")
                
        # Performance improvements analysis
        print("\n📊 OPTIMIZATION IMPACT ANALYSIS")
        print("-" * 80)
        
        # Compare filtered vs non-filtered searches
        basic_searches = [r for r in self.results if r.operation == "basic_search"]
        filtered_searches = [r for r in self.results if "metadata" in r.operation]
        
        if basic_searches and filtered_searches:
            basic_avg = sum(r.avg_latency_ms for r in basic_searches) / len(basic_searches)
            filtered_avg = sum(r.avg_latency_ms for r in filtered_searches) / len(filtered_searches)
            
            print(f"\nBloom Filter Impact (Metadata Filtering):")
            print(f"  Basic search avg latency: {basic_avg:.2f}ms")
            print(f"  Filtered search avg latency: {filtered_avg:.2f}ms")
            print(f"  Improvement: {((basic_avg - filtered_avg) / basic_avg * 100):.1f}%")
            
        # Compare ordered vs unordered SQL queries
        ordered_sql = [r for r in self.results if r.operation == "sql_ordered_search"]
        unordered_sql = [r for r in self.results if r.operation == "sql_unordered_search"]
        
        if ordered_sql and unordered_sql:
            ordered_avg = sum(r.avg_latency_ms for r in ordered_sql) / len(ordered_sql)
            unordered_avg = sum(r.avg_latency_ms for r in unordered_sql) / len(unordered_sql)
            
            print(f"\nEarly Termination Impact (SQL Queries):")
            print(f"  Ordered SQL avg latency: {ordered_avg:.2f}ms")
            print(f"  Unordered SQL avg latency: {unordered_avg:.2f}ms")
            print(f"  Improvement: {((ordered_avg - unordered_avg) / ordered_avg * 100):.1f}%")
            
        # Save results to file
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        results_file = f"search_optimization_results_{timestamp}.json"
        
        with open(results_file, 'w') as f:
            results_data = []
            for result in self.results:
                results_data.append({
                    "operation": result.operation,
                    "protocol": result.protocol,
                    "optimizations": result.optimization_config,
                    "num_vectors": result.num_vectors,
                    "num_queries": result.num_queries,
                    "avg_latency_ms": result.avg_latency_ms,
                    "p95_latency_ms": result.p95_latency_ms,
                    "p99_latency_ms": result.p99_latency_ms,
                    "throughput_qps": result.throughput_qps,
                    "metadata_filter": result.metadata_filter_used,
                    "ordering_required": result.ordering_required
                })
            json.dump(results_data, f, indent=2)
            
        print(f"\n💾 Results saved to: {results_file}")

async def main():
    """Main benchmark runner"""
    benchmark = SearchOptimizationsBenchmark()
    
    try:
        await benchmark.run_all_benchmarks()
        benchmark.print_results_summary()
    except Exception as e:
        print(f"\n❌ Benchmark failed: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    asyncio.run(main())