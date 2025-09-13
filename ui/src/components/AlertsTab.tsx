import React, { useEffect, useState } from 'react';
import './AlertsTab.css';

interface Alert {
  id: string;
  level: 'Critical' | 'Warning' | 'Info';
  message: string;
  component: string;
  triggered_at: string;
  metric_value: number;
  threshold: number;
  resolved: boolean;
}

interface AlertSummary {
  critical_alerts: number;
  warning_alerts: number;
  info_alerts: number;
  alerts_last_hour: number;
  alerts_last_24h: number;
  resolved_today: number;
}

const AlertsTab: React.FC = () => {
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [summary, setSummary] = useState<AlertSummary | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [filter, setFilter] = useState<'all' | 'Critical' | 'Warning' | 'Info'>('all');

  useEffect(() => {
    const fetchAlerts = async () => {
      setLoading(true);
      // This would be replaced with actual API call
      setTimeout(() => {
        setSummary({
          critical_alerts: 1,
          warning_alerts: 3,
          info_alerts: 2,
          alerts_last_hour: 2,
          alerts_last_24h: 8,
          resolved_today: 15,
        });

        setAlerts([
          {
            id: 'alert_001',
            level: 'Critical',
            message: 'High memory usage: 87.3%',
            component: 'system',
            triggered_at: '2025-09-12T14:23:45Z',
            metric_value: 87.3,
            threshold: 85.0,
            resolved: false,
          },
          {
            id: 'alert_002',
            level: 'Warning',
            message: 'Query latency P95 elevated: 156.7ms',
            component: 'query',
            triggered_at: '2025-09-12T14:18:12Z',
            metric_value: 156.7,
            threshold: 100.0,
            resolved: false,
          },
          {
            id: 'alert_003',
            level: 'Warning',
            message: 'Cache hit rate below threshold: 78.2%',
            component: 'cache',
            triggered_at: '2025-09-12T14:15:33Z',
            metric_value: 78.2,
            threshold: 80.0,
            resolved: false,
          },
          {
            id: 'alert_004',
            level: 'Info',
            message: 'Compaction completed successfully',
            component: 'storage',
            triggered_at: '2025-09-12T14:12:08Z',
            metric_value: 0,
            threshold: 0,
            resolved: true,
          },
          {
            id: 'alert_005',
            level: 'Warning',
            message: 'High disk I/O detected: 3247 ops/s',
            component: 'system',
            triggered_at: '2025-09-12T14:10:55Z',
            metric_value: 3247,
            threshold: 3000,
            resolved: false,
          },
          {
            id: 'alert_006',
            level: 'Info',
            message: 'Index optimization completed',
            component: 'index',
            triggered_at: '2025-09-12T14:08:22Z',
            metric_value: 0,
            threshold: 0,
            resolved: true,
          },
        ]);
        setLoading(false);
      }, 1000);
    };

    fetchAlerts();
    const interval = setInterval(fetchAlerts, 30000); // Update every 30 seconds
    return () => clearInterval(interval);
  }, []);

  const filteredAlerts = filter === 'all' ? alerts : alerts.filter(alert => alert.level === filter);
  const activeAlerts = alerts.filter(alert => !alert.resolved);

  const getLevelColor = (level: string) => {
    switch (level) {
      case 'Critical': return 'critical';
      case 'Warning': return 'warning';
      case 'Info': return 'info';
      default: return 'info';
    }
  };

  const formatTimeAgo = (timestamp: string) => {
    const now = new Date();
    const alertTime = new Date(timestamp);
    const diffMs = now.getTime() - alertTime.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    
    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays}d ago`;
  };

  if (loading) {
    return <div className="alerts-loading">Loading alerts...</div>;
  }

  return (
    <div className="alerts-container">
      <h2>System Alerts & Monitoring</h2>
      
      {summary && (
        <div className="alerts-summary">
          <div className="summary-cards">
            <div className="summary-card critical">
              <h3>Critical</h3>
              <span className="count">{summary.critical_alerts}</span>
            </div>
            <div className="summary-card warning">
              <h3>Warning</h3>
              <span className="count">{summary.warning_alerts}</span>
            </div>
            <div className="summary-card info">
              <h3>Info</h3>
              <span className="count">{summary.info_alerts}</span>
            </div>
            <div className="summary-card neutral">
              <h3>Last Hour</h3>
              <span className="count">{summary.alerts_last_hour}</span>
            </div>
            <div className="summary-card neutral">
              <h3>Last 24h</h3>
              <span className="count">{summary.alerts_last_24h}</span>
            </div>
            <div className="summary-card success">
              <h3>Resolved Today</h3>
              <span className="count">{summary.resolved_today}</span>
            </div>
          </div>
        </div>
      )}

      <div className="alerts-controls">
        <div className="filter-buttons">
          <button 
            className={filter === 'all' ? 'active' : ''}
            onClick={() => setFilter('all')}
          >
            All Alerts ({alerts.length})
          </button>
          <button 
            className={filter === 'Critical' ? 'active critical' : ''}
            onClick={() => setFilter('Critical')}
          >
            Critical ({alerts.filter(a => a.level === 'Critical').length})
          </button>
          <button 
            className={filter === 'Warning' ? 'active warning' : ''}
            onClick={() => setFilter('Warning')}
          >
            Warning ({alerts.filter(a => a.level === 'Warning').length})
          </button>
          <button 
            className={filter === 'Info' ? 'active info' : ''}
            onClick={() => setFilter('Info')}
          >
            Info ({alerts.filter(a => a.level === 'Info').length})
          </button>
        </div>
      </div>

      <div className="alerts-list">
        {filteredAlerts.length === 0 ? (
          <div className="no-alerts">
            <p>🎉 No alerts matching the current filter</p>
          </div>
        ) : (
          filteredAlerts.map((alert) => (
            <div key={alert.id} className={`alert-card ${getLevelColor(alert.level)} ${alert.resolved ? 'resolved' : 'active'}`}>
              <div className="alert-header">
                <div className="alert-level-indicator">
                  <span className={`level-badge ${getLevelColor(alert.level)}`}>
                    {alert.level}
                  </span>
                  <span className="component-badge">{alert.component}</span>
                  {alert.resolved && <span className="resolved-badge">✅ Resolved</span>}
                </div>
                <div className="alert-time">
                  {formatTimeAgo(alert.triggered_at)}
                </div>
              </div>
              
              <div className="alert-message">
                {alert.message}
              </div>
              
              {alert.metric_value > 0 && (
                <div className="alert-details">
                  <span className="metric-info">
                    Value: <strong>{alert.metric_value}</strong> | 
                    Threshold: <strong>{alert.threshold}</strong>
                  </span>
                </div>
              )}
              
              <div className="alert-actions">
                {!alert.resolved && (
                  <>
                    <button className="action-btn acknowledge">
                      Acknowledge
                    </button>
                    <button className="action-btn resolve">
                      Resolve
                    </button>
                  </>
                )}
                <button className="action-btn details">
                  View Details
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      <div className="alert-configuration">
        <h3>Alert Configuration</h3>
        <div className="config-section">
          <div className="threshold-config">
            <h4>Current Thresholds</h4>
            <ul>
              <li>CPU Usage: <strong>80%</strong></li>
              <li>Memory Usage: <strong>85%</strong></li>
              <li>Disk Usage: <strong>90%</strong></li>
              <li>Query Latency P95: <strong>100ms</strong></li>
              <li>Cache Hit Rate: <strong>80%</strong></li>
              <li>Error Rate: <strong>5%</strong></li>
            </ul>
          </div>
          <div className="notification-config">
            <h4>Notification Channels</h4>
            <ul>
              <li>✅ Dashboard alerts</li>
              <li>📧 Email notifications</li>
              <li>📱 Slack integration</li>
              <li>🔔 Webhook alerts</li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
};

export default AlertsTab;