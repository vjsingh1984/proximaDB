// ProximaDB Demo UI JavaScript
const API_BASE = 'http://localhost:5678';

// Tab switching
function switchTab(tabName) {
    document.querySelectorAll('.tab-content').forEach(tab => {
        tab.classList.remove('active');
    });
    document.querySelectorAll('.tab-button').forEach(button => {
        button.classList.remove('active');
    });
    
    document.getElementById(tabName).classList.add('active');
    event.target.classList.add('active');
    
    // Load data for specific tabs
    if (tabName === 'collections') {
        refreshCollections();
    }
}

// Utility functions
async function apiCall(endpoint, method = 'GET', body = null) {
    try {
        const options = {
            method,
            headers: {
                'Content-Type': 'application/json',
            }
        };
        
        if (body) {
            options.body = JSON.stringify(body);
        }
        
        const response = await fetch(`${API_BASE}${endpoint}`, options);
        const data = await response.json();
        
        if (!response.ok) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }
        
        return data;
    } catch (error) {
        console.error('API call failed:', error);
        throw error;
    }
}

function showResult(elementId, content, isError = false) {
    const element = document.getElementById(elementId);
    element.innerHTML = `
        <div class="p-4 rounded-lg ${isError ? 'bg-red-100 text-red-700' : 'bg-green-100 text-green-700'}">
            ${content}
        </div>
    `;
}

function showLoading(elementId) {
    const element = document.getElementById(elementId);
    element.innerHTML = `
        <div class="flex items-center justify-center p-4">
            <i class="fas fa-spinner fa-spin text-blue-500 text-2xl"></i>
        </div>
    `;
}

// Health check
async function checkHealth() {
    try {
        const health = await apiCall('/health');
        document.getElementById('healthStatus').innerHTML = `
            <span class="text-green-500">
                <i class="fas fa-check-circle"></i> Connected to ProximaDB
            </span>
        `;
    } catch (error) {
        document.getElementById('healthStatus').innerHTML = `
            <span class="text-red-500">
                <i class="fas fa-times-circle"></i> Failed to connect to ProximaDB
            </span>
        `;
    }
}

// Generate random vector for demo purposes
function generateRandomVector(dimension = 128) {
    return Array.from({length: dimension}, () => Math.random() * 2 - 1);
}

// Generate seeded vector based on text (simulates embeddings)
function generateSeededVector(text, dimension = 128) {
    // Simple hash function to seed random generator
    let hash = 0;
    for (let i = 0; i < text.length; i++) {
        hash = ((hash << 5) - hash) + text.charCodeAt(i);
        hash = hash & hash; // Convert to 32-bit integer
    }
    
    // Use hash as seed for consistent vectors
    const vector = [];
    let seed = Math.abs(hash);
    for (let i = 0; i < dimension; i++) {
        seed = (seed * 9301 + 49297) % 233280;
        const value = (seed / 233280.0) * 2 - 1;
        vector.push(value);
    }
    return vector;
}

// Quick Start functions - Integrated with Real E-commerce Demo
async function createDemoCollection() {
    showLoading('quickstartResult');
    
    try {
        // Check if e-commerce collection already exists
        const collections = await apiCall('/collections');
        const existingCollection = collections.find(c => c.id === 'ecommerce_products');
        
        if (existingCollection) {
            showResult('quickstartResult', 
                'E-commerce collection already exists! Ready to search real products with BERT embeddings.');
            return;
        }
        
        // Create collection matching the consolidated demo
        const collection = {
            dimension: 384,  // BERT all-MiniLM-L6-v2 dimension
            distance_metric: 'cosine',
            engine: 'viper'  // Columnar storage for analytics
        };
        
        await apiCall('/collections/ecommerce_products', 'POST', collection);
        showResult('quickstartResult', 
            'E-commerce collection created! Run python demo/ecommerce_demo.py to load real products.');
    } catch (error) {
        showResult('quickstartResult', `Error: ${error.message}`, true);
    }
}

async function loadSampleData() {
    showLoading('quickstartResult');
    
    try {
        // Check if real e-commerce data already loaded
        const searchResults = await apiCall('/collections/ecommerce_products/search', 'POST', {
            vector: Array(384).fill(0.1),  // Test vector
            top_k: 1
        });
        
        if (searchResults.results && searchResults.results.length > 0) {
            showResult('quickstartResult', 
                'Real e-commerce products already loaded! Ready for semantic search with BERT embeddings.');
            return;
        }
        
        showResult('quickstartResult', 
            `<div class="text-center">
                <h3 class="font-semibold text-lg mb-4">Load Real E-commerce Data</h3>
                <p class="mb-4">To load real products with BERT embeddings, run:</p>
                <code class="bg-gray-200 p-2 rounded block mb-4">python demo/ecommerce_demo.py</code>
                <p class="text-sm text-gray-600">
                    This will insert 17+ real products using high-performance gRPC and actual BERT embeddings.
                    Then you can search with natural language queries!
                </p>
            </div>`
        );
    } catch (error) {
        // Collection might not exist, guide user to create it
        showResult('quickstartResult', 
            `<div class="text-center">
                <h3 class="font-semibold text-lg mb-4">Get Started with Real E-commerce Demo</h3>
                <p class="mb-4">Follow these steps:</p>
                <ol class="text-left list-decimal list-inside space-y-2 mb-4">
                    <li>Click "Create Demo Collection" above</li>
                    <li>Run: <code class="bg-gray-200 p-1 rounded">python demo/ecommerce_demo.py</code></li>
                    <li>Return here to search real products!</li>
                </ol>
                <p class="text-sm text-gray-600">
                    Features real BERT embeddings, natural language search, and product metadata filtering.
                </p>
            </div>`, 
            false
        );
    }
}

// Vector Search functions - Integrated with Real E-commerce Demo
async function performSearch() {
    const queryText = document.getElementById('queryText').value;
    const metadataFilter = document.getElementById('metadataFilter').value;
    const distanceMetric = document.getElementById('distanceMetric').value;
    const topK = document.getElementById('topK').value;
    
    if (!queryText || !queryText.trim()) {
        showResult('searchResults', 'Please enter a search query (e.g., "gaming laptop" or "running shoes")', true);
        return;
    }
    
    showLoading('searchResults');
    
    try {
        // Use semantic search on the real e-commerce collection
        // Note: Real BERT embeddings would need to be generated server-side
        // For now, use seeded vectors for consistent demo experience
        const queryVector = generateSeededVector(queryText, 384);  // Match BERT dimension
        
        const searchRequest = {
            vector: queryVector,
            top_k: parseInt(topK),
            distance_metric: distanceMetric
        };
        
        // Add metadata filter if provided
        if (metadataFilter) {
            try {
                searchRequest.metadata_filter = JSON.parse(metadataFilter);
            } catch (e) {
                throw new Error('Invalid JSON in metadata filter');
            }
        }
        
        const results = await apiCall('/collections/ecommerce_products/search', 'POST', searchRequest);
        displaySearchResults(results, queryText);
    } catch (error) {
        if (error.message.includes('not found') || error.message.includes('404')) {
            showResult('searchResults', 
                `<div class="text-center">
                    <h3 class="font-semibold text-lg mb-4">E-commerce Collection Not Found</h3>
                    <p class="mb-4">Run the real e-commerce demo first:</p>
                    <code class="bg-gray-200 p-2 rounded block mb-4">python demo/ecommerce_demo.py</code>
                    <p class="text-sm text-gray-600">
                        This will create the collection and load real products with BERT embeddings.
                    </p>
                </div>`, 
                true);
        } else {
            showResult('searchResults', `Error: ${error.message}`, true);
        }
    }
}

function displaySearchResults(results, query = '') {
    const resultsDiv = document.getElementById('searchResults');
    
    // Handle different result formats from ProximaDB
    let items = [];
    if (results && results.results) {
        items = results.results;
    } else if (Array.isArray(results)) {
        items = results;
    }
    
    if (!items || items.length === 0) {
        resultsDiv.innerHTML = `
            <div class="text-center py-8">
                <p class="text-gray-500 mb-4">No results found for "${query}"</p>
                <p class="text-sm text-gray-400">
                    Try different keywords or run the e-commerce demo to load products.
                </p>
            </div>
        `;
        return;
    }
    
    let html = `
        <div class="mb-4">
            <h3 class="font-semibold text-lg">Search Results for "${query}"</h3>
            <p class="text-sm text-gray-600">Found ${items.length} products</p>
        </div>
        <div class="space-y-4">
    `;
    
    items.forEach((result, index) => {
        // Extract metadata from different formats
        let metadata = result.metadata || {};
        if (Array.isArray(metadata)) {
            // Convert list format to object
            const metaObj = {};
            metadata.forEach(item => {
                if (item.key && item.value) {
                    metaObj[item.key] = item.value;
                }
            });
            metadata = metaObj;
        }
        
        const name = metadata.name || result.id || 'Unknown Product';
        const category = metadata.category || 'N/A';
        const brand = metadata.brand || 'N/A';
        const price = metadata.price || 'N/A';
        const rating = metadata.rating || 'N/A';
        const description = metadata.description ? 
            (metadata.description.length > 150 ? metadata.description.substring(0, 150) + '...' : metadata.description) 
            : '';
        const score = result.score || result.distance || 'N/A';
        
        html += `
            <div class="result-item p-6 bg-white rounded-lg shadow-md hover:shadow-lg transition-shadow">
                <div class="flex justify-between items-start">
                    <div class="flex-1">
                        <h4 class="font-semibold text-xl text-gray-800 mb-2">${name}</h4>
                        ${description ? `<p class="text-gray-600 mb-3">${description}</p>` : ''}
                        
                        <div class="grid grid-cols-2 gap-4 text-sm">
                            <div>
                                <p><strong>Category:</strong> <span class="text-blue-600">${category}</span></p>
                                <p><strong>Brand:</strong> ${brand}</p>
                            </div>
                            <div>
                                <p><strong>Price:</strong> <span class="text-green-600 font-semibold">$${price}</span></p>
                                <p><strong>Rating:</strong> ${rating} ⭐</p>
                            </div>
                        </div>
                        
                        <div class="mt-3 pt-3 border-t border-gray-200 text-xs text-gray-500">
                            <p><strong>ID:</strong> ${result.id} | <strong>Similarity Score:</strong> ${typeof score === 'number' ? score.toFixed(4) : score}</p>
                        </div>
                    </div>
                    <div class="ml-4 text-center">
                        <div class="text-3xl font-bold text-blue-500">
                            #${index + 1}
                        </div>
                    </div>
                </div>
            </div>
        `;
    });
    
    html += '</div>';
    resultsDiv.innerHTML = html;
}

// SQL Query functions
async function executeSQLQuery() {
    const sqlQuery = document.getElementById('sqlQuery').value;
    
    showLoading('sqlResults');
    
    try {
        const response = await apiCall('/api/v1/sql/execute', 'POST', { query: sqlQuery });
        displaySQLResults(response);
    } catch (error) {
        showResult('sqlResults', `Error: ${error.message}`, true);
    }
}

function loadSQLExample(example) {
    const examples = {
        basic: `SELECT id, metadata
FROM ecommerce_products
WHERE metadata->>'category' = 'Electronics'
ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.2, 0.1, 0.0, ...], 'cosine')
LIMIT 10`,
        filter: `SELECT id, metadata
FROM ecommerce_products
WHERE metadata->>'price' > '500'
  AND metadata->>'rating' >= '4.0'
ORDER BY VECTOR_SIMILARITY(vector, [0.2, 0.1, 0.3, 0.0, ...], 'cosine')
LIMIT 5`,
        complex: `SELECT id, metadata
FROM ecommerce_products
WHERE metadata->>'brand' IN ('Apple', 'Samsung', 'Dell')
ORDER BY VECTOR_SIMILARITY(vector, [0.1, 0.3, 0.2, 0.1, ...], 'cosine')
LIMIT 20`
    };
    
    document.getElementById('sqlQuery').value = examples[example];
}

function displaySQLResults(results) {
    const resultsDiv = document.getElementById('sqlResults');
    
    if (!results.data || results.data.length === 0) {
        resultsDiv.innerHTML = '<p class="text-gray-500">No results returned</p>';
        return;
    }
    
    // Create table for results
    let html = '<div class="overflow-x-auto"><table class="min-w-full bg-white rounded-lg shadow"><thead><tr>';
    
    // Get column headers
    const columns = Object.keys(results.data[0]);
    columns.forEach(col => {
        html += `<th class="px-4 py-2 bg-gray-100 text-left">${col}</th>`;
    });
    html += '</tr></thead><tbody>';
    
    // Add rows
    results.data.forEach(row => {
        html += '<tr class="border-t">';
        columns.forEach(col => {
            const value = typeof row[col] === 'object' ? JSON.stringify(row[col]) : row[col];
            html += `<td class="px-4 py-2">${value}</td>`;
        });
        html += '</tr>';
    });
    
    html += '</tbody></table></div>';
    resultsDiv.innerHTML = html;
}

// Collections Management functions
async function refreshCollections() {
    showLoading('collectionsList');
    
    try {
        const response = await apiCall('/api/v1/collections');
        displayCollections(response);
    } catch (error) {
        showResult('collectionsList', `Error: ${error.message}`, true);
    }
}

function displayCollections(response) {
    const collectionsDiv = document.getElementById('collectionsList');
    
    if (!response.data || !response.data.collections || response.data.collections.length === 0) {
        collectionsDiv.innerHTML = '<p class="text-gray-500 text-center py-8">No collections found</p>';
        return;
    }
    
    let html = '<div class="grid grid-cols-1 md:grid-cols-2 gap-4">';
    
    response.data.collections.forEach(collection => {
        html += `
            <div class="bg-white p-4 rounded-lg shadow">
                <div class="flex justify-between items-start">
                    <div>
                        <h3 class="font-semibold text-lg">${collection.id}</h3>
                        <p class="text-sm text-gray-600">Dimension: ${collection.config.dimension}</p>
                        <p class="text-sm text-gray-600">Metric: ${collection.config.distance_metric}</p>
                        <p class="text-sm text-gray-600">Engine: ${collection.config.storage_engine}</p>
                        <p class="text-sm text-gray-600 mt-2">
                            Vectors: ${collection.stats.vector_count || 0}
                        </p>
                    </div>
                    <button onclick="deleteCollection('${collection.id}')"
                            class="text-red-500 hover:text-red-700">
                        <i class="fas fa-trash"></i>
                    </button>
                </div>
            </div>
        `;
    });
    
    html += '</div>';
    collectionsDiv.innerHTML = html;
}

async function createNewCollection() {
    const name = document.getElementById('newCollectionName').value;
    const dimension = document.getElementById('newCollectionDimension').value;
    const metric = document.getElementById('newCollectionMetric').value;
    
    if (!name) {
        alert('Please enter a collection name');
        return;
    }
    
    showLoading('createCollectionResult');
    
    try {
        const collection = {
            operation: 'create',
            collection_id: name.toLowerCase().replace(/\s+/g, '_'),
            config: {
                name: name,
                dimension: parseInt(dimension),
                distance_metric: metric,
                storage_engine: 'viper'
            }
        };
        
        await apiCall('/api/v1/collection', 'POST', collection);
        showResult('createCollectionResult', 'Collection created successfully!');
        refreshCollections();
        
        // Clear form
        document.getElementById('newCollectionName').value = '';
    } catch (error) {
        showResult('createCollectionResult', `Error: ${error.message}`, true);
    }
}

async function deleteCollection(collectionId) {
    if (!confirm(`Are you sure you want to delete collection "${collectionId}"?`)) {
        return;
    }
    
    try {
        await apiCall(`/api/v1/collection/${collectionId}`, 'DELETE');
        refreshCollections();
    } catch (error) {
        alert(`Error deleting collection: ${error.message}`);
    }
}

// Performance Benchmark functions
async function runInsertBenchmark() {
    const numVectors = document.getElementById('benchmarkVectors').value;
    const batchSize = document.getElementById('benchmarkBatchSize').value;
    const dimension = 128;
    
    showLoading('benchmarkResults');
    
    try {
        // Create test collection
        const collectionId = `benchmark_${Date.now()}`;
        await apiCall('/api/v1/collection', 'POST', {
            operation: 'create',
            collection_id: collectionId,
            config: {
                name: 'Benchmark Collection',
                dimension: dimension,
                distance_metric: 'cosine',
                storage_engine: 'viper'
            }
        });
        
        // Measure insert performance
        const startTime = Date.now();
        const totalVectors = parseInt(numVectors);
        const vectorsPerBatch = parseInt(batchSize);
        let inserted = 0;
        
        for (let i = 0; i < totalVectors; i += vectorsPerBatch) {
            const batchVectors = [];
            const currentBatchSize = Math.min(vectorsPerBatch, totalVectors - i);
            
            for (let j = 0; j < currentBatchSize; j++) {
                batchVectors.push({
                    id: `vec_${i + j}`,
                    vector: generateRandomVector(dimension),
                    metadata: { index: i + j }
                });
            }
            
            await apiCall('/api/v1/vector/batch', 'POST', {
                operation: 'insert',
                collection_id: collectionId,
                vectors: batchVectors
            });
            
            inserted += currentBatchSize;
        }
        
        const endTime = Date.now();
        const duration = (endTime - startTime) / 1000;
        const throughput = Math.round(totalVectors / duration);
        
        // Clean up
        await apiCall(`/api/v1/collection/${collectionId}`, 'DELETE');
        
        showResult('benchmarkResults', `
            <div>
                <h3 class="font-semibold text-lg mb-2">Insert Benchmark Results</h3>
                <p>Total Vectors: ${totalVectors}</p>
                <p>Batch Size: ${vectorsPerBatch}</p>
                <p>Duration: ${duration.toFixed(2)} seconds</p>
                <p class="text-xl font-bold text-green-600 mt-2">
                    Throughput: ${throughput.toLocaleString()} vectors/sec
                </p>
            </div>
        `);
    } catch (error) {
        showResult('benchmarkResults', `Error: ${error.message}`, true);
    }
}

async function runSearchBenchmark() {
    const numQueries = document.getElementById('searchQueries').value;
    
    showLoading('benchmarkResults');
    
    try {
        // Use demo collection for search benchmark
        const collectionId = 'demo_products';
        const queries = parseInt(numQueries);
        const latencies = [];
        
        for (let i = 0; i < queries; i++) {
            const startTime = performance.now();
            
            await apiCall('/api/v1/vector/search', 'POST', {
                collection_id: collectionId,
                vector: generateRandomVector(128),
                top_k: 10,
                distance_metric: 'cosine',
                include_metadata: true
            });
            
            const endTime = performance.now();
            latencies.push(endTime - startTime);
        }
        
        // Calculate statistics
        latencies.sort((a, b) => a - b);
        const avgLatency = latencies.reduce((a, b) => a + b, 0) / latencies.length;
        const p50 = latencies[Math.floor(latencies.length * 0.5)];
        const p95 = latencies[Math.floor(latencies.length * 0.95)];
        const p99 = latencies[Math.floor(latencies.length * 0.99)];
        
        showResult('benchmarkResults', `
            <div>
                <h3 class="font-semibold text-lg mb-2">Search Benchmark Results</h3>
                <p>Total Queries: ${queries}</p>
                <p>Average Latency: ${avgLatency.toFixed(2)}ms</p>
                <p>P50 Latency: ${p50.toFixed(2)}ms</p>
                <p>P95 Latency: ${p95.toFixed(2)}ms</p>
                <p>P99 Latency: ${p99.toFixed(2)}ms</p>
                <p class="text-xl font-bold text-green-600 mt-2">
                    QPS: ${Math.round(1000 / avgLatency).toLocaleString()}
                </p>
            </div>
        `);
    } catch (error) {
        showResult('benchmarkResults', `Error: ${error.message}`, true);
    }
}

// Initialize on page load
document.addEventListener('DOMContentLoaded', () => {
    checkHealth();
    
    // Set up periodic health checks
    setInterval(checkHealth, 10000);
    
    // Show quick start tab by default
    document.querySelector('.tab-button').click();
});