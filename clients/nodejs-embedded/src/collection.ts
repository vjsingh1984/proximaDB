/**
 * ProximaDB TypeScript SDK - Collection Management
 *
 * Provides a fluent API for collection CRUD operations and search.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import type {
  CollectionConfig,
  CollectionInfo,
  ProximaRecord,
  VectorRecord,
  SearchResult,
  JsonValue,
} from "./types";
import {
  DistanceMetric,
  StorageEngine,
  IndexType,
} from "./types";
import { SearchBuilder, SearchHttpClient } from "./search";

type RecordBatchResponse = {
  inserted_count?: number;
  success_count?: number;
  success?: number;
};

function recordPayload(record: {
  id: string;
  vector?: number[];
  metadata?: Record<string, JsonValue>;
  props?: Record<string, JsonValue>;
  source?: string;
}): ProximaRecord {
  return {
    id: record.id,
    vector: record.vector ?? [],
    props: record.props ?? record.metadata ?? {},
    source: record.source,
  };
}

function insertedCount(response: RecordBatchResponse): number {
  return response.success_count ?? response.inserted_count ?? response.success ?? 0;
}

/**
 * HTTP client interface for collection operations
 */
export interface CollectionHttpClient extends SearchHttpClient {
  get<T>(url: string): Promise<T>;
  delete<T>(url: string): Promise<T>;
}

// ============================================================================
// COLLECTION BUILDER
// ============================================================================

/**
 * Builder for creating collections
 */
export class CollectionBuilder {
  private client: CollectionHttpClient;
  private collectionName: string;
  private dimensionValue: number | null = null;
  private engineValue: StorageEngine = StorageEngine.Sst;
  private indexValue: IndexType = IndexType.Hnsw;
  private metricValue: DistanceMetric = DistanceMetric.Cosine;
  private descriptionValue: string | null = null;
  private tagsValue: string[] = [];

  constructor(client: CollectionHttpClient, name: string) {
    this.client = client;
    this.collectionName = name;
  }

  /**
   * Set the vector dimension (required)
   */
  dimension(dim: number): CollectionBuilder {
    this.dimensionValue = dim;
    return this;
  }

  /**
   * Set the storage engine
   */
  engine(eng: StorageEngine): CollectionBuilder {
    this.engineValue = eng;
    return this;
  }

  /**
   * Set the storage engine from string
   */
  engineStr(eng: string): CollectionBuilder {
    this.engineValue = eng as StorageEngine;
    return this;
  }

  /**
   * Set the index type
   */
  index(idx: IndexType): CollectionBuilder {
    this.indexValue = idx;
    return this;
  }

  /**
   * Set the distance metric
   */
  metric(met: DistanceMetric): CollectionBuilder {
    this.metricValue = met;
    return this;
  }

  /**
   * Set the collection description
   */
  description(desc: string): CollectionBuilder {
    this.descriptionValue = desc;
    return this;
  }

  /**
   * Add tags to the collection
   */
  tags(tagList: string[]): CollectionBuilder {
    this.tagsValue = tagList;
    return this;
  }

  /**
   * Execute the collection creation
   */
  async execute(): Promise<void> {
    if (this.dimensionValue === null) {
      throw new Error("Dimension is required for collection creation");
    }

    const request = {
      name: this.collectionName,
      dimension: this.dimensionValue,
      engine: this.engineValue,
      index_type: this.indexValue,
      distance_metric: this.metricValue,
      description: this.descriptionValue ?? undefined,
      tags: this.tagsValue.length > 0 ? this.tagsValue : undefined,
    };

    const url = this.client.url() + "/api/v1/collections";
    await this.client.post<{ success: boolean }>(url, request);
  }

  /**
   * Build the configuration without executing
   */
  build(): CollectionConfig {
    if (this.dimensionValue === null) {
      throw new Error("Dimension is required");
    }

    return {
      name: this.collectionName,
      dimension: this.dimensionValue,
      distanceMetric: this.metricValue,
      storageEngine: this.engineValue,
      indexType: this.indexValue,
      description: this.descriptionValue ?? undefined,
      tags: this.tagsValue.length > 0 ? this.tagsValue : undefined,
    };
  }
}

// ============================================================================
// INSERT BUILDER
// ============================================================================

/**
 * Builder for insert operations
 */
export class InsertBuilder {
  private client: CollectionHttpClient;
  private collectionName: string;
  private currentId: string | null = null;
  private currentVector: number[] | null = null;
  private currentMetadata: Record<string, JsonValue> = {};

  constructor(client: CollectionHttpClient, collection: string) {
    this.client = client;
    this.collectionName = collection;
  }

  /**
   * Set the vector ID
   */
  id(vectorId: string): InsertBuilder {
    this.currentId = vectorId;
    return this;
  }

  /**
   * Set the vector data
   */
  vector(vec: number[]): InsertBuilder {
    this.currentVector = vec;
    return this;
  }

  /**
   * Set metadata from object
   */
  metadata(meta: Record<string, JsonValue>): InsertBuilder {
    Object.assign(this.currentMetadata, meta);
    return this;
  }

  /**
   * Set a single metadata field
   */
  meta(key: string, value: JsonValue): InsertBuilder {
    this.currentMetadata[key] = value;
    return this;
  }

  /**
   * Execute single vector insert
   */
  async execute(): Promise<void> {
    if (this.currentId === null) {
      throw new Error("Vector ID is required");
    }
    if (this.currentVector === null) {
      throw new Error("Vector data is required");
    }

    const record = {
      id: this.currentId,
      vector: this.currentVector,
      props: this.currentMetadata,
    };

    const request = {
      records: [record],
      validate_schema: true,
    };

    const url = this.client.url() + `/api/v2/collections/${this.collectionName}/records/batch`;
    await this.client.post<RecordBatchResponse>(url, request);
  }
}

/**
 * Builder for batch insert operations
 */
export class BatchInsertBuilder {
  private client: CollectionHttpClient;
  private collectionName: string;
  private records: Array<{
    id: string;
    vector: number[];
    metadata: Record<string, JsonValue>;
  }> = [];

  constructor(
    client: CollectionHttpClient,
    collection: string,
    ids: string[],
    vectors: number[][]
  ) {
    this.client = client;
    this.collectionName = collection;

    if (ids.length !== vectors.length) {
      throw new Error("IDs and vectors arrays must have the same length");
    }

    for (let i = 0; i < ids.length; i++) {
      this.records.push({
        id: ids[i]!,
        vector: vectors[i]!,
        metadata: {},
      });
    }
  }

  /**
   * Add metadata for all vectors
   */
  withMetadata(metadataList: Array<Record<string, JsonValue>>): BatchInsertBuilder {
    if (metadataList.length !== this.records.length) {
      throw new Error("Metadata array must match the number of vectors");
    }

    for (let i = 0; i < metadataList.length; i++) {
      this.records[i]!.metadata = metadataList[i]!;
    }

    return this;
  }

  /**
   * Execute the batch insert
   */
  async execute(): Promise<number> {
    const request = {
      records: this.records.map(recordPayload),
      validate_schema: true,
    };

    const url = this.client.url() + `/api/v2/collections/${this.collectionName}/records/batch`;
    const response = await this.client.post<RecordBatchResponse>(url, request);
    return insertedCount(response);
  }
}

// ============================================================================
// UPDATE BUILDER
// ============================================================================

/**
 * Builder for update operations
 */
export class UpdateBuilder {
  private client: CollectionHttpClient;
  private collectionName: string;
  private vectorId: string;
  private newVector: number[] | null = null;
  private newMetadata: Record<string, JsonValue> = {};
  private replaceMetadataFlag: boolean = false;

  constructor(client: CollectionHttpClient, collection: string, id: string) {
    this.client = client;
    this.collectionName = collection;
    this.vectorId = id;
  }

  /**
   * Set a new vector
   */
  vector(vec: number[]): UpdateBuilder {
    this.newVector = vec;
    return this;
  }

  /**
   * Set metadata from object (merges with existing)
   */
  metadata(meta: Record<string, JsonValue>): UpdateBuilder {
    Object.assign(this.newMetadata, meta);
    return this;
  }

  /**
   * Set a single metadata field
   */
  meta(key: string, value: JsonValue): UpdateBuilder {
    this.newMetadata[key] = value;
    return this;
  }

  /**
   * Replace all metadata instead of merging
   */
  replaceMetadata(replace: boolean = true): UpdateBuilder {
    this.replaceMetadataFlag = replace;
    return this;
  }

  /**
   * Execute the update
   */
  async execute(): Promise<void> {
    const request = {
      collection: this.collectionName,
      id: this.vectorId,
      vector: this.newVector ?? undefined,
      metadata: Object.keys(this.newMetadata).length > 0 ? this.newMetadata : undefined,
      replace_metadata: this.replaceMetadataFlag,
    };

    const url = this.client.url() + "/api/v1/collections/" + this.collectionName + "/vectors/update";
    await this.client.post<{ success: boolean }>(url, request);
  }
}

// ============================================================================
// COLLECTION HANDLE
// ============================================================================

/**
 * Handle to a collection for fluent operations
 */
export class CollectionHandle {
  private client: CollectionHttpClient;
  private collectionName: string;

  constructor(client: CollectionHttpClient, name: string) {
    this.client = client;
    this.collectionName = name;
  }

  /**
   * Get the collection name
   */
  getName(): string {
    return this.collectionName;
  }

  /**
   * Start building a search query
   */
  search(): SearchBuilder {
    return new SearchBuilder(this.client, this.collectionName);
  }

  /**
   * Start building an insert operation
   */
  insert(): InsertBuilder {
    return new InsertBuilder(this.client, this.collectionName);
  }

  /**
   * Start building a batch insert operation
   */
  batch(ids: string[], vectors: number[][]): BatchInsertBuilder {
    return new BatchInsertBuilder(this.client, this.collectionName, ids, vectors);
  }

  /**
   * Start building an update operation for a vector
   */
  update(id: string): UpdateBuilder {
    return new UpdateBuilder(this.client, this.collectionName, id);
  }

  /**
   * Get collection information
   */
  async info(): Promise<CollectionInfo> {
    const url = this.client.url() + "/api/v1/collections/" + this.collectionName;
    return await this.client.get<CollectionInfo>(url);
  }

  /**
   * Get vector count
   */
  async count(): Promise<number> {
    const info = await this.info();
    return info.vectorCount ?? 0;
  }

  /**
   * Delete the collection
   */
  async delete(): Promise<void> {
    const url = this.client.url() + "/api/v1/collections/" + this.collectionName;
    await this.client.delete<unknown>(url);
  }

  /**
   * Get a vector by ID
   */
  async getVector(id: string): Promise<VectorRecord | null> {
    try {
      const url = this.client.url() + "/api/v1/collections/" + this.collectionName + "/vectors/" + id;
      return await this.client.get<VectorRecord>(url);
    } catch (e: unknown) {
      if (e instanceof Error && e.message.includes("404")) {
        return null;
      }
      throw e;
    }
  }

  /**
   * Check if a vector exists
   */
  async exists(id: string): Promise<boolean> {
    const vector = await this.getVector(id);
    return vector !== null;
  }

  /**
   * Delete a vector by ID
   */
  async deleteVector(id: string): Promise<void> {
    const url = this.client.url() + "/api/v1/collections/" + this.collectionName + "/vectors/" + id;
    await this.client.delete<unknown>(url);
  }

  /**
   * Delete multiple vectors by IDs
   */
  async deleteVectors(ids: string[]): Promise<number> {
    const request = {
      collection: this.collectionName,
      ids,
    };
    const url = this.client.url() + "/api/v1/collections/" + this.collectionName + "/vectors/delete";
    const response = await this.client.post<{ deleted_count: number }>(url, request);
    return response.deleted_count;
  }

  /**
   * Insert a single vector (convenience method)
   */
  async insertVector(
    id: string,
    vector: number[],
    metadata?: Record<string, JsonValue>
  ): Promise<void> {
    const builder = this.insert().id(id).vector(vector);
    if (metadata) {
      builder.metadata(metadata);
    }
    await builder.execute();
  }

  /**
   * Insert multiple vectors (convenience method)
   */
  async insertVectors(
    ids: string[],
    vectors: number[][],
    metadata?: Array<Record<string, JsonValue>>
  ): Promise<number> {
    const builder = this.batch(ids, vectors);
    if (metadata) {
      builder.withMetadata(metadata);
    }
    return await builder.execute();
  }

  /**
   * Search vectors (convenience method)
   */
  async searchVectors(
    queryVector: number[],
    topK: number = 10,
    filter?: string
  ): Promise<SearchResult[]> {
    const builder = this.search().vector(queryVector).topK(topK);
    if (filter) {
      builder.filter(filter);
    }
    return await builder.execute();
  }
}
