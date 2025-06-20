# ProximaDB Metadata Backend Architecture Requirements

## Overview

This document outlines the pluggable metadata backend architecture for ProximaDB metadata storage, designed to support multiple storage backends using strategy pattern with abstract factory.

## Design Patterns

### Strategy Pattern
- **MetadataBackend trait**: Core interface for all metadata operations
- **Multiple implementations**: WAL, PostgreSQL, DynamoDB, MongoDB, SQLite, Memory
- **Runtime switching**: Configurable backend selection without code changes

### Abstract Factory Pattern  
- **MetadataBackendFactory**: Creates appropriate backend instances
- **Configuration-driven**: Backend type determined by connection string/config
- **Pluggable architecture**: Easy to add new backends without modifying existing code

## Backend Implementations

### 1. WAL Backend (Default)
**File**: `src/storage/metadata/backends/wal_backend.rs`
- **Purpose**: Default backend using Avro WAL with B+Tree memtable
- **Benefits**: ACID guarantees, schema evolution, embedded deployment
- **Use cases**: Single-node, development, embedded scenarios
- **Technology**: Custom WAL with Avro serialization

### 2. PostgreSQL Backend (SQL/RDBMS)
**File**: `src/storage/metadata/backends/postgres_backend.rs`
- **Purpose**: Enterprise SQL backend with full ACID guarantees
- **Benefits**: Complex queries, joins, transactions, mature ecosystem
- **Use cases**: Enterprise deployments, complex metadata queries, reporting
- **Technology**: ODBC/JDBC connections, connection pooling, read replicas

#### Schema Design
```sql
CREATE TABLE collections (
    id VARCHAR(255) PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE,
    dimension INTEGER NOT NULL,
    distance_metric VARCHAR(50) NOT NULL,
    indexing_algorithm VARCHAR(50) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL,
    vector_count BIGINT DEFAULT 0,
    total_size_bytes BIGINT DEFAULT 0,
    config JSONB DEFAULT '{}',
    access_pattern VARCHAR(20) DEFAULT 'Normal',
    description TEXT,
    tags TEXT[],
    owner VARCHAR(255),
    version BIGINT DEFAULT 1
);

-- Indexes for performance
CREATE INDEX idx_collections_name ON collections(name);
CREATE INDEX idx_collections_access_pattern ON collections(access_pattern);
CREATE INDEX idx_collections_tags ON collections USING GIN(tags);
```

#### Features
- **Connection Pooling**: Min/max connections, timeout configuration
- **Read Replicas**: Load balancing across multiple read-only replicas
- **Transactions**: Full ACID support with isolation levels
- **SSL/TLS**: Secure connections with certificate validation
- **Backup/Restore**: pg_dump/pg_restore integration

### 3. DynamoDB Backend (NoSQL/Serverless)
**File**: `src/storage/metadata/backends/dynamodb_backend.rs`
- **Purpose**: AWS serverless NoSQL backend
- **Benefits**: Auto-scaling, pay-per-use, global tables, managed service
- **Use cases**: Cloud-native deployments, serverless, multi-region
- **Technology**: AWS SDK for DynamoDB

#### Schema Design
```
Table: proximadb_metadata
Partition Key: PK (String) = "COLLECTION#<collection_id>"
Sort Key: SK (String) = "METADATA"

Global Secondary Indexes:
1. GSI_NAME: PK=name, SK=created_at
2. GSI_ACCESS_PATTERN: PK=access_pattern, SK=updated_at  
3. GSI_OWNER: PK=owner, SK=created_at
4. GSI_TAGS: PK=tag, SK=created_at (sparse index)
```

#### Features
- **Auto-scaling**: Automatic capacity management
- **Global Tables**: Multi-region replication
- **Point-in-time Recovery**: Backup and restore capabilities
- **IAM Integration**: AWS IAM roles and policies
- **Encryption**: Encryption at rest and in transit

### 4. MongoDB Backend (Document NoSQL)
**File**: `src/storage/metadata/backends/mongodb_backend.rs`
- **Purpose**: Flexible document-based storage
- **Benefits**: Rich queries, horizontal scaling, flexible schema
- **Use cases**: Complex metadata structures, aggregation queries, Atlas cloud
- **Technology**: MongoDB driver, MongoDB Atlas support

#### Schema Design
```javascript
// Collection: collections
{
  _id: "collection_id",
  collection_id: "string",
  name: "string",
  dimension: "number",
  // ... other fields
  search_keywords: ["array of strings"],
  tags_normalized: ["lowercase tags"],
  expires_at: "ISODate" // TTL field
}

// Indexes
db.collections.createIndex({"collection_id": 1}, {unique: true})
db.collections.createIndex({"name": 1}, {unique: true})
db.collections.createIndex({"search_keywords": "text"})
db.collections.createIndex({"expires_at": 1}, {expireAfterSeconds: 0})
```

#### Features
- **Replica Sets**: High availability and failover
- **Sharding**: Horizontal scaling across multiple nodes
- **Text Search**: Full-text search on metadata fields
- **Aggregation Pipeline**: Complex query and analytics capabilities
- **Atlas Integration**: MongoDB cloud service support

### 5. SQLite Backend (Embedded SQL)
**File**: `src/storage/metadata/backends/sqlite_backend.rs`
- **Purpose**: Embedded SQL database for single-node deployments
- **Benefits**: Zero configuration, ACID guarantees, embedded deployment
- **Use cases**: Edge computing, development, small deployments
- **Technology**: SQLite with WAL mode

#### Schema Design
```sql
CREATE TABLE collections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    dimension INTEGER NOT NULL,
    -- ... other fields
    expires_at TEXT -- ISO timestamp for TTL
);

-- Virtual table for full-text search
CREATE VIRTUAL TABLE collections_fts USING fts5(
    id UNINDEXED,
    name, description, tags, owner,
    content='collections'
);
```

#### Features
- **WAL Mode**: Better concurrency support
- **FTS5**: Full-text search capabilities
- **Backup API**: SQLite backup API for hot backups
- **Optimization**: VACUUM, ANALYZE, and auto-optimization
- **Encryption**: Optional encryption at rest

### 6. Memory Backend (Testing/Development)
**File**: `src/storage/metadata/backends/memory_backend.rs`
- **Purpose**: Fast in-memory storage for testing and development
- **Benefits**: Fastest performance, simple implementation
- **Use cases**: Unit tests, development, temporary deployments
- **Technology**: In-memory HashMap with custom indexes

#### Features
- **Custom Indexes**: Name, owner, tags, access pattern indexes
- **Full-text Search**: Simple keyword matching
- **Statistics**: Operation counts and timing
- **No Persistence**: Data lost on restart (by design)

## Configuration System

### Connection String Format
```
# WAL Backend (default)
file://./data/metadata

# PostgreSQL
postgresql://user:password@host:5432/database

# DynamoDB  
dynamodb://region/table_name?access_key=KEY&secret_key=SECRET

# MongoDB
mongodb://user:password@host:27017/database

# SQLite
sqlite:///path/to/database.db

# Memory
memory://
```

### Backend Configuration
```toml
[metadata_storage]
backend_type = "postgresql"
connection_string = "postgresql://user:pass@host:5432/db"

[metadata_storage.connection]
pool_size = 10
timeout_seconds = 30
ssl_enabled = true

[metadata_storage.performance]
enable_read_replicas = true
read_replica_endpoints = ["replica1:5432", "replica2:5432"]
enable_caching = true
cache_ttl_seconds = 300
```

## Operations Support

### Core Operations
- **CRUD**: Create, Read, Update, Delete collections
- **Transactions**: Atomic batch operations where supported
- **Statistics**: Vector count and size tracking
- **Filtering**: Complex metadata queries and filtering
- **Health Checks**: Backend connectivity and health monitoring

### Advanced Operations
- **Backup/Restore**: Backend-specific backup mechanisms
- **Migration**: Cross-backend data migration tools
- **Monitoring**: Performance metrics and alerting
- **Scaling**: Auto-scaling and load balancing where applicable

## Implementation Architecture

### Factory Pattern
```rust
pub struct MetadataBackendFactory;

impl MetadataBackendFactory {
    pub async fn create_backend(
        backend_type: MetadataBackendType,
        config: MetadataBackendConfig,
    ) -> Result<Box<dyn MetadataBackend>>;
}
```

### Backend Trait
```rust
#[async_trait]
pub trait MetadataBackend: Send + Sync {
    // Core operations
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()>;
    async fn get_collection(&self, id: &CollectionId) -> Result<Option<CollectionMetadata>>;
    async fn update_collection(&self, id: &CollectionId, metadata: CollectionMetadata) -> Result<()>;
    async fn delete_collection(&self, id: &CollectionId) -> Result<bool>;
    
    // Advanced operations
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>>;
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>;
    async fn backup(&self, location: &str) -> Result<String>;
    async fn restore(&self, backup_id: &str, location: &str) -> Result<()>;
    
    // Monitoring
    async fn health_check(&self) -> Result<bool>;
    async fn get_stats(&self) -> Result<BackendStats>;
}
```

## Backend Selection Criteria

### WAL Backend
- **Best for**: Single-node, embedded, development
- **Limitations**: No horizontal scaling, single-connection
- **Performance**: Fastest for single-node scenarios

### PostgreSQL Backend  
- **Best for**: Enterprise, complex queries, ACID requirements
- **Limitations**: Operational complexity, vertical scaling limits
- **Performance**: Excellent for complex queries and reporting

### DynamoDB Backend
- **Best for**: AWS cloud-native, serverless, global scale
- **Limitations**: AWS vendor lock-in, query limitations
- **Performance**: Excellent horizontal scaling, pay-per-use

### MongoDB Backend
- **Best for**: Flexible schema, aggregation, horizontal scaling
- **Limitations**: Eventual consistency by default, operational complexity
- **Performance**: Good for complex document queries

### SQLite Backend
- **Best for**: Edge computing, small deployments, embedded
- **Limitations**: Single writer, no network access
- **Performance**: Fast for small to medium datasets

### Memory Backend
- **Best for**: Testing, development, temporary storage
- **Limitations**: No persistence, memory limitations
- **Performance**: Fastest possible, limited by RAM

## Security Considerations

### Authentication
- **SQL backends**: Username/password, integrated authentication
- **Cloud backends**: IAM roles, API keys, service accounts
- **Embedded backends**: File system permissions

### Encryption
- **In Transit**: TLS/SSL for network backends
- **At Rest**: Backend-specific encryption options
- **Key Management**: Integration with key management systems

### Access Control
- **Database-level**: User permissions and roles
- **Application-level**: Metadata filtering and tenant isolation
- **Network-level**: VPC, security groups, firewall rules

## Monitoring and Observability

### Metrics
- **Performance**: Operation latency, throughput, error rates
- **Resource**: Connection pool usage, memory consumption
- **Business**: Collection counts, data sizes, growth rates

### Logging
- **Audit Logs**: All metadata operations
- **Error Logs**: Failed operations and diagnostics
- **Performance Logs**: Slow queries and bottlenecks

### Alerting
- **Health Checks**: Backend availability monitoring
- **Performance**: SLA breach notifications  
- **Capacity**: Storage and connection limits

## Future Extensibility

### Additional Backends
- **Azure Cosmos DB**: Multi-model database service
- **Google Firestore**: Serverless document database
- **Apache Cassandra**: Wide-column distributed database
- **Redis**: In-memory data structure store
- **ClickHouse**: Column-oriented analytics database

### Advanced Features
- **Multi-backend**: Primary/fallback backend configurations
- **Data Tiering**: Hot/warm/cold storage policies
- **Cross-region**: Global metadata replication
- **Analytics**: Built-in metadata analytics and insights

## Development Guidelines

### Backend Implementation
1. **Implement MetadataBackend trait**: All required methods
2. **Handle errors gracefully**: Convert backend errors to common format
3. **Add comprehensive logging**: Use tracing for observability
4. **Write unit tests**: Mock backends for testing
5. **Document configuration**: Clear setup instructions

### Testing Strategy
1. **Unit Tests**: Individual backend implementations
2. **Integration Tests**: Real backend connections
3. **Performance Tests**: Load testing with realistic data
4. **Compatibility Tests**: Cross-backend migration testing

### Documentation Requirements
1. **Setup Guides**: Installation and configuration
2. **API Documentation**: Method signatures and examples
3. **Performance Characteristics**: Benchmarks and recommendations
4. **Troubleshooting**: Common issues and solutions

## Deployment Scenarios

### Single-Node Development
- **Backend**: WAL or SQLite
- **Benefits**: Simple setup, no external dependencies
- **Limitations**: No high availability

### Cloud-Native Production
- **Backend**: DynamoDB or MongoDB Atlas
- **Benefits**: Managed service, auto-scaling, high availability
- **Considerations**: Vendor lock-in, costs

### Enterprise On-Premises
- **Backend**: PostgreSQL with read replicas
- **Benefits**: Full control, mature tooling, compliance
- **Considerations**: Operational overhead, scaling limits

### Edge Computing
- **Backend**: SQLite or WAL
- **Benefits**: Minimal resources, offline capability
- **Limitations**: Limited query capabilities

This architecture provides a comprehensive, pluggable metadata storage system that can adapt to various deployment scenarios while maintaining consistent APIs and operational patterns.