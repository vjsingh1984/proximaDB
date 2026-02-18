/**
 * ProximaDB Embedded - In-process vector database for Node.js
 */

export interface DiskConfig {
  /** Path to the storage directory */
  path: string;
  /** Weight for data distribution (higher = more data) */
  weight?: number;
  /** Tags for storage tier identification */
  tags?: string[];
}

export interface ProximaDBConfig {
  /** Data directories (array of DiskConfig) */
  dataDirs?: DiskConfig[];
  /** Single data directory path */
  dataDir?: string;
  /** Metadata directory path */
  metadataDir?: string;
  /** Cache size in megabytes */
  cacheSizeMb?: number;
  /** Default storage engine */
  defaultEngine?: string;
  /** Enable write-ahead logging */
  enableWal?: boolean;
  /** WAL sync mode: "immediate", "batch", "async" */
  walSyncMode?: string;
}

export interface SearchResult {
  /** Vector ID */
  id: string;
  /** Similarity score */
  score: number;
  /** Metadata */
  metadata?: Record<string, string>;
}

export interface CollectionInfo {
  /** Collection name */
  name: string;
  /** Vector dimension */
  dimension: number;
  /** Number of vectors */
  vectorCount: number;
  /** Storage engine type */
  engine: string;
}

export interface StorageStats {
  /** Total vectors */
  totalVectors: number;
  /** Total collections */
  totalCollections: number;
  /** Disk usage in bytes */
  diskUsageBytes: number;
  /** Cache hit rate */
  cacheHitRate: number;
}

/**
 * ProximaDB embedded database for Node.js
 *
 * @example
 * ```javascript
 * const { ProximaDB } = require('proximadb-embedded');
 *
 * // Create database
 * const db = new ProximaDB({ dataDir: './my_database' });
 *
 * // Create collection
 * db.createCollection('embeddings', 768);
 *
 * // Insert vectors
 * const ids = ['vec_0', 'vec_1'];
 * const vectors = [[0.1, 0.2, ...], [0.3, 0.4, ...]];
 * db.insert('embeddings', ids, vectors);
 *
 * // Search
 * const results = db.search('embeddings', [0.1, 0.2, ...], 10);
 * ```
 */
export class ProximaDB {
  /**
   * Create a new ProximaDB instance
   * @param config - Configuration options
   */
  constructor(config?: ProximaDBConfig);

  /**
   * Create a new collection
   * @param name - Collection name
   * @param dimension - Vector dimension
   * @param engine - Storage engine type (optional)
   */
  createCollection(name: string, dimension: number, engine?: string): void;

  /**
   * Delete a collection
   * @param name - Collection name
   */
  deleteCollection(name: string): void;

  /**
   * Get collection information
   * @param name - Collection name
   * @returns Collection info or null if not found
   */
  getCollection(name: string): CollectionInfo | null;

  /**
   * List all collections
   * @returns Array of collection info
   */
  listCollections(): CollectionInfo[];

  /**
   * Insert vectors into a collection
   * @param collection - Collection name
   * @param ids - Array of vector IDs
   * @param vectors - 2D array of vectors
   * @param metadata - Optional array of metadata objects
   * @returns Number of vectors inserted
   */
  insert(
    collection: string,
    ids: string[],
    vectors: number[][],
    metadata?: Record<string, string>[]
  ): number;

  /**
   * Search for similar vectors
   * @param collection - Collection name
   * @param query - Query vector
   * @param topK - Number of results (default: 10)
   * @param filter - Optional filter expression
   * @returns Array of search results
   */
  search(
    collection: string,
    query: number[],
    topK?: number,
    filter?: string
  ): SearchResult[];

  /**
   * Flush all pending writes to disk
   */
  flush(): void;

  /**
   * Get storage statistics
   * @returns Storage statistics
   */
  stats(): StorageStats;
}

/**
 * Get ProximaDB version
 */
export function version(): string;
