import React, { useState, useEffect } from 'react';
import axios from 'axios';

function App() {
  const [collections, setCollections] = useState([]);

  useEffect(() => {
    axios.get('/api/v1/collections')
      .then(response => {
        setCollections(response.data.collections);
      })
      .catch(error => {
        console.error('Error fetching collections:', error);
      });
  }, []);

  return (
    <div className="App">
      <header className="App-header">
        <h1>ProximaDB</h1>
      </header>
      <div className="container">
        <h2>Collections</h2>
        <ul>
          {collections.map(collection => (
            <li key={collection.id}>{collection.name}</li>
          ))}
        </ul>
      </div>
    </div>
  );
}

export default App;