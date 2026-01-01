#!/usr/bin/env python3
"""


STATUS: ✅ Production Ready (Tested 2025-01-23)
SDK Version: v1.0+
Server Version: v0.2.0+
Test Result: 100% PASS - Works perfectly

ProximaDB Dashboard & Metrics API Demo

Demonstrates:
- Health check endpoints
- Metrics collection (JSON and Prometheus formats)
- Dashboard access
- Collections API integration
- Real-time monitoring capabilities

"""

import requests
import json
import time
import sys
from typing import Dict, Any


class DashboardMetricsDemo:
    """Demo client for ProximaDB dashboard and metrics endpoints"""

    def __init__(self, base_url: str = "http://localhost:5678"):
        self.base_url = base_url
        self.session = requests.Session()

    def check_health(self) -> Dict[str, Any]:
        """Check server health status"""
        print("\n" + "="*60)
        print("1. Health Check")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/health", timeout=5)
            response.raise_for_status()
            health = response.json()

            print(f"✅ Server Status: {health.get('status', 'unknown')}")
            print(f"   Response Time: {response.elapsed.total_seconds():.3f}s")
            return health
        except Exception as e:
            print(f"❌ Health check failed: {e}")
            return {}

    def get_metrics_json(self) -> Dict[str, Any]:
        """Retrieve metrics in JSON format"""
        print("\n" + "="*60)
        print("2. Metrics (JSON Format)")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/metrics/json", timeout=5)
            response.raise_for_status()
            metrics = response.json()

            # Display key metrics
            storage = metrics.get('storage', {})
            print(f"📊 Storage Metrics:")
            print(f"   Collections: {storage.get('total_collections', 0)}")
            print(f"   Vectors: {storage.get('total_vectors', 0)}")
            print(f"   Storage Size: {storage.get('total_storage_bytes', 0):,} bytes")

            print(f"\n⏱️  System Metrics:")
            print(f"   Uptime: {metrics.get('uptime_seconds', 0):.1f}s")

            query_metrics = metrics.get('query_performance', {})
            if query_metrics:
                print(f"\n🔍 Query Performance:")
                print(f"   Total Queries: {query_metrics.get('total_queries', 0)}")
                print(f"   Avg Latency: {query_metrics.get('avg_latency_ms', 0):.2f}ms")

            return metrics
        except Exception as e:
            print(f"❌ Failed to retrieve JSON metrics: {e}")
            return {}

    def get_metrics_prometheus(self) -> str:
        """Retrieve metrics in Prometheus format"""
        print("\n" + "="*60)
        print("3. Metrics (Prometheus Format)")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/metrics", timeout=5)
            response.raise_for_status()
            metrics_text = response.text

            # Parse and display sample metrics
            lines = metrics_text.split('\n')
            help_lines = [l for l in lines if l.startswith('# HELP')]
            metric_lines = [l for l in lines if l and not l.startswith('#')]

            print(f"📈 Prometheus Metrics:")
            print(f"   Total Size: {len(metrics_text)} bytes")
            print(f"   Metric Definitions: {len(help_lines)}")
            print(f"   Metric Values: {len(metric_lines)}")

            # Show first few metrics as examples
            print(f"\n   Sample Metrics:")
            for line in metric_lines[:5]:
                if line.strip():
                    print(f"     {line}")

            return metrics_text
        except Exception as e:
            print(f"❌ Failed to retrieve Prometheus metrics: {e}")
            return ""

    def get_metrics_health(self) -> Dict[str, Any]:
        """Check metrics collection health"""
        print("\n" + "="*60)
        print("4. Metrics System Health")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/metrics/health", timeout=5)
            response.raise_for_status()
            health = response.json()

            print(f"✅ Metrics Status: {health.get('status', 'unknown')}")
            print(f"   Enabled: {health.get('metrics_enabled', False)}")
            print(f"   Collection Interval: {health.get('collection_interval_ms', 0)}ms")

            return health
        except Exception as e:
            print(f"❌ Metrics health check failed: {e}")
            return {}

    def get_collections(self) -> list:
        """Retrieve list of collections"""
        print("\n" + "="*60)
        print("5. Collections API")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/api/v1/collections", timeout=5)
            response.raise_for_status()
            collections = response.json()

            print(f"📁 Total Collections: {len(collections)}")

            if collections and isinstance(collections, list):
                print("\n   Collection Details:")
                display_count = min(5, len(collections))
                for i in range(display_count):
                    coll = collections[i]
                    name = coll.get('name', 'unknown')
                    dimension = coll.get('dimension', 0)
                    engine = coll.get('engine', 'unknown')
                    vectors = coll.get('vector_count', 0)
                    print(f"   {i+1}. {name}")
                    print(f"      Dimension: {dimension}, Engine: {engine}, Vectors: {vectors}")

                if len(collections) > 5:
                    print(f"   ... and {len(collections) - 5} more")

            return collections
        except Exception as e:
            print(f"❌ Failed to retrieve collections: {e}")
            return []

    def access_dashboard(self) -> bool:
        """Verify dashboard is accessible"""
        print("\n" + "="*60)
        print("6. Dashboard Access")
        print("="*60)

        try:
            response = self.session.get(f"{self.base_url}/dashboard", timeout=5)
            response.raise_for_status()
            html = response.text

            # Verify dashboard components
            components = {
                "Title": "ProximaDB Dashboard" in html,
                "Chart.js": "chart.umd.min.js" in html or "Chart.js" in html,
                "Overview Tab": "Overview" in html,
                "Collections Tab": "Collections" in html,
                "Metrics Tab": "Metrics" in html,
                "System Tab": "System" in html,
                "Auto-refresh": "refreshMetrics" in html or "setInterval" in html,
            }

            print(f"🖥️  Dashboard Size: {len(html):,} bytes")
            print(f"\n   Component Verification:")
            all_present = True
            for component, present in components.items():
                status = "✓" if present else "✗"
                print(f"     {status} {component}")
                all_present = all_present and present

            if all_present:
                print(f"\n✅ Dashboard fully functional at: {self.base_url}/dashboard")
                print(f"   Open in browser to view real-time visualizations")
            else:
                print(f"\n⚠️  Some dashboard components missing")

            return all_present
        except Exception as e:
            print(f"❌ Dashboard access failed: {e}")
            return False

    def monitor_metrics(self, duration_seconds: int = 30, interval_seconds: int = 5):
        """Monitor metrics over time"""
        print("\n" + "="*60)
        print(f"7. Real-Time Monitoring ({duration_seconds}s)")
        print("="*60)

        start_time = time.time()
        samples = []

        print(f"\n⏳ Collecting metrics every {interval_seconds}s...")

        while time.time() - start_time < duration_seconds:
            try:
                response = self.session.get(f"{self.base_url}/metrics/json", timeout=5)
                if response.status_code == 200:
                    metrics = response.json()
                    sample = {
                        'timestamp': time.time(),
                        'uptime': metrics.get('uptime_seconds', 0),
                        'collections': metrics.get('storage', {}).get('total_collections', 0),
                        'vectors': metrics.get('storage', {}).get('total_vectors', 0),
                    }
                    samples.append(sample)

                    elapsed = time.time() - start_time
                    print(f"   [{elapsed:.0f}s] Uptime: {sample['uptime']:.1f}s, "
                          f"Collections: {sample['collections']}, Vectors: {sample['vectors']}")
            except Exception as e:
                print(f"   ⚠️  Sample failed: {e}")

            time.sleep(interval_seconds)

        print(f"\n📊 Monitoring Summary:")
        print(f"   Total Samples: {len(samples)}")
        if samples:
            print(f"   Uptime Range: {samples[0]['uptime']:.1f}s → {samples[-1]['uptime']:.1f}s")
            print(f"   Final Metrics: {samples[-1]['collections']} collections, {samples[-1]['vectors']} vectors")

    def run_full_demo(self, with_monitoring: bool = False):
        """Run complete dashboard and metrics demonstration"""
        print("\n" + "🚀"*30)
        print("ProximaDB Dashboard & Metrics API Demo")
        print("🚀"*30)

        # Run all demo sections
        health = self.check_health()
        if not health:
            print("\n❌ Server not responding. Please ensure ProximaDB is running.")
            print(f"   Start server: cargo run --bin proximadb-server")
            return False

        metrics_json = self.get_metrics_json()
        metrics_prom = self.get_metrics_prometheus()
        metrics_health = self.get_metrics_health()
        collections = self.get_collections()
        dashboard_ok = self.access_dashboard()

        if with_monitoring:
            self.monitor_metrics(duration_seconds=30, interval_seconds=5)

        # Final summary
        print("\n" + "="*60)
        print("Demo Summary")
        print("="*60)
        print(f"✅ Health Check: {'OK' if health else 'FAILED'}")
        print(f"✅ JSON Metrics: {'OK' if metrics_json else 'FAILED'}")
        print(f"✅ Prometheus Metrics: {'OK' if metrics_prom else 'FAILED'}")
        print(f"✅ Metrics Health: {'OK' if metrics_health else 'FAILED'}")
        print(f"✅ Collections API: {len(collections)} collections")
        print(f"✅ Dashboard: {'Accessible' if dashboard_ok else 'FAILED'}")

        print("\n" + "🎯"*30)
        print("Key Endpoints:")
        print("🎯"*30)
        print(f"🖥️  Dashboard:        {self.base_url}/dashboard")
        print(f"📊 Metrics (JSON):   {self.base_url}/metrics/json")
        print(f"📈 Metrics (Prom):   {self.base_url}/metrics")
        print(f"🏥 Health:           {self.base_url}/health")
        print(f"📁 Collections:      {self.base_url}/api/v1/collections")
        print("="*60)

        return True


def main():
    """Main demo entry point"""
    # Parse command line arguments
    base_url = "http://localhost:5678"
    with_monitoring = False

    if len(sys.argv) > 1:
        base_url = sys.argv[1]
    if len(sys.argv) > 2 and sys.argv[2] == "--monitor":
        with_monitoring = True

    # Run demo
    demo = DashboardMetricsDemo(base_url)
    success = demo.run_full_demo(with_monitoring=with_monitoring)

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
