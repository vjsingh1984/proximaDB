import React, { useState } from 'react';
import SystemOverviewTab from './SystemOverviewTab';
import CollectionsTab from './CollectionsTab';
import './Dashboard.css';

const Dashboard: React.FC = () => {
  const [activeTab, setActiveTab] = useState<'overview' | 'collections'>('overview');

  return (
    <div className="dashboard-container">
      <header className="dashboard-header">
        <div className="logo-section">
          {/* Placeholder for Logo - Replace with actual image later */}
          <svg width="50" height="50" viewBox="0 0 100 100" xmlns="http://www.w3.org/2000/svg">
            <circle cx="50" cy="50" r="40" fill="#007bff" />
            <text x="50" y="60" font-family="Arial, sans-serif" font-size="40" fill="#ffffff" text-anchor="middle">P</text>
          </svg>
          <h1>ProximaDB Dashboard</h1>
        </div>
        <p className="tagline">Vector Search, Accelerated.</p>
      </header>

      <nav className="dashboard-nav">
        <button
          className={activeTab === 'overview' ? 'active' : ''}
          onClick={() => setActiveTab('overview')}
        >
          System Overview
        </button>
        <button
          className={activeTab === 'collections' ? 'active' : ''}
          onClick={() => setActiveTab('collections')}
        >
          Collections
        </button>
      </nav>

      <main className="dashboard-content">
        {activeTab === 'overview' && <SystemOverviewTab />}
        {activeTab === 'collections' && <CollectionsTab />}
      </main>
    </div>
  );
};

export default Dashboard;
