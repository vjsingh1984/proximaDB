import React, { useEffect, useState } from 'react';
import './SecurityTab.css';

interface SecurityMetrics {
  authentication_enabled: boolean;
  authorization_enabled: boolean;
  tls_enabled: boolean;
  encryption_at_rest: boolean;
  audit_logging: boolean;
  failed_auth_attempts_last_hour: number;
  active_sessions: number;
  certificate_expiry_days: number;
  security_score: number;
}

interface SecurityEvent {
  id: string;
  type: 'Authentication' | 'Authorization' | 'Access' | 'Security';
  severity: 'High' | 'Medium' | 'Low';
  message: string;
  user: string;
  timestamp: string;
  ip_address: string;
}

const SecurityTab: React.FC = () => {
  const [metrics, setMetrics] = useState<SecurityMetrics | null>(null);
  const [events, setEvents] = useState<SecurityEvent[]>([]);
  const [loading, setLoading] = useState<boolean>(true);

  useEffect(() => {
    const fetchSecurityData = async () => {
      setLoading(true);
      // This would be replaced with actual API call
      setTimeout(() => {
        setMetrics({
          authentication_enabled: true,
          authorization_enabled: true,
          tls_enabled: true,
          encryption_at_rest: true,
          audit_logging: true,
          failed_auth_attempts_last_hour: 3,
          active_sessions: 45,
          certificate_expiry_days: 87,
          security_score: 95,
        });

        setEvents([
          {
            id: 'sec_001',
            type: 'Authentication',
            severity: 'Medium',
            message: 'Failed login attempt from unknown IP',
            user: 'admin@example.com',
            timestamp: '2025-09-12T14:25:33Z',
            ip_address: '192.168.1.100',
          },
          {
            id: 'sec_002',
            type: 'Access',
            severity: 'Low',
            message: 'Successful admin login',
            user: 'admin@company.com',
            timestamp: '2025-09-12T14:20:15Z',
            ip_address: '10.0.1.50',
          },
          {
            id: 'sec_003',
            type: 'Authorization',
            severity: 'High',
            message: 'Attempted access to restricted collection',
            user: 'user@example.com',
            timestamp: '2025-09-12T14:18:42Z',
            ip_address: '203.0.113.45',
          },
          {
            id: 'sec_004',
            type: 'Security',
            severity: 'Low',
            message: 'TLS certificate validation successful',
            user: 'system',
            timestamp: '2025-09-12T14:15:00Z',
            ip_address: 'localhost',
          },
          {
            id: 'sec_005',
            type: 'Authentication',
            severity: 'Medium',
            message: 'API key rotation completed',
            user: 'service_account',
            timestamp: '2025-09-12T14:10:22Z',
            ip_address: '10.0.2.100',
          },
        ]);
        setLoading(false);
      }, 1000);
    };

    fetchSecurityData();
    const interval = setInterval(fetchSecurityData, 60000); // Update every minute
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <div className="security-loading">Loading security information...</div>;
  }

  if (!metrics) {
    return <div className="security-error">No security data available.</div>;
  }

  const getSecurityStatus = (enabled: boolean) => enabled ? 'enabled' : 'disabled';
  const getSeverityColor = (severity: string) => {
    switch (severity) {
      case 'High': return 'high';
      case 'Medium': return 'medium';
      case 'Low': return 'low';
      default: return 'low';
    }
  };

  const getSecurityScoreColor = (score: number) => {
    if (score >= 90) return 'excellent';
    if (score >= 80) return 'good';
    if (score >= 70) return 'warning';
    return 'critical';
  };

  const formatTimeAgo = (timestamp: string) => {
    const now = new Date();
    const eventTime = new Date(timestamp);
    const diffMs = now.getTime() - eventTime.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    
    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays}d ago`;
  };

  return (
    <div className="security-container">
      <h2>Security Dashboard</h2>
      
      <div className="security-overview">
        <div className="security-score-card">
          <h3>Security Score</h3>
          <div className={`score-circle ${getSecurityScoreColor(metrics.security_score)}`}>
            <span className="score-value">{metrics.security_score}</span>
            <span className="score-max">/100</span>
          </div>
          <p className="score-description">
            {metrics.security_score >= 90 ? 'Excellent' : 
             metrics.security_score >= 80 ? 'Good' : 
             metrics.security_score >= 70 ? 'Needs Improvement' : 'Critical'}
          </p>
        </div>

        <div className="security-features">
          <h3>Security Features</h3>
          <div className="features-grid">
            <div className={`feature-item ${getSecurityStatus(metrics.authentication_enabled)}`}>
              <span className="feature-icon">🔐</span>
              <span className="feature-name">Authentication</span>
              <span className="feature-status">
                {metrics.authentication_enabled ? '✅ Enabled' : '❌ Disabled'}
              </span>
            </div>
            <div className={`feature-item ${getSecurityStatus(metrics.authorization_enabled)}`}>
              <span className="feature-icon">🛡️</span>
              <span className="feature-name">Authorization</span>
              <span className="feature-status">
                {metrics.authorization_enabled ? '✅ Enabled' : '❌ Disabled'}
              </span>
            </div>
            <div className={`feature-item ${getSecurityStatus(metrics.tls_enabled)}`}>
              <span className="feature-icon">🔒</span>
              <span className="feature-name">TLS Encryption</span>
              <span className="feature-status">
                {metrics.tls_enabled ? '✅ Enabled' : '❌ Disabled'}
              </span>
            </div>
            <div className={`feature-item ${getSecurityStatus(metrics.encryption_at_rest)}`}>
              <span className="feature-icon">💾</span>
              <span className="feature-name">Encryption at Rest</span>
              <span className="feature-status">
                {metrics.encryption_at_rest ? '✅ Enabled' : '❌ Disabled'}
              </span>
            </div>
            <div className={`feature-item ${getSecurityStatus(metrics.audit_logging)}`}>
              <span className="feature-icon">📝</span>
              <span className="feature-name">Audit Logging</span>
              <span className="feature-status">
                {metrics.audit_logging ? '✅ Enabled' : '❌ Disabled'}
              </span>
            </div>
          </div>
        </div>
      </div>

      <div className="security-metrics">
        <h3>Security Metrics</h3>
        <div className="metrics-grid">
          <div className="metric-card">
            <h4>Failed Auth Attempts</h4>
            <span className={`metric-value ${metrics.failed_auth_attempts_last_hour > 10 ? 'warning' : 'good'}`}>
              {metrics.failed_auth_attempts_last_hour}
            </span>
            <span className="metric-period">last hour</span>
          </div>
          <div className="metric-card">
            <h4>Active Sessions</h4>
            <span className="metric-value primary">{metrics.active_sessions}</span>
            <span className="metric-period">current</span>
          </div>
          <div className="metric-card">
            <h4>Certificate Expiry</h4>
            <span className={`metric-value ${metrics.certificate_expiry_days < 30 ? 'warning' : 'good'}`}>
              {metrics.certificate_expiry_days}
            </span>
            <span className="metric-period">days</span>
          </div>
        </div>
      </div>

      <div className="security-events">
        <h3>Recent Security Events</h3>
        <div className="events-list">
          {events.map((event) => (
            <div key={event.id} className={`event-card ${getSeverityColor(event.severity)}`}>
              <div className="event-header">
                <div className="event-type-indicator">
                  <span className={`type-badge ${event.type.toLowerCase()}`}>
                    {event.type}
                  </span>
                  <span className={`severity-badge ${getSeverityColor(event.severity)}`}>
                    {event.severity}
                  </span>
                </div>
                <div className="event-time">
                  {formatTimeAgo(event.timestamp)}
                </div>
              </div>
              
              <div className="event-message">
                {event.message}
              </div>
              
              <div className="event-details">
                <span className="event-user">User: {event.user}</span>
                <span className="event-ip">IP: {event.ip_address}</span>
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="security-config">
        <h3>Security Configuration</h3>
        <div className="config-sections">
          <div className="config-section">
            <h4>Authentication Methods</h4>
            <ul>
              <li>✅ API Keys</li>
              <li>✅ JWT Tokens</li>
              <li>✅ OAuth 2.0</li>
              <li>✅ mTLS Certificates</li>
            </ul>
          </div>
          <div className="config-section">
            <h4>Authorization</h4>
            <ul>
              <li>✅ Role-Based Access Control (RBAC)</li>
              <li>✅ Collection-level permissions</li>
              <li>✅ API endpoint restrictions</li>
              <li>✅ Tenant isolation</li>
            </ul>
          </div>
          <div className="config-section">
            <h4>Data Protection</h4>
            <ul>
              <li>✅ TLS 1.3 encryption in transit</li>
              <li>✅ AES-256 encryption at rest</li>
              <li>✅ Secure key management</li>
              <li>✅ Data anonymization</li>
            </ul>
          </div>
          <div className="config-section">
            <h4>Compliance</h4>
            <ul>
              <li>✅ SOC 2 Type II ready</li>
              <li>✅ GDPR compliant</li>
              <li>✅ HIPAA ready</li>
              <li>✅ Audit trail maintained</li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
};

export default SecurityTab;