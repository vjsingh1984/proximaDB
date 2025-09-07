import React, { useEffect, useState } from 'react';
import { HealthResponse } from '../types/api';
import { proximaDbApi } from '../api/proximaDbApi';
import './SystemOverviewTab.css';

const SystemOverviewTab: React.FC = () => {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fetchHealth = async () => {
      try {
        setLoading(true);
        const data = await proximaDbApi.getHealth();
        setHealth(data);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'An unknown error occurred');
      } finally {
        setLoading(false);
      }
    };

    fetchHealth();
  }, []);

  if (loading) {
    return <div className="system-overview-loading">Loading system overview...</div>;
  }

  if (error) {
    return <div className="system-overview-error">Error: {error}</div>;
  }

  if (!health) {
    return <div className="system-overview-no-data">No health data available.</div>;
  }

  return (
    <div className="system-overview-container">
      <h2>System Overview</h2>
      <div className="system-overview-cards">
        <div className="card">
          <h3>Status</h3>
          <p className={`status-${health.status.toLowerCase()}`}>{health.status}</p>
        </div>
        <div className="card">
          <h3>Version</h3>
          <p>{health.version}</p>
        </div>
        <div className="card">
          <h3>Uptime</h3>
          <p>{health.uptime_seconds} seconds</p>
        </div>
        <div className="card">
          <h3>Active Connections</h3>
          <p>{health.active_connections}</p>
        </div>
        <div className="card">
          <h3>Memory Usage</h3>
          <p>{(parseInt(health.memory_usage_bytes) / (1024 * 1024 * 1024)).toFixed(2)} GB</p>
        </div>
        <div className="card">
          <h3>Storage Usage</h3>
          <p>{(parseInt(health.storage_usage_bytes) / (1024 * 1024 * 1024)).toFixed(2)} GB</p>
        </div>
      </div>
    </div>
  );
};

export default SystemOverviewTab;
