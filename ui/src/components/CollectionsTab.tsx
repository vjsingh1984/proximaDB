import React, { useEffect, useState } from 'react';
import { Collection } from '../types/api';
import { proximaDbApi } from '../api/proximaDbApi';
import './CollectionsTab.css';

const CollectionsTab: React.FC = () => {
  const [collections, setCollections] = useState<Collection[]>([]);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fetchCollections = async () => {
      try {
        setLoading(true);
        const data = await proximaDbApi.listCollections();
        setCollections(data);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'An unknown error occurred');
      } finally {
        setLoading(false);
      }
    };

    fetchCollections();
  }, []);

  if (loading) {
    return <div className="collections-loading">Loading collections...</div>;
  }

  if (error) {
    return <div className="collections-error">Error: {error}</div>;
  }

  if (collections.length === 0) {
    return <div className="collections-no-data">No collections found.</div>;
  }

  return (
    <div className="collections-container">
      <h2>Collections</h2>
      <table className="collections-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>ID</th>
            <th>Vector Count</th>
            <th>Index Size (GB)</th>
            <th>Data Size (GB)</th>
            <th>Distance Metric</th>
            <th>Storage Engine</th>
          </tr>
        </thead>
        <tbody>
          {collections.map((collection) => (
            <tr key={collection.id}>
              <td>{collection.config.name}</td>
              <td>{collection.id}</td>
              <td>{collection.stats.vector_count}</td>
              <td>{(parseInt(collection.stats.index_size_bytes) / (1024 * 1024 * 1024)).toFixed(2)}</td>
              <td>{(parseInt(collection.stats.data_size_bytes) / (1024 * 1024 * 1024)).toFixed(2)}</td>
              <td>{collection.config.distance_metric}</td>
              <td>{collection.config.storage_engine}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

export default CollectionsTab;
