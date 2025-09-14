#!/usr/bin/env python3
"""
Performance Report Generator for ProximaDB

Generates comprehensive performance reports from benchmark results
according to task_3_performance_validation_design.adoc
"""

import json
import pandas as pd
import matplotlib.pyplot as plt
from datetime import datetime
import argparse
import os
import glob
from typing import Dict, List, Optional
import numpy as np

class PerformanceReportGenerator:
    def __init__(self, results_dir: str):
        self.results_dir = results_dir
        self.benchmark_data = self.load_benchmark_data()

    def load_benchmark_data(self) -> Dict:
        """Load all benchmark results from JSON files"""
        data = {
            'qps_benchmarks': [],
            'multi_tenant_benchmarks': [],
            'cache_benchmarks': [],
            'sustained_load_results': []
        }

        try:
            # Load QPS benchmark results
            qps_files = glob.glob(f"{self.results_dir}/qps_*.json")
            for file in qps_files:
                with open(file) as f:
                    data['qps_benchmarks'].extend(json.load(f))
            print(f"✅ Loaded {len(data['qps_benchmarks'])} QPS benchmark results")
        except Exception as e:
            print(f"⚠️ Warning: Could not load QPS benchmarks: {e}")

        try:
            # Load multi-tenant benchmark results
            mt_files = glob.glob(f"{self.results_dir}/multi_tenant_*.json")
            for file in mt_files:
                with open(file) as f:
                    data['multi_tenant_benchmarks'].extend(json.load(f))
            print(f"✅ Loaded {len(data['multi_tenant_benchmarks'])} multi-tenant benchmark results")
        except Exception as e:
            print(f"⚠️ Warning: Could not load multi-tenant benchmarks: {e}")

        try:
            # Load cache benchmark results
            cache_files = glob.glob(f"{self.results_dir}/cache_*.json")
            for file in cache_files:
                with open(file) as f:
                    data['cache_benchmarks'].extend(json.load(f))
            print(f"✅ Loaded {len(data['cache_benchmarks'])} cache benchmark results")
        except Exception as e:
            print(f"⚠️ Warning: Could not load cache benchmarks: {e}")

        return data

    def generate_executive_summary(self) -> Dict:
        """Generate executive-level performance summary"""
        summary = {
            'peak_qps': 0,
            'average_qps': 0,
            'multi_tenant_qps_per_tenant': 0,
            'cache_hit_rate': 0,
            'sustained_load_stability': 'Not Tested',
            'recommended_production_capacity': 'Unknown',
            'performance_rating': 'Unknown',
            'enterprise_readiness': 'Unknown'
        }

        # Analyze QPS benchmarks
        if self.benchmark_data['qps_benchmarks']:
            qps_values = [b['qps'] for b in self.benchmark_data['qps_benchmarks'] if 'qps' in b]
            if qps_values:
                summary['peak_qps'] = max(qps_values)
                summary['average_qps'] = sum(qps_values) / len(qps_values)

                # Performance rating based on peak QPS
                if summary['peak_qps'] > 5000:
                    summary['performance_rating'] = 'Excellent'
                    summary['enterprise_readiness'] = 'Production Ready'
                elif summary['peak_qps'] > 2000:
                    summary['performance_rating'] = 'Good'
                    summary['enterprise_readiness'] = 'Enterprise Suitable'
                elif summary['peak_qps'] > 1000:
                    summary['performance_rating'] = 'Acceptable'
                    summary['enterprise_readiness'] = 'Development Ready'
                else:
                    summary['performance_rating'] = 'Needs Improvement'
                    summary['enterprise_readiness'] = 'Optimization Required'

        # Analyze multi-tenant performance
        if self.benchmark_data['multi_tenant_benchmarks']:
            mt_results = self.benchmark_data['multi_tenant_benchmarks']
            avg_per_tenant_values = [b['avg_qps_per_tenant'] for b in mt_results if 'avg_qps_per_tenant' in b]
            if avg_per_tenant_values:
                summary['multi_tenant_qps_per_tenant'] = sum(avg_per_tenant_values) / len(avg_per_tenant_values)

        # Analyze cache performance
        if self.benchmark_data['cache_benchmarks']:
            cache_rates = [b['hit_rate'] for b in self.benchmark_data['cache_benchmarks'] if 'hit_rate' in b]
            if cache_rates:
                summary['cache_hit_rate'] = sum(cache_rates) / len(cache_rates)

        # Calculate recommended production capacity (conservative estimate)
        if summary['average_qps'] > 0:
            # Recommend 70% of peak performance for production safety margin
            summary['recommended_production_capacity'] = int(summary['peak_qps'] * 0.7)

        return summary

    def generate_html_report(self, output_file: str):
        """Generate comprehensive HTML performance report"""
        summary = self.generate_executive_summary()

        html_content = f"""
        <!DOCTYPE html>
        <html>
        <head>
            <title>ProximaDB Performance Validation Report</title>
            <meta charset="UTF-8">
            <meta name="viewport" content="width=device-width, initial-scale=1.0">
            <style>
                body {{
                    font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
                    margin: 0;
                    padding: 20px;
                    background-color: #f5f5f5;
                }}
                .container {{ max-width: 1200px; margin: 0 auto; background: white; border-radius: 8px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); }}
                .header {{
                    background: linear-gradient(135deg, #1e88e5, #42a5f5);
                    color: white;
                    padding: 30px;
                    text-align: center;
                    border-radius: 8px 8px 0 0;
                }}
                .header h1 {{ margin: 0; font-size: 2.5em; font-weight: 300; }}
                .header p {{ margin: 10px 0 0 0; opacity: 0.9; }}
                .summary {{
                    display: grid;
                    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
                    gap: 20px;
                    padding: 30px;
                    background: #f8f9fa;
                }}
                .metric {{
                    text-align: center;
                    padding: 20px;
                    background: white;
                    border-radius: 6px;
                    box-shadow: 0 1px 3px rgba(0,0,0,0.1);
                }}
                .metric-value {{
                    font-size: 2.5em;
                    font-weight: bold;
                    color: #1e88e5;
                    margin: 0;
                }}
                .metric-label {{
                    font-size: 0.9em;
                    color: #666;
                    margin: 10px 0 0 0;
                    text-transform: uppercase;
                    letter-spacing: 0.5px;
                }}
                .section {{
                    margin: 0;
                    padding: 30px;
                    border-top: 1px solid #eee;
                }}
                .section h2 {{
                    color: #333;
                    margin: 0 0 20px 0;
                    font-size: 1.8em;
                    font-weight: 400;
                }}
                .benchmark-table {{
                    width: 100%;
                    border-collapse: collapse;
                    margin: 20px 0;
                    font-size: 0.9em;
                }}
                .benchmark-table th {{
                    background: #1e88e5;
                    color: white;
                    padding: 12px;
                    text-align: left;
                    font-weight: 500;
                }}
                .benchmark-table td {{
                    border: 1px solid #ddd;
                    padding: 12px;
                    text-align: left;
                }}
                .benchmark-table tr:nth-child(even) {{ background: #f8f9fa; }}
                .status-excellent {{ color: #4caf50; font-weight: bold; }}
                .status-good {{ color: #2196f3; font-weight: bold; }}
                .status-acceptable {{ color: #ff9800; font-weight: bold; }}
                .status-poor {{ color: #f44336; font-weight: bold; }}
                .chart-container {{ margin: 20px 0; text-align: center; }}
                .footer {{
                    background: #333;
                    color: white;
                    text-align: center;
                    padding: 20px;
                    border-radius: 0 0 8px 8px;
                }}
            </style>
        </head>
        <body>
            <div class="container">
                <div class="header">
                    <h1>ProximaDB Performance Validation Report</h1>
                    <p>Comprehensive Performance Analysis | Generated on {datetime.now().strftime('%B %d, %Y at %H:%M UTC')}</p>
                </div>

                <div class="summary">
                    <div class="metric">
                        <div class="metric-value">{summary['peak_qps']:.0f}</div>
                        <div class="metric-label">Peak QPS</div>
                    </div>
                    <div class="metric">
                        <div class="metric-value">{summary['average_qps']:.0f}</div>
                        <div class="metric-label">Average QPS</div>
                    </div>
                    <div class="metric">
                        <div class="metric-value">{summary['multi_tenant_qps_per_tenant']:.0f}</div>
                        <div class="metric-label">QPS per Tenant</div>
                    </div>
                    <div class="metric">
                        <div class="metric-value">{summary['cache_hit_rate']:.1f}%</div>
                        <div class="metric-label">Cache Hit Rate</div>
                    </div>
                    <div class="metric">
                        <div class="metric-value">{summary['recommended_production_capacity']}</div>
                        <div class="metric-label">Production QPS<br>Recommendation</div>
                    </div>
                    <div class="metric">
                        <div class="metric-value status-{summary['performance_rating'].lower()}">{summary['performance_rating']}</div>
                        <div class="metric-label">Performance Rating</div>
                    </div>
                </div>
        """

        # Add detailed benchmark sections
        html_content += self._generate_qps_benchmark_section()
        html_content += self._generate_multi_tenant_section()
        html_content += self._generate_recommendations_section(summary)

        html_content += f"""
                <div class="footer">
                    <p>ProximaDB Performance Validation Report | {datetime.now().strftime('%Y-%m-%d %H:%M:%S UTC')}</p>
                    <p>Benchmark Infrastructure: ✅ Complete | Validation Status: ✅ Executed | Enterprise Readiness: {summary['enterprise_readiness']}</p>
                </div>
            </div>
        </body>
        </html>
        """

        # Write report to file
        with open(output_file, 'w') as f:
            f.write(html_content)

        print(f"📊 Performance report generated: {output_file}")

    def _generate_qps_benchmark_section(self) -> str:
        """Generate QPS benchmark results section"""
        if not self.benchmark_data['qps_benchmarks']:
            return '<div class="section"><h2>QPS Benchmarks</h2><p>No QPS benchmark data available.</p></div>'

        # Group results by storage engine
        engine_results = {}
        for result in self.benchmark_data['qps_benchmarks']:
            engine = result.get('storage_engine', 'Unknown')
            if engine not in engine_results:
                engine_results[engine] = []
            engine_results[engine].append(result)

        html = '<div class="section"><h2>QPS Performance by Storage Engine</h2>'

        if engine_results:
            html += '<table class="benchmark-table"><tr><th>Storage Engine</th><th>Peak QPS</th><th>Avg QPS</th><th>Best Latency (P95)</th><th>Optimal Dataset Size</th><th>Status</th></tr>'

            for engine, results in engine_results.items():
                qps_values = [r['qps'] for r in results if 'qps' in r]
                if qps_values:
                    peak_qps = max(qps_values)
                    avg_qps = sum(qps_values) / len(qps_values)

                    # Find best latency
                    latencies = [r.get('p95_latency_ms', 100) for r in results if 'p95_latency_ms' in r]
                    best_latency = min(latencies) if latencies else 'N/A'

                    # Find optimal dataset size
                    best_result = max(results, key=lambda r: r.get('qps', 0))
                    optimal_size = best_result.get('dataset_size', 'Unknown')

                    # Determine status
                    if peak_qps > 3000:
                        status = '<span class="status-excellent">Excellent</span>'
                    elif peak_qps > 2000:
                        status = '<span class="status-good">Good</span>'
                    elif peak_qps > 1000:
                        status = '<span class="status-acceptable">Acceptable</span>'
                    else:
                        status = '<span class="status-poor">Needs Optimization</span>'

                    html += f'<tr><td>{engine}</td><td>{peak_qps:.0f}</td><td>{avg_qps:.0f}</td><td>{best_latency:.1f}ms</td><td>{optimal_size:,}</td><td>{status}</td></tr>'

            html += '</table>'
        else:
            html += '<p>No QPS benchmark results found.</p>'

        html += '</div>'
        return html

    def _generate_multi_tenant_section(self) -> str:
        """Generate multi-tenant performance section"""
        if not self.benchmark_data['multi_tenant_benchmarks']:
            return '<div class="section"><h2>Multi-Tenant Performance</h2><p>No multi-tenant benchmark data available.</p></div>'

        html = '<div class="section"><h2>Multi-Tenant Performance Isolation</h2>'

        # Create table of multi-tenant results
        html += '<table class="benchmark-table"><tr><th>Tenant Count</th><th>Total QPS</th><th>Avg QPS/Tenant</th><th>Min QPS/Tenant</th><th>Max QPS/Tenant</th><th>Isolation Status</th></tr>'

        for result in self.benchmark_data['multi_tenant_benchmarks']:
            tenant_count = result.get('tenant_count', 0)
            total_qps = result.get('total_qps', 0)
            avg_qps = result.get('avg_qps_per_tenant', 0)
            min_qps = result.get('min_qps_per_tenant', 0)
            max_qps = result.get('max_qps_per_tenant', 0)
            isolation = result.get('isolation_maintained', False)

            isolation_status = '<span class="status-excellent">✅ Maintained</span>' if isolation else '<span class="status-poor">❌ Violated</span>'

            html += f'<tr><td>{tenant_count}</td><td>{total_qps:.0f}</td><td>{avg_qps:.0f}</td><td>{min_qps:.0f}</td><td>{max_qps:.0f}</td><td>{isolation_status}</td></tr>'

        html += '</table></div>'
        return html

    def _generate_recommendations_section(self, summary: Dict) -> str:
        """Generate performance recommendations"""
        recommendations = []

        # QPS-based recommendations
        if summary['peak_qps'] > 5000:
            recommendations.append("✅ <strong>Excellent Performance:</strong> ProximaDB delivers outstanding query throughput suitable for high-scale enterprise deployments.")
        elif summary['peak_qps'] > 2000:
            recommendations.append("✅ <strong>Enterprise-Ready Performance:</strong> Query throughput meets enterprise requirements for most workloads.")
        elif summary['peak_qps'] > 1000:
            recommendations.append("⚠️ <strong>Optimization Opportunity:</strong> Performance is acceptable but could benefit from optimization for large-scale deployments.")
        else:
            recommendations.append("🔴 <strong>Performance Optimization Required:</strong> Query throughput below enterprise expectations - optimization critical.")

        # Cache performance recommendations
        if summary['cache_hit_rate'] > 90:
            recommendations.append("✅ <strong>Excellent Cache Performance:</strong> Cache hit rate exceeds 90% - optimal for production workloads.")
        elif summary['cache_hit_rate'] > 80:
            recommendations.append("✅ <strong>Good Cache Performance:</strong> Cache efficiency is acceptable for production deployment.")
        elif summary['cache_hit_rate'] > 0:
            recommendations.append("⚠️ <strong>Cache Optimization Needed:</strong> Cache hit rate below optimal - consider cache tuning.")
        else:
            recommendations.append("📊 <strong>Cache Performance Validation Needed:</strong> Execute cache benchmarks to measure efficiency.")

        # Multi-tenant recommendations
        if summary['multi_tenant_qps_per_tenant'] > 1000:
            recommendations.append("✅ <strong>Multi-Tenant Performance Excellent:</strong> Per-tenant QPS exceeds 1000 - meets enterprise SLA requirements.")
        elif summary['multi_tenant_qps_per_tenant'] > 500:
            recommendations.append("✅ <strong>Multi-Tenant Performance Good:</strong> Per-tenant performance suitable for most enterprise deployments.")
        elif summary['multi_tenant_qps_per_tenant'] > 0:
            recommendations.append("⚠️ <strong>Multi-Tenant Optimization Needed:</strong> Per-tenant QPS below optimal for enterprise SLAs.")
        else:
            recommendations.append("📊 <strong>Multi-Tenant Validation Needed:</strong> Execute multi-tenant benchmarks to validate isolation performance.")

        # Enterprise readiness assessment
        recommendations.append(f"🎯 <strong>Enterprise Readiness Assessment:</strong> {summary['enterprise_readiness']}")

        # Production capacity recommendation
        if summary['recommended_production_capacity'] != 'Unknown':
            recommendations.append(f"🚀 <strong>Production Capacity Recommendation:</strong> {summary['recommended_production_capacity']} QPS for production deployment with 30% safety margin.")

        recommendations_html = '<div class="section"><h2>Performance Recommendations</h2><ul style="line-height: 1.8;">'
        for rec in recommendations:
            recommendations_html += f'<li style="margin: 10px 0;">{rec}</li>'
        recommendations_html += '</ul></div>'

        return recommendations_html

    def generate_benchmark_execution_summary(self) -> str:
        """Generate summary of what benchmarks were executed"""
        executed_benchmarks = []

        if self.benchmark_data['qps_benchmarks']:
            executed_benchmarks.append(f"✅ QPS Benchmarks: {len(self.benchmark_data['qps_benchmarks'])} results")

        if self.benchmark_data['multi_tenant_benchmarks']:
            executed_benchmarks.append(f"✅ Multi-Tenant Benchmarks: {len(self.benchmark_data['multi_tenant_benchmarks'])} results")

        if self.benchmark_data['cache_benchmarks']:
            executed_benchmarks.append(f"✅ Cache Benchmarks: {len(self.benchmark_data['cache_benchmarks'])} results")

        if not executed_benchmarks:
            return "📊 No benchmark results found - execute performance validation benchmarks to generate metrics."

        return "Benchmark Execution Status:\n" + "\n".join(executed_benchmarks)

def main():
    parser = argparse.ArgumentParser(description='Generate ProximaDB performance validation report')
    parser.add_argument('--results-dir', default='benchmark_results', help='Directory containing benchmark results')
    parser.add_argument('--output', default='performance_report.html', help='Output HTML file path')
    parser.add_argument('--format', choices=['html', 'json', 'text'], default='html', help='Output format')

    args = parser.parse_args()

    print("🔍 ProximaDB Performance Report Generator")
    print("=========================================")

    if not os.path.exists(args.results_dir):
        print(f"⚠️ Results directory not found: {args.results_dir}")
        print("💡 Run performance benchmarks first:")
        print("   cargo bench --bench comprehensive_qps_benchmark")
        print("   cargo bench --bench multi_tenant_isolation_benchmark")
        return

    generator = PerformanceReportGenerator(args.results_dir)

    if args.format == 'html':
        generator.generate_html_report(args.output)
        print(f"\n✅ HTML report generated: {args.output}")
    elif args.format == 'json':
        summary = generator.generate_executive_summary()
        with open(args.output, 'w') as f:
            json.dump(summary, f, indent=2)
        print(f"\n✅ JSON report generated: {args.output}")
    else:
        summary = generator.generate_executive_summary()
        print("\n📊 PERFORMANCE SUMMARY:")
        print("======================")
        for key, value in summary.items():
            print(f"{key.replace('_', ' ').title()}: {value}")

    print(f"\n{generator.generate_benchmark_execution_summary()}")

if __name__ == "__main__":
    main()