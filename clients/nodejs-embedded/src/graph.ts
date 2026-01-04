/**
 * ProximaDB TypeScript SDK - Graph Operations
 *
 * Provides a fluent API for graph database operations including
 * node and edge management, traversal queries, and graph analytics.
 *
 * Copyright 2025 ProximaDB Contributors
 * Licensed under the Apache License, Version 2.0
 */

import {
  GraphNode,
  GraphEdge,
  TraversalResult,
  TraversalDirection,
  JsonValue,
  GraphInfo,
} from "./types";

/**
 * HTTP client interface for graph operations
 */
export interface GraphHttpClient {
  get<T>(url: string): Promise<T>;
  post<T>(url: string, body: unknown): Promise<T>;
  delete<T>(url: string): Promise<T>;
  url(): string;
}

// ============================================================================
// NODE BUILDER
// ============================================================================

/**
 * Builder for creating graph nodes
 */
export class NodeBuilder {
  private handle: GraphHandle;
  private nodeId: string | null = null;
  private nodeLabel: string | null = null;
  private nodeProperties: Record<string, JsonValue> = {};
  private nodeVector: number[] | null = null;

  constructor(handle: GraphHandle) {
    this.handle = handle;
  }

  /**
   * Set the node ID
   */
  id(nodeId: string): NodeBuilder {
    this.nodeId = nodeId;
    return this;
  }

  /**
   * Set the node label (type)
   */
  label(nodeLabel: string): NodeBuilder {
    this.nodeLabel = nodeLabel;
    return this;
  }

  /**
   * Add a property to the node
   */
  property(key: string, value: JsonValue): NodeBuilder {
    this.nodeProperties[key] = value;
    return this;
  }

  /**
   * Add multiple properties at once
   */
  properties(props: Record<string, JsonValue>): NodeBuilder {
    Object.assign(this.nodeProperties, props);
    return this;
  }

  /**
   * Set the embedding vector for semantic operations
   */
  vector(vec: number[]): NodeBuilder {
    this.nodeVector = vec;
    return this;
  }

  /**
   * Execute the node addition
   */
  async execute(): Promise<void> {
    if (this.nodeId === null) {
      throw new Error("Node ID is required");
    }

    const node: GraphNode = {
      id: this.nodeId,
      label: this.nodeLabel ?? undefined,
      properties: this.nodeProperties,
      vector: this.nodeVector ?? undefined,
    };

    const request = {
      graph: this.handle.getName(),
      nodes: [node],
    };

    const url = this.handle.getClient().url() + "/api/v1/graphs/" + this.handle.getName() + "/nodes";
    await this.handle.getClient().post<{ added_count: number }>(url, request);
  }

  /**
   * Build the node without executing
   */
  build(): GraphNode {
    if (this.nodeId === null) {
      throw new Error("Node ID is required");
    }

    return {
      id: this.nodeId,
      label: this.nodeLabel ?? undefined,
      properties: this.nodeProperties,
      vector: this.nodeVector ?? undefined,
    };
  }
}

// ============================================================================
// EDGE BUILDER
// ============================================================================

/**
 * Builder for creating graph edges
 */
export class EdgeBuilder {
  private handle: GraphHandle;
  private sourceNode: string | null = null;
  private targetNode: string | null = null;
  private relationshipType: string | null = null;
  private edgeProperties: Record<string, JsonValue> = {};
  private edgeWeight: number | null = null;

  constructor(handle: GraphHandle) {
    this.handle = handle;
  }

  /**
   * Set the source node ID
   */
  from(nodeId: string): EdgeBuilder {
    this.sourceNode = nodeId;
    return this;
  }

  /**
   * Set the target node ID
   */
  to(nodeId: string): EdgeBuilder {
    this.targetNode = nodeId;
    return this;
  }

  /**
   * Set the relationship type
   */
  relationship(relType: string): EdgeBuilder {
    this.relationshipType = relType;
    return this;
  }

  /**
   * Add a property to the edge
   */
  property(key: string, value: JsonValue): EdgeBuilder {
    this.edgeProperties[key] = value;
    return this;
  }

  /**
   * Add multiple properties at once
   */
  properties(props: Record<string, JsonValue>): EdgeBuilder {
    Object.assign(this.edgeProperties, props);
    return this;
  }

  /**
   * Set the edge weight
   */
  weight(w: number): EdgeBuilder {
    this.edgeWeight = w;
    return this;
  }

  /**
   * Execute the edge addition
   */
  async execute(): Promise<void> {
    if (this.sourceNode === null) {
      throw new Error("Source node ID is required");
    }
    if (this.targetNode === null) {
      throw new Error("Target node ID is required");
    }
    if (this.relationshipType === null) {
      throw new Error("Relationship type is required");
    }

    const edge: GraphEdge = {
      source: this.sourceNode,
      target: this.targetNode,
      relationship: this.relationshipType,
      properties: this.edgeProperties,
      weight: this.edgeWeight ?? undefined,
    };

    const request = {
      graph: this.handle.getName(),
      edges: [edge],
    };

    const url = this.handle.getClient().url() + "/api/v1/graphs/" + this.handle.getName() + "/edges";
    await this.handle.getClient().post<{ added_count: number }>(url, request);
  }

  /**
   * Build the edge without executing
   */
  build(): GraphEdge {
    if (this.sourceNode === null) {
      throw new Error("Source node ID is required");
    }
    if (this.targetNode === null) {
      throw new Error("Target node ID is required");
    }
    if (this.relationshipType === null) {
      throw new Error("Relationship type is required");
    }

    return {
      source: this.sourceNode,
      target: this.targetNode,
      relationship: this.relationshipType,
      properties: this.edgeProperties,
      weight: this.edgeWeight ?? undefined,
    };
  }
}

// ============================================================================
// TRAVERSAL BUILDER
// ============================================================================

/**
 * Builder for graph traversal queries
 */
export class TraversalBuilder {
  private handle: GraphHandle;
  private startNodeId: string | null = null;
  private relationshipTypes: string[] = [];
  private traversalDirection: TraversalDirection = TraversalDirection.Outgoing;
  private maxDepthValue: number = 3;
  private limitValue: number = 100;
  private filterExpr: string | null = null;

  constructor(handle: GraphHandle) {
    this.handle = handle;
  }

  /**
   * Set the starting node
   */
  start(nodeId: string): TraversalBuilder {
    this.startNodeId = nodeId;
    return this;
  }

  /**
   * Add a relationship type to follow
   */
  relationship(relType: string): TraversalBuilder {
    this.relationshipTypes.push(relType);
    return this;
  }

  /**
   * Add multiple relationship types
   */
  relationships(relTypes: string[]): TraversalBuilder {
    this.relationshipTypes.push(...relTypes);
    return this;
  }

  /**
   * Set the traversal direction
   */
  direction(dir: TraversalDirection): TraversalBuilder {
    this.traversalDirection = dir;
    return this;
  }

  /**
   * Traverse outgoing edges only
   */
  outgoing(): TraversalBuilder {
    this.traversalDirection = TraversalDirection.Outgoing;
    return this;
  }

  /**
   * Traverse incoming edges only
   */
  incoming(): TraversalBuilder {
    this.traversalDirection = TraversalDirection.Incoming;
    return this;
  }

  /**
   * Traverse both directions
   */
  both(): TraversalBuilder {
    this.traversalDirection = TraversalDirection.Both;
    return this;
  }

  /**
   * Set the maximum traversal depth
   */
  maxDepth(depth: number): TraversalBuilder {
    this.maxDepthValue = depth;
    return this;
  }

  /**
   * Set the maximum number of results
   */
  limit(lim: number): TraversalBuilder {
    this.limitValue = lim;
    return this;
  }

  /**
   * Add a filter expression for nodes
   */
  filter(filterStr: string): TraversalBuilder {
    this.filterExpr = filterStr;
    return this;
  }

  /**
   * Execute the traversal
   */
  async execute(): Promise<TraversalResult> {
    if (this.startNodeId === null) {
      throw new Error("Start node is required for traversal");
    }

    const request = {
      graph: this.handle.getName(),
      start_node: this.startNodeId,
      relationships: this.relationshipTypes.length > 0 ? this.relationshipTypes : undefined,
      direction: this.traversalDirection,
      max_depth: this.maxDepthValue,
      limit: this.limitValue,
      filter: this.filterExpr ?? undefined,
    };

    const url = this.handle.getClient().url() + "/api/v1/graphs/" + this.handle.getName() + "/traverse";
    return await this.handle.getClient().post<TraversalResult>(url, request);
  }
}

// ============================================================================
// GRAPH HANDLE
// ============================================================================

/**
 * Handle to a graph for fluent operations
 */
export class GraphHandle {
  private client: GraphHttpClient;
  private graphName: string;

  constructor(client: GraphHttpClient, name: string) {
    this.client = client;
    this.graphName = name;
  }

  /**
   * Get the graph name
   */
  getName(): string {
    return this.graphName;
  }

  /**
   * Get the HTTP client (internal use)
   */
  getClient(): GraphHttpClient {
    return this.client;
  }

  /**
   * Start building a node addition
   */
  addNode(): NodeBuilder {
    return new NodeBuilder(this);
  }

  /**
   * Start building an edge addition
   */
  addEdge(): EdgeBuilder {
    return new EdgeBuilder(this);
  }

  /**
   * Start building a traversal query
   */
  traverse(): TraversalBuilder {
    return new TraversalBuilder(this);
  }

  /**
   * Add a batch of nodes
   */
  async addNodes(nodes: GraphNode[]): Promise<number> {
    const request = {
      graph: this.graphName,
      nodes,
    };
    const url = this.client.url() + "/api/v1/graphs/" + this.graphName + "/nodes";
    const response = await this.client.post<{ added_count: number }>(url, request);
    return response.added_count;
  }

  /**
   * Add a batch of edges
   */
  async addEdges(edges: GraphEdge[]): Promise<number> {
    const request = {
      graph: this.graphName,
      edges,
    };
    const url = this.client.url() + "/api/v1/graphs/" + this.graphName + "/edges";
    const response = await this.client.post<{ added_count: number }>(url, request);
    return response.added_count;
  }

  /**
   * Get a node by ID
   */
  async getNode(nodeId: string): Promise<GraphNode | null> {
    try {
      const url = this.client.url() + "/api/v1/graphs/" + this.graphName + "/nodes/" + nodeId;
      return await this.client.get<GraphNode>(url);
    } catch (e: unknown) {
      if (e instanceof Error && e.message.includes("404")) {
        return null;
      }
      throw e;
    }
  }

  /**
   * Delete a node by ID
   */
  async deleteNode(nodeId: string): Promise<void> {
    const url = this.client.url() + "/api/v1/graphs/" + this.graphName + "/nodes/" + nodeId;
    await this.client.delete<unknown>(url);
  }

  /**
   * Delete an edge
   */
  async deleteEdge(source: string, target: string, relationship: string): Promise<void> {
    const url = this.client.url() + "/api/v1/graphs/" + this.graphName + "/edges/" + source + "/" + target + "/" + relationship;
    await this.client.delete<unknown>(url);
  }

  /**
   * Get graph statistics
   */
  async info(): Promise<GraphInfo> {
    const url = this.client.url() + "/api/v1/graphs/" + this.graphName;
    return await this.client.get<GraphInfo>(url);
  }
}

// ============================================================================
// GRAPH BUILDER
// ============================================================================

/**
 * Builder for creating graphs
 */
export class GraphBuilder {
  private client: GraphHttpClient;
  private graphName: string;
  private graphDescription: string | null = null;

  constructor(client: GraphHttpClient, name: string) {
    this.client = client;
    this.graphName = name;
  }

  /**
   * Set the graph description
   */
  description(desc: string): GraphBuilder {
    this.graphDescription = desc;
    return this;
  }

  /**
   * Execute the graph creation
   */
  async execute(): Promise<void> {
    const request = {
      name: this.graphName,
      description: this.graphDescription ?? undefined,
    };

    const url = this.client.url() + "/api/v1/graphs";
    await this.client.post<{ success: boolean }>(url, request);
  }
}

// ============================================================================
// CONVENIENCE FUNCTIONS
// ============================================================================

/**
 * Create a new GraphNode
 */
export function createNode(id: string): GraphNode {
  return {
    id,
    properties: {},
  };
}

/**
 * Create a new GraphNode with builder pattern
 */
export function node(id: string): {
  label: (l: string) => ReturnType<typeof node>;
  property: (k: string, v: JsonValue) => ReturnType<typeof node>;
  vector: (v: number[]) => ReturnType<typeof node>;
  build: () => GraphNode;
} {
  const n: GraphNode = { id, properties: {} };
  return {
    label: (l: string) => { n.label = l; return node(id); },
    property: (k: string, v: JsonValue) => { n.properties[k] = v; return node(id); },
    vector: (v: number[]) => { n.vector = v; return node(id); },
    build: () => n,
  };
}

/**
 * Create a new GraphEdge
 */
export function createEdge(source: string, target: string, relationship: string): GraphEdge {
  return {
    source,
    target,
    relationship,
    properties: {},
  };
}

/**
 * Create a new GraphEdge with builder pattern
 */
export function edge(source: string, target: string, relationship: string): {
  property: (k: string, v: JsonValue) => ReturnType<typeof edge>;
  weight: (w: number) => ReturnType<typeof edge>;
  build: () => GraphEdge;
} {
  const e: GraphEdge = { source, target, relationship, properties: {} };
  return {
    property: (k: string, v: JsonValue) => { e.properties[k] = v; return edge(source, target, relationship); },
    weight: (w: number) => { e.weight = w; return edge(source, target, relationship); },
    build: () => e,
  };
}
