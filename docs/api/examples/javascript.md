# ProximaDB JavaScript Client Examples

Complete examples for using ProximaDB with JavaScript/TypeScript clients for both REST and gRPC APIs.

## Installation

```bash
# Install the official JavaScript client
npm install proximadb-js

# Or install dependencies for manual integration
npm install axios grpc @grpc/grpc-js @grpc/proto-loader
```

## REST API Examples

### Basic Setup

```javascript
import axios from 'axios';

class ProximaDBRestClient {
  constructor(baseUrl = 'http://localhost:5678', apiKey = null) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.headers = {
      'Content-Type': 'application/json'
    };
    if (apiKey) {
      this.headers['Authorization'] = `Bearer ${apiKey}`;
    }
  }

  async request(method, endpoint, data = null) {
    try {
      const response = await axios({
        method,
        url: `${this.baseUrl}${endpoint}`,
        headers: this.headers,
        data
      });
      return response.data;
    } catch (error) {
      if (error.response) {
        throw new ProximaDBError(
          error.response.data?.error?.code || 'UNKNOWN_ERROR',
          error.response.data?.error?.message || error.message,
          error.response.data?.error?.details || {}
        );
      }
      throw error;
    }
  }
}

class ProximaDBError extends Error {
  constructor(code, message, details = {}) {
    super(`${code}: ${message}`);
    this.code = code;
    this.details = details;
  }
}

// Initialize client
const client = new ProximaDBRestClient('http://localhost:5678', 'pk_live_1234567890abcdef');
```

### Collection Management

```javascript
// Create a collection
async function createCollectionExample() {
  const collectionData = {
    collection_id: 'js_embeddings',
    dimension: 384,
    distance_metric: 'cosine',
    description: 'JavaScript document embeddings'
  };
  
  try {
    const result = await client.request('POST', '/collections', collectionData);
    console.log(`Collection created: ${result.collection_id}`);
    return result;
  } catch (error) {
    if (error.code === 'COLLECTION_ALREADY_EXISTS') {
      console.log('Collection already exists, continuing...');
      return { collection_id: collectionData.collection_id };
    }
    throw error;
  }
}

// List all collections
async function listCollectionsExample() {
  const collections = await client.request('GET', '/collections');
  console.log(`Found ${collections.collections.length} collections:`);
  
  collections.collections.forEach(collection => {
    console.log(`- ${collection.id}: ${collection.vector_count} vectors`);
  });
  
  return collections;
}

// Get collection details
async function getCollectionExample(collectionId) {
  const collection = await client.request('GET', `/collections/${collectionId}`);
  console.log(`Collection ${collection.id}:`);
  console.log(`  Dimension: ${collection.dimension}`);
  console.log(`  Vectors: ${collection.vector_count}`);
  console.log(`  Size: ${collection.total_size_bytes} bytes`);
  return collection;
}

// Delete a collection
async function deleteCollectionExample(collectionId) {
  const result = await client.request('DELETE', `/collections/${collectionId}`);
  console.log(`Collection deleted: ${result.message}`);
  return result;
}
```

### Vector Operations

```javascript
import { v4 as uuidv4 } from 'uuid';

// Insert a single vector
async function insertVectorExample() {
  const vectorData = {
    id: uuidv4(),
    vector: Array(384).fill(0).map(() => Math.random()),
    metadata: {
      document_id: 'js_doc_001',
      title: 'JavaScript Example Document',
      category: 'programming',
      tags: ['javascript', 'nodejs', 'api']
    }
  };
  
  const result = await client.request('POST', '/collections/js_embeddings/vectors', vectorData);
  console.log(`Vector inserted: ${result.vector_id}`);
  return result;
}

// Get a vector by ID
async function getVectorExample(collectionId, vectorId) {
  const vector = await client.request('GET', `/collections/${collectionId}/vectors/${vectorId}`);
  console.log(`Vector ${vector.id}:`);
  console.log(`  Dimension: ${vector.vector.length}`);
  console.log(`  Metadata: ${JSON.stringify(vector.metadata, null, 2)}`);
  return vector;
}

// Search for similar vectors
async function searchVectorsExample() {
  const queryVector = Array(384).fill(0).map(() => Math.random());
  
  const searchData = {
    vector: queryVector,
    k: 10,
    filter: {
      category: 'programming'
    },
    threshold: 0.7
  };
  
  const results = await client.request('POST', '/collections/js_embeddings/search', searchData);
  
  console.log(`Found ${results.total_count} similar vectors:`);
  results.results.forEach(result => {
    console.log(`- ${result.id}: score=${result.score.toFixed(3)}`);
    if (result.metadata) {
      console.log(`  Title: ${result.metadata.title || 'N/A'}`);
    }
  });
  
  return results;
}

// Delete a vector
async function deleteVectorExample(collectionId, vectorId) {
  const result = await client.request('DELETE', `/collections/${collectionId}/vectors/${vectorId}`);
  console.log(`Vector deleted: ${result.message}`);
  return result;
}
```

### Batch Operations

```javascript
// Batch insert vectors
async function batchInsertExample() {
  const vectors = [];
  for (let i = 0; i < 100; i++) {
    vectors.push({
      id: uuidv4(),
      vector: Array(384).fill(0).map(() => Math.random()),
      metadata: {
        document_id: `js_doc_${i.toString().padStart(3, '0')}`,
        batch: 'js_example_batch',
        index: i
      }
    });
  }
  
  const batchData = { vectors };
  const result = await client.request('POST', '/collections/js_embeddings/vectors/batch', batchData);
  
  console.log(`Batch inserted ${result.inserted_count} vectors`);
  if (result.errors && result.errors.length > 0) {
    console.log(`Errors: ${result.errors}`);
  }
  
  return result;
}

// Batch search
async function batchSearchExample() {
  const searches = [];
  for (let i = 0; i < 5; i++) {
    searches.push({
      collection_id: 'js_embeddings',
      vector: Array(384).fill(0).map(() => Math.random()),
      k: 5,
      filter: { batch: 'js_example_batch' }
    });
  }
  
  const batchData = { searches };
  const results = await client.request('POST', '/batch/search', batchData);
  
  console.log(`Performed ${results.results.length} searches:`);
  results.results.forEach((searchResult, i) => {
    console.log(`  Search ${i}: ${searchResult.total_count} results`);
  });
  
  return results;
}
```

### Complete Example Application

```javascript
/**
 * ProximaDB REST API Example Application
 * Demonstrates document similarity search using embeddings.
 */

import * as tf from '@tensorflow/tfjs-node';
import { UniversalSentenceEncoder } from '@tensorflow-models/universal-sentence-encoder';

class DocumentSearchApp {
  constructor(apiKey = null) {
    this.client = new ProximaDBRestClient('http://localhost:5678', apiKey);
    this.model = null;
    this.collectionId = 'js_document_search';
  }
  
  async initialize() {
    // Load the Universal Sentence Encoder model
    console.log('Loading sentence encoder model...');
    this.model = await UniversalSentenceEncoder.load();
    console.log('Model loaded successfully');
  }
  
  async setup() {
    // Initialize the collection for document search
    try {
      const collectionData = {
        collection_id: this.collectionId,
        dimension: 512, // USE model outputs 512-dimensional embeddings
        distance_metric: 'cosine',
        description: 'JavaScript document similarity search'
      };
      await this.client.request('POST', '/collections', collectionData);
      console.log('Collection created successfully');
    } catch (error) {
      if (error.code === 'COLLECTION_ALREADY_EXISTS') {
        console.log('Collection already exists');
      } else {
        throw error;
      }
    }
  }
  
  async addDocuments(documents) {
    // Add documents to the search index
    const vectors = [];
    
    for (const doc of documents) {
      // Generate embedding
      const embeddings = await this.model.embed([doc.content]);
      const embedding = await embeddings.array();
      
      vectors.push({
        vector: embedding[0], // Get the first (and only) embedding
        metadata: {
          title: doc.title,
          content: doc.content.substring(0, 200), // Truncate for storage
          author: doc.author || 'Unknown',
          category: doc.category || 'general'
        }
      });
      
      embeddings.dispose(); // Clean up tensors
    }
    
    // Batch insert
    const batchData = { vectors };
    const result = await this.client.request('POST', `/collections/${this.collectionId}/vectors/batch`, batchData);
    
    console.log(`Added ${result.inserted_count} documents to search index`);
    return result;
  }
  
  async searchDocuments(query, k = 5, categoryFilter = null) {
    // Search for similar documents
    const embeddings = await this.model.embed([query]);
    const queryEmbedding = await embeddings.array();
    
    const searchData = {
      vector: queryEmbedding[0],
      k,
      threshold: 0.3
    };
    
    // Add category filter if specified
    if (categoryFilter) {
      searchData.filter = { category: categoryFilter };
    }
    
    const results = await this.client.request('POST', `/collections/${this.collectionId}/search`, searchData);
    
    embeddings.dispose(); // Clean up tensors
    
    return results.results.map(result => ({
      id: result.id,
      score: result.score,
      title: result.metadata.title,
      author: result.metadata.author,
      content: result.metadata.content
    }));
  }
}

async function main() {
  // Initialize the application
  const app = new DocumentSearchApp('pk_test_example123');
  await app.initialize();
  await app.setup();
  
  // Sample documents
  const documents = [
    {
      title: 'Introduction to Machine Learning',
      content: 'Machine learning is a subset of artificial intelligence that focuses on algorithms that can learn from data.',
      author: 'Dr. Smith',
      category: 'education'
    },
    {
      title: 'Deep Learning Fundamentals',
      content: 'Deep learning uses neural networks with multiple layers to model complex patterns in data.',
      author: 'Prof. Johnson',
      category: 'education'
    },
    {
      title: 'JavaScript Best Practices',
      content: 'JavaScript is a versatile programming language that runs in browsers and servers, known for its flexibility.',
      author: 'Dev Expert',
      category: 'programming'
    },
    {
      title: 'Node.js Development Guide',
      content: 'Node.js allows JavaScript to run on the server, enabling full-stack development with a single language.',
      author: 'Backend Guru',
      category: 'programming'
    }
  ];
  
  // Add documents to the index
  await app.addDocuments(documents);
  
  // Search for documents
  console.log('\n--- Search Results ---');
  const queries = [
    'artificial intelligence algorithms',
    'neural networks and deep learning',
    'JavaScript programming languages',
    'server-side development'
  ];
  
  for (const query of queries) {
    console.log(`\nQuery: '${query}'`);
    const results = await app.searchDocuments(query, 3);
    
    results.forEach((doc, i) => {
      console.log(`${i + 1}. ${doc.title} (score: ${doc.score.toFixed(3)})`);
      console.log(`   Author: ${doc.author}`);
      console.log(`   Content: ${doc.content}...`);
    });
  }
}

// Export for use in other modules
export { ProximaDBRestClient, DocumentSearchApp };

// Run if this is the main module
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch(console.error);
}
```

## gRPC API Examples

### Basic Setup

```javascript
import grpc from '@grpc/grpc-js';
import protoLoader from '@grpc/proto-loader';
import path from 'path';

// Load the protobuf definition
const PROTO_PATH = path.join(__dirname, '../../../proto/vectordb.proto');
const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
  keepCase: true,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true
});

const vectordbProto = grpc.loadPackageDefinition(packageDefinition);

class ProximaDBGrpcClient {
  constructor(host = 'localhost', port = 9090, apiKey = null) {
    this.host = host;
    this.port = port;
    this.apiKey = apiKey;
    this.client = new vectordbProto.VectorDB(
      `${host}:${port}`,
      grpc.credentials.createInsecure()
    );
  }
  
  getMetadata() {
    const metadata = new grpc.Metadata();
    if (this.apiKey) {
      metadata.add('authorization', `Bearer ${this.apiKey}`);
    }
    return metadata;
  }
  
  promisify(method) {
    return (request) => {
      return new Promise((resolve, reject) => {
        method.call(this.client, request, this.getMetadata(), (error, response) => {
          if (error) {
            reject(error);
          } else {
            resolve(response);
          }
        });
      });
    };
  }
}

// Initialize client
const grpcClient = new ProximaDBGrpcClient('localhost', 9090, 'pk_live_1234567890abcdef');
```

### Collection Management with gRPC

```javascript
async function createCollectionGrpc() {
  const createCollection = grpcClient.promisify(grpcClient.client.CreateCollection);
  
  const request = {
    collection_id: 'grpc_js_embeddings',
    name: 'gRPC JavaScript Document Embeddings',
    dimension: 384,
    schema_type: 'SCHEMA_TYPE_DOCUMENT'
  };
  
  try {
    const response = await createCollection(request);
    console.log(`Collection created: ${response.success}`);
    return response;
  } catch (error) {
    console.error('Error creating collection:', error.message);
    throw error;
  }
}

async function listCollectionsGrpc() {
  const listCollections = grpcClient.promisify(grpcClient.client.ListCollections);
  const response = await listCollections({});
  
  console.log(`Found ${response.collections.length} collections:`);
  response.collections.forEach(collection => {
    console.log(`- ${collection.id}: ${collection.vector_count} vectors`);
  });
  
  return response;
}

async function deleteCollectionGrpc(collectionId) {
  const deleteCollection = grpcClient.promisify(grpcClient.client.DeleteCollection);
  const response = await deleteCollection({ collection_id: collectionId });
  console.log(`Collection deleted: ${response.success}`);
  return response;
}
```

### Vector Operations with gRPC

```javascript
function createMetadataStruct(metadataObject) {
  // Convert JavaScript object to protobuf Struct
  const struct = {};
  for (const [key, value] of Object.entries(metadataObject)) {
    if (typeof value === 'string') {
      struct[key] = { string_value: value };
    } else if (typeof value === 'number') {
      struct[key] = { number_value: value };
    } else if (typeof value === 'boolean') {
      struct[key] = { bool_value: value };
    } else if (Array.isArray(value)) {
      struct[key] = { 
        list_value: { 
          values: value.map(v => ({ string_value: v.toString() })) 
        } 
      };
    }
  }
  return { fields: struct };
}

async function insertVectorGrpc() {
  const insert = grpcClient.promisify(grpcClient.client.Insert);
  
  // Create metadata
  const metadata = createMetadataStruct({
    document_id: 'grpc_js_doc_001',
    title: 'gRPC JavaScript Example Document',
    category: 'technical',
    tags: ['grpc', 'javascript', 'api']
  });
  
  // Create vector record
  const record = {
    id: uuidv4(),
    collection_id: 'grpc_js_embeddings',
    vector: Array(384).fill(0).map(() => Math.random()),
    metadata: metadata
  };
  
  const request = {
    collection_id: 'grpc_js_embeddings',
    record: record
  };
  
  const response = await insert(request);
  console.log(`Vector inserted: ${response.vector_id}`);
  return response;
}

async function searchVectorsGrpc() {
  const search = grpcClient.promisify(grpcClient.client.Search);
  const queryVector = Array(384).fill(0).map(() => Math.random());
  
  // Create filter
  const filters = createMetadataStruct({ category: 'technical' });
  
  const request = {
    collection_id: 'grpc_js_embeddings',
    vector: queryVector,
    k: 10,
    filters: filters,
    threshold: 0.7
  };
  
  const response = await search(request);
  
  console.log(`Found ${response.total_count} similar vectors:`);
  response.results.forEach(result => {
    console.log(`- ${result.id}: score=${result.score.toFixed(3)}`);
    if (result.metadata && result.metadata.fields.title) {
      console.log(`  Title: ${result.metadata.fields.title.string_value}`);
    }
  });
  
  return response;
}

async function batchInsertGrpc() {
  const batchInsert = grpcClient.promisify(grpcClient.client.BatchInsert);
  
  const records = [];
  for (let i = 0; i < 50; i++) {
    const metadata = createMetadataStruct({
      document_id: `grpc_js_batch_${i.toString().padStart(3, '0')}`,
      index: i,
      batch: 'grpc_js_example'
    });
    
    const record = {
      collection_id: 'grpc_js_embeddings',
      vector: Array(384).fill(0).map(() => Math.random()),
      metadata: metadata
    };
    records.push(record);
  }
  
  const request = {
    collection_id: 'grpc_js_embeddings',
    records: records
  };
  
  const response = await batchInsert(request);
  console.log(`Batch inserted ${response.inserted_count} vectors`);
  
  if (response.errors && response.errors.length > 0) {
    console.log(`Errors: ${response.errors}`);
  }
  
  return response;
}
```

### Advanced gRPC Example with Async/Await

```javascript
import { promisify } from 'util';

class AsyncProximaDBClient {
  constructor(host = 'localhost', port = 9090, apiKey = null) {
    this.host = host;
    this.port = port;
    this.apiKey = apiKey;
    this.client = new vectordbProto.VectorDB(
      `${host}:${port}`,
      grpc.credentials.createInsecure()
    );
    
    // Promisify all methods
    this.createCollection = promisify(this.client.CreateCollection.bind(this.client));
    this.search = promisify(this.client.Search.bind(this.client));
    this.batchInsert = promisify(this.client.BatchInsert.bind(this.client));
  }
  
  getMetadata() {
    const metadata = new grpc.Metadata();
    if (this.apiKey) {
      metadata.add('authorization', `Bearer ${this.apiKey}`);
    }
    return metadata;
  }
  
  async parallelSearch(queries, collectionId, k = 10) {
    // Perform multiple searches in parallel
    const searchPromises = queries.map(queryVector => {
      const request = {
        collection_id: collectionId,
        vector: queryVector,
        k
      };
      return this.search(request, this.getMetadata());
    });
    
    const results = await Promise.all(searchPromises);
    return results;
  }
  
  async streamInserts(vectors, collectionId) {
    // Insert vectors with streaming for better performance
    const chunkSize = 100;
    const results = [];
    
    for (let i = 0; i < vectors.length; i += chunkSize) {
      const chunk = vectors.slice(i, i + chunkSize);
      
      const records = chunk.map(vectorData => ({
        collection_id: collectionId,
        vector: vectorData.vector,
        metadata: createMetadataStruct(vectorData.metadata || {})
      }));
      
      const request = {
        collection_id: collectionId,
        records: records
      };
      
      const result = await this.batchInsert(request, this.getMetadata());
      results.push(result);
    }
    
    const totalInserted = results.reduce((sum, r) => sum + r.inserted_count, 0);
    console.log(`Total inserted: ${totalInserted} vectors`);
    return results;
  }
}

// Usage example
async function asyncExample() {
  const client = new AsyncProximaDBClient('localhost', 9090, 'pk_test_example123');
  
  // Parallel searches
  const queries = Array(10).fill(0).map(() => 
    Array(384).fill(0).map(() => Math.random())
  );
  
  const searchResults = await client.parallelSearch(queries, 'grpc_js_embeddings', 5);
  
  console.log(`Completed ${searchResults.length} parallel searches`);
  searchResults.forEach((result, i) => {
    console.log(`Search ${i}: ${result.total_count} results`);
  });
}
```

### Health and Monitoring

```javascript
async function healthCheckGrpc() {
  const health = grpcClient.promisify(grpcClient.client.Health);
  const response = await health({});
  
  console.log(`Health status: ${response.status}`);
  console.log(`Timestamp: ${response.timestamp}`);
  return response;
}

async function getStatusGrpc() {
  const status = grpcClient.promisify(grpcClient.client.Status);
  const response = await status({});
  
  console.log(`Node ID: ${response.node_id}`);
  console.log(`Version: ${response.version}`);
  console.log(`Role: ${response.role}`);
  
  if (response.storage) {
    console.log(`Total vectors: ${response.storage.total_vectors}`);
    console.log(`Total size: ${response.storage.total_size_bytes} bytes`);
  }
  
  return response;
}
```

## Error Handling and Best Practices

```javascript
import EventEmitter from 'events';

class ProximaDBClientError extends Error {
  constructor(code, message, details = {}) {
    super(`${code}: ${message}`);
    this.code = code;
    this.details = details;
  }
}

function retryWithBackoff(func, maxRetries = 3, baseDelay = 1000) {
  return async function (...args) {
    for (let attempt = 0; attempt < maxRetries; attempt++) {
      try {
        return await func.apply(this, args);
      } catch (error) {
        if (error.code === grpc.status.RESOURCE_EXHAUSTED && attempt < maxRetries - 1) {
          const delay = baseDelay * Math.pow(2, attempt);
          console.warn(`Rate limited, retrying in ${delay}ms...`);
          await new Promise(resolve => setTimeout(resolve, delay));
          continue;
        }
        throw new ProximaDBClientError(
          error.code || 'UNKNOWN_ERROR',
          error.message,
          error.details || {}
        );
      }
    }
  };
}

// Usage with retry
const robustSearch = retryWithBackoff(async function(queryVector, collectionId) {
  const search = grpcClient.promisify(grpcClient.client.Search);
  const request = {
    collection_id: collectionId,
    vector: queryVector,
    k: 10
  };
  return await search(request);
});

// Example usage
async function main() {
  try {
    // REST API examples
    console.log('=== REST API Examples ===');
    await createCollectionExample();
    await insertVectorExample();
    await searchVectorsExample();
    
    // gRPC API examples  
    console.log('\n=== gRPC API Examples ===');
    await createCollectionGrpc();
    await insertVectorGrpc();
    await searchVectorsGrpc();
    await healthCheckGrpc();
    
  } catch (error) {
    console.error(`Example failed: ${error.message}`);
  }
}

export {
  ProximaDBRestClient,
  ProximaDBGrpcClient,
  AsyncProximaDBClient,
  DocumentSearchApp,
  ProximaDBClientError
};

// Run if this is the main module
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}
```

This comprehensive JavaScript/TypeScript guide covers both REST and gRPC APIs with practical examples for building production applications with ProximaDB.