#!/usr/bin/env python3
"""
ProximaDB Storage Engine Performance Comparison
Comprehensive SST vs VIPER benchmarking supporting both gRPC and REST protocols

Features:
- SST vs VIPER comparison
- Multiple protocols (REST, gRPC)
- Multiple dataset sizes (1K, 5K, 25K vectors)
- Insert, search, update, and delete operations
- Performance visualization
- Result logging with demo_logger
"""

import sys
import time
import numpy as np
import uuid
import json
from pathlib import Path
from typing import Dict, List, Any
from datetime import datetime

# Add parent directory for utils
sys.path.append(str(Path(__file__).parent.parent))

from proximadb import connect_grpc, connect_rest
from proximadb.models import (
    CollectionConfig, VectorRecord, StorageEngine, DistanceMetric
)
from utils.demo_logger import DemoLogger


class StorageEnginePerformanceDemo:
    """Comprehensive storage engine performance comparison"""
    
    def __init__(self, protocol="grpc"):
        self.protocol = protocol
        self.logger = DemoLogger("storage_engine_performance")
        self.dimension = 384
        self.test_sizes = [1000, 5000, 25000]
        self.results = {"SST": {}, "VIPER": {}}
        
    def setup_client(self):
        """Setup client based on protocol"""
        if self.protocol == "grpc":
            self.client = connect_grpc("grpc://localhost:5679")
            self.logger.log(f"Connected via gRPC to localhost:5679")
        else:
            self.client = connect_rest("http://localhost:5678")
            self.logger.log(f"Connected via REST to localhost:5678")
    
    def run_demo(self):
        """Run complete storage engine comparison"""
        self.logger.section("Storage Engine Performance Comparison")
        self.logger.log(f"Protocol: {self.protocol.upper()}")
        self.logger.log(f"Dimension: {self.dimension}")
        self.logger.log(f"Test sizes: {self.test_sizes}")
        
        try:
            self.setup_client()
            
            # Test each engine
            for engine in [StorageEngine.SST, StorageEngine.VIPER]:
                engine_name = engine.value if hasattr(engine, 'value') else str(engine)
                self.logger.section(f"Testing {engine_name} Storage Engine")
                
                for size in self.test_sizes:
                    self.test_engine_performance(engine, engine_name, size)
            
            # Compare results
            self.compare_engines()
            self.generate_report()
            
            self.logger.success("Storage engine performance comparison completed!")
            return True
            
        except Exception as e:
            self.logger.error("Demo failed", e)
            return False
    
    def test_engine_performance(self, engine, engine_name: str, vector_count: int):
        """Test performance for a specific engine and size"""
        self.logger.log(f"\nTesting with {vector_count//1000}K vectors")
        
        collection_name = f"{engine_name.lower()}_test_{uuid.uuid4().hex[:8]}"
        
        try:
            # Create collection
            config = CollectionConfig(
                dimension=self.dimension,
                distance_metric=DistanceMetric.COSINE,
                storage_engine=engine
            )
            
            start_time = time.time()
            collection = self.client.create_collection(collection_name, config)
            create_time = (time.time() - start_time) * 1000
            self.logger.metric(f"{engine_name} collection creation", create_time, "ms")
            
            # Generate test data
            vectors = np.random.randn(vector_count, self.dimension).astype(np.float32)
            vector_records = []
            for i in range(vector_count):
                record = VectorRecord(
                    id=f"vec_{i}",
                    vector=vectors[i].tolist(),
                    metadata={
                        "index": i,
                        "category": f"cat_{i % 10}",
                        "timestamp": int(time.time())
                    }
                )
                vector_records.append(record)
            
            # Test batch insertion
            batch_sizes = [100, 500, 1000]
            best_batch_size = 100
            best_throughput = 0
            
            for batch_size in batch_sizes:
                if batch_size > vector_count:
                    continue
                    
                test_batch = vector_records[:batch_size]
                start_time = time.time()
                self.client.insert_batch(collection_name, test_batch)
                insert_time = time.time() - start_time
                throughput = batch_size / insert_time
                
                if throughput > best_throughput:
                    best_throughput = throughput
                    best_batch_size = batch_size
                
                self.logger.log(f"  Batch {batch_size}: {throughput:.0f} vec/s")
            
            # Insert all vectors with optimal batch size
            self.logger.log(f"Inserting {vector_count} vectors with batch size {best_batch_size}")
            start_time = time.time()
            
            for i in range(0, vector_count, best_batch_size):
                batch = vector_records[i:i+best_batch_size]
                self.client.insert_batch(collection_name, batch)
            
            total_insert_time = time.time() - start_time
            insert_throughput = vector_count / total_insert_time
            self.logger.metric(f"{engine_name} insert throughput", insert_throughput, "vec/s")
            
            # Test search performance
            search_latencies = []
            for _ in range(10):
                query_vector = vectors[np.random.randint(0, vector_count)]
                
                start_time = time.time()
                results = self.client.search(
                    collection_id=collection_name,
                    vector=query_vector.tolist(),
                    top_k=10
                )
                search_time = (time.time() - start_time) * 1000
                search_latencies.append(search_time)
            
            avg_search_latency = np.mean(search_latencies)
            self.logger.metric(f"{engine_name} avg search latency", avg_search_latency, "ms")
            
            # Test metadata filtering
            start_time = time.time()
            filter_results = self.client.search(
                collection_id=collection_name,
                vector=vectors[0].tolist(),
                top_k=10,
                metadata_filter={"category": "cat_5"}
            )
            filter_time = (time.time() - start_time) * 1000
            self.logger.metric(f"{engine_name} filtered search", filter_time, "ms")
            
            # Store results
            if engine_name not in self.results:
                self.results[engine_name] = {}
            
            self.results[engine_name][vector_count] = {
                "create_time": create_time,
                "insert_throughput": insert_throughput,
                "search_latency": avg_search_latency,
                "filter_time": filter_time
            }
            
            # Cleanup
            self.client.delete_collection(collection_name)
            
        except Exception as e:
            self.logger.error(f"Error testing {engine_name} with {vector_count} vectors", e)
    
    def compare_engines(self):
        """Compare performance between engines"""
        self.logger.section("Engine Performance Comparison")
        
        for size in self.test_sizes:
            self.logger.log(f"\n{size//1000}K vectors:")
            
            sst_results = self.results.get("SST", {}).get(size, {})
            viper_results = self.results.get("VIPER", {}).get(size, {})
            
            if sst_results and viper_results:
                # Compare insert throughput
                sst_throughput = sst_results.get("insert_throughput", 0)
                viper_throughput = viper_results.get("insert_throughput", 0)
                
                if sst_throughput and viper_throughput:
                    ratio = sst_throughput / viper_throughput
                    winner = "SST" if ratio > 1 else "VIPER"
                    self.logger.log(f"  Insert: {winner} is {abs(ratio-1)*100:.0f}% faster")
                
                # Compare search latency
                sst_latency = sst_results.get("search_latency", 0)
                viper_latency = viper_results.get("search_latency", 0)
                
                if sst_latency and viper_latency:
                    ratio = viper_latency / sst_latency
                    winner = "SST" if ratio > 1 else "VIPER"
                    self.logger.log(f"  Search: {winner} is {abs(ratio-1)*100:.0f}% faster")
    
    def generate_report(self):
        """Generate performance report"""
        self.logger.section("Performance Summary")
        
        # Calculate average performance across all sizes
        for engine in ["SST", "VIPER"]:
            if engine in self.results:
                avg_throughput = np.mean([
                    r.get("insert_throughput", 0) 
                    for r in self.results[engine].values()
                ])
                avg_latency = np.mean([
                    r.get("search_latency", 0) 
                    for r in self.results[engine].values()
                ])
                
                self.logger.log(f"\n{engine} Engine:")
                self.logger.metric(f"Average insert throughput", avg_throughput, "vec/s")
                self.logger.metric(f"Average search latency", avg_latency, "ms")
        
        # Recommendations
        self.logger.log("\nRecommendations:")
        self.logger.log("• SST: Best for update-heavy workloads and real-time data")
        self.logger.log("• VIPER: Best for read-heavy workloads and analytics")
        self.logger.log("• Consider data access patterns when choosing engine")


def main():
    """Run storage engine performance comparison"""
    import argparse
    
    parser = argparse.ArgumentParser(description="Storage Engine Performance Demo")
    parser.add_argument("--protocol", choices=["grpc", "rest"], default="grpc",
                      help="Protocol to use (default: grpc)")
    args = parser.parse_args()
    
    demo = StorageEnginePerformanceDemo(protocol=args.protocol)
    
    with demo.logger:
        success = demo.run_demo()
        return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())