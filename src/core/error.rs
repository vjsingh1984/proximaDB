use thiserror::Error;

#[derive(Error, Debug)]
pub enum VectorDBError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    
    #[error("Consensus error: {0}")]
    Consensus(#[from] ConsensusError),
    
    #[error("Network error: {0}")]
    Network(#[from] NetworkError),
    
    #[error("Query error: {0}")]
    Query(#[from] QueryError),
    
    #[error("Schema error: {0}")]
    Schema(#[from] SchemaError),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Internal error: {0}")]
    Internal(String),
    
    #[error("Quantization error: {0}")]
    Quantization(String),
}

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("LSM tree error: {0}")]
    LsmTree(String),
    
    #[error("MMAP error: {0}")]
    Mmap(String),
    
    #[error("Disk I/O error: {0}")]
    DiskIO(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Corruption detected: {0}")]
    Corruption(String),
    
    #[error("Resource already exists: {0}")]
    AlreadyExists(String),
    
    #[error("Resource not found: {0}")]
    NotFound(String),
    
    #[error("Index error: {0}")]
    IndexError(String),
    
    #[error("WAL error: {0}")]
    WalError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(String),
    
    #[error("Metadata error: {0}")]
    MetadataError(#[from] anyhow::Error),
}

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Raft error: {0}")]
    Raft(String),
    
    #[error("Leadership error: {0}")]
    Leadership(String),
    
    #[error("Replication error: {0}")]
    Replication(String),
}

#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),
    
    #[error("HTTP error: {0}")]
    Http(String),
    
    #[error("Connection error: {0}")]
    Connection(String),
}

#[derive(Error, Debug)]
pub enum QueryError {
    #[error("Vector search error: {0}")]
    VectorSearch(String),
    
    #[error("SQL parse error: {0}")]
    SqlParse(String),
    
    #[error("Invalid query: {0}")]
    InvalidQuery(String),
    
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),
}

#[derive(Error, Debug)]
pub enum SchemaError {
    #[error("Invalid schema: {0}")]
    InvalidSchema(String),
    
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),
    
    #[error("Schema validation error: {0}")]
    Validation(String),
}