/**
 * ProximaDB TypeScript SDK - Client
 *
 * Main entry point for connecting to ProximaDB servers.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import {
  ClientConfig,
  HealthStatus,
  CollectionInfo,
  GraphInfo,
  ProximaDBError,
  ErrorCode,
  ProbeResponse,
  SchemaResponse,
  UpdateSchemaRequest,
  UpdateSchemaResponse,
  QueryRequest,
  ExplainQueryRequest,
  QueryResponse,
} from "./types";
import { CollectionBuilder, CollectionHandle, CollectionHttpClient } from "./collection";
import { GraphBuilder, GraphHandle, GraphHttpClient } from "./graph";

/**
 * Default client configuration
 */
const DEFAULT_CONFIG: Required<ClientConfig> = {
  url: "http://localhost:5678",
  timeoutMs: 30000,
  maxRetries: 3,
  apiKey: "",
  poolConnections: true,
  maxIdleConnections: 10,
};

/**
 * HTTP response interface
 */
interface HttpResponse {
  ok: boolean;
  status: number;
  statusText: string;
  json(): Promise<unknown>;
  text(): Promise<string>;
}

/**
 * Minimal fetch-like function signature
 */
type FetchFunction = (url: string, init?: RequestInit) => Promise<HttpResponse>;

/**
 * Get the global fetch function
 */
function getFetch(): FetchFunction {
  if (typeof globalThis.fetch === "function") {
    return globalThis.fetch.bind(globalThis) as FetchFunction;
  }
  throw new Error(
    "No fetch implementation available. " +
    "Node.js 18+ includes fetch natively. " +
    "For older versions, install node-fetch."
  );
}

/**
 * ProximaDB client for connecting to a remote server
 */
export class ProximaDBClient implements CollectionHttpClient, GraphHttpClient {
  private config: Required<ClientConfig>;
  private fetchFn: FetchFunction;

  /**
   * Create a new client with the given URL
   */
  static connect(url: string): ProximaDBClient {
    return new ProximaDBClient({ url });
  }

  /**
   * Create a new client builder
   */
  static builder(): ClientBuilder {
    return new ClientBuilder();
  }

  /**
   * Create a client with configuration
   */
  constructor(config: Partial<ClientConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config };
    this.fetchFn = getFetch();

    // Validate URL
    try {
      new URL(this.config.url);
    } catch {
      throw new ProximaDBError(
        "Invalid URL: " + this.config.url,
        ErrorCode.InvalidConfig
      );
    }
  }

  /**
   * Get the server URL
   */
  url(): string {
    return this.config.url;
  }

  /**
   * Get the client configuration
   */
  getConfig(): ClientConfig {
    return { ...this.config };
  }

  // =========================================================================
  // HTTP Methods
  // =========================================================================

  /**
   * Make a GET request
   */
  async get<T>(requestUrl: string): Promise<T> {
    return this.request<T>("GET", requestUrl);
  }

  /**
   * Make a POST request
   */
  async post<T>(requestUrl: string, body: unknown): Promise<T> {
    return this.request<T>("POST", requestUrl, body);
  }

  /**
   * Make a PUT request
   */
  async put<T>(requestUrl: string, body: unknown): Promise<T> {
    return this.request<T>("PUT", requestUrl, body);
  }

  /**
   * Make a DELETE request
   */
  async delete<T>(requestUrl: string): Promise<T> {
    return this.request<T>("DELETE", requestUrl);
  }

  /**
   * Make an HTTP request with retry logic
   */
  private async request<T>(
    method: string,
    requestUrl: string,
    body?: unknown
  ): Promise<T> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      Accept: "application/json",
    };

    if (this.config.apiKey) {
      headers["Authorization"] = "Bearer " + this.config.apiKey;
    }

    const init: RequestInit = {
      method,
      headers,
    };

    if (body !== undefined) {
      init.body = JSON.stringify(body);
    }

    let lastError: Error | null = null;
    for (let attempt = 0; attempt <= this.config.maxRetries; attempt++) {
      try {
        const response = await this.fetchFn(requestUrl, init);
        return await this.handleResponse<T>(response);
      } catch (e: unknown) {
        lastError = e instanceof Error ? e : new Error(String(e));

        // Don't retry on client errors (4xx)
        if (lastError instanceof ProximaDBError) {
          if (lastError.statusCode && lastError.statusCode >= 400 && lastError.statusCode < 500) {
            throw lastError;
          }
        }

        // Wait before retry with exponential backoff
        if (attempt < this.config.maxRetries) {
          const delay = Math.min(1000 * Math.pow(2, attempt), 10000);
          await new Promise((resolve) => setTimeout(resolve, delay));
        }
      }
    }

    throw lastError ?? new Error("Request failed after retries");
  }

  /**
   * Handle HTTP response
   */
  private async handleResponse<T>(response: HttpResponse): Promise<T> {
    if (response.ok) {
      try {
        return (await response.json()) as T;
      } catch {
        throw new ProximaDBError(
          "Failed to parse response JSON",
          ErrorCode.ServerError,
          response.status
        );
      }
    }

    const statusCode = response.status;
    let message = response.statusText;

    try {
      message = await response.text();
    } catch {
      // Use statusText if text() fails
    }

    switch (statusCode) {
      case 401:
        throw new ProximaDBError(message, ErrorCode.AuthenticationFailed, statusCode);
      case 403:
        throw new ProximaDBError(message, ErrorCode.AuthorizationDenied, statusCode);
      case 404:
        throw new ProximaDBError(message, ErrorCode.CollectionNotFound, statusCode);
      case 409:
        throw new ProximaDBError(message, ErrorCode.CollectionExists, statusCode);
      case 429:
        throw new ProximaDBError(message, ErrorCode.RateLimited, statusCode);
      default:
        throw new ProximaDBError(message, ErrorCode.ServerError, statusCode);
    }
  }

  // =========================================================================
  // Collection Operations
  // =========================================================================

  /**
   * Get a handle to a collection for fluent operations
   */
  collection(name: string): CollectionHandle {
    return new CollectionHandle(this, name);
  }

  /**
   * Create a collection builder
   */
  createCollection(name: string): CollectionBuilder {
    return new CollectionBuilder(this, name);
  }

  /**
   * Delete a collection
   */
  async deleteCollection(name: string): Promise<void> {
    const requestUrl = this.config.url + "/api/v2/collections/" + name;
    await this.delete<unknown>(requestUrl);
  }

  /**
   * List all collections
   */
  async listCollections(): Promise<CollectionInfo[]> {
    const requestUrl = this.config.url + "/api/v2/collections";
    const response = await this.get<{ collections: CollectionInfo[] }>(requestUrl);
    return response.collections;
  }

  /**
   * Get the schema for a collection
   *
   * Wire endpoint: GET /api/v2/collections/{collection_id}/schema
   * OpenAPI operationId: getCollectionSchema
   */
  async getCollectionSchema(collectionId: string): Promise<SchemaResponse> {
    const requestUrl =
      this.config.url + "/api/v2/collections/" + collectionId + "/schema";
    return await this.get<SchemaResponse>(requestUrl);
  }

  /**
   * Update the schema for a collection
   *
   * Wire endpoint: PUT /api/v2/collections/{collection_id}/schema
   * OpenAPI operationId: updateCollectionSchema
   */
  async updateCollectionSchema(
    collectionId: string,
    schema: UpdateSchemaRequest
  ): Promise<UpdateSchemaResponse> {
    const requestUrl =
      this.config.url + "/api/v2/collections/" + collectionId + "/schema";
    return await this.put<UpdateSchemaResponse>(requestUrl, schema);
  }

  // =========================================================================
  // Query Facade (AQL / UQL)
  // =========================================================================

  /**
   * Execute an AQL or UQL query through the shared query facade
   *
   * Wire endpoint: POST /api/v2/query
   * OpenAPI operationId: executeQuery
   */
  async executeQuery(req: QueryRequest): Promise<QueryResponse> {
    const requestUrl = this.config.url + "/api/v2/query";
    return await this.post<QueryResponse>(requestUrl, req);
  }

  /**
   * Explain an AQL or UQL query through the shared query facade
   *
   * Wire endpoint: POST /api/v2/query/explain
   * OpenAPI operationId: explainQuery
   */
  async explainQuery(req: ExplainQueryRequest): Promise<QueryResponse> {
    const requestUrl = this.config.url + "/api/v2/query/explain";
    return await this.post<QueryResponse>(requestUrl, req);
  }

  // =========================================================================
  // Graph Operations
  // =========================================================================

  /**
   * Get a handle to a graph for fluent operations
   */
  graph(name: string): GraphHandle {
    return new GraphHandle(this, name);
  }

  /**
   * Create a graph builder
   */
  createGraph(name: string): GraphBuilder {
    return new GraphBuilder(this, name);
  }

  /**
   * Delete a graph
   *
   * Wire endpoint: DELETE /api/v2/graphs/{graph_id}
   * OpenAPI operationId: deleteGraph
   */
  async deleteGraph(name: string): Promise<void> {
    const requestUrl = this.config.url + "/api/v2/graphs/" + name;
    await this.delete<unknown>(requestUrl);
  }

  /**
   * List all graphs
   *
   * Wire endpoint: GET /api/v2/graphs
   * OpenAPI operationId: listGraphs
   */
  async listGraphs(): Promise<GraphInfo[]> {
    const requestUrl = this.config.url + "/api/v2/graphs";
    const response = await this.get<{ graphs: GraphInfo[] }>(requestUrl);
    return response.graphs;
  }

  // =========================================================================
  // Health and Monitoring
  // =========================================================================

  /**
   * Check if the server is healthy
   */
  async health(): Promise<HealthStatus> {
    const requestUrl = this.config.url + "/health";
    return await this.get<HealthStatus>(requestUrl);
  }

  /**
   * Kubernetes-style liveness probe
   *
   * Wire endpoint: GET /health/live
   * OpenAPI operationId: getLiveness
   */
  async healthLive(): Promise<ProbeResponse> {
    const requestUrl = this.config.url + "/health/live";
    return await this.get<ProbeResponse>(requestUrl);
  }

  /**
   * Kubernetes-style readiness probe
   *
   * Wire endpoint: GET /health/ready
   * OpenAPI operationId: getReadiness
   */
  async healthReady(): Promise<ProbeResponse> {
    const requestUrl = this.config.url + "/health/ready";
    return await this.get<ProbeResponse>(requestUrl);
  }

  /**
   * Check if the server is reachable
   */
  async ping(): Promise<boolean> {
    try {
      await this.health();
      return true;
    } catch {
      return false;
    }
  }
}

// ============================================================================
// CLIENT BUILDER
// ============================================================================

/**
 * Builder for creating a ProximaDB client with custom configuration
 */
export class ClientBuilder {
  private config: Partial<ClientConfig> = {};

  /**
   * Set the server URL
   */
  url(serverUrl: string): ClientBuilder {
    this.config.url = serverUrl;
    return this;
  }

  /**
   * Set the request timeout in milliseconds
   */
  timeoutMs(timeout: number): ClientBuilder {
    this.config.timeoutMs = timeout;
    return this;
  }

  /**
   * Set the maximum number of retries
   */
  maxRetries(retries: number): ClientBuilder {
    this.config.maxRetries = retries;
    return this;
  }

  /**
   * Set the API key for authentication
   */
  apiKey(key: string): ClientBuilder {
    this.config.apiKey = key;
    return this;
  }

  /**
   * Enable or disable connection pooling
   */
  poolConnections(enable: boolean): ClientBuilder {
    this.config.poolConnections = enable;
    return this;
  }

  /**
   * Set maximum idle connections in pool
   */
  maxIdleConnections(max: number): ClientBuilder {
    this.config.maxIdleConnections = max;
    return this;
  }

  /**
   * Build the ProximaDB client
   */
  build(): ProximaDBClient {
    return new ProximaDBClient(this.config);
  }

  /**
   * Connect to the server (alias for build)
   */
  connect(): ProximaDBClient {
    return this.build();
  }
}

// ============================================================================
// CONVENIENCE FUNCTIONS
// ============================================================================

/**
 * Create a ProximaDB client connected to the given URL
 */
export function connect(url: string = "http://localhost:5678"): ProximaDBClient {
  return ProximaDBClient.connect(url);
}

/**
 * Create a ProximaDB client with REST protocol
 */
export function connectRest(url: string = "http://localhost:5678"): ProximaDBClient {
  return ProximaDBClient.connect(url);
}
