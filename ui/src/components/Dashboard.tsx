import React, { useState } from 'react';
import SystemOverviewTab from './SystemOverviewTab';
import CollectionsTab from './CollectionsTab';
import PerformanceTab from './PerformanceTab';
import CacheTab from './CacheTab';
import SecurityTab from './SecurityTab';
import AlertsTab from './AlertsTab';
import MetricsTab from './MetricsTab';
import DiagnosticsTab from './DiagnosticsTab';
import SqlQueryTab from './SqlQueryTab';
import GraphVisualizationTab from './GraphVisualizationTab';
import ThemeToggle from './ThemeToggle';
import './Dashboard.css';

type TabType = 'overview' | 'collections' | 'query' | 'graph' | 'performance' | 'cache' | 'security' | 'alerts' | 'metrics' | 'diagnostics';

const Dashboard: React.FC = () => {
  const [activeTab, setActiveTab] = useState<TabType>('overview');

  const tabs = [
    { id: 'overview' as const, label: 'System Overview', icon: '🔍' },
    { id: 'collections' as const, label: 'Collections', icon: '📊' },
    { id: 'query' as const, label: 'SQL Query', icon: '💻' },
    { id: 'graph' as const, label: 'Graph Explorer', icon: '🔗' },
    { id: 'performance' as const, label: 'Performance', icon: '⚡' },
    { id: 'cache' as const, label: 'Cache', icon: '💾' },
    { id: 'security' as const, label: 'Security', icon: '🔒' },
    { id: 'alerts' as const, label: 'Alerts', icon: '🚨' },
    { id: 'metrics' as const, label: 'Metrics', icon: '📈' },
    { id: 'diagnostics' as const, label: 'Diagnostics', icon: '🔧' },
  ];

  return (
    <div className="dashboard-container">
      <header className="dashboard-header">
        <div className="logo-section">
          {/* ProximaDB Logo */}
          <svg width="50" height="50" viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
            <defs>
              <linearGradient id="logoGradient" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" stopColor="#007bff" />
                <stop offset="100%" stopColor="#0056b3" />
              </linearGradient>
            </defs>
            <circle cx="50" cy="50" r="40" fill="url(#logoGradient)" stroke="#004085" strokeWidth="2" />
            <circle cx="50" cy="50" r="25" fill="none" stroke="#ffffff" strokeWidth="2" opacity="0.8" />
            <circle cx="50" cy="50" r="15" fill="none" stroke="#ffffff" strokeWidth="2" opacity="0.6" />
            <circle cx="50" cy="50" r="5" fill="#ffffff" />
          </svg>
          <div className="header-text">
            <h1>ProximaDB Enterprise Dashboard</h1>
            <span className="version">v0.2.0 - Supported core, beta extended surfaces</span>
          </div>
        </div>
        <div className="status-indicators">
          <div className="status-badge healthy">See /health</div>
          <div className="uptime">Live status pending</div>
          <ThemeToggle />
        </div>
      </header>

      <nav className="dashboard-nav">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            className={`nav-tab ${activeTab === tab.id ? 'active' : ''}`}
            onClick={() => setActiveTab(tab.id)}
            title={tab.label}
          >
            <span className="tab-icon">{tab.icon}</span>
            <span className="tab-label">{tab.label}</span>
          </button>
        ))}
      </nav>

      <main className="dashboard-content">
        {activeTab === 'overview' && <SystemOverviewTab />}
        {activeTab === 'collections' && <CollectionsTab />}
        {activeTab === 'query' && <SqlQueryTab />}
        {activeTab === 'graph' && <GraphVisualizationTab />}
        {activeTab === 'performance' && <PerformanceTab />}
        {activeTab === 'cache' && <CacheTab />}
        {activeTab === 'security' && <SecurityTab />}
        {activeTab === 'alerts' && <AlertsTab />}
        {activeTab === 'metrics' && <MetricsTab />}
        {activeTab === 'diagnostics' && <DiagnosticsTab />}
      </main>
    </div>
  );
};

export default Dashboard;
