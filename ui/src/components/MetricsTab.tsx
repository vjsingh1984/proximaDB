import React, { useEffect, useState } from 'react';
import './MetricsTab.css';

interface MetricsData {
  system_metrics: {
    cpu_usage: number[];
    memory_usage: number[];
    disk_io: number[];
    network_throughput: number[];
    timestamps: string[];
  };
  query_metrics: {
    queries_per_second: number[];
    avg_latency: number[];
    error_rate: number[];
    timestamps: string[];
  };
  storage_metrics: {
    writes_per_second: number[];
    reads_per_second: number[];
    compaction_progress: number;
    storage_usage_gb: number;
  };
  cache_metrics: {
    hit_rates: { [key: string]: number };
    memory_usage: number[];
    eviction_rates: number[];
  };
}

const MetricsTab: React.FC = () => {
  const [metrics, setMetrics] = useState<MetricsData | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [timeRange, setTimeRange] = useState<'1h' | '6h' | '24h' | '7d'>('1h');

  useEffect(() => {
    const fetchMetrics = async () => {
      setLoading(true);
      // This would be replaced with actual API call
      setTimeout(() => {
        setMetrics({
          system_metrics: {
            cpu_usage: [45, 48, 52, 49, 46, 51, 55, 58, 54, 50],
            memory_usage: [62, 64, 66, 68, 65, 67, 69, 71, 68, 66],
            disk_io: [2200, 2400, 2800, 2600, 2300, 2700, 3100, 2900, 2500, 2600],
            network_throughput: [120, 135, 148, 142, 128, 156, 171, 165, 139, 144],
            timestamps: ['14:00', '14:06', '14:12', '14:18', '14:24', '14:30', '14:36', '14:42', '14:48', '14:54'],
          },
          query_metrics: {
            queries_per_second: [2800, 2950, 3200, 3100, 2850, 3050, 3300, 3150, 2900, 3000],
            avg_latency: [23, 25, 28, 26, 24, 27, 31, 29, 25, 26],
            error_rate: [0.08, 0.06, 0.12, 0.09, 0.07, 0.11, 0.15, 0.13, 0.08, 0.10],
            timestamps: ['14:00', '14:06', '14:12', '14:18', '14:24', '14:30', '14:36', '14:42', '14:48', '14:54'],
          },
          storage_metrics: {
            writes_per_second: [1200, 1350, 1480, 1420, 1280, 1560, 1710, 1650, 1390, 1440],
            reads_per_second: [8500, 8900, 9200, 9100, 8600, 9300, 9800, 9500, 8800, 9000],
            compaction_progress: 0,
            storage_usage_gb: 1203.4,
          },
          cache_metrics: {
            hit_rates: {
              'Query Cache': 89.7,
              'Metadata Cache': 95.2,
              'Index Cache': 91.8,
              'Distance Cache': 87.4,
            },
            memory_usage: [980, 1010, 1040, 1020, 990, 1030, 1060, 1050, 1015, 1025],
            eviction_rates: [12, 15, 18, 16, 13, 17, 21, 19, 14, 16],
          },
        });
        setLoading(false);
      }, 1000);
    };

    fetchMetrics();
    const interval = setInterval(fetchMetrics, 30000); // Update every 30 seconds
    return () => clearInterval(interval);
  }, [timeRange]);

  if (loading) {
    return <div className="metrics-loading">Loading metrics data...</div>;
  }

  if (!metrics) {
    return <div className="metrics-error">No metrics data available.</div>;
  }

  const renderChart = (data: number[], label: string, unit: string, color: string) => {
    const max = Math.max(...data);
    const min = Math.min(...data);
    const range = max - min || 1;
    
    return (
      <div className="chart-container">
        <h4>{label}</h4>
        <div className="chart">
          <div className="chart-content">
            {data.map((value, index) => {
              const height = ((value - min) / range) * 80 + 10; // 10-90% height
              return (
                <div
                  key={index}
                  className="chart-bar"
                  style={{
                    height: `${height}%`,
                    backgroundColor: color,
                  }}
                  title={`${value.toFixed(1)} ${unit}`}
                />
              );
            })}
          </div>
          <div className="chart-values">
            <span className="min-value">{min.toFixed(1)} {unit}</span>
            <span className="max-value">{max.toFixed(1)} {unit}</span>
          </div>
        </div>
      </div>
    );
  };

  return (
    <div className="metrics-container">
      <div className="metrics-header">
        <h2>System Metrics & Analytics</h2>
        <div className="time-range-selector">
          <button 
            className={timeRange === '1h' ? 'active' : ''}
            onClick={() => setTimeRange('1h')}
          >
            1 Hour
          </button>
          <button 
            className={timeRange === '6h' ? 'active' : ''}
            onClick={() => setTimeRange('6h')}
          >
            6 Hours
          </button>
          <button 
            className={timeRange === '24h' ? 'active' : ''}
            onClick={() => setTimeRange('24h')}
          >
            24 Hours
          </button>
          <button 
            className={timeRange === '7d' ? 'active' : ''}
            onClick={() => setTimeRange('7d')}
          >
            7 Days
          </button>
        </div>
      </div>

      <div className="metrics-section">
        <h3>System Resources</h3>
        <div className="charts-grid">
          {renderChart(metrics.system_metrics.cpu_usage, 'CPU Usage', '%', '#ff6b6b')}
          {renderChart(metrics.system_metrics.memory_usage, 'Memory Usage', '%', '#4ecdc4')}
          {renderChart(metrics.system_metrics.disk_io, 'Disk I/O', 'ops/s', '#45b7d1')}
          {renderChart(metrics.system_metrics.network_throughput, 'Network', 'Mbps', '#96ceb4')}
        </div>
      </div>

      <div className="metrics-section">
        <h3>Query Performance</h3>
        <div className="charts-grid">
          {renderChart(metrics.query_metrics.queries_per_second, 'Queries/Second', 'qps', '#feca57')}
          {renderChart(metrics.query_metrics.avg_latency, 'Average Latency', 'ms', '#ff9ff3')}
          {renderChart(metrics.query_metrics.error_rate, 'Error Rate', '%', '#ff6b6b')}
        </div>
      </div>

      <div className="metrics-section">
        <h3>Storage Performance</h3>
        <div className="charts-grid">
          {renderChart(metrics.storage_metrics.writes_per_second, 'Writes/Second', 'ops/s', '#48dbfb')}
          {renderChart(metrics.storage_metrics.reads_per_second, 'Reads/Second', 'ops/s', '#0abde3')}
          <div className="storage-info">
            <h4>Storage Status</h4>
            <div className="storage-stats">
              <div className="stat-item">
                <span className="stat-label">Total Usage</span>
                <span className="stat-value">{metrics.storage_metrics.storage_usage_gb.toFixed(1)} GB</span>
              </div>
              <div className="stat-item">
                <span className="stat-label">Compaction</span>
                <span className="stat-value">
                  {metrics.storage_metrics.compaction_progress > 0 
                    ? `${metrics.storage_metrics.compaction_progress}% Complete` 
                    : 'Idle'}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="metrics-section">
        <h3>Cache Performance</h3>
        <div className="cache-metrics-grid">
          <div className="cache-hit-rates">
            <h4>Cache Hit Rates</h4>
            <div className="hit-rates-list">
              {Object.entries(metrics.cache_metrics.hit_rates).map(([cacheName, hitRate]) => (
                <div key={cacheName} className="hit-rate-item">
                  <span className="cache-name">{cacheName}</span>
                  <div className="hit-rate-bar">
                    <div 
                      className="hit-rate-fill"
                      style={{ width: `${hitRate}%` }}
                    />
                    <span className="hit-rate-value">{hitRate.toFixed(1)}%</span>
                  </div>
                </div>
              ))}
            </div>
          </div>
          {renderChart(metrics.cache_metrics.memory_usage, 'Cache Memory', 'MB', '#6c5ce7')}
          {renderChart(metrics.cache_metrics.eviction_rates, 'Evictions/Min', 'evictions', '#fd79a8')}
        </div>
      </div>

      <div className="metrics-export">
        <h3>Export & Integration</h3>
        <div className="export-options">
          <button className="export-btn">📊 Export to CSV</button>
          <button className="export-btn">📈 Export to Prometheus</button>
          <button className="export-btn">📋 Generate Report</button>
          <button className="export-btn">🔗 Grafana Integration</button>
        </div>
      </div>
    </div>
  );
};

export default MetricsTab;