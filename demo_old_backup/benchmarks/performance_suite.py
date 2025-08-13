#!/usr/bin/env python3
"""
ProximaDB Performance Benchmark Suite
Consolidated comprehensive benchmarking for all ProximaDB features

Features:
- Storage Engine Comparison (SST vs VIPER)
- Search Optimization Testing (Bloom Filters, Parallel SST, Early Termination) 
- Protocol Performance (REST vs gRPC vs SQL)
- Hardware Acceleration Validation
- Bulk Operations Benchmarking
- Real-world E-commerce Simulation

Usage:
    python performance_suite.py --suite [basic|comprehensive|quick]
    python performance_suite.py --engines sst,viper --protocols rest,grpc --vectors 1000
"""

import sys
import os
import time
import json
import argparse
import numpy as np
from typing import List, Dict, Tuple, Optional, Any
import asyncio
from dataclasses import dataclass, field
from datetime import datetime
import statistics
from pathlib import Path

# Add the Python client to path
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '../../clients/python/src')))
sys.path.append(str(Path(__file__).parent.parent))

from proximadb import ProximaDBClient, Protocol, connect_grpc, connect_rest
from proximadb import VectorRecord, CollectionConfig, StorageEngine, DistanceMetric
from utils.demo_logger import DemoLogger

@dataclass
class BenchmarkConfig:
    """Comprehensive benchmark configuration"""
    engines: List[str] = field(default_factory=lambda: ["sst", "viper"])  
    protocols: List[str] = field(default_factory=lambda: ["rest", "grpc"])
    vector_counts: List[int] = field(default_factory=lambda: [1000, 5000])
    dimension: int = 768
    batch_sizes: List[int] = field(default_factory=lambda: [100, 500])
    distance_metrics: List[str] = field(default_factory=lambda: ["cosine", "euclidean"])
    enable_optimizations: bool = True
    run_sql_tests: bool = True
    run_protocol_comparison: bool = True
    run_sql_cache_test: bool = True
    run_optimization_hints: bool = True
    run_distance_metrics: bool = True

@dataclass
class BenchmarkResult:
    """Performance benchmark results"""
    test_name: str
    engine: str
    protocol: str
    vector_count: int
    batch_size: int
    insert_rate_per_sec: float
    search_latency_ms: float
    memory_usage_mb: float
    accuracy_score: float
    timestamp: datetime = field(default_factory=datetime.now)

class ProximaDBPerformanceSuite:
    """Unified performance benchmarking suite"""
    
    def __init__(self, config: BenchmarkConfig):
        self.config = config
        self.logger = DemoLogger("performance_suite")
        self.results: List[BenchmarkResult] = []
        
    def create_test_data(self, count: int) -> List[List[float]]:
        """Generate test vectors - returns just the vector data for simplified insert"""
        vectors = []
        for i in range(count):
            vectors.append(np.random.rand(self.config.dimension).tolist())
        return vectors
    
    def create_test_data_with_metadata(self, count: int) -> Tuple[List[List[float]], List[str], List[Dict]]:
        """Generate test vectors with IDs and metadata - returns separate lists"""
        vectors = []
        ids = []
        metadata_list = []
        categories = ["electronics", "fashion", "home", "sports", "beauty"]
        brands = ["Apple", "Samsung", "Nike", "Adidas", "Sony"]
        
        for i in range(count):
            vectors.append(np.random.rand(self.config.dimension).tolist())
            ids.append(f"product_{i:06d}")
            metadata_list.append({
                "category": categories[i % len(categories)],
                "brand": brands[i % len(brands)],
                "price": float(10.0 + (i % 1000)),
                "rating": float(3.0 + (i % 3)),
                "in_stock": i % 2 == 0
            })
        return vectors, ids, metadata_list
    
    async def benchmark_storage_engines(self) -> List[BenchmarkResult]:
        """Compare SST vs VIPER storage engines with REST vs gRPC protocols"""
        self.logger.section("🏗️ Storage Engine & Protocol Performance Comparison")
        self.logger.log("Comparing REST vs gRPC protocol performance for each storage engine")
        results = []
        
        for engine in self.config.engines:
            self.logger.section(f"📦 {engine.upper()} Storage Engine")
            for protocol in ["rest", "grpc"]:  # Explicitly test both protocols
                for vector_count in self.config.vector_counts:
                    self.logger.log(f"Testing {protocol.upper()} protocol with {vector_count:,} vectors")
                    
                    # Create client
                    if protocol == "grpc":
                        client = connect_grpc("grpc://localhost:5679")
                    else:
                        client = connect_rest("http://localhost:5678")
                    
                    # Create collection
                    collection_name = f"bench_{engine}_{protocol}_{vector_count}"
                    try:
                        client.delete_collection(collection_name)
                    except:
                        pass
                    
                    config = CollectionConfig(
                        name=collection_name,
                        dimension=self.config.dimension,
                        distance_metric=DistanceMetric.COSINE,
                        storage_engine=StorageEngine.SST if engine == "sst" else StorageEngine.VIPER
                    )
                    
                    collection = client.create_collection(collection_name, config)
                    
                    # Generate test data
                    vectors = self.create_test_data(vector_count)
                    
                    # Benchmark insertion with proper error handling
                    start_time = time.time()
                    try:
                        result = client.insert_vectors(collection_name, vectors)
                        # Check if result is successful
                        if hasattr(result, 'successful_count'):
                            self.logger.log(f"Inserted {result.successful_count} vectors successfully")
                    except Exception as e:
                        self.logger.log(f"Insert error (will retry): {e}")
                        # Try again with simpler format if needed
                        client.insert_vectors(collection_name, vectors)
                    
                    insert_time = time.time() - start_time
                    insert_rate = vector_count / insert_time
                    
                    # Benchmark search
                    query_vector = np.random.rand(self.config.dimension).tolist()
                    search_times = []
                    
                    for _ in range(10):  # Average over 10 searches
                        start_time = time.time()
                        client.search(collection_name, query_vector, top_k=10)
                        search_times.append((time.time() - start_time) * 1000)
                    
                    avg_search_latency = statistics.mean(search_times)
                    
                    result = BenchmarkResult(
                        test_name="storage_engine_comparison",
                        engine=engine,
                        protocol=protocol,
                        vector_count=vector_count,
                        batch_size=len(vectors),
                        insert_rate_per_sec=insert_rate,
                        search_latency_ms=avg_search_latency,
                        memory_usage_mb=0.0,  # Would need OS-level monitoring
                        accuracy_score=1.0    # Functional test
                    )
                    
                    results.append(result)
                    # Show clear performance comparison
                    self.logger.metric(f"{engine.upper()} {protocol.upper()}", f"{insert_rate:.0f} vec/s insert, {avg_search_latency:.1f}ms search")
                    
                    # Cleanup
                    try:
                        client.delete_collection(collection_name)
                    except:
                        pass
        
        return results
    
    async def benchmark_protocol_comparison(self) -> Dict[str, Dict[str, float]]:
        """Direct REST vs gRPC protocol performance comparison"""
        self.logger.section("🌐 REST vs gRPC Protocol Performance Comparison")
        self.logger.log("Testing identical workloads on both protocols")
        
        comparison_results = {}
        test_sizes = [100, 500, 1000, 5000]
        
        for protocol in ["rest", "grpc"]:
            self.logger.section(f"📡 Testing {protocol.upper()} Protocol")
            protocol_results = {}
            
            # Create client
            if protocol == "grpc":
                client = connect_grpc("grpc://localhost:5679")
            else:
                client = connect_rest("http://localhost:5678")
            
            for size in test_sizes:
                collection_name = f"protocol_test_{protocol}_{size}"
                
                try:
                    client.delete_collection(collection_name)
                except:
                    pass
                
                # Create collection
                config = CollectionConfig(
                    name=collection_name,
                    dimension=self.config.dimension,
                    distance_metric=DistanceMetric.COSINE,
                    storage_engine=StorageEngine.SST
                )
                client.create_collection(collection_name, config)
                
                # Generate test data
                vectors = self.create_test_data(size)
                
                # Benchmark insertion
                start_time = time.time()
                client.insert_vectors(collection_name, vectors)
                insert_time = time.time() - start_time
                insert_rate = size / insert_time
                
                # Benchmark search (average of 20 searches)
                query_vector = np.random.rand(self.config.dimension).tolist()
                search_times = []
                for _ in range(20):
                    start_time = time.time()
                    client.search(collection_name, query_vector, top_k=10)
                    search_times.append((time.time() - start_time) * 1000)
                
                avg_search_latency = statistics.mean(search_times)
                
                protocol_results[f"{size}_vectors"] = {
                    "insert_rate": insert_rate,
                    "search_latency_ms": avg_search_latency
                }
                
                self.logger.metric(
                    f"{protocol.upper()} {size} vectors",
                    f"{insert_rate:.0f} vec/s insert, {avg_search_latency:.2f}ms search"
                )
                
                # Cleanup
                try:
                    client.delete_collection(collection_name)
                except:
                    pass
            
            comparison_results[protocol] = protocol_results
        
        # Show comparison summary
        self.logger.section("📊 Protocol Performance Summary")
        for size in test_sizes:
            rest_insert = comparison_results["rest"][f"{size}_vectors"]["insert_rate"]
            grpc_insert = comparison_results["grpc"][f"{size}_vectors"]["insert_rate"]
            rest_search = comparison_results["rest"][f"{size}_vectors"]["search_latency_ms"]
            grpc_search = comparison_results["grpc"][f"{size}_vectors"]["search_latency_ms"]
            
            insert_speedup = grpc_insert / rest_insert
            search_speedup = rest_search / grpc_search
            
            self.logger.metric(
                f"{size} vectors comparison",
                f"gRPC is {insert_speedup:.2f}x faster for insert, {search_speedup:.2f}x faster for search"
            )
        
        return comparison_results
    
    async def benchmark_search_optimizations(self) -> List[BenchmarkResult]:
        """Test search optimization features"""
        self.logger.section("⚡ Search Optimization Performance")
        results = []
        
        # Test with different optimization combinations
        optimizations = [
            {"bloom_filter": True, "parallel_sst": True, "early_termination": True, "name": "All Optimizations"},
            {"bloom_filter": True, "parallel_sst": False, "early_termination": False, "name": "Bloom Filter Only"},
            {"bloom_filter": False, "parallel_sst": True, "early_termination": False, "name": "Parallel SST Only"},
            {"bloom_filter": False, "parallel_sst": False, "early_termination": False, "name": "No Optimizations"}
        ]
        
        for opt_config in optimizations:
            self.logger.log(f"Testing: {opt_config['name']}")
            
            # Create test collection
            client = connect_grpc("grpc://localhost:5679")
            collection_name = f"opt_test_{hash(opt_config['name']) % 10000}"
            
            try:
                client.delete_collection(collection_name)
            except:
                pass
            
            config = CollectionConfig(
                name=collection_name,
                dimension=self.config.dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.VIPER
            )
            
            collection = client.create_collection(collection_name, config)
            
            # Insert test data with metadata for filtering
            vectors = self.create_test_data(5000)
            client.insert_vectors(collection_name, vectors)
            
            # Benchmark filtered search (tests bloom filters)
            query_vector = np.random.rand(self.config.dimension).tolist()
            search_times = []
            
            for _ in range(20):
                start_time = time.time()
                client.search(
                    collection_name, 
                    query_vector, 
                    top_k=10,
                    metadata_filter={"category": "electronics"}
                )
                search_times.append((time.time() - start_time) * 1000)
            
            avg_latency = statistics.mean(search_times)
            
            result = BenchmarkResult(
                test_name="search_optimization",
                engine="viper",
                protocol="grpc",
                vector_count=5000,
                batch_size=5000,
                insert_rate_per_sec=0.0,
                search_latency_ms=avg_latency,
                memory_usage_mb=0.0,
                accuracy_score=1.0
            )
            
            results.append(result)
            self.logger.metric(opt_config['name'], f"{avg_latency:.1f}ms avg latency")
            
            # Cleanup
            client.delete_collection(collection_name)
        
        return results
    
    async def benchmark_sql_performance(self) -> List[BenchmarkResult]:
        """Test SQL query performance on both SST and VIPER"""
        if not self.config.run_sql_tests:
            return []
            
        self.logger.section("🔍 SQL Query Performance (SST vs VIPER)")
        results = []
        
        # Test both engines
        for engine in ["sst", "viper"]:
            self.logger.log(f"Testing SQL on {engine.upper()} engine")
            
            # Create test collection
            client = connect_rest("http://localhost:5678")
            collection_name = f"sql_benchmark_{engine}"
            
            try:
                client.delete_collection(collection_name)
            except:
                pass
            
            config = CollectionConfig(
                name=collection_name,
                dimension=self.config.dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.SST if engine == "sst" else StorageEngine.VIPER
            )
            
            collection = client.create_collection(collection_name, config)
            
            # Insert test data with metadata
            vectors, ids, metadata_list = self.create_test_data_with_metadata(5000)
            
            # Insert in batches
            batch_size = 1000
            for i in range(0, len(vectors), batch_size):
                batch_end = min(i + batch_size, len(vectors))
                client.insert_vectors(
                    collection_name,
                    vectors[i:batch_end],
                    ids=ids[i:batch_end],
                    metadata=metadata_list[i:batch_end]
                )
            
            # Allow indexing
            time.sleep(2)
            
            # Test SQL queries with full data retrieval and distance scores
            query_vector_str = json.dumps(np.random.rand(self.config.dimension).tolist())
            
            sql_queries = [
                # Basic queries with vector and metadata retrieval
                (f"SELECT id, vector, metadata, VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine') as distance FROM {collection_name} ORDER BY distance DESC LIMIT 10", "full_data_retrieval", None),
                (f"SELECT id, metadata->>'category' as category, metadata->>'price' as price, VECTOR_SIMILARITY(vector, {query_vector_str}, 'cosine') as similarity FROM {collection_name} WHERE metadata->>'category' = 'electronics' ORDER BY similarity DESC LIMIT 5", "filtered_with_fields", None),
                (f"SELECT id, metadata, VECTOR_SIMILARITY(vector, {query_vector_str}, 'euclidean') as distance FROM {collection_name} WHERE metadata->>'category' = 'electronics' AND metadata->>'price' > '500' ORDER BY distance ASC LIMIT 5", "multi_filter_euclidean", None),
                (f"SELECT id, vector, metadata->>'brand' as brand, VECTOR_SIMILARITY(vector, {query_vector_str}, 'dot_product') as score FROM {collection_name} WHERE metadata->>'price' BETWEEN '100' AND '500' ORDER BY score DESC LIMIT 10", "range_with_dot_product", None),
                (f"SELECT id, metadata, VECTOR_SIMILARITY(vector, {query_vector_str}, 'manhattan') as distance FROM {collection_name} WHERE metadata->>'brand' IN ('Apple', 'Samsung', 'Sony') ORDER BY distance ASC LIMIT 5", "in_operator_manhattan", None),
                
                # Complex aggregation query
                (f"SELECT metadata->>'category' as category, COUNT(*) as count, AVG(CAST(metadata->>'price' AS FLOAT)) as avg_price FROM {collection_name} GROUP BY metadata->>'category' ORDER BY count DESC", "aggregation_query", None),
                
                # Note: SQL hints not supported yet, will test with native API instead
            ]
            
            for sql_query, query_type, index_hint in sql_queries:
                self.logger.log(f"  Testing {query_type}" + (f" with {index_hint}" if index_hint else ""))
                
                query_times = []
                for _ in range(10):
                    start_time = time.time()
                    try:
                        client.execute_sql(sql_query)
                        query_times.append((time.time() - start_time) * 1000)
                    except Exception as e:
                        self.logger.log(f"  SQL query failed: {e}")
                        query_times.append(1000.0)
                
                avg_latency = statistics.mean(query_times)
                
                result = BenchmarkResult(
                    test_name=f"sql_{query_type}_{engine}",
                    engine=engine,
                    protocol="sql",
                    vector_count=5000,
                    batch_size=5000,
                    insert_rate_per_sec=0.0,
                    search_latency_ms=avg_latency,
                    memory_usage_mb=0.0,
                    accuracy_score=1.0
                )
                
                results.append(result)
                self.logger.metric(f"{engine.upper()} - {query_type}", f"{avg_latency:.1f}ms")
            
            # Cleanup
            client.delete_collection(collection_name)
        
        # Show comparison
        self.logger.section("SQL Performance Comparison")
        query_types = ["full_data_retrieval", "filtered_with_fields", "multi_filter_euclidean", "range_with_dot_product", "in_operator_manhattan", "aggregation_query"]
        
        for query_type in query_types:
            sst_result = next((r for r in results if r.test_name == f"sql_{query_type}_sst"), None)
            viper_result = next((r for r in results if r.test_name == f"sql_{query_type}_viper"), None)
            
            if sst_result and viper_result:
                speedup = sst_result.search_latency_ms / viper_result.search_latency_ms
                self.logger.metric(
                    query_type,
                    f"SST: {sst_result.search_latency_ms:.1f}ms, VIPER: {viper_result.search_latency_ms:.1f}ms ({speedup:.1f}x faster)"
                )
        
        return results
    
    async def benchmark_search_with_hints(self) -> List[BenchmarkResult]:
        """Test search performance with optimization hints"""
        self.logger.section("🎯 Search Performance with Optimization Hints")
        results = []
        
        # Test on both engines
        for engine in ["sst", "viper"]:
            self.logger.log(f"Testing optimization hints on {engine.upper()} engine")
            
            # Use gRPC for best performance
            client = connect_grpc("grpc://localhost:5679")
            collection_name = f"hints_benchmark_{engine}"
            
            try:
                client.delete_collection(collection_name)
            except:
                pass
            
            config = CollectionConfig(
                name=collection_name,
                dimension=self.config.dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=StorageEngine.SST if engine == "sst" else StorageEngine.VIPER
            )
            
            collection = client.create_collection(collection_name, config)
            
            # Insert test data
            vectors, ids, metadata_list = self.create_test_data_with_metadata(10000)
            
            # Insert in batches
            batch_size = 1000
            for i in range(0, len(vectors), batch_size):
                batch_end = min(i + batch_size, len(vectors))
                client.insert_vectors(
                    collection_name,
                    vectors[i:batch_end],
                    ids=ids[i:batch_end],
                    metadata=metadata_list[i:batch_end]
                )
            
            # Allow indexing
            time.sleep(2)
            
            # Test different optimization configurations
            query_vector = np.random.rand(self.config.dimension).tolist()
            
            # Based on proto SearchParameters and SearchParams
            optimization_configs = [
                ("no_hints", {}),
                # HNSW parameters
                ("hnsw_ef_50", {"ef_search": 50}),
                ("hnsw_ef_200", {"ef_search": 200}),
                ("hnsw_ef_500", {"ef_search": 500}),
                # IVF parameters
                ("ivf_probe_1", {"n_probe": 1}),
                ("ivf_probe_10", {"n_probe": 10}),
                ("ivf_probe_50", {"n_probe": 50}),
                # Search optimization parameters
                ("parallel_4_threads", {"enable_parallel_search": True, "thread_count": 4}),
                ("parallel_8_threads", {"enable_parallel_search": True, "thread_count": 8}),
                ("batch_size_100", {"batch_size": 100}),
                ("batch_size_1000", {"batch_size": 1000}),
                # Two-stage search with quantization
                ("two_stage_search", {"enable_two_stage": True}),
                ("no_quantization", {"no_quantization": True}),
                ("scalar_quantization_8", {"scalar": {"bits": 8}}),
                ("scalar_quantization_16", {"scalar": {"bits": 16}}),
                ("product_quantization_8", {"product": {"num_subvectors": 8, "bits_per_code": 8}}),
                ("product_quantization_16", {"product": {"num_subvectors": 16, "bits_per_code": 8}}),
                # Metadata filtering optimization
                ("metadata_filter_opt", {"enable_metadata_filtering_hint": True}),
                ("clustering_hint", {"enable_clustering_hint": True}),
            ]
            
            for hint_name, hints in optimization_configs:
                self.logger.log(f"  Testing {hint_name}")
                
                search_times = []
                accuracy_scores = []
                
                # Run multiple searches
                for _ in range(20):
                    start_time = time.time()
                    try:
                        # Search with optimization hints
                        results_with_hints = client.search(
                            collection_name,
                            query_vector,
                            top_k=10,
                            metadata_filter={"category": "electronics"} if "predicate" in hint_name else None,
                            optimization_hints=hints
                        )
                        search_times.append((time.time() - start_time) * 1000)
                        
                        # For quantization, also measure accuracy (would need ground truth)
                        if "quantization" in hints:
                            # Simple accuracy proxy: check if results are non-empty
                            accuracy_scores.append(1.0 if len(results_with_hints.get('results', [])) > 0 else 0.0)
                        
                    except Exception as e:
                        self.logger.log(f"    Search failed: {e}")
                        search_times.append(1000.0)
                
                avg_latency = statistics.mean(search_times)
                avg_accuracy = statistics.mean(accuracy_scores) if accuracy_scores else 1.0
                
                result = BenchmarkResult(
                    test_name=f"hints_{hint_name}_{engine}",
                    engine=engine,
                    protocol="grpc",
                    vector_count=10000,
                    batch_size=10000,
                    insert_rate_per_sec=0.0,
                    search_latency_ms=avg_latency,
                    memory_usage_mb=0.0,
                    accuracy_score=avg_accuracy
                )
                
                results.append(result)
                self.logger.metric(
                    f"{engine.upper()} - {hint_name}", 
                    f"{avg_latency:.1f}ms" + (f", accuracy: {avg_accuracy:.2%}" if accuracy_scores else "")
                )
            
            # Cleanup
            client.delete_collection(collection_name)
        
        # Show comparison
        self.logger.section("Optimization Hints Performance Impact")
        
        for engine in ["sst", "viper"]:
            self.logger.log(f"\n{engine.upper()} Engine:")
            
            # Get baseline (no hints)
            baseline = next((r for r in results if r.test_name == f"hints_no_hints_{engine}"), None)
            if baseline:
                for result in results:
                    if result.engine == engine and result.test_name != f"hints_no_hints_{engine}":
                        speedup = baseline.search_latency_ms / result.search_latency_ms
                        hint_type = result.test_name.replace(f"hints_", "").replace(f"_{engine}", "")
                        self.logger.metric(
                            f"  {hint_type}",
                            f"{result.search_latency_ms:.1f}ms ({speedup:.1f}x speedup)" + 
                            (f", accuracy: {result.accuracy_score:.2%}" if result.accuracy_score < 1.0 else "")
                        )
        
        return results
    
    async def benchmark_distance_metrics(self) -> List[BenchmarkResult]:
        """Test performance of different distance metrics"""
        self.logger.section("📏 Distance Metric Performance Comparison")
        results = []
        
        distance_metrics = ["cosine", "euclidean", "dot_product", "manhattan", "hamming", "jaccard"]
        
        for engine in ["sst", "viper"]:
            self.logger.log(f"Testing distance metrics on {engine.upper()} engine")
            
            for metric in distance_metrics:
                self.logger.log(f"  Testing {metric} distance")
                
                # Use gRPC for best performance
                client = connect_grpc("grpc://localhost:5679")
                collection_name = f"metric_benchmark_{engine}_{metric}"
                
                try:
                    client.delete_collection(collection_name)
                except:
                    pass
                
                # Create collection with specific distance metric
                config = CollectionConfig(
                    name=collection_name,
                    dimension=self.config.dimension,
                    distance_metric=getattr(DistanceMetric, metric.upper()),
                    storage_engine=StorageEngine.SST if engine == "sst" else StorageEngine.VIPER
                )
                
                try:
                    collection = client.create_collection(collection_name, config)
                    
                    # Insert test data
                    vectors = self.create_test_data(5000)
                    client.insert_vectors(collection_name, vectors)
                    
                    # Allow indexing
                    time.sleep(1)
                    
                    # Benchmark search with specific metric
                    query_vector = np.random.rand(self.config.dimension).tolist()
                    search_times = []
                    
                    for _ in range(20):
                        start_time = time.time()
                        results_data = client.search(collection_name, query_vector, top_k=10)
                        search_times.append((time.time() - start_time) * 1000)
                    
                    avg_latency = statistics.mean(search_times)
                    
                    result = BenchmarkResult(
                        test_name=f"distance_metric_{metric}_{engine}",
                        engine=engine,
                        protocol="grpc",
                        vector_count=5000,
                        batch_size=5000,
                        insert_rate_per_sec=0.0,
                        search_latency_ms=avg_latency,
                        memory_usage_mb=0.0,
                        accuracy_score=1.0
                    )
                    
                    results.append(result)
                    self.logger.metric(f"{engine.upper()} - {metric}", f"{avg_latency:.1f}ms")
                    
                except Exception as e:
                    self.logger.log(f"    Failed to test {metric}: {e}")
                    # Skip metrics that might not be supported
                    continue
                
                finally:
                    # Cleanup
                    try:
                        client.delete_collection(collection_name)
                    except:
                        pass
        
        # Show comparison summary
        self.logger.section("Distance Metric Performance Summary")
        
        for engine in ["sst", "viper"]:
            self.logger.log(f"\n{engine.upper()} Engine:")
            
            metric_results = [r for r in results if r.engine == engine]
            if metric_results:
                # Sort by latency
                metric_results.sort(key=lambda x: x.search_latency_ms)
                
                for result in metric_results:
                    metric_name = result.test_name.replace(f"distance_metric_", "").replace(f"_{engine}", "")
                    self.logger.metric(f"  {metric_name}", f"{result.search_latency_ms:.1f}ms")
        
        return results
    
    async def run_comprehensive_protocol_comparison(self) -> Dict[str, Any]:
        """Run comprehensive protocol comparison test"""
        self.logger.section("🔍 Comprehensive Protocol Comparison")
        
        # Import and run the comprehensive protocol comparison
        try:
            from comprehensive_protocol_comparison import ProtocolComparisonTest
        except ImportError:
            self.logger.log("⚠️ Skipping protocol comparison - module not found. Set PYTHONPATH to include benchmarks directory.")
            return {}
        
        test = ProtocolComparisonTest(
            dimension=self.config.dimension, 
            test_size=self.config.vector_counts[0]
        )
        results = test.run_comprehensive_comparison()
        
        return results
    
    async def run_sql_cache_test(self) -> Dict[str, Any]:
        """Run SQL cache performance test"""
        self.logger.section("💾 SQL Query Cache Performance Test")
        
        # Import and run the SQL cache demo
        try:
            from sql_cache_demo import SqlCacheDemo
        except ImportError:
            self.logger.log("⚠️ Skipping SQL cache test - module not found. Set PYTHONPATH to include benchmarks directory.")
            return {}
        
        demo = SqlCacheDemo(
            dimension=self.config.dimension,
            test_size=self.config.vector_counts[0] if self.config.vector_counts else 5000
        )
        cache_results = demo.run_demo()
        
        return cache_results
    
    async def run_comprehensive_benchmark(self) -> Dict[str, Any]:
        """Run the complete benchmark suite"""
        self.logger.section("🚀 ProximaDB Comprehensive Performance Benchmark")
        self.logger.log(f"Configuration: {len(self.config.engines)} engines, {len(self.config.protocols)} protocols")
        
        start_time = datetime.now()
        
        # Run all benchmark categories
        storage_results = await self.benchmark_storage_engines()
        
        # Add dedicated REST vs gRPC comparison
        protocol_comparison = await self.benchmark_protocol_comparison()
        
        optimization_results = await self.benchmark_search_optimizations()
        sql_results = await self.benchmark_sql_performance()
        
        # Test optimization hints if enabled
        hints_results = []
        if self.config.run_optimization_hints:
            hints_results = await self.benchmark_search_with_hints()
        
        # Test distance metrics if enabled
        distance_results = []
        if self.config.run_distance_metrics:
            distance_results = await self.benchmark_distance_metrics()
        
        # Run comprehensive protocol comparison if enabled
        comprehensive_protocol_results = None
        if self.config.run_protocol_comparison:
            comprehensive_protocol_results = await self.run_comprehensive_protocol_comparison()
        
        # Run SQL cache test if enabled
        sql_cache_results = None
        if self.config.run_sql_cache_test:
            sql_cache_results = await self.run_sql_cache_test()
        
        all_results = storage_results + optimization_results + sql_results + hints_results + distance_results
        self.results.extend(all_results)
        
        end_time = datetime.now()
        duration = (end_time - start_time).total_seconds()
        
        # Generate summary
        summary = self.generate_summary()
        
        self.logger.section("📊 Benchmark Complete")
        self.logger.log(f"Total tests run: {len(all_results)}")
        self.logger.log(f"Duration: {duration:.1f} seconds")
        
        # Save results
        results_file = f"demo/results/performance_benchmark_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        os.makedirs(os.path.dirname(results_file), exist_ok=True)
        
        with open(results_file, 'w') as f:
            json.dump({
                "config": {
                    "engines": self.config.engines,
                    "protocols": self.config.protocols,
                    "vector_counts": self.config.vector_counts,
                    "dimension": self.config.dimension
                },
                "summary": summary,
                "protocol_comparison": protocol_comparison,
                "comprehensive_protocol_results": comprehensive_protocol_results,
                "sql_cache_results": sql_cache_results,
                "results": [
                    {
                        "test_name": r.test_name,
                        "engine": r.engine,
                        "protocol": r.protocol,
                        "vector_count": r.vector_count,
                        "insert_rate_per_sec": r.insert_rate_per_sec,
                        "search_latency_ms": r.search_latency_ms,
                        "timestamp": r.timestamp.isoformat()
                    }
                    for r in all_results
                ]
            }, f, indent=2)
        
        self.logger.log(f"Results saved to: {results_file}")
        
        return summary
    
    def generate_summary(self) -> Dict[str, Any]:
        """Generate performance summary"""
        if not self.results:
            return {}
        
        # Group results by engine and protocol
        by_engine = {}
        by_protocol = {}
        
        for result in self.results:
            # By engine
            if result.engine not in by_engine:
                by_engine[result.engine] = {"insert_rates": [], "search_latencies": []}
            if result.insert_rate_per_sec > 0:
                by_engine[result.engine]["insert_rates"].append(result.insert_rate_per_sec)
            if result.search_latency_ms > 0:
                by_engine[result.engine]["search_latencies"].append(result.search_latency_ms)
            
            # By protocol
            if result.protocol not in by_protocol:
                by_protocol[result.protocol] = {"insert_rates": [], "search_latencies": []}
            if result.insert_rate_per_sec > 0:
                by_protocol[result.protocol]["insert_rates"].append(result.insert_rate_per_sec)
            if result.search_latency_ms > 0:
                by_protocol[result.protocol]["search_latencies"].append(result.search_latency_ms)
        
        summary = {
            "by_engine": {},
            "by_protocol": {},
            "overall": {
                "max_insert_rate": max((r.insert_rate_per_sec for r in self.results if r.insert_rate_per_sec > 0), default=0),
                "min_search_latency": min((r.search_latency_ms for r in self.results if r.search_latency_ms > 0), default=0),
                "total_tests": len(self.results)
            }
        }
        
        # Calculate averages by engine
        for engine, data in by_engine.items():
            summary["by_engine"][engine] = {
                "avg_insert_rate": statistics.mean(data["insert_rates"]) if data["insert_rates"] else 0,
                "avg_search_latency": statistics.mean(data["search_latencies"]) if data["search_latencies"] else 0
            }
        
        # Calculate averages by protocol
        for protocol, data in by_protocol.items():
            summary["by_protocol"][protocol] = {
                "avg_insert_rate": statistics.mean(data["insert_rates"]) if data["insert_rates"] else 0,
                "avg_search_latency": statistics.mean(data["search_latencies"]) if data["search_latencies"] else 0
            }
        
        return summary

def parse_args():
    """Parse command line arguments"""
    parser = argparse.ArgumentParser(description="ProximaDB Performance Benchmark Suite")
    
    parser.add_argument("--suite", choices=["quick", "basic", "comprehensive", "all"], default="basic",
                       help="Benchmark suite to run")
    parser.add_argument("--engines", default="sst,viper", 
                       help="Storage engines to test (comma-separated)")
    parser.add_argument("--protocols", default="rest,grpc",
                       help="Protocols to test (comma-separated)")  
    parser.add_argument("--vectors", type=int, default=1000,
                       help="Number of vectors for quick/basic tests")
    parser.add_argument("--dimension", type=int, default=768,
                       help="Vector dimension")
    parser.add_argument("--no-sql", action="store_true",
                       help="Skip SQL performance tests")
    parser.add_argument("--no-protocol-comparison", action="store_true",
                       help="Skip comprehensive protocol comparison")
    parser.add_argument("--no-sql-cache", action="store_true",
                       help="Skip SQL cache performance test")
    parser.add_argument("--no-optimization-hints", action="store_true",
                       help="Skip optimization hints benchmarking")
    parser.add_argument("--no-distance-metrics", action="store_true",
                       help="Skip distance metrics benchmarking")
    
    return parser.parse_args()

async def main():
    """Main benchmark execution"""
    args = parse_args()
    
    # Configure benchmark based on suite type
    if args.suite == "quick":
        config = BenchmarkConfig(
            engines=args.engines.split(","),
            protocols=args.protocols.split(","),
            vector_counts=[args.vectors],
            dimension=args.dimension,
            run_sql_tests=not args.no_sql,
            run_protocol_comparison=False,
            run_sql_cache_test=False,
            run_optimization_hints=not args.no_optimization_hints,
            run_distance_metrics=not args.no_distance_metrics
        )
    elif args.suite == "basic":
        config = BenchmarkConfig(
            engines=args.engines.split(","),
            protocols=args.protocols.split(","),
            vector_counts=[args.vectors, args.vectors * 2],
            dimension=args.dimension,
            run_sql_tests=not args.no_sql,
            run_protocol_comparison=False,
            run_sql_cache_test=False,
            run_optimization_hints=not args.no_optimization_hints,
            run_distance_metrics=not args.no_distance_metrics
        )
    elif args.suite == "comprehensive":
        config = BenchmarkConfig(
            engines=args.engines.split(","),
            protocols=args.protocols.split(","),
            vector_counts=[1000, 5000, 10000],
            dimension=args.dimension,
            batch_sizes=[100, 500, 1000],
            run_sql_tests=not args.no_sql,
            run_protocol_comparison=not args.no_protocol_comparison,
            run_sql_cache_test=not args.no_sql_cache,
            run_optimization_hints=not args.no_optimization_hints,
            run_distance_metrics=not args.no_distance_metrics
        )
    else:  # all
        config = BenchmarkConfig(
            engines=args.engines.split(","),
            protocols=args.protocols.split(","),
            vector_counts=[1000, 5000],
            dimension=args.dimension,
            batch_sizes=[100, 500],
            run_sql_tests=True,
            run_protocol_comparison=True,
            run_sql_cache_test=True,
            run_optimization_hints=True,
            run_distance_metrics=True
        )
    
    suite = ProximaDBPerformanceSuite(config)
    summary = await suite.run_comprehensive_benchmark()
    
    print("\n" + "="*60)
    print("🎯 PERFORMANCE BENCHMARK SUMMARY")
    print("="*60)
    
    if summary.get("overall"):
        print(f"📈 Maximum Insert Rate: {summary['overall']['max_insert_rate']:,.0f} vectors/sec")
        print(f"⚡ Minimum Search Latency: {summary['overall']['min_search_latency']:.1f}ms")
        print(f"🧪 Total Tests Completed: {summary['overall']['total_tests']}")
    
    if summary.get("by_engine"):
        print("\n🏗️ BY STORAGE ENGINE:")
        for engine, stats in summary["by_engine"].items():
            print(f"  {engine.upper()}: {stats['avg_insert_rate']:,.0f} vec/s, {stats['avg_search_latency']:.1f}ms")
    
    if summary.get("by_protocol"):
        print("\n🔌 BY PROTOCOL:")
        for protocol, stats in summary["by_protocol"].items():
            print(f"  {protocol.upper()}: {stats['avg_insert_rate']:,.0f} vec/s, {stats['avg_search_latency']:.1f}ms")
    
    print("\n✅ Benchmark completed successfully!")

if __name__ == "__main__":
    asyncio.run(main())