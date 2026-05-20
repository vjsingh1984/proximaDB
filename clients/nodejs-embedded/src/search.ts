/**
 * ProximaDB TypeScript SDK - Search Builder
 *
 * Provides a fluent API for building and executing vector similarity searches.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import type {
  Filter,
  SearchResult,
  JsonValue,
  SearchResultIterator,
} from "./types";
import { SearchMode } from "./types";
import { FilterBuilder, filterToExpression } from "./filter";

/**
 * Internal search request structure
 */
interface SearchRequest {
  vector: number[];
  top_k: number;
  filter?: string;
  searchMode?: string;
  include_vector: boolean;
  include_text: boolean;
  timeoutMs?: number;
}

/**
 * HTTP client interface for search operations
 */
export interface SearchHttpClient {
  post<T>(url: string, body: unknown): Promise<T>;
  url(): string;
}

/**
 * Builder for search queries with fluent API
 */
export class SearchBuilder {
  private client: SearchHttpClient;
  private collectionName: string;
  private queryVector: number[] | null = null;
  private k: number = 10;
  private filterExpr: string | null = null;
  private filterBuilder: FilterBuilder | null = null;
  private searchMode: SearchMode = SearchMode.Exact;
  private nprobeValue: number | null = null;
  private adaptiveThreshold: number | null = null;
  private includeVectorsFlag: boolean = false;
  private minScoreValue: number | null = null;
  private timeoutMsValue: number | null = null;

  constructor(client: SearchHttpClient, collection: string) {
    this.client = client;
    this.collectionName = collection;
  }

  /**
   * Set the query vector
   */
  vector(vec: number[]): SearchBuilder {
    this.queryVector = vec;
    return this;
  }

  /**
   * Set the number of results to return
   */
  topK(k: number): SearchBuilder {
    this.k = k;
    return this;
  }

  /**
   * Alias for topK
   */
  limit(k: number): SearchBuilder {
    return this.topK(k);
  }

  /**
   * Set a filter expression string
   */
  filter(filterStr: string): SearchBuilder {
    this.filterExpr = filterStr;
    return this;
  }

  /**
   * Apply a pre-built filter
   */
  withFilter(filterObj: Filter): SearchBuilder {
    this.filterExpr = filterToExpression(filterObj);
    return this;
  }

  /**
   * Set the search mode
   */
  mode(searchMode: SearchMode): SearchBuilder {
    this.searchMode = searchMode;
    return this;
  }

  /**
   * Use exact search (100% recall)
   */
  exact(): SearchBuilder {
    this.searchMode = SearchMode.Exact;
    return this;
  }

  /**
   * Use approximate search for faster results
   */
  approximate(): SearchBuilder {
    this.searchMode = SearchMode.Approximate;
    return this;
  }

  /**
   * Use approximate search with specific nprobe value
   */
  approximateWithNprobe(nprobe: number): SearchBuilder {
    this.searchMode = SearchMode.Approximate;
    this.nprobeValue = nprobe;
    return this;
  }

  /**
   * Use adaptive search mode
   */
  adaptive(threshold: number): SearchBuilder {
    this.searchMode = SearchMode.Adaptive;
    this.adaptiveThreshold = threshold;
    return this;
  }

  /**
   * Include vectors in results
   */
  includeVectors(include: boolean = true): SearchBuilder {
    this.includeVectorsFlag = include;
    return this;
  }

  /**
   * Include metadata in results
   */
  includeMetadata(_include: boolean = true): SearchBuilder {
    return this;
  }

  /**
   * Set minimum score threshold
   */
  minScore(score: number): SearchBuilder {
    this.minScoreValue = score;
    return this;
  }

  /**
   * Set request timeout in milliseconds
   */
  timeoutMs(timeout: number): SearchBuilder {
    this.timeoutMsValue = timeout;
    return this;
  }

  /**
   * Set timeout in seconds (convenience method)
   */
  timeoutSecs(secs: number): SearchBuilder {
    this.timeoutMsValue = secs * 1000;
    return this;
  }

  // =========================================================================
  // Inline filter methods for convenience
  // =========================================================================

  /**
   * Add an equality filter condition
   */
  filterEq(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.eq(field, value);
    return this;
  }

  /**
   * Add a not-equal filter condition
   */
  filterNe(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.ne(field, value);
    return this;
  }

  /**
   * Add a greater-than filter condition
   */
  filterGt(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.gt(field, value);
    return this;
  }

  /**
   * Add a greater-than-or-equal filter condition
   */
  filterGte(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.gte(field, value);
    return this;
  }

  /**
   * Add a less-than filter condition
   */
  filterLt(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.lt(field, value);
    return this;
  }

  /**
   * Add a less-than-or-equal filter condition
   */
  filterLte(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.lte(field, value);
    return this;
  }

  /**
   * Add a range filter condition (inclusive)
   */
  filterRange(field: string, min: JsonValue, max: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.range(field, min, max);
    return this;
  }

  /**
   * Add an IN filter condition
   */
  filterIn(field: string, values: JsonValue[]): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.inList(field, values);
    return this;
  }

  /**
   * Add a contains filter condition
   */
  filterContains(field: string, value: JsonValue): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.contains(field, value);
    return this;
  }

  /**
   * Add an exists filter condition
   */
  filterExists(field: string): SearchBuilder {
    this.ensureFilterBuilder();
    this.filterBuilder = this.filterBuilder!.exists(field);
    return this;
  }

  private ensureFilterBuilder(): void {
    if (this.filterBuilder === null) {
      this.filterBuilder = FilterBuilder.new();
    }
  }

  private buildFilter(): string | undefined {
    if (this.filterBuilder !== null) {
      return this.filterBuilder.toExpression();
    }
    return this.filterExpr ?? undefined;
  }

  private buildSearchModeString(): string {
    switch (this.searchMode) {
      case SearchMode.Exact:
        return "exact";
      case SearchMode.Approximate:
        if (this.nprobeValue !== null) {
          return "approximate:" + this.nprobeValue;
        }
        return "approximate";
      case SearchMode.Adaptive:
        if (this.adaptiveThreshold !== null) {
          return "adaptive:" + this.adaptiveThreshold;
        }
        return "adaptive:10000";
      default:
        return "exact";
    }
  }

  /**
   * Execute the search query
   */
  async execute(): Promise<SearchResult[]> {
    if (this.queryVector === null) {
      throw new Error("Query vector is required");
    }

    if (this.k <= 0 || this.k > 10000) {
      throw new Error("topK must be between 1 and 10000");
    }

    const request: SearchRequest = {
      vector: this.queryVector,
      top_k: this.k,
      filter: this.buildFilter(),
      searchMode: this.buildSearchModeString(),
      include_vector: this.includeVectorsFlag,
      include_text: false,
      timeoutMs: this.timeoutMsValue ?? undefined,
    };

    const url = this.client.url() + `/api/v2/collections/${this.collectionName}/search`;
    const response = await this.client.post<{ results: SearchResult[] }>(url, request);
    
    let results = response.results;

    // Apply min_score filter if set
    if (this.minScoreValue !== null) {
      results = results.filter((r) => r.score >= this.minScoreValue!);
    }

    return results;
  }

  /**
   * Execute the search and return a streaming iterator
   */
  async *stream(): AsyncGenerator<SearchResult, void, unknown> {
    const results = await this.execute();
    for (const result of results) {
      yield result;
    }
  }

  /**
   * Create an async iterator for streaming results
   */
  async iterate(): Promise<SearchResultIterator> {
    const results = await this.execute();
    let index = 0;
    const total = results.length;

    const iterator: SearchResultIterator = {
      [Symbol.asyncIterator](): AsyncIterableIterator<SearchResult> {
        return iterator;
      },
      async next(): Promise<IteratorResult<SearchResult>> {
        if (index < results.length) {
          return { value: results[index++]!, done: false };
        }
        return { value: undefined as unknown as SearchResult, done: true };
      },
      getTotal(): number {
        return total;
      },
      isComplete(): boolean {
        return index >= results.length;
      },
    };

    return iterator;
  }
}

/**
 * Create a new search builder
 */
export function createSearchBuilder(
  client: SearchHttpClient,
  collection: string
): SearchBuilder {
  return new SearchBuilder(client, collection);
}
