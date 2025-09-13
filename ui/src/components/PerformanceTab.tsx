import React, { useEffect, useState } from 'react';
import './PerformanceTab.css';

interface PerformanceMetrics {
  queries_per_second: number;
  avg_latency_ms: number;
  p95_latency_ms: number;
  p99_latency_ms: number;
  error_rate_percent: number;
  cpu_usage_percent: number;
  memory_usage_percent: number;
  disk_io_ops_per_sec: number;
  network_throughput_mbps: number;
  active_connections: number;
  slow_queries_count: number;
}

const PerformanceTab: React.FC = () => {
  const [metrics, setMetrics] = useState<PerformanceMetrics | null>(null);
  const [loading, setLoading] = useState<boolean>(true);

  useEffect(() => {
    // Simulate fetching performance metrics
    const fetchMetrics = async () => {
      setLoading(true);
      // This would be replaced with actual API call
      setTimeout(() => {
        setMetrics({
          queries_per_second: 2847.6,
          avg_latency_ms: 23.4,
          p95_latency_ms: 89.2,
          p99_latency_ms: 156.7,
          error_rate_percent: 0.08,
          cpu_usage_percent: 45.2,
          memory_usage_percent: 62.8,
          disk_io_ops_per_sec: 2847.3,
          network_throughput_mbps: 156.8,
          active_connections: 1247,
          slow_queries_count: 12,
        });
        setLoading(false);
      }, 1000);
    };

    fetchMetrics();
    const interval = setInterval(fetchMetrics, 5000); // Update every 5 seconds
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <div className="performance-loading">Loading performance metrics...</div>;
  }

  if (!metrics) {
    return <div className="performance-error">No performance data available.</div>;
  }

  const getLatencyColor = (latency: number) => {
    if (latency < 50) return 'excellent';
    if (latency < 100) return 'good';
    if (latency < 200) return 'warning';
    return 'critical';
  };

  const getUsageColor = (usage: number) => {
    if (usage < 60) return 'excellent';
    if (usage < 80) return 'good';
    if (usage < 90) return 'warning';
    return 'critical';
  };

  return (
    <div className="performance-container">
      <h2>Performance Metrics</h2>
      
      <div className="performance-section">
        <h3>Query Performance</h3>
        <div className="metrics-grid">
          <div className="metric-card">
            <h4>Queries/Second</h4>
            <span className="metric-value primary">{metrics.queries_per_second.toFixed(1)}</span>
            <span className="metric-unit">ops/s</span>
          </div>
          <div className="metric-card">
            <h4>Average Latency</h4>
            <span className={`metric-value ${getLatencyColor(metrics.avg_latency_ms)}`}>
              {metrics.avg_latency_ms.toFixed(1)}
            </span>
            <span className="metric-unit">ms</span>
          </div>
          <div className="metric-card">
            <h4>P95 Latency</h4>
            <span className={`metric-value ${getLatencyColor(metrics.p95_latency_ms)}`}>
              {metrics.p95_latency_ms.toFixed(1)}
            </span>
            <span className="metric-unit">ms</span>
          </div>
          <div className="metric-card">
            <h4>P99 Latency</h4>
            <span className={`metric-value ${getLatencyColor(metrics.p99_latency_ms)}`}>
              {metrics.p99_latency_ms.toFixed(1)}
            </span>
            <span className="metric-unit">ms</span>
          </div>
          <div className="metric-card">
            <h4>Error Rate</h4>
            <span className={`metric-value ${metrics.error_rate_percent < 1 ? 'excellent' : 'warning'}`}>
              {metrics.error_rate_percent.toFixed(3)}
            </span>
            <span className="metric-unit">%</span>
          </div>
          <div className="metric-card">
            <h4>Slow Queries</h4>
            <span className={`metric-value ${metrics.slow_queries_count < 20 ? 'good' : 'warning'}`}>
              {metrics.slow_queries_count}
            </span>
            <span className="metric-unit">count</span>
          </div>
        </div>
      </div>

      <div className="performance-section">
        <h3>System Resources</h3>
        <div className="metrics-grid">
          <div className="metric-card">
            <h4>CPU Usage</h4>
            <span className={`metric-value ${getUsageColor(metrics.cpu_usage_percent)}`}>
              {metrics.cpu_usage_percent.toFixed(1)}
            </span>
            <span className="metric-unit">%</span>
            <div className="progress-bar">
              <div 
                className={`progress-fill ${getUsageColor(metrics.cpu_usage_percent)}`}
                style={{ width: `${metrics.cpu_usage_percent}%` }}
              ></div>
            </div>
          </div>
          <div className="metric-card">
            <h4>Memory Usage</h4>
            <span className={`metric-value ${getUsageColor(metrics.memory_usage_percent)}`}>
              {metrics.memory_usage_percent.toFixed(1)}
            </span>
            <span className="metric-unit">%</span>
            <div className="progress-bar">
              <div 
                className={`progress-fill ${getUsageColor(metrics.memory_usage_percent)}`}
                style={{ width: `${metrics.memory_usage_percent}%` }}
              ></div>
            </div>
          </div>
          <div className="metric-card">
            <h4>Disk I/O</h4>
            <span className="metric-value primary">{metrics.disk_io_ops_per_sec.toFixed(0)}</span>
            <span className="metric-unit">ops/s</span>
          </div>
          <div className="metric-card">
            <h4>Network Throughput</h4>
            <span className="metric-value primary">{metrics.network_throughput_mbps.toFixed(1)}</span>
            <span className="metric-unit">Mbps</span>
          </div>
          <div className="metric-card">
            <h4>Active Connections</h4>
            <span className="metric-value primary">{metrics.active_connections}</span>
            <span className="metric-unit">conns</span>
          </div>
        </div>
      </div>

      <div className="performance-section">
        <h3>Performance Trends</h3>
        <div className="trend-placeholder">
          <p>📈 Performance trend charts would be displayed here</p>
          <p>• Query latency over time</p>
          <p>• Throughput trends</p>
          <p>• Resource utilization patterns</p>
          <p>• Error rate timeline</p>
        </div>
      </div>
    </div>
  );
};

export default PerformanceTab;