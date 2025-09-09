# ProximaDB Graph Database API Documentation

This document provides comprehensive API documentation for ProximaDB's native graph database capabilities, including REST endpoints, gRPC services, and usage examples.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [REST API](#rest-api)
- [gRPC API](#grpc-api)  
- [Query Language](#query-language)
- [Hybrid Queries](#hybrid-queries)
- [Performance Tuning](#performance-tuning)
- [Best Practices](#best-practices)
- [Examples](#examples)

## Overview

ProximaDB's graph database provides a high-performance, Arc-based zero-copy architecture for graph operations. It supports multiple operation modes:

- **Graph-Only Mode**: Pure graph database operations
- **Unified Mode**: Combined vector and graph operations with semantic capabilities
- **Hybrid Mode**: Advanced queries combining vector similarity with graph traversal

### Key Features

- **Native Graph Storage**: CSR (Compressed Sparse Row) format for optimal traversal performance
- **Cost-Based Query Planning**: Intelligent query optimization with statistics-based decisions
- **Pattern Matching**: Cypher-like query language for expressing graph patterns
- **Vector-Graph Integration**: Semantic graph traversal using embeddings
- **Real-time Monitoring**: Prometheus metrics and performance profiling
- **ACID Transactions**: Full transaction support with WAL durability

## Architecture

```text
┌─────────────────────────────────────────┐
│              ProximaDB                  │
│            Graph Database               │
├─────────────────────────────────────────┤
│                                         │
│  ┌──────────────┐  ┌─────────────────┐  │
│  │   REST API   │  │   gRPC API      │  │
│  │   Port 5678  │  │   Port 5679     │  │
│  └──────┬───────┘  └─────────┬───────┘  │
│         │                    │          │
│  ┌──────▼────────────────────▼───────┐  │
│  │         GraphService              │  │
│  │      (Business Logic)             │  │
│  └──────────────┬────────────────────┘  │
│                 │                       │
│  ┌──────────────▼────────────────────┐  │
│  │          ORION Engine             │  │
│  │       (CSR Storage)               │  │
│  └───────────────────────────────────┘  │
│                                         │
│  ┌───────────────────────────────────┐  │
│  │        Arc Memory Pool            │  │
│  │    ┌─────────┬─────────────────┐  │  │
│  │    │  Nodes  │  Edges & Props  │  │  │
│  │    └─────────┴─────────────────┘  │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

## REST API

The REST API provides intuitive HTTP endpoints for all graph operations.

### Base URL

```
http://localhost:5678/v1/graph
```

### Authentication

Currently, ProximaDB uses simple API key authentication:

```bash
# Include API key in headers
curl -H "Authorization: Bearer YOUR_API_KEY" \
     -H "Content-Type: application/json" \
     http://localhost:5678/v1/graph/nodes
```

### Node Operations

#### Create Node

Create a new node in the graph.

```http
POST /v1/graph/nodes
Content-Type: application/json

{
  "id": "person_123",
  "labels": ["Person", "Employee"],
  "properties": {
    "name": "Alice Johnson",
    "age": 30,
    "department": "Engineering",
    "email": "alice@company.com"
  },
  "embedding": [0.1, 0.2, 0.3, ...] // Optional vector embedding
}
```

**Response:**
```json
{
  "success": true,
  "data": {
    "id": "person_123",
    "labels": ["Person", "Employee"],
    "properties": {
      "name": "Alice Johnson",
      "age": 30,
      "department": "Engineering",
      "email": "alice@company.com"
    },
    "created_at": "2025-09-09T10:30:00Z",
    "updated_at": "2025-09-09T10:30:00Z"
  }
}
```

#### Get Node

Retrieve a node by its ID.

```http
GET /v1/graph/nodes/{node_id}
```

**Example:**
```bash
curl http://localhost:5678/v1/graph/nodes/person_123
```

**Response:**
```json
{
  "success": true,
  "data": {
    "id": "person_123",
    "labels": ["Person", "Employee"],
    "properties": {
      "name": "Alice Johnson",
      "age": 30,
      "department": "Engineering",
      "email": "alice@company.com"
    },
    "created_at": "2025-09-09T10:30:00Z",
    "updated_at": "2025-09-09T10:30:00Z"
  }
}
```

#### Update Node

Update an existing node.

```http
PUT /v1/graph/nodes/{node_id}
Content-Type: application/json

{
  "properties": {
    "age": 31,
    "department": "Senior Engineering"
  }
}
```

#### Delete Node

Delete a node and all its relationships.

```http
DELETE /v1/graph/nodes/{node_id}
```

#### Query Nodes

Query nodes by labels and properties.

```http
POST /v1/graph/nodes/query
Content-Type: application/json

{
  "labels": ["Person"],
  "properties": {
    "department": "Engineering"
  },
  "limit": 10,
  "offset": 0
}
```

### Edge Operations

#### Create Edge

Create a relationship between two nodes.

```http
POST /v1/graph/edges
Content-Type: application/json

{
  "id": "knows_456",
  "from_node_id": "person_123",
  "to_node_id": "person_789", 
  "edge_type": "KNOWS",
  "properties": {
    "since": "2020-01-15",
    "strength": 0.8,
    "context": "work"
  }
}
```

#### Get Edge

Retrieve an edge by its ID.

```http
GET /v1/graph/edges/{edge_id}
```

#### Update Edge

Update edge properties.

```http
PUT /v1/graph/edges/{edge_id}
Content-Type: application/json

{
  "properties": {
    "strength": 0.9,
    "last_contact": "2025-09-08"
  }
}
```

#### Delete Edge

Delete a relationship.

```http
DELETE /v1/graph/edges/{edge_id}
```

### Graph Traversal

#### BFS Traversal

Breadth-First Search traversal from a starting node.

```http
POST /v1/graph/traverse/bfs
Content-Type: application/json

{
  "start_node_id": "person_123",
  "max_depth": 3,
  "edge_types": ["KNOWS", "WORKS_WITH"],
  "node_filters": [
    {
      "property": "department",
      "operator": "equals",
      "value": "Engineering"
    }
  ],
  "limit": 100
}
```

**Response:**
```json
{
  "success": true,
  "data": {
    "nodes": [
      {
        "id": "person_123",
        "distance": 0,
        "path": []
      },
      {
        "id": "person_456", 
        "distance": 1,
        "path": ["knows_789"]
      }
    ],
    "stats": {
      "execution_time_ms": 45,
      "nodes_visited": 87,
      "edges_traversed": 156
    }
  }
}
```

#### DFS Traversal

Depth-First Search traversal.

```http
POST /v1/graph/traverse/dfs
Content-Type: application/json

{
  "start_node_id": "person_123",
  "max_depth": 3,
  "edge_types": ["MANAGES"],
  "limit": 50
}
```

#### Shortest Path

Find shortest path between two nodes.

```http
POST /v1/graph/path/shortest
Content-Type: application/json

{
  "start_node_id": "person_123",
  "end_node_id": "person_789",
  "edge_types": ["KNOWS", "WORKS_WITH"],
  "algorithm": "dijkstra"
}
```

### Pattern Matching

Execute Cypher-like pattern queries.

```http
POST /v1/graph/pattern/match
Content-Type: application/json

{
  "pattern": "MATCH (alice:Person {name: 'Alice'})-[:KNOWS]->(friend:Person) RETURN friend",
  "parameters": {},
  "limit": 50
}
```

**Advanced Pattern:**
```http
POST /v1/graph/pattern/match
Content-Type: application/json

{
  "pattern": "MATCH (p:Person)-[:WORKS_AT]->(c:Company {industry: 'Tech'}) WHERE p.age > 25 RETURN p.name, c.name",
  "parameters": {
    "min_age": 25
  },
  "limit": 100
}
```

### Hybrid Vector-Graph Queries

Combine vector similarity with graph traversal.

```http
POST /v1/graph/hybrid/query
Content-Type: application/json

{
  "vector_component": {
    "query_vector": [0.1, 0.2, 0.3, ...],
    "threshold": 0.7,
    "max_results": 100
  },
  "graph_component": {
    "start_nodes": ["person_123"],
    "max_depth": 3,
    "edge_types": ["SIMILAR_TO", "KNOWS"],
    "algorithm": "semantic_bfs"
  },
  "fusion": {
    "strategy": "balanced",
    "ranking": "harmonic_mean"
  },
  "result_spec": {
    "limit": 20,
    "include_scores": true,
    "include_paths": true
  }
}
```

### Batch Operations

Process multiple operations in a single request.

```http
POST /v1/graph/batch
Content-Type: application/json

{
  "operations": [
    {
      "type": "create_node",
      "data": {
        "id": "person_new",
        "labels": ["Person"],
        "properties": {"name": "New Person"}
      }
    },
    {
      "type": "create_edge", 
      "data": {
        "from_node_id": "person_123",
        "to_node_id": "person_new",
        "edge_type": "KNOWS"
      }
    }
  ]
}
```

### Statistics and Analytics

#### Graph Statistics

Get overall graph statistics.

```http
GET /v1/graph/stats
```

**Response:**
```json
{
  "success": true,
  "data": {
    "total_nodes": 1000000,
    "total_edges": 5000000,
    "avg_node_degree": 5.0,
    "connected_components": 12,
    "graph_diameter": 8,
    "label_stats": {
      "Person": 800000,
      "Company": 50000,
      "Product": 150000
    },
    "edge_type_stats": {
      "KNOWS": 2000000,
      "WORKS_AT": 800000,
      "OWNS": 300000,
      "SIMILAR_TO": 1900000
    }
  }
}
```

#### Performance Metrics

Get performance and monitoring metrics.

```http
GET /v1/graph/metrics
```

**Response:**
```json
{
  "success": true,
  "data": {
    "timestamp": "2025-09-09T10:30:00Z",
    "operation_counts": {
      "node_create": 1000,
      "node_read": 50000,
      "edge_create": 2000,
      "traversal_bfs": 500
    },
    "performance": {
      "avg_query_time_ms": 23.5,
      "p95_query_time_ms": 89.2,
      "p99_query_time_ms": 156.7,
      "queries_per_second": 1247
    },
    "resource_usage": {
      "memory_used_mb": 2048,
      "cpu_usage_percent": 45.2,
      "cache_hit_ratio": 0.87
    }
  }
}
```

## gRPC API

The gRPC API provides high-performance binary protocol access to all graph operations.

### Service Definition

```protobuf
service GraphService {
  // Node operations
  rpc CreateNode(CreateNodeRequest) returns (CreateNodeResponse);
  rpc GetNode(GetNodeRequest) returns (GetNodeResponse);
  rpc UpdateNode(UpdateNodeRequest) returns (UpdateNodeResponse);
  rpc DeleteNode(DeleteNodeRequest) returns (DeleteNodeResponse);
  rpc QueryNodes(QueryNodesRequest) returns (QueryNodesResponse);
  
  // Edge operations  
  rpc CreateEdge(CreateEdgeRequest) returns (CreateEdgeResponse);
  rpc GetEdge(GetEdgeRequest) returns (GetEdgeResponse);
  rpc UpdateEdge(UpdateEdgeRequest) returns (UpdateEdgeResponse);
  rpc DeleteEdge(DeleteEdgeRequest) returns (DeleteEdgeResponse);
  
  // Traversal operations
  rpc TraverseBFS(TraversalRequest) returns (TraversalResponse);
  rpc TraverseDFS(TraversalRequest) returns (TraversalResponse);
  rpc FindShortestPath(ShortestPathRequest) returns (PathResponse);
  
  // Pattern matching
  rpc MatchPattern(PatternMatchRequest) returns (PatternMatchResponse);
  
  // Hybrid queries
  rpc ExecuteHybridQuery(HybridQueryRequest) returns (HybridQueryResponse);
  
  // Batch operations
  rpc BatchOperations(BatchRequest) returns (BatchResponse);
  
  // Statistics and monitoring
  rpc GetGraphStats(GraphStatsRequest) returns (GraphStatsResponse);
  rpc GetMetrics(MetricsRequest) returns (MetricsResponse);
}
```

### Connection Example

```python
import grpc
from proximadb import graph_pb2_grpc, graph_pb2

# Create gRPC channel
channel = grpc.insecure_channel('localhost:5679')
stub = graph_pb2_grpc.GraphServiceStub(channel)

# Create a node
request = graph_pb2.CreateNodeRequest(
    node=graph_pb2.Node(
        id="person_123",
        labels=["Person", "Employee"],
        properties={
            "name": graph_pb2.PropertyValue(string_value="Alice Johnson"),
            "age": graph_pb2.PropertyValue(int_value=30)
        }
    )
)

response = stub.CreateNode(request)
print(f"Created node: {response.node.id}")
```

### Streaming Operations

For large result sets, use streaming operations:

```python
# Stream traversal results
request = graph_pb2.TraversalRequest(
    start_node_id="person_123",
    algorithm=graph_pb2.TraversalAlgorithm.BFS,
    max_depth=5
)

for response in stub.StreamTraversal(request):
    print(f"Found node: {response.node.id} at distance {response.distance}")
```

## Query Language

ProximaDB supports a Cypher-like query language for expressing graph patterns.

### Basic Patterns

#### Node Patterns

```cypher
// Find all persons
MATCH (n:Person) RETURN n

// Find persons with specific property
MATCH (n:Person {name: "Alice"}) RETURN n

// Find persons with age filter
MATCH (n:Person) WHERE n.age > 25 RETURN n
```

#### Relationship Patterns

```cypher
// Find direct relationships
MATCH (alice:Person {name: "Alice"})-[:KNOWS]->(friend) RETURN friend

// Find bidirectional relationships  
MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN a, b

// Find specific relationship types
MATCH (emp:Person)-[:WORKS_AT]->(company:Company) RETURN emp.name, company.name
```

#### Variable Length Paths

```cypher
// Find friends of friends (2 hops)
MATCH (alice:Person {name: "Alice"})-[:KNOWS*2]-(friend_of_friend) 
RETURN friend_of_friend

// Find paths of length 1 to 3
MATCH (a:Person)-[:KNOWS*1..3]-(b:Person) 
WHERE a.name = "Alice" 
RETURN b

// Find shortest paths
MATCH path = shortestPath((a:Person)-[:KNOWS*]-(b:Person))
WHERE a.name = "Alice" AND b.name = "Bob"
RETURN path
```

### Advanced Queries

#### Aggregation

```cypher
// Count relationships per person
MATCH (p:Person)-[:KNOWS]-(friend)
RETURN p.name, count(friend) as friend_count
ORDER BY friend_count DESC

// Average age by department
MATCH (p:Person)
RETURN p.department, avg(p.age) as avg_age
```

#### Complex Filters

```cypher
// Multiple conditions with AND/OR
MATCH (p:Person)
WHERE (p.age > 25 AND p.department = "Engineering") 
   OR (p.age > 30 AND p.department = "Sales")
RETURN p

// Pattern-based filters
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WHERE c.industry IN ["Tech", "Finance"]
  AND p.salary > 100000
RETURN p.name, c.name, p.salary
```

#### Subqueries

```cypher
// Find persons with more than 5 connections
MATCH (p:Person)
WHERE size((p)-[:KNOWS]-()) > 5
RETURN p

// Find highly connected people in same company
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WITH p, c, size((p)-[:KNOWS]-()) as connections
WHERE connections > avg(connections)
RETURN p.name, c.name, connections
```

## Hybrid Queries

Hybrid queries combine vector similarity search with graph traversal for powerful semantic operations.

### Semantic Similarity Within Graph

Find semantically similar nodes within N hops of a starting node:

```json
{
  "vector_component": {
    "query_vector": [0.1, 0.2, 0.3, ...],
    "threshold": 0.8,
    "max_results": 50
  },
  "graph_component": {
    "start_nodes": ["person_123"],
    "max_depth": 3,
    "edge_types": ["KNOWS", "WORKS_WITH"]
  },
  "fusion": {
    "strategy": "vector_first",
    "weights": {
      "vector_weight": 0.7,
      "graph_weight": 0.3
    }
  }
}
```

### Semantic Path Finding

Use embeddings to guide path finding:

```json
{
  "operation": "semantic_path",
  "start_node": "person_123", 
  "end_node": "person_456",
  "guidance_vector": [0.5, 0.3, 0.8, ...],
  "max_depth": 5,
  "edge_types": ["SIMILAR_TO", "RELATED_TO"]
}
```

### Context-Aware Recommendations

Find recommendations based on both graph structure and semantic similarity:

```json
{
  "operation": "recommend",
  "target_node": "person_123",
  "recommendation_type": "person",
  "context_vector": [0.2, 0.7, 0.1, ...],
  "graph_constraints": {
    "exclude_direct_connections": true,
    "min_hops": 2,
    "max_hops": 4
  },
  "similarity_threshold": 0.75
}
```

## Performance Tuning

### Indexing Strategy

Create indexes for frequently queried properties:

```http
POST /v1/graph/indexes
Content-Type: application/json

{
  "name": "person_name_idx",
  "type": "btree",
  "entity": "node",
  "properties": ["name"],
  "labels": ["Person"]
}
```

### Query Optimization

1. **Use specific labels** in patterns:
   ```cypher
   // Good: Specific label
   MATCH (p:Person {name: "Alice"})
   
   // Avoid: No label (scans all nodes)
   MATCH (p {name: "Alice"})
   ```

2. **Filter early** in traversals:
   ```cypher
   // Good: Filter immediately
   MATCH (p:Person {department: "Engineering"})-[:KNOWS]->(friend)
   
   // Avoid: Filter after traversal
   MATCH (p:Person)-[:KNOWS]->(friend)
   WHERE p.department = "Engineering"
   ```

3. **Use appropriate traversal depth**:
   ```json
   {
     "max_depth": 3,  // Usually sufficient for most use cases
     "early_termination": true,
     "limit": 100
   }
   ```

### Memory Management

Configure memory pools for optimal performance:

```json
{
  "memory_config": {
    "node_pool_size_mb": 1024,
    "edge_pool_size_mb": 2048,
    "query_cache_size_mb": 512,
    "plan_cache_size": 10000
  }
}
```

### Monitoring Performance

Monitor query performance with detailed metrics:

```bash
# Get slow queries
curl http://localhost:5678/v1/graph/slow-queries?limit=10

# Get query execution plans
curl -X POST http://localhost:5678/v1/graph/explain \
  -d '{"pattern": "MATCH (p:Person)-[:KNOWS]->(f) RETURN f"}'

# Get real-time metrics
curl http://localhost:5678/v1/graph/metrics/realtime
```

## Best Practices

### Schema Design

1. **Use meaningful labels**:
   ```cypher
   // Good
   CREATE (p:Person:Employee {name: "Alice"})
   
   // Avoid generic labels
   CREATE (n:Node {type: "person", name: "Alice"})
   ```

2. **Normalize relationship types**:
   ```cypher
   // Good: Consistent naming
   -[:WORKS_AT]->
   -[:REPORTS_TO]->
   -[:MANAGES]->
   
   // Avoid: Inconsistent naming  
   -[:works_at]->
   -[:reportsTo]->
   -[:MANAGE]->
   ```

3. **Property design**:
   ```json
   {
     "properties": {
       "name": "Alice Johnson",        // String
       "age": 30,                     // Integer
       "active": true,                // Boolean
       "skills": ["Java", "Python"],  // Array
       "metadata": {                  // Object
         "created_by": "admin",
         "source": "hr_system"
       }
     }
   }
   ```

### Query Patterns

1. **Use parameterized queries**:
   ```cypher
   MATCH (p:Person {name: $person_name})-[:KNOWS]->(f)
   RETURN f
   ```

2. **Batch operations** when possible:
   ```json
   {
     "operations": [
       {"type": "create_node", "data": {...}},
       {"type": "create_node", "data": {...}},
       {"type": "create_edge", "data": {...}}
     ]
   }
   ```

3. **Use appropriate limits**:
   ```cypher
   MATCH (p:Person)-[:KNOWS]->(f)
   RETURN f
   LIMIT 100  // Always limit large result sets
   ```

### Error Handling

Handle errors gracefully in your applications:

```python
import grpc
from proximadb import graph_pb2_grpc, graph_pb2

try:
    response = stub.CreateNode(request)
    print(f"Created node: {response.node.id}")
except grpc.RpcError as e:
    if e.code() == grpc.StatusCode.ALREADY_EXISTS:
        print("Node already exists")
    elif e.code() == grpc.StatusCode.INVALID_ARGUMENT:
        print(f"Invalid request: {e.details()}")
    else:
        print(f"Unexpected error: {e}")
```

## Examples

### Social Network Analysis

```python
# Find mutual friends
def find_mutual_friends(person1, person2):
    query = """
    MATCH (p1:Person {id: $person1})-[:KNOWS]->(mutual)<-[:KNOWS]-(p2:Person {id: $person2})
    RETURN mutual.name as mutual_friend
    """
    return execute_cypher(query, {"person1": person1, "person2": person2})

# Find influencers (people with many connections)
def find_influencers(min_connections=10):
    query = """
    MATCH (p:Person)
    WITH p, size((p)-[:KNOWS]-()) as connections
    WHERE connections >= $min_connections
    RETURN p.name, connections
    ORDER BY connections DESC
    """
    return execute_cypher(query, {"min_connections": min_connections})
```

### Recommendation System

```python
# Content-based recommendations using embeddings
async def recommend_similar_content(user_id, content_vector, limit=10):
    request = {
        "vector_component": {
            "query_vector": content_vector,
            "threshold": 0.8,
            "max_results": 50
        },
        "graph_component": {
            "start_nodes": [user_id],
            "max_depth": 3,
            "edge_types": ["LIKED", "VIEWED", "SHARED"],
            "algorithm": "semantic_bfs"
        },
        "fusion": {
            "strategy": "balanced"
        },
        "result_spec": {
            "limit": limit,
            "include_scores": true
        }
    }
    
    return await execute_hybrid_query(request)

# Collaborative filtering
def collaborative_recommendations(user_id, limit=10):
    query = """
    MATCH (u:User {id: $user_id})-[:LIKED]->(item)<-[:LIKED]-(similar_user)
    MATCH (similar_user)-[:LIKED]->(recommendation)
    WHERE NOT (u)-[:LIKED]->(recommendation)
    WITH recommendation, count(similar_user) as score
    RETURN recommendation, score
    ORDER BY score DESC
    LIMIT $limit
    """
    return execute_cypher(query, {"user_id": user_id, "limit": limit})
```

### Knowledge Graph Queries

```python
# Entity relationship exploration
def explore_entity_relationships(entity_id, max_depth=3):
    query = f"""
    MATCH path = (e:Entity {{id: $entity_id}})-[*1..{max_depth}]-(related)
    RETURN path, length(path) as depth
    ORDER BY depth, related.importance DESC
    """
    return execute_cypher(query, {"entity_id": entity_id})

# Semantic search in knowledge graph
async def semantic_knowledge_search(query_text, embedding, limit=20):
    request = {
        "vector_component": {
            "query_vector": embedding,
            "threshold": 0.7,
            "max_results": 100
        },
        "graph_component": {
            "start_nodes": [],  # Start from all nodes
            "max_depth": 2,
            "edge_types": ["RELATED_TO", "PART_OF", "INSTANCE_OF"]
        },
        "fusion": {
            "strategy": "vector_first",
            "weights": {
                "vector_weight": 0.8,
                "graph_weight": 0.2
            }
        },
        "result_spec": {
            "limit": limit,
            "include_paths": true
        }
    }
    
    return await execute_hybrid_query(request)
```

### Real-time Analytics

```python
# Real-time graph analytics
class GraphAnalytics:
    def __init__(self, graph_client):
        self.client = graph_client
    
    async def compute_centrality(self, node_type="Person"):
        """Compute betweenness centrality for nodes"""
        query = f"""
        MATCH (n:{node_type})
        WITH n, size((n)--()) as degree
        RETURN n.id, degree
        ORDER BY degree DESC
        LIMIT 100
        """
        return await self.client.execute_cypher(query)
    
    async def detect_communities(self, algorithm="louvain"):
        """Detect communities in the graph"""
        request = {
            "algorithm": algorithm,
            "node_types": ["Person"],
            "edge_types": ["KNOWS", "WORKS_WITH"],
            "parameters": {
                "resolution": 1.0,
                "iterations": 10
            }
        }
        return await self.client.detect_communities(request)
    
    async def monitor_graph_changes(self):
        """Monitor real-time changes in graph structure"""
        async for event in self.client.stream_graph_events():
            if event.type == "node_created":
                await self.handle_new_node(event.node)
            elif event.type == "edge_created":
                await self.handle_new_edge(event.edge)
```

### Production Deployment

```yaml
# docker-compose.yml
version: '3.8'
services:
  proximadb:
    image: proximadb:latest
    ports:
      - "5678:5678"  # REST API
      - "5679:5679"  # gRPC API
      - "9090:9090"  # Prometheus metrics
    environment:
      - PROXIMADB_MODE=unified
      - PROXIMADB_LOG_LEVEL=info
      - PROXIMADB_MEMORY_LIMIT=8GB
    volumes:
      - ./data:/data
      - ./config:/config
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:5678/health"]
      interval: 30s
      timeout: 10s
      retries: 3

  prometheus:
    image: prom/prometheus
    ports:
      - "9091:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'

  grafana:
    image: grafana/grafana
    ports:
      - "3000:3000"
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
    volumes:
      - grafana-data:/var/lib/grafana
```

This comprehensive API documentation provides everything needed to start building applications with ProximaDB's graph database capabilities. For more advanced usage and latest updates, refer to the official ProximaDB documentation and community resources.