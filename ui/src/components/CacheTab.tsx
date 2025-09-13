import React, { useEffect, useState } from 'react';
import './CacheTab.css';

interface CacheMetrics {
  overall_hit_rate_percent: number;
  query_cache_hit_rate_percent: number;
  metadata_cache_hit_rate_percent: number;
  index_cache_hit_rate_percent: number;
  distance_cache_hit_rate_percent: number;
  memory_usage_mb: number;
  total_memory_mb: number;
  eviction_rate_per_sec: number;
  total_hits: number;
  total_misses: number;
  total_evictions: number;
}

interface CacheTypeMetrics {
  name: string;
  hit_rate: number;
  memory_usage_mb: number;
  entries: number;
  avg_entry_size_kb: number;
}

const CacheTab: React.FC = () => {
  const [metrics, setMetrics] = useState<CacheMetrics | null>(null);
  const [cacheTypes, setCacheTypes] = useState<CacheTypeMetrics[]>([]);
  const [loading, setLoading] = useState<boolean>(true);

  useEffect(() => {
    const fetchCacheMetrics = async () => {
      setLoading(true);
      // This would be replaced with actual API call
      setTimeout(() => {
        setMetrics({
          overall_hit_rate_percent: 94.2,
          query_cache_hit_rate_percent: 89.7,
          metadata_cache_hit_rate_percent: 95.2,
          index_cache_hit_rate_percent: 91.8,
          distance_cache_hit_rate_percent: 87.4,
          memory_usage_mb: 1024.6,
          total_memory_mb: 2048.0,
          eviction_rate_per_sec: 15.3,
          total_hits: 285472,
          total_misses: 17834,
          total_evictions: 12047,
        });

        setCacheTypes([
          { name: 'Query Cache', hit_rate: 89.7, memory_usage_mb: 412.3, entries: 8472, avg_entry_size_kb: 48.7 },
          { name: 'Metadata Cache', hit_rate: 95.2, memory_usage_mb: 256.8, entries: 15847, avg_entry_size_kb: 16.2 },
          { name: 'Index Cache', hit_rate: 91.8, memory_usage_mb: 298.4, entries: 3421, avg_entry_size_kb: 87.3 },
          { name: 'Distance Tables', hit_rate: 87.4, memory_usage_mb: 57.1, entries: 1247, avg_entry_size_kb: 45.8 },
        ]);
        setLoading(false);
      }, 1000);
    };

    fetchCacheMetrics();
    const interval = setInterval(fetchCacheMetrics, 10000); // Update every 10 seconds
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <div className="cache-loading">Loading cache metrics...</div>;
  }

  if (!metrics) {
    return <div className="cache-error">No cache data available.</div>;
  }

  const getHitRateColor = (rate: number) => {
    if (rate >= 95) return 'excellent';
    if (rate >= 90) return 'good';
    if (rate >= 80) return 'warning';
    return 'critical';
  };

  const memoryUsagePercent = (metrics.memory_usage_mb / metrics.total_memory_mb) * 100;

  return (
    <div className="cache-container">
      <h2>Cache Performance</h2>
      
      <div className="cache-overview">
        <div className="cache-summary-cards">
          <div className="summary-card">
            <h3>Overall Hit Rate</h3>
            <span className={`large-metric ${getHitRateColor(metrics.overall_hit_rate_percent)}`}>
              {metrics.overall_hit_rate_percent.toFixed(1)}%
            </span>
            <div className="hit-rate-bar">
              <div 
                className={`hit-rate-fill ${getHitRateColor(metrics.overall_hit_rate_percent)}`}
                style={{ width: `${metrics.overall_hit_rate_percent}%` }}
              ></div>
            </div>
          </div>

          <div className="summary-card">
            <h3>Memory Usage</h3>
            <span className="large-metric primary">
              {metrics.memory_usage_mb.toFixed(0)} MB
            </span>
            <span className="memory-total">of {metrics.total_memory_mb.toFixed(0)} MB</span>
            <div className="memory-bar">
              <div 
                className="memory-fill"
                style={{ width: `${memoryUsagePercent}%` }}
              ></div>
            </div>
          </div>

          <div className="summary-card">
            <h3>Eviction Rate</h3>
            <span className="large-metric secondary">
              {metrics.eviction_rate_per_sec.toFixed(1)}
            </span>
            <span className="metric-unit">evictions/sec</span>
          </div>
        </div>
      </div>

      <div className="cache-section">
        <h3>Cache Statistics</h3>
        <div className="stats-grid">
          <div className="stat-item">
            <span className="stat-label">Total Hits</span>
            <span className="stat-value excellent">{metrics.total_hits.toLocaleString()}</span>
          </div>
          <div className="stat-item">
            <span className="stat-label">Total Misses</span>
            <span className="stat-value warning">{metrics.total_misses.toLocaleString()}</span>
          </div>
          <div className="stat-item">
            <span className="stat-label">Total Evictions</span>
            <span className="stat-value secondary">{metrics.total_evictions.toLocaleString()}</span>
          </div>
          <div className="stat-item">
            <span className="stat-label">Hit/Miss Ratio</span>
            <span className="stat-value primary">
              {(metrics.total_hits / metrics.total_misses).toFixed(1)}:1
            </span>
          </div>
        </div>
      </div>

      <div className="cache-section">
        <h3>Cache Types Performance</h3>
        <div className="cache-types-table">
          <div className="table-header">
            <span>Cache Type</span>
            <span>Hit Rate</span>
            <span>Memory</span>
            <span>Entries</span>
            <span>Avg Size</span>
          </div>
          {cacheTypes.map((cache, index) => (
            <div key={index} className="table-row">
              <span className="cache-name">{cache.name}</span>
              <span className={`hit-rate ${getHitRateColor(cache.hit_rate)}`}>
                {cache.hit_rate.toFixed(1)}%
              </span>
              <span className="memory-usage">
                {cache.memory_usage_mb.toFixed(1)} MB
              </span>
              <span className="entries-count">
                {cache.entries.toLocaleString()}
              </span>
              <span className="avg-size">
                {cache.avg_entry_size_kb.toFixed(1)} KB
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="cache-section">
        <h3>Cache Configuration</h3>
        <div className="config-grid">
          <div className="config-item">
            <h4>Memory Pool Optimization</h4>
            <p>✅ Vector pools active</p>
            <p>✅ HashMap pools active</p>
            <p>✅ Batch processing enabled</p>
          </div>
          <div className="config-item">
            <h4>Unified Cache Orchestration</h4>
            <p>✅ Cross-cache coordination</p>
            <p>✅ Dynamic memory allocation</p>
            <p>✅ Predictive prefetching</p>
          </div>
          <div className="config-item">
            <h4>Cache Policies</h4>
            <p>📋 LRU eviction strategy</p>
            <p>📋 TTL-based expiration</p>
            <p>📋 Size-based limits</p>
          </div>
        </div>
      </div>
    </div>
  );
};

export default CacheTab;