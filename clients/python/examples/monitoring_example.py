#!/usr/bin/env python3
"""


STATUS: ⚠️  Requires Future Feature
SDK Version: v1.1+ (requires proximadb.telemetry module)
Server Version: v0.1.5+
Test Result: SKIP - Telemetry infrastructure not yet implemented

Monitoring and Telemetry Example for ProximaDB Python SDK v1.0

This example demonstrates comprehensive monitoring:
- Metrics collection (counters, gauges, histograms)
- Distributed tracing with spans
- Custom telemetry exporters
- Performance monitoring
- Error tracking
- Dashboard integration

"""

# import asyncio
import json
import logging
import time
import random
from datetime import datetime, timedelta
from typing import Dict, List, Any, Optional
import numpy as np

from proximadb import ProximaDBClient, ClientConfig
from proximadb.models import CollectionConfig, VectorRecord, DistanceMetric

# Future feature imports - will trigger helpful error message when executed
try:
    from proximadb.telemetry import (
        init_telemetry,
        get_telemetry_manager,
        MetricType,
        MetricUnit,
        Metric,
        ConsoleExporter,
        HTTPExporter,
        PrometheusExporter
    )
    from proximadb.telemetry_decorators import (
        trace_operation,
        record_metrics,
        SpanContext,
        MetricsContext,
        instrument_client
    )
except (ImportError, ModuleNotFoundError) as e:
    print("=" * 70)
    print("🚧 FUTURE FEATURE - Not Yet Implemented")
    print("=" * 70)
    print(f"\n❌ Error: {e}\n")
    print(f"📋 This example requires: proximadb.telemetry")
    print(f"   Expected in SDK v1.1+\n")
    print(f"💡 Workaround:")
    print(f"   Use dashboard_metrics_demo.py for current monitoring capabilities\n")
    print("=" * 70)
    exit(1)

from proximadb.exceptions import ProximaDBError


# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)


class CustomMetricsExporter:
    """Custom metrics exporter for demonstration"""
    
    def __init__(self, export_file: str = "metrics_export.jsonl"):
        self.export_file = export_file
        self.metrics_buffer = []
    
    def export_metrics(self, metrics: Dict[str, Any]):
        """Export metrics to file"""
        timestamp = datetime.now().isoformat()
        
        # Format metrics for export
        for metric_name, metric_data in metrics.items():
            for key, value in metric_data.get("values", {}).items():
                self.metrics_buffer.append({
                    "timestamp": timestamp,
                    "metric": metric_name,
                    "labels": self._parse_labels(key),
                    "value": value,
                    "type": metric_data.get("type", "unknown")
                })
        
        # Write to file
        with open(self.export_file, "a") as f:
            for metric in self.metrics_buffer:
                f.write(json.dumps(metric) + "\n")
        
        self.metrics_buffer.clear()
        logger.info(f"Exported metrics to {self.export_file}")
    
    def export_spans(self, spans: List[Any]):
        """Export trace spans"""
        # Similar implementation for spans
        pass
    
    def _parse_labels(self, label_str: str) -> Dict[str, str]:
        """Parse label string into dictionary"""
        labels = {}
        if "{" in label_str and "}" in label_str:
            label_part = label_str[label_str.index("{") + 1:label_str.rindex("}")]
            for pair in label_part.split(","):
                if "=" in pair:
                    key, value = pair.split("=", 1)
                    labels[key.strip()] = value.strip()
        return labels


class MonitoredProximaDBClient:
    """Client wrapper with enhanced monitoring capabilities"""
    
    def __init__(self, base_client: ProximaDBClient):
        self.client = instrument_client(base_client)
        self.telemetry = get_telemetry_manager()
        self._register_custom_metrics()
    
    def _register_custom_metrics(self):
        """Register custom application metrics"""
        # Business metrics
        self.telemetry.metrics_collector.register_metric(Metric(
            name="vector_quality_score",
            type=MetricType.HISTOGRAM,
            unit=MetricUnit.RATIO,
            description="Quality score of inserted vectors",
            buckets=[0.1, 0.3, 0.5, 0.7, 0.9, 0.95, 0.99, 1.0]
        ))
        
        self.telemetry.metrics_collector.register_metric(Metric(
            name="search_relevance_score",
            type=MetricType.HISTOGRAM,
            unit=MetricUnit.RATIO,
            description="Relevance scores from search results",
            buckets=[0.1, 0.3, 0.5, 0.7, 0.9, 0.95, 0.99, 1.0]
        ))
        
        self.telemetry.metrics_collector.register_metric(Metric(
            name="collection_growth_rate",
            type=MetricType.GAUGE,
            unit=MetricUnit.COUNT,
            description="Vectors added per minute"
        ))
        
        self.telemetry.metrics_collector.register_metric(Metric(
            name="query_complexity",
            type=MetricType.HISTOGRAM,
            unit=MetricUnit.COUNT,
            description="Number of filter conditions in queries",
            buckets=[1, 2, 3, 5, 10, 20, 50]
        ))
    
    @trace_operation("custom_insert_with_validation")
    def insert_with_validation(self, collection_name: str, 
                                   vectors: List[VectorRecord]) -> Dict[str, Any]:
        """Insert vectors with quality validation and monitoring"""
        with SpanContext("validate_vectors") as span:
            span.set_tag("vector_count", len(vectors))
            span.set_tag("collection", collection_name)
            
            # Validate vector quality
            quality_scores = []
            for vector in vectors:
                score = self._calculate_vector_quality(vector.vector)
                quality_scores.append(score)
                
                # Record quality metric
                self.telemetry.metrics_collector.observe_histogram(
                    "vector_quality_score",
                    score,
                    {"collection": collection_name}
                )
            
            avg_quality = sum(quality_scores) / len(quality_scores)
            span.set_tag("avg_quality", f"{avg_quality:.3f}")
            
            if avg_quality < 0.5:
                span.set_tag("warning", "low_quality_vectors")
                logger.warning(f"Low quality vectors detected: avg={avg_quality:.3f}")
        
        # Insert with monitoring
        with MetricsContext("vector_insertion") as ctx:
            response = self.client.ainsert_vectors(collection_name, vectors)
            
            # Record custom metrics
            ctx.increment("vectors_inserted", len(vectors))
            if response.errors:
                ctx.increment("insertion_errors", len(response.errors))
            
            return {
                "response": response,
                "quality_scores": quality_scores,
                "avg_quality": avg_quality
            }
    
    @trace_operation("monitored_search")
    def search_with_monitoring(self, collection_name: str,
                                   query_vector: List[float],
                                   top_k: int = 10,
                                   filters: Optional[List[Any]] = None) -> Dict[str, Any]:
        """Search with comprehensive monitoring"""
        start_time = time.time()
        
        with SpanContext("search_operation") as span:
            span.set_tag("collection", collection_name)
            span.set_tag("top_k", top_k)
            span.set_tag("has_filters", filters is not None)
            
            if filters:
                # Record query complexity
                self.telemetry.metrics_collector.observe_histogram(
                    "query_complexity",
                    len(filters),
                    {"collection": collection_name}
                )
                span.set_tag("filter_count", len(filters))
            
            # Perform search
            try:
                results = self.client.search(
                    collection_name,
                    query_vector,
                    top_k=top_k
                )
                
                # Analyze results
                if results.results:
                    scores = [r.score for r in results.results]
                    avg_score = sum(scores) / len(scores)
                    
                    # Record relevance metrics
                    for score in scores:
                        self.telemetry.metrics_collector.observe_histogram(
                            "search_relevance_score",
                            score,
                            {"collection": collection_name}
                        )
                    
                    span.set_tag("avg_relevance", f"{avg_score:.3f}")
                    span.set_tag("result_count", len(results.results))
                    
                    # Log slow queries
                    duration = time.time() - start_time
                    if duration > 1.0:  # Slow query threshold
                        span.set_tag("slow_query", True)
                        logger.warning(f"Slow query detected: {duration:.2f}s")
                
                return {
                    "results": results,
                    "duration_ms": (time.time() - start_time) * 1000,
                    "avg_relevance": avg_score if results.results else 0
                }
                
            except Exception as e:
                span.set_tag("error", True)
                span.set_tag("error_type", type(e).__name__)
                span.log("error_details", error=str(e))
                raise
    
    def _calculate_vector_quality(self, vector: List[float]) -> float:
        """Calculate quality score for a vector"""
        # Simple quality metrics for demonstration
        arr = np.array(vector)
        
        # Check for NaN or inf values
        if np.any(np.isnan(arr)) or np.any(np.isinf(arr)):
            return 0.0
        
        # Check magnitude
        magnitude = np.linalg.norm(arr)
        if magnitude == 0:
            return 0.0
        
        # Check variance (diversity of values)
        variance = np.var(arr)
        
        # Combine metrics into quality score
        quality = min(1.0, variance * 10) * min(1.0, magnitude)
        return float(quality)
    
    def get_monitoring_dashboard(self) -> Dict[str, Any]:
        """Get current monitoring dashboard data"""
        metrics = self.telemetry.metrics_collector.get_metrics()
        
        dashboard = {
            "timestamp": datetime.now().isoformat(),
            "system_metrics": {},
            "business_metrics": {},
            "performance_metrics": {}
        }
        
        # System metrics
        if "proximadb_requests_total" in metrics:
            total_requests = sum(metrics["proximadb_requests_total"]["values"].values())
            dashboard["system_metrics"]["total_requests"] = total_requests
        
        if "proximadb_errors_total" in metrics:
            total_errors = sum(metrics["proximadb_errors_total"]["values"].values())
            dashboard["system_metrics"]["total_errors"] = total_errors
            dashboard["system_metrics"]["error_rate"] = (
                total_errors / total_requests if total_requests > 0 else 0
            )
        
        # Performance metrics
        if "proximadb_request_duration" in metrics:
            durations = metrics["proximadb_request_duration"]["values"]
            for key, histogram in durations.items():
                operation = key.split("{")[0] if "{" in key else key
                dashboard["performance_metrics"][operation] = {
                    "count": histogram.get("count", 0),
                    "mean_ms": histogram.get("mean", 0) * 1000,
                    "p95_ms": histogram.get("p95", 0) * 1000 if "p95" in histogram else None
                }
        
        # Business metrics
        if "vector_quality_score" in metrics:
            quality_data = list(metrics["vector_quality_score"]["values"].values())
            if quality_data:
                dashboard["business_metrics"]["vector_quality"] = {
                    "mean": quality_data[0].get("mean", 0),
                    "count": quality_data[0].get("count", 0)
                }
        
        if "search_relevance_score" in metrics:
            relevance_data = list(metrics["search_relevance_score"]["values"].values())
            if relevance_data:
                dashboard["business_metrics"]["search_relevance"] = {
                    "mean": relevance_data[0].get("mean", 0),
                    "count": relevance_data[0].get("count", 0)
                }
        
        return dashboard
    
    def __getattr__(self, name):
        """Proxy to underlying client"""
        return getattr(self.client, name)


def simulate_production_workload(client: MonitoredProximaDBClient,
                                     collection_name: str,
                                     duration_seconds: int = 60):
    """Simulate production workload for monitoring"""
    print(f"\n🔄 Simulating production workload for {duration_seconds}s...")
    
    start_time = time.time()
    operation_count = 0
    
    while time.time() - start_time < duration_seconds:
        operation_type = random.choice(["insert", "search", "mixed"])
        
        try:
            if operation_type == "insert":
                # Generate batch of vectors
                vectors = []
                for i in range(random.randint(10, 50)):
                    vector = np.random.randn(128)
                    # Add some low quality vectors
                    if random.random() < 0.1:
                        vector *= 0.01  # Low magnitude
                    
                    vectors.append(VectorRecord(
                        id=f"vec_{int(time.time() * 1000000)}_{i}",
                        vector=vector.tolist(),
                        metadata={
                            "timestamp": time.time(),
                            "batch_id": operation_count,
                            "quality": "low" if np.linalg.norm(vector) < 0.1 else "normal"
                        }
                    ))
                
                result = client.insert_with_validation(collection_name, vectors)
                logger.info(f"Inserted {len(vectors)} vectors, "
                          f"avg quality: {result['avg_quality']:.3f}")
            
            elif operation_type == "search":
                # Perform search
                query = np.random.randn(128).tolist()
                filters = None
                
                # Sometimes add filters
                if random.random() < 0.3:
                    filters = [
                        {"field": "metadata.quality", "operator": "=", "value": "normal"}
                    ]
                
                result = client.search_with_monitoring(
                    collection_name,
                    query,
                    top_k=random.randint(5, 20),
                    filters=filters
                )
                logger.info(f"Search completed in {result['duration_ms']:.2f}ms, "
                          f"avg relevance: {result['avg_relevance']:.3f}")
            
            else:  # mixed
                # Insert then immediately search
                vector = VectorRecord(
                    id=f"mixed_{int(time.time() * 1000000)}",
                    vector=np.random.randn(128).tolist(),
                    metadata={"type": "mixed_operation"}
                )
                
                client.ainsert_vector(collection_name, vector)
                client.search_with_monitoring(
                    collection_name,
                    vector.vector,
                    top_k=5
                )
            
            operation_count += 1
            
            # Simulate variable load
            time.sleep(random.uniform(0.1, 0.5))
            
        except Exception as e:
            logger.error(f"Operation failed: {e}")
            # Continue on errors
    
    print(f"✅ Completed {operation_count} operations")


def main():
    print("🚀 Monitoring and Telemetry Example for ProximaDB")
    print("=" * 50)
    
    # Initialize telemetry with multiple exporters
    custom_exporter = CustomMetricsExporter("demo_metrics.jsonl")
    
    telemetry = init_telemetry(
        exporters=[
            ConsoleExporter(),  # Log to console
            custom_exporter,    # Custom file exporter
            # PrometheusExporter(port=9090),  # Uncomment for Prometheus
            # HTTPExporter("http://localhost:4318/v1/metrics")  # OTLP endpoint
        ],
        export_interval=10.0,  # Export every 10 seconds
        service_name="proximadb-monitoring-demo",
        service_version="1.0.0"
    )
    
    telemetry.start()
    
    # Create monitored client
    base_client = ProximaDBClient(
        ClientConfig(
            url="http://localhost:5678",
            timeout=30.0
        )
    )
    
    client = MonitoredProximaDBClient(base_client)
    
    collection_name = "monitoring_demo"
    
    try:
        # Create collection
        print(f"\n📦 Creating collection '{collection_name}'...")
        
        config = CollectionConfig(
            name=collection_name,
            dimension=128,
            distance_metric=DistanceMetric.COSINE
        )
        
        try:
            client.adelete_collection(collection_name)
        except:
            pass
        
        collection = client.acreate_collection(config)
        print(f"✅ Collection created: {collection.id}")
        
        # Run workload simulation
        simulate_production_workload(client, collection_name, duration_seconds=30)
        
        # Wait for final metrics export
        print("\n⏳ Waiting for final metrics export...")
        time.sleep(12)
        
        # Display dashboard
        dashboard = client.get_monitoring_dashboard()
        
        print("\n📊 Monitoring Dashboard")
        print("=" * 50)
        
        print("\n🖥️  System Metrics:")
        for key, value in dashboard["system_metrics"].items():
            print(f"   - {key}: {value}")
        
        print("\n⚡ Performance Metrics:")
        for operation, metrics in dashboard["performance_metrics"].items():
            print(f"   - {operation}:")
            print(f"     • Count: {metrics['count']}")
            print(f"     • Mean: {metrics['mean_ms']:.2f}ms")
            if metrics.get('p95_ms'):
                print(f"     • P95: {metrics['p95_ms']:.2f}ms")
        
        print("\n💼 Business Metrics:")
        for metric, data in dashboard["business_metrics"].items():
            print(f"   - {metric}:")
            print(f"     • Mean: {data['mean']:.3f}")
            print(f"     • Count: {data['count']}")
        
        # Show exported metrics file
        print(f"\n📄 Metrics exported to: demo_metrics.jsonl")
        print("   First few lines:")
        with open("demo_metrics.jsonl", "r") as f:
            for i, line in enumerate(f):
                if i >= 5:
                    break
                metric = json.loads(line)
                print(f"   {metric['timestamp']}: {metric['metric']} = {metric['value']}")
        
        # Demonstrate custom monitoring queries
        print("\n🔍 Custom Monitoring Queries:")
        
        # Get current stats
        metrics = client.telemetry.metrics_collector.get_metrics()
        
        # Calculate SLIs (Service Level Indicators)
        if "proximadb_request_duration" in metrics:
            durations = []
            for histogram in metrics["proximadb_request_duration"]["values"].values():
                if "values" in histogram:
                    durations.extend(histogram["values"])
            
            if durations:
                p99 = np.percentile(durations, 99)
                print(f"   - P99 Latency: {p99*1000:.2f}ms")
                print(f"   - SLO Status: {'✅ PASS' if p99 < 0.1 else '❌ FAIL'} (<100ms)")
        
        print("\n✅ Monitoring example completed!")
        
    finally:
        # Cleanup
        print("\n🧹 Cleaning up...")
        try:
            client.adelete_collection(collection_name)
            print("✅ Demo collection deleted")
        except Exception as e:
            print(f"⚠️  Cleanup failed: {e}")
        
        # Stop telemetry
        telemetry.stop()


if __name__ == "__main__":
    try:
        main()
    except (ImportError, ModuleNotFoundError) as e:
        print("=" * 70)
        print("🚧 FUTURE FEATURE - Not Yet Implemented")
        print("=" * 70)
        print(f"\n❌ Error: {e}\n")
        print(f"📋 This example requires: proximadb.telemetry")
        print(f"   Expected in SDK v1.1+\n")
        print(f"💡 Workaround:")
        print(f"   Use dashboard_metrics_demo.py for current monitoring capabilities\n")
        print("=" * 70)
        exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        exit(1)